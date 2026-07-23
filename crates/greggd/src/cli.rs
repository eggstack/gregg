//! CLI argument parsing and subcommand dispatch for `greggd`.
//!
//! Uses `clap` derive macros for structured argument parsing. Each
//! subcommand has a stable help message and returns a meaningful exit code.

use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{Config, ConfigError};
use crate::service::{ServiceError, ServiceManager};

/// Lightweight metrics daemon for the gregg monitoring system.
#[derive(Parser)]
#[command(
    name = "greggd",
    version,
    about = "Lightweight Linux and macOS metrics daemon",
    long_about = "greggd runs on designated systems and exposes a read-only JSON API \
                  for the gregg terminal client. It samples CPU, memory, swap, and \
                  load metrics on a configurable interval and serves cached immutable \
                  snapshots over HTTP/1."
)]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(
        long,
        short = 'c',
        global = true,
        help = "Path to the TOML configuration file",
        value_name = "PATH"
    )]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Run the daemon in the foreground (used by systemd/launchd).
    Run,
    /// Start the greggd system service.
    Start,
    /// Stop the greggd system service.
    Stop,
    /// Restart the greggd system service.
    Restart,
    /// Idempotent service check: starts the service if not active.
    Croncheck,
    /// Update the bind address and restart the service.
    Host {
        /// The new IPv4 or IPv6 address to bind to.
        address: IpAddr,
    },
    /// Update the TCP port and restart the service.
    Port {
        /// The new port number (1-65535).
        port: u16,
    },
}

/// Exit codes returned by greggd commands.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    /// Configuration error (invalid, missing, or unwritable).
    ConfigError = 1,
    /// Service management command failed.
    ServiceError = 2,
    /// The daemon could not start (port conflict, etc.).
    RuntimeError = 3,
    /// Permission denied for the requested operation.
    PermissionDenied = 4,
}

impl From<&ConfigError> for ExitCode {
    fn from(e: &ConfigError) -> Self {
        match e {
            ConfigError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                Self::PermissionDenied
            }
            ConfigError::AtomicWrite { source, .. } => match source {
                crate::config::AtomicWriteError::Io(io)
                    if io.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    Self::PermissionDenied
                }
                _ => Self::ConfigError,
            },
            _ => Self::ConfigError,
        }
    }
}

impl From<&ServiceError> for ExitCode {
    fn from(e: &ServiceError) -> Self {
        match e {
            ServiceError::CommandFailed { .. }
            | ServiceError::ExecFailed { .. }
            | ServiceError::NotAvailable { .. }
            | ServiceError::StateQueryFailed { .. } => Self::ServiceError,
        }
    }
}

/// Resolve the config path: explicit `--config` or platform default.
pub fn resolve_config_path(explicit: Option<&PathBuf>) -> PathBuf {
    explicit.cloned().unwrap_or_else(Config::default_path)
}

/// Load or create the configuration.
///
/// If the config file exists, load and validate it. If it does not exist
/// and no explicit path was given, use defaults. If an explicit path was
/// given but the file is missing, return an error.
pub fn load_config(path: &std::path::Path, explicit: bool) -> Result<Config, ConfigError> {
    if path.exists() {
        Config::load(path)
    } else if explicit {
        Err(ConfigError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("configuration file not found: {}", path.display()),
            ),
        })
    } else {
        // No explicit path and file doesn't exist — use defaults.
        Ok(Config::default())
    }
}

/// Error returned when config validation fails during mutation.
///
/// This is separate from `ConfigError` because it carries the violations
/// for structured reporting and requires a distinct exit code.
#[derive(Debug)]
pub struct ConfigValidationError(pub Vec<crate::config::ConfigViolation>);

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "configuration validation failed:")?;
        for v in &self.0 {
            write!(f, "\n  - {v}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigValidationError {}

/// Update a single field in the config and atomically persist it.
///
/// This is the shared logic for `host` and `port` subcommands.
pub fn mutate_and_restart(
    path: &std::path::Path,
    explicit: bool,
    mutate: impl FnOnce(&mut Config),
    service: &dyn ServiceManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = load_config(path, explicit)?;
    mutate(&mut config);

    let violations = config.validate();
    if !violations.is_empty() {
        eprintln!("configuration validation failed:");
        for v in &violations {
            eprintln!("  - {v}");
        }
        return Err(Box::new(ConfigValidationError(violations)));
    }

    config.write_atomic(path)?;

    service.restart().map_err(|e| {
        eprintln!("failed to restart service: {e}");
        Box::new(e) as Box<dyn std::error::Error>
    })?;

    Ok(())
}

/// Dispatch a subcommand.
///
/// # Errors
///
/// Returns an error if the command fails.
pub fn dispatch(
    command: &Command,
    config_path: &std::path::Path,
    service: &dyn ServiceManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let explicit = true; // resolve_config_path already determined this

    match command {
        Command::Run => {
            // Delegate to the async run entry point.
            // This is handled in main.rs.
            unreachable!("Command::Run is handled in main.rs")
        }
        Command::Start => {
            service.start().map_err(|e| {
                eprintln!("failed to start service: {e}");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            Ok(())
        }
        Command::Stop => {
            service.stop().map_err(|e| {
                eprintln!("failed to stop service: {e}");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            Ok(())
        }
        Command::Restart => {
            service.restart().map_err(|e| {
                eprintln!("failed to restart service: {e}");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            Ok(())
        }
        Command::Croncheck => match service.is_active() {
            Ok(true) => {
                // Already active — idempotent success.
                Ok(())
            }
            Ok(false) => {
                // Not active — start it.
                service.start().map_err(|e| {
                    eprintln!("failed to start service: {e}");
                    Box::new(e) as Box<dyn std::error::Error>
                })?;
                Ok(())
            }
            Err(e) => {
                eprintln!("could not determine service state: {e}");
                Err(Box::new(e))
            }
        },
        Command::Host { address } => mutate_and_restart(
            config_path,
            explicit,
            |config| {
                config.host = *address;
            },
            service,
        ),
        Command::Port { port } => mutate_and_restart(
            config_path,
            explicit,
            |config| {
                config.port = *port;
            },
            service,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::NoopServiceManager;
    use std::path::Path;
    use std::sync::Mutex;

    /// A fake service manager that records all calls and allows
    /// controlling `is_active` and error behavior.
    #[derive(Debug)]
    struct FakeServiceManager {
        active: Mutex<bool>,
        /// Record of all method calls made.
        calls: Mutex<Vec<&'static str>>,
        /// If set, `start` returns this error.
        start_error: Mutex<Option<ServiceError>>,
        /// If set, `restart` returns this error.
        restart_error: Mutex<Option<ServiceError>>,
        /// If set, `is_active` returns this error.
        is_active_error: Mutex<Option<ServiceError>>,
    }

    impl FakeServiceManager {
        fn new() -> Self {
            Self {
                active: Mutex::new(false),
                calls: Mutex::new(Vec::new()),
                start_error: Mutex::new(None),
                restart_error: Mutex::new(None),
                is_active_error: Mutex::new(None),
            }
        }

        fn set_active(&self, active: bool) {
            *self.active.lock().unwrap() = active;
        }

        fn set_start_error(&self, err: ServiceError) {
            *self.start_error.lock().unwrap() = Some(err);
        }

        fn set_restart_error(&self, err: ServiceError) {
            *self.restart_error.lock().unwrap() = Some(err);
        }

        fn set_is_active_error(&self, err: ServiceError) {
            *self.is_active_error.lock().unwrap() = Some(err);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ServiceManager for FakeServiceManager {
        fn start(&self) -> Result<(), ServiceError> {
            self.calls.lock().unwrap().push("start");
            if let Some(err) = self.start_error.lock().unwrap().take() {
                return Err(err);
            }
            *self.active.lock().unwrap() = true;
            Ok(())
        }

        fn stop(&self) -> Result<(), ServiceError> {
            self.calls.lock().unwrap().push("stop");
            *self.active.lock().unwrap() = false;
            Ok(())
        }

        fn restart(&self) -> Result<(), ServiceError> {
            self.calls.lock().unwrap().push("restart");
            if let Some(err) = self.restart_error.lock().unwrap().take() {
                return Err(err);
            }
            Ok(())
        }

        fn is_active(&self) -> Result<bool, ServiceError> {
            self.calls.lock().unwrap().push("is_active");
            if let Some(err) = self.is_active_error.lock().unwrap().take() {
                return Err(err);
            }
            Ok(*self.active.lock().unwrap())
        }
    }

    // --- CLI parsing tests ---

    #[test]
    fn cli_parses_run_command() {
        let cli = Cli::try_parse_from(["greggd", "run"]).unwrap();
        assert!(matches!(cli.command, Command::Run));
        assert!(cli.config.is_none());
    }

    #[test]
    fn cli_parses_start_command() {
        let cli = Cli::try_parse_from(["greggd", "start"]).unwrap();
        assert!(matches!(cli.command, Command::Start));
    }

    #[test]
    fn cli_parses_stop_command() {
        let cli = Cli::try_parse_from(["greggd", "stop"]).unwrap();
        assert!(matches!(cli.command, Command::Stop));
    }

    #[test]
    fn cli_parses_restart_command() {
        let cli = Cli::try_parse_from(["greggd", "restart"]).unwrap();
        assert!(matches!(cli.command, Command::Restart));
    }

    #[test]
    fn cli_parses_croncheck_command() {
        let cli = Cli::try_parse_from(["greggd", "croncheck"]).unwrap();
        assert!(matches!(cli.command, Command::Croncheck));
    }

    #[test]
    fn cli_parses_host_command() {
        let cli = Cli::try_parse_from(["greggd", "host", "127.0.0.1"]).unwrap();
        match cli.command {
            Command::Host { address } => {
                assert_eq!(address, "127.0.0.1".parse::<IpAddr>().unwrap());
            }
            _ => panic!("expected Host command"),
        }
    }

    #[test]
    fn cli_parses_port_command() {
        let cli = Cli::try_parse_from(["greggd", "port", "11320"]).unwrap();
        match cli.command {
            Command::Port { port } => assert_eq!(port, 11320),
            _ => panic!("expected Port command"),
        }
    }

    #[test]
    fn cli_parses_config_flag() {
        let cli = Cli::try_parse_from(["greggd", "--config", "/tmp/test.toml", "run"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/test.toml")));
    }

    // --- Config resolution tests ---

    #[test]
    fn resolve_config_path_explicit() {
        let explicit = PathBuf::from("/custom/path.toml");
        let resolved = resolve_config_path(Some(&explicit));
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn resolve_config_path_default() {
        let resolved = resolve_config_path(None);
        assert_eq!(resolved, Config::default_path());
    }

    #[test]
    fn load_config_from_existing_file() {
        let dir = std::env::temp_dir().join("greggd_test_cli_load");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let loaded = load_config(&path, true).unwrap();
        assert_eq!(config, loaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_explicit_missing_file_errors() {
        let path = PathBuf::from("/nonexistent/greggd.toml");
        let result = load_config(&path, true);
        assert!(result.is_err());
    }

    #[test]
    fn load_config_implicit_missing_file_uses_defaults() {
        let path = PathBuf::from("/nonexistent/greggd.toml");
        let config = load_config(&path, false).unwrap();
        assert_eq!(config, Config::default());
    }

    // --- Exit code tests ---

    #[test]
    fn exit_code_from_config_error() {
        let err = ConfigError::Io {
            path: PathBuf::from("test"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let code = ExitCode::from(&err);
        assert_eq!(code, ExitCode::ConfigError);
    }

    #[test]
    fn exit_code_from_permission_denied_io() {
        let err = ConfigError::Io {
            path: PathBuf::from("/etc/gregg/greggd.toml"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied"),
        };
        let code = ExitCode::from(&err);
        assert_eq!(code, ExitCode::PermissionDenied);
    }

    #[test]
    fn exit_code_from_permission_denied_atomic_write() {
        let err = ConfigError::AtomicWrite {
            path: PathBuf::from("/etc/gregg/greggd.toml"),
            source: crate::config::AtomicWriteError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "permission denied",
            )),
        };
        let code = ExitCode::from(&err);
        assert_eq!(code, ExitCode::PermissionDenied);
    }

    #[test]
    fn exit_code_from_service_error() {
        let err = ServiceError::CommandFailed {
            command: "test".into(),
            exit_status: Some(1),
            stderr: String::new(),
        };
        let code = ExitCode::from(&err);
        assert_eq!(code, ExitCode::ServiceError);
    }

    // --- Croncheck behavioral tests ---

    #[test]
    fn croncheck_active_does_nothing() {
        let service = FakeServiceManager::new();
        service.set_active(true);

        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_ok());

        let calls = service.calls();
        assert_eq!(calls, vec!["is_active"]);
        // Should NOT call start since service is already active.
    }

    #[test]
    fn croncheck_inactive_starts_service() {
        let service = FakeServiceManager::new();
        service.set_active(false);

        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_ok());

        let calls = service.calls();
        assert_eq!(calls, vec!["is_active", "start"]);
    }

    #[test]
    fn croncheck_error_returns_error() {
        let service = FakeServiceManager::new();
        service.set_is_active_error(ServiceError::StateQueryFailed {
            source: std::io::Error::other("query failed"),
        });

        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_err());

        let calls = service.calls();
        assert_eq!(calls, vec!["is_active"]);
        // Should NOT call start on error.
    }

    #[test]
    fn croncheck_active_with_noop_manager() {
        let service = NoopServiceManager;
        // NoopServiceManager::is_active() always returns false, so it will try to start.
        // But start() succeeds silently.
        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_ok());
    }

    // --- Start/stop/restart dispatch tests ---

    #[test]
    fn start_dispatch_calls_service_start() {
        let service = FakeServiceManager::new();
        let result = dispatch(&Command::Start, Path::new("/dev/null"), &service);
        assert!(result.is_ok());
        assert_eq!(service.calls(), vec!["start"]);
    }

    #[test]
    fn stop_dispatch_calls_service_stop() {
        let service = FakeServiceManager::new();
        let result = dispatch(&Command::Stop, Path::new("/dev/null"), &service);
        assert!(result.is_ok());
        assert_eq!(service.calls(), vec!["stop"]);
    }

    #[test]
    fn restart_dispatch_calls_service_restart() {
        let service = FakeServiceManager::new();
        let result = dispatch(&Command::Restart, Path::new("/dev/null"), &service);
        assert!(result.is_ok());
        assert_eq!(service.calls(), vec!["restart"]);
    }

    #[test]
    fn start_dispatch_error_returns_error() {
        let service = FakeServiceManager::new();
        service.set_start_error(ServiceError::CommandFailed {
            command: "systemctl start greggd".into(),
            exit_status: Some(1),
            stderr: "unit not found".into(),
        });

        let result = dispatch(&Command::Start, Path::new("/dev/null"), &service);
        assert!(result.is_err());
    }

    #[test]
    fn restart_dispatch_error_returns_error() {
        let service = FakeServiceManager::new();
        service.set_restart_error(ServiceError::CommandFailed {
            command: "systemctl restart greggd".into(),
            exit_status: Some(1),
            stderr: "unit not found".into(),
        });

        let result = dispatch(&Command::Restart, Path::new("/dev/null"), &service);
        assert!(result.is_err());
    }

    // --- Host/port mutation tests ---

    #[test]
    fn host_mutation_persists_and_restarts() {
        let dir = std::env::temp_dir().join("greggd_test_cli_host_mutate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // Write initial config.
        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let service = FakeServiceManager::new();
        let new_addr: IpAddr = "127.0.0.1".parse().unwrap();

        let result = dispatch(&Command::Host { address: new_addr }, &path, &service);
        assert!(result.is_ok());

        // Verify the file was updated.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.host, new_addr);

        // Verify restart was called.
        let calls = service.calls();
        assert!(calls.contains(&"restart"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_mutation_persists_and_restarts() {
        let dir = std::env::temp_dir().join("greggd_test_cli_port_mutate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // Write initial config.
        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let service = FakeServiceManager::new();

        let result = dispatch(&Command::Port { port: 11320 }, &path, &service);
        assert!(result.is_ok());

        // Verify the file was updated.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.port, 11320);

        // Verify restart was called.
        let calls = service.calls();
        assert!(calls.contains(&"restart"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_mutation_validates_before_persisting() {
        let dir = std::env::temp_dir().join("greggd_test_cli_host_validate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let service = FakeServiceManager::new();

        // Mutate to invalid state (empty name) — validation should fail.
        let result = mutate_and_restart(
            &path,
            true,
            |config| {
                config.name = String::new();
            },
            &service,
        );

        // Should fail due to validation.
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.downcast_ref::<ConfigValidationError>().is_some());

        // The original config should be unchanged.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.name, "greggd");

        // restart should NOT have been called.
        assert!(!service.calls().contains(&"restart"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mutation_does_not_restart_on_write_failure() {
        let dir = std::env::temp_dir().join("greggd_test_cli_no_restart");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let service = FakeServiceManager::new();

        // Try to write to a path that will fail (nonexistent parent).
        let result = mutate_and_restart(
            Path::new("/nonexistent_dir/config.toml"),
            true,
            |config| {
                config.port = 11320;
            },
            &service,
        );

        assert!(result.is_err());

        // restart should NOT have been called.
        let calls = service.calls();
        assert!(!calls.contains(&"restart"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mutation_restart_failure_returns_error() {
        let dir = std::env::temp_dir().join("greggd_test_cli_restart_fail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let service = FakeServiceManager::new();
        service.set_restart_error(ServiceError::CommandFailed {
            command: "systemctl restart greggd".into(),
            exit_status: Some(1),
            stderr: "failed".into(),
        });

        let result = dispatch(&Command::Port { port: 11320 }, &path, &service);

        assert!(result.is_err());

        // The file SHOULD have been written (persistence succeeded).
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.port, 11320);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Path-with-spaces test ---

    #[test]
    fn write_atomic_works_with_spaces_in_path() {
        let dir = std::env::temp_dir().join("greggd test with spaces");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config file.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_explicit_missing_file_errors_display() {
        let path = PathBuf::from("/nonexistent/greggd.toml");
        let result = load_config(&path, true);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("configuration file not found"));
    }
}
