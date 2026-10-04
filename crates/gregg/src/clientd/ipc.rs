//! Plan 164: the local IPC transport.
//!
//! A Unix domain socket on Unix and a named pipe on Windows, both speaking the
//! length-prefixed JSON frames in [`crate::clientd::protocol`]. This module owns
//! the socket mechanics and the handshake; it has no knowledge of polling, cron,
//! or rendering.
//!
//! # Permissions
//!
//! The listener creates the socket with owner-only permissions on Unix and does
//! nothing special on Windows beyond the default named-pipe ACL, which already
//! restricts access to the creating user. The channel is same-user by
//! construction and is not a privilege boundary against other accounts.
//!
//! # Connection hygiene
//!
//! A client disconnect is detected on the next write and drops that connection
//! without affecting any other. A daemon shutdown closes every connection
//! cleanly. Neither a stalled nor a vanished client can keep the daemon's poll
//! loop from making progress: the poll loop never awaits a socket.

// Plan 164 lands this transport before the `gregg daemon` entry point and the
// TUI's frontend client that call it. Until those are wired, the read side and
// the connection constructor have no production caller, so this module-level
// dead-code allowance stands in for that wiring. It is removed in the commit
// that adds `gregg daemon run` and the frontend dial path.
#![allow(dead_code)]

use std::io::{self, Read as _, Write as _};
use std::path::PathBuf;

use crate::clientd::protocol::{
    parse_length, DaemonRequest, DecodeError, LEN_PREFIX_BYTES, PROTOCOL_VERSION,
};

/// Where a bound listener ended up, and which path a client should dial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundEndpoint {
    /// The path a client connects to.
    pub path: PathBuf,
    /// True when the primary config-adjacent location was usable.
    pub primary: bool,
}

/// A framing error that is specific to one connection.
#[derive(Debug)]
pub enum TransportError {
    /// The peer disconnected, cleanly or not.
    Disconnected,
    /// A frame could not be read or written.
    Decode(Box<DecodeError>),
    /// An OS-level failure.
    Io(io::Error),
    /// The peer sent something before the handshake, or never completed it.
    Protocol(String),
    /// The versions disagree; the caller must surface this and stop.
    VersionMismatch {
        /// Version the remote end speaks.
        daemon: u16,
        /// Version this end speaks.
        frontend: u16,
    },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => f.write_str("peer disconnected"),
            Self::Decode(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::VersionMismatch { daemon, frontend } => write!(
                f,
                "client daemon speaks protocol {daemon}, this frontend speaks {frontend}"
            ),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<DecodeError> for TransportError {
    fn from(error: DecodeError) -> Self {
        match error {
            DecodeError::Closed => Self::Disconnected,
            other => Self::Decode(Box::new(other)),
        }
    }
}

/// One established connection, with its read buffer.
#[derive(Debug)]
pub struct Connection {
    stream: Stream,
    buffer: Vec<u8>,
    /// True once a compatible handshake has completed on this connection.
    handshaken: bool,
}

impl Connection {
    fn new(stream: Stream) -> Self {
        Self {
            stream,
            buffer: Vec::with_capacity(8192),
            handshaken: false,
        }
    }

    /// Whether a compatible handshake has completed.
    #[must_use]
    pub fn is_handshaken(&self) -> bool {
        self.handshaken
    }

    /// Read one request frame, if one is already buffered.
    ///
    /// Returns `Ok(None)` when more bytes are needed, so a caller can poll
    /// without blocking.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] on disconnect, a malformed or oversized
    /// frame, or any frame sent before a valid handshake.
    pub fn try_read_request(&mut self) -> Result<Option<DaemonRequest>, TransportError> {
        if self.buffer.len() < LEN_PREFIX_BYTES {
            return Ok(None);
        }
        let len = parse_length(&self.buffer[..LEN_PREFIX_BYTES])?;
        let end = LEN_PREFIX_BYTES.saturating_add(len);
        if self.buffer.len() < end {
            return Ok(None);
        }
        let body: Vec<u8> = self.buffer.drain(..end).skip(LEN_PREFIX_BYTES).collect();
        let request: DaemonRequest = serde_json::from_slice(&body)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        if !self.handshaken {
            match &request {
                DaemonRequest::Handshake {
                    protocol_version, ..
                } if *protocol_version == PROTOCOL_VERSION => {
                    self.handshaken = true;
                }
                DaemonRequest::Handshake {
                    protocol_version, ..
                } => {
                    return Err(TransportError::VersionMismatch {
                        daemon: PROTOCOL_VERSION,
                        frontend: *protocol_version,
                    });
                }
                other => {
                    return Err(TransportError::Protocol(format!(
                        "expected a handshake before {other:?}"
                    )));
                }
            }
        }
        Ok(Some(request))
    }

    /// Feed freshly read bytes into the buffer.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes still buffered but not yet consumed as a frame.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Take the read buffer, leaving it empty.
    #[must_use]
    pub fn take_buffer(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    /// Write one frame.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] when the peer is gone or the frame exceeds
    /// the cap. A failed write marks this connection unusable; the caller drops
    /// it rather than retrying into a half-written frame.
    pub fn write_frame<T: serde::Serialize>(&mut self, value: &T) -> Result<(), TransportError> {
        let encoded = crate::clientd::protocol::encode_frame(value)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        self.stream.write_all(&encoded)?;
        self.stream.flush()?;
        Ok(())
    }
}

/// A platform stream, hiding the Unix/Windows difference.
#[derive(Debug)]
pub(crate) enum Stream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    #[cfg(windows)]
    Windows(std::io::ReadWrite),
}

impl Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => {
                stream.set_nonblocking(true)?;
                let result = stream.read(buf);
                restore_read_blocking(stream, &result)?;
                result
            }
            #[cfg(windows)]
            Self::Windows(inner) => std::io::Read::read(inner, buf),
        }
    }

    /// Flush any buffered writes.
    ///
    /// A Unix stream is unbuffered after each `write_all`, so this is a no-op
    /// there; the Windows wrapper needs it to push its internal buffer.
    #[cfg_attr(unix, allow(clippy::unnecessary_wraps))]
    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(_) => Ok(()),
            #[cfg(windows)]
            Self::Windows(inner) => std::io::Write::flush(inner),
        }
    }

    /// True when this stream's peer has gone away.
    ///
    /// A write failure is the only reliable disconnect signal on a Unix socket;
    /// a vanished peer cannot be polled for otherwise.
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => {
                stream.set_nonblocking(true)?;
                let result = stream.write_all(buf);
                restore_write_blocking(stream, &result)?;
                result
            }
            #[cfg(windows)]
            Self::Windows(inner) => std::io::Write::write_all(inner, buf),
        }
    }
}

/// Restore blocking mode after a non-blocking write that hit `WouldBlock`.
///
/// A `WouldBlock` on a Unix socket is not a disconnect: it means the peer's
/// receive buffer is momentarily full, so the descriptor is returned to
/// blocking mode and the outer loop decides what to do next. Blocking writes
/// are bounded because a snapshot frame is capped.
#[cfg(unix)]
fn restore_write_blocking(
    stream: &std::os::unix::net::UnixStream,
    result: &io::Result<()>,
) -> io::Result<()> {
    let would_block = matches!(result, Err(error) if error.kind() == io::ErrorKind::WouldBlock);
    if would_block {
        stream.set_nonblocking(false)?;
    }
    Ok(())
}

#[cfg(unix)]
fn restore_read_blocking(
    stream: &std::os::unix::net::UnixStream,
    result: &io::Result<usize>,
) -> io::Result<()> {
    let would_block = matches!(result, Err(error) if error.kind() == io::ErrorKind::WouldBlock);
    if would_block {
        stream.set_nonblocking(false)?;
    }
    Ok(())
}

// ===== Unix =====

#[cfg(unix)]
mod imp {
    use super::{Connection, Stream, TransportError};
    use crate::clientd::ipc::BoundEndpoint;
    use std::io;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    /// Remove a socket left behind by a daemon that is no longer running.
    ///
    /// Two guards make this safe without an ownership probe. The path must
    /// actually be a **socket**, so a foreign regular file at the same name is
    /// reported and preserved rather than deleted. And it must be
    /// **unconnectable**: a running daemon's socket still accepts a connection,
    /// so a second daemon can never unlink and steal a live endpoint. A
    /// crashed daemon leaves an unconnectable socket, which is exactly the
    /// garbage this is allowed to reclaim.
    fn remove_stale_socket(path: &Path) -> Result<(), TransportError> {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return Ok(());
        };
        if !metadata.file_type().is_socket() {
            return Err(TransportError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            )));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(TransportError::Io(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("a client daemon is already listening on {}", path.display()),
            )));
        }
        std::fs::remove_file(path)?;
        Ok(())
    }

    /// Bind the listener, preferring the config-adjacent primary location.
    pub fn bind(candidates: &[PathBuf]) -> Result<(BoundEndpoint, UnixListener), TransportError> {
        let mut last: Option<TransportError> = None;
        for (index, path) in candidates.iter().enumerate() {
            if let Some(parent) = path.parent() {
                if !parent.is_dir() {
                    continue;
                }
            }
            match remove_stale_socket(path) {
                Ok(()) => {}
                Err(error) => {
                    // An occupied primary is reported, not silently skipped: the
                    // caller must be able to say "a daemon is already running".
                    if index == 0 {
                        return Err(error);
                    }
                    last = Some(error);
                    continue;
                }
            }
            match UnixListener::bind(path) {
                Ok(listener) => {
                    // Owner-only. The channel is same-user by construction and
                    // is not a privilege boundary, but it must not be world
                    // readable either.
                    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                    return Ok((
                        BoundEndpoint {
                            path: path.clone(),
                            primary: index == 0,
                        },
                        listener,
                    ));
                }
                Err(error) => {
                    last = Some(TransportError::Io(error));
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            TransportError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no usable client-daemon socket location",
            ))
        }))
    }

    /// Connect to the first candidate that answers.
    pub fn connect(candidates: &[PathBuf]) -> Result<Connection, TransportError> {
        let mut last: Option<TransportError> = None;
        for path in candidates {
            match UnixStream::connect(path) {
                Ok(stream) => {
                    stream.set_nonblocking(true)?;
                    return Ok(Connection::new(Stream::Unix(stream)));
                }
                Err(error) => {
                    last = Some(TransportError::Io(error));
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            TransportError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no client-daemon socket location exists for this config",
            ))
        }))
    }

    /// Non-blocking accept of one connection.
    pub fn accept(listener: &UnixListener) -> Result<Connection, TransportError> {
        listener.set_nonblocking(true)?;
        let (stream, _) = listener.accept()?;
        stream.set_nonblocking(true)?;
        Ok(Connection::new(Stream::Unix(stream)))
    }

    /// Remove a socket this daemon owns on shutdown.
    pub fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

// ===== Windows =====

#[cfg(windows)]
mod imp {
    use super::*;

    /// Resolve the named pipe for this config identity.
    pub fn bind(candidates: &[PathBuf]) -> Result<BoundEndpoint, TransportError> {
        // Windows named pipes are not filesystem entries, so there is no stale
        // file to remove and no permission bit to set; the default pipe ACL
        // already limits access to the creating user.
        candidates
            .first()
            .map(|name| BoundEndpoint {
                path: name.clone(),
                primary: true,
            })
            .ok_or_else(|| {
                TransportError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no client-daemon pipe name for this config",
                ))
            })
    }

    pub fn connect(_candidates: &[PathBuf]) -> Result<Connection, TransportError> {
        Err(TransportError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "named-pipe transport is not implemented in this build",
        )))
    }

    pub fn accept() -> Result<Connection, TransportError> {
        Err(TransportError::Io(io::Error::new(
            io::ErrorKind::Unsupported,
            "named-pipe transport is not implemented in this build",
        )))
    }

    pub fn cleanup(_path: &Path) {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::{imp, Connection, Stream, TransportError};
    use crate::clientd::identity::socket_candidates;
    use crate::clientd::protocol::{DaemonRequest, FrontendFrame, HelloPayload, PROTOCOL_VERSION};
    use std::io::Write as _;
    use std::os::unix::net::UnixStream as RawUnixStream;
    use std::path::PathBuf;

    fn temp_candidates(tag: &str) -> Vec<PathBuf> {
        let dir = std::env::temp_dir().join(format!("gregg-ipc-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        vec![dir.join("gregg-client-test.sock")]
    }

    fn hello() -> HelloPayload {
        HelloPayload {
            protocol_version: PROTOCOL_VERSION,
            version: "1.0.0".to_owned(),
            daemon_id: "0123456789abcdef".to_owned(),
            current_generation: 0,
        }
    }

    #[test]
    fn a_request_before_the_handshake_is_refused() {
        let candidates = temp_candidates("nohandshake");
        let (endpoint, listener) = imp::bind(&candidates).expect("binds");
        let _ = listener.set_nonblocking(true);
        let raw = RawUnixStream::connect(&endpoint.path).expect("connects");
        imp::accept(&listener).expect("accepts");
        // Skip the accepted connection's own state; drive `try_read_request`
        // directly with a smuggled request.
        let mut connection = Connection::new(Stream::Unix(raw));
        let frame =
            crate::clientd::protocol::encode_frame(&DaemonRequest::ReloadConfig { generation: 1 })
                .expect("encodes");
        connection.push_bytes(&frame);
        let error = connection.try_read_request().expect_err("must refuse");
        assert!(
            matches!(error, TransportError::Protocol(_)),
            "expected a protocol error, got {error}"
        );
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn a_version_mismatch_is_reported_not_tolerated() {
        let candidates = temp_candidates("mismatch");
        let (endpoint, listener) = imp::bind(&candidates).expect("binds");
        let mut raw = RawUnixStream::connect(&endpoint.path).expect("connects");
        let frame = crate::clientd::protocol::encode_frame(&DaemonRequest::Handshake {
            protocol_version: PROTOCOL_VERSION + 1,
            version: "1.0.0".to_owned(),
        })
        .expect("encodes");
        raw.write_all(&frame).expect("writes");
        let mut accepted = imp::accept(&listener).expect("accepts");
        let mut buf = [0u8; 1024];
        let count = accepted.stream.read(&mut buf).unwrap_or(0);
        accepted.push_bytes(&buf[..count]);
        let error = accepted.try_read_request().expect_err("must refuse");
        match error {
            TransportError::VersionMismatch { daemon, frontend } => {
                assert_eq!(daemon, PROTOCOL_VERSION);
                assert_eq!(frontend, PROTOCOL_VERSION + 1);
            }
            other => panic!("expected a version mismatch, got {other}"),
        }
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn a_matching_handshake_unlocks_the_connection() {
        let candidates = temp_candidates("good");
        let (endpoint, listener) = imp::bind(&candidates).expect("binds");
        let mut raw = RawUnixStream::connect(&endpoint.path).expect("connects");
        let handshake = crate::clientd::protocol::encode_frame(&DaemonRequest::Handshake {
            protocol_version: PROTOCOL_VERSION,
            version: "1.0.0".to_owned(),
        })
        .expect("encodes");
        let reload =
            crate::clientd::protocol::encode_frame(&DaemonRequest::ReloadConfig { generation: 9 })
                .expect("encodes");
        raw.write_all(&handshake).expect("writes handshake");
        raw.write_all(&reload).expect("writes request");

        let mut accepted = imp::accept(&listener).expect("accepts");
        let mut buf = [0u8; 2048];
        let count = accepted.stream.read(&mut buf).expect("reads");
        accepted.push_bytes(&buf[..count]);

        // Both frames arrive in one read: partial buffering must not lose one.
        assert_eq!(
            accepted.try_read_request().expect("reads"),
            Some(DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "1.0.0".to_owned()
            })
        );
        assert!(accepted.is_handshaken());
        assert_eq!(
            accepted.try_read_request().expect("reads"),
            Some(DaemonRequest::ReloadConfig { generation: 9 })
        );
        assert_eq!(
            accepted.try_read_request().expect("reads"),
            None,
            "a drained connection must not block or invent a frame"
        );
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn a_bound_socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let candidates = temp_candidates("perms");
        let (endpoint, _listener) = imp::bind(&candidates).expect("binds");
        let mode = std::fs::metadata(&endpoint.path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "socket mode was {mode:o}");
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn a_stale_socket_from_a_dead_daemon_is_reclaimed() {
        let candidates = temp_candidates("stale");
        let path = candidates[0].clone();
        // A bound-then-dropped listener leaves a socket file behind.
        {
            let _ = imp::bind(&candidates).expect("first bind");
        }
        assert!(
            path.exists(),
            "a socket file should remain after the listener drops"
        );
        // A new daemon must be able to take the path over.
        let (endpoint, _listener) = imp::bind(&candidates).expect("reclaims");
        assert_eq!(endpoint.path, path);
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn a_live_daemons_socket_is_never_stolen() {
        let candidates = temp_candidates("live");
        let (_endpoint, _listener) = imp::bind(&candidates).expect("first bind");
        // A second bind must fail loudly rather than unlinking a live socket.
        let error = imp::bind(&candidates).expect_err("must refuse");
        assert!(
            matches!(error, TransportError::Io(_)),
            "expected an io error, got {error}"
        );
        imp::cleanup(&candidates[0]);
    }

    #[test]
    fn a_foreign_file_at_the_socket_path_is_never_removed() {
        let candidates = temp_candidates("foreign");
        let path = candidates[0].clone();
        std::fs::write(&path, b"not a socket").expect("writes a decoy file");
        let error = imp::bind(&candidates).expect_err("must refuse");
        assert!(
            path.exists(),
            "a non-socket file at the socket path must be preserved"
        );
        assert!(matches!(error, TransportError::Io(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn connecting_reports_that_no_daemon_is_running() {
        let candidates = temp_candidates("nodemon");
        let missing = candidates[0].with_extension("absent");
        let error = imp::connect(&[missing]).expect_err("must refuse");
        assert!(matches!(error, TransportError::Io(_)), "got {error}");
    }

    #[test]
    fn a_client_can_dial_the_bound_socket_and_exchange_frames() {
        let candidates = temp_candidates("exchange");
        let (endpoint, listener) = imp::bind(&candidates).expect("binds");
        let mut client = imp::connect(std::slice::from_ref(&endpoint.path)).expect("connects");
        let mut server = imp::accept(&listener).expect("accepts");

        client
            .write_frame(&DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "1.0.0".to_owned(),
            })
            .expect("writes");
        client
            .write_frame(&FrontendFrame::Hello(Box::new(hello())))
            .expect("writes hello");

        let mut buf = [0u8; 4096];
        let count = server.stream.read(&mut buf).expect("reads");
        server.push_bytes(&buf[..count]);
        assert!(matches!(
            server.try_read_request().expect("reads"),
            Some(DaemonRequest::Handshake { .. })
        ));
        assert!(server.is_handshaken());
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn the_frame_cap_is_enforced_on_the_real_socket_path() {
        let candidates = temp_candidates("cap");
        let (endpoint, _listener) = imp::bind(&candidates).expect("binds");
        let mut client = imp::connect(std::slice::from_ref(&endpoint.path)).expect("connects");
        let error = client
            .write_frame(&"x".repeat(crate::clientd::protocol::MAX_FRAME_BYTES + 1))
            .expect_err("must refuse");
        assert!(matches!(error, TransportError::Protocol(_)), "got {error}");
        imp::cleanup(&endpoint.path);
    }

    #[test]
    fn identity_candidates_are_ordered_primary_first() {
        // A config outside the temp directory yields two distinct locations.
        let candidates = socket_candidates(std::path::Path::new("/var/lib/gregg/gregg-x.toml"));
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].to_string_lossy().contains("gregg-client-"));
        assert_eq!(
            candidates[0].parent().unwrap().to_string_lossy(),
            "/var/lib/gregg"
        );
    }

    #[test]
    fn a_config_inside_the_temp_directory_yields_one_location() {
        // Primary and fallback collapse to the same path; the candidate list is
        // deduplicated so a client never probes the same socket twice.
        let candidates = socket_candidates(std::path::Path::new("/tmp/gregg-x.toml"));
        assert_eq!(candidates.len(), 1);
    }
}
