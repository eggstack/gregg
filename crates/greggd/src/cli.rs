//! CLI argument parsing and subcommand dispatch for `greggd`.
//!
//! Uses `clap` derive macros for structured argument parsing. Each
//! subcommand has a stable help message and returns a meaningful exit code.

use std::fmt;
use std::io::{BufRead, BufReader, Write};
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

/// Probe `/v2/healthz`, accepting only a syntactically valid HTTP 200 status.
pub fn probe_health(target: SocketAddr) -> Result<(), CroncheckError> {
    const TIMEOUT: Duration = Duration::from_millis(750);
    let mut stream = TcpStream::connect_timeout(&target, TIMEOUT)
        .map_err(|e| CroncheckError(format!("health probe connection failed: {e}")))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|e| CroncheckError(format!("health probe timeout setup failed: {e}")))?;
    stream
        .write_all(b"GET /v2/healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| CroncheckError(format!("health probe request failed: {e}")))?;

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader
        .read_line(&mut line)
        .map_err(|e| CroncheckError(format!("health probe response failed: {e}")))?;
    let mut fields = line.split_whitespace();
    let http = fields.next();
    let status = fields.next().and_then(|value| value.parse::<u16>().ok());
    if http.is_some_and(|value| value.starts_with("HTTP/1.")) && status == Some(200) {
        Ok(())
    } else {
        Err(CroncheckError(format!(
            "health probe returned malformed or unhealthy status: {}",
            line.trim()
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
        for args in ["run", "croncheck", "version"] {
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

    fn probe_against(response: &'static [u8]) -> Result<(), CroncheckError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 256];
            let _ = std::io::Read::read(&mut stream, &mut request);
            stream.write_all(response).unwrap();
        });
        let result = probe_health(target);
        worker.join().unwrap();
        result
    }

    #[test]
    fn health_probe_accepts_only_http_200() {
        assert!(probe_against(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").is_ok());
        assert!(probe_against(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").is_err());
        assert!(probe_against(b"not HTTP\r\n").is_err());
    }
}

#[cfg(all(test, any()))]
mod tests {
    use super::*;
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
    fn cli_parses_service_command() {
        let cli = Cli::try_parse_from(["greggd", "service"]).unwrap();
        assert!(matches!(cli.command, Command::Service));
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
        let service = FakeServiceManager::new();
        // FakeServiceManager defaults to inactive, so croncheck will try to start.
        // start() succeeds silently by default.
        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_ok());
        assert_eq!(service.calls(), vec!["is_active", "start"]);
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
    fn implicit_missing_config_starts_from_defaults() {
        let dir = std::env::temp_dir().join("greggd_test_cli_implicit_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let service = FakeServiceManager::new();

        dispatch_with_config_intent(&Command::Port { port: 11320 }, &path, false, &service)
            .unwrap();

        assert_eq!(Config::load(&path).unwrap().port, 11320);
        assert_eq!(service.calls(), vec!["restart"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_missing_config_does_not_write_or_restart() {
        let dir = std::env::temp_dir().join("greggd_test_cli_explicit_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let service = FakeServiceManager::new();

        let result = dispatch_with_config_intent(
            &Command::Host {
                address: "127.0.0.1".parse().unwrap(),
            },
            &path,
            true,
            &service,
        );

        assert!(result.is_err());
        assert!(!path.exists());
        assert!(service.calls().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    // --- Service restart loop protection tests ---

    #[test]
    fn croncheck_with_failing_start_returns_error_without_looping() {
        // When the service is inactive and start fails, croncheck must
        // return an error after a single start attempt, not loop.
        let service = FakeServiceManager::new();
        service.set_active(false);
        service.set_start_error(ServiceError::CommandFailed {
            command: "systemctl start greggd".into(),
            exit_status: Some(1),
            stderr: "invalid config".into(),
        });

        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_err());

        // Exactly one is_active + one start call, no loop.
        let calls = service.calls();
        assert_eq!(calls, vec!["is_active", "start"]);
    }

    #[test]
    fn repeated_croncheck_calls_each_make_single_start_attempt() {
        // Simulate a cron daemon calling croncheck every minute while the
        // service is inactive and start keeps failing. Each invocation
        // should be independent: one is_active + one start per call.
        for i in 0..5 {
            let service = FakeServiceManager::new();
            service.set_active(false);
            service.set_start_error(ServiceError::CommandFailed {
                command: "systemctl start greggd".into(),
                exit_status: Some(1),
                stderr: format!("attempt {i}"),
            });

            let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
            assert!(result.is_err(), "iteration {i} should fail");

            let calls = service.calls();
            assert_eq!(
                calls,
                vec!["is_active", "start"],
                "iteration {i} should have exactly 2 calls"
            );
        }
    }

    #[test]
    fn croncheck_start_success_sets_active_and_does_not_restart() {
        // When croncheck starts the service successfully, it should not
        // call restart. Only start should be called.
        let service = FakeServiceManager::new();
        service.set_active(false);

        let result = dispatch(&Command::Croncheck, Path::new("/dev/null"), &service);
        assert!(result.is_ok());

        let calls = service.calls();
        assert_eq!(calls, vec!["is_active", "start"]);
        assert!(!calls.contains(&"restart"));
    }
}
