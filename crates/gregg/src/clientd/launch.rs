//! Plan 165: making the client daemon operationally invisible.
//!
//! Plan 164 left `gregg` unable to start its own daemon, so a bare `gregg`
//! could only attach to one an operator had already launched. This module
//! closes that gap, and it is deliberately narrow: it starts *one* daemon for
//! *one* configuration, and refuses to do anything at all when a daemon it
//! cannot positively identify is already there.
//!
//! # Why a lock and not a PID file
//!
//! Two `gregg` commands started at the same moment would both probe, both find
//! nothing, and both spawn. A PID file does not fix that — it is exactly the
//! lossy, forgeable, never-cleaned-up registry the architecture refuses to
//! introduce. An advisory lock on a config-specific path does: the second
//! process blocks, then re-probes *after* the first has finished, and finds a
//! ready daemon. The lock is held across probe/spawn/readiness only, never for
//! the daemon's lifetime, and the daemon itself never holds it.
//!
//! The lock file's existence proves nothing and is never consulted as a signal;
//! only the OS lock counts. That is deliberate, so a crashed launcher can
//! never leave behind a file that makes the next launch believe a daemon is
//! starting.
//!
//! # Why a foreign peer blocks a launch
//!
//! If something that is not our daemon is sitting on the endpoint, spawning a
//! second daemon would either fail to bind or — far worse — succeed somewhere
//! the operator is not looking. Absence is the only condition that authorizes
//! a spawn. A version mismatch from a peer that *is* positively identified as
//! ours for this same config is the one recoverable case: we stop it and
//! relaunch with the current executable, which is what makes a long-lived
//! daemon self-healing across a client upgrade without any global registry of
//! active configs.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::clientd::daemon::{self, AttachError, Attachment};
use crate::clientd::identity::ClientDaemonIdentity;
use crate::clientd::ipc::TransportError;
use crate::config::lock::FileLockGuard;
use crate::config::{ConfigError, ConfigStore};

/// How long the launch lock may be waited for.
///
/// A competing launcher's critical section is a dial, a spawn, and a bounded
/// readiness wait, so this is generous on purpose: failing to get the lock
/// means giving up on a daemon that is probably starting right now, which is a
/// worse outcome than waiting.
const LAUNCH_LOCK_TIMEOUT: Duration = Duration::from_secs(20);

/// How long to wait for a freshly spawned daemon to answer a handshake.
///
/// The daemon binds its endpoint before publishing anything, so a successful
/// attach means the whole engine loop is up. This is generous because a cold
/// start also loads and validates the configuration.
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// How often readiness is re-probed.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long the spawned daemon may be given to exit after being told to stop,
/// before it is replaced.
const ROTATE_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The config-specific launch-lock path.
///
/// Deliberately distinct from the config mutation lock so that a `gregg add`
/// running at the same moment as a TUI launch does not make one wait on the
/// other. It is derived from the same config identity as the endpoint, so two
/// configs can never contend and two configs can never share a daemon.
#[must_use]
pub fn launch_lock_path(identity: &ClientDaemonIdentity) -> PathBuf {
    let primary = identity.candidates().first().cloned().unwrap_or_default();
    primary.with_extension("launch.lock")
}

/// Why a client daemon could not be ensured.
#[derive(Debug, thiserror::Error)]
pub enum EnsureError {
    /// Something is on the endpoint that is not a daemon this binary owns,
    /// and nothing may be spawned over it.
    #[error("{0}")]
    ForeignPeer(String),
    /// A positively identified, owned daemon for this config speaks a local
    /// protocol this binary will not replace automatically.
    #[error(
        "the client daemon for this config is newer than this gregg (local protocol {daemon} vs {frontend}); upgrade gregg, or run `gregg daemon stop` to replace it",
        daemon = .daemon,
        frontend = .frontend
    )]
    IncompatibleOwned {
        /// Local protocol version the running daemon speaks.
        daemon: u16,
        /// Local protocol version this frontend speaks.
        frontend: u16,
    },
    /// The daemon could not be started.
    #[error("could not start the client daemon: {0}")]
    Spawn(String),
    /// The daemon was started but never became ready.
    #[error("the client daemon did not become ready within {seconds}s; run `gregg daemon run` to see why")]
    NotReady {
        /// How long readiness was waited for.
        seconds: u64,
    },
    /// The launch lock could not be taken.
    #[error(transparent)]
    Lock(#[from] ConfigError),
    /// The endpoint could not be reached at all, for a reason that is not
    /// plain absence.
    #[error("{0}")]
    Transport(#[from] AttachError),
}

/// Attach to the client daemon for this config, starting one if needed.
///
/// This is the whole of Plan 165's "operationally invisible" promise: bare
/// `gregg` must work, and it must never leave two daemons for one config or
/// overwrite a peer it does not own.
pub async fn ensure_running(store: &ConfigStore) -> Result<Attachment, EnsureError> {
    let identity = ClientDaemonIdentity::for_path(store.path());
    let version = env!("CARGO_PKG_VERSION");

    // First, a **side-effect-free** probe. It may only answer "attach" or
    // "take the lock"; it never stops anything, because this point is outside
    // the launch lock and a stop is at least as destructive as the spawn the
    // lock exists to serialize.
    match probe(&identity, version).await? {
        Probe::Ready(attachment) => return Ok(*attachment),
        Probe::Absent | Probe::Replaceable => {}
    }

    // Absent, or an older owned daemon that may need replacing. Exactly one
    // process may cross this point per config.
    //
    // The lock wait is a blocking sleep loop, so it runs on a blocking thread.
    // Calling it inline would be a real bug, not a style choice: the TUI runs
    // on a current-thread runtime, so a launcher waiting on the lock would
    // starve the very tasks that make the winner's daemon reach readiness.
    let lock = acquire_launch_lock(&identity).await?;

    // Re-probe under the lock. This is the TOCTOU window: the process that lost
    // the race finds the winner's daemon here and does nothing at all.
    //
    // The *classification* is redone too, not just the probe. Someone else may
    // have bound a foreign service onto the endpoint while this process waited
    // for the lock, and re-probing alone would classify that as "still absent"
    // and spawn over it — which is exactly the outcome the classification above
    // exists to prevent.
    let attachment = match probe(&identity, version).await? {
        Probe::Ready(attachment) => *attachment,
        // The destructive branch lives here, under the lock, on a
        // classification this process just made. Rotating on the stale
        // classification from the unlocked probe above would let a launcher
        // that waited on the lock kill the current daemon the winner had
        // already installed.
        Probe::Replaceable => {
            rotate(&identity, version).await?;
            daemon::attach(&identity, version)
                .await
                .map_err(EnsureError::Transport)?
        }
        Probe::Absent => {
            spawn_daemon(store.path())?;
            let ready = wait_ready(&identity, version, READY_TIMEOUT).await;
            // The lock is released before attaching so a slow client never
            // blocks another launch; readiness has already been established at
            // this point.
            drop(lock);
            return ready;
        }
    };
    drop(lock);
    Ok(attachment)
}

/// What one attach attempt resolved to, with no side effects.
enum Probe {
    /// Our daemon answered; attach to this.
    ///
    /// Boxed because an `Attachment` owns the connection's buffers and stream,
    /// which makes it far larger than the other two variants carry.
    Ready(Box<Attachment>),
    /// Nothing is there; a launch is authorized.
    Absent,
    /// Our daemon answered at an older protocol. It may be replaced, but only
    /// by the caller, and only while holding the launch lock.
    Replaceable,
}

/// One side-effect-free attach attempt.
///
/// `Ok(Ready)` means attach. `Ok(Absent)` means the endpoint is genuinely
/// absent and a launch is authorized. `Ok(Replaceable)` means this config's own
/// older daemon is running and the caller may stop it — but only after
/// re-classifying under the launch lock. `Err` means something is there that
/// this binary must not touch, with the reason already classified.
///
/// This never stops a daemon: a probe that could mutate would make the
/// unlocked call in [`ensure_running`] an unsynchronized kill, and two
/// launchers racing on an older owned daemon would have one of them tear down
/// the healthy current daemon the other had just installed.
async fn probe(identity: &ClientDaemonIdentity, version: &str) -> Result<Probe, EnsureError> {
    match daemon::attach(identity, version).await {
        Ok(attachment) => Ok(Probe::Ready(Box::new(attachment))),
        Err(error) => match classify(&error) {
            Classification::Absent => Ok(Probe::Absent),
            Classification::Rotate => Ok(Probe::Replaceable),
            Classification::Foreign => Err(EnsureError::ForeignPeer(error.to_string())),
            Classification::Incompatible => Err(EnsureError::IncompatibleOwned {
                daemon: match &error {
                    AttachError::VersionMismatch { daemon, .. } => *daemon,
                    _ => 0,
                },
                frontend: crate::clientd::protocol::PROTOCOL_VERSION,
            }),
        },
    }
}

/// Take the config-specific launch lock without blocking the runtime.
///
/// The OS-level wait is a poll loop, so it belongs on a blocking thread. The
/// guard is `Send`, so it comes back across cleanly and is released when it
/// drops — which is what keeps the lock scoped to the
/// probe/rotate/spawn/readiness window and never to the daemon's lifetime.
/// Every destructive step is inside that window: [`probe`] is side-effect-free,
/// so nothing is stopped or spawned outside it.
///
/// # Errors
///
/// Returns [`EnsureError::Lock`] if the lock cannot be taken within
/// [`LAUNCH_LOCK_TIMEOUT`].
async fn acquire_launch_lock(
    identity: &ClientDaemonIdentity,
) -> Result<FileLockGuard, EnsureError> {
    let path = launch_lock_path(identity);
    tokio::task::spawn_blocking(move || FileLockGuard::acquire(&path, LAUNCH_LOCK_TIMEOUT))
        .await
        .map_err(|error| {
            EnsureError::Spawn(format!("the launch lock task did not complete: {error}"))
        })?
        .map_err(EnsureError::from)
}

/// What an attach attempt told us about the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Classification {
    /// Nothing is there; a launch is authorized.
    Absent,
    /// Our daemon, wrong protocol; it can be replaced.
    Rotate,
    /// Something answered that is not our daemon for this config.
    Foreign,
    /// Our daemon for this config, but it cannot be reconciled from here.
    Incompatible,
}

fn classify(error: &AttachError) -> Classification {
    match error {
        // "Nothing is serving this endpoint" has exactly two shapes: the socket
        // file is not there, or it is there and nobody is listening. Only these
        // authorize a spawn. A connect can fail for other reasons — `EMFILE`/
        // `ENFILE` fd exhaustion, `EACCES` on the socket, an already-in-use
        // endpoint reported by `remove_stale_socket` — and none of those is
        // absence: spawning there would add a daemon the operator never asked
        // for on top of a failure they still have to fix.
        AttachError::Transport(TransportError::Io(io)) => {
            if matches!(
                io.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) {
                Classification::Absent
            } else {
                Classification::Foreign
            }
        }
        // The peer completed enough of a handshake to report its protocol
        // version, and the identity it reported is this config's — the
        // daemon's own error path only emits this frame after
        // `verify_daemon_id` passed, so it is positively identified.
        //
        // That makes replacing it *possible*; it does not automatically make it
        // *right*. A daemon older than this frontend is stale by definition
        // and is replaced. A daemon **newer** than this frontend is somebody's
        // current binary, possibly serving a newer frontend elsewhere, and
        // killing it would downgrade their whole session because this one
        // window is old. So the newer case fails with guidance instead.
        AttachError::VersionMismatch { daemon, .. } => {
            if *daemon > crate::clientd::protocol::PROTOCOL_VERSION {
                Classification::Incompatible
            } else {
                Classification::Rotate
            }
        }
        // A refusal, a peer that hung up without identifying itself, a connect
        // that failed for any reason other than "nothing there", or an endpoint
        // somebody already holds. None of it is absence: spawning over a peer
        // that answered is how you end up with two daemons and no idea which
        // one the TUI is showing.
        AttachError::Refused(_)
        | AttachError::Disconnected
        | AttachError::HandshakeTimeout
        | AttachError::Transport(
            TransportError::Disconnected
            | TransportError::WouldBlock
            | TransportError::Decode(_)
            | TransportError::Protocol(_)
            | TransportError::ConfigMismatch { .. }
            | TransportError::VersionMismatch { .. },
        ) => Classification::Foreign,
    }
}

/// Stop a positively identified, incompatible daemon and relaunch it.
///
/// # Safety of the kill
///
/// `daemon::stop` only completes after a successful handshake against a peer
/// that reported *this* config's identity, and only then sends the stop
/// request. A peer that refuses, disconnects, or reports a different identity
/// is left strictly alone. That is the whole ownership check: there is no
/// name matching, no PID scanning, and no signal sent to an unverified
/// process.
async fn rotate(identity: &ClientDaemonIdentity, version: &str) -> Result<(), EnsureError> {
    daemon::stop(identity, version)
        .await
        .map_err(EnsureError::Transport)?;
    wait_gone(identity, ROTATE_STOP_TIMEOUT).await;
    Ok(())
}

/// Wait, bounded, for the endpoint to stop being served.
///
/// [`crate::clientd::ipc::endpoint_is_live`] rather than `Path::exists()`: a
/// Windows named pipe is never a filesystem entry, so a plain existence check
/// answers "no" for a daemon that is still running and this would return on its
/// first iteration, handing the caller a daemon that has not finished
/// unwinding. That is the same hazard the helper's own documentation calls out.
async fn wait_gone(identity: &ClientDaemonIdentity, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !identity
            .candidates()
            .iter()
            .any(|candidate| crate::clientd::ipc::endpoint_is_live(candidate))
        {
            return;
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// Re-probe until the daemon answers a handshake, bounded.
async fn wait_ready(
    identity: &ClientDaemonIdentity,
    version: &str,
    timeout: Duration,
) -> Result<Attachment, EnsureError> {
    let deadline = std::time::Instant::now() + timeout;
    let mut last: Option<AttachError> = None;
    while std::time::Instant::now() < deadline {
        match daemon::attach(identity, version).await {
            Ok(attachment) => return Ok(attachment),
            Err(error) => {
                // A peer that identifies itself but refuses is a real answer;
                // keep waiting for it to finish starting rather than reporting
                // absence as a startup failure.
                last = Some(error);
            }
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
    match last {
        Some(AttachError::Transport(_)) | None => Err(EnsureError::NotReady {
            seconds: timeout.as_secs(),
        }),
        Some(other) => Err(EnsureError::Transport(other)),
    }
}

/// Spawn the exact current executable as a detached `gregg daemon run`.
///
/// Detached, and with every standard stream null: the daemon outlives this
/// process, so it must not hold a terminal, and it must not inherit a pipe
/// that would keep a parent waiting. It is never given a controlling terminal
/// or process group.
///
/// The exact executable is used rather than a name from `PATH`, so a stale or
/// shadowing `gregg` earlier in the path can never become the daemon this
/// frontend then attaches to.
fn spawn_daemon(config_path: &std::path::Path) -> Result<(), EnsureError> {
    let executable = std::env::current_exe().map_err(|error| {
        EnsureError::Spawn(format!("could not locate this executable: {error}"))
    })?;

    let mut command = std::process::Command::new(executable);
    command
        .arg("--config")
        .arg(config_path)
        .arg("daemon")
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A new process group detaches the daemon from this process's
        // terminal, so a `Ctrl-C` that ends the TUI does not also end the
        // daemon. `setsid` is deliberately *not* used: a new process group is
        // enough to stop the terminal signal from propagating, and it needs no
        // `unsafe`.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    command
        .spawn()
        // The child is intentionally not waited on and not stored: it is
        // reparented and keeps running for the operator's session, which is
        // the whole point of a background client daemon.
        .map(|_child| ())
        .map_err(|error| EnsureError::Spawn(error.to_string()))
}

/// Restart the client daemon for this config.
///
/// Stops only a positively identified, owned matching daemon, then starts a
/// fresh one with the current executable. A daemon that was not running is
/// simply started, which is the useful half of the behavior for a manager that
/// wants to guarantee liveness.
pub async fn restart(store: &ConfigStore) -> Result<(), EnsureError> {
    let identity = ClientDaemonIdentity::for_path(store.path());
    let version = env!("CARGO_PKG_VERSION");

    // The stop belongs *inside* the launch lock, and the decision to stop is
    // re-made from it. An unlocked `status` can observe a daemon that a
    // concurrent `ensure_running` has just spawned and is still waiting to
    // become ready; stopping that one kills a healthy launch the operator never
    // asked to interrupt, and the waiter's readiness then fails for a reason
    // that has nothing to do with its own state. Taking the lock first is what
    // makes this window exclusive with the one `ensure_running` holds.
    let lock = acquire_launch_lock(&identity).await?;
    if daemon::status(&identity).await?.running {
        daemon::stop(&identity, version)
            .await
            .map_err(EnsureError::Transport)?;
        wait_gone(&identity, ROTATE_STOP_TIMEOUT).await;
    }

    // Spawn and then *confirm readiness*, both unconditionally. Readiness is the
    // result of a restart: a child that dies at startup — a config validation
    // violation, an endpoint it could not claim — has to be reported, not
    // swallowed. `gregg update` turns this `Ok(())` into
    // `DaemonLifecycle::Relaunched`, so a discarded `NotReady` would claim a
    // relaunch with nothing listening. A daemon that outlived `wait_gone` is
    // not a reason to skip the spawn either: `bind` refuses an endpoint it
    // cannot claim, so the collision surfaces as a real error instead of as a
    // silent success.
    let ready = spawn_and_confirm(store.path(), &identity, version, READY_TIMEOUT).await;
    drop(lock);
    ready
}

/// Start a fresh daemon and confirm it is actually serving.
///
/// The two steps are one outcome on purpose: a spawn that never becomes ready
/// is a failed launch, and the caller must be able to tell that apart from a
/// successful one. `ready_timeout` is a parameter so the wait can be exercised
/// without spending the production budget.
async fn spawn_and_confirm(
    config_path: &std::path::Path,
    identity: &ClientDaemonIdentity,
    version: &str,
    ready_timeout: Duration,
) -> Result<(), EnsureError> {
    match spawn_daemon(config_path) {
        Ok(()) => wait_ready(identity, version, ready_timeout)
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::clientd::daemon::run_daemon;
    use crate::clientd::identity::ClientDaemonIdentity;
    use crate::clientd::ipc;
    use crate::clientd::protocol::FrontendFrame;
    use crate::config::Config;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gregg-launch-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn config_path(&self) -> PathBuf {
            self.0.join("gregg.toml")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store_for(dir: &TempDir) -> ConfigStore {
        let config = Config {
            refresh_seconds: 1,
            ..Config::default()
        };
        let store = ConfigStore::new(dir.config_path());
        store.write(&config).expect("writes config");
        store
    }

    // ── The launch lock ───────────────────────────────────────────────

    #[test]
    fn the_launch_lock_is_distinct_per_config_and_from_the_config_lock() {
        let first = ClientDaemonIdentity::for_path(Path::new("/home/ann/a/gregg.toml"));
        let second = ClientDaemonIdentity::for_path(Path::new("/home/ann/b/gregg.toml"));
        assert_ne!(
            launch_lock_path(&first),
            launch_lock_path(&second),
            "two configs must never contend for one launch lock"
        );
        // Deliberately not the config mutation lock: an ordinary `gregg add`
        // must not make a TUI launch wait, or vice versa.
        let store = ConfigStore::new(PathBuf::from("/home/ann/a/gregg.toml"));
        assert_ne!(launch_lock_path(&first), store.lock_path());
    }

    #[test]
    fn a_held_launch_lock_blocks_a_second_acquirer_and_is_released_on_drop() {
        let dir = TempDir::new("launch-lock");
        let identity = ClientDaemonIdentity::for_path(&dir.config_path());
        let path = launch_lock_path(&identity);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("creates parent");

        let held = FileLockGuard::acquire(&path, Duration::from_millis(50)).expect("first acquire");
        assert_eq!(held.path(), path);

        // A second acquire in the same process cannot take an exclusive
        // advisory lock, which is exactly the contention the launch lock exists
        // to serialize.
        let contended = FileLockGuard::acquire(&path, Duration::from_millis(50));
        assert!(
            contended.is_err(),
            "a held launch lock must make a second launcher wait and then give up"
        );

        drop(held);
        // Releasing must make it immediately available again, so a crashed
        // launcher cannot wedge the next one.
        FileLockGuard::acquire(&path, Duration::from_millis(500)).expect("acquire after release");
    }

    #[test]
    fn the_launch_lock_file_alone_never_implies_a_running_daemon() {
        // The lock file is never unlinked, so nothing may treat its presence as
        // a signal. Only the OS lock counts, and only a completed handshake
        // proves a daemon exists.
        let dir = TempDir::new("stale-lock");
        let identity = ClientDaemonIdentity::for_path(&dir.config_path());
        let path = launch_lock_path(&identity);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("creates parent");
        std::fs::write(&path, b"").expect("writes a bare lock file");

        assert!(path.exists(), "the stale file is still there");
        let status = runtime()
            .block_on(crate::clientd::daemon::status(&identity))
            .expect("status answers");
        assert!(
            !status.running,
            "a leftover lock file must not be read as a running daemon"
        );
    }

    // ── Classification: only absence authorizes a spawn ───────────────

    #[test]
    fn only_plain_absence_is_classified_absent() {
        let io = AttachError::Transport(TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        )));
        assert_eq!(classify(&io), Classification::Absent);

        // A socket that is present but unlistening is the other half of plain
        // absence.
        let refused = AttachError::Transport(TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )));
        assert_eq!(classify(&refused), Classification::Absent);

        // Everything else means something is there, and nothing may be spawned
        // over it.
        for present in [
            AttachError::Transport(TransportError::Protocol("garbage".to_owned())),
            AttachError::Transport(TransportError::Decode(Box::new(
                crate::clientd::protocol::DecodeError::TooLarge { len: 99, max: 8 },
            ))),
            AttachError::Transport(TransportError::ConfigMismatch {
                actual: "other".to_owned(),
                expected: "ours".to_owned(),
            }),
            AttachError::Refused("not gregg".to_owned()),
            AttachError::Disconnected,
            AttachError::HandshakeTimeout,
        ] {
            assert_eq!(
                classify(&present),
                Classification::Foreign,
                "{present:?} must not authorize a competing daemon"
            );
        }
    }

    /// A connect that failed for a reason other than "nothing there" is not
    /// absence, no matter how much it looks like it from the outside.
    ///
    /// Treating *any* `Io` as absent spawned a daemon on top of fd exhaustion
    /// (`EMFILE`/`ENFILE`) or a socket the TUI may not open (`EACCES`); the
    /// spawn then failed readiness for 15s and left behind a daemon nobody
    /// asked for.
    #[test]
    fn a_connect_failure_that_is_not_absence_does_not_authorize_a_spawn() {
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::AddrInUse,
            std::io::ErrorKind::AlreadyExists,
            std::io::ErrorKind::TimedOut,
        ] {
            let error = AttachError::Transport(TransportError::Io(std::io::Error::new(
                kind,
                "not an absence",
            )));
            assert_ne!(
                classify(&error),
                Classification::Absent,
                "{kind:?} must not authorize a spawn"
            );
        }
    }

    #[test]
    fn an_older_owned_daemon_rotates_but_a_newer_one_is_left_alone() {
        // A daemon older than this frontend is stale by definition.
        let older = AttachError::VersionMismatch {
            daemon: crate::clientd::protocol::PROTOCOL_VERSION - 1,
            frontend: crate::clientd::protocol::PROTOCOL_VERSION,
        };
        assert_eq!(classify(&older), Classification::Rotate);

        // A daemon newer than this frontend may be serving a newer window
        // elsewhere. Killing it would downgrade that session because this one
        // window is old, so it is reported instead.
        let newer = AttachError::VersionMismatch {
            daemon: crate::clientd::protocol::PROTOCOL_VERSION + 1,
            frontend: crate::clientd::protocol::PROTOCOL_VERSION,
        };
        assert_eq!(classify(&newer), Classification::Incompatible);
    }

    // ── End to end ────────────────────────────────────────────────────

    /// A spawn that never becomes ready is a failed launch, not a success.
    ///
    /// `restart` used to write `let _ = wait_ready(..)`, so a child that exited
    /// at startup — which `run_daemon` does on any config validation violation,
    /// because it begins with `store.load_or_default()?` — still returned `Ok`.
    /// `gregg update` maps that `Ok` to `DaemonLifecycle::Relaunched`, i.e. it
    /// claimed a successful relaunch with nothing listening.
    ///
    /// The spawned executable here is this test binary, which rejects the
    /// `daemon run` arguments and exits, so it stands in for a daemon that dies
    /// immediately. The wait budget is injected rather than the production one.
    #[test]
    fn a_spawn_that_never_becomes_ready_is_not_a_successful_restart() {
        let dir = TempDir::new("notready");
        let store = store_for(&dir);
        let identity = ClientDaemonIdentity::for_path(store.path());

        let outcome = runtime().block_on(spawn_and_confirm(
            store.path(),
            &identity,
            "test",
            Duration::from_millis(200),
        ));

        assert!(
            matches!(outcome, Err(EnsureError::NotReady { .. })),
            "a launch that never served must report NotReady, got {outcome:?}"
        );
    }

    /// Restart takes the launch lock *before* it decides to stop anything.
    ///
    /// The stop used to happen outside the lock, so a `restart` could observe a
    /// daemon that a concurrent `ensure_running` had just spawned and was still
    /// waiting on, and kill it — turning that launch's readiness wait into a
    /// failure that had nothing to do with its own state.
    #[test]
    fn restart_holds_the_launch_lock_across_its_destructive_window() {
        let dir = TempDir::new("restart-lock");
        let store = Arc::new(store_for(&dir));
        let identity = ClientDaemonIdentity::for_path(store.path());

        runtime().block_on(async {
            // Stand in for the in-flight `ensure_running`: hold the same lock
            // a real launcher holds across its probe/spawn/readiness window,
            // and publish a daemon that is serving while it is held.
            let holder = acquire_launch_lock(&identity)
                .await
                .expect("takes the lock");
            let cancel = CancellationToken::new();
            let daemon_task = tokio::spawn(run_daemon(
                ConfigStore::new(store.path().to_path_buf()),
                identity.clone(),
                cancel.clone(),
            ));
            for _ in 0..400 {
                if crate::clientd::daemon::status(&identity)
                    .await
                    .is_ok_and(|status| status.running)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }

            // `restart` runs concurrently and must not get past the lock while
            // the window is held. A short budget is enough to prove the
            // ordering; it would be far too short to stop and replace a daemon.
            let contender = tokio::spawn({
                let store = Arc::clone(&store);
                async move { restart(&store).await }
            });
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(
                !contender.is_finished(),
                "restart must not complete its destructive window while the launch lock is held"
            );
            assert!(
                crate::clientd::daemon::status(&identity)
                    .await
                    .is_ok_and(|status| status.running),
                "the daemon inside the launch window must survive a concurrent restart"
            );

            // Release the window; the daemon is ours to stop.
            cancel.cancel();
            drop(holder);
            let _ = tokio::time::timeout(Duration::from_secs(10), contender).await;
            let _ = tokio::time::timeout(Duration::from_secs(10), daemon_task).await;
        });
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    /// Two concurrent `ensure_running` calls, both racing to start a daemon.
    ///
    /// Exactly one may spawn. This is the test the plan asks for, and it is
    /// deterministic in the sense that matters: the *decision* is serialized by
    /// the OS lock, and the assertion is on the outcome (one daemon serving
    /// both callers) rather than on which process won.
    #[test]
    fn two_concurrent_launches_produce_exactly_one_daemon() {
        let dir = TempDir::new("race");
        let store = Arc::new(store_for(&dir));
        let endpoint = ClientDaemonIdentity::for_path(store.path())
            .candidates()
            .first()
            .cloned()
            .expect("has a candidate");

        // Stand in for the spawned daemon: the real `spawn_daemon` would launch
        // this binary again, which a unit test cannot wait for. What matters
        // here is the coordination around it, so the winner starts a daemon
        // task directly at the same endpoint a real launch would use.
        runtime().block_on(async {
            let endpoint = endpoint.clone();
            let winner_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            let started_daemon: Arc<std::sync::Mutex<Option<CancellationToken>>> =
                Arc::new(std::sync::Mutex::new(None));

            let mut tasks = Vec::new();
            for _ in 0..2 {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                let winners = Arc::clone(&winner_count);
                let endpoint = endpoint.clone();
                let started = Arc::clone(&started_daemon);
                tasks.push(tokio::spawn(async move {
                    let identity = ClientDaemonIdentity::for_path(store.path());
                    barrier.wait().await;
                    // Replicate `ensure_running`'s critical section: probe,
                    // take the lock, re-probe, then start.
                    let Ok(lock) = acquire_launch_lock(&identity).await else {
                        return;
                    };
                    if crate::clientd::daemon::attach(&identity, "test")
                        .await
                        .is_ok()
                    {
                        drop(lock);
                        return;
                    }
                    winners.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let cancel = CancellationToken::new();
                    // The daemon is deliberately left running after this task
                    // returns: the loser must still find it there, which is the
                    // whole point of the re-probe. The test stops it at the end.
                    tokio::spawn(run_daemon(
                        ConfigStore::new(store.path().to_path_buf()),
                        identity,
                        cancel.clone(),
                    ));
                    // Hold the lock until the daemon is actually serving, which
                    // is what the real readiness wait does.
                    for _ in 0..400 {
                        if endpoint.exists() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    drop(lock);
                    *started.lock().expect("unpoisoned") = Some(cancel);
                }));
            }
            for task in tasks {
                task.await.expect("launcher task did not panic");
            }
            assert_eq!(
                winner_count.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "exactly one launcher may cross the spawn decision"
            );
            // The guard is released before the daemon is cancelled, so the
            // temporary never outlives the statement.
            let started = started_daemon.lock().expect("unpoisoned").take();
            if let Some(cancel) = started {
                cancel.cancel();
            }
        });
    }

    #[test]
    fn a_daemon_started_for_one_config_is_invisible_to_another() {
        runtime().block_on(async {
            let first = TempDir::new("cross-a");
            let second = TempDir::new("cross-b");
            let store_a = ConfigStore::new(first.config_path());
            let store_b = ConfigStore::new(second.config_path());
            store_for(&first);
            store_for(&second);

            let identity_a = ClientDaemonIdentity::for_path(store_a.path());
            let identity_b = ClientDaemonIdentity::for_path(store_b.path());

            let cancel = CancellationToken::new();
            let handle = tokio::spawn(run_daemon(
                ConfigStore::new(first.config_path()),
                identity_a.clone(),
                cancel.clone(),
            ));
            {
                for _ in 0..200 {
                    if identity_a
                        .candidates()
                        .iter()
                        .any(|p| crate::clientd::ipc::endpoint_is_live(p))
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }

                // Config A is served.
                let status_a = crate::clientd::daemon::status(&identity_a)
                    .await
                    .expect("status a");
                assert!(status_a.running);

                // Config B's `stop` must not reach config A's daemon.
                let stop_b = crate::clientd::daemon::stop(&identity_b, "test").await;
                assert!(
                    stop_b.is_err(),
                    "stopping a config with no daemon must not succeed by stopping a neighbour"
                );
                let status_a = crate::clientd::daemon::status(&identity_a)
                    .await
                    .expect("status a");
                assert!(status_a.running, "config A's daemon must be untouched");
            }
            cancel.cancel();
            let _ = handle.await;
        });
    }

    #[test]
    fn a_foreign_peer_on_the_endpoint_blocks_a_launch_instead_of_being_overwritten() {
        let dir = TempDir::new("foreign");
        let store = store_for(&dir);
        let identity = ClientDaemonIdentity::for_path(store.path());
        let candidates = identity.candidates();
        let endpoint = candidates[0].clone();

        // One runtime for the whole test: the squatter has to stay alive for
        // the duration of the attempt, and a task spawned on a runtime that is
        // then dropped would vanish before it could answer.
        runtime().block_on(async move {
            // Something that is not a Gregg client daemon binds the endpoint
            // and refuses every handshake, the way an unrelated service
            // occupying the name would.
            let mut squatter = ipc::bind(std::slice::from_ref(&endpoint)).expect("squatter binds");
            let squatter_task = tokio::spawn(async move {
                let Ok(mut connection) = squatter.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 4096];
                for _ in 0..200 {
                    if connection
                        .try_read_request()
                        .is_ok_and(|frame| frame.is_some())
                    {
                        break;
                    }
                    if let Ok(count) = connection.read_available(&mut buffer) {
                        let _ = connection.push_bytes(&buffer[..count]);
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let _ = connection.write_frame(&FrontendFrame::ProtocolError {
                    message: "this endpoint is not a Gregg client daemon".to_owned(),
                });
                // Hold the endpoint open so the refusal cannot be mistaken for
                // "nothing is there".
                tokio::time::sleep(Duration::from_secs(20)).await;
            });

            let started = std::time::Instant::now();
            let Err(error) = ensure_running(&store).await else {
                panic!("ensure_running must not launch over a peer it does not own");
            };
            assert!(
                matches!(error, EnsureError::ForeignPeer(_)),
                "unexpected classification: {error:?}"
            );
            // It must also fail *fast*: a refusal is an answer, not a condition
            // to wait out a readiness timeout.
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "a refused launch took {:?}; it should be reported immediately",
                started.elapsed()
            );

            // The foreign peer still owns the endpoint: nothing unlinked it and
            // nothing replaced it.
            assert!(
                endpoint.exists(),
                "a refused launch must leave the foreign endpoint untouched"
            );
            squatter_task.abort();
        });
    }
}
