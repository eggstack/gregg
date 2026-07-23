//! systemd service management adapter for Linux.
//!
//! Wraps `systemctl` with fixed argument arrays. No shell interpolation
//! is used. stderr is captured for diagnostic messages.

use std::process::Command;

use super::{ServiceError, ServiceManager};

/// The systemd unit name for greggd.
const UNIT_NAME: &str = "greggd";

/// Service manager backed by systemd's `systemctl`.
#[derive(Debug, Clone)]
pub struct SystemdManager {
    unit: String,
}

impl SystemdManager {
    /// Create a new manager using the default unit name.
    #[must_use]
    pub fn new() -> Self {
        Self {
            unit: UNIT_NAME.to_owned(),
        }
    }

    /// Create a manager with a custom unit name (for testing).
    #[must_use]
    pub fn with_unit(unit: impl Into<String>) -> Self {
        Self { unit: unit.into() }
    }

    /// Run `systemctl <action> <unit>` and return the result.
    fn run_systemctl(&self, action: &str) -> Result<(), ServiceError> {
        let output = Command::new("systemctl")
            .args([action, &self.unit])
            .output()
            .map_err(|e| ServiceError::ExecFailed {
                command: format!("systemctl {action} {}", self.unit),
                source: e,
            })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(ServiceError::CommandFailed {
                command: format!("systemctl {action} {}", self.unit),
                exit_status: output.status.code(),
                stderr,
            })
        }
    }

    /// Check if the unit is active by parsing `systemctl is-active`.
    fn check_active(&self) -> Result<bool, ServiceError> {
        let output = Command::new("systemctl")
            .args(["is-active", &self.unit])
            .output()
            .map_err(|e| ServiceError::ExecFailed {
                command: format!("systemctl is-active {}", self.unit),
                source: e,
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let active = output.status.success() && stdout.trim() == "active";
        Ok(active)
    }
}

impl Default for SystemdManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager for SystemdManager {
    fn start(&self) -> Result<(), ServiceError> {
        self.run_systemctl("start")
    }

    fn stop(&self) -> Result<(), ServiceError> {
        self.run_systemctl("stop")
    }

    fn restart(&self) -> Result<(), ServiceError> {
        self.run_systemctl("restart")
    }

    fn is_active(&self) -> Result<bool, ServiceError> {
        self.check_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_manager_construction() {
        let manager = SystemdManager::new();
        assert_eq!(manager.unit, "greggd");

        let custom = SystemdManager::with_unit("greggd-test");
        assert_eq!(custom.unit, "greggd-test");
    }

    #[test]
    fn systemd_manager_clone() {
        let manager = SystemdManager::new();
        let cloned = manager.clone();
        assert_eq!(manager.unit, cloned.unit);
    }

    #[test]
    fn systemd_manager_debug() {
        let manager = SystemdManager::new();
        let debug = format!("{manager:?}");
        assert!(debug.contains("SystemdManager"));
    }
}
