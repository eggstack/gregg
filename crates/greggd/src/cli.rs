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

use clap::{Parser, Subcommand, ValueEnum};

use crate::config::{Config, ConfigError};
#[cfg(any(target_os = "windows", test))]
use crate::service::ServiceError;
use crate::startup::StartupMethodArg;

mod job_selector;

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
    /// Restart the daemon through its detected startup manager.
    #[allow(clippy::doc_markdown)]
    Restart,
    /// Manage automatic startup (systemd, launchd, or cron).
    Startup {
        #[command(subcommand)]
        command: StartupCommand,
    },
    /// Ensure greggd is running. Probes the configured local health endpoint and,
    /// if the endpoint is definitely refused, spawns `greggd run` as a detached child.
    /// Intended for cron, Task Scheduler, and other operator-managed
    /// supervisors that have no built-in readiness monitoring.
    Croncheck,
    /// Print the configured bind address without probing or mutating state.
    Configprint,
    /// Show read-only local diagnostic status: version, config path,
    /// configured bind address, local health classification, and detected
    /// startup-manager state. Never starts, stops, restarts, installs, or
    /// mutates configuration; exit 0 only when a valid Gregg health
    /// endpoint answered (ready, warming, or failed).
    Status,
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
    /// Manage local scheduled jobs in this config; changes apply on the next daemon start.
    Job {
        #[command(subcommand)]
        command: JobCommand,
    },
    /// Edit the complete daemon configuration transactionally.
    ///
    /// Resolves the editor from $VISUAL, $EDITOR, then platform fallbacks. The
    /// editor receives a private candidate file directly, never through a shell.
    /// Changes apply on the next daemon start; a running daemon is not restarted.
    Edit,
    /// Uninstall the greggd daemon binary and its Gregg-owned startup integration.
    ///
    /// Removes only the exact invoked executable; a sibling `gregg` binary
    /// sharing the install directory is never touched, and install
    /// directories are never removed recursively. Startup teardown removes
    /// only canonical Gregg artifacts actually present (systemd unit,
    /// launchd plist, managed cron block, or the `greggd` SCM
    /// registration). Configuration is preserved by default; `--purge`
    /// additionally removes the resolved daemon config file (and the macOS
    /// daemon log). `--dry-run` prints the exact resources that would be
    /// removed without mutating anything. There is no interactive prompt;
    /// invoking `uninstall` is the explicit destructive action.
    Uninstall {
        /// Print the exact resources that would be removed without
        /// stopping, mutating, or deleting anything.
        #[arg(long)]
        dry_run: bool,
        /// Also remove the resolved daemon config/data files
        /// (destructive). Without this flag configuration is preserved.
        #[arg(long)]
        purge: bool,
    },
    /// Print the binary version.
    Version,
    /// Update the daemon binary to the latest stable crates.io version.
    ///
    /// The updater uses crates.io as the version authority and downloads the
    /// exact tagged GitHub Release asset when available. Checksum and
    /// candidate `version` are verified before any replacement. If no
    /// prebuilt asset exists for the current host (HTTP 404), Cargo is used
    /// as a fallback when available. A checksum or version mismatch is a
    /// hard error and does not fall back to Cargo. After a successful
    /// replacement, the daemon is restarted through its detected manager
    /// when it was running (systemd, launchd, SCM, or direct cron); an
    /// intentionally stopped service remains stopped. No `sudo` is invoked
    /// internally; rerun with `sudo greggd update` when the install location
    /// requires it.
    Update,
    /// Internal: Windows SCM service entry point. Not for interactive use.
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    Service,
}

/// Scheduled-job management commands.
#[derive(Subcommand)]
pub enum JobCommand {
    /// Add one validated local scheduled command.
    Add {
        /// Stable operator-facing job name.
        #[arg(value_name = "NAME")]
        name: String,
        /// Five-field cron schedule in local civil time.
        #[arg(long, value_name = "SCHEDULE")]
        schedule: String,
        /// Working directory for the command.
        #[arg(long, value_name = "DIR")]
        working_dir: Option<PathBuf>,
        /// Maximum allowed cached load before the job is admitted.
        #[arg(long, value_name = "LOAD")]
        max_load: Option<f32>,
        /// Load averaging period: 1m, 5m, or 15m. Requires --max-load.
        #[arg(long, value_name = "WINDOW")]
        load_window: Option<String>,
        /// Delay between retries while the load gate is closed.
        #[arg(long, value_name = "MILLISECONDS")]
        retry_interval_ms: Option<u64>,
        /// Maximum time a due occurrence may wait for the load gate.
        #[arg(long, value_name = "MILLISECONDS")]
        max_wait_ms: Option<u64>,
        /// Executable and argv values; required after the -- separator.
        #[arg(last = true, required = true, num_args = 1.., allow_hyphen_values = true, value_name = "PROGRAM [ARGS...]")]
        command: Vec<String>,
    },
    /// List configured jobs and their argv values without changing the config.
    List,
    /// Remove one job by exact name, or select one interactively when omitted.
    Remove {
        /// Exact configured job name; omit to open the terminal selector.
        #[arg(value_name = "NAME")]
        name: Option<String>,
    },
}

/// Startup subcommands for automatic startup management.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum StartupCommand {
    /// Install and enable automatic startup for the detected or specified manager.
    Install {
        /// Startup manager to install. `auto` detects the platform default.
        #[arg(long, value_enum, default_value_t = StartupMethodArg::Auto, value_name = "METHOD")]
        method: StartupMethodArg,
    },
    /// Print instructions for enabling automatic startup without mutating state.
    Instructions {
        /// Startup manager to describe. `auto` detects the platform default.
        #[arg(long, value_enum, default_value_t = StartupMethodArg::Auto, value_name = "METHOD")]
        method: StartupMethodArg,
    },
}

impl ValueEnum for StartupMethodArg {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Auto, Self::Systemd, Self::Launchd, Self::Cron]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(match self {
            Self::Auto => clap::builder::PossibleValue::new("auto"),
            Self::Systemd => clap::builder::PossibleValue::new("systemd"),
            Self::Launchd => clap::builder::PossibleValue::new("launchd"),
            Self::Cron => clap::builder::PossibleValue::new("cron"),
        })
    }
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

#[cfg(any(target_os = "windows", test))]
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

impl From<&crate::startup::InstallError> for ExitCode {
    fn from(e: &crate::startup::InstallError) -> Self {
        match e {
            crate::startup::InstallError::Permission { .. } => Self::PermissionDenied,
            crate::startup::InstallError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                Self::PermissionDenied
            }
            crate::startup::InstallError::BinaryMissing { .. }
            | crate::startup::InstallError::UnsupportedMethod { .. } => Self::ConfigError,
            _ => Self::ServiceError,
        }
    }
}

impl From<&crate::uninstall::UninstallError> for ExitCode {
    fn from(e: &crate::uninstall::UninstallError) -> Self {
        match e {
            crate::uninstall::UninstallError::Permission { .. } => Self::PermissionDenied,
            crate::uninstall::UninstallError::Service { .. } => Self::ServiceError,
            crate::uninstall::UninstallError::CurrentExe(_)
            | crate::uninstall::UninstallError::Io { .. }
            | crate::uninstall::UninstallError::UncertainStop { .. }
            | crate::uninstall::UninstallError::Cron { .. }
            | crate::uninstall::UninstallError::CargoHandoff { .. }
            | crate::uninstall::UninstallError::CargoFailed(_) => Self::RuntimeError,
        }
    }
}

impl From<&crate::update::UpdateError> for ExitCode {
    fn from(e: &crate::update::UpdateError) -> Self {
        match e {
            crate::update::UpdateError::PermissionDenied { .. } => Self::PermissionDenied,
            _ => Self::RuntimeError,
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

/// A user-facing config command failure that is not a schema violation.
#[derive(Debug)]
pub struct JobCommandError(pub String);

impl fmt::Display for JobCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for JobCommandError {}

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
///
/// The load→mutate→validate→write sequence holds an exclusive advisory
/// lock on `<config>.lock` (same `<config>.lock` scheme as the `gregg`
/// client) so concurrent `greggd host`/`port` mutations cannot lose an
/// update despite the atomic rename.
pub fn mutate_config(
    path: &std::path::Path,
    explicit: bool,
    mutate: impl FnOnce(&mut Config),
) -> Result<(), Box<dyn std::error::Error>> {
    mutate_config_with_result(path, explicit, |config| {
        mutate(config);
        Ok(())
    })
}

fn mutate_config_with_result<T>(
    path: &std::path::Path,
    explicit: bool,
    mutate: impl FnOnce(&mut Config) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let _lock = acquire_config_lock(path);
    let mut config = load_config(path, explicit)?;
    let result = mutate(&mut config)?;

    let violations = config.validate();
    if !violations.is_empty() {
        return Err(Box::new(ConfigValidationError(violations)));
    }

    config.write_atomic(path)?;
    Ok(result)
}

fn dispatch_job_command(
    command: &JobCommand,
    config_path: &std::path::Path,
    explicit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        JobCommand::Add {
            name,
            schedule,
            working_dir,
            max_load,
            load_window,
            retry_interval_ms,
            max_wait_ms,
            command,
        } => {
            let job = crate::config::ScheduledJobConfig {
                name: name.clone(),
                schedule: schedule.clone(),
                command: command.clone(),
                working_dir: working_dir.clone(),
                max_load: *max_load,
                load_window: load_window.clone(),
                retry_interval_ms: *retry_interval_ms,
                max_wait_ms: *max_wait_ms,
            };
            mutate_config_with_result(config_path, explicit, |config| {
                if config.jobs.iter().any(|existing| existing.name == job.name) {
                    return Err(Box::new(JobCommandError(format!(
                        "scheduled job {job_name:?} already exists",
                        job_name = job.name
                    ))));
                }
                config.jobs.push(job);
                Ok(())
            })?;
            println!(
                "Added scheduled job {name:?} in {}. Changes take effect on the next daemon start; a running daemon was not restarted.",
                config_path.display()
            );
            Ok(())
        }
        JobCommand::List => {
            let config = load_config(config_path, explicit)?;
            print!("{}", render_job_list(config_path, &config));
            Ok(())
        }
        JobCommand::Remove { name: Some(name) } => {
            remove_named_job(config_path, explicit, name)?;
            println!(
                "Removed scheduled job {name:?} from {}. Changes take effect on the next daemon start; a running daemon was not restarted.",
                config_path.display()
            );
            Ok(())
        }
        JobCommand::Remove { name: None } => {
            let config = load_config(config_path, explicit)?;
            if config.jobs.is_empty() {
                println!("No scheduled jobs configured in {}.", config_path.display());
                return Ok(());
            }
            let Some(index) = job_selector::select(&config.jobs)? else {
                println!("Removal cancelled; configuration unchanged.");
                return Ok(());
            };
            let name = config.jobs[index].name.clone();
            remove_named_job(config_path, explicit, &name)?;
            println!(
                "Removed scheduled job {name:?} from {}. Changes take effect on the next daemon start; a running daemon was not restarted.",
                config_path.display()
            );
            Ok(())
        }
    }
}

fn remove_named_job(
    config_path: &std::path::Path,
    explicit: bool,
    name: &str,
) -> Result<crate::config::ScheduledJobConfig, Box<dyn std::error::Error>> {
    mutate_config_with_result(config_path, explicit, |config| {
        let Some(index) = config.jobs.iter().position(|job| job.name == name) else {
            return Err(Box::new(JobCommandError(format!(
                "no scheduled job named {name:?} is configured"
            ))));
        };
        Ok(config.jobs.remove(index))
    })
}

fn render_job_list(config_path: &std::path::Path, config: &Config) -> String {
    if config.jobs.is_empty() {
        return format!(
            "No scheduled jobs configured in {}.\n",
            config_path.display()
        );
    }
    let mut lines = vec![format!(
        "Scheduled jobs in {} (config order):",
        config_path.display()
    )];
    for job in &config.jobs {
        lines.push(format!("- {}", job.name));
        lines.push(format!("  schedule: {}", job.schedule));
        lines.push(format!("  command argv: {:?}", job.command));
        lines.push(format!(
            "  working directory: {}",
            job.working_dir
                .as_ref()
                .map_or_else(|| "(not set)".to_owned(), |path| path.display().to_string())
        ));
        if let Some(max_load) = job.max_load {
            lines.push(format!(
                "  load gate: max-load <= {max_load}, window {}, retry {} ms, max wait {} ms",
                job.effective_load_window(),
                job.effective_retry_interval_ms(),
                job.effective_max_wait_ms()
            ));
        } else {
            lines.push("  load gate: none".to_owned());
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn edit_config(
    config_path: &std::path::Path,
    explicit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let editor = resolve_editor().ok_or_else(|| {
        Box::new(ConfigError::EditorFailed {
            path: config_path.to_path_buf(),
            message: "no editor found; set $VISUAL or $EDITOR".to_owned(),
        }) as Box<dyn std::error::Error>
    })?;
    edit_config_with_editor(config_path, explicit, &editor)
}

fn edit_config_with_editor(
    config_path: &std::path::Path,
    explicit: bool,
    editor: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let _lock = acquire_config_lock(config_path);
    let config = load_config(config_path, explicit)?;
    config.edit_transaction(config_path, |candidate| {
        let status = ProcessCommand::new(editor)
            .arg(candidate)
            .status()
            .map_err(|error| ConfigError::EditorFailed {
                path: candidate.to_path_buf(),
                message: format!("failed to launch editor {editor:?}: {error}"),
            })?;
        if !status.success() {
            return Err(ConfigError::EditorFailed {
                path: candidate.to_path_buf(),
                message: format!("editor exited with status: {status}"),
            });
        }
        Ok(())
    })?;
    println!(
        "Updated daemon configuration at {}. Changes take effect on the next daemon start; a running daemon was not restarted.",
        config_path.display()
    );
    Ok(())
}

fn resolve_editor() -> Option<String> {
    for variable in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(variable) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    #[cfg(windows)]
    let fallbacks = ["hx", "code", "notepad"];
    #[cfg(not(windows))]
    let fallbacks = ["hx", "vim", "vi"];
    fallbacks
        .into_iter()
        .find(|candidate| executable_exists(candidate))
        .map(str::to_owned)
}

fn executable_exists(candidate: &str) -> bool {
    let candidate_path = std::path::Path::new(candidate);
    if candidate_path.is_absolute() || candidate_path.components().count() > 1 {
        return candidate_path.is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    #[cfg(windows)]
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    std::env::split_paths(&path).any(|directory| {
        #[cfg(windows)]
        {
            directory.join(candidate).is_file()
                || extensions
                    .split(';')
                    .any(|extension| directory.join(format!("{candidate}{extension}")).is_file())
        }
        #[cfg(not(windows))]
        {
            directory.join(candidate).is_file()
        }
    })
}

/// Advisory cross-process lock guard for daemon config mutation.
///
/// Holds the open lock file so the `flock` (Unix) is retained until the
/// guard drops. On non-Unix targets this is a no-op placeholder.
struct ConfigLockGuard {
    #[allow(dead_code)]
    file: Option<std::fs::File>,
}

/// Acquire `<config>.lock` around config mutation (best-effort, bounded
/// ~5s wait). A lock failure still returns a guard so the mutation can
/// proceed; the atomic rename guarantees no torn writes (only the
/// lost-update protection degrades).
#[cfg_attr(unix, allow(unsafe_code))]
fn acquire_config_lock(config_path: &std::path::Path) -> ConfigLockGuard {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let lock_path = {
            let mut s = config_path.as_os_str().to_owned();
            s.push(".lock");
            std::path::PathBuf::from(s)
        };
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path);
        match file {
            Ok(file) => {
                let fd = file.as_raw_fd();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    // SAFETY: `flock` is called with an owned open fd and
                    // constant flags; it does not touch Rust-managed memory.
                    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                    if rc == 0 {
                        return ConfigLockGuard { file: Some(file) };
                    }
                    let errno = std::io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO);
                    if errno != libc::EWOULDBLOCK && errno != libc::EAGAIN {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                ConfigLockGuard { file: Some(file) }
            }
            Err(_) => ConfigLockGuard { file: None },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = config_path;
        ConfigLockGuard { file: None }
    }
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

/// Total wall-clock bound for one bounded health fetch.
///
/// The per-`read` timeout alone is not a bound: a peer that dribbles one
/// byte per `CRONCHECK_TIMEOUT` keeps the read loop alive indefinitely.
/// This deadline is enforced across the whole exchange, so `status`,
/// `croncheck`, `update`, and `uninstall` probes all stay bounded.
const CRONCHECK_DEADLINE: Duration = Duration::from_secs(5);

/// Classify a raw `/v2/healthz` response body into its readiness state.
///
/// Returns `None` unless the response is a well-formed HTTP/1.x reply whose
/// status/JSON pair is a valid Gregg health response: `200` + `ready`, or
/// `503` + `warming`/`failed`. Anything else (wrong status, non-JSON body,
/// schema mismatch, truncated headers) is not a valid Gregg endpoint.
fn classify_health_response(response: &[u8]) -> Option<gregg_protocol::ReadinessState> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let headers = &response[..header_end];
    let body = &response[header_end + 4..];
    let status_line = headers.split(|byte| *byte == 10).next()?;
    let mut status_parts = status_line.split(|byte| *byte == 32 || *byte == 13);
    let version = status_parts.next()?;
    let status = status_parts
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u16>().ok())?;
    if version != b"HTTP/1.0" && version != b"HTTP/1.1" {
        return None;
    }
    let Ok(health) = serde_json::from_slice::<gregg_protocol::v2::HealthResponseV2>(body) else {
        return None;
    };
    match (status, health.state) {
        (200, gregg_protocol::ReadinessState::Ready) => Some(gregg_protocol::ReadinessState::Ready),
        (503, gregg_protocol::ReadinessState::Warming) => {
            Some(gregg_protocol::ReadinessState::Warming)
        }
        (503, gregg_protocol::ReadinessState::Failed) => {
            Some(gregg_protocol::ReadinessState::Failed)
        }
        _ => None,
    }
}

/// Raw outcome of the bounded local health fetch shared by `croncheck`,
/// restart/update coordination, and `status`.
#[derive(Debug, PartialEq, Eq)]
enum FetchOutcome {
    /// The listener definitely refused the connection: nothing accepts
    /// traffic on this address.
    Refused,
    /// No usable answer: timeout, unreachable host, write/read failure, or
    /// an over-cap response body.
    Failed,
    /// The peer answered within bounds; classification happens separately.
    Responded(Vec<u8>),
}

/// Perform the bounded raw-HTTP `GET /v2/healthz` fetch: finite
/// connection/read deadline, bounded response body, no service-manager
/// invocation, no mutation.
fn fetch_health_bytes(target: SocketAddr) -> FetchOutcome {
    let started = std::time::Instant::now();
    let mut stream = match TcpStream::connect_timeout(&target, CRONCHECK_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return FetchOutcome::Refused
        }
        Err(_) => return FetchOutcome::Failed,
    };
    let _ = stream.set_write_timeout(Some(CRONCHECK_TIMEOUT));
    if stream
        .write_all(b"GET /v2/healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return FetchOutcome::Failed;
    }
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        // Re-derive the per-read timeout from the total deadline on every
        // iteration so a slow-loris peer cannot stretch the exchange past
        // `CRONCHECK_DEADLINE` by trickling bytes just inside each read.
        let remaining = CRONCHECK_DEADLINE
            .checked_sub(started.elapsed())
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return FetchOutcome::Failed;
        }
        let _ = stream.set_read_timeout(Some(remaining.min(CRONCHECK_TIMEOUT)));
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                // Check before buffering so at most the cap itself is held.
                if response.len().saturating_add(read) > MAX_CRONCHECK_RESPONSE_BYTES {
                    return FetchOutcome::Failed;
                }
                response.extend_from_slice(&chunk[..read]);
            }
            Err(_) => return FetchOutcome::Failed,
        }
    }
    FetchOutcome::Responded(response)
}

/// Detailed local health classification used by `greggd status`.
///
/// Unlike [`CroncheckProbe`], which answers only "definitely running /
/// definitely absent / ambiguous", this distinguishes a valid Gregg
/// readiness state from an unreachable endpoint and from a peer that
/// answered but is not a valid Gregg health endpoint. It never infers
/// process ownership from port occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthProbe {
    /// A valid Gregg endpoint reports `ready`.
    Ready,
    /// A valid Gregg endpoint reports `warming`.
    Warming,
    /// A valid Gregg endpoint reports `failed` (alive but unhealthy).
    Failed,
    /// No listener answered: refused, timed out, or otherwise unreachable.
    Unreachable,
    /// Something answered but it is not a valid Gregg health endpoint.
    NotGregg,
}

/// Run the authoritative bounded health probe and classify the result.
pub(crate) fn probe_health(target: SocketAddr) -> HealthProbe {
    match fetch_health_bytes(target) {
        FetchOutcome::Refused | FetchOutcome::Failed => HealthProbe::Unreachable,
        FetchOutcome::Responded(bytes) => match classify_health_response(&bytes) {
            Some(gregg_protocol::ReadinessState::Ready) => HealthProbe::Ready,
            Some(gregg_protocol::ReadinessState::Warming) => HealthProbe::Warming,
            Some(gregg_protocol::ReadinessState::Failed) => HealthProbe::Failed,
            None => HealthProbe::NotGregg,
        },
    }
}

pub(crate) fn probe_greggd(target: SocketAddr) -> CroncheckProbe {
    // Same bounded fetch and strict validation as probe_health; only the
    // three-way watchdog answer differs. Refusal alone permits spawning;
    // anything else that is not a valid Gregg response stays ambiguous.
    match fetch_health_bytes(target) {
        FetchOutcome::Refused => CroncheckProbe::Absent,
        FetchOutcome::Responded(bytes) if classify_health_response(&bytes).is_some() => {
            CroncheckProbe::Running
        }
        FetchOutcome::Responded(_) | FetchOutcome::Failed => CroncheckProbe::Ambiguous,
    }
}

/// Wait, bounded, for a just-spawned daemon to answer health on `target`.
///
/// Split out of the `croncheck` arm so the readiness *rule* is testable
/// without a real spawn: the deadline, not the spawn, decides the outcome.
/// Reaching the deadline still absent is returned as an error, never
/// swallowed — this is the entry point the managed cron block schedules, so an
/// unconditional success would leave the watchdog reporting a healthy daemon
/// forever while the daemon never ran. `Ambiguous` stops the wait immediately
/// and is reported, exactly as it is when it is seen before any spawn.
///
/// `probe` is injected so the rule can be exercised against a daemon that never
/// answers without racing a real one for a port.
pub(crate) fn wait_for_spawned_daemon(
    target: SocketAddr,
    deadline: std::time::Instant,
    mut probe: impl FnMut(SocketAddr) -> CroncheckProbe,
) -> Result<(), CroncheckProbe> {
    let mut last;
    loop {
        last = probe(target);
        if last != CroncheckProbe::Absent {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    match last {
        CroncheckProbe::Running => Ok(()),
        other => Err(other),
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
pub(crate) fn build_daemon_command(
    config_path: &std::path::Path,
    explicit: bool,
) -> std::io::Result<ProcessCommand> {
    let exe = std::env::current_exe()?;
    Ok(build_daemon_command_for(&exe, config_path, explicit))
}

pub(crate) fn build_daemon_command_for(
    exe: &std::path::Path,
    config_path: &std::path::Path,
    explicit: bool,
) -> ProcessCommand {
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

    cmd
}

/// Dispatch a subcommand for a config path whose explicit/implicit intent is
/// not available to the caller.
///
/// This compatibility shim treats a path that does not exist as *implicit*,
/// so `load_config` falls back to [`Config::default`] instead of returning
/// [`ConfigError::Io`]. That is the wrong answer whenever the operator typed
/// `--config /nonexistent.toml`, so the binary entry point must use
/// [`dispatch_with_config_intent`] instead, which preserves the intent
/// explicitly. Prefer that function in new code.
#[deprecated(
    since = "1.0.15",
    note = "cannot distinguish an explicit --config from the implicit default; use dispatch_with_config_intent"
)]
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
#[allow(clippy::too_many_lines)]
pub fn dispatch_with_config_intent(
    command: &Command,
    config_path: &std::path::Path,
    explicit: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Run => Err(Box::new(std::io::Error::other(
            "Command::Run is handled at the binary boundary",
        )) as Box<dyn std::error::Error>),
        Command::Stop => {
            // Unix uses the local control socket and is dispatched at the
            // binary boundary (see main.rs) so it can return errors as the
            // runtime/library boundary without a global tracing init.
            #[cfg(unix)]
            {
                Err(Box::new(std::io::Error::other(
                    "Command::Stop is handled at the binary boundary on Unix",
                )) as Box<dyn std::error::Error>)
            }
            #[cfg(not(unix))]
            {
                Err(Box::new(std::io::Error::other(
                    "Command::Stop is handled at the binary boundary on Windows",
                )) as Box<dyn std::error::Error>)
            }
        }
        Command::Croncheck => {
            let config = load_config(config_path, explicit)?;
            let target = croncheck_target(&config);
            match probe_greggd(target) {
                CroncheckProbe::Running => Ok(()),
                CroncheckProbe::Absent => {
                    build_daemon_command(config_path, explicit)?.spawn()?;
                    // `run` binds before its first sample; 2s covers bind plus
                    // the first health answer. The child was spawned with null
                    // stdio, so nothing else reports it if it never comes up.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    wait_for_spawned_daemon(target, deadline, probe_greggd).map_err(|probe| {
                        Box::new(std::io::Error::other(format!(
                            "croncheck started greggd but it did not become healthy within \
                             2s (last probe: {probe:?})"
                        ))) as Box<dyn std::error::Error>
                    })
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
        Command::Job { command } => dispatch_job_command(command, config_path, explicit),
        Command::Edit => edit_config(config_path, explicit),
        Command::Status => {
            let config = load_config(config_path, explicit)?;
            let report = crate::status::gather_status(
                &config,
                config_path,
                version_string(),
                probe_health,
                crate::startup::startup_state(),
            );
            print!("{}", crate::status::render_status(&report));
            if crate::status::status_is_present(&report) {
                Ok(())
            } else {
                Err(Box::new(std::io::Error::other(format!(
                    "greggd status: configured endpoint is {} (health: {})",
                    crate::status::status_outcome(&report),
                    crate::status::health_token(report.health),
                ))) as Box<dyn std::error::Error>)
            }
        }
        Command::Host { address } => mutate_config(config_path, explicit, |config| {
            config.host = *address;
        }),
        Command::Port { port } => mutate_config(config_path, explicit, |config| {
            config.port = *port;
        }),
        Command::Uninstall { dry_run, purge } => {
            crate::uninstall::run_uninstall(config_path, explicit, *dry_run, *purge)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        }
        Command::Version => {
            println!("{}", version_string());
            Ok(())
        }
        Command::Update => {
            let outcome = crate::update::run_update(config_path, explicit)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            match outcome {
                crate::update::UpdateOutcome::AlreadyCurrent { .. }
                | crate::update::UpdateOutcome::UpdatedBinary { .. }
                | crate::update::UpdateOutcome::UpdatedFromCargo { .. } => {
                    println!("{outcome}");
                    Ok(())
                }
                crate::update::UpdateOutcome::UpdatedButRestartFailed { .. } => {
                    // update.rs already printed the installed version and the
                    // exact restart command needed; convert to a nonzero exit
                    // so automation notices incomplete activation.
                    eprintln!("{outcome}");
                    Err(Box::new(crate::update::UpdateError::RestartFailed(
                        outcome.to_string(),
                    )) as Box<dyn std::error::Error>)
                }
            }
        }
        Command::Restart => {
            #[cfg(target_os = "windows")]
            {
                Err(Box::new(std::io::Error::other(
                    "Windows service commands are dispatched at the binary boundary",
                )) as Box<dyn std::error::Error>)
            }
            #[cfg(not(target_os = "windows"))]
            {
                let exe = std::env::current_exe()?;
                crate::startup::restart_daemon(&exe, config_path, explicit)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                Ok(())
            }
        }
        Command::Startup { command } => match command {
            StartupCommand::Install { method } => {
                let exe = std::env::current_exe()?;
                crate::startup::install_startup(&exe, config_path, explicit, *method)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                Ok(())
            }
            StartupCommand::Instructions { method } => {
                let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("greggd"));
                let resolved = crate::startup::resolve_startup_method(*method);
                let text =
                    crate::startup::render_instructions(resolved, &exe, config_path, explicit);
                println!("{text}");
                Ok(())
            }
        },
        #[cfg(target_os = "windows")]
        Command::Start => Err(Box::new(std::io::Error::other(
            "Windows service commands are dispatched at the binary boundary",
        )) as Box<dyn std::error::Error>),
        #[cfg(target_os = "windows")]
        Command::Service => Err(Box::new(std::io::Error::other(
            "Command::Service is handled in main.rs",
        )) as Box<dyn std::error::Error>),
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod native_tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use clap::Parser;
    use std::net::TcpListener;

    #[test]
    fn parser_accepts_run_stop_croncheck_mutations_and_version_but_not_windows_lifecycle() {
        for args in [
            "run",
            "stop",
            "croncheck",
            "configprint",
            "version",
            "restart",
        ] {
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
        assert!(Cli::try_parse_from(["greggd", "startup", "install"]).is_ok());
        assert!(
            Cli::try_parse_from(["greggd", "startup", "install", "--method", "systemd"]).is_ok()
        );
        assert!(Cli::try_parse_from(["greggd", "startup", "install", "--method", "cron"]).is_ok());
        assert!(Cli::try_parse_from(["greggd", "uninstall"]).is_ok());
        assert!(Cli::try_parse_from(["greggd", "uninstall", "--dry-run", "--purge"]).is_ok());
        assert!(Cli::try_parse_from(["greggd", "startup", "instructions"]).is_ok());
        assert!(
            Cli::try_parse_from(["greggd", "startup", "instructions", "--method", "launchd"])
                .is_ok()
        );
        // `croncheck` no longer takes a `--target` flag: it operates on
        // the configured local bind only.
        assert!(
            Cli::try_parse_from(["greggd", "croncheck", "--target", "192.168.182.143:11310"])
                .is_err()
        );
        {
            let command = "start";
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
        // The wildcard is this test's subject, so it is set explicitly. The
        // default is loopback (Plan 187) and is not what this asserts.
        let mut config = Config {
            host: "0.0.0.0".parse().unwrap(),
            ..Config::default()
        };
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
        let mut config = Config::default();
        config.allow_privileged_jobs = true;
        config.jobs.push(crate::config::ScheduledJobConfig {
            name: "preserved-job".to_owned(),
            schedule: "0 3 * * 0".to_owned(),
            command: vec!["/usr/bin/true".to_owned()],
            working_dir: Some(std::path::PathBuf::from("/tmp/not-required")),
            max_load: Some(8.0),
            load_window: Some("5m".to_owned()),
            retry_interval_ms: Some(60_000),
            max_wait_ms: Some(120_000),
        });
        config.write_atomic(&path).unwrap();
        mutate_config(&path, false, |config| config.port = 11320).unwrap();
        let updated = Config::load(&path).unwrap();
        assert_eq!(updated.port, 11320);
        assert!(updated.allow_privileged_jobs);
        assert_eq!(updated.jobs, config.jobs);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn plan196_config_path(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "greggd-plan196-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create test directory");
        let path = dir.join("greggd.toml");
        (dir, path)
    }

    #[test]
    fn job_cli_parses_config_path_and_preserves_trailing_argv() {
        let cli = Cli::try_parse_from([
            "greggd",
            "--config",
            "/tmp/plan196-first.toml",
            "job",
            "add",
            "rotate-logs",
            "--schedule",
            "0 2 * * *",
            "--working-dir",
            "/var/log",
            "--",
            "/usr/local/bin/rotate",
            "--keep",
            "14 days",
        ])
        .expect("valid job add command");
        assert_eq!(
            cli.config,
            Some(std::path::PathBuf::from("/tmp/plan196-first.toml"))
        );
        let Command::Job {
            command:
                JobCommand::Add {
                    name,
                    schedule,
                    working_dir,
                    command,
                    ..
                },
        } = cli.command
        else {
            panic!("expected job add");
        };
        assert_eq!(name, "rotate-logs");
        assert_eq!(schedule, "0 2 * * *");
        assert_eq!(working_dir, Some(std::path::PathBuf::from("/var/log")));
        assert_eq!(
            command,
            vec![
                "/usr/local/bin/rotate".to_owned(),
                "--keep".to_owned(),
                "14 days".to_owned()
            ]
        );

        let cli = Cli::try_parse_from([
            "greggd",
            "job",
            "add",
            "x",
            "--schedule",
            "0 2 * * *",
            "--config",
            "/tmp/plan196-second.toml",
            "--",
            "program",
            "two words",
            "--literal",
        ])
        .expect("global config can follow the nested subcommand");
        assert_eq!(
            cli.config,
            Some(std::path::PathBuf::from("/tmp/plan196-second.toml"))
        );
        let Command::Job {
            command: JobCommand::Add { command, .. },
        } = cli.command
        else {
            panic!("expected job add");
        };
        assert_eq!(
            command,
            vec![
                "program".to_owned(),
                "two words".to_owned(),
                "--literal".to_owned()
            ]
        );
    }

    #[test]
    fn job_and_edit_command_forms_are_explicit_and_bounded() {
        assert!(matches!(
            Cli::try_parse_from(["greggd", "job", "list"])
                .unwrap()
                .command,
            Command::Job {
                command: JobCommand::List
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["greggd", "job", "remove"])
                .unwrap()
                .command,
            Command::Job {
                command: JobCommand::Remove { name: None }
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["greggd", "job", "remove", "exact-name"]).unwrap().command,
            Command::Job { command: JobCommand::Remove { name: Some(name) } } if name == "exact-name"
        ));
        assert!(matches!(
            Cli::try_parse_from(["greggd", "edit"]).unwrap().command,
            Command::Edit
        ));
        assert!(Cli::try_parse_from([
            "greggd",
            "job",
            "add",
            "missing-separator",
            "--schedule",
            "0 2 * * *",
            "program"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "greggd",
            "job",
            "add",
            "missing-command",
            "--schedule",
            "0 2 * * *",
            "--"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["greggd", "job", "list", "unexpected"]).is_err());
    }

    #[test]
    fn job_add_preserves_config_and_exact_argv_without_root_opt_in() {
        let (dir, path) = plan196_config_path("add");
        let mut original = Config::default();
        original.name = "preserved-name".to_owned();
        original.port = 11328;
        original.scheduler_history_limit = Some(3);
        original.write_atomic(&path).unwrap();
        let command = JobCommand::Add {
            name: "rotate-logs".to_owned(),
            schedule: "0 2 * * *".to_owned(),
            working_dir: Some(std::path::PathBuf::from("/var/log")),
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
            command: vec![
                "/usr/local/bin/rotate".to_owned(),
                "--keep".to_owned(),
                "14 days".to_owned(),
            ],
        };
        dispatch_job_command(&command, &path, true).unwrap();
        let updated = Config::load(&path).unwrap();
        assert_eq!(updated.name, "preserved-name");
        assert_eq!(updated.port, 11328);
        assert_eq!(updated.scheduler_history_limit, Some(3));
        assert!(!updated.allow_privileged_jobs);
        assert_eq!(updated.jobs.len(), 1);
        assert_eq!(
            updated.jobs[0].command,
            vec![
                "/usr/local/bin/rotate".to_owned(),
                "--keep".to_owned(),
                "14 days".to_owned()
            ]
        );
        assert_eq!(
            updated.jobs[0].working_dir,
            Some(std::path::PathBuf::from("/var/log"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_or_duplicate_job_add_does_not_write_config() {
        let (dir, path) = plan196_config_path("add-reject");
        let mut original = Config::default();
        original.jobs.push(crate::config::ScheduledJobConfig {
            name: "duplicate".to_owned(),
            schedule: "0 2 * * *".to_owned(),
            command: vec!["program".to_owned()],
            working_dir: None,
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        });
        original.write_atomic(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let make_add = |name: &str, schedule: &str| JobCommand::Add {
            name: name.to_owned(),
            schedule: schedule.to_owned(),
            working_dir: None,
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
            command: vec!["program".to_owned()],
        };
        let duplicate = dispatch_job_command(&make_add("duplicate", "0 3 * * *"), &path, true)
            .expect_err("duplicate should fail");
        assert!(duplicate.to_string().contains("already exists"));
        let invalid = dispatch_job_command(&make_add("new-job", "not a schedule"), &path, true)
            .expect_err("invalid schedule should fail");
        assert!(invalid
            .to_string()
            .contains("configuration validation failed"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn job_list_is_read_only_and_renders_argv_as_escaped_values() {
        let (dir, path) = plan196_config_path("list");
        let mut config = Config::default();
        config.jobs.push(crate::config::ScheduledJobConfig {
            name: "quoted".to_owned(),
            schedule: "0 2 * * *".to_owned(),
            command: vec![
                "program".to_owned(),
                "two words".to_owned(),
                "a\"b".to_owned(),
            ],
            working_dir: Some(std::path::PathBuf::from("/var/tmp")),
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        });
        config.write_atomic(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let rendered = render_job_list(&path, &config);
        assert!(rendered.contains(&format!("command argv: {:?}", config.jobs[0].command)));
        assert!(rendered.contains("working directory: /var/tmp"));
        dispatch_job_command(&JobCommand::List, &path, true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn named_job_removal_is_exact_and_missing_name_does_not_write() {
        let (dir, path) = plan196_config_path("remove");
        let mut config = Config::default();
        for name in ["first", "second", "third"] {
            config.jobs.push(crate::config::ScheduledJobConfig {
                name: name.to_owned(),
                schedule: "0 2 * * *".to_owned(),
                command: vec!["program".to_owned()],
                working_dir: None,
                max_load: None,
                load_window: None,
                retry_interval_ms: None,
                max_wait_ms: None,
            });
        }
        config.write_atomic(&path).unwrap();
        remove_named_job(&path, true, "second").unwrap();
        let updated = Config::load(&path).unwrap();
        assert_eq!(
            updated
                .jobs
                .iter()
                .map(|job| job.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "third"]
        );
        let before = std::fs::read(&path).unwrap();
        let missing =
            remove_named_job(&path, true, "absent").expect_err("unknown name should fail");
        assert!(missing.to_string().contains("no scheduled job named"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn omitted_name_non_tty_removal_refuses_without_writing() {
        let (dir, path) = plan196_config_path("non-tty");
        let mut config = Config::default();
        config.jobs.push(crate::config::ScheduledJobConfig {
            name: "existing".to_owned(),
            schedule: "0 2 * * *".to_owned(),
            command: vec!["program".to_owned()],
            working_dir: None,
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        });
        config.write_atomic(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = dispatch_job_command(&JobCommand::Remove { name: None }, &path, true)
            .expect_err("non-TTY interactive selection should refuse");
        assert!(error.to_string().contains("requires a terminal"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn edit_launches_editor_directly_with_one_candidate_argument() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, path) = plan196_config_path("editor-direct");
        Config::default().write_atomic(&path).unwrap();
        let editor = dir.join("controlled-editor");
        std::fs::write(
            &editor,
            "#!/bin/sh\n[ \"$#\" -eq 1 ] || exit 21\nprintf '%s\\n' 'name = \"edited\"' 'host = \"127.0.0.1\"' 'port = 11319' 'sample_interval_ms = 1000' 'stale_after_ms = 10000' > \"$1\"\n",
        ).unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
        edit_config_with_editor(&path, true, editor.to_str().unwrap()).unwrap();
        let updated = Config::load(&path).unwrap();
        assert_eq!(updated.name, "edited");
        assert_eq!(updated.port, 11319);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn editor_nonzero_exit_preserves_original_config() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, path) = plan196_config_path("editor-failure");
        Config::default().write_atomic(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let editor = dir.join("failing-editor");
        std::fs::write(&editor, "#!/bin/sh\nexit 7\n").unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = edit_config_with_editor(&path, true, editor.to_str().unwrap())
            .expect_err("nonzero editor status should fail");
        assert!(error.to_string().contains("editor exited with status"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".greggd-edit-")
        }));
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

    /// The watchdog's whole job is to restart a daemon that is not running, so
    /// "spawned but never came up" has to be visible as a failure. Reporting
    /// success there would leave cron believing a dead daemon is healthy.
    #[test]
    fn a_spawned_daemon_that_never_answers_fails_the_watchdog() {
        let target = unbound_loopback();
        let deadline = std::time::Instant::now();
        assert_eq!(
            wait_for_spawned_daemon(target, deadline, |_| CroncheckProbe::Absent),
            Err(CroncheckProbe::Absent),
            "a spawn that never became healthy must not report success"
        );
    }

    /// An answer that is not a valid Gregg health response stops the wait at
    /// once and is reported rather than retried to the deadline.
    #[test]
    fn an_ambiguous_answer_during_the_wait_is_reported_immediately() {
        let target = unbound_loopback();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        assert_eq!(
            wait_for_spawned_daemon(target, deadline, |_| CroncheckProbe::Ambiguous),
            Err(CroncheckProbe::Ambiguous)
        );
    }

    #[test]
    fn a_daemon_that_answers_within_the_wait_is_a_success() {
        let target = unbound_loopback();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut calls = 0_u32;
        assert_eq!(
            wait_for_spawned_daemon(target, deadline, |_| {
                calls += 1;
                if calls < 2 {
                    CroncheckProbe::Absent
                } else {
                    CroncheckProbe::Running
                }
            }),
            Ok(())
        );
        assert_eq!(calls, 2);
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
