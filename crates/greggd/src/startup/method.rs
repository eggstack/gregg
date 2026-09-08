//! Startup-method identity, standard paths, environment detection, and selection.

use super::process::{run_bounded_command, MANAGER_COMMAND_TIMEOUT};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// ── Startup method ──────────────────────────────────────────────────────────

/// Internal startup method. `Direct` means unmanaged / no manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMethod {
    Systemd,
    Launchd,
    Cron,
    WindowsScm,
    Direct,
}
impl fmt::Display for StartupMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::Cron => "cron",
            Self::WindowsScm => "windows-scm",
            Self::Direct => "direct",
        };
        write!(f, "{s}")
    }
}

/// CLI argument for `startup install` / `instructions`.
///
/// `Auto` defers to platform detection; explicit variants override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupMethodArg {
    #[default]
    Auto,
    Systemd,
    Launchd,
    Cron,
}
impl std::str::FromStr for StartupMethodArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "systemd" => Ok(Self::Systemd),
            "launchd" => Ok(Self::Launchd),
            "cron" => Ok(Self::Cron),
            other => Err(format!(
                "unknown startup method '{other}'; expected auto, systemd, launchd, or cron"
            )),
        }
    }
}
impl fmt::Display for StartupMethodArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Auto => "auto",
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::Cron => "cron",
        };
        write!(f, "{s}")
    }
}
// ── Standard paths ─────────────────────────────────────────────────────────
pub fn standard_systemd_binary() -> PathBuf {
    PathBuf::from("/usr/local/bin/greggd")
}
pub fn standard_systemd_config() -> PathBuf {
    PathBuf::from("/etc/gregg/greggd.toml")
}
pub fn standard_systemd_unit_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system/greggd.service")
}
pub fn standard_systemd_config_dir() -> PathBuf {
    PathBuf::from("/etc/gregg")
}
pub fn standard_launchd_binary() -> PathBuf {
    PathBuf::from("/usr/local/bin/greggd")
}
pub fn standard_launchd_config() -> PathBuf {
    PathBuf::from("/Library/Application Support/gregg/greggd.toml")
}
pub fn standard_launchd_plist_path() -> PathBuf {
    PathBuf::from("/Library/LaunchDaemons/com.eggstack.greggd.plist")
}
pub fn launchd_label() -> &'static str {
    "com.eggstack.greggd"
}
// ── Systemd environment detection ─────────────────────────────────────────

/// Pure helper: does the host look like a running systemd environment given
/// injected probes? Used by tests to avoid touching the real filesystem.
///
/// `run_systemd_exists` mirrors `Path::new("/run/systemd/system").exists()`.
/// `proc1_comm_systemd` mirrors whether `/proc/1/comm` equals `systemd`.
pub fn is_systemd_environment_with(run_systemd_exists: bool, proc1_comm_is_systemd: bool) -> bool {
    // Require the /run/systemd/system directory. On Linux this is the
    // canonical indicator that systemd owns PID 1; checking proc1 adds a
    // second reliable signal without relying solely on systemctl presence.
    run_systemd_exists && proc1_comm_is_systemd
}

/// Real check: inspect `/run/systemd/system` and `/proc/1/comm` when available.
pub fn is_systemd_environment() -> bool {
    let run_exists = Path::new("/run/systemd/system").exists();
    // On non-Linux hosts this file won't exist; treat as non-systemd.
    let proc1_is_systemd = read_proc1_comm().is_some_and(|comm| comm == "systemd");
    // If /proc/1/comm is unavailable (container, non-Linux), fall back to
    // directory existence plus a bounded systemctl probe as equivalent signal.
    if run_exists && proc1_is_systemd {
        return true;
    }
    if run_exists {
        // Bounded systemctl probe: try `systemctl is-system-running --quiet`
        // with a short timeout. Use a best-effort check without blocking long.
        return systemctl_probe_is_running();
    }
    false
}
fn read_proc1_comm() -> Option<String> {
    let content = fs::read_to_string("/proc/1/comm").ok()?;
    Some(content.trim().to_string())
}
fn systemctl_probe_is_running() -> bool {
    run_bounded_command(
        "systemctl",
        &["is-system-running", "--quiet"],
        MANAGER_COMMAND_TIMEOUT,
    )
    .is_ok()
}
// ── Auto detection ────────────────────────────────────────────────────────

/// Pure helper for auto method selection. `os` values are `std::env::consts::OS`
/// style: "linux", "macos", "windows", etc.
pub fn auto_method_for(os: &str, is_systemd: bool) -> StartupMethod {
    match os {
        "windows" => StartupMethod::WindowsScm,
        "macos" | "darwin" => StartupMethod::Launchd,
        "linux" => {
            if is_systemd {
                StartupMethod::Systemd
            } else {
                StartupMethod::Cron
            }
        }
        _ => StartupMethod::Cron,
    }
}

/// Real auto detection using the current OS and live systemd probe.
pub fn auto_detect_method() -> StartupMethod {
    let os = std::env::consts::OS;
    let is_systemd = is_systemd_environment();
    auto_method_for(os, is_systemd)
}

/// Resolve a CLI `--method` argument to a concrete `StartupMethod`.
/// `Auto` defers to `auto_detect_method()`.
pub fn resolve_startup_method(arg: StartupMethodArg) -> StartupMethod {
    match arg {
        StartupMethodArg::Auto => auto_detect_method(),
        StartupMethodArg::Systemd => StartupMethod::Systemd,
        StartupMethodArg::Launchd => StartupMethod::Launchd,
        StartupMethodArg::Cron => StartupMethod::Cron,
    }
}

/// Pure helper for tests.
pub fn resolve_startup_method_with(
    arg: StartupMethodArg,
    os: &str,
    is_systemd: bool,
) -> StartupMethod {
    match arg {
        StartupMethodArg::Auto => auto_method_for(os, is_systemd),
        StartupMethodArg::Systemd => StartupMethod::Systemd,
        StartupMethodArg::Launchd => StartupMethod::Launchd,
        StartupMethodArg::Cron => StartupMethod::Cron,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_method_linux_systemd_is_systemd() {
        assert_eq!(auto_method_for("linux", true), StartupMethod::Systemd);
    }
    #[test]
    fn auto_method_linux_no_systemd_is_cron() {
        assert_eq!(auto_method_for("linux", false), StartupMethod::Cron);
    }
    #[test]
    fn auto_method_macos_is_launchd() {
        assert_eq!(auto_method_for("macos", false), StartupMethod::Launchd);
        assert_eq!(auto_method_for("macos", true), StartupMethod::Launchd);
    }
    #[test]
    fn auto_method_windows_is_scm() {
        assert_eq!(auto_method_for("windows", false), StartupMethod::WindowsScm);
    }
    #[test]
    fn explicit_overrides_auto() {
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Systemd, "linux", false),
            StartupMethod::Systemd
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Cron, "linux", true),
            StartupMethod::Cron
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Launchd, "linux", true),
            StartupMethod::Launchd
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Auto, "linux", true),
            StartupMethod::Systemd
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Auto, "linux", false),
            StartupMethod::Cron
        );
    }
    #[test]
    fn is_systemd_environment_with_helper() {
        assert!(is_systemd_environment_with(true, true));
        assert!(!is_systemd_environment_with(true, false));
        assert!(!is_systemd_environment_with(false, true));
        assert!(!is_systemd_environment_with(false, false));
    }
}
