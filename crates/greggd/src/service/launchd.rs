//! launchd service management adapter for macOS.
//!
//! Uses `launchctl` with `bootstrap`, `bootout`, and `kickstart` flows
//! appropriate to supported macOS versions. Command construction is
//! centralized and testable. Paths with spaces are passed as
//! argument-array elements, not shell-quoted strings.

use std::process::Command;

use super::{ServiceError, ServiceManager};

/// The launchd service label for greggd.
const SERVICE_LABEL: &str = "com.eggstack.greggd";

/// Service manager backed by macOS launchd.
#[derive(Debug, Clone)]
pub struct LaunchdManager {
    label: String,
    /// The target domain for launchctl commands. Defaults to
    /// `system/$(domainname -A)` for system daemons.
    domain: Option<String>,
}

impl LaunchdManager {
    /// Create a new manager with default system domain.
    #[must_use]
    pub fn new() -> Self {
        Self {
            label: SERVICE_LABEL.to_owned(),
            domain: None,
        }
    }

    /// Create a manager with a custom label and domain (for testing).
    #[must_use]
    pub fn with_label(label: impl Into<String>, domain: Option<String>) -> Self {
        Self {
            label: label.into(),
            domain,
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
    #[allow(
        clippy::unused_self,
        reason = "kept for API consistency with systemd adapter"
    )]
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

    /// Check if the service is loaded by parsing `launchctl list`.
    fn check_loaded(&self) -> Result<bool, ServiceError> {
        let output = Command::new("launchctl")
            .args(["list"])
            .output()
            .map_err(|e| ServiceError::ExecFailed {
                command: "launchctl list".into(),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            return Err(ServiceError::CommandFailed {
                command: "launchctl list".into(),
                exit_status: output.status.code(),
                stderr,
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // A loaded service appears as a line with the label. Active services
        // have a PID; loaded but stopped services have `0` as PID.
        Ok(stdout.lines().any(|line| line.contains(&self.label)))
    }
}

impl Default for LaunchdManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager for LaunchdManager {
    fn start(&self) -> Result<(), ServiceError> {
        self.kickstart()
    }

    fn stop(&self) -> Result<(), ServiceError> {
        self.bootout()
    }

    fn restart(&self) -> Result<(), ServiceError> {
        self.kickstart()
    }

    fn is_active(&self) -> Result<bool, ServiceError> {
        self.check_loaded()
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
    }

    #[test]
    fn launchd_manager_with_custom_label() {
        let manager = LaunchdManager::with_label("com.test.greggd", Some("system".into()));
        assert_eq!(manager.label, "com.test.greggd");
        assert_eq!(manager.domain, Some("system".into()));
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
    }

    #[test]
    fn launchd_manager_debug() {
        let manager = LaunchdManager::new();
        let debug = format!("{manager:?}");
        assert!(debug.contains("LaunchdManager"));
    }
}
