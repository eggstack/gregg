//! The optional Gregg desktop frontend.
//!
//! # What this is
//!
//! A second way to *look* at the fleet, never a second way to *poll* it. The
//! GUI attaches to the existing per-config client daemon over the same local
//! protocol the terminal client uses, and renders the documents that daemon
//! already publishes. A TUI window and a GUI window therefore cost the fleet
//! exactly what one window costs.
//!
//! # How it is put together
//!
//! - [`model`] is a **pure projection**: one daemon document in, one view
//!   model out. Every truthfulness rule the GUI keeps — absent is absent,
//!   unpolled is not offline, selection is keyed by stable id — lives there,
//!   where it is testable without a desktop session.
//! - [`bridge`] owns the client-daemon attachment and the intents. It never
//!   touches a Slint component.
//! - This file owns the Slint components and the wiring between them.
//!
//! # How it learns that there is work to do
//!
//! The bridge calls a notification callback whenever it publishes something,
//! and the callback schedules the update on the UI thread with
//! [`slint::invoke_from_event_loop`]. There is **no polling timer**: an idle
//! GUI redraws for no reason at all, and a redraw only ever happens because
//! something actually changed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, SharedString};

mod bridge;
mod model;
mod platform;

#[cfg(test)]
mod evidence;

use bridge::{Bridge, BridgeMessage, CronDesired, DaemonResolution, EggpoolDesired};
use model::{ConnectionState, GuiViewModel, Selection};

slint::include_modules!();

/// The Slint attribution the royalty-free Slint license requires.
///
/// Slint's royalty-free desktop license (`LicenseRef-Slint-Royalty-free-2.0`)
/// requires that an application credit Slint and point at the license and the
/// download page. A GUI that ships without that credit does not satisfy the
/// license it was built under, so the text is part of the application rather
/// than a README nobody reads.
const SLINT_ATTRIBUTION: &str = "\
Built with Slint (https://slint.dev), used under the royalty-free \
LicenseRef-Slint-Royalty-free-2.0. \
See https://slint.dev/legal/licenses/ for the license and https://slint.dev/ \
for downloads. \
Gregg is MIT licensed; see the project repository.";

/// How many scheduler records the GUI asks the daemon to publish.
const CRON_DISPLAY_HISTORY: usize = 5;

/// The GUI's own state.
///
/// It lives behind a mutex rather than a `RefCell` because the bridge's
/// notification callback captures it and runs on the bridge's thread, even
/// though the state itself is only ever *read* on the UI thread.
#[derive(Default)]
struct UiState {
    /// The last document the daemon published.
    snapshot: Option<Box<gregg::clientd::snapshot::FrontendSnapshot>>,
    /// The last generation applied, so a stale document is never applied.
    generation: u64,
    /// Which host is selected, by stable id.
    selection: Selection,
    /// The connection state.
    connection: ConnectionState,
    /// Whether the `EggPool` pane is open.
    eggpool_visible: bool,
    /// The health the tray icon is currently showing, so it is only redrawn
    /// when the health actually changes.
    tray_health: Option<TrayHealth>,
}

/// The parsed command line.
struct Options {
    /// The `gregg` client executable to launch, if the operator named one.
    gregg: Option<PathBuf>,
    /// The configuration file.
    config: PathBuf,
}

fn parse_options() -> Result<Option<Options>, String> {
    let mut gregg: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gregg" => {
                let value = args.next().ok_or("--gregg needs a path")?;
                gregg = Some(PathBuf::from(value));
            }
            "--config" => {
                let value = args.next().ok_or("--config needs a path")?;
                config = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(None);
            }
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }
    Ok(Some(Options {
        gregg,
        config: config.unwrap_or_else(gregg::config::Config::default_path),
    }))
}

/// The usage text.
fn usage() -> String {
    "gregg-gui — the optional Gregg desktop frontend\n\
     \n\
     Usage: gregg-gui [--config <path>] [--gregg <path>]\n\
     \n\
       --config <path>  the configuration file to read\n\
       --gregg <path>   the `gregg` client executable to launch as the client daemon\n\
       --help           print this message\n\
     \n\
     The GUI is a frontend: it attaches to the per-config client daemon and\n\
     renders what that daemon already polls. It never polls the fleet itself."
        .to_owned()
}

/// The current wall clock, in Unix milliseconds.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// A small procedural tray icon, in the fleet's health colour.
///
/// No binary asset is committed: an image file in the repository is a thing
/// that can go stale, and a tray icon is a circle.
///
/// The colour is **light** on purpose. A menu bar is dark on both macOS's
/// dark appearance and on most Linux desktops, and a dark icon on a dark bar
/// is invisible — which is exactly how a tray icon that "works" can still
/// leave the operator with no tray entry at all.
fn tray_icon(health: TrayHealth) -> slint::Image {
    const SIZE: u32 = 32;
    let (red, green, blue) = health.rgb();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(SIZE, SIZE);
    let stride = SIZE as usize * 4;
    {
        let pixels = buffer.make_mut_bytes();
        let center = (SIZE as f32 - 1.0) / 2.0;
        let radius = SIZE as f32 * 0.42;
        for y in 0..SIZE as usize {
            for x in 0..SIZE as usize {
                let dx = x as f32 - center;
                let dy = y as f32 - center;
                let distance = (dx * dx + dy * dy).sqrt();
                // A soft edge, so the icon does not look like a jagged blob at
                // the small sizes a tray uses.
                let alpha = ((radius - distance).max(0.0) * 255.0).min(255.0) as u8;
                let offset = y * stride + x * 4;
                pixels[offset] = red;
                pixels[offset + 1] = green;
                pixels[offset + 2] = blue;
                pixels[offset + 3] = alpha;
            }
        }
    }
    slint::Image::from_rgba8(buffer)
}

/// The fleet's health, as the tray shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayHealth {
    /// Every host answered.
    Healthy,
    /// Some host has not been polled yet.
    Pending,
    /// Some host's last poll failed.
    Offline,
}

impl TrayHealth {
    /// The icon colour.
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            // Light green, light amber, light red: all of them read against a
            // dark menu bar.
            Self::Healthy => (0x6e, 0xd1, 0x6e),
            Self::Pending => (0xf2, 0xc0, 0x6e),
            Self::Offline => (0xf2, 0x7b, 0x6e),
        }
    }
}

fn main() {
    let options = match parse_options() {
        Ok(Some(options)) => options,
        Ok(None) => return,
        Err(error) => {
            eprintln!("gregg-gui: {error}\n\n{}", usage());
            std::process::exit(2);
        }
    };

    // The GUI is not a Gregg client, so it names the executable it launches.
    // A missing or unusable helper is an actionable diagnostic, never a
    // silent fallback to launching the GUI itself.
    let frontend_exe = match bridge::frontend_executable() {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("gregg-gui: {reason}");
            std::process::exit(2);
        }
    };
    let daemon = match bridge::resolve_daemon(options.gregg.as_deref(), &frontend_exe) {
        DaemonResolution::Selected(daemon) => daemon,
        DaemonResolution::Missing(reason) => {
            eprintln!("gregg-gui: {reason}");
            std::process::exit(2);
        }
    };

    let window = MainWindow::new().expect("the window component is constructible");
    let tray = TrayIcon::new().expect("the tray component is constructible");
    tray.set_tray_icon(tray_icon(TrayHealth::Pending));

    let state: Arc<Mutex<UiState>> = Arc::new(Mutex::new(UiState {
        connection: ConnectionState::Connecting,
        ..UiState::default()
    }));

    // The bridge needs a callback it can call from its own thread. The
    // callback only *schedules* work on the UI thread; it never touches a
    // component itself, because Slint components are not `Send`.
    // The bridge is created with the notification callback, and the callback
    // needs the bridge, so the handle is published through a cell the
    // callback can read once the bridge exists.
    let bridge_slot: Arc<OnceLock<Arc<Bridge>>> = Arc::new(OnceLock::new());
    let notify_state = Arc::clone(&state);
    let notify_window = window.as_weak();
    let notify_tray = tray.as_weak();
    let notify_slot = Arc::clone(&bridge_slot);
    let notify = Arc::new(move || {
        let state = Arc::clone(&notify_state);
        let window = notify_window.clone();
        let tray = notify_tray.clone();
        let slot = Arc::clone(&notify_slot);
        // `invoke_from_event_loop` fails when the event loop is not running
        // yet. The message stays queued in the bridge, and the first drain
        // after the loop starts picks it up, so nothing is lost.
        let _ = slint::invoke_from_event_loop(move || {
            let Some(bridge) = slot.get() else {
                return;
            };
            apply_pending(bridge, &state, &window, &tray);
        });
    });

    let bridge = Arc::new(Bridge::spawn(options.config.clone(), daemon, notify));
    let _ = bridge_slot.set(Arc::clone(&bridge));

    // The callbacks run on the UI thread, so the state is taken behind its
    // mutex rather than through a `RefCell` borrow that could conflict with
    // the scheduled update.
    let callback_bridge = Arc::clone(&bridge);
    window.on_refresh_requested(move || {
        callback_bridge.request_reload();
    });

    window.on_quit_requested(move || {
        let _ = slint::quit_event_loop();
    });

    let callback_state = Arc::clone(&state);
    let callback_bridge = Arc::clone(&bridge);
    window.on_host_selected(move |index| {
        let mut state = callback_state.lock().expect("state lock");
        let Some(snapshot) = state.snapshot.as_ref() else {
            return;
        };
        let Some(card) = snapshot.systems.get(index as usize) else {
            return;
        };
        state.selection.selected_id = Some(card.id.clone());
        // Changing host closes the open cron job: the retained records belong
        // to a job of the *previous* host, and the intent for it must be
        // released so the daemon stops transmitting them.
        state.selection.cron_job = None;
        callback_bridge.set_cron_intent(None);
    });

    let callback_state = Arc::clone(&state);
    let callback_bridge = Arc::clone(&bridge);
    window.on_job_selected(move |job| {
        let mut state = callback_state.lock().expect("state lock");
        let Some(selected) = state.selection.selected_id.clone() else {
            return;
        };
        state.selection.cron_job = Some((selected.clone(), job.to_string()));
        // The cron intent is published for the *open* job only. A closed
        // view releases it, so a retained record nobody is looking at stops
        // being transmitted.
        callback_bridge.set_cron_intent(Some(CronDesired {
            system_id: selected,
            job: job.to_string(),
            display_history: CRON_DISPLAY_HISTORY,
        }));
    });

    let callback_state = Arc::clone(&state);
    let callback_bridge = Arc::clone(&bridge);
    window.on_eggpool_toggled(move || {
        let mut state = callback_state.lock().expect("state lock");
        state.eggpool_visible = !state.eggpool_visible;
        // The `EggPool` intent governs what the daemon *transmits*, never what
        // it fetches, so a released intent costs the fleet nothing.
        if state.eggpool_visible {
            callback_bridge.set_eggpool_intent(Some(EggpoolDesired {
                active: true,
                period: gregg::eggpool::EggpoolPeriod::Hour,
                refresh: false,
            }));
        } else {
            callback_bridge.set_eggpool_intent(None);
        }
    });

    let about_window = window.as_weak();
    window.on_about_requested(move || {
        if let Some(window) = about_window.upgrade() {
            window.set_about_text(SLINT_ATTRIBUTION.into());
        }
    });

    // The tray's menu items.
    let tray_window = window.as_weak();
    tray.on_show_window(move || {
        if let Some(window) = tray_window.upgrade() {
            // The application is activated *before* the window is shown. On
            // macOS a background application cannot bring its own window to
            // the front, and a tray application is a background application —
            // so without this, "Open Monitor" from the tray does nothing at
            // all when the GUI was launched the way a real one is launched.
            platform::activate();
            let _ = window.show();
        }
    });

    let callback_bridge = Arc::clone(&bridge);
    tray.on_refresh_requested(move || {
        callback_bridge.request_reload();
    });

    let tray_window = window.as_weak();
    tray.on_about_requested(move || {
        if let Some(window) = tray_window.upgrade() {
            window.set_about_text(SLINT_ATTRIBUTION.into());
            platform::activate();
            let _ = window.show();
        }
    });

    tray.on_quit_requested(move || {
        let _ = slint::quit_event_loop();
    });

    // A desktop with no tray service still needs a usable window, and a
    // process the operator can always quit. The window is shown either way.
    if let Err(error) = window.show() {
        eprintln!("gregg-gui: could not show the monitor window: {error}");
    }

    // The first drain happens after the event loop is running, so a message
    // published before it started is not lost.
    //
    // `run_event_loop_until_quit` rather than `run_event_loop`: closing the
    // monitor window must **hide** it and leave the tray and the observation
    // running, and the daemon must keep polling. Slint's own close handling
    // hides the window on a close request, and an event loop that quits when
    // its last window closes would turn that into "the GUI exited". Quit is
    // the only thing that ends the GUI.
    if let Err(error) = slint::run_event_loop_until_quit() {
        eprintln!("gregg-gui: the event loop ended with an error: {error}");
    }
}

/// Take everything the bridge published and push it into the components.
fn apply_pending(
    bridge: &Arc<Bridge>,
    state: &Arc<Mutex<UiState>>,
    window: &slint::Weak<MainWindow>,
    tray: &slint::Weak<TrayIcon>,
) {
    let messages = bridge.drain();
    if messages.is_empty() {
        return;
    }

    let mut changed = false;
    {
        let Ok(mut state) = state.lock() else {
            return;
        };
        for message in messages {
            match message {
                BridgeMessage::Attached {
                    protocol_version,
                    daemon_id,
                } => {
                    state.connection = ConnectionState::Connected {
                        protocol_version,
                        daemon_id,
                    };
                    changed = true;
                }
                BridgeMessage::Snapshot { snapshot } => {
                    // Only a newer document is applied. A late arrival must
                    // not overwrite a newer view with an older one.
                    if snapshot.is_newer_than(state.generation) {
                        state.generation = snapshot.generation;
                        state.snapshot = Some(snapshot);
                        changed = true;
                    }
                }
                BridgeMessage::Failed { reason } => {
                    state.connection = ConnectionState::Disconnected { reason };
                    changed = true;
                }
                BridgeMessage::Incompatible { reason } => {
                    state.connection = ConnectionState::Incompatible { reason };
                    changed = true;
                }
                BridgeMessage::ShuttingDown => {
                    state.connection = ConnectionState::Disconnected {
                        reason: "the client daemon is shutting down".to_owned(),
                    };
                    changed = true;
                }
            }
        }
        if !changed {
            return;
        }
        render(&mut state, window, tray);
    }
}

/// Push the current model into the components.
fn render(state: &mut UiState, window: &slint::Weak<MainWindow>, tray: &slint::Weak<TrayIcon>) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let Some(tray) = tray.upgrade() else {
        return;
    };

    let model: Option<GuiViewModel> = state.snapshot.as_ref().map(|snapshot| {
        model::project(snapshot, &state.selection, &state.connection, now_unix_ms())
    });

    let Some(model) = model else {
        window.set_status_text(SharedString::from(connection_text(&state.connection)));
        return;
    };

    let hosts: Vec<HostRow> = model
        .systems
        .iter()
        .map(|card| HostRow {
            name: card.name.clone().into(),
            endpoint: card.endpoint.clone().into(),
            status: card.status.clone().into(),
            headline: card.headline.clone().into(),
            online: card.is_online(),
            selected: model.selected_id.as_deref() == Some(card.id.as_str()),
        })
        .collect();
    window.set_hosts(slint::ModelRc::from(hosts.as_slice()));

    let metrics: Vec<ValueRow> = model
        .selected
        .as_ref()
        .map(|card| {
            card.metrics
                .iter()
                .map(|row| ValueRow {
                    label: row.label.clone().into(),
                    value: row.value.clone().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    window.set_metrics(slint::ModelRc::from(metrics.as_slice()));

    // The drive and interface rows come from the document, not from the
    // card: only the selected host has them.
    let details: Vec<ValueRow> = state
        .snapshot
        .as_ref()
        .zip(model.selected_id.as_deref())
        .map(|(snapshot, selected)| {
            let (drives, interfaces) = model::detail_rows(snapshot, selected);
            let mut rows: Vec<ValueRow> = drives
                .into_iter()
                .map(|drive| ValueRow {
                    label: drive.mount.clone().into(),
                    value: drive.usage.clone().into(),
                })
                .collect();
            rows.extend(interfaces.into_iter().map(|interface| ValueRow {
                label: interface.name.clone().into(),
                value: interface.throughput.clone().into(),
            }));
            rows
        })
        .unwrap_or_default();
    window.set_details(slint::ModelRc::from(details.as_slice()));

    let jobs: Vec<JobRow> = model
        .cron
        .as_ref()
        .map(|cron| {
            cron.jobs
                .iter()
                .map(|job| JobRow {
                    name: job.name.clone().into(),
                    state: job.state.clone().into(),
                    outcome: job.outcome.clone().into(),
                    selected: state
                        .selection
                        .cron_job
                        .as_ref()
                        .is_some_and(|(_, open)| open == &job.name),
                })
                .collect()
        })
        .unwrap_or_default();
    window.set_jobs(slint::ModelRc::from(jobs.as_slice()));

    let records = model
        .cron
        .as_ref()
        .map(|cron| cron.records.join("\n"))
        .unwrap_or_default();
    window.set_cron_records(records.into());

    let eggpool: Vec<ValueRow> = model
        .eggpool
        .as_ref()
        .filter(|_| state.eggpool_visible)
        .map(|eggpool| {
            eggpool
                .values
                .iter()
                .map(|(label, value)| ValueRow {
                    label: label.clone().into(),
                    value: value.clone().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    window.set_eggpool(slint::ModelRc::from(eggpool.as_slice()));

    window.set_fleet_summary(model.tray_summary.clone().into());
    window.set_status_text(SharedString::from(connection_text(&state.connection)));

    // The tray shows the fleet health, not the connection state: the tray is
    // what the operator glances at.
    tray.set_summary_text(model.tray_summary.clone().into());
    tray.set_status_text(SharedString::from(tray_status(&model)));

    // The icon carries the health too, so the fleet's state is readable
    // without opening the menu. It is only re-rendered when the health
    // actually changes, because every re-render replaces the platform icon.
    let health = if model.counts.offline > 0 {
        TrayHealth::Offline
    } else if model.counts.pending > 0 {
        TrayHealth::Pending
    } else {
        TrayHealth::Healthy
    };
    if state.tray_health != Some(health) {
        state.tray_health = Some(health);
        tray.set_tray_icon(tray_icon(health));
    }
}

/// The connection state, in one line.
fn connection_text(connection: &ConnectionState) -> String {
    match connection {
        ConnectionState::Connecting => "connecting to the client daemon…".to_owned(),
        ConnectionState::Connected { daemon_id, .. } => format!("connected to {daemon_id}"),
        ConnectionState::Disconnected { reason } => format!("disconnected: {reason}"),
        ConnectionState::Incompatible { reason } => format!("incompatible daemon: {reason}"),
    }
}

/// The tray's tooltip: the fleet's health, in one line.
fn tray_status(model: &GuiViewModel) -> String {
    if model.counts.total == 0 {
        return "Gregg — no systems configured".to_owned();
    }
    if model.counts.offline > 0 {
        return format!(
            "Gregg — {} offline, {} online",
            model.counts.offline, model.counts.online
        );
    }
    format!("Gregg — {} online", model.counts.online)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeMessage;

    /// A bridge whose messages the test chooses, and the state they are
    /// applied to.
    fn bridge_and_state() -> (
        Arc<Bridge>,
        std::sync::mpsc::Sender<BridgeMessage>,
        Arc<Mutex<UiState>>,
    ) {
        let (bridge, messages) = Bridge::with_publisher();
        let state = Arc::new(Mutex::new(UiState {
            connection: ConnectionState::Connecting,
            ..UiState::default()
        }));
        (Arc::new(bridge), messages, state)
    }

    /// Apply whatever the bridge published.
    ///
    /// The components are never constructed: a test has no desktop session,
    /// and the mapping is about the state, not about what is drawn.
    fn apply(bridge: &Arc<Bridge>, state: &Arc<Mutex<UiState>>) {
        let window: slint::Weak<MainWindow> = slint::Weak::default();
        let tray: slint::Weak<TrayIcon> = slint::Weak::default();
        apply_pending(bridge, state, &window, &tray);
    }

    /// A document with the given generation and number of hosts.
    fn document(generation: u64, hosts: usize) -> gregg::clientd::snapshot::FrontendSnapshot {
        let systems = (0..hosts)
            .map(|index| {
                gregg::clientd::snapshot::SystemSnapshotDto::placeholder(
                    index,
                    gregg::state::Reachability::Online,
                )
            })
            .collect();
        let mut snapshot = gregg::clientd::snapshot::FrontendSnapshot::empty(systems);
        snapshot.generation = generation;
        snapshot
    }

    #[test]
    fn an_attached_daemon_is_connected() {
        let (bridge, messages, state) = bridge_and_state();
        messages
            .send(BridgeMessage::Attached {
                protocol_version: 3,
                daemon_id: "abc".to_owned(),
            })
            .expect("queues the message");
        apply(&bridge, &state);
        assert_eq!(
            state.lock().expect("state lock").connection,
            ConnectionState::Connected {
                protocol_version: 3,
                daemon_id: "abc".to_owned(),
            }
        );
    }

    #[test]
    fn a_failed_attachment_is_disconnected_with_its_reason() {
        let (bridge, messages, state) = bridge_and_state();
        messages
            .send(BridgeMessage::Failed {
                reason: "the client daemon is not running".to_owned(),
            })
            .expect("queues the message");
        apply(&bridge, &state);
        assert_eq!(
            state.lock().expect("state lock").connection,
            ConnectionState::Disconnected {
                reason: "the client daemon is not running".to_owned(),
            }
        );
    }

    #[test]
    fn an_incompatible_daemon_is_not_a_plain_disconnect() {
        let (bridge, messages, state) = bridge_and_state();
        messages
            .send(BridgeMessage::Incompatible {
                reason: "the client daemon speaks local protocol 2".to_owned(),
            })
            .expect("queues the message");
        apply(&bridge, &state);
        let connection = state.lock().expect("state lock").connection.clone();
        assert_eq!(
            connection,
            ConnectionState::Incompatible {
                reason: "the client daemon speaks local protocol 2".to_owned(),
            }
        );
        assert!(
            !matches!(connection, ConnectionState::Disconnected { .. }),
            "a version mismatch must not be reported as a lost connection"
        );
    }

    #[test]
    fn a_shutting_down_daemon_is_disconnected_with_a_shutdown_reason() {
        let (bridge, messages, state) = bridge_and_state();
        messages
            .send(BridgeMessage::ShuttingDown)
            .expect("queues the message");
        apply(&bridge, &state);
        let ConnectionState::Disconnected { reason } =
            state.lock().expect("state lock").connection.clone()
        else {
            panic!("a shutting-down daemon is a disconnect");
        };
        assert!(
            reason.contains("shutting down"),
            "the reason must say what happened: {reason}"
        );
    }

    #[test]
    fn only_a_newer_document_is_applied() {
        let (bridge, messages, state) = bridge_and_state();

        messages
            .send(BridgeMessage::Snapshot {
                snapshot: Box::new(document(5, 2)),
            })
            .expect("queues the document");
        apply(&bridge, &state);
        {
            let state = state.lock().expect("state lock");
            assert_eq!(state.generation, 5);
            assert_eq!(
                state.snapshot.as_ref().expect("a document").systems.len(),
                2
            );
        }

        // The same generation again is dropped: it is not newer.
        messages
            .send(BridgeMessage::Snapshot {
                snapshot: Box::new(document(5, 1)),
            })
            .expect("queues the document");
        apply(&bridge, &state);
        {
            let state = state.lock().expect("state lock");
            assert_eq!(
                state.snapshot.as_ref().expect("a document").systems.len(),
                2,
                "an equal generation must not overwrite the view"
            );
        }

        // An older generation is dropped too.
        messages
            .send(BridgeMessage::Snapshot {
                snapshot: Box::new(document(4, 1)),
            })
            .expect("queues the document");
        apply(&bridge, &state);
        {
            let state = state.lock().expect("state lock");
            assert_eq!(
                state.snapshot.as_ref().expect("a document").systems.len(),
                2,
                "an older generation must not overwrite the view"
            );
        }

        // A newer one is applied.
        messages
            .send(BridgeMessage::Snapshot {
                snapshot: Box::new(document(6, 1)),
            })
            .expect("queues the document");
        apply(&bridge, &state);
        let state = state.lock().expect("state lock");
        assert_eq!(state.generation, 6);
        assert_eq!(
            state.snapshot.as_ref().expect("a document").systems.len(),
            1
        );
    }
}
