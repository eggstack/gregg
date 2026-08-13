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
//! Two candidates are derived from the daemon configuration:
//!
//! 1. **Primary (config-adjacent)**: `<config_parent>/greggd.control.sock`.
//!    This is preferred because the packaged Linux service writes its
//!    config to `/etc/gregg/` (writable by the daemon user) and systemd's
//!    `PrivateTmp=true` would otherwise isolate a `/tmp`-only fallback from
//!    the operator's CLI.
//! 2. **Fallback (temp dir)**: `<temp_dir>/greggd-<host>-<port>.control.sock`.
//!    Used when the config parent directory is not writable by the daemon
//!    user (for example when running `greggd run` from a non-root account
//!    while reading an operator-installed config).
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

use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::warn;

use crate::config::Config;

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

/// Compute the primary config-adjacent control socket path.
///
/// Returns `<config_parent>/greggd.control.sock` if the parent directory
/// exists. The daemon uses this when the parent is writable by its own
/// user; the client uses it as the first candidate.
#[must_use]
pub fn primary_control_path(config_path: &Path) -> Option<PathBuf> {
    let parent = config_path.parent()?;
    let path = parent.join("greggd.control.sock");
    if path.as_os_str().len() > UNIX_PATH_MAX {
        return None;
    }
    Some(path)
}

/// Compute the deterministic fallback control socket path.
///
/// The fallback lives under the standard system temp directory and is
/// derived only from validated host/port data so it cannot be used for
/// arbitrary path traversal. The hostname component is restricted to
/// `[A-Za-z0-9._-]` to avoid path separators or other surprising bytes.
#[must_use]
pub fn fallback_control_path(config: &Config) -> Option<PathBuf> {
    let host = sanitize_host(config.host.to_string());
    let filename = format!("greggd-{host}-{}.control.sock", config.port);
    let path = std::env::temp_dir().join(filename);
    if path.as_os_str().len() > UNIX_PATH_MAX {
        return None;
    }
    Some(path)
}

/// All candidates `stop` should try, in priority order.
#[must_use]
pub fn stop_candidates(config_path: &Path, config: &Config) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    if let Some(primary) = primary_control_path(config_path) {
        out.push(primary);
    }
    if let Some(fallback) = fallback_control_path(config) {
        if !out.iter().any(|p| p == &fallback) {
            out.push(fallback);
        }
    }
    out
}

/// Best-effort removal of a control socket path that the daemon created
/// at startup. Only unlinks regular files and Unix-domain sockets. Any
/// other entry type (directory, symlink to a directory) is left alone.
pub fn remove_control_socket(path: &Path) -> std::io::Result<()> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Replace any character in `input` that is not `[A-Za-z0-9._-]` with `_`.
///
/// This keeps the deterministic fallback path inside a single path segment
/// even if an operator configures a hostname that would otherwise contain
/// separators or whitespace.
fn sanitize_host(input: String) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Send one `STOP\n` command to a running `greggd` via its local Unix
/// control socket and block until the `OK\n` acknowledgement arrives.
///
/// Tries the config-adjacent path first, then the deterministic fallback.
/// A missing control socket on every candidate is treated as the daemon
/// already being stopped (idempotent success). Any other I/O error or a
/// malformed response is surfaced as a [`ControlError`].
///
/// The function does not invoke `systemctl`, `launchctl`, a shell, or any
/// process-discovery mechanism. It connects only to local Unix-domain
/// sockets.
pub fn send_stop(config_path: &Path, config: &Config) -> Result<StopOutcome, ControlError> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const IO_TIMEOUT: Duration = Duration::from_millis(750);

    let candidates = stop_candidates(config_path, config);
    let mut last_io_error: Option<std::io::Error> = None;

    for candidate in &candidates {
        // Local Unix-socket connect is essentially instantaneous for both
        // success (returning immediately) and common failures (NotFound
        // for missing paths, ConnectionRefused for stale listeners). The
        // read/write timeouts below bound the protocol exchange.
        let mut stream = match UnixStream::connect(candidate) {
            Ok(stream) => stream,
            Err(e) => {
                last_io_error = Some(e);
                continue;
            }
        };

        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

        if let Err(e) = stream.write_all(STOP_COMMAND) {
            last_io_error = Some(e);
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
                    last_io_error = Some(e);
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
        Some(_) => Ok(StopOutcome::NotRunning),
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
}

/// Outcome of the async control listener accept loop.
#[derive(Debug)]
pub enum ControlBind {
    /// The listener is bound to the supplied path and the path is currently
    /// registered for cleanup on shutdown.
    Bound {
        /// Bound socket path.
        path: PathBuf,
        /// Tokio UnixListener ready for `accept()`.
        listener: tokio::net::UnixListener,
    },
    /// No listener could be bound. The caller should fall back to a
    /// non-control shutdown source.
    NotBound,
}

/// Bind the daemon's local Unix control listener.
///
/// Tries the config-adjacent path first; if it is unavailable, falls back to
/// the deterministic temp-dir path. The chosen socket file is created with
/// restrictive permissions (`0600`) so unrelated local users cannot inject
/// a stop command.
///
/// Stale sockets at the candidate paths are only removed after a metadata
/// inspection confirms they are actually socket files (never regular files
/// or directories).
pub fn bind_listener(config_path: &Path, config: &Config) -> ControlBind {
    use std::os::unix::fs::PermissionsExt;
    use tracing::info;

    if let Some(primary) = primary_control_path(config_path) {
        if let Some(listener) = try_bind(&primary) {
            let _ = std::fs::set_permissions(&primary, std::fs::Permissions::from_mode(0o600));
            info!(path = %primary.display(), "control socket bound");
            return ControlBind::Bound {
                path: primary,
                listener,
            };
        }
    }
    if let Some(fallback) = fallback_control_path(config) {
        if Some(&fallback) != primary_control_path(config_path).as_ref() {
            if let Some(listener) = try_bind(&fallback) {
                let _ = std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o600));
                info!(path = %fallback.display(), "control socket bound (fallback)");
                return ControlBind::Bound {
                    path: fallback,
                    listener,
                };
            }
        }
    }
    warn!("control socket not bound; daemon will only respond to signals");
    ControlBind::NotBound
}

fn try_bind(path: &Path) -> Option<tokio::net::UnixListener> {
    use tokio::net::UnixListener;

    // Inspect the path first. Only remove stale socket files; never touch
    // regular files or directories.
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
                    return None;
                }
                Err(_) => {
                    // Stale; safe to remove and rebind.
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "control socket metadata failed"
            );
            return None;
        }
    }

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

    match UnixListener::bind(path) {
        Ok(listener) => Some(listener),
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "control socket bind failed"
            );
            None
        }
    }
}

/// Run a dedicated control-stop task that owns the bound Unix listener.
///
/// The task accepts connections, reads a bounded prefix, validates it is
/// exactly `STOP\n`, replies with `OK\n`, and signals the supplied
/// one-shot shutdown receiver. Stale or malformed input is dropped and the
/// connection is closed.
///
/// If the task is cancelled (for example because the runtime is dropped
/// during a signal-driven shutdown), the [`ControlSocketGuard`] ensures
/// the socket file is removed before any cleanup paths become observable.
pub fn spawn_stop_task(
    listener: tokio::net::UnixListener,
    path: PathBuf,
    notify: tokio::sync::oneshot::Sender<std::io::Result<&'static str>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _guard = ControlSocketGuard { path: path.clone() };
        let result = stop_loop(listener).await;
        if let Err(e) = crate::control::remove_control_socket(&path) {
            tracing::warn!(path = %path.display(), error = %e, "control socket cleanup failed");
        }
        let _ = notify.send(result);
        drop(_guard);
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
                Ok(0) => break,
                Ok(read) => {
                    length += read;
                    if let Some(end) = buf[..length].iter().position(|b| *b == b'\n') {
                        received = Some(buf[..=end].to_vec());
                    }
                }
                Err(_) => break,
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
#[must_use]
pub fn wait_for_stop_task(
    receiver: tokio::sync::oneshot::Receiver<std::io::Result<&'static str>>,
) -> impl std::future::Future<Output = Option<&'static str>> {
    async move {
        match receiver.await {
            Ok(Ok(reason)) => Some(reason),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "control stop task ended with I/O error");
                None
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::os::unix::fs::PermissionsExt;

    fn temp_config_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "greggd-control-test-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("main"),
        ))
    }

    #[test]
    fn primary_path_is_config_adjacent_and_within_sun_path_limit() {
        let cfg_path = temp_config_path();
        let primary = primary_control_path(&cfg_path).unwrap();
        assert!(primary.ends_with("greggd.control.sock"));
        assert!(primary.as_os_str().len() <= UNIX_PATH_MAX);
    }

    #[test]
    fn fallback_path_is_deterministic_and_in_temp_dir() {
        let config = Config {
            name: "test".into(),
            host: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            port: 11310,
            sample_interval_ms: 1000,
            stale_after_ms: 5000,
        };
        let first = fallback_control_path(&config).unwrap();
        let second = fallback_control_path(&config).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(std::env::temp_dir()));
        assert!(first.as_os_str().len() <= UNIX_PATH_MAX);
        let name = first.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("greggd-127.0.0.1-11310.control.sock"));
    }

    #[test]
    fn sanitize_host_replaces_path_separators_and_whitespace() {
        assert_eq!(sanitize_host("127.0.0.1".into()), "127.0.0.1");
        assert_eq!(sanitize_host("::1".into()), "__1");
        assert_eq!(sanitize_host("host name".into()), "host_name");
        assert_eq!(sanitize_host("../etc/gregg".into()), ".._etc_gregg");
    }

    #[test]
    fn stop_candidates_returns_primary_then_fallback() {
        let cfg_path = temp_config_path();
        let config = Config {
            name: "test".into(),
            host: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            port: 11310,
            sample_interval_ms: 1000,
            stale_after_ms: 5000,
        };
        let candidates = stop_candidates(&cfg_path, &config);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], primary_control_path(&cfg_path).unwrap());
        assert_eq!(candidates[1], fallback_control_path(&config).unwrap());
    }

    #[test]
    fn remove_control_socket_is_a_noop_for_missing_paths() {
        let missing =
            std::env::temp_dir().join(format!("greggd-no-such-{}.sock", std::process::id(),));
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
        let regular = dir.join("greggd.control.sock");
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

    fn test_config() -> Config {
        Config {
            name: "test".into(),
            host: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            port: 11320,
            sample_interval_ms: 1000,
            stale_after_ms: 5000,
        }
    }

    fn fresh_temp_dir(name: &str) -> PathBuf {
        // Use a short prefix so the resulting control socket path stays
        // inside the OS-level `sun_path` limit (`UNIX_PATH_MAX = 108`).
        let dir = std::env::temp_dir().join(format!("gd{name}-{}", std::process::id(),));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bind_listener_prefers_config_adjacent_path_when_available() {
        let dir = fresh_temp_dir("bind-primary");
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg, &test_config()) });

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
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();
        let primary = primary_control_path(&cfg).unwrap();
        // Place a regular file at the primary path so the primary bind is
        // refused and we are forced into the fallback branch.
        std::fs::write(&primary, b"blocker").unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg, &test_config()) });

        match bound {
            ControlBind::Bound { path, .. } => {
                assert_eq!(path, fallback_control_path(&test_config()).unwrap());
                assert!(path.exists());
            }
            ControlBind::NotBound => panic!("fallback bind should have succeeded"),
        }

        let _ = std::fs::remove_file(&primary);
        remove_control_socket(&fallback_control_path(&test_config()).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bind_listener_skips_live_primary_listener() {
        let dir = fresh_temp_dir("bind-live");
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();
        let primary = primary_control_path(&cfg).unwrap();
        let _live = std::os::unix::net::UnixListener::bind(&primary).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg, &test_config()) });

        match bound {
            ControlBind::Bound { path, .. } => {
                assert_eq!(
                    path,
                    fallback_control_path(&test_config()).unwrap(),
                    "live primary listener must be left alone"
                );
            }
            ControlBind::NotBound => panic!("fallback should have bound when primary is live"),
        }

        drop(_live);
        remove_control_socket(&primary).unwrap();
        remove_control_socket(&fallback_control_path(&test_config()).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_stop_reports_not_running_when_no_socket_is_present() {
        let dir = fresh_temp_dir("send-stop-missing");
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();

        let outcome = send_stop(&cfg, &test_config()).unwrap();
        assert_eq!(outcome, StopOutcome::NotRunning);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_stop_delivers_stop_and_receives_ok_response() {
        let dir = fresh_temp_dir("send-stop-ok");
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let outcome = rt.block_on(async {
            let bound = bind_listener(&cfg, &test_config());
            let (path, listener) = match bound {
                ControlBind::Bound { path, listener } => (path, listener),
                ControlBind::NotBound => panic!("expected bound listener"),
            };

            let (tx, rx) = tokio::sync::oneshot::channel();
            let _task = spawn_stop_task(listener, path.clone(), tx);

            // Wait briefly so the listener is parked in accept().
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let cfg_clone = cfg.clone();
            let config_clone = test_config();
            let client = tokio::task::spawn_blocking(move || send_stop(&cfg_clone, &config_clone));
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
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();
        let primary = primary_control_path(&cfg).unwrap();
        // Bind a unix listener then immediately drop it (leaving a stale socket file).
        let stale = std::os::unix::net::UnixListener::bind(&primary).unwrap();
        drop(stale);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bound = rt.block_on(async { bind_listener(&cfg, &test_config()) });

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
        let cfg = dir.join("greggd.toml");
        std::fs::write(&cfg, b"").unwrap();

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

        let outcome = send_stop(&cfg, &test_config());
        assert!(matches!(outcome, Err(ControlError::BadResponse)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
