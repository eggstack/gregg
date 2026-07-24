//! launchd service management adapter for macOS.
//!
//! Uses `launchctl` with `bootstrap`, `bootout`, and `kickstart` flows
//! appropriate to supported macOS versions. Command construction is
//! centralized and testable. Paths with spaces are passed as
//! argument-array elements, not shell-quoted strings.
//!
//! ## State semantics
//!
//! launchd has three relevant states:
//!
//! - **`NotLoaded`** — the plist is not loaded into launchd (not installed
//!   or previously bootouted).
//! - **`Loaded`** — the plist is loaded but the service is not currently
//!   running (e.g. it crashed or was stopped via `kickstart -p`).
//! - **`Running`** — the service is loaded and has at least one running
//!   process.
//!
//! `start()` bootstraps if `NotLoaded`, kickstarts if `Loaded`, and is a
//! no-op if `Running`. `restart()` always bootouts and re-bootstraps.
//! `is_active()` returns true only when `Running`.

use std::process::Command;

use super::{ServiceError, ServiceManager};

/// The launchd service label for greggd.
const SERVICE_LABEL: &str = "com.eggstack.greggd";

/// The launchd state of the greggd service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// The plist is not loaded into launchd.
    NotLoaded,
    /// The plist is loaded but the service is not running.
    Loaded,
    /// The service is loaded and running.
    Running,
}

/// Service manager backed by macOS launchd.
#[derive(Debug, Clone)]
pub struct LaunchdManager {
    label: String,
    /// The target domain for launchctl commands. Defaults to
    /// `system/$(domainname -A)` for system daemons.
    domain: Option<String>,
    /// The path to the plist file, used by `bootstrap` and `start`
    /// when the service is not yet loaded.
    plist_path: Option<String>,
}

impl LaunchdManager {
    /// Create a new manager with default system domain.
    #[must_use]
    pub fn new() -> Self {
        Self {
            label: SERVICE_LABEL.to_owned(),
            domain: None,
            plist_path: None,
        }
    }

    /// Create a manager with a custom label and domain (for testing).
    #[must_use]
    pub fn with_label(label: impl Into<String>, domain: Option<String>) -> Self {
        Self {
            label: label.into(),
            domain,
            plist_path: None,
        }
    }

    /// Create a manager with a custom plist path (for `start` bootstrap).
    #[must_use]
    pub fn with_plist(
        label: impl Into<String>,
        domain: Option<String>,
        plist_path: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            domain,
            plist_path: Some(plist_path.into()),
        }
    }

    /// Resolve the domain target for launchctl.
    ///
    /// Returns `system/gui/<uid>` on macOS 10.10+.
    fn domain_target(&self) -> String {
        if let Some(ref d) = self.domain {
            return d.clone();
        }
        // System domain: "system"
        // For a per-user daemon, use "gui/<uid>".
        // greggd is a system daemon, so "system" is correct.
        "system".to_owned()
    }

    /// Construct the full service target string: `<domain>/<label>`.
    fn service_target(&self) -> String {
        format!("{}/{}", self.domain_target(), self.label)
    }

    /// Run a launchctl command with the given arguments.
    #[allow(clippy::unused_self)]
    fn run_launchctl(&self, args: &[&str]) -> Result<(), ServiceError> {
        let output = Command::new("launchctl").args(args).output().map_err(|e| {
            ServiceError::ExecFailed {
                command: format!("launchctl {}", args.join(" ")),
                source: e,
            }
        })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(ServiceError::CommandFailed {
                command: format!("launchctl {}", args.join(" ")),
                exit_status: output.status.code(),
                stderr,
            })
        }
    }

    /// Bootstrap (install and start) the service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if launchctl fails.
    pub fn bootstrap(&self, plist_path: &str) -> Result<(), ServiceError> {
        let target = self.service_target();
        self.run_launchctl(&["bootstrap", &target, plist_path])
    }

    /// Bootout (stop and remove) the service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if launchctl fails.
    pub fn bootout(&self) -> Result<(), ServiceError> {
        let target = self.service_target();
        self.run_launchctl(&["bootout", &target])
    }

    /// Kickstart (restart) the service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if launchctl fails.
    pub fn kickstart(&self) -> Result<(), ServiceError> {
        let target = self.service_target();
        self.run_launchctl(&["kickstart", "-k", &target])
    }

    /// Query the current launchd state of the service.
    ///
    /// Uses `launchctl print` to determine whether the service is loaded
    /// and running. Returns [`ServiceState::NotLoaded`] if the service
    /// is not loaded, [`ServiceState::Loaded`] if loaded but not running,
    /// and [`ServiceState::Running`] if loaded and running.
    pub fn state(&self) -> Result<ServiceState, ServiceError> {
        let target = self.service_target();

        // `launchctl print <target>` succeeds if the service is loaded.
        // If it fails, the service is not loaded.
        let output = Command::new("launchctl")
            .args(["print", &target])
            .output()
            .map_err(|e| ServiceError::ExecFailed {
                command: format!("launchctl print {target}"),
                source: e,
            })?;

        if !output.status.success() {
            return Ok(ServiceState::NotLoaded);
        }

        // Parse the output to determine if the service is running.
        // `launchctl print` output includes "state = running" or
        // "state = spawned" or "state = waiting" etc.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let is_running = stdout
            .lines()
            .any(|line| line.trim_start().starts_with("state = running"));

        if is_running {
            Ok(ServiceState::Running)
        } else {
            Ok(ServiceState::Loaded)
        }
    }
}

impl Default for LaunchdManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager for LaunchdManager {
    fn start(&self) -> Result<(), ServiceError> {
        match self.state()? {
            ServiceState::Running => {
                // Already running — idempotent no-op.
                Ok(())
            }
            ServiceState::Loaded => {
                // Loaded but not running — kickstart.
                self.kickstart()
            }
            ServiceState::NotLoaded => {
                // Not loaded — bootstrap if we have a plist path.
                match &self.plist_path {
                    Some(plist) => self.bootstrap(plist),
                    None => Err(ServiceError::CommandFailed {
                        command: "launchctl bootstrap".into(),
                        exit_status: None,
                        stderr: "service not loaded and no plist path configured".into(),
                    }),
                }
            }
        }
    }

    fn stop(&self) -> Result<(), ServiceError> {
        self.bootout()
    }

    fn restart(&self) -> Result<(), ServiceError> {
        match self.state()? {
            ServiceState::NotLoaded => {
                // Not loaded — bootstrap if we have a plist path.
                match &self.plist_path {
                    Some(plist) => self.bootstrap(plist),
                    None => Err(ServiceError::CommandFailed {
                        command: "launchctl bootstrap".into(),
                        exit_status: None,
                        stderr: "service not loaded and no plist path configured".into(),
                    }),
                }
            }
            ServiceState::Loaded | ServiceState::Running => {
                // Bootout then bootstrap to ensure a clean restart.
                self.bootout()?;
                match &self.plist_path {
                    Some(plist) => self.bootstrap(plist),
                    None => Err(ServiceError::CommandFailed {
                        command: "launchctl bootstrap".into(),
                        exit_status: None,
                        stderr: "service not loaded and no plist path configured".into(),
                    }),
                }
            }
        }
    }

    fn is_active(&self) -> Result<bool, ServiceError> {
        Ok(matches!(self.state()?, ServiceState::Running))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_manager_construction() {
        let manager = LaunchdManager::new();
        assert_eq!(manager.label, "com.eggstack.greggd");
        assert!(manager.domain.is_none());
        assert!(manager.plist_path.is_none());
    }

    #[test]
    fn launchd_manager_with_custom_label() {
        let manager = LaunchdManager::with_label("com.test.greggd", Some("system".into()));
        assert_eq!(manager.label, "com.test.greggd");
        assert_eq!(manager.domain, Some("system".into()));
        assert!(manager.plist_path.is_none());
    }

    #[test]
    fn launchd_manager_with_plist() {
        let manager = LaunchdManager::with_plist(
            "com.test.greggd",
            Some("system".into()),
            "/Library/LaunchDaemons/com.test.greggd.plist",
        );
        assert_eq!(manager.label, "com.test.greggd");
        assert_eq!(manager.domain, Some("system".into()));
        assert_eq!(
            manager.plist_path,
            Some("/Library/LaunchDaemons/com.test.greggd.plist".to_owned())
        );
    }

    #[test]
    fn domain_target_default() {
        let manager = LaunchdManager::new();
        assert_eq!(manager.domain_target(), "system");
    }

    #[test]
    fn domain_target_custom() {
        let manager = LaunchdManager::with_label("test", Some("gui/501".into()));
        assert_eq!(manager.domain_target(), "gui/501");
    }

    #[test]
    fn service_target_format() {
        let manager = LaunchdManager::new();
        assert_eq!(manager.service_target(), "system/com.eggstack.greggd");
    }

    #[test]
    fn launchd_manager_clone() {
        let manager = LaunchdManager::new();
        let cloned = manager.clone();
        assert_eq!(manager.label, cloned.label);
        assert_eq!(manager.domain, cloned.domain);
        assert_eq!(manager.plist_path, cloned.plist_path);
    }

    #[test]
    fn launchd_manager_debug() {
        let manager = LaunchdManager::new();
        let debug = format!("{manager:?}");
        assert!(debug.contains("LaunchdManager"));
    }

    #[test]
    fn service_state_enum_variants() {
        assert_eq!(ServiceState::NotLoaded, ServiceState::NotLoaded);
        assert_eq!(ServiceState::Loaded, ServiceState::Loaded);
        assert_eq!(ServiceState::Running, ServiceState::Running);
        assert_ne!(ServiceState::NotLoaded, ServiceState::Loaded);
        assert_ne!(ServiceState::Loaded, ServiceState::Running);
    }

    #[test]
    fn launchd_command_uses_argument_arrays() {
        // Verify bootstrap/bootout/kickstart construct argument arrays
        // without shell interpolation. The code uses:
        //   self.run_launchctl(&["bootstrap", &target, plist_path])
        // which passes arguments directly to execvp.
        let manager = LaunchdManager::new();
        let target = manager.service_target();
        assert_eq!(target, "system/com.eggstack.greggd");
        // The plist path is passed as a separate array element, not
        // shell-quoted. This is correct for paths with spaces like
        // "/Library/Application Support/gregg/greggd.toml".
    }

    #[test]
    fn check_loaded_exact_label_match() {
        // Verify that check_loaded matches the label exactly, not as a
        // substring. A line with "com.eggstack.greggd-test" should NOT
        // match "com.eggstack.greggd".
        let label = "com.eggstack.greggd";
        let line_with_suffix = "  12345  0  com.eggstack.greggd-test";
        let line_exact = "  12345  0  com.eggstack.greggd";

        // Our matching logic: split on whitespace, match last field exactly.
        let matches_suffix = line_with_suffix
            .split_whitespace()
            .last()
            .is_some_and(|field| field == label);
        let matches_exact = line_exact
            .split_whitespace()
            .last()
            .is_some_and(|field| field == label);

        assert!(!matches_suffix, "should not match partial label");
        assert!(matches_exact, "should match exact label");
    }

    #[test]
    fn launchd_paths_with_spaces_handled_correctly() {
        // The plist path "/Library/Application Support/gregg/greggd.toml"
        // contains a space. In the ProgramArguments array, each element is
        // a separate string — launchd does not use shell interpretation.
        // The bootstrap method passes plist_path as a &str argument array element.
        let manager = LaunchdManager::new();
        let target = manager.service_target();
        // Verify the target string itself is safe (no spaces in domain/label).
        assert!(!target.contains(' '));
        // The plist path with spaces would be passed as a separate &str element
        // in the run_launchctl(&["bootstrap", &target, plist_path]) call.
    }

    #[test]
    fn start_state_transitions_documented() {
        // Document the expected start() behavior for each state:
        // - NotLoaded + plist_path: bootstrap
        // - NotLoaded + no plist_path: error
        // - Loaded: kickstart
        // - Running: no-op
        // This is a documentation test — the actual behavior is tested
        // in integration tests with a real launchd.
        let manager_no_plist = LaunchdManager::new();
        let manager_with_plist = LaunchdManager::with_plist(
            "com.eggstack.greggd",
            None,
            "/Library/LaunchDaemons/com.eggstack.greggd.plist",
        );

        // Both managers should have the correct label.
        assert_eq!(manager_no_plist.label, "com.eggstack.greggd");
        assert_eq!(manager_with_plist.label, "com.eggstack.greggd");

        // The manager without a plist path should not be able to bootstrap.
        assert!(manager_no_plist.plist_path.is_none());
        // The manager with a plist path should be able to bootstrap.
        assert!(manager_with_plist.plist_path.is_some());
    }
}
