//! CLI argument parsing and subcommand dispatch for `greggd`.
//!
//! Uses `clap` derive macros for structured argument parsing. Each
//! subcommand has a stable help message and returns a meaningful exit code.

use std::fmt;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
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
    /// Stop a running greggd instance via the local Unix control socket
    /// (Linux/macOS) or via the Windows Service Control Manager (Windows).
    Stop,
    /// Start the greggd Windows service.
    #[cfg(target_os = "windows")]
    Start,
    /// Restart the greggd Windows service.
    #[cfg(target_os = "windows")]
    Restart,
    /// Ensure greggd is running. Probes the configured local health endpoint and,
    /// if the endpoint is definitely refused, spawns `greggd run` as a detached child.
    /// Intended for cron, Task Scheduler, and other operator-managed
    /// supervisors that have no built-in readiness monitoring.
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
    match std::fs::metadata(path) {
        Ok(_) => Config::load(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if explicit {
                Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source: error,
                })
            } else {
                // No explicit path and file doesn't exist — use defaults.
                Ok(Config::default())
            }
        }
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
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

/// Map wildcard bind addresses to local loopback addresses for probing.
#[must_use]
pub fn probe_address(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(value) if value.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(value) if value.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        value => value,
    }
}

/// Derive the local probe target from daemon configuration.
#[must_use]
pub fn croncheck_target(config: &Config) -> SocketAddr {
    SocketAddr::new(probe_address(config.host), config.port)
}

/// Return the configured bind address in canonical socket-address form.
#[must_use]
pub fn config_address(config: &Config) -> SocketAddr {
    SocketAddr::new(config.host, config.port)
}

/// Return the bind address for `configprint`, resolving wildcards to
/// the host's primary local IP so the output is a usable address a
/// remote client can dial.
///
/// A specific host is returned unchanged. A `0.0.0.0` wildcard is
/// resolved to the local IPv4 address and an `::` wildcard to the
/// local IPv6 address (with an IPv4 fallback). If the local interface
/// cannot be resolved, the wildcard is preserved verbatim so the
/// output is still a valid socket address.
#[must_use]
pub fn display_address(config: &Config) -> SocketAddr {
    display_address_from(
        config,
        crate::net::local_ipv4_address,
        crate::net::local_ipv6_address,
    )
}

/// Same as [`display_address`] but with explicit local-address probes
/// for testability. The probes are plain `fn() -> Option<IpAddr>`
/// callbacks so callers can inject deterministic addresses without
/// touching the kernel's routing table.
#[must_use]
#[allow(clippy::module_name_repetitions)]
pub fn display_address_from(
    config: &Config,
    local_ipv4: fn() -> Option<IpAddr>,
    local_ipv6: fn() -> Option<IpAddr>,
) -> SocketAddr {
    let host = match config.host {
        IpAddr::V4(value) if value.is_unspecified() => local_ipv4().unwrap_or(config.host),
        IpAddr::V6(value) if value.is_unspecified() => {
            local_ipv6().or_else(local_ipv4).unwrap_or(config.host)
        }
        other => other,
    };
    SocketAddr::new(host, config.port)
}

/// Bounded TCP-connect check used by `croncheck`.
///
/// Returns `true` if a listener accepts the connection within the
/// timeout, `false` otherwise. A refusal, timeout, or unreachable host
/// all mean the daemon is not accepting traffic on this address.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CroncheckProbe {
    Running,
    Absent,
    Ambiguous,
}

const CRONCHECK_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_CRONCHECK_RESPONSE_BYTES: usize = 256 * 1024;

fn parse_greggd_health(response: &[u8]) -> bool {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = &response[..header_end];
    let body = &response[header_end + 4..];
    let Some(status_line) = headers.split(|byte| *byte == 10).next() else {
        return false;
    };
    let mut status_parts = status_line.split(|byte| *byte == 32 || *byte == 13);
    let Some(version) = status_parts.next() else {
        return false;
    };
    let Some(status) = status_parts
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u16>().ok())
    else {
        return false;
    };
    if version != b"HTTP/1.0" && version != b"HTTP/1.1" {
        return false;
    }
    let Ok(health) = serde_json::from_slice::<gregg_protocol::v2::HealthResponseV2>(body) else {
        return false;
    };
    matches!(
        (status, health.state),
        (200, gregg_protocol::ReadinessState::Ready)
            | (
                503,
                gregg_protocol::ReadinessState::Warming | gregg_protocol::ReadinessState::Failed
            )
    )
}

fn probe_greggd(target: SocketAddr) -> CroncheckProbe {
    let mut stream = match TcpStream::connect_timeout(&target, CRONCHECK_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return CroncheckProbe::Absent
        }
        Err(_) => return CroncheckProbe::Ambiguous,
    };
    let _ = stream.set_read_timeout(Some(CRONCHECK_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CRONCHECK_TIMEOUT));
    if stream
        .write_all(b"GET /v2/healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return CroncheckProbe::Ambiguous;
    }
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                if response.len() > MAX_CRONCHECK_RESPONSE_BYTES {
                    return CroncheckProbe::Ambiguous;
                }
            }
            Err(_) => return CroncheckProbe::Ambiguous,
        }
    }
    if parse_greggd_health(&response) {
        CroncheckProbe::Running
    } else {
        CroncheckProbe::Ambiguous
    }
}

/// Build the [`Command`] used by `croncheck` to spawn `greggd run` as a
/// detached watchdog child. Stdio is closed; the daemon's own logging is
/// independent of croncheck's. On Unix the child is placed in a new
/// process group so signals sent to croncheck's group (for example
/// SIGHUP from a closing terminal) do not reach the daemon.
///
/// Exposed crate-internally so tests can inspect `program()` and `get_args()`
/// without actually forking. The caller is responsible for `.spawn()`.
fn build_daemon_command(
    config_path: &std::path::Path,
    explicit: bool,
) -> std::io::Result<ProcessCommand> {
    let exe = std::env::current_exe()?;
    let mut cmd = ProcessCommand::new(exe);
    cmd.arg("run");
    if explicit {
        cmd.arg("--config").arg(config_path);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    Ok(cmd)
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
    let explicit = !matches!(
        std::fs::metadata(config_path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    dispatch_with_config_intent(command, config_path, explicit)
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
        Command::Stop => {
            // Unix uses the local control socket and is dispatched at the
            // binary boundary (see main.rs) so it can return errors as the
            // runtime/library boundary without a global tracing init.
            #[cfg(unix)]
            {
                unreachable!("Command::Stop is handled at the binary boundary on Unix")
            }
            #[cfg(not(unix))]
            {
                unreachable!("Command::Stop is handled at the binary boundary on Windows")
            }
        }
        Command::Croncheck => {
            let config = load_config(config_path, explicit)?;
            let target = croncheck_target(&config);
            match probe_greggd(target) {
                CroncheckProbe::Running => Ok(()),
                CroncheckProbe::Absent => {
                    build_daemon_command(config_path, explicit)?.spawn()?;
                    Ok(())
                }
                CroncheckProbe::Ambiguous => Err(Box::new(std::io::Error::other(
                    "croncheck could not prove greggd is absent or healthy",
                ))),
            }
        }
        Command::Configprint => {
            let config = load_config(config_path, explicit)?;
            println!("{}", display_address(&config));
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
        Command::Start | Command::Restart => {
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

    #[test]
    fn parser_accepts_run_stop_croncheck_mutations_and_version_but_not_windows_lifecycle() {
        for args in ["run", "stop", "croncheck", "configprint", "version"] {
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
        // `croncheck` no longer takes a `--target` flag: it operates on
        // the configured local bind only.
        assert!(
            Cli::try_parse_from(["greggd", "croncheck", "--target", "192.168.182.143:11310"])
                .is_err()
        );
        for command in ["start", "restart"] {
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
    fn display_address_preserves_specific_hosts() {
        let config = Config {
            host: "192.168.182.143".parse().unwrap(),
            port: 11310,
            ..Config::default()
        };
        assert_eq!(
            display_address_from(&config, || None, || None).to_string(),
            "192.168.182.143:11310"
        );
        let config = Config {
            host: "fd00::10".parse().unwrap(),
            port: 11320,
            ..Config::default()
        };
        assert_eq!(
            display_address_from(&config, || None, || None).to_string(),
            "[fd00::10]:11320"
        );
    }

    #[test]
    fn display_address_resolves_ipv4_wildcard_to_local_ipv4() {
        let config = Config {
            host: "0.0.0.0".parse().unwrap(),
            ..Config::default()
        };
        let resolved = display_address_from(
            &config,
            || Some("192.168.182.143".parse().unwrap()),
            || None,
        );
        assert_eq!(resolved.to_string(), "192.168.182.143:11310");
    }

    #[test]
    fn display_address_resolves_ipv6_wildcard_to_local_ipv6() {
        let config = Config {
            host: "::".parse().unwrap(),
            ..Config::default()
        };
        let resolved = display_address_from(&config, || None, || Some("fd00::10".parse().unwrap()));
        assert_eq!(resolved.to_string(), "[fd00::10]:11310");
    }

    #[test]
    fn display_address_falls_back_from_ipv6_to_ipv4_wildcard() {
        let config = Config {
            host: "::".parse().unwrap(),
            ..Config::default()
        };
        let resolved = display_address_from(
            &config,
            || Some("192.168.182.143".parse().unwrap()),
            || None,
        );
        assert_eq!(resolved.to_string(), "192.168.182.143:11310");
    }

    #[test]
    fn display_address_preserves_wildcard_when_no_local_address() {
        let config = Config {
            host: "0.0.0.0".parse().unwrap(),
            ..Config::default()
        };
        let resolved = display_address_from(&config, || None, || None);
        assert_eq!(resolved.to_string(), "0.0.0.0:11310");

        let config = Config {
            host: "::".parse().unwrap(),
            ..Config::default()
        };
        let resolved = display_address_from(&config, || None, || None);
        assert_eq!(resolved.to_string(), "[::]:11310");
    }

    #[test]
    fn display_address_prefers_configured_loopback_over_resolved_address() {
        // 127.0.0.1 is loopback but not unspecified, so it must pass
        // through unchanged; resolution only applies to wildcards.
        let config = Config {
            host: "127.0.0.1".parse().unwrap(),
            ..Config::default()
        };
        let resolved = display_address_from(
            &config,
            || Some("192.168.182.143".parse().unwrap()),
            || None,
        );
        assert_eq!(resolved.to_string(), "127.0.0.1:11310");
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
    fn load_config_propagates_non_not_found_metadata_errors() {
        let dir = std::env::temp_dir().join(format!("greggd-config-error-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let path = blocker.join("config.toml");

        let result = load_config(&path, false);
        assert!(matches!(
            result,
            Err(ConfigError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotADirectory
        ));
        let _ = std::fs::remove_dir_all(dir);
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

    fn http_fixture(status: u16, body: &str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap();
        let body = body.to_string();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                let response = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        target
    }

    fn silent_fixture() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        target
    }

    fn unbound_loopback() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    }

    #[test]
    fn croncheck_accepts_a_warming_health_response() {
        let target = http_fixture(
            503,
            r#"{"schema_version":2,"state":"warming","category":"warming","message":"warming"}"#,
        );
        assert_eq!(probe_greggd(target), CroncheckProbe::Running);
    }

    #[test]
    fn croncheck_refuses_a_closed_port() {
        let target = unbound_loopback();
        assert_eq!(probe_greggd(target), CroncheckProbe::Absent);
    }

    #[test]
    fn croncheck_accepts_a_failed_health_response() {
        let target = http_fixture(
            503,
            r#"{"schema_version":2,"state":"failed","category":"collector_failure","message":"failed"}"#,
        );
        assert_eq!(probe_greggd(target), CroncheckProbe::Running);
    }

    #[test]
    fn croncheck_rejects_unrelated_http() {
        let target = http_fixture(200, r#"{"ok":true}"#);
        assert_eq!(probe_greggd(target), CroncheckProbe::Ambiguous);
    }

    #[test]
    fn croncheck_rejects_malformed_health() {
        let target = http_fixture(200, "not-json");
        assert_eq!(probe_greggd(target), CroncheckProbe::Ambiguous);
    }

    #[test]
    fn croncheck_rejects_silent_peer_within_bound() {
        let target = silent_fixture();
        assert_eq!(probe_greggd(target), CroncheckProbe::Ambiguous);
    }

    #[test]
    fn croncheck_dispatch_exits_when_greggd_is_running_without_spawning() {
        // A valid non-ready Gregg health response still proves the daemon
        // exists; croncheck must not start a second copy.
        let target = http_fixture(
            503,
            r#"{"schema_version":2,"state":"failed","category":"collector_failure","message":"failed"}"#,
        );
        let dir = std::env::temp_dir().join("greggd_croncheck_listener_up_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("greggd.toml");
        std::fs::write(
            &path,
            format!(
                "name = \"loopback-croncheck-test\"\n\
                 host = \"127.0.0.1\"\n\
                 port = {}\n\
                 sample_interval_ms = 1000\n\
                 stale_after_ms = 10000\n",
                target.port()
            ),
        )
        .unwrap();
        dispatch_with_config_intent(&Command::Croncheck, &path, true).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_daemon_command_includes_run_and_explicit_config() {
        let dir = std::env::temp_dir().join("greggd_build_daemon_explicit_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let cmd = build_daemon_command(&path, true).unwrap();
        assert_eq!(
            cmd.get_program(),
            std::env::current_exe().unwrap().as_os_str()
        );
        let args: Vec<std::ffi::OsString> =
            cmd.get_args().map(std::ffi::OsStr::to_os_string).collect();
        assert_eq!(
            args,
            vec![
                std::ffi::OsString::from("run"),
                std::ffi::OsString::from("--config"),
                path.as_os_str().to_os_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_daemon_command_omits_config_when_implicit() {
        let cmd = build_daemon_command(std::path::Path::new("/nonexistent.toml"), false).unwrap();
        assert_eq!(
            cmd.get_program(),
            std::env::current_exe().unwrap().as_os_str()
        );
        let args: Vec<std::ffi::OsString> =
            cmd.get_args().map(std::ffi::OsStr::to_os_string).collect();
        assert_eq!(args, vec![std::ffi::OsString::from("run")]);
    }
}
