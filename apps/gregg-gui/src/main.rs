//! The optional Gregg desktop frontend.
//!
//! # What this is
//!
//! A second way to *look* at the fleet, never a second way to *poll* it. The
//! GUI attaches to the existing per-config client daemon over the same local
//! protocol the terminal client uses, and renders the documents that daemon
//! already publishes. A TUI window and a GUI panel therefore cost the fleet
//! exactly what one window costs.
//!
//! # The shape
//!
//! There is **one** surface: a tray pulldown. It opens from the native tray
//! menu, it is borderless and always on top, and it carries every pane the
//! old monitor window carried — the fleet summary, the host list, the
//! selected host's metrics, drives, interfaces, cron, records and
//! `EggPool` — plus the forms that add, remove and edit hosts.
//!
//! # How it is put together
//!
//! - [`model`] is a **pure projection**: one daemon document in, one view
//!   model out. Every truthfulness rule the GUI keeps — absent is absent,
//!   unpolled is not offline, selection is keyed by stable id — lives there,
//!   where it is testable without a desktop session.
//! - [`bridge`] owns the client-daemon attachment and the intents. It never
//!   touches a Slint component.
//! - [`hosts`] owns the configuration mutations, through the CLI's own
//!   library code, behind a seam a test can replace.
//! - [`placement`] owns where the panel opens and how far it may be moved.
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ComponentHandle, SharedString};

mod bridge;
mod hosts;
mod model;
mod placement;
mod platform;
mod state;

#[cfg(test)]
mod evidence;

use bridge::{Bridge, BridgeMessage, CronDesired, DaemonResolution, EggpoolDesired};
use hosts::{HostForm, HostMutations, HostTarget};
use model::{ConnectionState, GuiViewModel, Selection};

slint::include_modules!();

/// One measured column, as the fleet's table shows it.
///
/// A cell with no reading carries a negative fraction, so its bar is empty
/// rather than full: an empty bar says "not measured", and a full one would
/// say "measured as everything".
fn fleet_cell(cell: &model::FleetCell) -> FleetCell {
    FleetCell {
        pct_text: cell.pct_text.clone().into(),
        fraction: cell.fraction,
        extra: cell.extra.clone().into(),
    }
}

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

/// How many times the pulldown's placement is retried.
///
/// The work area is read from the pulldown's native window, and the native
/// window is created by the event loop. A handful of ticks is enough for the
/// loop to resume it, and the bound keeps a window that never appears from
/// turning into an endless retry.
const PLACEMENT_ATTEMPTS: u8 = 20;

/// The gap between two placement attempts.
const PLACEMENT_RETRY: Duration = Duration::from_millis(16);

/// What the panel is showing.
///
/// The Slint side has its own `PanelMode`; this is the Rust side's copy of
/// the same thing, and the two are kept in step by [`push_view`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ViewMode {
    /// The fleet: one row per host, with its bars.
    #[default]
    Fleet,
    /// The selected host's detail: its metrics, its drives, its interfaces,
    /// its scheduler and the `EggPool` plane.
    Detail,
    /// The form that adds a host.
    AddHost,
    /// The form that edits one host.
    EditHost,
}

impl ViewMode {
    /// Whether this view is one of the two the operator looks at.
    fn is_panel(&self) -> bool {
        matches!(self, Self::Fleet | Self::Detail)
    }

    /// The other view: the fleet, or the selected host's detail.
    ///
    /// A form is not one of the two, and it does not toggle: the form is
    /// left alone until it is submitted or abandoned.
    fn toggled(&self) -> Self {
        match self {
            Self::Fleet => Self::Detail,
            Self::Detail => Self::Fleet,
            other => *other,
        }
    }
}

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
    /// Whether the panel is on screen.
    panel_visible: bool,
    /// Where the operator last put the panel, in logical pixels.
    panel_position: Option<(f32, f32)>,
    /// How big the operator last made the panel, in logical pixels.
    panel_size: Option<(f32, f32)>,
    /// What the panel is showing.
    view_mode: ViewMode,
    /// The host-management form's state.
    form: HostForm,
    /// What the last action said.
    panel_message: String,
    /// Whether the last action failed.
    panel_message_is_error: bool,
}

/// The parsed command line.
struct Options {
    /// The `gregg` client executable to launch, if the operator named one.
    gregg: Option<PathBuf>,
    /// The configuration file.
    config: PathBuf,
    /// Whether the panel opens at startup instead of waiting for the tray.
    panel: bool,
}

fn parse_options() -> Result<Option<Options>, String> {
    let mut gregg: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut panel = false;
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
            "--panel" => {
                panel = true;
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
        panel,
    }))
}

/// The usage text.
fn usage() -> String {
    "gregg-gui — the optional Gregg desktop frontend\n\
     \n\
     Usage: gregg-gui [--config <path>] [--gregg <path>] [--panel]\n\
     \n\
       --config <path>  the configuration file to read\n\
       --gregg <path>   the `gregg` client executable to launch as the client daemon\n\
       --panel          open the panel at startup instead of waiting for the tray\n\
       --help           print this message\n\
     \n\
     The GUI is a frontend: it attaches to the per-config client daemon and\n\
     renders what that daemon already polls. It never polls the fleet itself.\n\
     \n\
     The GUI is a tray pulldown: clicking the tray's icon opens the pulldown,\n\
     and the pulldown carries the fleet, the metrics and the forms that add,\n\
     remove and edit hosts. Click the tray's icon again, or press Esc, to\n\
     dismiss it."
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

    // The desktop is asked about its system tray before anything else: the
    // answer decides what closing the panel means, and it is reported on one
    // machine-readable line.
    let tray_support = platform::tray_support();
    println!("{}", platform::tray_status_line(&tray_support));

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

    let panel = Pulldown::new().expect("the panel component is constructible");
    let tray = TrayIcon::new().expect("the tray component is constructible");
    tray.set_tray_icon(tray_icon(TrayHealth::Pending));
    // macOS carries no native menu: a menu attached to the status item
    // swallows the click that opens the pulldown.
    tray.set_native_menu(platform::native_tray_menu());
    panel.set_transparent_window(platform::transparent_window());
    panel.set_pointer_at_top(matches!(
        placement::Anchor::for_platform(),
        placement::Anchor::TopRight
    ));
    panel.set_tray_warning(platform::tray_warning_text(&tray_support).into());

    // Where the operator left the pulldown is remembered across restarts: it
    // is read from a small file beside the configuration.
    let geometry_path = state::path_for(&options.config);
    let geometry = state::load(&geometry_path);

    let state: Arc<Mutex<UiState>> = Arc::new(Mutex::new(UiState {
        connection: ConnectionState::Connecting,
        panel_position: geometry.position,
        panel_size: geometry.size,
        ..UiState::default()
    }));

    // The configuration mutations go through the CLI's own library code, so
    // the panel inherits its endpoint grammar, its lock and its atomic
    // write. The seam is behind a trait so a test can replace it.
    let manager: Arc<dyn HostMutations + Send + Sync> =
        Arc::new(hosts::ConfigMutations::new(options.config.clone()));

    // The bridge needs a callback it can call from its own thread. The
    // callback only *schedules* work on the UI thread; it never touches a
    // component itself, because Slint components are not `Send`.
    // The bridge is created with the notification callback, and the callback
    // needs the bridge, so the handle is published through a cell the
    // callback can read once the bridge exists.
    let bridge_slot: Arc<OnceLock<Arc<Bridge>>> = Arc::new(OnceLock::new());
    let notify_state = Arc::clone(&state);
    let notify_panel = panel.as_weak();
    let notify_tray = tray.as_weak();
    let notify_slot = Arc::clone(&bridge_slot);
    let notify = Arc::new(move || {
        let state = Arc::clone(&notify_state);
        let panel = notify_panel.clone();
        let tray = notify_tray.clone();
        let slot = Arc::clone(&notify_slot);
        // `invoke_from_event_loop` fails when the event loop is not running
        // yet. The message stays queued in the bridge, and the first drain
        // after the loop starts picks it up, so nothing is lost.
        let _ = slint::invoke_from_event_loop(move || {
            let Some(bridge) = slot.get() else {
                return;
            };
            apply_pending(bridge, &state, &panel, &tray);
        });
    });

    let bridge = Arc::new(Bridge::spawn(options.config.clone(), daemon, notify));
    let _ = bridge_slot.set(Arc::clone(&bridge));

    // The callbacks run on the UI thread, so the state is taken behind its
    // mutex rather than through a `RefCell` borrow that could conflict with
    // the scheduled update.
    let callback_bridge = Arc::clone(&bridge);
    panel.on_refresh_requested(move || {
        callback_bridge.request_reload();
    });

    panel.on_quit_requested(move || {
        let _ = slint::quit_event_loop();
    });

    // Closing the panel hides it, and the tray brings it back. That is only
    // true while the desktop has a tray: without one, hiding the panel
    // would leave the operator with no way back to the GUI and no way to
    // quit it, so the GUI ends instead.
    //
    // The desktop is asked again *here*, and not at startup, because a tray
    // host can start after the desktop session is up — Slint retries the
    // registration whenever the icon is set again — and a GUI that decided
    // at startup would keep quitting the panel on a desktop that has a
    // tray by now.
    let callback_state = Arc::clone(&state);
    panel.window().on_close_requested(move || {
        let support = platform::tray_support();
        match platform::close_action(&support) {
            platform::CloseAction::HideToTray => {
                if let Ok(mut state) = callback_state.lock() {
                    state.panel_visible = false;
                }
                slint::CloseRequestResponse::HideWindow
            }
            platform::CloseAction::Quit => {
                eprintln!(
                    "gregg-gui: close with tray=unavailable -> quitting ({})",
                    platform::tray_reason(&support)
                );
                let _ = slint::quit_event_loop();
                slint::CloseRequestResponse::KeepWindowShown
            }
        }
    });

    let callback_state = Arc::clone(&state);
    let callback_bridge = Arc::clone(&bridge);
    panel.on_host_selected(move |id| {
        let mut state = callback_state.lock().expect("state lock");
        state.selection.selected_id = Some(id.to_string());
        // Changing host closes the open cron job: the retained records belong
        // to a job of the *previous* host, and the intent for it must be
        // released so the daemon stops transmitting them.
        state.selection.cron_job = None;
        callback_bridge.set_cron_intent(None);
    });

    // The two views. The fleet is a list of hosts and their bars; the detail
    // is the selected host's own numbers. The control toggles between them,
    // and it is the *selected* host whose detail it is.
    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_view_toggled(move || {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        state.view_mode = state.view_mode.toggled();
        push_view(&mut state, &callback_panel);
    });

    let callback_state = Arc::clone(&state);
    let callback_bridge = Arc::clone(&bridge);
    panel.on_job_selected(move |job| {
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
    panel.on_eggpool_toggled(move || {
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

    let about_panel = panel.as_weak();
    panel.on_about_requested(move || {
        if let Some(panel) = about_panel.upgrade() {
            panel.set_about_text(SLINT_ATTRIBUTION.into());
        }
    });

    // ------------------------------------------------------- host management
    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_add_host_requested(move || {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        state.view_mode = ViewMode::AddHost;
        state.form = HostForm::adding();
        state.panel_message.clear();
        state.panel_message_is_error = false;
        push_view(&mut state, &callback_panel);
    });

    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_edit_host_requested(move |id| {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        let Some(target) = host_target(&state, id.as_str()) else {
            state.panel_message = format!("{id} is no longer configured");
            state.panel_message_is_error = true;
            push_view(&mut state, &callback_panel);
            return;
        };
        state.view_mode = ViewMode::EditHost;
        state.form = HostForm::editing(target);
        state.panel_message.clear();
        state.panel_message_is_error = false;
        push_view(&mut state, &callback_panel);
    });

    let callback_state = Arc::clone(&state);
    let callback_manager = Arc::clone(&manager);
    let callback_bridge = Arc::clone(&bridge);
    let callback_panel = panel.as_weak();
    panel.on_remove_host_requested(move |id| {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        let Some(target) = host_target(&state, id.as_str()) else {
            state.panel_message = format!("{id} is no longer configured");
            state.panel_message_is_error = true;
            push_view(&mut state, &callback_panel);
            return;
        };
        apply_mutation(
            &mut state,
            &callback_manager,
            &callback_bridge,
            &callback_panel,
            &hosts::HostMutation::Remove {
                endpoint: target.endpoint,
            },
        );
    });

    let callback_state = Arc::clone(&state);
    let callback_manager = Arc::clone(&manager);
    let callback_bridge = Arc::clone(&bridge);
    let callback_panel = panel.as_weak();
    panel.on_form_submitted(move || {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        let Some(mutation) = state.form.to_mutation() else {
            return;
        };
        apply_mutation(
            &mut state,
            &callback_manager,
            &callback_bridge,
            &callback_panel,
            &mutation,
        );
    });

    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_form_cancelled(move || {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        state.view_mode = ViewMode::Fleet;
        state.form = HostForm::default();
        state.panel_message.clear();
        state.panel_message_is_error = false;
        push_view(&mut state, &callback_panel);
    });

    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_endpoint_edited(move |text| {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        state.form.endpoint = text.to_string();
        push_view(&mut state, &callback_panel);
    });

    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_name_edited(move |text| {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        state.form.name = text.to_string();
        push_view(&mut state, &callback_panel);
    });

    // ------------------------------------------------------ the panel itself
    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_close_requested(move || {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        state.panel_visible = false;
        // A panel that is dismissed leaves the fleet view behind it.
        state.view_mode = ViewMode::Fleet;
        state.form = HostForm::default();
        if let Some(panel) = callback_panel.upgrade() {
            let _ = panel.hide();
        }
    });

    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_dragged(move |dx, dy| {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        let Some(panel) = callback_panel.upgrade() else {
            return;
        };
        let current = panel.window().position();
        let scale = panel.window().scale_factor().max(0.01);
        // The position is read back in physical pixels and kept in logical
        // ones, so the panel does not drift on a scaled display.
        let x = current.x as f32 / scale + dx;
        let y = current.y as f32 / scale + dy;
        state.panel_position = Some((x, y));
        panel
            .window()
            .set_position(slint::LogicalPosition::new(x, y));
    });

    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    panel.on_resized(move |dx, dy| {
        let Ok(mut state) = callback_state.lock() else {
            return;
        };
        let Some(panel) = callback_panel.upgrade() else {
            return;
        };
        let current = panel.window().size();
        let scale = panel.window().scale_factor().max(0.01);
        let width = (current.width as f32 / scale + dx).max(placement::MIN_PANEL_SIZE.0);
        let height = (current.height as f32 / scale + dy).max(placement::MIN_PANEL_SIZE.1);
        state.panel_size = Some((width, height));
        panel
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    });

    // The placement is remembered across restarts: the operator's position
    // and size are written to a small file beside the configuration, and read
    // back at startup.
    let callback_state = Arc::clone(&state);
    let callback_path = geometry_path.clone();
    panel.on_geometry_changed(move || {
        let Ok(state) = callback_state.lock() else {
            return;
        };
        let geometry = state::PanelGeometry {
            position: state.panel_position,
            size: state.panel_size,
        };
        if geometry.is_empty() {
            return;
        }
        if let Err(error) = state::store(&callback_path, geometry) {
            eprintln!("gregg-gui: cannot remember the pulldown's placement: {error}");
        }
    });

    // ------------------------------------------------------- the tray's click
    // The tray's **click** opens the pulldown, and the pulldown carries the
    // content: the fleet, the metrics, the cron, the records, the `EggPool`
    // plane and the forms. The native menu is the context menu where there is
    // one, and it carries `Quit` and nothing else.
    let callback_state = Arc::clone(&state);
    let callback_panel = panel.as_weak();
    tray.on_tray_clicked(move || {
        let Some(panel) = callback_panel.upgrade() else {
            return;
        };
        toggle_pulldown(&panel, &callback_state);
    });

    tray.on_quit_requested(move || {
        let _ = slint::quit_event_loop();
    });

    // A desktop with no tray service still needs a usable surface, and a
    // process the operator can always quit. The panel is shown either way:
    // with a tray it is opened from the tray, and without one it is the only
    // control surface there is.
    //
    // `--panel` opens it whatever the desktop has: an operator who launches
    // the GUI that way wants to see it, and the native exercises need a
    // panel on screen to prove the windowing works on that platform.
    //
    // The first open waits for the event loop. The pulldown's placement is
    // read from the display its *native* window is on, and the native window
    // does not exist until the event loop has resumed it — so an open before
    // the loop starts is a panel the window manager places by guesswork, and
    // it lands under the menu bar.
    if options.panel || platform::tray_support() != platform::TraySupport::Available {
        let weak = panel.as_weak();
        let state = Arc::clone(&state);
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            show_pulldown(&panel, &state);
        });
    }

    // The first drain happens after the event loop is running, so a message
    // published before it started is not lost.
    //
    // `run_event_loop_until_quit` rather than `run_event_loop`: closing the
    // panel must **hide** it and leave the tray and the observation running,
    // and the daemon must keep polling. Slint's own close handling hides the
    // window on a close request, and an event loop that quits when its last
    // window closes would turn that into "the GUI exited". Quit is the only
    // thing that ends the GUI.
    if let Err(error) = slint::run_event_loop_until_quit() {
        eprintln!("gregg-gui: the event loop ended with an error: {error}");
    }
}

/// Bring the pulldown up, and place it on the display.
///
/// The application is activated *before* the panel is shown. On macOS a
/// background application cannot bring its own window to the front, and a
/// tray application is a background application — so without this, the
/// pulldown does not come to the front when the tray's click opens it.
///
/// The tray state is re-probed when the panel comes back: a
/// `StatusNotifierItem` host can start after the GUI did, and the panel must
/// not keep claiming there is no tray when there is one by now. The probe
/// runs off the UI thread, so a slow session bus never freezes the panel.
fn show_pulldown(panel: &Pulldown, state: &Arc<Mutex<UiState>>) {
    platform::activate();
    if let Ok(mut state) = state.lock() {
        state.panel_visible = true;
    }
    let _ = panel.show();
    place_pulldown(panel, state, PLACEMENT_ATTEMPTS);

    let weak = panel.as_weak();
    platform::tray_support_async(move |support| {
        let text = platform::tray_warning_text(&support);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(panel) = weak.upgrade() {
                panel.set_tray_warning(text.into());
            }
        });
    });
}

/// Place the pulldown, and give it the size its content calls for.
///
/// The work area is read from the pulldown's **native** window, and the
/// native window appears on the event loop's own terms: it is created when
/// the event loop resumes the window, which is after `show()`. So the
/// placement is retried for a bounded number of ticks, and a pulldown whose
/// native window never appears is left to the window manager — which places
/// it, if not where the platform's convention says.
fn place_pulldown(panel: &Pulldown, state: &Arc<Mutex<UiState>>, attempts_left: u8) {
    let Ok(mut guard) = state.lock() else {
        return;
    };
    let area = placement::work_area(panel.window());
    let content = (
        placement::metrics::DEFAULT_WIDTH,
        placement::content_height(&content_for(&guard, panel)),
    );
    let size = placement::panel_size(guard.panel_size, content, area.as_ref());
    panel.window().set_size(size);
    if let Some(position) = placement::open_position(
        guard.panel_position,
        (size.width, size.height),
        area.as_ref(),
    ) {
        guard.panel_position = Some((position.x, position.y));
        panel.window().set_position(position);
        if let Some(area) = area.as_ref() {
            // The placement, in one machine-readable line.
            //
            // It says where the pulldown was put and where it may be put, so
            // that a native exercise can check the *position* and not only
            // that a window appeared.
            println!(
                "gregg-gui: pulldown=placed x={} y={} width={} height={} area={},{},{}x{}",
                position.x,
                position.y,
                size.width,
                size.height,
                area.left,
                area.top,
                area.width,
                area.height
            );
        }
    }
    if area.is_none() && attempts_left > 0 {
        let weak = panel.as_weak();
        let state = Arc::clone(state);
        // The next tick is after the event loop has resumed the window.
        slint::Timer::single_shot(PLACEMENT_RETRY, move || {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            place_pulldown(&panel, &state, attempts_left - 1);
        });
    }
}

/// Show the pulldown, or hide it if it is already up.
fn toggle_pulldown(panel: &Pulldown, state: &Arc<Mutex<UiState>>) {
    let Ok(mut guard) = state.lock() else {
        return;
    };
    if guard.panel_visible {
        guard.panel_visible = false;
        drop(guard);
        let _ = panel.hide();
        return;
    }
    drop(guard);
    show_pulldown(panel, state);
}

/// What the pulldown is showing, as the height calculation sees it.
///
/// The lines the pulldown shows are read from the pulldown itself: they are
/// what is on screen, and the height has to answer for what is on screen.
fn content_for(state: &UiState, panel: &Pulldown) -> placement::Content {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return placement::Content::default();
    };
    let model = model::project(snapshot, &state.selection, &state.connection, now_unix_ms());
    let metrics = model.selected.as_ref().map_or(0, |card| card.metrics.len());
    let (drives, interfaces) = model
        .selected_id
        .as_deref()
        .map(|selected| model::detail_rows(snapshot, selected))
        .unwrap_or_default();
    let jobs = model.cron.as_ref().map_or(0, |cron| cron.jobs.len());
    let record_lines = model.cron.as_ref().map_or(0, |cron| {
        cron.records
            .iter()
            .map(|record| record.lines().count().max(1))
            .sum::<usize>()
    });
    let eggpool = model
        .eggpool
        .as_ref()
        .filter(|_| state.eggpool_visible)
        .map_or(0, |eggpool| eggpool.values.len());
    placement::Content {
        hosts: model.systems.len(),
        selected: model.selected_id.is_some(),
        metrics,
        details: drives.len() + interfaces.len(),
        jobs,
        eggpool,
        record_lines,
        about: !panel.get_about_text().is_empty(),
        status: !panel.get_status_text().is_empty(),
        message: !panel.get_panel_message().is_empty(),
        warning: !panel.get_tray_warning().is_empty(),
        form: !state.view_mode.is_panel(),
        detail: state.view_mode == ViewMode::Detail,
    }
}

/// The host a panel action names, as the form's target.
///
/// The action names the host by its stable id, so a fleet that reordered
/// between the moment the list was drawn and the moment the button was
/// pressed still acts on the host the operator meant.
fn host_target(state: &UiState, id: &str) -> Option<HostTarget> {
    let snapshot = state.snapshot.as_ref()?;
    let system = snapshot.systems.iter().find(|system| system.id == id)?;
    Some(HostTarget {
        id: system.id.clone(),
        endpoint: system.endpoint.display_address(),
        name: system.configured_name.clone(),
    })
}

/// Apply one configuration mutation, and say what happened.
///
/// A refusal leaves the configuration byte-identical and keeps the operator
/// in the form, so the mistake can be corrected. A success returns to the
/// fleet view and asks the daemon to re-read the configuration.
fn apply_mutation(
    state: &mut UiState,
    manager: &Arc<dyn HostMutations + Send + Sync>,
    bridge: &Arc<Bridge>,
    panel: &slint::Weak<Pulldown>,
    mutation: &hosts::HostMutation,
) {
    match manager.apply(mutation) {
        Ok(message) => {
            // A seam that says nothing still says what was attempted.
            let message = if message.trim().is_empty() {
                mutation.describe()
            } else {
                message
            };
            state.panel_message = message;
            state.panel_message_is_error = false;
            state.view_mode = ViewMode::Fleet;
            state.form = HostForm::default();
            // The daemon re-reads the configuration at its own reload
            // boundary and republishes. The GUI never asks it to apply a
            // mutation: it has no such message, and it does not need one.
            bridge.request_reload();
        }
        Err(reason) => {
            state.panel_message = reason;
            state.panel_message_is_error = true;
        }
    }
    push_view(state, panel);
}

/// What the form says, in the operator's words.
///
/// It is separate from the panel so that it can be asserted without a
/// desktop session: a form that says the wrong thing misleads the operator,
/// and that must not need a screen to prove.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FormView {
    /// The form's heading.
    title: String,
    /// What the form says about what it is about to do.
    note: String,
    /// The label on the confirming button.
    apply_text: String,
    /// Whether the confirming button does anything.
    can_apply: bool,
}

/// The form's presentation, for the current state.
fn form_view(state: &UiState) -> FormView {
    match state.view_mode {
        ViewMode::Fleet | ViewMode::Detail => FormView {
            title: String::new(),
            note: String::new(),
            apply_text: String::new(),
            can_apply: false,
        },
        ViewMode::AddHost => FormView {
            title: "Add a host".to_owned(),
            note: "The endpoint is added to the configuration exactly as `gregg add` would add \
                   it: an explicit port, no HTTPS, and the whole configuration is validated                    before it is written."
                .to_owned(),
            apply_text: "Add".to_owned(),
            can_apply: !state.form.endpoint.trim().is_empty(),
        },
        ViewMode::EditHost => {
            let changed = state.form.endpoint_changed();
            FormView {
                title: "Edit this host".to_owned(),
                note: if changed {
                    "The endpoint has moved. Applying this replaces the host: the endpoint is \
                     part of its identity, so the host gets a new identity and its selection \
                     and history do not follow it."
                        .to_owned()
                } else {
                    "The name is changed in place: the endpoint has not moved, so the host \
                     keeps its identity."
                        .to_owned()
                },
                apply_text: if changed {
                    "Replace endpoint".to_owned()
                } else {
                    "Save name".to_owned()
                },
                can_apply: state.form.to_mutation().is_some(),
            }
        }
    }
}

/// Push the form's state into the panel.
///
/// The form's fields are pushed **before** the mode changes, so a text field
/// that is created by the mode change reads the value the operator is meant
/// to see rather than an empty one.
///
/// The pulldown's size follows the view: the fleet is a list of hosts and
/// their bars, and the detail is the selected host's own numbers, and the
/// two are not the same height.
fn push_view(state: &mut UiState, panel: &slint::Weak<Pulldown>) {
    let Some(panel) = panel.upgrade() else {
        return;
    };

    panel.set_panel_message(state.panel_message.clone().into());
    panel.set_panel_message_is_error(state.panel_message_is_error);

    match state.view_mode {
        ViewMode::Fleet => {
            panel.set_mode(PanelMode::Fleet);
            panel.set_form_endpoint(SharedString::new());
            panel.set_form_name(SharedString::new());
        }
        ViewMode::Detail => {
            panel.set_mode(PanelMode::Detail);
            panel.set_form_endpoint(SharedString::new());
            panel.set_form_name(SharedString::new());
        }
        ViewMode::AddHost | ViewMode::EditHost => {
            let view = form_view(state);
            panel.set_form_title(view.title.into());
            panel.set_form_note(view.note.into());
            panel.set_form_apply_text(view.apply_text.into());
            panel.set_form_can_apply(view.can_apply);
            panel.set_form_endpoint(state.form.endpoint.clone().into());
            panel.set_form_name(state.form.name.clone().into());
            panel.set_mode(if state.view_mode == ViewMode::AddHost {
                PanelMode::AddHost
            } else {
                PanelMode::EditHost
            });
        }
    }

    resize_panel(state, &panel);
}

/// Take everything the bridge published and push it into the components.
fn apply_pending(
    bridge: &Arc<Bridge>,
    state: &Arc<Mutex<UiState>>,
    panel: &slint::Weak<Pulldown>,
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
                    // The panel shows the reason, but a GUI is often launched
                    // in the background where nobody is looking at the panel.
                    // The reason also goes to stderr, so the launcher's own
                    // output says why there is no daemon. The launcher starts
                    // the daemon with every stream null, so this line is the
                    // only place the reason can surface.
                    eprintln!("gregg-gui: cannot attach to the client daemon: {reason}");
                    state.connection = ConnectionState::Disconnected { reason };
                    changed = true;
                }
                BridgeMessage::Incompatible { reason } => {
                    eprintln!("gregg-gui: the client daemon is incompatible: {reason}");
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
        render(&mut state, panel, tray);
    }
}

/// Push the current model into the components.
fn render(state: &mut UiState, panel: &slint::Weak<Pulldown>, tray: &slint::Weak<TrayIcon>) {
    let Some(panel) = panel.upgrade() else {
        return;
    };
    let Some(tray) = tray.upgrade() else {
        return;
    };

    let model: Option<GuiViewModel> = state.snapshot.as_ref().map(|snapshot| {
        model::project(snapshot, &state.selection, &state.connection, now_unix_ms())
    });

    let Some(model) = model else {
        panel.set_status_text(SharedString::from(connection_text(&state.connection)));
        return;
    };

    // The fleet's rows: one line per host, with a bar for each of the five
    // used shares. The strip is the model's own, so the compact row and the
    // TUI's condensed row cannot disagree about a number.
    let hosts: Vec<FleetRow> = model
        .systems
        .iter()
        .map(|card| {
            let strip = &card.strip;
            FleetRow {
                id: strip.id.clone().into(),
                name: strip.name.clone().into(),
                io_token: strip.io_token.clone().into(),
                load: strip.load.clone().into(),
                status: strip.status.clone().into(),
                online: strip.online,
                selected: model.selected_id.as_deref() == Some(card.id.as_str()),
                cpu: fleet_cell(&strip.cpu),
                memory: fleet_cell(&strip.memory),
                swap: fleet_cell(&strip.swap),
                disk: fleet_cell(&strip.disk),
                network: fleet_cell(&strip.network),
            }
        })
        .collect();
    let hosts = slint::ModelRc::from(hosts.as_slice());
    panel.set_hosts(hosts.clone());

    // The heading of the swap-or-commit column is the fleet's, not one host's:
    // a fleet may hold both kinds of host.
    panel.set_swap_heading(model::swap_heading(&model.systems).into());

    let metrics: Vec<MetricRow> = model
        .selected
        .as_ref()
        .map(|card| {
            card.metrics
                .iter()
                .map(|row| MetricRow {
                    label: row.label.clone().into(),
                    pct_text: row.pct_text.clone().into(),
                    // A row with no reading leaves its bar empty, and an empty
                    // bar is not a zero: the share is negative, not zero.
                    fraction: row.pct.map_or(-1.0, |pct| pct / 100.0),
                    value: row.value.clone().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    panel.set_metrics(slint::ModelRc::from(metrics.as_slice()));

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
    panel.set_details(slint::ModelRc::from(details.as_slice()));

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
    panel.set_jobs(slint::ModelRc::from(jobs.as_slice()));

    let records = model
        .cron
        .as_ref()
        .map(|cron| cron.records.join("\n"))
        .unwrap_or_default();
    panel.set_cron_records(records.into());

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
    panel.set_eggpool(slint::ModelRc::from(eggpool.as_slice()));

    panel.set_fleet_summary(model.tray_summary.clone().into());
    panel.set_status_text(SharedString::from(connection_text(&state.connection)));

    // The tray shows the fleet health, not the connection state: the tray is
    // what the operator glances at. The fleet itself is in the pulldown.
    tray.set_status_text(SharedString::from(tray_status(&model)));

    // The selected host's own actions are in the pulldown, aimed at the
    // selection and not at a row's place in the list.
    let (selected_id, selected_label) = match model.selected_id.as_deref() {
        Some(selected) => {
            let label = model
                .systems
                .iter()
                .find(|card| card.id == selected)
                .map(|card| format!("{}  {}", card.name, card.endpoint))
                .unwrap_or_default();
            (selected.to_owned(), label)
        }
        None => (String::new(), String::new()),
    };
    panel.set_selected_host_id(selected_id.clone().into());
    panel.set_selected_host_label(selected_label.into());

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

    // The pulldown's size follows what it carries. The host list's height
    // and the form's height are the two parts of it that are the content's,
    // and the rest is computed from them.
    let content = content_for(state, &panel);
    panel.set_host_list_height(placement::host_list_height(content.hosts));
    panel.set_form_height(placement::metrics::FORM_HEIGHT);
    resize_panel(state, &panel);
}

/// Give the pulldown the size its content calls for.
///
/// The pulldown is as tall as what it shows, and it is as wide as the fleet
/// table needs. The operator's own size wins, though: a placement they chose
/// is not something a new host may overrule.
fn resize_panel(state: &mut UiState, panel: &Pulldown) {
    if !state.panel_visible {
        return;
    }
    let content = content_for(state, panel);
    let area = placement::work_area(panel.window());
    let size = placement::panel_size(
        state.panel_size,
        (
            placement::metrics::DEFAULT_WIDTH,
            placement::content_height(&content),
        ),
        area.as_ref(),
    );
    panel.window().set_size(size);
}

/// The connection state, in one line.
///
/// A healthy attachment says nothing: there is nothing to report, and a line
/// that reports nothing is chrome. The daemon's identity is not reported
/// either — it is a hash that means nothing to an operator looking at a
/// pulldown.
fn connection_text(connection: &ConnectionState) -> String {
    match connection {
        ConnectionState::Connecting => "connecting to the client daemon…".to_owned(),
        ConnectionState::Connected { .. } => String::new(),
        ConnectionState::Disconnected { reason } => format!("disconnected: {reason}"),
        ConnectionState::Incompatible { reason } => format!("incompatible daemon: {reason}"),
    }
}

/// The tray's tooltip: the fleet's health, in one line.
///
/// It carries the summary — the counts *and* the names of the hosts the
/// summary can fit — so that the tray alone tells the operator what the
/// fleet is doing, and the pulldown is opened for the detail.
fn tray_status(model: &GuiViewModel) -> String {
    if model.counts.total == 0 {
        return "Gregg — no systems configured".to_owned();
    }
    format!("Gregg — {}", model.tray_summary)
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
        let panel: slint::Weak<Pulldown> = slint::Weak::default();
        let tray: slint::Weak<TrayIcon> = slint::Weak::default();
        apply_pending(bridge, state, &panel, &tray);
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

    /// The `.slint` file, so that a test can say what the pulldown carries.
    const SLINT: &str = include_str!("ui/main.slint");

    #[test]
    fn the_tray_has_no_launcher_entry() {
        assert!(
            !SLINT.contains("Open Gregg"),
            "the launcher entry is gone: the tray's click opens the pulldown"
        );
        assert!(
            SLINT.contains("root.tray-clicked()"),
            "the tray's click reaches the GUI"
        );
    }

    #[test]
    fn the_pulldown_drops_the_application_chrome() {
        for heading in ["METRICS", "CRON", "EGGPOOL"] {
            assert!(
                !SLINT.contains(heading),
                "the {heading} heading is chrome a dropdown does not have"
            );
        }
        assert!(
            !SLINT.contains("text: \"Close\""),
            "the Close button that overhung the header is gone"
        );
    }

    #[test]
    fn the_hosts_actions_are_not_on_every_row() {
        // The actions are the pulldown's own affordances, aimed at the
        // selection: a button on every row crowds the row and blurs into the
        // selection tint.
        assert_eq!(
            SLINT.matches("root.edit-host-requested(").count(),
            1,
            "one place asks to edit a host"
        );
        assert_eq!(
            SLINT.matches("root.remove-host-requested(").count(),
            1,
            "one place asks to remove a host"
        );
    }

    #[test]
    fn a_healthy_attachment_says_nothing() {
        assert_eq!(
            connection_text(&ConnectionState::Connected {
                protocol_version: 3,
                daemon_id: "abc".to_owned(),
            }),
            "",
            "a line that says nothing is chrome"
        );
    }

    #[test]
    fn the_daemons_identity_is_not_reported() {
        let text = connection_text(&ConnectionState::Connected {
            protocol_version: 3,
            daemon_id: "3f2a9c".to_owned(),
        });
        assert!(
            !text.contains("3f2a9c"),
            "the daemon's identity means nothing to an operator"
        );
    }

    #[test]
    fn the_native_menu_is_detached_only_where_it_would_swallow_the_click() {
        // On macOS a menu attached to the status item is popped by the very
        // click that is supposed to open the pulldown, so it is detached
        // there. Windows and Linux keep theirs, where a right-click reaches
        // it and a left-click does not.
        #[cfg(target_os = "macos")]
        assert!(!platform::native_tray_menu());
        #[cfg(not(target_os = "macos"))]
        assert!(platform::native_tray_menu());
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

    #[test]
    fn the_panel_starts_on_the_fleet_and_hidden() {
        let state = UiState::default();
        assert_eq!(state.view_mode, ViewMode::Fleet);
        assert!(!state.panel_visible);
        assert!(state.panel_position.is_none());
    }

    /// The two views are the fleet and the selected host's detail, and the
    /// control at the bottom of the pulldown is what moves between them.
    #[test]
    fn the_two_views_toggle_between_each_other() {
        assert_eq!(ViewMode::Fleet.toggled(), ViewMode::Detail);
        assert_eq!(ViewMode::Detail.toggled(), ViewMode::Fleet);
        assert_eq!(ViewMode::Fleet.toggled().toggled(), ViewMode::Fleet);
    }

    /// A form is not one of the two views, and the control does not dismiss
    /// it: the operator's half-typed endpoint is still there.
    #[test]
    fn a_form_is_not_toggled_away() {
        assert_eq!(ViewMode::AddHost.toggled(), ViewMode::AddHost);
        assert_eq!(ViewMode::EditHost.toggled(), ViewMode::EditHost);
    }

    #[test]
    fn a_host_action_names_the_host_by_its_stable_id() {
        let (bridge, messages, state) = bridge_and_state();
        messages
            .send(BridgeMessage::Snapshot {
                snapshot: Box::new(document(1, 2)),
            })
            .expect("queues the document");
        apply(&bridge, &state);
        let state = state.lock().expect("state lock");
        let snapshot = state.snapshot.as_ref().expect("a document");
        let id = snapshot.systems[1].id.clone();
        let target = host_target(&state, &id).expect("the host is found");
        assert_eq!(target.id, id);
        assert!(!target.endpoint.is_empty());
    }

    #[test]
    fn an_action_for_a_host_that_is_gone_finds_nothing() {
        let (bridge, messages, state) = bridge_and_state();
        messages
            .send(BridgeMessage::Snapshot {
                snapshot: Box::new(document(1, 1)),
            })
            .expect("queues the document");
        apply(&bridge, &state);
        let state = state.lock().expect("state lock");
        assert!(host_target(&state, "no-such-host").is_none());
    }

    /// A seam that records what it was asked to do.
    struct RecordingMutations {
        seen: std::sync::Mutex<Vec<hosts::HostMutation>>,
        outcome: Result<String, String>,
    }

    impl RecordingMutations {
        fn new(outcome: Result<&str, &str>) -> Self {
            Self {
                seen: std::sync::Mutex::new(Vec::new()),
                outcome: match outcome {
                    Ok(message) => Ok(message.to_owned()),
                    Err(reason) => Err(reason.to_owned()),
                },
            }
        }
    }

    impl HostMutations for RecordingMutations {
        fn apply(&self, mutation: &hosts::HostMutation) -> Result<String, String> {
            self.seen
                .lock()
                .expect("the recording lock")
                .push(mutation.clone());
            self.outcome.clone()
        }
    }

    /// The state a form starts in, for the edit form of one host.
    fn editing_state(endpoint: &str, name: &str) -> UiState {
        UiState {
            view_mode: ViewMode::EditHost,
            form: HostForm::editing(HostTarget {
                id: "host-1".to_owned(),
                endpoint: endpoint.to_owned(),
                name: (!name.is_empty()).then(|| name.to_owned()),
            }),
            ..UiState::default()
        }
    }

    #[test]
    fn a_rename_is_presented_as_an_in_place_change() {
        let mut state = editing_state("10.0.0.5:11310", "Deadpool");
        state.form.name = "Deadpool Prime".to_owned();
        let view = form_view(&state);
        assert_eq!(view.apply_text, "Save name");
        assert!(
            view.note.contains("in place"),
            "the note says the identity is kept: {}",
            view.note
        );
        assert!(view.can_apply);
    }

    #[test]
    fn an_endpoint_change_is_presented_as_a_replacement() {
        let mut state = editing_state("10.0.0.5:11310", "Deadpool");
        state.form.endpoint = "10.0.0.6:11310".to_owned();
        let view = form_view(&state);
        assert_eq!(
            view.apply_text, "Replace endpoint",
            "the button says what it does"
        );
        assert!(
            view.note.contains("replaces the host") && view.note.contains("identity"),
            "the note says the host is replaced and its identity changes: {}",
            view.note
        );
        assert!(view.can_apply);
    }

    #[test]
    fn an_untouched_edit_form_offers_nothing() {
        let state = editing_state("10.0.0.5:11310", "Deadpool");
        let view = form_view(&state);
        assert!(
            !view.can_apply,
            "a form with nothing to apply must not offer to apply it"
        );
    }

    #[test]
    fn the_add_form_needs_an_endpoint_before_it_offers_anything() {
        let mut state = UiState {
            view_mode: ViewMode::AddHost,
            form: HostForm::adding(),
            ..UiState::default()
        };
        assert!(!form_view(&state).can_apply);
        state.form.endpoint = "10.0.0.5:11310".to_owned();
        assert!(form_view(&state).can_apply);
    }

    #[test]
    fn a_refused_mutation_keeps_the_operator_in_the_form() {
        let (bridge, _messages, desired) = Bridge::with_publisher_and_desired();
        let bridge = Arc::new(bridge);
        let manager: Arc<dyn HostMutations + Send + Sync> =
            Arc::new(RecordingMutations::new(Err("the endpoint has no port")));
        let mut state = editing_state("10.0.0.5:11310", "Deadpool");
        state.form.name = "Deadpool Prime".to_owned();
        let panel: slint::Weak<Pulldown> = slint::Weak::default();

        apply_mutation(
            &mut state,
            &manager,
            &bridge,
            &panel,
            &hosts::HostMutation::Rename {
                endpoint: "10.0.0.5:11310".to_owned(),
                name: Some("Deadpool Prime".to_owned()),
            },
        );

        assert_eq!(state.panel_message, "the endpoint has no port");
        assert!(state.panel_message_is_error, "the refusal is marked as one");
        assert_eq!(
            state.view_mode,
            ViewMode::EditHost,
            "the operator stays in the form so the mistake can be corrected"
        );
        assert!(
            !desired.borrow().reload,
            "a refused mutation must not ask the daemon to reload"
        );
    }

    #[test]
    fn an_applied_mutation_returns_to_the_fleet_and_asks_for_a_reload() {
        let (bridge, _messages, desired) = Bridge::with_publisher_and_desired();
        let bridge = Arc::new(bridge);
        let manager: Arc<dyn HostMutations + Send + Sync> =
            Arc::new(RecordingMutations::new(Ok("added 10.0.0.6:11310")));
        let mut state = UiState {
            view_mode: ViewMode::AddHost,
            form: HostForm::adding(),
            ..UiState::default()
        };
        state.form.endpoint = "10.0.0.6:11310".to_owned();
        let panel: slint::Weak<Pulldown> = slint::Weak::default();

        apply_mutation(
            &mut state,
            &manager,
            &bridge,
            &panel,
            &hosts::HostMutation::Add {
                endpoint: "10.0.0.6:11310".to_owned(),
                name: None,
            },
        );

        assert_eq!(state.panel_message, "added 10.0.0.6:11310");
        assert!(!state.panel_message_is_error);
        assert_eq!(state.view_mode, ViewMode::Fleet);
        assert!(
            desired.borrow().reload,
            "the daemon has to re-read the configuration at its own reload boundary"
        );
    }

    #[test]
    fn a_seam_that_says_nothing_still_says_what_was_attempted() {
        let (bridge, _messages, _desired) = Bridge::with_publisher_and_desired();
        let bridge = Arc::new(bridge);
        let manager: Arc<dyn HostMutations + Send + Sync> =
            Arc::new(RecordingMutations::new(Ok("")));
        let mut state = UiState::default();
        let panel: slint::Weak<Pulldown> = slint::Weak::default();

        apply_mutation(
            &mut state,
            &manager,
            &bridge,
            &panel,
            &hosts::HostMutation::Remove {
                endpoint: "10.0.0.5:11310".to_owned(),
            },
        );

        assert_eq!(state.panel_message, "remove 10.0.0.5:11310");
    }
}
