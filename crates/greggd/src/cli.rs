//! CLI argument parsing and subcommand dispatch for `greggd`.
//!
//! Uses `clap` derive macros for structured argument parsing. Each
//! subcommand has a stable help message and returns a meaningful exit code.

use std::fmt;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::config::{Config, ConfigError};
#[cfg(target_os = "windows")]
use crate::service::ServiceError;

/// Lightweight metrics daemon for the gregg monitoring system.
#[derive(Parser)]
#[command(
    name = "greggd",
    version,
    about = "Lightweight Linux, macOS, and Windows metrics daemon",
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
    /// Run the daemon in the foreground.
    Run,
    /// Start the greggd Windows service.
    #[cfg(target_os = "windows")]
    Start,
    /// Stop the greggd Windows service.
    #[cfg(target_os = "windows")]
    Stop,
    /// Restart the greggd Windows service.
    #[cfg(target_os = "windows")]
    Restart,
    /// Probe the daemon health endpoint without changing process state.
    Croncheck,
    /// Print the configured bind address without probing or mutating state.
    Configprint,
    /// Update the bind address (applies on the next daemon start).
    Host {
        /// The new IPv4 or IPv6 address to bind to.
        address: IpAddr,
    },
    /// Update the TCP port (applies on the next daemon start).
    Port {
        /// The new port number (1-65535).
        port: u16,
    },
    /// Print the binary version.
    Version,
    /// Internal: Windows SCM service entry point. Not for interactive use.
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    Service,
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

#[cfg(target_os = "windows")]
impl From<&ServiceError> for ExitCode {
    fn from(e: &ServiceError) -> Self {
        match e {
            ServiceError::CommandFailed { .. }
            | ServiceError::ExecFailed { .. }
            | ServiceError::NotAvailable { .. }
            | ServiceError::StateQueryFailed { .. }
            | ServiceError::Timeout { .. } => Self::ServiceError,
            ServiceError::AccessDenied => Self::PermissionDenied,
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
pub fn mutate_config(
    path: &std::path::Path,
    explicit: bool,
    mutate: impl FnOnce(&mut Config),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = load_config(path, explicit)?;
    mutate(&mut config);

    let violations = config.validate();
    if !violations.is_empty() {
        return Err(Box::new(ConfigValidationError(violations)));
    }

    config.write_atomic(path)?;

    Ok(())
}

/// Return the compile-time version rendered for the daemon binary.
#[must_use]
pub fn version_string() -> String {
    format!("greggd {}", env!("CARGO_PKG_VERSION"))
}

/// Error returned by the bounded local health probe.
#[derive(Debug)]
pub struct CroncheckError(String);

impl std::fmt::Display for CroncheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CroncheckError {}

/// Map wildcard bind addresses to local loopback addresses for probing.
#[must_use]
pub fn probe_address(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(value) if value.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(value) if value.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        value => value,
    }
}

/// Derive the local health-probe target from daemon configuration.
#[must_use]
pub fn croncheck_target(config: &Config) -> SocketAddr {
    SocketAddr::new(probe_address(config.host), config.port)
}

/// Return the configured bind address in canonical socket-address form.
#[must_use]
pub fn config_address(config: &Config) -> SocketAddr {
    SocketAddr::new(config.host, config.port)
}

/// Probe `/v2/healthz`, accepting only a syntactically valid HTTP 200 status.
pub fn probe_health(target: SocketAddr) -> Result<(), CroncheckError> {
    const TIMEOUT: Duration = Duration::from_millis(750);
    const MAX_STATUS_LINE_BYTES: usize = 512;
    let mut stream = TcpStream::connect_timeout(&target, TIMEOUT)
        .map_err(|e| CroncheckError(format!("health probe connection failed: {e}")))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|e| CroncheckError(format!("health probe timeout setup failed: {e}")))?;
    stream
        .write_all(b"GET /v2/healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| CroncheckError(format!("health probe request failed: {e}")))?;

    let mut response = [0_u8; MAX_STATUS_LINE_BYTES];
    let mut length = 0;
    let line_length = loop {
        if length == response.len() {
            return Err(CroncheckError(
                "health probe response status line too long".into(),
            ));
        }
        let read = stream
            .read(&mut response[length..])
            .map_err(|e| CroncheckError(format!("health probe response failed: {e}")))?;
        if read == 0 {
            return Err(CroncheckError(
                "health probe response ended before status line CRLF".into(),
            ));
        }
        length += read;
        if let Some(end) = response[..length]
            .windows(2)
            .position(|window| window == b"\r\n")
        {
            break end;
        }
    };

    let line = std::str::from_utf8(&response[..line_length])
        .map_err(|_| CroncheckError("health probe response status line is not text".into()))?;
    let mut fields = line.split_ascii_whitespace();
    let http = fields.next();
    let status = fields.next();
    let valid_version = matches!(http, Some("HTTP/1.0" | "HTTP/1.1"));
    let valid_status = status.is_some_and(|value| {
        value.len() == 3 && value.as_bytes().iter().all(u8::is_ascii_digit) && value == "200"
    });
    if valid_version && valid_status {
        Ok(())
    } else {
        Err(CroncheckError(format!(
            "health probe returned malformed or unhealthy status: {line}"
        )))
    }
}

/// Dispatch a subcommand using the path's current existence as a compatibility
/// fallback. The binary entry point uses [`dispatch_with_config_intent`] so a
/// missing explicit path remains distinguishable from a missing default path.
///
/// # Errors
///
/// Returns an error if the command fails.
pub fn dispatch(
    command: &Command,
    config_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_with_config_intent(command, config_path, config_path.exists())
}

/// Dispatch a command while preserving whether the config path was explicit.
pub fn dispatch_with_config_intent(
    command: &Command,
    config_path: &std::path::Path,
    explicit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Run => {
            // Delegate to the async run entry point.
            // This is handled in main.rs.
            unreachable!("Command::Run is handled in main.rs")
        }
        Command::Croncheck => {
            let config = load_config(config_path, explicit)?;
            probe_health(croncheck_target(&config))?;
            println!("greggd healthy");
            Ok(())
        }
        Command::Configprint => {
            let config = load_config(config_path, explicit)?;
            println!("{}", config_address(&config));
            Ok(())
        }
        Command::Host { address } => mutate_config(config_path, explicit, |config| {
            config.host = *address;
        }),
        Command::Port { port } => mutate_config(config_path, explicit, |config| {
            config.port = *port;
        }),
        Command::Version => {
            println!("{}", version_string());
            Ok(())
        }
        #[cfg(target_os = "windows")]
        Command::Start | Command::Stop | Command::Restart => {
            unreachable!("Windows service commands are dispatched at the binary boundary")
        }
        #[cfg(target_os = "windows")]
        Command::Service => {
            unreachable!("Command::Service is handled in main.rs")
        }
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod native_tests {
    use super::*;
    use clap::Parser;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parser_accepts_run_croncheck_mutations_and_version_but_not_windows_lifecycle() {
        for args in ["run", "croncheck", "configprint", "version"] {
            let argv = if args == "host" {
                vec!["greggd", "host", "127.0.0.1"]
            } else if args == "port" {
                vec!["greggd", "port", "11310"]
            } else {
                vec!["greggd", args]
            };
            assert!(Cli::try_parse_from(argv).is_ok(), "failed to parse {args}");
        }
        assert!(Cli::try_parse_from(["greggd", "host", "127.0.0.1"]).is_ok());
        assert!(Cli::try_parse_from(["greggd", "port", "11310"]).is_ok());
        for command in ["start", "stop", "restart"] {
            assert!(Cli::try_parse_from(["greggd", command]).is_err());
        }
    }

    #[test]
    fn wildcard_probe_addresses_use_loopback() {
        assert_eq!(
            probe_address("0.0.0.0".parse::<IpAddr>().unwrap()),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            probe_address("::".parse::<IpAddr>().unwrap()),
            "::1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            probe_address("192.0.2.1".parse::<IpAddr>().unwrap()),
            "192.0.2.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn config_address_preserves_wildcards_and_formats_ipv6() {
        let mut config = Config::default();
        assert_eq!(config_address(&config).to_string(), "0.0.0.0:11310");
        config.host = "fd00::10".parse().unwrap();
        config.port = 11320;
        assert_eq!(config_address(&config).to_string(), "[fd00::10]:11320");
    }

    #[test]
    fn configprint_uses_default_for_missing_implicit_config() {
        let path =
            std::env::temp_dir().join(format!("greggd-configprint-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        dispatch_with_config_intent(&Command::Configprint, &path, false).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn configprint_rejects_missing_explicit_config() {
        let path = std::env::temp_dir().join(format!(
            "greggd-configprint-missing-{}.toml",
            std::process::id(),
        ));
        let _ = std::fs::remove_file(&path);
        assert!(dispatch_with_config_intent(&Command::Configprint, &path, true).is_err());
    }

    #[test]
    fn version_string_uses_package_version() {
        assert_eq!(
            version_string(),
            format!("greggd {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn config_mutation_is_persisted_without_service_dispatch() {
        let dir = std::env::temp_dir().join("greggd_native_mutation_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        mutate_config(&path, false, |config| config.port = 11320).unwrap();
        assert_eq!(Config::load(&path).unwrap().port, 11320);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn probe_against(response: impl Into<Vec<u8>>) -> Result<(), CroncheckError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap();
        let response = response.into();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 256];
            let _ = std::io::Read::read(&mut stream, &mut request);
            stream.write_all(&response).unwrap();
        });
        let result = probe_health(target);
        worker.join().unwrap();
        result
    }

    #[test]
    fn health_probe_accepts_only_http_200() {
        assert!(probe_against(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").is_ok());
        assert!(probe_against(b"HTTP/1.0 200 OK\r\n\r\n").is_ok());
        assert!(probe_against(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").is_err());
        assert!(probe_against(b"not HTTP\r\n").is_err());
    }

    #[test]
    fn health_probe_rejects_premature_eof() {
        assert!(probe_against(b"HTTP/1.1 200 OK").is_err());
    }

    #[test]
    fn health_probe_rejects_overlong_status_line() {
        let mut response = vec![b'X'; 512];
        response.extend_from_slice(b"\r\n");
        assert!(probe_against(response).is_err());
    }

    #[test]
    fn health_probe_rejects_invalid_http_version() {
        assert!(probe_against(b"HTTP/1.xyz 200 OK\r\n").is_err());
    }

    #[test]
    fn health_probe_rejects_closed_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap();
        drop(listener);

        assert!(probe_health(target).is_err());
    }
}
