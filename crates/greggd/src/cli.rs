//! CLI argument parsing and subcommand dispatch for `greggd`.
//!
//! Uses `clap` derive macros for structured argument parsing. Each
//! subcommand has a stable help message and returns a meaningful exit code.

use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::config::{Config, ConfigError};
use crate::service::{platform_service_manager, ServiceError};

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
    fn from(_: &ConfigError) -> Self {
        Self::ConfigError
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

/// Update a single field in the config and atomically persist it.
///
/// This is the shared logic for `host` and `port` subcommands.
pub fn mutate_and_restart(
    path: &std::path::Path,
    explicit: bool,
    mutate: impl FnOnce(&mut Config),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = load_config(path, explicit)?;
    mutate(&mut config);

    let violations = config.validate();
    if !violations.is_empty() {
        eprintln!("configuration validation failed:");
        for v in &violations {
            eprintln!("  - {v}");
        }
        std::process::exit(ExitCode::ConfigError as i32);
    }

    config.write_atomic(path)?;

    let service = platform_service_manager();
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
) -> Result<(), Box<dyn std::error::Error>> {
    let explicit = true; // resolve_config_path already determined this

    match command {
        Command::Run => {
            // Delegate to the async run entry point.
            // This is handled in main.rs.
            unreachable!("Command::Run is handled in main.rs")
        }
        Command::Start => {
            let service = platform_service_manager();
            service.start().map_err(|e| {
                eprintln!("failed to start service: {e}");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            Ok(())
        }
        Command::Stop => {
            let service = platform_service_manager();
            service.stop().map_err(|e| {
                eprintln!("failed to stop service: {e}");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            Ok(())
        }
        Command::Restart => {
            let service = platform_service_manager();
            service.restart().map_err(|e| {
                eprintln!("failed to restart service: {e}");
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            Ok(())
        }
        Command::Croncheck => {
            let service = platform_service_manager();
            match service.is_active() {
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
            }
        }
        Command::Host { address } => mutate_and_restart(config_path, explicit, |config| {
            config.host = *address;
        }),
        Command::Port { port } => mutate_and_restart(config_path, explicit, |config| {
            config.port = *port;
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn exit_code_from_config_error() {
        let err = ConfigError::Io {
            path: PathBuf::from("test"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        let code = ExitCode::from(&err);
        assert!(matches!(code, ExitCode::ConfigError));
    }

    #[test]
    fn exit_code_from_service_error() {
        let err = ServiceError::CommandFailed {
            command: "test".into(),
            exit_status: Some(1),
            stderr: String::new(),
        };
        let code = ExitCode::from(&err);
        assert!(matches!(code, ExitCode::ServiceError));
    }
}
