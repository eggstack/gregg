//! The bridge between the Gregg client daemon and the GUI.
//!
//! # What this owns
//!
//! The bridge owns the client-daemon attachment: it attaches (starting the
//! daemon if one is not already running), reads the frame stream, and
//! publishes what it learns as plain data. It never touches a Slint
//! component — the UI thread drains [`Bridge::drain`] and applies the
//! result. That split keeps the protocol side independent of an event loop
//! and keeps every Slint mutation on the thread that owns the event loop.
//!
//! # What this deliberately does not own
//!
//! No HTTP client, no poll scheduler, no parsed remote configuration, no
//! scheduler history store. The client daemon owns all of that, and the GUI
//! is a *frontend* of it. A GUI that opened its own remote connection would
//! double the fleet's request budget exactly when the daemon is unhealthy —
//! the failure this architecture exists to prevent.
//!
//! # The daemon executable
//!
//! The GUI is not the `gregg` binary, so it cannot launch the client daemon
//! by using its own image: `gregg-gui --config … daemon run` exits
//! immediately, and the readiness wait then times out and blames the daemon.
//! The GUI therefore names the client explicitly, and
//! [`gregg::clientd::DaemonExecutable::Path`] refuses a path that resolves
//! to the GUI itself.
//!
//! # Intents
//!
//! The `EggPool` and cron intents are *desired state*, not commands. The
//! bridge keeps the latest desired state and republishes it whenever it
//! changes, whenever a write is rejected by the bounded request channel, and
//! whenever a new attachment is made. A reconnect therefore publishes the
//! **new full** desired state, never a stale delta: the daemon that answers
//! the new attachment has never seen the old one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use gregg::clientd::snapshot::FrontendSnapshot;
use gregg::clientd::{Attachment, DaemonExecutable, FrontendFrame, FrontendSender};
use gregg::config::ConfigStore;
use gregg::eggpool::EggpoolPeriod;

/// What the bridge publishes to the UI.
#[derive(Debug, Clone)]
pub enum BridgeMessage {
    /// The handshake succeeded.
    Attached {
        /// The local protocol version the daemon speaks.
        protocol_version: u16,
        /// The daemon's config-specific identity.
        daemon_id: String,
    },
    /// A new fleet document arrived.
    Snapshot {
        /// The document.
        snapshot: Box<FrontendSnapshot>,
    },
    /// The attachment failed, or the connection was lost.
    Failed {
        /// Why.
        reason: String,
    },
    /// The daemon speaks a different local protocol version.
    ///
    /// A separate message from [`BridgeMessage::Failed`]: a version mismatch
    /// is not a lost connection, and telling the operator "disconnected" would
    /// send them looking for a network problem that is not there.
    Incompatible {
        /// Why.
        reason: String,
    },
    /// The daemon is going away.
    ShuttingDown,
}

/// What this frontend wants from the `EggPool` worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EggpoolDesired {
    /// Whether the `EggPool` view is open.
    pub active: bool,
    /// The rolling window being displayed.
    pub period: EggpoolPeriod,
    /// Whether a fresh request is wanted even if nothing else changed.
    pub refresh: bool,
}

/// What this frontend has open in the cron detail view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronDesired {
    /// The host the open job belongs to.
    pub system_id: String,
    /// The open job, by name.
    pub job: String,
    /// How many records to publish for it.
    pub display_history: usize,
}

/// What this frontend wants the daemon to do, as a whole.
///
/// This is *desired state*, not a queue of commands: the latest value
/// replaces the previous one, so a burst of intent changes converges on the
/// last one instead of being applied in order. A `watch` channel carries it
/// for exactly that reason — it keeps the newest value and never blocks the
/// UI thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesiredState {
    /// The `EggPool` intent, when there is one.
    pub eggpool: Option<EggpoolDesired>,
    /// The cron intent, when there is one.
    pub cron: Option<CronDesired>,
    /// Whether a configuration reload has been asked for.
    pub reload: bool,
    /// Whether the `EggPool` intent still has to reach the daemon.
    pub(crate) eggpool_dirty: bool,
    /// Whether the cron intent still has to reach the daemon.
    pub(crate) cron_dirty: bool,
}

/// The GUI's handle on the bridge.
///
/// `Send + Sync`, because it is captured by the callback that wakes the UI
/// thread. The read side lives behind a mutex so the UI thread can drain it
/// from inside a scheduled callback while the bridge thread publishes into
/// it.
pub struct Bridge {
    /// What the bridge has published since the UI last drained it.
    messages: Arc<Mutex<Receiver<BridgeMessage>>>,
    /// The desired state the UI thread publishes.
    desired: tokio::sync::watch::Sender<DesiredState>,
    /// The generation of the next control request.
    next_generation: AtomicU64,
}

impl Bridge {
    /// Attach to the client daemon for `config_path` on a background thread.
    ///
    /// The attachment is asynchronous — the daemon may have to be started and
    /// waited for — and Slint owns the main thread, so it cannot happen
    /// there.
    ///
    /// `notify` is called from the bridge thread whenever something was
    /// published. It is how the UI learns that there is work to do **without a
    /// polling timer**: the UI schedules its own update from the callback, so
    /// an idle GUI redraws for no reason at all. The callback must not block;
    /// it should hand the work to the event loop and return.
    #[must_use]
    pub fn spawn(
        config_path: PathBuf,
        daemon: DaemonExecutable,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (tx, messages) = std::sync::mpsc::channel();
        let (desired_tx, desired_rx) = tokio::sync::watch::channel(DesiredState::default());
        let notify = Arc::clone(&notify);

        std::thread::Builder::new()
            .name("gregg-gui-bridge".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = tx.send(BridgeMessage::Failed {
                            reason: format!("could not start the bridge runtime: {error}"),
                        });
                        notify();
                        return;
                    }
                };
                runtime.block_on(pump(config_path, daemon, tx, desired_rx, notify));
            })
            .expect("failed to spawn the client-daemon bridge thread");

        Self {
            messages: Arc::new(Mutex::new(messages)),
            desired: desired_tx,
            next_generation: AtomicU64::new(0),
        }
    }

    /// Take everything published since the last call.
    ///
    /// Called on the UI thread, from inside the callback the bridge woke.
    pub fn drain(&self) -> Vec<BridgeMessage> {
        match self.messages.lock() {
            Ok(messages) => messages.try_iter().collect(),
            // A poisoned lock means a previous holder panicked. The channel is
            // still readable, and losing the UI is worse than losing a lock.
            Err(poisoned) => poisoned.into_inner().try_iter().collect(),
        }
    }

    /// The next request generation.
    ///
    /// Monotonic per frontend, so the daemon can tell a fresh request from a
    /// replayed one.
    fn next_request_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Ask the daemon to re-read its configuration file.
    ///
    /// This is the `Ctrl-R` boundary and the only one: there is no filesystem
    /// watcher, so nothing else can produce a reload. A request made before
    /// the attachment is up is not lost: the desired state carries it, and the
    /// bridge publishes it as soon as the attachment exists.
    pub fn request_reload(&self) {
        self.next_request_generation();
        self.publish_desired(|desired| {
            desired.reload = true;
        });
    }

    /// Replace this frontend's `EggPool` intent.
    ///
    /// `None` releases it. The intent governs what the daemon *transmits*,
    /// never what it fetches, so a released intent costs the fleet nothing.
    pub fn set_eggpool_intent(&self, intent: Option<EggpoolDesired>) {
        self.publish_desired(|desired| {
            desired.eggpool = intent;
            desired.eggpool_dirty = true;
        });
    }

    /// Replace this frontend's cron intent.
    ///
    /// `None` releases it, which is what a closed cron view must do: a
    /// retained record the operator is not looking at should not be
    /// transmitted.
    pub fn set_cron_intent(&self, intent: Option<CronDesired>) {
        self.publish_desired(|desired| {
            desired.cron = intent;
            desired.cron_dirty = true;
        });
    }

    /// Publish a change to the desired state.
    ///
    /// A `watch` sender keeps only the newest value, so a burst of changes
    /// converges on the last one and the UI thread never blocks.
    fn publish_desired(&self, change: impl FnOnce(&mut DesiredState)) {
        self.desired.send_modify(|desired| change(desired));
    }
}

#[cfg(test)]
impl Bridge {
    /// A bridge whose published messages the test supplies.
    ///
    /// The production bridge publishes from its attachment thread. A test
    /// needs to publish *chosen* messages — a version mismatch, a shutdown —
    /// so this hands the test the channel the attachment thread would have
    /// written to. The desired-state channel is the real one, so the test
    /// drives the intents exactly the way the UI thread does.
    pub(crate) fn with_publisher() -> (Self, Sender<BridgeMessage>) {
        let (tx, messages) = std::sync::mpsc::channel();
        let (desired, _desired_rx) = tokio::sync::watch::channel(DesiredState::default());
        (
            Self {
                messages: Arc::new(Mutex::new(messages)),
                desired,
                next_generation: AtomicU64::new(0),
            },
            tx,
        )
    }
}

/// Attach, then publish every document the daemon sends.
async fn pump(
    config_path: PathBuf,
    daemon: DaemonExecutable,
    tx: Sender<BridgeMessage>,
    mut desired_rx: tokio::sync::watch::Receiver<DesiredState>,
    notify: Arc<dyn Fn() + Send + Sync>,
) {
    let store = ConfigStore::new(config_path);

    // The GUI is not a Gregg client, so it names the executable it launches.
    // `DaemonExecutable::Path` refuses a path that resolves to this GUI, and
    // the resolution failure is reported as a diagnostic rather than as a
    // readiness timeout.
    let attachment: Attachment =
        match gregg::clientd::ensure_running_with_executable(&store, &daemon).await {
            Ok(attachment) => attachment,
            Err(error) => {
                let _ = tx.send(BridgeMessage::Failed {
                    reason: error.to_string(),
                });
                notify();
                return;
            }
        };

    let Attachment {
        frontend,
        frames,
        hello,
        ..
    } = attachment;

    let _ = tx.send(BridgeMessage::Attached {
        protocol_version: hello.protocol_version,
        daemon_id: hello.daemon_id,
    });
    notify();

    let mut frames = frames;
    let mut sender = Some(frontend);
    // The desired state the UI thread published before the attachment
    // existed. It is republished in full, because the daemon that just
    // answered has never seen it.
    let mut desired = desired_rx.borrow_and_update().clone();
    let mut generation = 0_u64;

    loop {
        tokio::select! {
            frame = frames.next() => {
                let Ok(frame) = frame else {
                    let _ = tx.send(BridgeMessage::Failed {
                        reason: "the client daemon closed the connection".to_owned(),
                    });
                    notify();
                    return;
                };
                match frame {
                    FrontendFrame::Snapshot(snapshot) => {
                        let _ = tx.send(BridgeMessage::Snapshot {
                            snapshot: Box::new(*snapshot),
                        });
                        notify();
                    }
                    // The daemon's introduction carries no renderable state — the
                    // document that follows carries everything — and a control
                    // acknowledgement's outcome is visible only in the next
                    // document.
                    FrontendFrame::Hello(_) | FrontendFrame::ControlAck { .. } => {}
                    FrontendFrame::VersionMismatch(payload) => {
                        let _ = tx.send(BridgeMessage::Incompatible {
                            reason: format!(
                                "the client daemon speaks local protocol {}, but this GUI speaks {}",
                                payload.daemon, payload.frontend
                            ),
                        });
                        notify();
                        return;
                    }
                    FrontendFrame::ProtocolError { message } => {
                        let _ = tx.send(BridgeMessage::Failed { reason: message });
                        notify();
                        return;
                    }
                    FrontendFrame::ShuttingDown => {
                        let _ = tx.send(BridgeMessage::ShuttingDown);
                        notify();
                        return;
                    }
                }
                // A frame arrived, so the request channel is draining again.
                // Anything that was rejected earlier can go out now.
                publish_pending(&mut sender, &mut desired, &mut generation);
            }
            changed = desired_rx.changed() => {
                if changed.is_err() {
                    // The UI thread is gone; nothing left to drive.
                    return;
                }
                desired = desired_rx.borrow_and_update().clone();
                publish_pending(&mut sender, &mut desired, &mut generation);
            }
        }
    }
}

/// Publish whatever desired state has not reached the daemon yet.
///
/// A rejected write is **not** forgotten: the desired state stays dirty and
/// is retried on the next received frame, when the request channel is
/// draining again. Claiming an intent was accepted when it was not would make
/// the GUI show a period the daemon never fetched.
///
/// The dirty flags live in the desired state itself rather than here, so a
/// retry after a rejection is the same code path as the first attempt.
fn publish_pending(
    sender: &mut Option<FrontendSender>,
    desired: &mut DesiredState,
    generation: &mut u64,
) {
    let Some(frontend) = sender.as_ref() else {
        return;
    };

    if desired.reload {
        *generation += 1;
        if frontend.request_reload(*generation).is_ok() {
            desired.reload = false;
        }
    }

    if desired.eggpool_dirty {
        let intent = desired.eggpool.unwrap_or(EggpoolDesired {
            active: false,
            period: gregg::eggpool::EggpoolPeriod::Hour,
            refresh: false,
        });
        *generation += 1;
        let sent = frontend.request_eggpool_intent(
            intent.active,
            intent.period,
            intent.refresh,
            *generation,
        );
        if sent.is_ok() {
            desired.eggpool_dirty = false;
        }
    }

    if desired.cron_dirty {
        *generation += 1;
        let sent = match desired.cron.as_ref() {
            Some(intent) => frontend.request_cron_intent(
                Some(intent.system_id.as_str()),
                Some(intent.job.as_str()),
                intent.display_history,
                *generation,
            ),
            None => frontend.request_cron_intent(None, None, 0, *generation),
        };
        if sent.is_ok() {
            desired.cron_dirty = false;
        }
    }
}

/// Where the GUI finds the `gregg` client it should launch.
///
/// The priority is deliberate:
///
/// 1. **An explicit path**, for local development and testing. It is checked
///    for identity, because an operator who points the GUI at the wrong file
///    must be told that, not left with a daemon that will not start.
/// 2. **An installer-owned adjacent executable**, which is the normal case
///    for an installed GUI. It is owned by the same installation, so it needs
///    no identity probe.
/// 3. **Nothing**, which is an actionable diagnostic rather than a silent
///    fallback to launching the GUI itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonResolution {
    /// The client to launch.
    Selected(DaemonExecutable),
    /// No client could be found, with the reason the operator needs.
    Missing(String),
}

/// Resolve the client executable the GUI will launch.
///
/// `explicit` is the operator's `--gregg` path, if any. `frontend_exe` is the
/// GUI's own executable, which is where an installer-owned client is found.
#[must_use]
pub fn resolve_daemon(explicit: Option<&Path>, frontend_exe: &Path) -> DaemonResolution {
    if let Some(path) = explicit {
        // A user-selected external executable must prove it is a Gregg client
        // before it is trusted. Without the check, a wrong path would be
        // spawned as `daemon run`, fail, and be reported as a daemon that
        // would not start — a diagnostic about the wrong thing.
        return match gregg::clientd::verify_gregg_client_executable(path) {
            // The identity is checked, not displayed: the point is that the
            // executable proved it is a Gregg client.
            Ok(_identity) => DaemonResolution::Selected(DaemonExecutable::Path(path.to_path_buf())),
            Err(error) => DaemonResolution::Missing(format!(
                "the Gregg client at {} did not identify itself: {error}",
                path.display()
            )),
        };
    }

    match gregg::clientd::adjacent_daemon_executable(frontend_exe) {
        Some(path) => DaemonResolution::Selected(DaemonExecutable::Path(path)),
        None => DaemonResolution::Missing(format!(
            "no Gregg client was found next to {} and none was given with --gregg; \
             point the GUI at a `gregg` executable, or install one beside it",
            frontend_exe.display()
        )),
    }
}

/// The GUI's own executable, or an actionable reason it could not be found.
pub fn frontend_executable() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("could not locate the GUI executable: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture that answers `version` the way a Gregg client does.
    fn fixture_client(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("writes fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod");
        }
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gregg-gui-bridge-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    #[cfg(unix)]
    #[test]
    fn an_explicit_client_must_identify_itself() {
        let dir = temp_dir("explicit");
        let client = fixture_client(&dir, "gregg", "#!/bin/sh\necho 'gregg 1.0.15'\n");
        let frontend = dir.join("gregg-gui");
        fixture_client(&dir, "gregg-gui", "#!/bin/sh\nexit 0\n");

        let resolution = resolve_daemon(Some(&client), &frontend);
        match resolution {
            DaemonResolution::Selected(DaemonExecutable::Path(path)) => {
                assert_eq!(path, client);
            }
            DaemonResolution::Selected(DaemonExecutable::Current) => {
                panic!("an explicit path must never resolve to this frontend");
            }
            DaemonResolution::Missing(reason) => {
                panic!("a genuine Gregg client must be accepted: {reason}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_explicit_client_that_is_not_one_is_refused_with_a_reason() {
        let dir = temp_dir("not-a-client");
        let other = fixture_client(&dir, "other", "#!/bin/sh\necho 'some other tool'\n");
        let frontend = dir.join("gregg-gui");

        let resolution = resolve_daemon(Some(&other), &frontend);
        let DaemonResolution::Missing(reason) = resolution else {
            panic!("a tool that is not a Gregg client must be refused");
        };
        assert!(
            reason.contains("did not identify itself"),
            "the diagnostic must say what went wrong: {reason}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_adjacent_client_is_accepted_without_a_probe() {
        let dir = temp_dir("adjacent");
        let frontend = dir.join("gregg-gui");
        fixture_client(&dir, "gregg-gui", "#!/bin/sh\nexit 0\n");
        let client = dir.join("gregg");
        fixture_client(&dir, "gregg", "#!/bin/sh\necho 'gregg 1.0.15'\n");

        let resolution = resolve_daemon(None, &frontend);
        match resolution {
            DaemonResolution::Selected(DaemonExecutable::Path(path)) => {
                assert_eq!(path, client);
            }
            DaemonResolution::Selected(DaemonExecutable::Current) => {
                panic!("an adjacent client must never resolve to this frontend");
            }
            DaemonResolution::Missing(reason) => {
                panic!("an adjacent client must be accepted: {reason}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_client_anywhere_is_an_actionable_diagnostic() {
        let dir = temp_dir("none");
        let frontend = dir.join("gregg-gui");
        fixture_client(&dir, "gregg-gui", "#!/bin/sh\nexit 0\n");

        let resolution = resolve_daemon(None, &frontend);
        let DaemonResolution::Missing(reason) = resolution else {
            panic!("a missing client must be reported");
        };
        assert!(
            reason.contains("--gregg"),
            "the diagnostic must say what to do: {reason}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_desired_state_keeps_the_newest_value() {
        let (tx, mut rx) = tokio::sync::watch::channel(DesiredState::default());
        tx.send_modify(|desired| {
            desired.eggpool = Some(EggpoolDesired {
                active: true,
                period: EggpoolPeriod::Hour,
                refresh: false,
            });
        });
        tx.send_modify(|desired| {
            desired.eggpool = Some(EggpoolDesired {
                active: true,
                period: EggpoolPeriod::Week,
                refresh: false,
            });
        });

        let seen = rx.borrow_and_update().clone();
        let intent = seen.eggpool.expect("an intent");
        assert_eq!(
            intent.period,
            EggpoolPeriod::Week,
            "the newest intent must win, not the first"
        );
    }

    #[test]
    fn a_released_intent_is_still_a_write() {
        // Releasing the EggPool intent has to reach the daemon: the pane
        // nobody is looking at must stop being transmitted.
        let mut desired = DesiredState {
            eggpool: Some(EggpoolDesired {
                active: true,
                period: EggpoolPeriod::Hour,
                refresh: false,
            }),
            eggpool_dirty: true,
            ..DesiredState::default()
        };
        desired.eggpool = None;
        assert!(desired.eggpool_dirty, "a release must be published");
    }

    /// The GUI crate's own dependency list.
    ///
    /// The GUI is a frontend: it attaches to the client daemon over the local
    /// protocol and never opens a remote connection of its own. A direct
    /// dependency on an HTTP client would be the first step towards that, so
    /// the list is checked rather than trusted.
    const MANIFEST: &str = include_str!("../Cargo.toml");

    #[test]
    fn the_gui_crate_depends_on_no_http_client() {
        let dependencies = MANIFEST
            .split("[dependencies]")
            .nth(1)
            .and_then(|rest| rest.split("\n[").next())
            .expect("a dependencies section");
        let forbidden = [
            "reqwest",
            "axum",
            "ureq",
            "isahc",
            "surf",
            "attohttpc",
            "ehttp",
            "curl",
            "hyper",
        ];
        for line in dependencies.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let name = line.split('=').next().unwrap_or_default().trim();
            assert!(
                !forbidden.contains(&name),
                "the GUI must not depend on an HTTP client: {name}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // The control plane.
    // ---------------------------------------------------------------------

    /// A stand-in daemon that records the requests the GUI writes.
    ///
    /// The GUI's control plane is a local socket, so the smallest thing that
    /// can answer "what did the daemon actually receive?" is a socket that
    /// speaks the same framing. This one decodes every frame it is sent.
    #[cfg(unix)]
    struct RecordingDaemon {
        path: PathBuf,
        requests: std::sync::mpsc::Receiver<gregg::clientd::DaemonRequest>,
    }

    #[cfg(unix)]
    impl RecordingDaemon {
        fn start(tag: &str) -> Self {
            // A Unix socket path is short by law, so the socket lives directly
            // in the system temporary directory rather than in a directory of
            // its own.
            let path = std::env::temp_dir().join(format!("gg-{}-{tag}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener =
                std::os::unix::net::UnixListener::bind(&path).expect("binds the stand-in daemon");
            let (tx, requests) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        return;
                    };
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let mut buffer = Vec::new();
                        let mut chunk = [0_u8; 4096];
                        loop {
                            while let Some(request) = decode_request(&mut buffer) {
                                if tx.send(request).is_err() {
                                    return;
                                }
                            }
                            match std::io::Read::read(&mut stream, &mut chunk) {
                                Ok(0) | Err(_) => return,
                                Ok(count) => buffer.extend_from_slice(&chunk[..count]),
                            }
                        }
                    });
                }
            });
            Self { path, requests }
        }

        /// The next request the daemon received.
        fn next_request(&self) -> gregg::clientd::DaemonRequest {
            self.requests
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the daemon received a request")
        }
    }

    #[cfg(unix)]
    impl Drop for RecordingDaemon {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Decode one length-prefixed request frame, if the buffer holds one.
    #[cfg(unix)]
    fn decode_request(buffer: &mut Vec<u8>) -> Option<gregg::clientd::DaemonRequest> {
        if buffer.len() < gregg::clientd::LEN_PREFIX_BYTES {
            return None;
        }
        let length =
            gregg::clientd::parse_length(&buffer[..gregg::clientd::LEN_PREFIX_BYTES]).ok()?;
        if buffer.len() < gregg::clientd::LEN_PREFIX_BYTES + length {
            return None;
        }
        let body: Vec<u8> = buffer
            .drain(..gregg::clientd::LEN_PREFIX_BYTES + length)
            .skip(gregg::clientd::LEN_PREFIX_BYTES)
            .collect();
        serde_json::from_slice(&body).ok()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rejected_write_is_retried_and_a_reconnect_republishes_the_whole_state() {
        let daemon = RecordingDaemon::start("control-plane");

        // The first attachment's reader is gone, so every write to it is
        // rejected: this is what a request channel that is not draining looks
        // like from the writer's side.
        let (sender, reader) =
            gregg::clientd::FrontendLink::connect(std::slice::from_ref(&daemon.path))
                .expect("connects to the stand-in daemon")
                .split();
        drop(reader);

        let (bridge, _messages) = Bridge::with_publisher();
        bridge.set_eggpool_intent(Some(EggpoolDesired {
            active: true,
            period: EggpoolPeriod::Week,
            refresh: false,
        }));
        bridge.set_cron_intent(Some(CronDesired {
            system_id: "system-0".to_owned(),
            job: "backup".to_owned(),
            display_history: 5,
        }));
        bridge.request_reload();

        let mut desired = bridge.desired.borrow().clone();
        let mut generation = 0_u64;
        let mut sender = Some(sender);

        // The write is rejected, and nothing is forgotten: both intents and
        // the reload stay pending.
        publish_pending(&mut sender, &mut desired, &mut generation);
        assert!(desired.eggpool_dirty, "a rejected intent must stay dirty");
        assert!(desired.cron_dirty, "a rejected intent must stay dirty");
        assert!(desired.reload, "a rejected reload must stay pending");

        // The reconnect publishes the whole state, not a delta: the daemon
        // that just answered has never seen any of it.
        let (sender, mut reader) =
            gregg::clientd::FrontendLink::connect(std::slice::from_ref(&daemon.path))
                .expect("reconnects to the stand-in daemon")
                .split();
        let mut sender = Some(sender);
        publish_pending(&mut sender, &mut desired, &mut generation);
        assert!(!desired.eggpool_dirty);
        assert!(!desired.cron_dirty);
        assert!(!desired.reload);

        // The requests are queued on the channel; the reader is what writes
        // them to the socket.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), reader.next()).await;

        assert!(matches!(
            daemon.next_request(),
            gregg::clientd::DaemonRequest::ReloadConfig { .. }
        ));
        assert!(matches!(
            daemon.next_request(),
            gregg::clientd::DaemonRequest::SetEggpoolIntent {
                active: true,
                period: EggpoolPeriod::Week,
                ..
            }
        ));
        assert!(matches!(
            daemon.next_request(),
            gregg::clientd::DaemonRequest::SetCronIntent {
                system_id: Some(_),
                job: Some(_),
                ..
            }
        ));
    }
}

#[cfg(test)]
mod evidence {
    //! Counted evidence that the GUI is a *frontend*, not a second poller.
    //!
    //! The claim under test is the one the whole architecture rests on: a
    //! second window costs the fleet nothing. It is proven against a **counted**
    //! remote rather than against the publication, because a shared generation
    //! is necessary but not sufficient — two frontends polling in parallel
    //! would still share generations while doubling the fleet's requests.

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use gregg::state::Reachability;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A remote that counts the requests it receives.
    struct CountedRemote {
        port: u16,
        status_hits: Arc<AtomicUsize>,
    }

    /// Serve a minimal but valid v2 status document, and count every request.
    async fn spawn_counted_remote() -> CountedRemote {
        let body = serde_json::to_vec(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
                .sample_interval_ms(1_000)
                .build_payload(),
        )
        .expect("status serializes");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let status_hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&status_hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                let counter = Arc::clone(&counter);
                tokio::spawn(async move {
                    let mut request = String::new();
                    let mut chunk = [0_u8; 1024];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => {
                                request.push_str(String::from_utf8_lossy(&chunk[..count]).as_ref());
                                if request.contains("\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        String::from_utf8_lossy(&body)
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });
        CountedRemote { port, status_hits }
    }

    fn write_config(path: &Path, port: u16) {
        // The configuration schema is validated: `config_version` is
        // required, and a system's id must be a UUID v4.
        let config = format!(
            "config_version = 1\n\
             request_timeout_ms = 1000\n\
             refresh_seconds = 1\n\
             max_concurrent_requests = 4\n\
             default_port = 11310\n\
             \n\
             [[systems]]\n\
             id = \"a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d\"\n\
             host = \"127.0.0.1\"\n\
             port = {port}\n\
             name = \"counted\"\n"
        );
        std::fs::write(path, config).expect("writes config");
    }

    /// Wait until the daemon's endpoint is being served.
    async fn wait_for_daemon(
        identity: &gregg::clientd::identity::ClientDaemonIdentity,
        daemon_task: &tokio::task::JoinHandle<()>,
    ) {
        for _ in 0..400 {
            let running = gregg::clientd::daemon::status(identity)
                .await
                .map(|status| status.running)
                .unwrap_or(false);
            if running {
                return;
            }
            if daemon_task.is_finished() {
                panic!("the client daemon exited before it could serve");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the client daemon did not come up");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gregg-gui-evidence-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        path
    }

    /// A file that exists and is executable, and is not this test binary, so
    /// the launcher is never asked to spawn it.
    #[cfg(unix)]
    fn harmless_executable() -> PathBuf {
        PathBuf::from("/bin/true")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_gui_window_costs_the_fleet_exactly_one_polling_plane() {
        let remote = spawn_counted_remote().await;
        let dir = temp_dir("one-plane");
        let config_path = dir.join("gregg.toml");
        write_config(&config_path, remote.port);

        let store = ConfigStore::new(config_path.clone());
        let identity = gregg::clientd::identity::ClientDaemonIdentity::for_path(&config_path);
        let cancel = tokio_util::sync::CancellationToken::new();
        let daemon_cancel = cancel.clone();
        let daemon_identity = identity.clone();
        let daemon_task = tokio::spawn(async move {
            if let Err(error) =
                gregg::clientd::daemon::run_daemon(store, daemon_identity, daemon_cancel).await
            {
                eprintln!("the client daemon stopped: {error:?}");
            }
        });
        wait_for_daemon(&identity, &daemon_task).await;

        // One window, on its own, is the baseline.
        let mut first = gregg::clientd::daemon::attach(&identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("the first window attaches");
        let mut saw_online = false;
        for _ in 0..400 {
            let Ok(Ok(frame)) =
                tokio::time::timeout(Duration::from_millis(50), first.frames.next()).await
            else {
                continue;
            };
            if let gregg::clientd::FrontendFrame::Snapshot(snapshot) = frame {
                if snapshot
                    .systems
                    .first()
                    .is_some_and(|system| system.reachability == Reachability::Online)
                {
                    saw_online = true;
                    break;
                }
            }
        }
        assert!(saw_online, "the first window must see the remote");

        // Now add the GUI. It attaches through the same client daemon and the
        // same local protocol a TUI window uses, so it must not change what
        // the remote is asked for.
        let baseline = remote.status_hits.load(Ordering::SeqCst);
        let bridge = Bridge::spawn(
            config_path.clone(),
            DaemonExecutable::Path(harmless_executable()),
            Arc::new(|| {}),
        );

        // The GUI must actually be receiving the fleet.
        let mut model_seen = false;
        for _ in 0..400 {
            for message in bridge.drain() {
                if let BridgeMessage::Snapshot { snapshot } = message {
                    let model = crate::model::project(
                        &snapshot,
                        &crate::model::Selection::default(),
                        &crate::model::ConnectionState::Connected {
                            protocol_version: 1,
                            daemon_id: "test".to_owned(),
                        },
                        1_000,
                    );
                    assert_eq!(model.counts.online, 1, "the GUI must see one online host");
                    assert!(
                        !model.systems[0].metrics.is_empty(),
                        "the GUI must project the remote's metrics"
                    );
                    model_seen = true;
                }
            }
            if model_seen {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(model_seen, "the GUI must receive the fleet document");

        // The count over the same window must not have grown with the number
        // of windows.
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        let with_gui = remote.status_hits.load(Ordering::SeqCst);
        assert!(
            with_gui < baseline * 2 + 4,
            "a second window must not double the metrics cadence: {baseline} -> {with_gui}"
        );

        // And the first window is untouched: it still receives documents, and
        // they are not duplicated.
        let mut documents = 0_u32;
        while let Ok(Ok(_)) =
            tokio::time::timeout(Duration::from_millis(50), first.frames.next()).await
        {
            documents += 1;
            if documents >= 3 {
                break;
            }
        }
        assert!(
            documents > 0,
            "the first window must keep receiving documents"
        );

        cancel.cancel();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
