//! Local Unix-domain control socket for `greggd`.
//!
//! The control socket carries one fixed-size command (`STOP\n`) between an
//! operator shell and a foreground `greggd run` instance. It is intentionally
//! small: there is no JSON, no version negotiation, no general RPC surface,
//! and no authentication beyond the local Unix socket peer check (root
//! privileges are required to impersonate another user).
//!
//! The HTTP API remains read-only and is unrelated to this module.
//!
//! ## Path selection
//!
//! Two candidates are derived from the resolved daemon configuration path:
//!
//! 1. **Primary (config-adjacent)**: `<config_parent>/greggd-<id>.control.sock`,
//!    where `<id>` is a deterministic 64-bit FNV-1a hex digest of the
//!    normalized config identity path. This is preferred because the packaged Linux
//!    service writes its config to `/etc/gregg/` (writable by the daemon user)
//!    and systemd's `PrivateTmp=true` would otherwise isolate a `/tmp`-only
//!    fallback from the operator's CLI.
//! 2. **Fallback (temp dir)**: `<temp_dir>/greggd-<id>.control.sock`, using
//!    the same `<id>` digest. Used when the config parent directory is not
//!    writable by the daemon user (for example when running `greggd run`
//!    from a non-root account while reading an operator-installed config).
//!
//! The `<id>` is derived from the normalized config identity path only.
//! Existing files use their filesystem-canonical path, so relative, absolute,
//! and symlink spellings of the same file converge. A missing implicit default
//! uses a deterministic lexical absolute path instead. Editing `host` or
//! `port` inside the same TOML does not change the `<id>`, so the daemon
//! continues to advertise `greggd stop` at the same path. Two different config
//! files in the same directory produce different `<id>` values, so
//! `greggd --config B stop` cannot reach a daemon launched from config A merely
//! because the two configs share a parent directory.
//!
//! `run` selects whichever path it can actually bind. `stop` tries the
//! primary first and falls back to the deterministic temp path if the
//! primary is missing or unreachable.
//!
//! ## Protocol
//!
//! ```text
//! client -> STOP\n
//! daemon -> OK\n
//! ```
//!
//! The daemon accepts only an exact `STOP\n` line. Anything else (overlong
//! input, extra whitespace, partial lines, malformed commands) is dropped
//! after reading a small bounded prefix and the connection is closed.
//!
//! The daemon never mutates the HTTP API or TOML configuration through this
//! socket.

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

/// Maximum length of the `sun_path` field on Unix-domain sockets.
///
/// Linux `UNIX_PATH_MAX` is 108. macOS uses a similar bound. We leave a
/// small margin for a trailing NUL terminator.
const UNIX_PATH_MAX: usize = 108;

/// Maximum bytes read from a single control connection before closing it
/// without responding. The `STOP\n` command is 5 bytes; we use a small
/// bound so malformed clients cannot hold the connection open.
pub(crate) const MAX_CONTROL_REQUEST_BYTES: usize = 32;

/// Maximum bytes written in a single control response. `OK\n` is 3 bytes.
const MAX_CONTROL_RESPONSE_BYTES: usize = 16;

/// Wire-level STOP command. Terminated with `\n`.
pub const STOP_COMMAND: &[u8] = b"STOP\n";

/// Wire-level OK acknowledgement. Terminated with `\n`.
pub const OK_RESPONSE: &[u8] = b"OK\n";

/// Errors returned by the control socket helpers.
#[derive(Debug, Error)]
pub enum ControlError {
    /// The control socket path would exceed the OS-level `sun_path` limit.
    #[error("control socket path {0:?} exceeds UNIX_PATH_MAX ({UNIX_PATH_MAX} bytes)")]
    PathTooLong(PathBuf),

    /// A filesystem operation failed while preparing the control socket.
    #[error("control socket filesystem error: {0}")]
    Io(#[from] std::io::Error),

    /// The peer did not respond with the expected acknowledgement.
    #[error("control socket returned unexpected acknowledgement")]
    BadResponse,
}

/// Errors returned when the foreground daemon cannot establish a secure
/// control listener. The runtime error variant is intended to surface a
/// clear diagnostic instead of silently starting a daemon that advertises
/// `greggd stop` but cannot be controlled by it.
#[derive(Debug, Error)]
pub enum ControlSetupError {
    /// Neither candidate control-socket path could be bound with
    /// restrictive `0600` permissions. The variant retains both attempted
    /// paths so the operator can inspect or correct the filesystem state.
    #[error(
        "could not bind a restrictive greggd control socket; \
         primary {primary:?}, fallback {fallback:?}. \
         Run greggd from a directory the daemon user can own, or fix \
         permissions on the temp directory."
    )]
    NoSecureControl {
        /// The config-adjacent candidate path, if one was computed.
        primary: Option<PathBuf>,
        /// The temp-dir fallback candidate path, if one was computed.
        fallback: Option<PathBuf>,
    },
    /// Failed to register a Unix signal handler during shutdown-source setup.
    #[error("failed to register signal handler: {0}")]
    SignalRegistration(#[from] std::io::Error),
}

/// Compute a stable 64-bit FNV-1a hex digest for the given config identity.
///
/// The digest is computed over a normalized representation of the path so that
/// two operators using the same config file observe the same control-socket
/// filename across runs. Existing files are normalized with filesystem
/// canonicalization; absent paths use a lexical absolute fallback. The
/// algorithm is FNV-1a
/// (Fowler-Noll-Vo) using the standard 64-bit offset basis and prime, which
/// is deliberately stable across Rust releases (unlike `DefaultHasher`,
/// whose algorithm is not a compatibility contract).
///
/// The result is rendered as 16 lowercase hex characters and never depends
/// on host/port fields, the current PID, the system time, or any random
/// source.
#[must_use]
pub fn config_id_for_path(config_path: &Path) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let bytes = control_identity_path(config_path);
    let bytes = bytes.as_os_str().as_bytes();
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

/// Normalize a config path for control-socket identity.
///
/// Existing paths are filesystem-canonicalized so relative, absolute, and
/// symlink spellings of one file converge. A missing path is made absolute and
/// normalized lexically without requiring the file to exist; this preserves
/// the supported implicit-default configuration behavior.
fn control_identity_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Compute the primary config-adjacent control socket path.
///
/// Returns `<config_parent>/greggd-<id>.control.sock` where `<id>` is the
/// [`config_id_for_path`] digest of the resolved config path. The daemon
/// uses this when the parent is writable by its own user; the client uses
/// it as the first candidate.
#[must_use]
pub fn primary_control_path(config_path: &Path) -> Option<PathBuf> {
    let identity_path = control_identity_path(config_path);
    let parent = identity_path.parent()?;
    let path = parent.join(format!(
        "greggd-{}.control.sock",
        config_id_for_path(config_path)
    ));
    if path.as_os_str().len() > UNIX_PATH_MAX {
        return None;
    }
    Some(path)
}

/// Compute the deterministic fallback control socket path.
///
/// The fallback lives under the standard system temp directory and shares
/// the same `<id>` digest as the primary, so the two paths always agree on
/// the daemon's identity regardless of host/port edits inside the TOML.
#[must_use]
pub fn fallback_control_path(config_path: &Path) -> Option<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "greggd-{}.control.sock",
        config_id_for_path(config_path)
    ));
    if path.as_os_str().len() > UNIX_PATH_MAX {
        return None;
    }
    Some(path)
}

/// All candidates `stop` should try, in priority order.
#[must_use]
pub fn stop_candidates(config_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    if let Some(primary) = primary_control_path(config_path) {
        out.push(primary);
    }
    if let Some(fallback) = fallback_control_path(config_path) {
        if !out.iter().any(|p| p == &fallback) {
            out.push(fallback);
        }
    }
    out
}

/// Best-effort removal of a control socket path that the daemon created
/// at startup. Only unlinks regular files and Unix-domain sockets. Any
/// other entry type (directory, symlink to a directory) is left alone.
///
/// A `NotFound` from the unlink itself is treated as success: another
/// process may have removed the confirmed socket between the metadata
/// check and the unlink, and the goal — no stale socket at the path — is
/// already satisfied in that case.
pub fn remove_control_socket(path: &Path) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        },
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Severity rank for candidate I/O errors: missing/refused means "not
/// running"; other failures are real diagnostics; a permission denial
/// must never be masked by a later lower-severity candidate.
fn stop_error_severity(error: &std::io::Error) -> u8 {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => 0,
        std::io::ErrorKind::PermissionDenied => 2,
        _ => 1,
    }
}

/// Keep the most severe I/O error observed across candidates so an
/// earlier `PermissionDenied` is not overwritten by, say, a later
/// fallback candidate that simply has no socket.
fn record_stop_error(slot: &mut Option<std::io::Error>, error: std::io::Error) {
    if slot.as_ref().map_or(true, |previous| {
        stop_error_severity(&error) > stop_error_severity(previous)
    }) {
        *slot = Some(error);
    }
}

/// Send one `STOP\n` command to a running `greggd` via its local Unix
/// control socket and block until the `OK\n` acknowledgement arrives.
///
/// Tries the config-adjacent path first, then the deterministic fallback.
/// A missing control socket on every candidate is treated as the daemon
/// already being stopped (idempotent success). A permission error or a
/// malformed response is surfaced as a [`ControlError`]. Any other
/// unexpected I/O condition — for example a daemon that accepts the
/// connection but closes or goes quiet without replying — yields
/// [`StopOutcome::Uncertain`] with a warning diagnostic rather than being
/// silently conflated with "nothing was listening".
///
/// The function does not invoke `systemctl`, `launchctl`, a shell, or any
/// process-discovery mechanism. It connects only to local Unix-domain
/// sockets.
pub fn send_stop(config_path: &Path) -> Result<StopOutcome, ControlError> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const IO_TIMEOUT: Duration = Duration::from_millis(750);

    let candidates = stop_candidates(config_path);
    let mut last_io_error: Option<std::io::Error> = None;

    for candidate in &candidates {
        // Local Unix-socket connect is essentially instantaneous for both
        // success (returning immediately) and common failures (NotFound
        // for missing paths, ConnectionRefused for stale listeners). The
        // read/write timeouts below bound the protocol exchange.
        let mut stream = match UnixStream::connect(candidate) {
            Ok(stream) => stream,
            Err(e) => {
                record_stop_error(&mut last_io_error, e);
                continue;
            }
        };

        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

        if let Err(e) = stream.write_all(STOP_COMMAND) {
            record_stop_error(&mut last_io_error, e);
            continue;
        }

        let mut buf = [0_u8; MAX_CONTROL_RESPONSE_BYTES];
        let mut length = 0;
        let mut response: Option<Vec<u8>> = None;
        while length < buf.len() && response.is_none() {
            match stream.read(&mut buf[length..]) {
                Ok(0) => break,
                Ok(read) => {
                    length += read;
                    if let Some(end) = buf[..length].iter().position(|b| *b == b'\n') {
                        response = Some(buf[..=end].to_vec());
                    }
                }
                Err(e) => {
                    record_stop_error(&mut last_io_error, e);
                    break;
                }
            }
        }

        if response.is_none() && length == buf.len() {
            // Buffer exhausted without finding a newline. Treat as malformed.
            return Err(ControlError::BadResponse);
        }

        if let Some(bytes) = response {
            if bytes.as_slice() == OK_RESPONSE {
                return Ok(StopOutcome::Stopped {
                    path: candidate.clone(),
                });
            }
            return Err(ControlError::BadResponse);
        }

        // Connected and STOP was delivered, but the peer closed or went
        // quiet without a newline-terminated reply. Record a diagnostic
        // so a live-but-misbehaving daemon is distinguishable from
        // "nothing was listening" in the final classification.
        record_stop_error(
            &mut last_io_error,
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "control socket accepted STOP but sent no complete response",
            ),
        );
    }

    // No candidate accepted the request. Surface a NotFound as the daemon
    // being already stopped (idempotent success). Anything else — for
    // example a permission error — is surfaced so the caller can present
    // a useful diagnostic.
    match last_io_error {
        Some(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            Ok(StopOutcome::NotRunning)
        }
        Some(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(ControlError::Io(e)),
        // An unexpected condition (silent close, timeout, shadowed path)
        // must never claim "not running": a live-but-stuck daemon would be
        // indistinguishable from an absent one.
        Some(e) => {
            tracing::warn!(error = ?e, "control socket stop attempt failed unexpectedly");
            Ok(StopOutcome::Uncertain)
        }
        None => Ok(StopOutcome::NotRunning),
    }
}

/// Result of a `greggd stop` invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum StopOutcome {
    /// The daemon acknowledged the stop request.
    Stopped {
        /// The control socket path the request was delivered over.
        path: PathBuf,
    },
    /// No candidate control socket accepted the request. The daemon is
    /// either already stopped, never started, or running with a different
    /// config identity.
    NotRunning,
    /// An unexpected I/O condition prevented classifying the daemon state;
    /// a live-but-unresponsive daemon cannot be distinguished from an
    /// absent one. Callers must not treat this as a successful stop.
    Uncertain,
}

/// Outcome of the async control listener accept loop.
#[derive(Debug)]
pub enum ControlBind {
    /// The listener is bound to the supplied path and the path is currently
    /// registered for cleanup on shutdown.
    Bound {
        /// Bound socket path.
        path: PathBuf,
        /// Tokio `UnixListener` ready for `accept()`.
        listener: tokio::net::UnixListener,
    },
    /// No listener could be bound. The caller should fall back to a
    /// non-control shutdown source.
    NotBound,
}

/// Bind the daemon's local Unix control listener.
///
/// Tries the config-adjacent path first; if that path cannot be bound or its
/// permissions cannot be secured, falls back to the deterministic temp-dir
/// path. The chosen socket file is created inside a private `0700` staging
/// directory and atomically renamed into its final location only after a
/// restrictive `0600` mode has been applied and verified, so the socket
/// inode never exists at a publicly reachable path with wider permissions.
/// A listener is only published if `0600` was applied successfully; if neither
/// candidate yields a secure listener the function returns
/// [`ControlBind::NotBound`] so the caller can decide whether a
/// permission-failure or no-secure-control-channel is the appropriate
/// runtime error for the platform.
///
/// Stale sockets at the candidate paths are only removed after a metadata
/// inspection confirms they are actually socket files (never regular files
/// or directories) and the local connect attempt is classified as
/// [`stale_connect_error`] rather than a permission or unknown error.
pub fn bind_listener(config_path: &Path) -> ControlBind {
    use tracing::info;

    if let Some(primary) = primary_control_path(config_path) {
        if let Some(bound) = try_bind_secure(&primary) {
            info!(path = %primary.display(), "control socket bound");
            return bound;
        }
    }
    if let Some(fallback) = fallback_control_path(config_path) {
        if Some(&fallback) != primary_control_path(config_path).as_ref() {
            if let Some(bound) = try_bind_secure(&fallback) {
                info!(path = %fallback.display(), "control socket bound (fallback)");
                return bound;
            }
        }
    }
    warn!("control socket not bound; daemon will only respond to signals");
    ControlBind::NotBound
}

/// Return true when the I/O error kind from a `connect()` attempt is
/// sufficient evidence that no live listener owns the socket file.
///
/// The classification deliberately permits only:
/// - [`std::io::ErrorKind::ConnectionRefused`] (no listener accepted);
/// - [`std::io::ErrorKind::NotFound`] (the path disappeared between
///   metadata and connect).
///
/// Other kinds — including `PermissionDenied`, `TimedOut`, and any
/// platform-specific surprise — are never treated as proof of staleness,
/// because the underlying socket may still belong to a live daemon that is
/// merely temporarily inaccessible to the caller.
#[must_use]
pub fn stale_connect_error(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
    )
}

fn try_bind_secure(path: &Path) -> Option<ControlBind> {
    use std::os::unix::fs::PermissionsExt;

    // Inspect the final path first. Only remove stale socket files; never
    // touch regular files or directories, and never displace a live
    // listener.
    prepare_final_path(path)?;

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!(
                parent = %parent.display(),
                error = %e,
                "control socket parent directory creation failed"
            );
            return None;
        }
    }

    // Bind inside a process-private staging directory with mode 0700 so
    // the socket inode never exists at a publicly reachable path with
    // umask-derived permissions. Once the staged inode is verified as
    // `0600`, an atomic same-parent rename publishes it.
    let stage_dir = private_staging_dir(path);
    let _ = std::fs::remove_dir_all(&stage_dir);
    if let Err(e) = std::fs::create_dir(&stage_dir)
        .and_then(|()| std::fs::set_permissions(&stage_dir, std::fs::Permissions::from_mode(0o700)))
    {
        warn!(
            dir = %stage_dir.display(),
            error = %e,
            "control socket staging directory setup failed"
        );
        let _ = std::fs::remove_dir_all(&stage_dir);
        return None;
    }

    let staged_socket = stage_dir.join("s");
    let listener = match tokio::net::UnixListener::bind(&staged_socket) {
        Ok(listener) => listener,
        Err(e) => {
            warn!(
                path = %staged_socket.display(),
                error = %e,
                "control socket bind failed"
            );
            let _ = std::fs::remove_dir_all(&stage_dir);
            return None;
        }
    };

    // Restrict and verify the staged inode before publishing. Some
    // filesystems can report success while silently retaining wider
    // permissions, so the metadata check is mandatory.
    if let Err(e) = std::fs::set_permissions(&staged_socket, std::fs::Permissions::from_mode(0o600))
    {
        warn!(
            path = %staged_socket.display(),
            error = %e,
            "control socket permission update failed; closing listener"
        );
        drop(listener);
        let _ = std::fs::remove_dir_all(&stage_dir);
        return None;
    }

    match std::fs::metadata(&staged_socket) {
        Ok(meta) => {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                warn!(
                    path = %staged_socket.display(),
                    mode = format!("{mode:o}"),
                    "control socket permissions are not 0600 after chmod; closing listener"
                );
                drop(listener);
                let _ = std::fs::remove_dir_all(&stage_dir);
                return None;
            }
        }
        Err(e) => {
            warn!(
                path = %staged_socket.display(),
                error = %e,
                "control socket metadata check failed; closing listener"
            );
            drop(listener);
            let _ = std::fs::remove_dir_all(&stage_dir);
            return None;
        }
    }

    // Publish only into a still-absent final path; if another process
    // bound the path during setup, abandon this candidate rather than
    // displacing it.
    if path.exists() {
        warn!(
            path = %path.display(),
            "control socket path appeared during secure setup; closing listener"
        );
        drop(listener);
        let _ = std::fs::remove_dir_all(&stage_dir);
        return None;
    }

    if let Err(e) = std::fs::rename(&staged_socket, path) {
        warn!(
            path = %path.display(),
            error = %e,
            "control socket publish rename failed; closing listener"
        );
        drop(listener);
        let _ = std::fs::remove_dir_all(&stage_dir);
        return None;
    }
    let _ = std::fs::remove_dir(&stage_dir);

    Some(ControlBind::Bound {
        path: path.to_path_buf(),
        listener,
    })
}

/// Inspect the final control-socket path before a bind attempt.
///
/// Returns `Some(())` when the path is free for binding (possibly after
/// removing a confirmed-stale socket file). Returns `None` when the path
/// holds a live listener or a non-socket entry, or when inspection itself
/// failed; in those cases the existing entry is always left in place.
fn prepare_final_path(path: &Path) -> Option<()> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let ft = meta.file_type();
            if !ft.is_socket() {
                warn!(
                    path = %path.display(),
                    "control socket path exists but is not a socket; skipping"
                );
                return None;
            }
            // Try to connect to confirm the entry is actually stale.
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    // Live listener; do not rebind.
                    None
                }
                Err(e) if stale_connect_error(e.kind()) => {
                    // Stale; safe to remove and rebind.
                    let _ = std::fs::remove_file(path);
                    Some(())
                }
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "control socket connect failed with non-stale classification; \
                         leaving existing entry in place"
                    );
                    None
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(()),
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "control socket metadata failed"
            );
            None
        }
    }
}

/// Process-private staging directory for the socket that will be published
/// at `path`.
///
/// It lives in the same parent as the final socket so the publish rename
/// stays within one filesystem and is atomic, and it is named after this
/// process so concurrent daemons never share one. The staging name plus the
/// one-character socket filename are shorter than the final socket's own
/// filename, so any parent short enough to pass the [`UNIX_PATH_MAX`]
/// candidate check also fits the staged path.
fn private_staging_dir(path: &Path) -> PathBuf {
    path.with_file_name(format!(".gd-stage-{}", std::process::id()))
}

/// Run a dedicated control-stop task that owns the bound Unix listener.
///
/// The task accepts connections, reads a bounded prefix, validates it is
/// exactly `STOP\n`, replies with `OK\n`, and signals the supplied
/// one-shot shutdown receiver. Stale or malformed input is dropped and the
/// connection is closed.
///
/// If the task is cancelled (for example because the runtime is dropped
/// during a signal-driven shutdown), the control-socket RAII guard ensures
/// the socket file is removed before any cleanup paths become observable.
pub fn spawn_stop_task(
    listener: tokio::net::UnixListener,
    path: PathBuf,
    notify: tokio::sync::oneshot::Sender<std::io::Result<&'static str>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let guard = ControlSocketGuard { path: path.clone() };
        let result = stop_loop(listener).await;
        let _ = notify.send(result);
        drop(guard);
    })
}

/// RAII guard that removes a control socket path when dropped.
///
/// This guarantees the socket file is cleaned up even if the spawning
/// runtime is dropped before the dedicated control task completes its
/// own cleanup path. The actual cleanup is a no-op for non-socket paths,
/// so dropping the guard on a healthy stop is harmless.
struct ControlSocketGuard {
    path: PathBuf,
}

impl Drop for ControlSocketGuard {
    fn drop(&mut self) {
        if let Err(e) = crate::control::remove_control_socket(&self.path) {
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "control socket guard cleanup failed"
            );
        }
    }
}

async fn stop_loop(listener: tokio::net::UnixListener) -> std::io::Result<&'static str> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => return Err(e),
        };

        let mut buf = [0_u8; MAX_CONTROL_REQUEST_BYTES];
        let mut length = 0;
        let mut received: Option<Vec<u8>> = None;
        // Read until we see a newline, run out of buffer, or get EOF.
        while length < buf.len() && received.is_none() {
            match stream.read(&mut buf[length..]).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    length += read;
                    if let Some(end) = buf[..length].iter().position(|b| *b == b'\n') {
                        received = Some(buf[..=end].to_vec());
                    }
                }
            }
        }

        if let Some(bytes) = received {
            if bytes.as_slice() == STOP_COMMAND {
                let _ = stream.write_all(OK_RESPONSE).await;
                let _ = stream.flush().await;
                let _ = stream.shutdown().await;
                return Ok("control-stop");
            }
        }
        let _ = stream.shutdown().await;
    }
}

/// Wait for the dedicated control-stop task to complete and surface the
/// shutdown reason.
///
/// The returned future resolves to `Err` only if the control task itself
/// returned an I/O error. If no control task was registered (for example
/// because `bind_listener` could not bind a socket), this future never
/// resolves and the daemon must rely on signals instead.
pub async fn wait_for_stop_task(
    receiver: tokio::sync::oneshot::Receiver<std::io::Result<&'static str>>,
) -> Option<&'static str> {
    match receiver.await {
        Ok(Ok(reason)) => Some(reason),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "control stop task ended with I/O error");
            None
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_config_path(tag: &str) -> PathBuf {
        // Keep the path short enough to stay below UNIX_PATH_MAX even after
        // the `greggd-<id>.control.sock` suffix is appended.
        std::env::temp_dir().join(format!(
            "gd{tag}-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("main"),
        ))
    }

    fn repo_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("gd{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_id_for_path_is_deterministic_and_hex_encoded() {
        let cfg_path = temp_config_path("id-deterministic");
        let id = config_id_for_path(&cfg_path);
        assert_eq!(id.len(), 16);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Deterministic: same path -> same digest.
        assert_eq!(id, config_id_for_path(&cfg_path));
    }

    #[test]
    fn config_id_changes_only_when_path_changes() {
        let a = temp_config_path("id-a");
        let b = temp_config_path("id-b");
        assert_ne!(config_id_for_path(&a), config_id_for_path(&b));
    }

    #[test]
    fn existing_file_relative_and_absolute_spellings_share_identity() {
        let dir = repo_temp_dir("path-spellings");
        let config = make_config_file(&dir, "greggd.toml");
        let current_dir = std::env::current_dir().unwrap();
        let relative = PathBuf::from(".").join(config.strip_prefix(current_dir).unwrap());

        assert!(relative.is_relative());
        assert_eq!(
            config_id_for_path(&relative),
            config_id_for_path(&config),
            "relative and absolute spellings of one existing file must converge"
        );
        assert_eq!(
            primary_control_path(&relative),
            primary_control_path(&config),
            "equivalent spellings must select the same primary socket"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_file_symlink_and_target_share_identity() {
        let dir = fresh_temp_dir("symlink-identity");
        let target = make_config_file(&dir, "target.toml");
        let link_dir = dir.join("links");
        std::fs::create_dir_all(&link_dir).unwrap();
        let link = link_dir.join("config-link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(
            config_id_for_path(&link),
            config_id_for_path(&target),
            "symlink and target spellings must converge"
        );
        assert_eq!(primary_control_path(&link), primary_control_path(&target));
        assert_eq!(fallback_control_path(&link), fallback_control_path(&target));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_different_files_keep_distinct_identities() {
        let dir = fresh_temp_dir("different-files");
        let a = make_config_file(&dir, "a.toml");
        let b = make_config_file(&dir, "b.toml");

        assert_ne!(config_id_for_path(&a), config_id_for_path(&b));
        assert_ne!(primary_control_path(&a), primary_control_path(&b));
        assert_ne!(fallback_control_path(&a), fallback_control_path(&b));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_contents_do_not_change_existing_file_identity() {
        let dir = fresh_temp_dir("content-identity");
        let config = make_config_file(&dir, "greggd.toml");
        let before = config_id_for_path(&config);

        std::fs::write(&config, b"host = \"127.0.0.1\"\nport = 11311\n").unwrap();

        assert_eq!(before, config_id_for_path(&config));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absent_absolute_path_has_deterministic_identity_without_creation() {
        let dir = std::env::temp_dir().join(format!("gdmissing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("greggd.toml");
        assert!(!config.exists());

        assert_eq!(config_id_for_path(&config), config_id_for_path(&config));
        let primary = primary_control_path(&config).unwrap();
        let fallback = fallback_control_path(&config).unwrap();
        assert!(primary.as_os_str().len() <= UNIX_PATH_MAX);
        assert!(fallback.as_os_str().len() <= UNIX_PATH_MAX);
    }

    #[test]
    fn primary_path_derives_from_config_path_in_same_directory() {
        let dir = std::env::temp_dir().join(format!("gd-id-cf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let a = dir.join("a.toml");
        let b = dir.join("b.toml");
        let pa = primary_control_path(&a);
        let pb = primary_control_path(&b);
        assert!(
            pa.is_some() && pb.is_some(),
            "both primary paths must fit in UNIX_PATH_MAX; got {pa:?} / {pb:?}"
        );
        assert_ne!(
            pa, pb,
            "different config files in the same directory must produce different control identities"
        );
        assert_ne!(
            fallback_control_path(&a),
            fallback_control_path(&b),
            "fallback identities must also differ"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn primary_path_is_config_adjacent_and_within_sun_path_limit() {
        let cfg_path = temp_config_path("primary-adjacent");
        let primary = primary_control_path(&cfg_path).unwrap();
        let name = primary.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("greggd-"));
        assert!(name.ends_with(".control.sock"));
        assert!(primary.as_os_str().len() <= UNIX_PATH_MAX);
    }

    #[test]
    fn fallback_path_is_deterministic_and_in_temp_dir() {
        let cfg_path = temp_config_path("fallback-deterministic");
        let first = fallback_control_path(&cfg_path).unwrap();
        let second = fallback_control_path(&cfg_path).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(std::env::temp_dir()));
        assert!(first.as_os_str().len() <= UNIX_PATH_MAX);
        let name = first.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("greggd-"));
        assert!(name.ends_with(".control.sock"));
    }

    #[test]
    fn stop_candidates_returns_primary_then_fallback() {
        // Use a deep directory so primary (config-adjacent) and fallback
        // (temp-dir root) point at different paths. Putting the file at
        // the temp-dir root directly would alias the two candidates.
        let parent = std::env::temp_dir().join(format!("gd-cand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        let cfg_path = parent.join("greggd.toml");

        let candidates = stop_candidates(&cfg_path);
        let primary = primary_control_path(&cfg_path);
        let fallback = fallback_control_path(&cfg_path);
        assert!(
            primary.is_some() && fallback.is_some(),
            "both candidates must fit in UNIX_PATH_MAX; primary={primary:?} fallback={fallback:?}"
        );
        assert_ne!(
            primary, fallback,
            "primary and fallback must not alias each other"
        );
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], primary.unwrap());
        assert_eq!(candidates[1], fallback.unwrap());

        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn remove_control_socket_is_a_noop_for_missing_paths() {
        let missing =
            std::env::temp_dir().join(format!("greggd-no-such-{}.sock", std::process::id()));
        remove_control_socket(&missing).unwrap();
    }

    #[test]
    fn remove_control_socket_leaves_regular_files_alone() {
        let dir = std::env::temp_dir().join(format!(
            "greggd-control-remove-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("main"),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let regular = dir.join("greggd-control-regular-file");
        std::fs::write(&regular, b"not a socket").unwrap();

        // A regular file at the control path must not be removed. The
        // daemon-side cleanup intentionally refuses to delete arbitrary
        // files because that would risk data loss.
        remove_control_socket(&regular).unwrap();
        assert!(regular.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wire_constants_have_expected_format() {
        assert_eq!(STOP_COMMAND, b"STOP\n");
        assert_eq!(OK_RESPONSE, b"OK\n");
    }

    #[test]
    fn stale_connect_error_classifies_only_documented_kinds() {
        assert!(stale_connect_error(std::io::ErrorKind::ConnectionRefused));
        assert!(stale_connect_error(std::io::ErrorKind::NotFound));
        // Other kinds must NOT be treated as stale. Each one would be
        // unsafe evidence that the socket file is abandoned.
        assert!(!stale_connect_error(std::io::ErrorKind::PermissionDenied));
        assert!(!stale_connect_error(std::io::ErrorKind::TimedOut));
        assert!(!stale_connect_error(std::io::ErrorKind::AddrInUse));
        assert!(!stale_connect_error(std::io::ErrorKind::Other));
    }

    fn fresh_temp_dir(name: &str) -> PathBuf {
        // Use a short prefix so the resulting control socket path stays
        // inside the OS-level `sun_path` limit (`UNIX_PATH_MAX = 108`).
        let dir = std::env::temp_dir().join(format!("gd{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_config_file(dir: &Path, name: &str) -> PathBuf {
        let cfg = dir.join(name);
        std::fs::write(&cfg, b"").unwrap();
        cfg
    }

    #[test]
    fn bind_listener_prefers_config_adjacent_path_when_available() {
        let dir = fresh_temp_dir("bind-primary");
        let cfg = make_config_file(&dir, "greggd.toml");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg) });

        match bound {
            ControlBind::Bound { path, .. } => {
                assert_eq!(path, primary_control_path(&cfg).unwrap());
                assert!(path.exists());
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "control socket must have restrictive permissions"
                );
            }
            ControlBind::NotBound => panic!("primary bind should have succeeded"),
        }

        remove_control_socket(&primary_control_path(&cfg).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bind_listener_falls_back_when_config_parent_is_not_writable() {
        let dir = fresh_temp_dir("bind-fallback");
        let cfg = make_config_file(&dir, "greggd.toml");
        let primary = primary_control_path(&cfg).unwrap();
        // Place a regular file at the primary path so the primary bind is
        // refused and we are forced into the fallback branch.
        std::fs::write(&primary, b"blocker").unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg) });

        match bound {
            ControlBind::Bound { path, .. } => {
                assert_eq!(path, fallback_control_path(&cfg).unwrap());
                assert!(path.exists());
            }
            ControlBind::NotBound => panic!("fallback bind should have succeeded"),
        }

        let _ = std::fs::remove_file(&primary);
        remove_control_socket(&fallback_control_path(&cfg).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bind_listener_skips_live_primary_listener() {
        let dir = fresh_temp_dir("bind-live");
        let cfg = make_config_file(&dir, "greggd.toml");
        let primary = primary_control_path(&cfg).unwrap();
        let live = std::os::unix::net::UnixListener::bind(&primary).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg) });

        match bound {
            ControlBind::Bound { path, .. } => {
                assert_eq!(
                    path,
                    fallback_control_path(&cfg).unwrap(),
                    "live primary listener must be left alone"
                );
            }
            ControlBind::NotBound => panic!("fallback should have bound when primary is live"),
        }

        drop(live);
        remove_control_socket(&primary).unwrap();
        remove_control_socket(&fallback_control_path(&cfg).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_stop_reports_not_running_when_no_socket_is_present() {
        let dir = fresh_temp_dir("send-stop-missing");
        let cfg = make_config_file(&dir, "greggd.toml");

        let outcome = send_stop(&cfg).unwrap();
        assert_eq!(outcome, StopOutcome::NotRunning);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_error_bookkeeping_never_downgrades_severity() {
        // A primary candidate failing with PermissionDenied must not be
        // overwritten by a later fallback candidate failing with NotFound.
        let mut slot = None;
        record_stop_error(
            &mut slot,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        record_stop_error(
            &mut slot,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(
            slot.as_ref().map(std::io::Error::kind),
            Some(std::io::ErrorKind::PermissionDenied)
        );

        // Lower-severity first, then higher: the higher wins.
        let mut slot = None;
        record_stop_error(
            &mut slot,
            std::io::Error::from(std::io::ErrorKind::TimedOut),
        );
        record_stop_error(
            &mut slot,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(
            slot.as_ref().map(std::io::Error::kind),
            Some(std::io::ErrorKind::TimedOut)
        );
        record_stop_error(
            &mut slot,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert_eq!(
            slot.as_ref().map(std::io::Error::kind),
            Some(std::io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn send_stop_treats_silent_close_as_uncertain_with_diagnostic() {
        let dir = fresh_temp_dir("send-stop-silent");
        let cfg = make_config_file(&dir, "greggd.toml");

        // A daemon that accepts STOP and closes without replying is live
        // but stuck; it must not be reported as Stopped or as cleanly not
        // running. It is classified as uncertain with a recorded
        // diagnostic so scripts can distinguish it from "no socket".
        let primary = primary_control_path(&cfg).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&primary).unwrap();

        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                use std::io::Read;
                let mut stream = stream;
                let mut buf = [0_u8; 32];
                let _ = stream.read(&mut buf);
                drop(stream);
            }
        });

        let outcome = send_stop(&cfg);
        assert_eq!(outcome.unwrap(), StopOutcome::Uncertain);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_stop_delivers_stop_and_receives_ok_response() {
        let dir = fresh_temp_dir("send-stop-ok");
        let cfg = make_config_file(&dir, "greggd.toml");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let outcome = rt.block_on(async {
            let bound = bind_listener(&cfg);
            let (path, listener) = match bound {
                ControlBind::Bound { path, listener } => (path, listener),
                ControlBind::NotBound => panic!("expected bound listener"),
            };

            let (tx, rx) = tokio::sync::oneshot::channel();
            let _task = spawn_stop_task(listener, path.clone(), tx);

            // Wait briefly so the listener is parked in accept().
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let cfg_clone = cfg.clone();
            let client = tokio::task::spawn_blocking(move || send_stop(&cfg_clone));
            let reason = rx.await.expect("control task must signal completion");
            let outcome = client.await.expect("client task must complete");
            (outcome, reason)
        });

        let (outcome, reason) = outcome;
        assert!(matches!(outcome, Ok(StopOutcome::Stopped { .. })));
        assert!(reason.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bind_listener_rebinds_stale_primary_socket_file() {
        let dir = fresh_temp_dir("rebind-stale");
        let cfg = make_config_file(&dir, "greggd.toml");
        let primary = primary_control_path(&cfg).unwrap();
        // Bind a unix listener then immediately drop it (leaving a stale socket file).
        let stale = std::os::unix::net::UnixListener::bind(&primary).unwrap();
        drop(stale);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg) });

        match bound {
            ControlBind::Bound { path, .. } => {
                assert_eq!(path, primary);
                // The bind succeeded and overwrote the stale entry.
            }
            ControlBind::NotBound => panic!("bind_listener must rebind stale primary"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_stop_rejects_malformed_responses() {
        let dir = fresh_temp_dir("send-stop-malformed");
        let cfg = make_config_file(&dir, "greggd.toml");

        // Bind a raw std listener at the primary path and serve a bad response.
        let primary = primary_control_path(&cfg).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&primary).unwrap();

        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buf = [0_u8; 32];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"NOPE\n");
                let _ = stream.flush();
            }
        });

        let outcome = send_stop(&cfg);
        assert!(matches!(outcome, Err(ControlError::BadResponse)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cross_config_stop_isolates_two_daemons_in_same_directory() {
        let dir = fresh_temp_dir("cross-stop");
        let cfg_a = make_config_file(&dir, "a.toml");
        let cfg_b = make_config_file(&dir, "b.toml");

        let primary_a = primary_control_path(&cfg_a).unwrap();
        let primary_b = primary_control_path(&cfg_b).unwrap();
        assert_ne!(primary_a, primary_b);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let (b_stopped, b_reason_ok, a_socket_present, a_final_stop) = rt.block_on(async {
            let bound_a = bind_listener(&cfg_a);
            let bound_b = bind_listener(&cfg_b);

            let (
                ControlBind::Bound {
                    path: path_a,
                    listener: listener_a,
                },
                ControlBind::Bound {
                    path: path_b,
                    listener: listener_b,
                },
            ) = (bound_a, bound_b)
            else {
                panic!("both control listeners must bind concurrently")
            };
            assert_eq!(path_a, primary_a);
            assert_eq!(path_b, primary_b);

            let (tx_a, mut rx_a) = tokio::sync::oneshot::channel();
            let (tx_b, rx_b) = tokio::sync::oneshot::channel();
            let _task_a = spawn_stop_task(listener_a, path_a.clone(), tx_a);
            let _task_b = spawn_stop_task(listener_b, path_b.clone(), tx_b);

            // Give the listeners a moment to park in accept().
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            let cfg_b_for_stop_b = cfg_b.clone();
            let client_b = tokio::task::spawn_blocking(move || send_stop(&cfg_b_for_stop_b));
            let stopped_b = client_b.await.expect("client task must complete");

            // B's notification must resolve because we targeted B.
            let reason_b = rx_b.await.expect("B control task must signal completion");

            // A's notification must NOT have resolved yet because we never
            // targeted A. `try_recv` returns `Empty` when the sender is
            // still alive and no value has been sent. Both `Closed` and
            // `Ok` would prove A responded (incorrectly).
            let a_still_alive = matches!(
                rx_a.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            );
            let a_socket_present = a_still_alive && primary_a.exists();

            // Now stop A explicitly while the runtime is still alive so the
            // task does not get cancelled and the RAII guard removes the
            // socket out from under send_stop.
            let cfg_a_for_stop_a = cfg_a.clone();
            let client_a = tokio::task::spawn_blocking(move || send_stop(&cfg_a_for_stop_a));
            let stopped_a = client_a.await.expect("client task must complete");
            let reason_a = rx_a.await.expect("A control task must signal completion");

            (
                stopped_b,
                reason_b.is_ok(),
                a_socket_present,
                (stopped_a, reason_a.is_ok()),
            )
        });

        match &b_stopped {
            Ok(StopOutcome::Stopped { path }) => assert_eq!(
                path, &primary_b,
                "send_stop(cfg_b) must resolve on the B primary path"
            ),
            other => panic!("send_stop(cfg_b) must succeed; got {other:?}"),
        }
        assert!(b_reason_ok, "daemon B control task must complete cleanly");
        assert!(
            a_socket_present,
            "daemon A's primary socket must remain on disk after sending STOP to daemon B"
        );
        assert!(
            matches!(a_final_stop.0, Ok(StopOutcome::Stopped { .. })),
            "daemon A must respond to a follow-up send_stop(cfg_a)"
        );
        assert!(
            a_final_stop.1,
            "daemon A control task must complete cleanly"
        );

        remove_control_socket(&primary_a).unwrap();
        remove_control_socket(&primary_b).unwrap();
        remove_control_socket(&fallback_control_path(&cfg_a).unwrap()).unwrap();
        remove_control_socket(&fallback_control_path(&cfg_b).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
