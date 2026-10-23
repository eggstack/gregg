//! The little that has to be different on each desktop.
//!
//! # Why this file exists
//!
//! A tray application is a **background** application. On macOS an
//! `NSStatusItem` menu does not activate the app that owns it, and
//! `NSWindow.orderFront:` from a background app does not bring the window to
//! the front. A GUI launched the way a real one is launched — a login item, a
//! launcher, an agent — is a background app from its first moment, so
//! opening the pulldown from the tray would do **nothing at all**: the
//! operator clicks the tray item and no panel appears.
//!
//! Slint 1.18 exposes no activation API, so the activation is done here
//! through AppKit. Everything else in the GUI is platform-neutral, and this
//! file is the only place that knows about the difference.
//!
//! The other two desktops need nothing: on Windows and Linux, showing a
//! window that the process owns is enough to bring it up, and their window
//! managers do not require the application to be activated first.
//!
//! # The tray is not always there
//!
//! On Linux the tray is a `StatusNotifierItem`, and the desktop has to run a
//! **host** for it — the panel, the GNOME Shell extension, and so on. A
//! desktop without one has no tray at all: Slint logs the failure and carries
//! on with no icon, so the operator would be left with a window they can
//! close and no way to bring it back.
//!
//! So the GUI asks the desktop whether it has a tray, and answers that
//! question with one machine-readable line:
//!
//! ```text
//! gregg-gui: tray=available
//! gregg-gui: tray=unavailable reason="no StatusNotifierItem host on this desktop"
//! ```
//!
//! With no tray, closing the panel ends the GUI instead of hiding it, and the
//! panel says so. The decision is taken when the panel is closed, not at
//! startup: a tray host can start after the desktop session
//! is up, and Slint retries the registration when the icon is set again.

/// The reason a desktop with no tray host gets.
const NO_TRAY_HOST: &str = "no StatusNotifierItem host on this desktop";

/// The reason a desktop whose session bus cannot be reached gets.
const NO_SESSION_BUS: &str = "the D-Bus session bus could not be reached";

/// The reason a desktop whose session bus did not answer in time gets.
#[cfg(target_os = "linux")]
const SESSION_BUS_TIMEOUT: &str = "the D-Bus session bus did not answer in time";

/// How long the Linux tray probe may take before the answer is "no tray".
///
/// A wedged session bus must not turn a window close into a hang, so the
/// probe is bounded. "It did not answer" counts as "there is no tray",
/// because that is the safe answer: the operator keeps a window that quits
/// instead of a window that vanishes.
#[cfg(target_os = "linux")]
const TRAY_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// The well-known name a `StatusNotifierItem` host owns.
#[cfg(target_os = "linux")]
const STATUS_NOTIFIER_WATCHER: &str = "org.kde.StatusNotifierWatcher";

/// Whether this desktop has a system tray the GUI can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraySupport {
    /// The desktop has a tray, so closing the window can hide it.
    Available,
    /// The desktop has no usable tray, and the operator is told why.
    Unavailable {
        /// Why there is no tray, in words an operator can act on.
        reason: String,
    },
}

/// What closing the monitor window should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Hide the window: the tray is what brings it back.
    HideToTray,
    /// End the GUI: with no tray there is nothing that brings it back.
    Quit,
}

/// What the probe found means for the tray.
///
/// This is the whole decision, with no I/O in it: the desktop probe only has
/// to say whether it reached the bus and whether the bus has a tray host.
pub fn classify_tray_support(bus_reachable: bool, watcher_present: bool) -> TraySupport {
    match (bus_reachable, watcher_present) {
        (false, _) => TraySupport::Unavailable {
            reason: NO_SESSION_BUS.to_owned(),
        },
        (true, false) => TraySupport::Unavailable {
            reason: NO_TRAY_HOST.to_owned(),
        },
        (true, true) => TraySupport::Available,
    }
}

/// What closing the monitor window does, given the desktop's tray.
///
/// With a tray, closing the window hides it and the tray brings it back.
/// Without one, hiding the window would leave the operator with nothing:
/// the GUI would go on running with no way to reach it and no way to quit
/// it, so closing the window ends the GUI instead.
pub fn close_action(tray: &TraySupport) -> CloseAction {
    match tray {
        TraySupport::Available => CloseAction::HideToTray,
        TraySupport::Unavailable { .. } => CloseAction::Quit,
    }
}

/// The one machine-readable line the GUI prints about its tray.
///
/// The shape is fixed, because a CI job asserts on it: either
/// `gregg-gui: tray=available` or
/// `gregg-gui: tray=unavailable reason="..."`, on one line.
pub fn tray_status_line(tray: &TraySupport) -> String {
    match tray {
        TraySupport::Available => "gregg-gui: tray=available".to_owned(),
        TraySupport::Unavailable { reason } => {
            format!("gregg-gui: tray=unavailable reason=\"{}\"", flatten(reason))
        }
    }
}

/// The reason a tray is unavailable, or an empty string when it is available.
pub fn tray_reason(tray: &TraySupport) -> &str {
    match tray {
        TraySupport::Available => "",
        TraySupport::Unavailable { reason } => reason,
    }
}

/// The line the window shows when the desktop has no tray.
///
/// It is empty when the desktop has a tray, so the window only ever says
/// this when it is true.
pub fn tray_warning_text(tray: &TraySupport) -> String {
    match tray {
        TraySupport::Available => String::new(),
        TraySupport::Unavailable { .. } => {
            "no system tray on this desktop - closing this window quits Gregg".to_owned()
        }
    }
}

/// A reason, flattened onto one line and stripped of double quotes.
///
/// The status line is machine-readable: a reason carrying a newline or a
/// quote would break the line's shape.
fn flatten(reason: &str) -> String {
    reason
        .chars()
        .map(|character| match character {
            '"' => '\'',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ask the desktop whether it has a system tray, off the UI thread.
///
/// The probe is bounded, so the answer always arrives: a desktop with no tray
/// answers "unavailable" rather than nothing at all.
pub fn tray_support_async(apply: impl FnOnce(TraySupport) + Send + 'static) {
    std::thread::spawn(move || {
        let support = tray_support();
        apply(support);
    });
}

/// Bring the application to the front, so a window it raises is really
/// visible.
///
/// This is called immediately before showing the monitor window. It is a
/// no-op everywhere except macOS.
pub fn activate() {
    #[cfg(target_os = "macos")]
    activate_macos();
}

/// Whether this platform's tray carries a native menu.
///
/// The tray's **click** opens the pulldown, and on macOS a menu attached to
/// the status item swallows that click: AppKit pops the menu instead, and
/// the status item's own action never runs. So the menu is detached there,
/// and the pulldown carries the application's own actions — `Quit` among
/// them — instead.
///
/// Windows and Linux keep their menu, where it is the *context* menu: a
/// right-click reaches it and a left-click does not, so the click is still
/// the pulldown's.
#[must_use]
pub fn native_tray_menu() -> bool {
    #[cfg(target_os = "macos")]
    {
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Whether this platform can give a window a transparent background.
///
/// Only macOS: a frameless window there is transparent, so a rounded card
/// and its shadow are the desktop's own colours showing through. Windows and
/// Linux have no transparent window, so the card's corners are filled in
/// there and the shadow reads as a soft edge.
#[must_use]
pub fn transparent_window() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Install the desktop's own rules for this application.
///
/// # macOS
///
/// The application becomes a **menu-bar-only** application: no Dock icon,
/// no application menu, no launcher entry. A tray-only GUI has none of
/// those anyway — the pulldown carries `Reload`, `About` and `Quit` itself —
/// and there is a second reason that matters more:
///
/// A **Regular** (Dock) application can never put a window into another
/// application's full-screen space. Not at any window level, and not with
/// any collection behaviour: the desktop keeps a Dock application's windows
/// out of another application's space. An **Accessory** application can.
/// That is the difference between a pulldown that opens over the
/// application the operator is looking at and one that opens nowhere.
///
/// The policy is installed through the event-loop builder, because that is
/// where the desktop's own decision is made: it is applied when the
/// application finishes launching, which is before the pulldown's window
/// exists — and a window's right to reach another application's space is
/// fixed when the window is created.
///
/// The other two desktops need nothing: they have no full-screen spaces
/// that another application's windows are kept out of, so this is a no-op
/// there.
pub fn install_backend() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use slint::winit_030::winit::event_loop::EventLoop;
        use slint::winit_030::winit::platform::macos::{
            ActivationPolicy, EventLoopBuilderExtMacOS,
        };
        use slint::winit_030::SlintEvent;

        // The builder carries the desktop's own event type, so it is
        // Slint's builder and not a bare `winit` one.
        let mut builder = EventLoop::<SlintEvent>::with_user_event();
        builder.with_activation_policy(ActivationPolicy::Accessory);
        slint::BackendSelector::new()
            .with_winit_event_loop_builder(builder)
            .select()
            .map_err(|error| format!("the desktop backend could not be started: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// Let the pulldown float over another application's full-screen space.
///
/// A full-screen application owns a **space** of its own, and that space is
/// what the operator is looking at. A window that is not in that space is
/// not on the screen at all: the click reaches the tray, the pulldown is
/// built and placed, and the operator sees nothing happen. That is the
/// behaviour this fixes.
///
/// Three things have to be true, and all three are native:
///
/// - the window **joins every space**, including another application's
///   full-screen space (`CanJoinAllSpaces` and `FullScreenAuxiliary`), it
///   **stays put** when the space changes (`Stationary`), and it **stays
///   visible** when the application is deactivated;
/// - the window is **above** the full-screen content. `always-on-top` only
///   reaches the floating level, and the floating level is below
///   full-screen content, so the level is raised to the one a native status
///   item's own menu is drawn at;
/// - the application is a **menu-bar-only** application, which
///   [`install_backend`] takes care of.
///
/// `title` is the pulldown's own window title, and the native window is
/// found by it: the pulldown is the only window this application owns that
/// carries one, because the status item's own windows are AppKit's. The
/// title comes from the `.slint` file that owns the window, so the two
/// cannot drift apart.
///
/// Returns whether the pulldown's native window was found and configured.
/// It is `false` while the native window does not exist yet: the window is
/// created when the event loop resumes it, which is after the panel is
/// shown, so the caller asks again on the next tick.
#[cfg(target_os = "macos")]
pub fn prepare_overlay(title: &str) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSPopUpMenuWindowLevel, NSWindowCollectionBehavior};

    let Some(marker) = MainThreadMarker::new() else {
        return false;
    };
    let application = NSApplication::sharedApplication(marker);
    let Some(window) = application
        .windows()
        .iter()
        .find(|window| window.title().to_string() == title)
    else {
        return false;
    };
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setLevel(NSPopUpMenuWindowLevel);
    window.setHidesOnDeactivate(false);
    true
}

/// Let the pulldown float over another application's full-screen space.
///
/// The other two desktops need nothing: their window managers put a window
/// where it is asked to be, and the title is of no interest to them.
#[cfg(not(target_os = "macos"))]
pub fn prepare_overlay(_title: &str) -> bool {
    true
}

/// Activate the macOS application.
#[cfg(target_os = "macos")]
fn activate_macos() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    // `MainThreadMarker::new` succeeds only on the main thread. Slint's
    // callbacks run on the UI thread, which is the main thread, so this is
    // the normal case; a `None` means the caller was somewhere else, and
    // there is nothing sensible to activate from there.
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(marker);
    // `activateIgnoringOtherApps` is the call that makes the app frontmost.
    // It is deprecated in the newest SDK in favour of `activate`, which
    // requires macOS 14; this crate supports older systems, so the older
    // spelling is the one that works everywhere. The deprecation is a
    // warning in Apple's headers, not a behaviour change.
    #[allow(deprecated)]
    application.activateIgnoringOtherApps(true);
}

/// Ask the desktop whether it has a system tray.
///
/// macOS and Windows have a native tray that always works: the desktop has a
/// tray, and there is no host to look for. Linux has a `StatusNotifierItem`
/// host, and the probe asks the session bus whether one is running there.
pub fn tray_support() -> TraySupport {
    #[cfg(target_os = "linux")]
    {
        linux_tray_support()
    }
    #[cfg(not(target_os = "linux"))]
    {
        classify_tray_support(true, true)
    }
}

/// Probe the Linux session bus for a tray host.
#[cfg(target_os = "linux")]
fn linux_tray_support() -> TraySupport {
    match probe_linux() {
        LinuxProbe::Answered { watcher_present } => classify_tray_support(true, watcher_present),
        LinuxProbe::Unreachable => classify_tray_support(false, false),
        LinuxProbe::TimedOut => TraySupport::Unavailable {
            reason: SESSION_BUS_TIMEOUT.to_owned(),
        },
    }
}

/// What the Linux probe found.
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum LinuxProbe {
    /// The bus answered, and the watcher name is (or is not) owned.
    Answered {
        /// Whether a tray host owns the watcher's name.
        watcher_present: bool,
    },
    /// There was no session bus to talk to.
    Unreachable,
    /// The session bus did not answer in time.
    TimedOut,
}

/// Ask the session bus whether a `StatusNotifierItem` host is there.
///
/// The probe runs on its own thread and the caller waits for a bounded time,
/// so a wedged bus cannot hang the caller. A thread that misses the deadline
/// is left to finish on its own: it is a probe, and the caller has already
/// been given its answer.
#[cfg(target_os = "linux")]
fn probe_linux() -> LinuxProbe {
    let (sender, receiver) = std::sync::mpsc::channel();
    let Ok(worker) = std::thread::Builder::new()
        .name("gregg-gui-tray-probe".to_owned())
        .spawn(move || {
            let _ = sender.send(probe_linux_now());
        })
    else {
        return LinuxProbe::Unreachable;
    };
    // The probe thread is detached: it has nothing left to do once it has
    // answered, and the caller must not wait for it.
    drop(worker);
    match receiver.recv_timeout(TRAY_PROBE_TIMEOUT) {
        Ok(probe) => probe,
        Err(_) => LinuxProbe::TimedOut,
    }
}

/// The probe itself: talk to the session bus.
#[cfg(target_os = "linux")]
fn probe_linux_now() -> LinuxProbe {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return LinuxProbe::Unreachable;
    };
    let Ok(proxy) = zbus::blocking::fdo::DBusProxy::new(&connection) else {
        return LinuxProbe::Unreachable;
    };
    let Ok(name) = zbus::names::BusName::try_from(STATUS_NOTIFIER_WATCHER) else {
        return LinuxProbe::Unreachable;
    };
    // The name has an owner when a tray host is running. A name that is
    // merely *activatable* does not count: no host is there yet, and the
    // tray would not be there either.
    match proxy.name_has_owner(name) {
        Ok(watcher_present) => LinuxProbe::Answered { watcher_present },
        Err(_) => LinuxProbe::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activating_the_application_is_safe_on_this_platform() {
        super::activate();
    }

    #[test]
    fn preparing_the_overlay_without_a_pulldown_is_safe() {
        // There is no pulldown in a test process, so there is nothing to
        // prepare — but the call has to get there without panicking, because
        // it reaches AppKit.
        let _ = super::prepare_overlay("Gregg");
    }

    #[test]
    fn a_bus_with_a_tray_host_has_a_tray() {
        assert_eq!(classify_tray_support(true, true), TraySupport::Available);
    }

    #[test]
    fn a_bus_without_a_tray_host_has_no_tray() {
        let support = classify_tray_support(true, false);
        assert_eq!(
            support,
            TraySupport::Unavailable {
                reason: NO_TRAY_HOST.to_owned()
            }
        );
    }

    #[test]
    fn an_unreachable_bus_has_no_tray() {
        let support = classify_tray_support(false, false);
        assert_eq!(
            support,
            TraySupport::Unavailable {
                reason: NO_SESSION_BUS.to_owned()
            }
        );
        // An unreachable bus says nothing about the host, so the two
        // answers must not differ.
        assert_eq!(support, classify_tray_support(false, true));
    }

    #[test]
    fn a_desktop_with_a_tray_hides_the_window_on_close() {
        assert_eq!(
            close_action(&TraySupport::Available),
            CloseAction::HideToTray
        );
    }

    #[test]
    fn a_desktop_without_a_tray_quits_the_gui_on_close() {
        let support = TraySupport::Unavailable {
            reason: NO_TRAY_HOST.to_owned(),
        };
        assert_eq!(close_action(&support), CloseAction::Quit);
    }

    #[test]
    fn the_status_line_is_one_machine_readable_line() {
        assert_eq!(
            tray_status_line(&TraySupport::Available),
            "gregg-gui: tray=available"
        );
        assert_eq!(
            tray_status_line(&TraySupport::Unavailable {
                reason: NO_TRAY_HOST.to_owned()
            }),
            "gregg-gui: tray=unavailable reason=\"no StatusNotifierItem host on this desktop\""
        );
    }

    #[test]
    fn the_status_line_never_carries_a_second_line_or_a_quote() {
        let line = tray_status_line(&TraySupport::Unavailable {
            reason: "a \"quoted\"\nreason".to_owned(),
        });
        assert_eq!(line.lines().count(), 1);
        // The reason is quoted, so the line carries exactly the two
        // delimiters and nothing else.
        assert_eq!(line.matches('"').count(), 2);
        assert!(line.contains("reason=\"a 'quoted' reason\""));
    }

    #[test]
    fn the_window_only_warns_when_there_is_no_tray() {
        assert!(tray_warning_text(&TraySupport::Available).is_empty());
        assert_eq!(
            tray_warning_text(&TraySupport::Unavailable {
                reason: NO_TRAY_HOST.to_owned()
            }),
            "no system tray on this desktop - closing this window quits Gregg"
        );
    }

    #[test]
    fn this_desktop_has_a_tray_or_a_reason_why_it_does_not() {
        // The probe is the desktop's own business, but whatever it answers
        // has to be one of the two states the rest of the GUI knows.
        match tray_support() {
            TraySupport::Available => {}
            TraySupport::Unavailable { reason } => {
                assert!(!reason.is_empty(), "a missing tray is always explained");
            }
        }
    }
}
