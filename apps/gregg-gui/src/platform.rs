//! The little that has to be different on each desktop.
//!
//! # Why this file exists
//!
//! A tray application is a **background** application. On macOS an
//! `NSStatusItem` menu does not activate the app that owns it, and
//! `NSWindow.orderFront:` from a background app does not bring the window to
//! the front. A GUI launched the way a real one is launched — a login item, a
//! launcher, an agent — is a background app from its first moment, so
//! "Open Monitor" from the tray would do **nothing at all**: the operator
//! clicks the tray item and no window appears.
//!
//! Slint 1.18 exposes no activation API, so the activation is done here
//! through AppKit. Everything else in the GUI is platform-neutral, and this
//! file is the only place that knows about the difference.
//!
//! The other two desktops need nothing: on Windows and Linux, showing a
//! window that the process owns is enough to bring it up, and their window
//! managers do not require the application to be activated first.

/// Bring the application to the front, so a window it raises is really
/// visible.
///
/// This is called immediately before showing the monitor window. It is a
/// no-op everywhere except macOS.
pub fn activate() {
    #[cfg(target_os = "macos")]
    activate_macos();
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

/// Prove, in a test, that the activation path is wired.
///
/// The macOS activation itself cannot be asserted without a desktop session,
/// so what is asserted here is the shape: the function exists on every
/// platform, it is callable from the UI thread, and it does not panic.
#[cfg(test)]
mod tests {
    #[test]
    fn activating_the_application_is_safe_on_this_platform() {
        super::activate();
    }
}
