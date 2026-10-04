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

use std::io::{self, Write as _};
// Only the Unix arm of `Stream` reads through the `Read` trait; the Windows arm
// goes through `PeekNamedPipe`/`ReadFile`, and an unused import here would be a
// warning, which CI treats as an error.
#[cfg(unix)]
use std::io::Read as _;
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
    /// Nothing is available right now.
    ///
    /// Distinct from [`Self::Disconnected`]: the descriptor is non-blocking, so
    /// "no bytes yet" is the normal state of an idle connection and must never
    /// be mistaken for end of file.
    WouldBlock,
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
    /// The peer is a Gregg client daemon, but for a different config.
    ConfigMismatch {
        /// Identity the peer is serving.
        actual: String,
        /// Identity the caller asked for.
        expected: String,
    },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disconnected => f.write_str("peer disconnected"),
            Self::WouldBlock => f.write_str("no data available"),
            Self::Decode(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::VersionMismatch { daemon, frontend } => write!(
                f,
                "client daemon speaks protocol {daemon}, this frontend speaks {frontend}"
            ),
            Self::ConfigMismatch { actual, expected } => write!(
                f,
                "endpoint is serving client daemon {actual}, not the requested {expected}"
            ),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::WouldBlock {
            Self::WouldBlock
        } else {
            Self::Io(error)
        }
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
    /// True once a compatible handshake frame has been parsed.
    handshaken: bool,
    /// The config identity the peer claimed in its handshake.
    ///
    /// Parsing the frame and checking the claim are separate steps because
    /// only the daemon knows which config *it* is serving. A mismatched claim
    /// is refused in [`Connection::verify_daemon_id`], not silently ignored.
    claimed_daemon_id: Option<String>,
}

impl Connection {
    fn new(stream: Stream) -> Self {
        Self {
            stream,
            buffer: Vec::with_capacity(8192),
            handshaken: false,
            claimed_daemon_id: None,
        }
    }

    /// Whether a compatible handshake has completed.
    #[must_use]
    pub fn is_handshaken(&self) -> bool {
        self.handshaken
    }

    /// Check the peer's claimed config identity against this daemon's own.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::ConfigMismatch`] when the peer is a Gregg
    /// client daemon serving a different configuration. A peer that never
    /// handshaked at all is a protocol error, not a match.
    pub fn verify_daemon_id(&self, expected: &str) -> Result<(), TransportError> {
        let Some(claimed) = self.claimed_daemon_id.as_deref() else {
            return Err(TransportError::Protocol(
                "no handshake identity was presented".to_owned(),
            ));
        };
        if claimed == expected {
            Ok(())
        } else {
            Err(TransportError::ConfigMismatch {
                actual: claimed.to_owned(),
                expected: expected.to_owned(),
            })
        }
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
                    protocol_version,
                    daemon_id,
                    ..
                } if *protocol_version == PROTOCOL_VERSION => {
                    // The identity check is the server's, because only the
                    // server knows which config it is serving. Record the
                    // claim here and let `try_handshake` compare it.
                    self.claimed_daemon_id = Some(daemon_id.clone());
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

    /// Take one complete frame of an arbitrary type from the buffer.
    ///
    /// This is the framing half of [`Self::try_read_request`], exposed so a
    /// frontend can read daemon-pushed frames with the same length-prefix
    /// discipline (and the same [`crate::clientd::protocol::MAX_FRAME_BYTES`]
    /// bound) instead of re-deriving it.
    ///
    /// Returns `Ok(None)` when more bytes are needed.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] for a malformed prefix, an oversized
    /// announcement, or a body that is not a valid `T`.
    pub fn try_read_frame<T: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<Option<T>, TransportError> {
        if self.buffer.len() < LEN_PREFIX_BYTES {
            return Ok(None);
        }
        let len = parse_length(&self.buffer[..LEN_PREFIX_BYTES])?;
        let end = LEN_PREFIX_BYTES.saturating_add(len);
        if self.buffer.len() < end {
            return Ok(None);
        }
        let body: Vec<u8> = self.buffer.drain(..end).skip(LEN_PREFIX_BYTES).collect();
        let value = serde_json::from_slice(&body)
            .map_err(|error| TransportError::Protocol(error.to_string()))?;
        Ok(Some(value))
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

    /// Write an already-encoded frame.
    ///
    /// The daemon encodes each state document once and hands the same bytes to
    /// every attached frontend, so publication cost does not scale with the
    /// number of open TUI windows. The bytes must be exactly what
    /// [`crate::clientd::protocol::encode_frame`] would have produced.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] when the peer is gone.
    pub fn write_encoded(&mut self, encoded: &[u8]) -> Result<(), TransportError> {
        self.stream.write_all(encoded)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Read whatever bytes are available right now.
    ///
    /// Returns [`TransportError::WouldBlock`] when the peer has nothing
    /// buffered yet, which is the normal state of an idle non-blocking socket
    /// and is emphatically not a disconnect. `Ok(0)` means end of file, and
    /// is reported as [`TransportError::Disconnected`].
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] on an OS-level failure or at end of file.
    pub fn read_available(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let count = self.stream.read(buf)?;
        if count == 0 {
            return Err(TransportError::Disconnected);
        }
        Ok(count)
    }
}

/// A platform stream, hiding the Unix/Windows difference.
#[derive(Debug)]
pub(crate) enum Stream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    /// One end of a named pipe. `File` is the owning wrapper for the raw
    /// `HANDLE`, so the handle has exactly one owner and is closed when the
    /// stream is dropped.
    #[cfg(windows)]
    Windows(std::fs::File),
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
            Self::Windows(pipe) => read_pipe(pipe, buf),
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
            Self::Windows(pipe) => pipe.flush(),
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
            Self::Windows(pipe) => pipe.write_all(buf),
        }
    }
}

/// Read the bytes already queued in a byte-mode named pipe, without blocking.
///
/// A pipe created in synchronous mode has no non-blocking read, and the client
/// daemon runs on a **current-thread** runtime where one blocking read would
/// stall polling, cron, and every other frontend at the same time. So the
/// available count comes from `PeekNamedPipe`, which returns immediately, and
/// `ReadFile` is then asked for at most that many bytes and at most what the
/// caller's buffer holds — so it has nothing left to wait for.
///
/// `Ok(0)` means end of stream. A closed peer surfaces as `ERROR_BROKEN_PIPE`
/// or `ERROR_NO_DATA` rather than a zero-length read, and
/// [`Connection::read_available`] turns that into
/// [`TransportError::Disconnected`], exactly as a zero-length Unix read does.
#[cfg(windows)]
#[allow(unsafe_code)] // windows-sys PeekNamedPipe/ReadFile and raw-handle reads.
fn read_pipe(pipe: &std::fs::File, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    /// Every "the other end is gone" code, as a comparison value.
    const fn closed(code: i32) -> bool {
        code == ERROR_BROKEN_PIPE.cast_signed()
            || code == ERROR_NO_DATA.cast_signed()
            || code == ERROR_PIPE_NOT_CONNECTED.cast_signed()
    }

    if buf.is_empty() {
        return Ok(0);
    }
    let mut available: u32 = 0;
    // SAFETY: `pipe` owns a live pipe handle for the whole call. Passing null
    // for the peek buffer, its size, and the message-boundary out-parameter is
    // documented as meaning "I only want the total byte count", and
    // `available` is a live `u32` for the call to write into.
    let peeked = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &raw mut available,
            std::ptr::null_mut(),
        )
    };
    if peeked == 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error().is_some_and(closed) {
            Ok(0)
        } else {
            Err(error)
        };
    }
    if available == 0 {
        // Connected, but nothing has been written yet. Not a disconnect.
        return Err(io::Error::from(io::ErrorKind::WouldBlock));
    }
    // Ask for no more than is queued *and* no more than fits. Asking for less
    // than the queued count is deliberate: it keeps `ERROR_MORE_DATA` out of
    // the picture, and the leftover bytes are simply read on the next poll.
    let want = available.min(u32::try_from(buf.len()).unwrap_or(u32::MAX));
    let mut read: u32 = 0;
    // SAFETY: `buf` is a live, exclusively borrowed slice of `want` writable
    // bytes, `read` is a live `u32` out-parameter, and a null `OVERLAPPED` is
    // required for this handle because it was created without
    // `FILE_FLAG_OVERLAPPED`.
    let ok = unsafe {
        ReadFile(
            pipe.as_raw_handle(),
            buf.as_mut_ptr(),
            want,
            &raw mut read,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error().is_some_and(closed) {
            Ok(0)
        } else {
            Err(error)
        };
    }
    Ok(read as usize)
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

    /// A bound listener plus the endpoint clients must dial.
    #[derive(Debug)]
    pub struct Listener {
        inner: UnixListener,
        endpoint: BoundEndpoint,
    }

    impl Listener {
        /// Where this listener is reachable.
        pub fn endpoint(&self) -> &BoundEndpoint {
            &self.endpoint
        }

        /// Accept one connection, or report that none is waiting.
        ///
        /// `async` only so that one signature can serve both platforms: a
        /// Windows accept has to park a blocking `ConnectNamedPipe` somewhere
        /// that cannot stall the runtime, while a Unix accept is a
        /// non-blocking syscall that has nothing to await. Clippy has renamed
        /// this lint once already, so both spellings are allowed.
        #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
        pub async fn accept(&mut self) -> Result<Connection, TransportError> {
            self.inner.set_nonblocking(true)?;
            let (stream, _) = self.inner.accept()?;
            stream.set_nonblocking(true)?;
            Ok(Connection::new(Stream::Unix(stream)))
        }
    }

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
    pub fn bind(candidates: &[PathBuf]) -> Result<Listener, TransportError> {
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
                    return Ok(Listener {
                        inner: listener,
                        endpoint: BoundEndpoint {
                            path: path.clone(),
                            primary: index == 0,
                        },
                    });
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

    /// Remove a socket this daemon owns on shutdown.
    pub fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// True while a daemon is still holding this endpoint.
    ///
    /// A socket file outlives a crashed daemon, so its presence is the honest
    /// answer to "is anything still bound here".
    pub fn endpoint_is_live(path: &Path) -> bool {
        path.exists()
    }
}

// ===== Windows =====

#[cfg(windows)]
mod imp {
    use super::*;
    use std::path::Path;

    /// Owner-only DACL: full access for the object's owner and nobody else.
    ///
    /// A null security descriptor would inherit the process token's default
    /// DACL, which normally also admits `SYSTEM` and `Administrators`. The
    /// client daemon is a per-user background process, so the narrower owner
    /// DACL is both the plan's "current-user access only" rule and the smallest
    /// surface that cannot be widened by a permissive process token. `P` marks
    /// the DACL protected, so no parent ACEs are inherited.
    const OWNER_ONLY_SDDL: &str = "D:P(A;;GA;;;OW)";

    /// Instances to keep pre-created for this name.
    ///
    /// A named pipe serves one client at a time, so the listener always keeps
    /// one instance waiting and creates the next on accept. This is only the
    /// instance count passed to `CreateNamedPipeW`; it is not a limit this
    /// process enforces.
    const PIPE_MAX_INSTANCES: u32 = 255;

    /// In-buffer and out-buffer size for each instance, in bytes.
    ///
    /// Large enough that a full snapshot frame is written without the writer
    /// interleaving with the reader's next poll.
    const PIPE_BUFFER_BYTES: u32 = 65_536;

    /// `OWNER_ONLY_SDDL` as the NUL-terminated UTF-16 the `W` APIs take.
    fn wide_sddl() -> Vec<u16> {
        OWNER_ONLY_SDDL
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    /// The pipe name as a NUL-terminated UTF-16 string.
    ///
    /// The name is built from this crate's own config identity, so it is
    /// representable; a name that is not is reported rather than mangled into
    /// a different pipe.
    fn wide_pipe_name(name: &Path) -> Result<Vec<u16>, TransportError> {
        let name = name.to_str().ok_or_else(|| {
            TransportError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pipe name is not valid UTF-8",
            ))
        })?;
        Ok(name.encode_utf16().chain(std::iter::once(0)).collect())
    }

    /// A bound named-pipe listener.
    ///
    /// The first instance is created in [`Listener::new`]; further instances
    /// share the same name and are created on demand by [`Listener::accept`],
    /// which is the only way Windows multiplexes them.
    #[derive(Debug)]
    pub struct Listener {
        name: PathBuf,
        endpoint: BoundEndpoint,
        /// Handle to the pending instance, rotated by `accept`.
        pending: Option<Stream>,
    }

    impl Listener {
        /// Create the first instance of the named pipe.
        pub fn new(name: PathBuf) -> Result<Self, TransportError> {
            let pending = create_instance(&name)?;
            let endpoint = BoundEndpoint {
                path: name.clone(),
                primary: true,
            };
            Ok(Self {
                name,
                endpoint,
                pending: Some(pending),
            })
        }

        /// Where clients must connect.
        pub fn endpoint(&self) -> &BoundEndpoint {
            &self.endpoint
        }

        /// Accept one connection.
        ///
        /// Unlike the Unix listener this does not report "nothing waiting", and
        /// does not need to: the wait happens on the blocking pool (see
        /// [`connect_instance`]), so parking here costs the runtime nothing.
        pub async fn accept(&mut self) -> Result<Connection, TransportError> {
            let Some(stream) = self.pending.take() else {
                return Err(TransportError::Disconnected);
            };
            // Rotate a fresh pending instance first: a named pipe only accepts
            // one client at a time, so the next arrival must find a waiting
            // instance or it is refused outright.
            self.pending = Some(create_instance(&self.name)?);
            let stream = connect_instance(stream).await?;
            Ok(Connection::new(stream))
        }
    }

    /// Wait for a client on this instance without stalling the runtime.
    ///
    /// `ConnectNamedPipe` on a synchronous handle blocks until a client
    /// connects, and the client daemon runs on a current-thread runtime where
    /// that would freeze polling, cron, and every other frontend at once. The
    /// blocking call therefore runs on the blocking pool, the one place it
    /// cannot take the runtime down with it. The instance is *moved* into the
    /// closure, so the handle keeps a single owner for the whole wait and is
    /// closed if the wait is ever abandoned.
    ///
    /// `ERROR_PIPE_CONNECTED` (535) means a client won the race between
    /// instance creation and this call. That is a success, not a failure.
    #[allow(unsafe_code)] // windows-sys ConnectNamedPipe on the owned handle.
    async fn connect_instance(stream: Stream) -> Result<Stream, TransportError> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::ERROR_PIPE_CONNECTED;
        use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

        let Stream::Windows(pipe) = stream;
        tokio::task::spawn_blocking(move || {
            // SAFETY: `pipe` owns a live named-pipe server handle for the whole
            // closure, and a null `OVERLAPPED` selects the synchronous form of
            // the API, which is the form this handle was created for.
            let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut()) };
            if ok == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED.cast_signed()) {
                    return Err(TransportError::Io(error));
                }
            }
            Ok(Stream::Windows(pipe))
        })
        .await
        .map_err(|_| TransportError::Disconnected)?
    }

    /// Create one pipe instance with the owner-only DACL.
    #[allow(unsafe_code)] // windows-sys named-pipe, SDDL, and raw-handle calls.
    fn create_instance(name: &Path) -> Result<Stream, TransportError> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::{LocalFree, HLOCAL, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_WAIT,
        };

        let wide = wide_pipe_name(name)?;
        // The conversion *allocates* the descriptor and returns a pointer to it
        // through its out-parameter, so the out-parameter is a pointer-sized
        // slot. There is no caller-supplied string buffer to size here.
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `wide` and `wide_sddl()` are NUL-terminated, `descriptor` is a
        // live pointer-sized out-slot, and the size out-parameter is documented
        // as optional so null is correct. The descriptor the API allocates is
        // self-relative and is only valid until `LocalFree`, so it is released
        // in this same block, after `CreateNamedPipeW` has copied what it needs.
        let stream = unsafe {
            let ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide_sddl().as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                return Err(TransportError::Io(io::Error::last_os_error()));
            }
            let attributes = SECURITY_ATTRIBUTES {
                nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
                    .unwrap_or(u32::MAX),
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            // `PIPE_REJECT_REMOTE_CLIENTS` is a pipe-*mode* flag, so it belongs
            // in `dwpipemode`. Folding it into `dwopenmode` instead would OR
            // incompatible access rights together and leave the pipe reachable
            // off-box. `FILE_FLAG_OVERLAPPED` is deliberately *not* set: this
            // transport reads with `PeekNamedPipe`, and the blocking accept is
            // moved off the runtime instead.
            let handle = CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_MAX_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                std::ptr::from_ref(&attributes),
            );
            LocalFree(descriptor as HLOCAL);
            if handle == INVALID_HANDLE_VALUE {
                return Err(TransportError::Io(io::Error::last_os_error()));
            }
            // SAFETY: `CreateNamedPipeW` returned a fresh handle that nothing
            // else owns, and `File` takes that ownership and closes it on drop.
            Stream::Windows(std::fs::File::from_raw_handle(handle))
        };
        Ok(stream)
    }

    /// Bind the listener for this config identity.
    pub fn bind(candidates: &[PathBuf]) -> Result<Listener, TransportError> {
        let name = candidates.first().ok_or_else(|| {
            TransportError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no client-daemon pipe name for this config",
            ))
        })?;
        Listener::new(name.clone())
    }

    /// Connect to the named pipe.
    #[allow(unsafe_code)] // windows-sys CreateFileW and raw-handle adoption.
    pub fn connect(candidates: &[PathBuf]) -> Result<Connection, TransportError> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};

        let name = candidates.first().ok_or_else(|| {
            TransportError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "no client-daemon pipe name for this config",
            ))
        })?;
        let wide = wide_pipe_name(name)?;
        // A client handle needs read and write access to the owner-only pipe it
        // created itself; anything less fails closed rather than downgrading.
        // SAFETY: `wide` is NUL-terminated; a share mode of 0 is correct
        // because this process is the only opener of the client end; null
        // `SECURITY_ATTRIBUTES` and a null template handle are both documented
        // as acceptable when opening an existing object; and the returned
        // handle is adopted by the owning `File` on the next line.
        let stream = unsafe {
            let handle = CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(TransportError::Io(io::Error::last_os_error()));
            }
            Stream::Windows(std::fs::File::from_raw_handle(handle))
        };
        Ok(Connection::new(stream))
    }

    /// Named pipes are not filesystem entries, so there is nothing to unlink.
    pub fn cleanup(_path: &Path) {}

    /// True while a daemon is still holding this endpoint.
    ///
    /// A pipe name is never a file, so `path.exists()` would answer "no" for a
    /// daemon that is running and make every caller that waits for the endpoint
    /// to disappear return immediately. `WaitNamedPipeW` with a zero timeout is
    /// the non-destructive equivalent: it reports whether an instance is
    /// available right now without connecting to it and without waiting, so the
    /// listener's one always-pending instance is exactly what it detects.
    #[allow(unsafe_code)] // windows-sys WaitNamedPipeW on a borrowed name.
    pub fn endpoint_is_live(path: &Path) -> bool {
        use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
        let Ok(wide) = wide_pipe_name(path) else {
            return false;
        };
        // SAFETY: `wide` is NUL-terminated and outlives the call, and a zero
        // timeout makes this a pure poll that cannot block.
        unsafe { WaitNamedPipeW(wide.as_ptr(), 0) != 0 }
    }
}

// The daemon and the frontend client are written once, against this uniform
// `bind`/`connect`/`accept`/`cleanup` surface, so neither carries a
// `#[cfg]` of its own.
pub use imp::{bind, cleanup, connect, endpoint_is_live, Listener};

#[cfg(all(test, unix))]
mod tests {
    use super::{bind, cleanup, connect, Connection, Stream, TransportError};
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

    #[tokio::test]
    async fn a_request_before_the_handshake_is_refused() {
        let candidates = temp_candidates("nohandshake");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let raw = RawUnixStream::connect(&endpoint.path).expect("connects");
        listener.accept().await.expect("accepts");
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
        cleanup(&endpoint.path);
    }

    #[tokio::test]
    async fn a_version_mismatch_is_reported_not_tolerated() {
        let candidates = temp_candidates("mismatch");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let mut raw = RawUnixStream::connect(&endpoint.path).expect("connects");
        let frame = crate::clientd::protocol::encode_frame(&DaemonRequest::Handshake {
            protocol_version: PROTOCOL_VERSION + 1,
            version: "1.0.0".to_owned(),
            daemon_id: "0123456789abcdef".to_owned(),
        })
        .expect("encodes");
        raw.write_all(&frame).expect("writes");
        let mut accepted = listener.accept().await.expect("accepts");
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
        cleanup(&endpoint.path);
    }

    #[tokio::test]
    async fn a_matching_handshake_unlocks_the_connection() {
        let candidates = temp_candidates("good");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let mut raw = RawUnixStream::connect(&endpoint.path).expect("connects");
        let handshake = crate::clientd::protocol::encode_frame(&DaemonRequest::Handshake {
            protocol_version: PROTOCOL_VERSION,
            version: "1.0.0".to_owned(),
            daemon_id: "0123456789abcdef".to_owned(),
        })
        .expect("encodes");
        let reload =
            crate::clientd::protocol::encode_frame(&DaemonRequest::ReloadConfig { generation: 9 })
                .expect("encodes");
        raw.write_all(&handshake).expect("writes handshake");
        raw.write_all(&reload).expect("writes request");

        let mut accepted = listener.accept().await.expect("accepts");
        let mut buf = [0u8; 2048];
        let count = accepted.stream.read(&mut buf).expect("reads");
        accepted.push_bytes(&buf[..count]);

        // Both frames arrive in one read: partial buffering must not lose one.
        assert_eq!(
            accepted.try_read_request().expect("reads"),
            Some(DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "1.0.0".to_owned(),
                daemon_id: "0123456789abcdef".to_owned(),
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
        cleanup(&endpoint.path);
    }

    #[test]
    fn a_bound_socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let candidates = temp_candidates("perms");
        let listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let mode = std::fs::metadata(&endpoint.path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "socket mode was {mode:o}");
        cleanup(&endpoint.path);
    }

    #[test]
    fn a_stale_socket_from_a_dead_daemon_is_reclaimed() {
        let candidates = temp_candidates("stale");
        let path = candidates[0].clone();
        // A bound-then-dropped listener leaves a socket file behind.
        {
            let _first = bind(&candidates).expect("first bind");
        }
        assert!(
            path.exists(),
            "a socket file should remain after the listener drops"
        );
        // A new daemon must be able to take the path over.
        let reclaimed = bind(&candidates).expect("reclaims");
        let endpoint = reclaimed.endpoint().clone();
        assert_eq!(endpoint.path, path);
        cleanup(&endpoint.path);
    }

    #[test]
    fn a_live_daemons_socket_is_never_stolen() {
        let candidates = temp_candidates("live");
        let _first = bind(&candidates).expect("first bind");
        // A second bind must fail loudly rather than unlinking a live socket.
        let error = bind(&candidates).expect_err("must refuse");
        assert!(
            matches!(error, TransportError::Io(_)),
            "expected an io error, got {error}"
        );
        cleanup(&candidates[0]);
    }

    #[test]
    fn a_foreign_file_at_the_socket_path_is_never_removed() {
        let candidates = temp_candidates("foreign");
        let path = candidates[0].clone();
        std::fs::write(&path, b"not a socket").expect("writes a decoy file");
        let error = bind(&candidates).expect_err("must refuse");
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
        let error = connect(&[missing]).expect_err("must refuse");
        assert!(matches!(error, TransportError::Io(_)), "got {error}");
    }

    #[tokio::test]
    async fn a_client_can_dial_the_bound_socket_and_exchange_frames() {
        let candidates = temp_candidates("exchange");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let mut client = connect(std::slice::from_ref(&endpoint.path)).expect("connects");
        let mut server = listener.accept().await.expect("accepts");

        client
            .write_frame(&DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "1.0.0".to_owned(),
                daemon_id: "0123456789abcdef".to_owned(),
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
        cleanup(&endpoint.path);
    }

    #[test]
    fn the_frame_cap_is_enforced_on_the_real_socket_path() {
        let candidates = temp_candidates("cap");
        let listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let mut client = connect(std::slice::from_ref(&endpoint.path)).expect("connects");
        let error = client
            .write_frame(&"x".repeat(crate::clientd::protocol::MAX_FRAME_BYTES + 1))
            .expect_err("must refuse");
        assert!(matches!(error, TransportError::Protocol(_)), "got {error}");
        cleanup(&endpoint.path);
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

// The Windows transport had never been compiled, let alone run, until this
// module existed. These tests are the only thing that exercises the named-pipe
// path end to end: the SDDL descriptor, `CreateNamedPipeW`, the blocking
// `ConnectNamedPipe` that is parked on the blocking pool, `CreateFileW` on the
// client side, and the `PeekNamedPipe`/`ReadFile` pair that stands in for a
// non-blocking read. A test that cannot fail is worthless, so each one asserts
// an observable outcome rather than merely that the calls returned.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::{bind, cleanup, connect, endpoint_is_live, Connection, TransportError};
    use crate::clientd::protocol::{DaemonRequest, HelloPayload, PROTOCOL_VERSION};
    use crate::clientd::FrontendFrame;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// A pipe name unique to this test binary and tag.
    ///
    /// Named pipes are a machine-global namespace, so the name has to be
    /// unlikely to collide with a developer's own running daemon.
    fn pipe_candidates(tag: &str) -> Vec<PathBuf> {
        vec![PathBuf::from(format!(
            r"\\.\pipe\gregg-test-{tag}-{}",
            std::process::id()
        ))]
    }

    fn hello() -> FrontendFrame {
        FrontendFrame::Hello(Box::new(HelloPayload {
            protocol_version: PROTOCOL_VERSION,
            version: "1.0.0".to_owned(),
            daemon_id: "0123456789abcdef".to_owned(),
            current_generation: 0,
        }))
    }

    /// Poll `read_available` until it yields a frame or the deadline passes.
    async fn read_request(connection: &mut Connection, what: &str) -> Option<DaemonRequest> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buffer = [0_u8; 4096];
        while Instant::now() < deadline {
            if let Some(frame) = connection.try_read_request().expect("decodes") {
                return Some(frame);
            }
            match connection.read_available(&mut buffer) {
                Ok(count) => connection.push_bytes(&buffer[..count]),
                Err(TransportError::WouldBlock) => {}
                Err(error) => panic!("{what}: read failed: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        None
    }

    #[tokio::test]
    async fn a_client_and_daemon_exchange_frames_over_a_real_named_pipe() {
        let candidates = pipe_candidates("exchange");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        assert!(
            endpoint_is_live(&endpoint.path),
            "a bound pipe must report as live"
        );

        let mut client = connect(std::slice::from_ref(&endpoint.path)).expect("connects");
        let mut server = listener.accept().await.expect("accepts");

        // The handshake must be the first frame and must unlock the connection.
        client
            .write_frame(&DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "1.0.0".to_owned(),
                daemon_id: "0123456789abcdef".to_owned(),
            })
            .expect("writes handshake");
        client.write_frame(&hello()).expect("writes hello");

        assert!(matches!(
            read_request(&mut server, "server").await,
            Some(DaemonRequest::Handshake { .. })
        ));
        assert!(server.is_handshaken());

        // And the reply has to come back the other way, which is the only proof
        // that the write path and the peek/read path both work.
        server
            .write_frame(&FrontendFrame::ControlAck {
                generation: 7,
                accepted: true,
                detail: None,
            })
            .expect("writes reply");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buffer = [0_u8; 4096];
        let mut seen = None;
        while Instant::now() < deadline && seen.is_none() {
            match client.read_available(&mut buffer) {
                Ok(count) => client.push_bytes(&buffer[..count]),
                Err(TransportError::WouldBlock) => {}
                Err(error) => panic!("client read failed: {error}"),
            }
            if let Some(frame) = client.try_read_frame().expect("decodes") {
                seen = Some(frame);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            matches!(
                seen,
                Some(FrontendFrame::ControlAck {
                    generation: 7,
                    accepted: true,
                    ..
                })
            ),
            "the daemon's reply never reached the client: {seen:?}"
        );
        cleanup(&endpoint.path);
    }

    #[tokio::test]
    async fn an_accept_waits_for_a_client_that_connects_later() {
        // The client here arrives *after* the accept is already parked, which
        // is the ordering that exercises the blocking `ConnectNamedPipe` on the
        // blocking pool rather than the `ERROR_PIPE_CONNECTED` shortcut.
        let candidates = pipe_candidates("late");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();

        let client_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            connect(std::slice::from_ref(&endpoint.path))
        });

        let server = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .expect("accept must not hang")
            .expect("accepts");
        // The runtime must still be usable while the accept is parked: a
        // current-thread runtime that blocked here would not reach this line
        // until the client arrived, and the sleep below would never run.
        let client = client_thread
            .join()
            .expect("client thread")
            .expect("connects");
        drop(client);
        drop(server);
        cleanup(&listener.endpoint().path);
    }

    #[tokio::test]
    async fn a_closed_pipe_reports_a_disconnect_not_an_error() {
        // `read_available` must turn a vanished peer into `Disconnected`, or
        // the daemon would log a protocol failure every time a TUI exits.
        let candidates = pipe_candidates("closed");
        let mut listener = bind(&candidates).expect("binds");
        let endpoint = listener.endpoint().clone();
        let client = connect(std::slice::from_ref(&endpoint.path)).expect("connects");
        let mut server = listener.accept().await.expect("accepts");
        drop(client);

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buffer = [0_u8; 64];
        loop {
            // Data and "nothing yet" are both fine here; only a real fault or
            // a closed pipe ends the wait.
            match server.read_available(&mut buffer) {
                Err(TransportError::Disconnected) => break,
                Err(TransportError::WouldBlock) | Ok(_) => {}
                Err(other) => panic!("expected a disconnect, got {other}"),
            }
            assert!(
                Instant::now() < deadline,
                "a closed pipe never reported a disconnect"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cleanup(&endpoint.path);
    }

    #[test]
    fn connecting_reports_that_no_daemon_is_running() {
        let candidates = pipe_candidates("absent");
        let missing = candidates[0].with_extension("absent");
        let error = connect(&[missing]).expect_err("must refuse");
        assert!(matches!(error, TransportError::Io(_)), "got {error}");
    }
}
