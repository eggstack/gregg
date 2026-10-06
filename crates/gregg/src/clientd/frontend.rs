//! Plan 164: the TUI's side of the local client-daemon channel.
//!
//! A frontend owns no network state at all. It dials the config-specific
//! endpoint, completes a versioned handshake, and from then on receives
//! complete state documents and sends three kinds of request: a reload, an
//! `EggPool` intent, and a stop.
//!
//! # One owner per socket
//!
//! The socket has exactly one owner, the [`FrameStream`]. [`FrontendSender`]
//! is a bounded channel into it. Splitting the socket into separate read and
//! write handles would need platform-specific duplication that the Windows
//! named-pipe API does not offer, and it would introduce a second owner whose
//! failure mode is a half-written frame nobody can recover.
//!
//! # No fallback, by design
//!
//! There is deliberately no path here that resumes direct remote polling when
//! the daemon cannot be reached. If the daemon is absent or broken, the TUI
//! says so and exits. A fallback would double the fleet's request budget
//! whenever the daemon was unhealthy — exactly when the operator most needs
//! the truth about what is reachable — and it would make daemon failure
//! invisible, which is the opposite of what observability is for.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::clientd::identity::ClientDaemonIdentity;
use crate::clientd::ipc::{self, Connection, TransportError};
use crate::clientd::protocol::{
    encode_frame, DaemonRequest, DecodeError, FrontendFrame, LEN_PREFIX_BYTES, PROTOCOL_VERSION,
};
use crate::clientd::snapshot::FrontendSnapshot;
use crate::eggpool::EggpoolPeriod;

/// Capacity of the per-frontend outbound request channel.
///
/// Requests are rare and idempotent-by-reduction. A full channel means the
/// frame reader is not draining, and the newest pending request supersedes the
/// older ones anyway, so the sender drops rather than blocks.
const REQUEST_CHANNEL_CAPACITY: usize = 8;

/// Read buffer for the frontend's connection.
const READ_BUFFER_BYTES: usize = 8192;

/// How long a frontend waits for a terminal frame before giving up.
///
/// Only terminal frames are awaited. A stop request must not block forever
/// behind a daemon that died without replying.
pub const TERMINAL_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// A connected frontend, before it has been split into a sender and a reader.
pub struct FrontendLink {
    connection: Connection,
}

impl FrontendLink {
    /// Dial the first candidate endpoint that answers.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] when no candidate path exists or is
    /// connectable. A connect failure is the only failure Plan 165
    /// classifies as "absent"; everything after this point is a live endpoint
    /// with something wrong with it.
    pub fn connect(candidates: &[PathBuf]) -> Result<Self, TransportError> {
        Ok(Self {
            connection: ipc::connect(candidates)?,
        })
    }

    /// Send the versioned handshake, identifying the config we expect.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] when the frame cannot be written.
    pub fn handshake(
        &mut self,
        identity: &ClientDaemonIdentity,
        version: &str,
    ) -> Result<(), TransportError> {
        self.connection.write_frame(&DaemonRequest::Handshake {
            protocol_version: PROTOCOL_VERSION,
            version: version.to_owned(),
            daemon_id: identity.id().to_owned(),
        })
    }

    /// Write one request frame immediately.
    ///
    /// Used by short-lived callers such as the CLI's post-mutation
    /// notification, which has no event loop to pump a reader. A TUI uses
    /// [`Self::split`] instead, because its writes have to interleave with
    /// incoming state documents.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] when the frame cannot be written.
    pub fn send(&mut self, request: &DaemonRequest) -> Result<(), TransportError> {
        self.connection.write_frame(request)
    }

    /// Split into a request sender and a frame stream.
    ///
    /// The reader is the only task that touches the socket. Requests are
    /// delivered to it through the channel so it can interleave writes with
    /// reads, which is what keeps `Ctrl-R` responsive while state documents
    /// are streaming in.
    ///
    /// The candidate path this link actually reached.
    ///
    /// Not necessarily the first candidate: a daemon binds a fallback when the
    /// config-adjacent location is unusable, and a diagnostic that names the
    /// primary would then point at a file that does not exist.
    #[must_use]
    pub fn endpoint(&self) -> Option<&Path> {
        self.connection.endpoint()
    }

    pub fn split(self) -> (FrontendSender, FrameStream) {
        let (request_tx, request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
        (
            FrontendSender {
                requests: request_tx,
            },
            FrameStream {
                connection: self.connection,
                requests: request_rx,
            },
        )
    }
}

/// The write side of a frontend connection.
pub struct FrontendSender {
    requests: mpsc::Sender<DaemonRequest>,
}

impl FrontendSender {
    /// Ask the daemon to re-read its configuration file.
    ///
    /// This is `Ctrl-R`, and the plan's only config-reload boundary. There is
    /// no filesystem watcher, so nothing else can produce a reload.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the reader is not draining.
    /// The operator's most recent request is the one that matters and it is
    /// the one that gets through on the next read.
    pub fn request_reload(&self, generation: u64) -> Result<(), FrontError> {
        self.requests
            .try_send(DaemonRequest::ReloadConfig { generation })
            .map_err(|_| FrontError::ChannelFull)
    }

    /// Ask the daemon to stop.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the reader is not draining.
    pub fn request_shutdown(&self, generation: u64) -> Result<(), FrontError> {
        self.requests
            .try_send(DaemonRequest::Shutdown { generation })
            .map_err(|_| FrontError::ChannelFull)
    }

    /// Report what this frontend currently wants from the `EggPool` worker.
    ///
    /// Replaces the previous intent wholesale, so a lost update cannot compose
    /// with a newer one into a state nobody asked for. The daemon reduces
    /// every attached frontend's intent into one converged worker state, so
    /// two windows cannot race the worker into two activations.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the reader is not draining.
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn request_eggpool_intent(
        &self,
        active: bool,
        period: EggpoolPeriod,
        refresh: bool,
        generation: u64,
    ) -> Result<(), FrontError> {
        self.requests
            .try_send(DaemonRequest::SetEggpoolIntent {
                active,
                period,
                refresh,
                generation,
            })
            .map_err(|_| FrontError::ChannelFull)
    }

    /// Report what this frontend currently has open in the cron detail view.
    ///
    /// Replaces the previous intent wholesale, so closing the pane cannot leave
    /// the daemon transmitting records for a view nobody is looking at.
    ///
    /// This does not add a remote request. The scheduler plane is polled by the
    /// daemon on its own cadence; the intent only decides which retained records
    /// are published.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the reader is not draining.
    pub fn request_cron_intent(
        &self,
        system_id: Option<&str>,
        job: Option<&str>,
        display_history: usize,
        generation: u64,
    ) -> Result<(), FrontError> {
        self.requests
            .try_send(DaemonRequest::SetCronIntent {
                system_id: system_id.map(str::to_owned),
                job: job.map(str::to_owned),
                display_history,
                generation,
            })
            .map_err(|_| FrontError::ChannelFull)
    }
}

/// One frontend's declared wish for the `EggPool` worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EggpoolIntentRequest {
    /// Whether this frontend has the `EggPool` pane open.
    pub active: bool,
    /// The rolling window it is displaying.
    pub period: EggpoolPeriod,
    /// Force a new worker request even if nothing else changed.
    pub refresh: bool,
}

/// Everything the TUI's event loop needs from its connection.
///
/// The event loop does not care that requests travel over a Unix socket or a
/// named pipe, and it must not care: a `Ctrl-R` and a pane change are the same
/// action whichever transport carries them. This trait is the seam that keeps
/// the event loop testable without a live socket, and it is deliberately
/// narrow — four requests, no general RPC.
pub trait ControlSink {
    /// `Ctrl-R`: re-read the configuration file.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the request could not be
    /// queued. The newest request supersedes older ones, so a dropped one is
    /// recovered by the operator pressing the key again.
    fn request_reload(&self, generation: u64) -> Result<(), FrontError>;

    /// Report this frontend's `EggPool` intent.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the request could not be
    /// queued.
    fn request_eggpool_intent(
        &self,
        intent: &EggpoolIntentRequest,
        generation: u64,
    ) -> Result<(), FrontError>;

    /// Report what this frontend currently has open in the cron detail view.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::ChannelFull`] when the request could not be
    /// queued.
    fn request_cron_intent(
        &self,
        system_id: Option<&str>,
        job: Option<&str>,
        display_history: usize,
        generation: u64,
    ) -> Result<(), FrontError>;
}

impl ControlSink for FrontendSender {
    fn request_reload(&self, generation: u64) -> Result<(), FrontError> {
        self.request_reload(generation)
    }

    fn request_eggpool_intent(
        &self,
        intent: &EggpoolIntentRequest,
        generation: u64,
    ) -> Result<(), FrontError> {
        self.request_eggpool_intent(intent.active, intent.period, intent.refresh, generation)
    }

    fn request_cron_intent(
        &self,
        system_id: Option<&str>,
        job: Option<&str>,
        display_history: usize,
        generation: u64,
    ) -> Result<(), FrontError> {
        self.request_cron_intent(system_id, job, display_history, generation)
    }
}

/// The read side of a frontend connection, and the only socket owner.
pub struct FrameStream {
    connection: Connection,
    requests: mpsc::Receiver<DaemonRequest>,
}

impl FrameStream {
    /// Pull the next frame, writing any pending request first.
    ///
    /// # Errors
    ///
    /// Returns [`FrontError::Disconnected`] when the daemon closed the
    /// connection, and a decode or protocol error when a frame violated the
    /// channel contract.
    pub async fn next(&mut self) -> Result<FrontendFrame, FrontError> {
        loop {
            while let Ok(request) = self.requests.try_recv() {
                self.connection
                    .write_frame(&request)
                    .map_err(FrontError::Transport)?;
            }
            if let Some(frame) = self
                .connection
                .try_read_frame::<FrontendFrame>()
                .map_err(FrontError::Transport)?
            {
                return Ok(frame);
            }
            let mut read_buffer = [0_u8; READ_BUFFER_BYTES];
            match self.connection.read_available(&mut read_buffer) {
                Ok(count) => self
                    .connection
                    .push_bytes(&read_buffer[..count])
                    .map_err(FrontError::Transport)?,
                // Nothing yet. Yield and let the caller retry.
                Err(TransportError::WouldBlock) => {}
                Err(TransportError::Disconnected) => return Err(FrontError::Disconnected),
                Err(error) => return Err(FrontError::Transport(error)),
            }
            // Yield unless more of the frame is already buffered. Reading a
            // partial frame and then looping with no await point would spin
            // this task at full speed: the loop condition only depends on
            // bytes already in hand, and the socket has nothing more until the
            // event loop is polled again — a busy loop the poll interval was
            // meant to prevent.
            if self.connection.buffered() < LEN_PREFIX_BYTES {
                tokio::time::sleep(Duration::from_millis(10)).await;
            } else {
                tokio::task::yield_now().await;
            }
        }
    }

    /// Wait for a frame that ends the session, bounded.
    ///
    /// # Errors
    ///
    /// Propagates any transport or decode failure, and reports
    /// [`FrontError::Timeout`] rather than hanging when the daemon neither
    /// replies nor closes.
    pub async fn next_terminal(&mut self) -> Result<FrontendFrame, FrontError> {
        tokio::time::timeout(TERMINAL_FRAME_TIMEOUT, self.next())
            .await
            .map_err(|_| FrontError::Timeout)?
    }
}

/// Why a frontend could not talk to its daemon.
#[derive(Debug, thiserror::Error)]
pub enum FrontError {
    /// The connection failed or closed.
    #[error("the client daemon connection failed: {0}")]
    Transport(#[source] TransportError),
    /// A frame header violated the channel contract.
    #[error("the client daemon sent an invalid frame: {0}")]
    Decode(#[source] DecodeError),
    /// A frame body was not a valid value.
    #[error("the client daemon sent a malformed frame: {0}")]
    Malformed(String),
    /// The daemon closed the connection.
    #[error("the client daemon closed the connection")]
    Disconnected,
    /// The daemon did not answer a terminal request in time.
    #[error("the client daemon did not answer in time")]
    Timeout,
    /// Too many requests are outstanding and unacknowledged.
    #[error("the client daemon has not acknowledged recent requests")]
    ChannelFull,
}

impl FrontError {
    /// Whether the daemon is gone rather than misbehaving.
    #[must_use]
    pub fn is_disconnect(&self) -> bool {
        matches!(self, Self::Disconnected)
    }
}

/// Encode a frame for tests and for bounded CLI probes.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, FrontError> {
    encode_frame(value).map_err(|error| FrontError::Malformed(error.to_string()))
}

/// A first document for a frontend that has not received one.
///
/// Not a fallback: this exists so `AppState` has a structurally valid starting
/// point and so tests can assert what a TUI renders before any state arrives.
#[must_use]
pub fn placeholder_snapshot() -> FrontendSnapshot {
    FrontendSnapshot::empty(Vec::new())
}
