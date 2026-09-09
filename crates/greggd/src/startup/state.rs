//! Detected startup-manager state for restart/update decisions.

#[cfg(all(unix, target_os = "macos"))]
use super::launchd::{launchd_is_loaded, launchd_plist_exists};
#[cfg(all(unix, target_os = "linux"))]
use super::method::is_systemd_environment;
#[cfg(all(unix, target_os = "linux"))]
use super::systemd::{systemd_is_active, systemd_unit_exists};
use std::fmt;

// ── Manager state detection for restart / update ──────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupState {
    SystemdActive,
    SystemdInstalledStopped,
    LaunchdLoaded,
    LaunchdInstalledUnloaded,
    WindowsServiceRunning,
    WindowsServiceStopped,
    UnmanagedOrCron,
}
impl fmt::Display for StartupState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::SystemdActive => "systemd-active",
            Self::SystemdInstalledStopped => "systemd-installed-stopped",
            Self::LaunchdLoaded => "launchd-loaded",
            Self::LaunchdInstalledUnloaded => "launchd-installed-unloaded",
            Self::WindowsServiceRunning => "windows-running",
            Self::WindowsServiceStopped => "windows-stopped",
            Self::UnmanagedOrCron => "unmanaged-or-cron",
        };
        write!(f, "{s}")
    }
}

/// Pure helper: decide systemd state from injected probes.
pub fn systemd_state_with(unit_exists: bool, is_active: bool) -> StartupState {
    if is_active {
        StartupState::SystemdActive
    } else if unit_exists {
        StartupState::SystemdInstalledStopped
    } else {
        StartupState::UnmanagedOrCron
    }
}

/// Pure helper: decide launchd state from injected probes.
pub fn launchd_state_with(plist_exists: bool, is_loaded: bool) -> StartupState {
    if is_loaded {
        StartupState::LaunchdLoaded
    } else if plist_exists {
        StartupState::LaunchdInstalledUnloaded
    } else {
        StartupState::UnmanagedOrCron
    }
}
pub fn startup_state() -> StartupState {
    #[cfg(target_os = "windows")]
    {
        // Delegate to existing SCM manager when on Windows.
        match crate::service::platform_service_manager().is_active() {
            Ok(true) => StartupState::WindowsServiceRunning,
            Ok(false) => StartupState::WindowsServiceStopped,
            Err(_) => StartupState::UnmanagedOrCron,
        }
    }
    #[cfg(all(unix, target_os = "macos"))]
    {
        let plist_exists = launchd_plist_exists();
        let loaded = launchd_is_loaded();
        let state = launchd_state_with(plist_exists, loaded);
        if state != StartupState::UnmanagedOrCron {
            return state;
        }
        // Not launchd-managed: fall through to systemd/cron check.
        // On macOS, systemd is not relevant, so return unmanaged.
        StartupState::UnmanagedOrCron
    }
    #[cfg(all(unix, target_os = "linux"))]
    {
        let unit_exists = systemd_unit_exists();
        let active = if unit_exists || is_systemd_environment() {
            systemd_is_active()
        } else {
            false
        };
        let state = systemd_state_with(unit_exists, active);
        if state != StartupState::UnmanagedOrCron {
            return state;
        }
        StartupState::UnmanagedOrCron
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        StartupState::UnmanagedOrCron
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_state_helpers() {
        assert_eq!(systemd_state_with(true, true), StartupState::SystemdActive);
        assert_eq!(
            systemd_state_with(true, false),
            StartupState::SystemdInstalledStopped
        );
        assert_eq!(
            systemd_state_with(false, false),
            StartupState::UnmanagedOrCron
        );
        assert_eq!(launchd_state_with(false, true), StartupState::LaunchdLoaded);
        assert_eq!(
            launchd_state_with(true, false),
            StartupState::LaunchdInstalledUnloaded
        );
    }
}
