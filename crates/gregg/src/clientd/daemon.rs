//! Plan 164: the per-config Gregg client daemon.
//!
//! `gregg daemon run` is a foreground process that owns everything derived
//! from the network for one configuration file: the endpoint HTTP client, the
//! poll scheduler and its generation loop, normalized fleet state, the
//! `EggPool` worker, and the `Ctrl-R` config-reload boundary. Frontends attach
//! over local IPC and receive complete state documents.
//!
//! # Why the split exists
//!
//! A TUI that owns its own poller multiplies the fleet's request budget by the
//! number of open windows, and closing one window silently stops a fraction of
//! what the operator is watching. Moving ownership here means closing every TUI
//! changes nothing about polling, and a second window costs a socket rather
//! than a poll generation.
//!
//! # Fan-out discipline
//!
//! State is published through a [`watch`] channel, which is a *latest value*
//! slot rather than a queue. A frontend that cannot keep up observes a newer
//! generation and skips the ones it missed, which is safe because every
//! document is complete and self-contained. The alternative — queueing each
//! generation per frontend — would let one slow window hold memory
//! proportional to fleet size times poll cadence, and would let it delay
//! publication for everyone else.
//!
//! Each document is serialized **once**, at publication time, and every
//! frontend is handed the same bytes. Serialization therefore costs the same
//! with one TUI open as with five, which is the same discipline `greggd`
//! already uses for its scheduler publication cell.
//!
//! Control acknowledgements are the one non-latest-state message, because a
//! caller needs a definite answer. They ride a small per-connection channel
//! rather than the shared slot, so one frontend's request cannot displace
//! another's state.
//!
//! # Failure semantics
//!
//! Losing every frontend is not a reason to stop. The daemon keeps polling
//! with no TUI attached, because continuous background observation is the
//! point of the architecture. A dead poll or `EggPool` task *is* a reason to
//! stop: continuing would publish stale state indefinitely, which is
//! indistinguishable from a healthy fleet that happens not to be changing.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::clientd::identity::ClientDaemonIdentity;
use crate::clientd::ipc::{self, BoundEndpoint, Connection, TransportError};
use crate::clientd::protocol::{
    encode_frame, DaemonRequest, FrontendFrame, HelloPayload, PROTOCOL_VERSION,
};
use crate::clientd::snapshot::FrontendSnapshot;
use crate::config::{ConfigError, ConfigStore};
use crate::eggpool::{self, EggpoolDesiredState, EggpoolPeriod};
use crate::scheduler::SchedulerCommand;
use crate::state::{now_unix_ms, EggpoolIntent, EggpoolIntents, FleetState};

/// How often a connection looks for inbound bytes.
///
/// The connection loop is a single task per frontend, so it polls for
/// readability rather than splitting the socket into separate async read and
/// write halves. That keeps one transport implementation for both platforms.
/// The cost is one timer wake per attached frontend every 20 ms, and the only
/// latency it adds is to a control request the operator just pressed a key
/// for. State delivery is unaffected: it is driven by the watch channel.
const READ_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How often the engine re-reduces frontend `EggPool` intents.
///
/// A disconnect changes the converged worker state even though nobody sends a
/// request, so the reduction cannot live only on the request path.
const INTENT_REDUCE_INTERVAL: Duration = Duration::from_millis(250);

/// Capacity of the per-connection control-ack channel.
///
/// A frontend with this many unacknowledged requests outstanding is not
/// reading. Draining is not an option, because the daemon must not block, so
/// the frame is dropped: a frontend that stopped reading will be closed by the
/// failed state write that follows.
const ACK_CHANNEL_CAPACITY: usize = 8;

/// Capacity of the shared inbound-request channel.
///
/// Requests are rare and small (`Ctrl-R`, an intent update, a stop). A full
/// channel means the engine is wedged, so the sending connection is closed
/// rather than parked behind a queue.
const REQUEST_CHANNEL_CAPACITY: usize = 64;

/// Read buffer for one connection.
const READ_BUFFER_BYTES: usize = 8192;

/// One publication: the encoded frame plus the generation it carries.
struct Document {
    /// Local IPC generation, mirrored from the encoded document so a
    /// connection can report it without decoding.
    #[allow(dead_code)]
    generation: u64,
    /// Encoded `FrontendFrame::Snapshot`, produced once per publication.
    encoded: Vec<u8>,
}

/// The shared publication cell and control surface.
struct Hub {
    /// Latest complete state document. `watch` is a replacement slot, so
    /// `subscribe` immediately yields the current value and a slow reader
    /// observes the newest generation on its next `changed`.
    snapshots: watch::Sender<Arc<Document>>,
    /// Where connections hand decoded requests to the engine.
    requests: mpsc::Sender<Inbound>,
    /// Per-frontend `EggPool` intents, reduced into one converged worker state.
    intents: Arc<Mutex<EggpoolIntents>>,
    /// Config identity this daemon serves.
    daemon_id: String,
    /// Fires when a frontend asks the daemon to stop.
    shutdown: CancellationToken,
}

/// One decoded request plus the frontend that sent it.
struct Inbound {
    /// Subscriber identity, used to replace that frontend's intent.
    subscriber: u64,
    /// The request itself.
    request: DaemonRequest,
    /// Where to deliver an acknowledgement for this connection.
    acks: mpsc::Sender<FrontendFrame>,
}

/// What a request asked the engine loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    /// Keep running; publication already happened.
    Continue,
    /// Unwind now. The acknowledgement is written by the connection before it
    /// observes the cancellation, so `gregg daemon stop` always sees its own
    /// reply.
    Stop,
}

/// Why the client daemon could not start or could not continue.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// The local endpoint could not be bound.
    #[error("could not bind the client-daemon endpoint: {0}")]
    Bind(#[source] TransportError),
    /// The configuration could not be read.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A published state document could not be encoded.
    #[error("could not encode a state document: {0}")]
    Encode(String),
    /// The poll scheduler task died, so the daemon can no longer observe the
    /// fleet.
    #[error("the poll scheduler stopped unexpectedly")]
    SchedulerStopped,
    /// The `EggPool` worker task died.
    #[error("the EggPool worker stopped unexpectedly")]
    EggpoolWorkerStopped,
}

impl DaemonError {
    /// Whether the failure justifies exiting the whole process.
    ///
    /// A dead poller or a dead `EggPool` worker is not recoverable in place.
    /// A config that cannot be read is the one failure that is *not* fatal to
    /// the process, because the daemon can be retried once the operator
    /// fixes the file and a later attach can restart it.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        !matches!(self, Self::Config(_))
    }
}

/// Run the client daemon in the foreground until `cancel` fires.
///
/// This never forks or self-daemonizes: a supervisor (or Plan 165's user
/// startup registration) owns the process lifetime, and a test harness can
/// drive it directly.
pub async fn run_daemon(
    store: ConfigStore,
    identity: ClientDaemonIdentity,
    cancel: CancellationToken,
) -> Result<(), DaemonError> {
    let config = store.load_or_default()?;
    let config_path = store.path().to_path_buf();
    let mut fleet = FleetState::from_config(&config);

    let timeout = Duration::from_millis(config.request_timeout_ms);
    let refresh = Duration::from_secs(config.refresh_seconds);
    let max_concurrent = config.max_concurrent_requests as usize;
    let endpoints: Vec<crate::endpoint::Endpoint> = config
        .systems
        .iter()
        .map(crate::config::SystemEntry::to_endpoint)
        .collect();

    // A token dedicated to the poll and worker tasks, so a stop request
    // unwinds them even while the main loop is still acknowledging the caller
    // that asked for it.
    let tasks = CancellationToken::new();
    let (scheduler_tx, scheduler_rx) = mpsc::channel::<SchedulerCommand>(4);
    let mut batch_rx = Some(
        crate::scheduler::PollScheduler::new(
            crate::clock::RealClock,
            crate::poller::HttpClient::new(timeout),
            refresh,
            max_concurrent,
        )
        .run(endpoints, tasks.clone(), scheduler_rx),
    );

    let eggpool_worker = config.eggpool.clone().map(|entry| {
        eggpool::spawn_worker(eggpool::EggpoolClient::new(timeout), entry, tasks.clone())
    });
    let eggpool_control = eggpool_worker.as_ref().map(|worker| worker.control.clone());
    let mut eggpool_results = eggpool_worker.map(|worker| worker.results);

    // Bind before the first document is published, so a frontend can never
    // observe a daemon that is not yet serving.
    let candidates = identity.candidates();
    let listener = ipc::bind(&candidates).map_err(DaemonError::Bind)?;
    let endpoint: BoundEndpoint = listener.endpoint().clone();

    let (snapshots, _) = watch::channel(encode_document(&fleet, 1)?);
    let (requests, mut request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
    let hub = Arc::new(Hub {
        snapshots,
        requests,
        intents: Arc::new(Mutex::new(EggpoolIntents::default())),
        daemon_id: identity.id().to_owned(),
        shutdown: tasks.clone(),
    });

    let accept_task = tokio::spawn({
        let hub = Arc::clone(&hub);
        let cancel = tasks.clone();
        async move { accept_loop(listener, hub, cancel).await }
    });

    let mut engine = Engine {
        // The initial document was already published at generation 1 below, so
        // the counter starts there rather than producing a duplicate 1 on the
        // first real change.
        generation: 1,
        converged: fleet.eggpool_desired_state(&EggpoolIntents::default()),
        store: ConfigStore::new(config_path),
    };

    let result = engine
        .run(
            &hub,
            &mut fleet,
            &mut batch_rx,
            &mut eggpool_results,
            eggpool_control.as_ref(),
            &scheduler_tx,
            &mut request_rx,
            &cancel,
        )
        .await;

    tasks.cancel();
    accept_task.abort();
    drop(eggpool_control);
    ipc::cleanup(&endpoint.path);
    result
}

impl Hub {
    /// Apply one change to the frontend intent set.
    ///
    /// A poisoned lock still holds usable data: the guard is only poisoned if
    /// a previous holder panicked mid-update, and every mutation here is a
    /// single push or retain. Failing closed on a panic would stop the
    /// `EggPool` worker from ever converging again, which is strictly worse
    /// than converging from a possibly-stale set.
    fn with_intents<T>(&self, edit: impl FnOnce(&mut EggpoolIntents) -> T) -> T {
        let mut guard = self
            .intents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        edit(&mut guard)
    }
}

/// Mutable engine bookkeeping that is not part of the fleet.
struct Engine {
    /// Local IPC generation of the most recent publication.
    generation: u64,
    /// The `EggPool` desired state the worker currently holds.
    converged: Option<EggpoolDesiredState>,
    /// The config file this daemon reloads.
    ///
    /// Resolved from the daemon's own startup path, never from a
    /// frontend-supplied one, so no frontend can redirect the daemon onto a
    /// different configuration.
    store: ConfigStore,
}

impl Engine {
    /// The daemon's main loop: mutate fleet state, publish on real change.
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &mut self,
        hub: &Arc<Hub>,
        fleet: &mut FleetState,
        batch_rx: &mut Option<mpsc::Receiver<crate::poller::PollBatch>>,
        eggpool_results: &mut Option<mpsc::Receiver<eggpool::EggpoolResult>>,
        eggpool_control: Option<&eggpool::EggpoolControl>,
        scheduler_tx: &mpsc::Sender<SchedulerCommand>,
        request_rx: &mut mpsc::Receiver<Inbound>,
        cancel: &CancellationToken,
    ) -> Result<(), DaemonError> {
        let mut stop = Control::Continue;
        let mut tick = tokio::time::interval(INTENT_REDUCE_INTERVAL);

        while stop == Control::Continue {
            let mut dirty = false;
            tokio::select! {
                () = cancel.cancelled() => {
                    stop = Control::Stop;
                }
                () = hub.shutdown.cancelled() => {
                    // A frontend asked the daemon to stop. Its acknowledgement
                    // is already queued on its own connection, so exiting here
                    // cannot lose it.
                    stop = Control::Stop;
                }

                // An empty system list produces no batches at all, so the
                // receiver is retired rather than left pending forever.
                maybe = recv_opt(batch_rx), if batch_rx.is_some() => {
                    let was_initialized = fleet.last_applied_generation != 0;
                    match maybe {
                        Some(batch) => dirty |= fleet.apply_batch_owned_changed(batch),
                        None => *batch_rx = None,
                    }
                    // The transition from "never polled" to "polled at least
                    // once" is published even when it changed nothing visible.
                    // A frontend places its selection once, on the first
                    // document that carries reachability; if that document
                    // were suppressed, the placement would instead fire on
                    // some later, unrelated change and undo whatever the
                    // operator had selected in the meantime. This costs one
                    // publication in the daemon's entire life, not one per
                    // poll.
                    dirty |= was_initialized != (fleet.last_applied_generation != 0);
                }

                maybe = recv_opt(eggpool_results), if eggpool_results.is_some() => {
                    if let Some(result) = maybe {
                        dirty |= fleet.apply_eggpool_result_changed(&result);
                    } else {
                        fleet.mark_eggpool_worker_unavailable();
                        *eggpool_results = None;
                        dirty = true;
                    }
                }

                Some(inbound) = request_rx.recv() => {
                    stop = self
                        .handle_request(fleet, hub, scheduler_tx, inbound)
                        .await;
                    dirty = true;
                }

                _ = tick.tick() => {
                    dirty |= self.reduce_intents(fleet, hub, eggpool_control);
                }
            }

            if dirty && stop == Control::Continue {
                self.generation = self.generation.saturating_add(1);
                let document = encode_document(fleet, self.generation)?;
                // A send with no receivers is not an error: a daemon with no
                // frontend attached still advances its own state.
                let _ = hub.snapshots.send(document);
            }
        }
        Ok(())
    }

    /// Recompute the converged `EggPool` worker state from every attached
    /// frontend's intent, publishing only on a real change.
    fn reduce_intents(
        &mut self,
        fleet: &mut FleetState,
        hub: &Arc<Hub>,
        eggpool_control: Option<&eggpool::EggpoolControl>,
    ) -> bool {
        let Some(control) = eggpool_control else {
            return false;
        };
        // A poisoned lock still holds usable data: the guard is only poisoned
        // if a previous holder panicked mid-update, and `set`/`remove` are
        // single pushes. Losing the whole daemon over that would be a worse
        // outcome than converging from a possibly-stale intent set.
        let guard = hub
            .intents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let intents: EggpoolIntents = guard.clone();
        let Some(desired) = fleet.eggpool_desired_state(&intents) else {
            return false;
        };
        if self.converged == Some(desired) {
            return false;
        }
        if control.publish(desired).is_err() {
            fleet.mark_eggpool_worker_unavailable();
            return true;
        }
        self.converged = Some(desired);
        false
    }

    /// Apply one frontend request, returning whether the daemon should stop.
    async fn handle_request(
        &mut self,
        fleet: &mut FleetState,
        hub: &Arc<Hub>,
        scheduler_tx: &mpsc::Sender<SchedulerCommand>,
        inbound: Inbound,
    ) -> Control {
        let Inbound {
            subscriber,
            request,
            acks,
        } = inbound;

        let mut stop = Control::Continue;
        let (ack_generation, accepted, detail) = match request {
            // Handled by the connection before it ever reaches the engine.
            DaemonRequest::Handshake { .. } => return stop,
            DaemonRequest::ReloadConfig { generation } => {
                let (accepted, detail) = self.reload_config(fleet, scheduler_tx).await;
                (generation, accepted, detail)
            }
            DaemonRequest::SetEggpoolIntent {
                active,
                period,
                refresh,
                generation,
            } => {
                let previous = hub.with_intents(|guard| guard.set(subscriber, active, period));
                // A new window, a newly opened pane, or an explicit manual
                // refresh each need a fresh worker generation; without one the
                // worker would return the previous generation's result and the
                // reducer would reject it as stale. An identical re-send of
                // the same intent is a no-op, which is what keeps a
                // reconnecting frontend from minting generations forever.
                let changed = previous != Some(EggpoolIntent { active, period });
                if fleet.set_eggpool_period(period) || changed || refresh {
                    fleet.begin_eggpool_request();
                }
                (generation, true, None)
            }
            DaemonRequest::Shutdown { generation } => {
                stop = Control::Stop;
                (generation, true, None)
            }
        };

        let frame = FrontendFrame::ControlAck {
            generation: ack_generation,
            accepted,
            detail,
        };
        // `try_send` rather than `send`: a frontend that stopped reading must
        // not be able to stall the engine loop. The connection's own state
        // write is what will notice and close it.
        let _ = acks.try_send(frame);
        stop
    }

    /// Re-read the config and reconcile, or keep the last-known-good fleet.
    async fn reload_config(
        &mut self,
        fleet: &mut FleetState,
        scheduler_tx: &mpsc::Sender<SchedulerCommand>,
    ) -> (bool, Option<String>) {
        match self.store.load_existing() {
            Ok(config) => {
                let endpoints: Vec<crate::endpoint::Endpoint> = config
                    .systems
                    .iter()
                    .map(crate::config::SystemEntry::to_endpoint)
                    .collect();
                fleet.reconcile_systems(&config);
                match config.eggpool.clone() {
                    Some(entry) => fleet.adopt_eggpool_endpoint(entry),
                    None => fleet.clear_eggpool(),
                }
                fleet.clear_config_reload_error();
                // Accepted config that changes endpoints polls immediately
                // rather than waiting out the remaining cadence.
                let _ = scheduler_tx
                    .send(SchedulerCommand::ReplaceEndpoints(endpoints))
                    .await;
                (true, None)
            }
            Err(error) => {
                let message = format!("config reload failed: {error}");
                fleet.set_config_reload_error(message.clone());
                // Still poll: a temporarily invalid file must not freeze
                // metrics that were already being collected.
                let _ = scheduler_tx.send(SchedulerCommand::Refresh).await;
                (false, Some(message))
            }
        }
    }
}

fn encode_document(fleet: &FleetState, generation: u64) -> Result<Arc<Document>, DaemonError> {
    let dto = fleet.to_dto(std::time::Instant::now(), now_unix_ms(), generation);
    let frame = FrontendFrame::Snapshot(Box::new(dto));
    let encoded = encode_frame(&frame).map_err(|error| DaemonError::Encode(error.to_string()))?;
    Ok(Arc::new(Document {
        generation,
        encoded,
    }))
}

async fn recv_opt<T>(receiver: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Accept connections until cancelled.
async fn accept_loop(mut listener: ipc::Listener, hub: Arc<Hub>, cancel: CancellationToken) {
    let mut next_subscriber = 1_u64;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match listener.accept() {
            Ok(connection) => {
                let subscriber = next_subscriber;
                next_subscriber = next_subscriber.wrapping_add(1).max(1);
                let hub = Arc::clone(&hub);
                tokio::spawn(async move {
                    serve(connection, hub, subscriber).await;
                });
            }
            // No connection is waiting. That is the normal state, not an error.
            Err(TransportError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // A pipe instance can legitimately be taken by a client that
            // disconnects between instance creation and the connect call.
            Err(TransportError::Disconnected) => {}
            Err(_) => return,
        }
    }
}

/// Serve one frontend for its whole lifetime.
async fn serve(mut connection: Connection, hub: Arc<Hub>, subscriber: u64) {
    if let Err(error) = serve_inner(&mut connection, &hub, subscriber).await {
        // One frontend's protocol failure is that frontend's problem. The
        // daemon keeps polling and other frontends keep their state.
        let _ = connection.write_frame(&FrontendFrame::ProtocolError(error.to_string()));
    }
    // A departed frontend must stop shaping the `EggPool` worker, or the
    // worker would stay activated for a window nobody is watching.
    hub.with_intents(|intents| intents.remove(subscriber));
}

async fn serve_inner(
    connection: &mut Connection,
    hub: &Arc<Hub>,
    subscriber: u64,
) -> Result<(), TransportError> {
    let mut snapshots = hub.snapshots.subscribe();
    let (ack_tx, mut ack_rx) = mpsc::channel(ACK_CHANNEL_CAPACITY);
    let mut handshake_done = false;
    let mut read_buffer = vec![0_u8; READ_BUFFER_BYTES];

    loop {
        if !handshake_done {
            if let Some(request) = connection.try_read_request()? {
                match request {
                    DaemonRequest::Handshake { .. } => {
                        // `try_read_request` already refused a protocol
                        // version this daemon does not speak; this is the
                        // config-identity check, so a TUI cannot attach to a
                        // daemon serving a different configuration.
                        connection.verify_daemon_id(&hub.daemon_id)?;
                        handshake_done = true;
                        connection.write_frame(&FrontendFrame::Hello(Box::new(HelloPayload {
                            protocol_version: PROTOCOL_VERSION,
                            version: env!("CARGO_PKG_VERSION").to_owned(),
                            daemon_id: hub.daemon_id.clone(),
                            current_generation: snapshots.borrow().generation,
                        })))?;
                        // A new subscriber gets the complete current state
                        // immediately: an empty TUI must never have to wait for
                        // the next poll generation to learn what is configured.
                        let current = Arc::clone(&snapshots.borrow());
                        connection.write_encoded(&current.encoded)?;
                        hub.with_intents(|intents| {
                            intents.set(subscriber, false, EggpoolPeriod::Hour);
                        });
                    }
                    other => {
                        return Err(TransportError::Protocol(format!(
                            "expected a handshake, got {other:?}"
                        )));
                    }
                }
            }
        } else if let Some(request) = connection.try_read_request()? {
            // `try_send`, never `send`: a full channel means the engine is
            // wedged, and blocking here would let one frontend hold the
            // accept loop hostage.
            if hub
                .requests
                .try_send(Inbound {
                    subscriber,
                    request,
                    acks: ack_tx.clone(),
                })
                .is_err()
            {
                return Err(TransportError::Protocol(
                    "the client daemon is not accepting requests".to_owned(),
                ));
            }
        }

        let mut write_state = false;
        tokio::select! {
            changed = snapshots.changed() => {
                if changed.is_err() {
                    // The daemon is gone. Report the reason rather than
                    // looking like a network failure.
                    let _ = connection.write_frame(&FrontendFrame::ShuttingDown);
                    return Ok(());
                }
                write_state = true;
            }
            Some(frame) = ack_rx.recv() => {
                // A failed ack write means the peer is gone; there is no
                // partial frame to salvage, so the connection ends here.
                connection.write_frame(&frame)?;
            }
            () = tokio::time::sleep(READ_POLL_INTERVAL) => {
                match connection.read_available(&mut read_buffer) {
                    Ok(count) => connection.push_bytes(&read_buffer[..count]),
                    // Nothing yet. The loop retries on its next tick.
                    Err(TransportError::WouldBlock) => {}
                    Err(TransportError::Disconnected) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        }

        if write_state {
            let latest = Arc::clone(&snapshots.borrow_and_update());
            connection.write_encoded(&latest.encoded)?;
        }
    }
}

/// Why a frontend could not attach to a client daemon.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// No compatible daemon answered on any candidate endpoint.
    #[error("no client daemon is listening for this config: {0}")]
    Transport(#[source] TransportError),
    /// The endpoint accepted a connection but never identified itself.
    #[error("the client daemon did not complete a handshake in time")]
    HandshakeTimeout,
    /// The endpoint closed during the handshake.
    #[error("the client daemon closed the connection during the handshake")]
    Disconnected,
    /// The daemon speaks a different local protocol version.
    #[error(
        "the client daemon for this config speaks local protocol {daemon}, but this frontend speaks {frontend}; stop it with `gregg daemon stop` and start it again with the current binary",
        daemon = .daemon,
        frontend = .frontend
    )]
    VersionMismatch {
        /// Local protocol version the daemon speaks.
        daemon: u16,
        /// Local protocol version this frontend speaks.
        frontend: u16,
    },
    /// The daemon refused this frontend for a stated reason.
    #[error("the client daemon refused this frontend: {0}")]
    Refused(String),
}

impl AttachError {
    /// Whether the endpoint is simply not there.
    ///
    /// Only a connect-level failure counts as "absent". A version mismatch, an
    /// identity mismatch, or a malformed peer is a *live* endpoint that must
    /// never authorize a competing daemon.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Transport(_))
    }
}

/// An established attachment to a running client daemon.
pub struct Attachment {
    /// The send side for control requests.
    pub frontend: crate::clientd::frontend::FrontendSender,
    /// The read side of the connection, and the only socket owner.
    pub frames: crate::clientd::frontend::FrameStream,
    /// What the daemon said about itself.
    pub hello: HelloPayload,
}

/// Attach to the client daemon for one configuration.
///
/// Plan 164's contract is narrow on purpose. A frontend that cannot reach a
/// compatible daemon must **fail** rather than quietly resume direct remote
/// polling: a silent fallback would recreate the duplicate ownership this
/// architecture removes, and would hide the daemon's failure from the operator
/// entirely. Plan 165 turns an *absent* endpoint into a bounded lazy launch; it
/// does not relax anything here.
pub async fn attach(
    identity: &ClientDaemonIdentity,
    version: &str,
) -> Result<Attachment, AttachError> {
    let candidates = identity.candidates();
    let mut link = crate::clientd::frontend::FrontendLink::connect(&candidates)
        .map_err(AttachError::Transport)?;
    link.handshake(identity, version)
        .map_err(AttachError::Transport)?;
    let (sender, mut stream) = link.split();

    // Wait for the daemon to identify itself before trusting anything else.
    let first = stream.next_terminal().await.map_err(|error| match error {
        crate::clientd::frontend::FrontError::Timeout => AttachError::HandshakeTimeout,
        crate::clientd::frontend::FrontError::Disconnected => AttachError::Disconnected,
        other => AttachError::Refused(other.to_string()),
    })?;
    match first {
        FrontendFrame::Hello(hello) => Ok(Attachment {
            frontend: sender,
            frames: stream,
            hello: *hello,
        }),
        FrontendFrame::VersionMismatch(payload) => Err(AttachError::VersionMismatch {
            daemon: payload.daemon,
            frontend: payload.frontend,
        }),
        FrontendFrame::ProtocolError(message) => Err(AttachError::Refused(message)),
        other => Err(AttachError::Refused(format!(
            "expected a hello frame, got {other:?}"
        ))),
    }
}

/// What `gregg daemon status` reports about one configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    /// The config identity the answer is about.
    pub daemon_id: String,
    /// Whether a compatible daemon answered the handshake.
    pub running: bool,
    /// The daemon's workspace version, when it answered.
    pub version: Option<String>,
    /// The local protocol version the daemon speaks.
    pub protocol_version: Option<u16>,
    /// The endpoint a client would dial, for diagnostics.
    pub endpoint: PathBuf,
    /// Why the daemon is not usable, when it is not running.
    pub detail: Option<String>,
}

impl DaemonStatus {
    /// Render one bounded, operator-readable line.
    #[must_use]
    pub fn render(&self) -> String {
        if self.running {
            format!(
                "client daemon {} running (protocol {}, version {}) at {}",
                self.daemon_id,
                self.protocol_version.unwrap_or_default(),
                self.version.as_deref().unwrap_or("unknown"),
                self.endpoint.display()
            )
        } else {
            format!(
                "client daemon {} not running at {}: {}",
                self.daemon_id,
                self.endpoint.display(),
                self.detail
                    .as_deref()
                    .unwrap_or("no client daemon answered")
            )
        }
    }
}

/// Report whether a matching client daemon is running.
///
/// Read-only and bounded: one dial, one handshake, one answer. Identity comes
/// from the protocol handshake, never from a process name or a PID file, so the
/// answer cannot be a guess about an unrelated process that happens to share a
/// name.
pub async fn status(identity: &ClientDaemonIdentity) -> Result<DaemonStatus, AttachError> {
    let endpoint = identity
        .candidates()
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("<unresolved>"));
    match attach(identity, env!("CARGO_PKG_VERSION")).await {
        Ok(attachment) => Ok(DaemonStatus {
            daemon_id: attachment.hello.daemon_id.clone(),
            running: true,
            version: Some(attachment.hello.version.clone()),
            protocol_version: Some(attachment.hello.protocol_version),
            endpoint,
            detail: None,
        }),
        Err(error) => Ok(DaemonStatus {
            daemon_id: identity.id().to_owned(),
            running: false,
            version: None,
            protocol_version: None,
            endpoint,
            detail: Some(error.to_string()),
        }),
    }
}

/// Stop the matching client daemon.
///
/// Only a positively identified daemon serving this same configuration is
/// asked to stop. A foreign endpoint, a different config's daemon, and a
/// protocol mismatch are all reported rather than forced: killing something
/// that merely looks similar is exactly the failure mode ownership checks
/// exist to prevent.
pub async fn stop(identity: &ClientDaemonIdentity, version: &str) -> Result<(), AttachError> {
    let mut attachment = attach(identity, version).await?;
    attachment
        .frontend
        .request_shutdown(1)
        .map_err(|error| AttachError::Refused(error.to_string()))?;
    match attachment.frames.next_terminal().await {
        Ok(FrontendFrame::ControlAck {
            accepted, detail, ..
        }) => {
            if accepted {
                Ok(())
            } else {
                Err(AttachError::Refused(detail.unwrap_or_else(|| {
                    "the daemon refused the stop request".to_owned()
                })))
            }
        }
        // The daemon closes after honouring a stop, so a clean disconnect
        // immediately after the request is a success, not a failure.
        Ok(_) | Err(_) => Ok(()),
    }?;

    // Wait for the endpoint to go away. Returning on the acknowledgement alone
    // would report success for a daemon that is still unwinding, and the very
    // next command would find it still listening.
    for _ in 0..200 {
        if identity.candidates().iter().all(|path| !path.exists()) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(AttachError::Refused(
        "the client daemon accepted the stop but is still listening".to_owned(),
    ))
}

/// The state a frontend starts from when no daemon has answered yet.
///
/// A TUI is never constructed from a configuration file, so this exists only
/// so the first frame has something structurally valid to be compared against
/// in tests.
#[must_use]
pub fn empty_document() -> FrontendSnapshot {
    FrontendSnapshot::empty(Vec::new())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::clientd::frontend::FrontendLink;
    use crate::clientd::snapshot::FrontendSnapshot;
    use crate::config::{Config, SystemEntry};
    use std::time::Duration;

    /// A per-test temporary directory that cleans itself up.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gregg-clientd-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn config_path(&self) -> std::path::PathBuf {
            self.0.join("gregg.toml")
        }

        fn socket_path(&self) -> std::path::PathBuf {
            self.0.join("gregg-client.sock")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A config with no systems and a one-second cadence.
    ///
    /// An empty system list means the scheduler produces no batches at all,
    /// which is exactly what the ownership and fan-out tests need: they are
    /// about the local channel, not about metrics, and must not depend on a
    /// reachable host.
    fn write_empty_config(dir: &TempDir) -> ConfigStore {
        let config = Config {
            refresh_seconds: 1,
            ..Config::default()
        };
        let store = ConfigStore::new(dir.config_path());
        store.write(&config).expect("writes config");
        store
    }

    /// A running daemon plus the identity needed to reach it.
    struct Running {
        cancel: CancellationToken,
        handle: tokio::task::JoinHandle<Result<(), DaemonError>>,
        identity: ClientDaemonIdentity,
    }

    impl Running {
        fn start(dir: &TempDir, store: ConfigStore) -> Self {
            let identity = ClientDaemonIdentity::for_path(&dir.config_path());
            let cancel = CancellationToken::new();
            let handle = {
                let cancel = cancel.clone();
                let identity = ClientDaemonIdentity::for_path(&dir.config_path());
                tokio::spawn(async move { run_daemon(store, identity, cancel).await })
            };
            Self {
                cancel,
                handle,
                identity,
            }
        }

        /// Wait for the endpoint to appear, bounded.
        async fn ready(&self) {
            for _ in 0..200 {
                if self.identity.candidates().iter().any(|path| path.exists()) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("the daemon never bound its endpoint");
        }

        async fn shutdown(self) {
            self.cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.handle).await;
        }
    }

    /// The next state document, bounded.
    async fn next_snapshot(frames: &mut crate::clientd::frontend::FrameStream) -> FrontendSnapshot {
        for _ in 0..200 {
            if let FrontendFrame::Snapshot(document) = frames.next().await.expect("reads") {
                return *document;
            }
        }
        panic!("no state document arrived");
    }

    #[tokio::test]
    async fn one_config_identity_gets_one_endpoint_and_two_configs_never_share_one() {
        let first = TempDir::new("identity-a");
        let second = TempDir::new("identity-b");
        let a = ClientDaemonIdentity::for_path(&first.config_path());
        let b = ClientDaemonIdentity::for_path(&second.config_path());
        assert_ne!(a.id(), b.id(), "distinct configs must not collide");
        assert_ne!(a.candidates()[0], b.candidates()[0]);

        // The same file reached through an equivalent spelling converges.
        let direct = ClientDaemonIdentity::for_path(&first.config_path());
        assert_eq!(a.id(), direct.id());
    }

    #[tokio::test]
    async fn a_new_subscriber_receives_complete_current_state_immediately() {
        let dir = TempDir::new("first-subscriber");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        link.handshake(&running.identity, "test")
            .expect("handshakes");
        let (sender, mut frames) = link.split();

        // The first frame after the handshake is the complete current state,
        // not a delta: a fresh TUI never has to wait for a poll.
        let document = next_snapshot(&mut frames).await;
        assert_eq!(document.systems.len(), 0);
        assert_eq!(
            document.generation, 1,
            "the initial document is generation 1"
        );

        drop(sender);
        running.shutdown().await;
    }

    #[tokio::test]
    async fn two_subscribers_share_one_publication_instead_of_one_poll_each() {
        let dir = TempDir::new("fanout");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        let mut first = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        first
            .handshake(&running.identity, "test")
            .expect("handshakes");
        let (first_sender, mut first_frames) = first.split();
        let mut second = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        second
            .handshake(&running.identity, "test")
            .expect("handshakes");
        let (second_sender, mut second_frames) = second.split();

        let initial_first = next_snapshot(&mut first_frames).await;
        let initial_second = next_snapshot(&mut second_frames).await;
        assert_eq!(
            initial_first.generation, initial_second.generation,
            "both subscribers observe the same publication, not two polls"
        );

        // One reload produces exactly one publication, and both subscribers
        // see the same generation of it.
        first_sender.request_reload(1).expect("queues");
        let reloaded_first = next_snapshot(&mut first_frames).await;
        let reloaded_second = next_snapshot(&mut second_frames).await;
        assert!(reloaded_first.generation > initial_first.generation);
        assert_eq!(reloaded_first.generation, reloaded_second.generation);

        drop(first_sender);
        drop(second_sender);
        running.shutdown().await;
    }

    #[tokio::test]
    async fn a_disconnected_subscriber_does_not_stop_the_daemon() {
        let dir = TempDir::new("disconnect");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        {
            let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
            link.handshake(&running.identity, "test")
                .expect("handshakes");
            let (_sender, mut frames) = link.split();
            next_snapshot(&mut frames).await;
        }
        // Give the accept loop a moment to notice the closed socket.
        tokio::time::sleep(Duration::from_millis(80)).await;

        // The endpoint is still served, which is the whole point: losing a
        // window must not stop background polling.
        let status = status(&running.identity).await.expect("status answers");
        assert!(status.running, "the daemon must outlive its last frontend");

        // And it still answers a fresh attach.
        let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        link.handshake(&running.identity, "test")
            .expect("handshakes");
        let (_sender, mut frames) = link.split();
        next_snapshot(&mut frames).await;

        running.shutdown().await;
    }

    #[tokio::test]
    async fn a_stop_request_from_an_identified_daemon_stops_it() {
        let dir = TempDir::new("stop");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;
        stop(&running.identity, "test")
            .await
            .expect("stop acknowledged");

        let status = status(&running.identity).await.expect("status answers");
        assert!(!status.running, "the daemon must be gone after a stop");
        assert!(
            !dir.socket_path().exists(),
            "the endpoint must be unlinked on an orderly exit"
        );
    }

    #[tokio::test]
    async fn a_handshake_for_a_different_config_is_refused() {
        let dir = TempDir::new("mismatch");
        let other = TempDir::new("mismatch-other");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        // A TUI that believes it is talking to a different config must not be
        // handed this daemon's fleet.
        let foreign = ClientDaemonIdentity::for_path(&other.config_path());
        let mut framed = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        framed
            .send(&crate::clientd::protocol::DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "test".to_owned(),
                daemon_id: foreign.id().to_owned(),
            })
            .expect("writes");
        let (foreign_sender, mut framed_frames) = framed.split();
        let _ = foreign_sender;
        // The refused handshake produces a protocol error, never a snapshot.
        let mut saw_snapshot = false;
        for _ in 0..50 {
            match framed_frames.next().await {
                Ok(FrontendFrame::Snapshot(_)) => {
                    saw_snapshot = true;
                    break;
                }
                // A refusal, a close, or any other non-snapshot frame ends
                // the wait: the assertion is only that no state was served.
                Ok(FrontendFrame::ProtocolError(_)) | Err(_) => break,
                // Neither frame is a snapshot; keep reading.
                Ok(_) => {}
            }
        }
        assert!(
            !saw_snapshot,
            "a frontend for another config must never receive this daemon's state"
        );

        running.shutdown().await;
    }

    #[tokio::test]
    async fn an_invalid_reload_keeps_the_last_known_good_fleet_and_reports_it() {
        let dir = TempDir::new("reload-failure");
        let store = write_empty_config(&dir);
        let mut config = store.load_existing().expect("loads");
        config.systems.push(SystemEntry {
            id: "kept".into(),
            host: "127.0.0.1".into(),
            port: 11310,
            name: None,
        });
        store.write(&config).expect("writes");
        let running = Running::start(&dir, store);
        running.ready().await;

        let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        link.handshake(&running.identity, "test")
            .expect("handshakes");
        let (sender, mut frames) = link.split();
        let initial = next_snapshot(&mut frames).await;
        assert_eq!(initial.systems.len(), 1);
        assert!(initial.config_reload_error.is_none());

        // Corrupt the file behind the daemon's back.
        let store = ConfigStore::new(dir.config_path());
        std::fs::write(dir.config_path(), "this is not = valid = toml [[[")
            .expect("writes broken config");
        sender.request_reload(1).expect("queues");

        let mut saw_error = false;
        for _ in 0..10 {
            let document = next_snapshot(&mut frames).await;
            if let Some(error) = document.config_reload_error {
                assert!(
                    error.contains("config reload failed"),
                    "unexpected diagnostic: {error}"
                );
                saw_error = true;
                // The last-known-good fleet survives an invalid file.
                assert_eq!(
                    document.systems.len(),
                    1,
                    "an invalid reload must not erase the systems already being polled"
                );
                break;
            }
        }
        assert!(saw_error, "a rejected reload must publish a diagnostic");

        // Repairing the file clears it.
        store.write(&config).expect("rewrites");
        sender.request_reload(2).expect("queues");
        let mut cleared = false;
        for _ in 0..10 {
            let document = next_snapshot(&mut frames).await;
            if document.config_reload_error.is_none() {
                cleared = true;
                break;
            }
        }
        assert!(cleared, "a successful reload must clear the diagnostic");

        drop(sender);
        running.shutdown().await;
    }

    #[tokio::test]
    async fn eggpool_intents_converge_deterministically_and_free_on_disconnect() {
        let dir = TempDir::new("intents");
        let store = write_empty_config(&dir);
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        store.write(&config).expect("writes");

        let identity = ClientDaemonIdentity::for_path(&dir.config_path());

        // Two frontends, each asking for a different window. The reduction is
        // the shortest *active* window, and it does not depend on the order
        // the intents arrived in.
        let mut intents = EggpoolIntents::default();
        intents.set(1, true, EggpoolPeriod::Week);
        intents.set(2, true, EggpoolPeriod::Day);
        assert_eq!(
            intents.converged_period(EggpoolPeriod::Hour),
            EggpoolPeriod::Day
        );
        assert!(intents.any_active());

        // Reversing the insertion order changes nothing.
        let mut reversed = EggpoolIntents::default();
        reversed.set(2, true, EggpoolPeriod::Day);
        reversed.set(1, true, EggpoolPeriod::Week);
        assert_eq!(
            reversed.converged_period(EggpoolPeriod::Hour),
            intents.converged_period(EggpoolPeriod::Hour)
        );

        // An inactive frontend's window must not win: it is not being shown.
        reversed.set(2, false, EggpoolPeriod::Month);
        assert_eq!(
            reversed.converged_period(EggpoolPeriod::Hour),
            EggpoolPeriod::Week
        );

        // The last pane leaving converges the worker to inactive.
        reversed.set(1, false, EggpoolPeriod::Week);
        assert!(!reversed.any_active());
        assert_eq!(
            reversed.converged_period(EggpoolPeriod::Hour),
            EggpoolPeriod::Hour
        );

        // Re-sending the same intent replaces the entry rather than adding one,
        // so a frontend that refreshes often cannot leak an active entry that
        // keeps the worker running after it disconnected.
        let mut repeated = EggpoolIntents::default();
        for _ in 0..100 {
            repeated.set(7, true, EggpoolPeriod::Hour);
        }
        assert_eq!(repeated.len(), 1);
        repeated.remove(7);
        assert!(repeated.is_empty());

        let running = Running::start(&dir, store);
        running.ready().await;
        // A real attach registers an inactive intent, so the worker starts
        // inactive even though `EggPool` is configured.
        let mut link = FrontendLink::connect(&identity.candidates()).expect("connects");
        link.handshake(&identity, "test").expect("handshakes");
        let (_sender, mut frames) = link.split();
        let document = next_snapshot(&mut frames).await;
        assert!(document.eggpool.is_some());
        assert_eq!(
            document.eggpool.as_ref().map(|e| e.worker_state),
            Some(crate::state::EggpoolWorkerState::Idle),
            "a frontend that has not opened the pane leaves the worker inactive"
        );
        running.shutdown().await;
    }
}
