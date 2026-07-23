//! Service management abstraction.
//!
//! Provides a platform-independent trait for controlling the native system
//! service manager (systemd on Linux, launchd on macOS). External command
//! invocation is acceptable here because `systemctl`/`launchctl` are the
//! native administrative interfaces.
//!
//! A [`NoopServiceManager`] is provided for testing and development.

use std::fmt;

pub mod launchd;
pub mod systemd;

/// Errors returned by service management operations.
#[derive(Debug)]
pub enum ServiceError {
    /// The service manager command failed with the given exit status.
    CommandFailed {
        command: String,
        exit_status: Option<i32>,
        stderr: String,
    },
    /// The service manager command could not be executed.
    ExecFailed {
        command: String,
        source: std::io::Error,
    },
    /// Platform-specific service manager is not available.
    NotAvailable { platform: String },
    /// The service state could not be determined.
    StateQueryFailed { source: std::io::Error },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandFailed {
                command,
                exit_status,
                stderr,
            } => {
                write!(f, "command `{command}` failed")?;
                if let Some(status) = exit_status {
                    write!(f, " (exit status: {status})")?;
                }
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            Self::ExecFailed { command, source } => {
                write!(f, "failed to execute `{command}`: {source}")
            }
            Self::NotAvailable { platform } => {
                write!(f, "service manager not available on {platform}")
            }
            Self::StateQueryFailed { source } => {
                write!(f, "failed to query service state: {source}")
            }
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExecFailed { source, .. } | Self::StateQueryFailed { source } => Some(source),
            _ => None,
        }
    }
}

/// Trait for platform-specific service management.
///
/// Implementations wrap the native service manager (systemd, launchd)
/// and provide a uniform interface for `start`/`stop`/`restart`/`is_active`
/// operations.
pub trait ServiceManager: Send + Sync {
    /// Start the greggd service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if the command fails or cannot be executed.
    fn start(&self) -> Result<(), ServiceError>;

    /// Stop the greggd service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if the command fails or cannot be executed.
    fn stop(&self) -> Result<(), ServiceError>;

    /// Restart the greggd service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if the command fails or cannot be executed.
    fn restart(&self) -> Result<(), ServiceError>;

    /// Check whether the greggd service is currently active (running).
    ///
    /// Returns `true` if the service is active, `false` if inactive or
    /// stopped.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if the state cannot be determined.
    fn is_active(&self) -> Result<bool, ServiceError>;
}

/// A no-op service manager for testing and development.
///
/// All operations succeed without side effects. `is_active` always
/// returns `false`.
#[derive(Debug, Default)]
pub struct NoopServiceManager;

impl ServiceManager for NoopServiceManager {
    fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    fn stop(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    fn restart(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    fn is_active(&self) -> Result<bool, ServiceError> {
        Ok(false)
    }
}

/// Return the platform-appropriate service manager.
///
/// On Linux, returns a [`systemd::SystemdManager`]. On macOS, returns a
/// [`launchd::LaunchdManager`]. On other platforms, returns a
/// [`NoopServiceManager`].
#[must_use]
pub fn platform_service_manager() -> Box<dyn ServiceManager> {
    #[cfg(target_os = "linux")]
    {
        Box::new(systemd::SystemdManager::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(launchd::LaunchdManager::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Box::new(NoopServiceManager)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_manager_operations_succeed() {
        let manager = NoopServiceManager;
        assert!(manager.start().is_ok());
        assert!(manager.stop().is_ok());
        assert!(manager.restart().is_ok());
        assert!(!manager.is_active().unwrap());
    }

    #[test]
    fn service_error_display() {
        let err = ServiceError::CommandFailed {
            command: "systemctl stop greggd".into(),
            exit_status: Some(1),
            stderr: "unit not found".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("systemctl stop greggd"));
        assert!(msg.contains("exit status: 1"));
        assert!(msg.contains("unit not found"));
    }

    #[test]
    fn service_error_exec_failed() {
        let err = ServiceError::ExecFailed {
            command: "systemctl".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let msg = format!("{err}");
        assert!(msg.contains("systemctl"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn service_error_not_available() {
        let err = ServiceError::NotAvailable {
            platform: "windows".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("windows"));
    }
}
