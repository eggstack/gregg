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

    if let Some(outcome) = probe(&identity, version).await? {
        return Ok(outcome);
    }

    // Absent. Exactly one process may cross this point per config.
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
    if let Some(outcome) = probe(&identity, version).await? {
        drop(lock);
        return Ok(outcome);
    }

    spawn_daemon(store.path())?;
    let ready = wait_ready(&identity, version, READY_TIMEOUT).await;
    // The lock is released before attaching so a slow client never blocks
    // another launch; readiness has already been established at this point.
    drop(lock);
    ready
}

/// One attach attempt, resolved into "ready", "leave it alone", or "absent".
///
/// `Ok(Some(attachment))` means attach. `Ok(None)` means the endpoint is
/// genuinely absent and a launch is authorized. `Err` means something is there
/// that this binary must not touch, with the reason already classified.
async fn probe(
    identity: &ClientDaemonIdentity,
    version: &str,
) -> Result<Option<Attachment>, EnsureError> {
    match daemon::attach(identity, version).await {
        Ok(attachment) => Ok(Some(attachment)),
        Err(error) => match classify(&error) {
            Classification::Absent => Ok(None),
            Classification::Rotate => {
                rotate(identity, version).await?;
                daemon::attach(identity, version)
                    .await
                    .map(Some)
                    .map_err(EnsureError::Transport)
            }
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
/// probe/spawn/readiness window and never to the daemon's lifetime.
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
        // A connect that could not even be made, or a peer that hung up before
        // saying anything, is what "nothing is serving this endpoint" looks
        // like from here. Only this authorizes a spawn.
        AttachError::Transport(TransportError::Io(_)) => Classification::Absent,
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
        // A refusal, a peer that hung up without identifying itself, or a
        // connect that failed for any reason other than "nothing there" is
        // somebody else's service. None of it is absence: spawning over a peer
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
    if daemon::status(&identity).await?.running {
        daemon::stop(&identity, version)
            .await
            .map_err(EnsureError::Transport)?;
        wait_gone(&identity, ROTATE_STOP_TIMEOUT).await;
    }
    let lock = acquire_launch_lock(&identity).await?;
    if !daemon::status(&identity).await?.running {
        spawn_daemon(store.path())?;
        let _ = wait_ready(&identity, version, READY_TIMEOUT).await;
    }
    drop(lock);
    Ok(())
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
                        connection.push_bytes(&buffer[..count]);
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
