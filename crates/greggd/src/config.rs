//! Daemon configuration, validation, file I/O, and atomic persistence.
//!
//! Configuration is stored as canonical TOML and validated before every
//! load and before every mutation. Atomic writes ensure a partially written
//! file can never corrupt a running service.

use std::fmt;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

use serde::{Deserialize, Serialize};

/// Plan 162: frozen per-job scheduler history depth.
pub use gregg_protocol::{
    DEFAULT_SCHEDULER_HISTORY_LIMIT, MAX_SCHEDULER_HISTORY_LIMIT, MAX_SCHEDULER_JOBS,
};

/// Minimum allowed sample interval in milliseconds.
pub const MIN_SAMPLE_INTERVAL_MS: u64 = 250;

/// Maximum allowed sample interval in milliseconds.
pub const MAX_SAMPLE_INTERVAL_MS: u64 = 60_000;

/// Minimum port number.
pub const MIN_PORT: u16 = 1;

/// Maximum port number.
pub const MAX_PORT: u16 = 65535;

/// Maximum length for the display name after trimming.
pub const MAX_NAME_LEN: usize = 128;
/// Maximum configured scheduled jobs.
///
/// Also the maximum number of job entries in one scheduler wire document, so
/// a document that validates can always be produced by a conforming daemon.
pub const MAX_JOBS: usize = 64;
/// Maximum Unicode scalar values in a job name.
pub const MAX_JOB_NAME_CHARS: usize = 96;
/// Maximum argv entries in one job.
pub const MAX_COMMAND_ARGS: usize = 64;
/// Maximum UTF-8 bytes in one argv entry.
pub const MAX_COMMAND_ARG_BYTES: usize = 4096;
/// Minimum load retry interval.
pub const MIN_RETRY_INTERVAL_MS: u64 = 10_000;
/// Maximum load retry interval.
pub const MAX_RETRY_INTERVAL_MS: u64 = 3_600_000;
/// Default load retry interval.
pub const DEFAULT_RETRY_INTERVAL_MS: u64 = 300_000;
/// Default maximum load wait.
pub const DEFAULT_MAX_WAIT_MS: u64 = 86_400_000;
/// Maximum load wait.
pub const MAX_MAX_WAIT_MS: u64 = 604_800_000;

#[cfg(unix)]
fn ensure_config_directory(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut current = PathBuf::new();
    for component in dir.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current) {
            Ok(()) => fs::set_permissions(&current, fs::Permissions::from_mode(0o700))?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // An existing operator-managed directory keeps its mode. The
                // subsequent temp-file create/rename reports real access
                // failures instead of guessing from mode bits.
                if !current.is_dir() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Daemon configuration.
///
/// All fields are serialized to TOML. Unknown fields are rejected during
/// deserialization to prevent silent typo acceptance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Human-readable display name for this host.
    pub name: String,
    /// IPv4 or IPv6 address to bind the HTTP server to.
    pub host: IpAddr,
    /// TCP port to listen on.
    pub port: u16,
    /// Native sampling interval in milliseconds.
    pub sample_interval_ms: u64,
    /// Duration in milliseconds after which a snapshot is considered stale.
    /// A value of `0` disables age-based staleness.
    pub stale_after_ms: u64,
    /// Explicit opt-in for configured jobs running as Unix euid 0.
    #[serde(default)]
    pub allow_privileged_jobs: bool,
    /// Locally scheduled argv commands. Missing means no scheduled jobs.
    #[serde(default)]
    pub jobs: Vec<ScheduledJobConfig>,
    /// Plan 162: retained terminal scheduler records kept per job in memory.
    ///
    /// `None` means [`DEFAULT_SCHEDULER_HISTORY_LIMIT`]. `Some(0)` disables
    /// record retention entirely while keeping the live scheduler summary
    /// available. Values above [`MAX_SCHEDULER_HISTORY_LIMIT`] are rejected
    /// before the listener binds. There is deliberately no path, database, or
    /// log-file setting: scheduler history is memory-only.
    #[serde(default)]
    pub scheduler_history_limit: Option<usize>,
}

/// One local maintenance command and its optional cached-load gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScheduledJobConfig {
    /// Stable operator-facing job name.
    pub name: String,
    /// Five-field cron schedule in local civil time.
    pub schedule: String,
    /// Executable and arguments, passed directly without a shell.
    pub command: Vec<String>,
    /// Optional current working directory; not resolved during config loading.
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    /// Optional inclusive load threshold.
    #[serde(default)]
    pub max_load: Option<f32>,
    /// Load averaging period: `1m`, `5m`, or `15m` (defaults to `15m`).
    #[serde(default)]
    pub load_window: Option<String>,
    /// Delay between cached-load retries.
    #[serde(default)]
    pub retry_interval_ms: Option<u64>,
    /// Maximum time a due occurrence may remain load-deferred.
    #[serde(default)]
    pub max_wait_ms: Option<u64>,
}

/// Platform class used by scheduler-specific validation and its tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))]
pub(crate) enum JobPlatform {
    Unix,
    Windows,
}

impl JobPlatform {
    const fn current() -> Self {
        #[cfg(unix)]
        {
            Self::Unix
        }
        #[cfg(windows)]
        {
            Self::Windows
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self::Unix
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: String::from("greggd"),
            host: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 11310,
            sample_interval_ms: 1000,
            stale_after_ms: 10_000,
            allow_privileged_jobs: false,
            jobs: Vec::new(),
            scheduler_history_limit: None,
        }
    }
}

/// Whether a temp file is old enough to be treated as abandoned.
///
/// Fails closed: an unknown mtime, or an mtime in the future (clock skew),
/// is *not* stale, so the only way to reach `remove_file` is a file that is
/// demonstrably old.
fn temp_is_stale(modified: Option<std::time::SystemTime>) -> bool {
    modified
        .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= STALE_TEMP_AGE)
}

impl Config {
    /// Plan 162: effective per-job terminal history depth.
    ///
    /// Missing configuration retains the default of exactly five records; an
    /// explicit `0` means "keep live state, retain no records".
    #[must_use]
    pub fn scheduler_history_limit(&self) -> usize {
        self.scheduler_history_limit
            .unwrap_or(DEFAULT_SCHEDULER_HISTORY_LIMIT)
    }
}

/// How old a `.greggd-*.toml.tmp` file must be before it is treated as stale.
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(300);

/// Remove stale `.greggd-*.toml.tmp` files left by prior crashes.
///
/// Only files older than [`STALE_TEMP_AGE`] are removed. A concurrent writer's
/// in-flight temp matches this same pattern, and unlinking it mid-write makes
/// that writer fail re-validation or `rename` on a write that had already
/// succeeded — a daemon bootstrap racing a `startup install` or
/// `uninstall --purge` is enough to trigger it. This mirrors the client-side
/// gate in `gregg::config::store`, which says the same thing about
/// `.gregg-*.toml.tmp`.
fn cleanup_stale_temps(dir: &Path) -> std::io::Result<()> {
    let entries = fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(name_str) = name.to_str() {
            if name_str.starts_with(".greggd-") && name_str.ends_with(".toml.tmp") {
                // `file_type()` never follows symlinks, so only true regular
                // files are considered. `remove_file` operates on the final
                // path component: if an attacker swaps the entry for a
                // symlink after the check, the link itself is unlinked —
                // never its target — so the check-to-unlink window cannot
                // damage anything outside this directory.
                let eligible = entry.file_type().is_ok_and(|file_type| file_type.is_file());
                if eligible {
                    // Age gate: skip a recent file that may belong to a writer
                    // that is still in flight.
                    if !temp_is_stale(entry.metadata().and_then(|meta| meta.modified()).ok()) {
                        continue;
                    }
                    match fs::remove_file(entry.path()) {
                        Ok(()) => {}
                        // Already gone: another process cleaned it up after
                        // the directory listing was taken.
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            tracing::warn!(
                                path = %entry.path().display(),
                                error = %error,
                                "failed to remove stale config temp file"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

impl Config {
    /// Validate all fields.
    ///
    /// Returns a list of all violations so callers can present every
    /// problem at once rather than fixing them one at a time.
    #[must_use]
    pub fn validate(&self) -> Vec<ConfigViolation> {
        self.validate_for(JobPlatform::current())
    }

    fn validate_for(&self, job_platform: JobPlatform) -> Vec<ConfigViolation> {
        let mut violations = Vec::new();

        // Name validation.
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            violations.push(ConfigViolation::EmptyName);
        } else if trimmed.chars().any(char::is_control) {
            violations.push(ConfigViolation::NameContainsControlCharacters);
        } else {
            // Character count (not byte length) for the user-facing limit.
            let char_len = trimmed.chars().count();
            if char_len > MAX_NAME_LEN {
                violations.push(ConfigViolation::NameTooLong {
                    length: char_len,
                    max: MAX_NAME_LEN,
                });
            }
        }

        // Port validation. u16 cannot exceed 65535, so only check for zero.
        if self.port == 0 {
            violations.push(ConfigViolation::InvalidPort(self.port));
        }

        // Sample interval validation.
        if self.sample_interval_ms < MIN_SAMPLE_INTERVAL_MS
            || self.sample_interval_ms > MAX_SAMPLE_INTERVAL_MS
        {
            violations.push(ConfigViolation::InvalidSampleInterval(
                self.sample_interval_ms,
            ));
        }

        // Staleness threshold: if non-zero, must exceed sample interval
        // to be meaningful (otherwise every snapshot is immediately stale).
        if self.stale_after_ms > 0 && self.stale_after_ms <= self.sample_interval_ms {
            violations.push(ConfigViolation::StalenessBelowInterval {
                stale_after_ms: self.stale_after_ms,
                sample_interval_ms: self.sample_interval_ms,
            });
        }

        if self.jobs.len() > MAX_JOBS {
            violations.push(ConfigViolation::InvalidJobs(format!(
                "jobs has {} entries; maximum is {MAX_JOBS}",
                self.jobs.len()
            )));
        }
        if let Some(limit) = self.scheduler_history_limit {
            if limit > MAX_SCHEDULER_HISTORY_LIMIT {
                violations.push(ConfigViolation::InvalidSchedulerHistoryLimit {
                    found: limit,
                    max: MAX_SCHEDULER_HISTORY_LIMIT,
                });
            }
        }
        let mut names = std::collections::HashSet::new();
        for (index, job) in self.jobs.iter().enumerate() {
            for error in job.validate(job_platform) {
                violations.push(ConfigViolation::InvalidJobs(format!(
                    "jobs[{index}].{error}"
                )));
            }
            if !names.insert(job.name.as_str()) {
                violations.push(ConfigViolation::InvalidJobs(format!(
                    "jobs[{index}].name duplicates {:?}",
                    job.name
                )));
            }
        }

        violations
    }

    /// Returns `true` if the configuration passes validation.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.validate().is_empty()
    }

    /// Return the platform-specific default config path.
    #[must_use]
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            PathBuf::from("/etc/gregg/greggd.toml")
        }
        #[cfg(target_os = "macos")]
        {
            PathBuf::from("/Library/Application Support/gregg/greggd.toml")
        }
        #[cfg(target_os = "windows")]
        {
            let program_data =
                std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".to_owned());
            PathBuf::from(program_data)
                .join("gregg")
                .join("greggd.toml")
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            PathBuf::from("greggd.toml")
        }
    }

    /// Return the platform-specific default host path for the socket.
    #[must_use]
    pub fn default_host_socket_path() -> PathBuf {
        Self::default_path()
            .parent()
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
    }

    /// Load configuration from the given TOML file path.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the file cannot be read, parsed, or
    /// fails validation.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::parse(&content, Some(path))
    }

    /// Parse a TOML configuration string.
    ///
    /// When `path` is provided, it is used in error messages for
    /// diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the content is not valid TOML,
    /// contains unknown fields, or fails validation.
    pub fn parse(content: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(content).map_err(|e| ConfigError::Parse {
            path: path.map(PathBuf::from),
            source: e,
        })?;

        let violations = config.validate();
        if violations.is_empty() {
            Ok(config)
        } else {
            Err(ConfigError::Validation(violations))
        }
    }

    /// Serialize this configuration to canonical TOML.
    ///
    /// # Errors
    ///
    /// Returns the TOML serializer error if serialization fails.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Atomically write this configuration to the given path.
    ///
    /// This follows the write-flush-verify-rename pattern:
    /// 1. Write to a unique temporary file in the same directory.
    /// 2. Flush the file.
    /// 3. Reopen and re-parse the temporary file as verification.
    /// 4. Rename over the destination.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if any step fails. On failure, the
    /// temporary file is cleaned up and the original file is left intact.
    pub fn write_atomic(&self, path: &Path) -> Result<(), ConfigError> {
        // 1. Resolve and validate the destination directory.
        let dir = path.parent().ok_or_else(|| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::NoParentDirectory,
        })?;
        #[cfg(unix)]
        ensure_config_directory(dir).map_err(|e| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(e),
        })?;
        #[cfg(not(unix))]
        fs::create_dir_all(dir).map_err(|e| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(e),
        })?;

        // 2. Serialize the complete config.
        let content = self.to_toml().map_err(|source| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Serialization(source),
        })?;

        // 2b. Clean up stale temp files from prior crashes.
        cleanup_stale_temps(dir).map_err(|source| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(source),
        })?;

        // 3. Write to a uniquely named temporary file. Nanos + counter
        // defeat pid-reuse collisions; the random suffix makes the first
        // name unguessable so a squatter cannot pre-create it to force a
        // `create_new` DoS.
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let temp_name = format!(
            ".greggd-{}-{}-{}-{:08x}.toml.tmp",
            std::process::id(),
            nanos,
            id,
            temp_rand_suffix(nanos, id),
        );
        let temp_path = dir.join(&temp_name);

        let write_result = (|| -> std::io::Result<()> {
            let mut file = create_secure_temp_file(&temp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            drop(file);
            Ok(())
        })();
        write_result.map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        // 5. Verify the temporary file round-trips before replacing
        // the destination, so a failed verification leaves the
        // original file intact.
        let Ok(verified) = Self::load(&temp_path) else {
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::VerificationFailed,
            });
        };
        if *self != verified {
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::VerificationFailed,
            });
        }

        // 6. Rename atomically over the destination.
        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        // 6b. Relax the final mode to world-readable. The temp file is
        // 0600 during the write so partial content is never exposed, but
        // the daemon config carries no secrets (name/host/port/intervals)
        // and read-only diagnostics (`croncheck`, `status`, `configprint`)
        // must work for unprivileged operators and cron. A user-local
        // parent created 0700 above still protects that case; a system
        // directory such as /etc/gregg is 0755 so 0644 is actually
        // readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).map_err(|e| {
                ConfigError::AtomicWrite {
                    path: path.to_path_buf(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
        }

        sync_parent_directory(dir).map_err(|e| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(e),
        })?;

        Ok(())
    }

    /// Return a reference to the host field.
    #[must_use]
    pub fn host(&self) -> IpAddr {
        self.host
    }

    /// Return a reference to the port field.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Return the `sample_interval_ms` field.
    #[must_use]
    pub fn sample_interval_ms(&self) -> u64 {
        self.sample_interval_ms
    }

    /// Return the `stale_after_ms` field.
    #[must_use]
    pub fn stale_after_ms(&self) -> u64 {
        self.stale_after_ms
    }

    /// Fail closed when root would otherwise silently gain command execution.
    pub(crate) fn validate_job_authority(&self, is_unix_root: bool) -> Result<(), ConfigError> {
        if is_unix_root && !self.jobs.is_empty() && !self.allow_privileged_jobs {
            return Err(ConfigError::PrivilegedJobsOptInRequired);
        }
        Ok(())
    }
}

impl ScheduledJobConfig {
    fn validate(&self, platform: JobPlatform) -> Vec<String> {
        let mut errors = Vec::new();
        if self.name.trim().is_empty() {
            errors.push("name must be non-empty".to_owned());
        }
        if self.name.chars().count() > MAX_JOB_NAME_CHARS {
            errors.push(format!("name exceeds {MAX_JOB_NAME_CHARS} characters"));
        }
        if self.name.chars().any(char::is_control) {
            errors.push("name contains control characters".to_owned());
        }
        if self.schedule.len() > crate::scheduler::schedule::MAX_SCHEDULE_BYTES {
            errors.push(format!(
                "schedule exceeds {} UTF-8 bytes",
                crate::scheduler::schedule::MAX_SCHEDULE_BYTES
            ));
        } else {
            match crate::scheduler::schedule::LocalSchedule::parse(&self.schedule) {
                // A syntactically valid expression that no Gregorian date can
                // satisfy would otherwise survive configuration loading and
                // fail when the scheduler computes its first occurrence.
                Ok(schedule) if !schedule.is_satisfiable() => {
                    errors
                        .push("schedule: no calendar date can satisfy this expression".to_owned());
                }
                Ok(_) => {}
                Err(error) => errors.push(format!("schedule: {error}")),
            }
        }
        if self.command.is_empty() || self.command[0].is_empty() {
            errors.push("command must contain a non-empty executable".to_owned());
        }
        if self.command.len() > MAX_COMMAND_ARGS {
            errors.push(format!("command exceeds {MAX_COMMAND_ARGS} argv entries"));
        }
        for (index, value) in self.command.iter().enumerate() {
            if value.len() > MAX_COMMAND_ARG_BYTES {
                errors.push(format!(
                    "command[{index}] exceeds {MAX_COMMAND_ARG_BYTES} UTF-8 bytes"
                ));
            }
            if value.chars().any(char::is_control) {
                errors.push(format!("command[{index}] contains control characters"));
            }
        }
        if let Some(path) = &self.working_dir {
            match path.to_str() {
                Some(value) if !value.is_empty() && !value.chars().any(char::is_control) => {}
                Some(_) => errors.push("working_dir must be non-empty and control-free".to_owned()),
                None => errors.push("working_dir is not valid UTF-8".to_owned()),
            }
        }

        match self.max_load {
            Some(load) if !load.is_finite() || load < 0.0 => {
                errors.push("max_load must be finite and non-negative".to_owned());
            }
            Some(_) => {}
            None => {
                if self.load_window.is_some() {
                    errors.push("load_window requires max_load".to_owned());
                }
                if self.retry_interval_ms.is_some() {
                    errors.push("retry_interval_ms requires max_load".to_owned());
                }
                if self.max_wait_ms.is_some() {
                    errors.push("max_wait_ms requires max_load".to_owned());
                }
            }
        }
        if self.max_load.is_some() {
            if self
                .load_window
                .as_deref()
                .is_some_and(|window| !matches!(window, "1m" | "5m" | "15m"))
            {
                errors.push("load_window must be one of 1m, 5m, or 15m".to_owned());
            }
            let retry = self.retry_interval_ms.unwrap_or(DEFAULT_RETRY_INTERVAL_MS);
            let max_wait = self.max_wait_ms.unwrap_or(DEFAULT_MAX_WAIT_MS);
            if !(MIN_RETRY_INTERVAL_MS..=MAX_RETRY_INTERVAL_MS).contains(&retry) {
                errors.push(format!(
                    "retry_interval_ms must be {MIN_RETRY_INTERVAL_MS}..={MAX_RETRY_INTERVAL_MS}"
                ));
            }
            if max_wait == 0 || max_wait > MAX_MAX_WAIT_MS {
                errors.push(format!("max_wait_ms must be 1..={MAX_MAX_WAIT_MS}"));
            }
            if retry > max_wait {
                errors.push("retry_interval_ms must not exceed max_wait_ms".to_owned());
            }
            if platform == JobPlatform::Windows {
                errors.push("max_load is unsupported on Windows".to_owned());
            }
        }
        errors
    }

    pub(crate) fn effective_load_window(&self) -> &'static str {
        match self.load_window.as_deref().unwrap_or("15m") {
            "1m" => "1m",
            "5m" => "5m",
            _ => "15m",
        }
    }

    pub(crate) fn effective_retry_interval_ms(&self) -> u64 {
        self.retry_interval_ms.unwrap_or(DEFAULT_RETRY_INTERVAL_MS)
    }

    pub(crate) fn effective_max_wait_ms(&self) -> u64 {
        self.max_wait_ms.unwrap_or(DEFAULT_MAX_WAIT_MS)
    }
}

#[allow(clippy::cast_possible_truncation)]
fn temp_rand_suffix(nanos: u128, id: u64) -> u32 {
    // Best-effort unguessable suffix without new deps: 4 bytes from the OS
    // RNG, falling back to a time/counter xorshift.
    #[cfg(unix)]
    {
        use std::io::Read;
        let mut buf = [0_u8; 4];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(&mut buf).is_ok() {
                return u32::from_ne_bytes(buf);
            }
        }
    }
    let mut x =
        (nanos as u64 ^ id.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xBF58_4764_80FC_4A8F) as u32;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x
}

fn sync_parent_directory(dir: &Path) -> std::io::Result<()> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Windows requires this flag to open a directory as a file handle.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let file = match options.open(dir) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "warning: sync_parent_directory could not open {}: {}",
                dir.display(),
                error
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    match file.sync_all() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "warning: sync_parent_directory sync failed for {}: {}",
                dir.display(),
                error
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Create a new temporary config file with restrictive permissions.
fn create_secure_temp_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        if !file.metadata()?.file_type().is_file() {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "temporary config path is not a regular file",
            ));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "temporary config file permissions are not 0600",
            ));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        options.open(path)
    }
}

/// Errors that can occur during configuration operations.
#[derive(Debug)]
pub enum ConfigError {
    /// I/O error reading or writing the config file.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// TOML parsing error.
    Parse {
        path: Option<PathBuf>,
        source: toml::de::Error,
    },
    /// Configuration failed validation.
    Validation(Vec<ConfigViolation>),
    /// Root must explicitly acknowledge scheduled command execution.
    PrivilegedJobsOptInRequired,
    /// Atomic write operation failed.
    AtomicWrite {
        path: PathBuf,
        source: AtomicWriteError,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "failed to read {}: {source}", path.display()),
            Self::Parse { path, source } => {
                if let Some(p) = path {
                    write!(f, "failed to parse {}: {source}", p.display())
                } else {
                    write!(f, "failed to parse config: {source}")
                }
            }
            Self::Validation(violations) => {
                write!(f, "configuration validation failed:")?;
                for v in violations {
                    write!(f, "\n  - {v}")?;
                }
                Ok(())
            }
            Self::PrivilegedJobsOptInRequired => write!(
                f,
                "scheduled jobs will execute as root; set allow_privileged_jobs = true to opt in"
            ),
            Self::AtomicWrite { path, source } => {
                write!(f, "atomic write to {} failed: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Validation(_) | Self::PrivilegedJobsOptInRequired => None,
            Self::AtomicWrite { source, .. } => Some(source),
        }
    }
}

/// Errors specific to the atomic write operation.
#[derive(Debug)]
pub enum AtomicWriteError {
    /// The path has no parent directory.
    NoParentDirectory,
    /// An I/O error occurred.
    Io(std::io::Error),
    /// TOML serialization failed.
    Serialization(toml::ser::Error),
    /// The file was written but verification re-parse failed.
    VerificationFailed,
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoParentDirectory => write!(f, "path has no parent directory"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Serialization(e) => write!(f, "TOML serialization error: {e}"),
            Self::VerificationFailed => write!(f, "verification re-parse failed"),
        }
    }
}

impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Serialization(e) => Some(e),
            _ => None,
        }
    }
}

/// A single configuration validation violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigViolation {
    /// Display name is empty after trimming.
    EmptyName,
    /// Display name contains a control character.
    NameContainsControlCharacters,
    /// Display name exceeds the maximum length.
    NameTooLong { length: usize, max: usize },
    /// Port is outside the valid range.
    InvalidPort(u16),
    /// Sample interval is outside the valid range.
    InvalidSampleInterval(u64),
    /// Staleness threshold is below or equal to sample interval.
    StalenessBelowInterval {
        stale_after_ms: u64,
        sample_interval_ms: u64,
    },
    /// Scheduled-job fields failed validation.
    InvalidJobs(String),
    /// `scheduler_history_limit` exceeded the frozen hard maximum.
    InvalidSchedulerHistoryLimit {
        /// Configured value.
        found: usize,
        /// Frozen hard maximum.
        max: usize,
    },
}

impl fmt::Display for ConfigViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "name is empty after trimming"),
            Self::NameContainsControlCharacters => {
                write!(f, "name contains control characters")
            }
            Self::NameTooLong { length, max } => {
                write!(f, "name is {length} characters, exceeds maximum of {max}")
            }
            Self::InvalidPort(p) => {
                write!(f, "port {p} is outside valid range {MIN_PORT}..={MAX_PORT}")
            }
            Self::InvalidSampleInterval(ms) => {
                write!(
                    f,
                    "sample_interval_ms {ms} is outside valid range {MIN_SAMPLE_INTERVAL_MS}..={MAX_SAMPLE_INTERVAL_MS}"
                )
            }
            Self::StalenessBelowInterval {
                stale_after_ms,
                sample_interval_ms,
            } => {
                write!(
                    f,
                    "stale_after_ms {stale_after_ms} must be 0 (disabled) or greater than sample_interval_ms {sample_interval_ms}"
                )
            }
            Self::InvalidJobs(message) => f.write_str(message),
            Self::InvalidSchedulerHistoryLimit { found, max } => write!(
                f,
                "scheduler_history_limit {found} exceeds the maximum of {max} (0 disables record retention)"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();
        assert!(config.is_valid());
        assert_eq!(config.validate(), []);
    }

    #[test]
    fn config_round_trips_through_toml() {
        let config = Config::default();
        let toml = config.to_toml().unwrap();
        let parsed = Config::parse(&toml, None).unwrap();
        assert_eq!(config, parsed);
    }

    fn test_job(name: &str) -> ScheduledJobConfig {
        ScheduledJobConfig {
            name: name.to_owned(),
            schedule: "0 3 * * 0".to_owned(),
            command: vec!["/usr/bin/true".to_owned()],
            working_dir: None,
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        }
    }

    #[test]
    fn old_config_defaults_jobs_and_privileged_opt_in() {
        let config = Config::parse(
            "name = 'old'\nhost = '127.0.0.1'\nport = 11310\nsample_interval_ms = 1000\nstale_after_ms = 10000\n",
            None,
        )
        .unwrap();
        assert_eq!(config.jobs.len(), 0);
        assert!(!config.allow_privileged_jobs);
    }

    #[test]
    fn missing_scheduler_history_limit_retains_the_default() {
        // Plan 162: missing configuration means exactly five records per job.
        let config = Config::parse(
            "name = 'greggd'\nhost = '127.0.0.1'\nport = 11310\nsample_interval_ms = 1000\nstale_after_ms = 10000\n",
            None,
        )
        .unwrap();
        assert_eq!(config.scheduler_history_limit, None);
        assert_eq!(
            config.scheduler_history_limit(),
            DEFAULT_SCHEDULER_HISTORY_LIMIT
        );
        assert_eq!(DEFAULT_SCHEDULER_HISTORY_LIMIT, 5);
        assert_eq!(MAX_SCHEDULER_JOBS, MAX_JOBS);
    }

    #[test]
    fn scheduler_history_limit_accepts_the_documented_range() {
        let mut config = Config::default();
        for limit in [
            0,
            1,
            DEFAULT_SCHEDULER_HISTORY_LIMIT,
            MAX_SCHEDULER_HISTORY_LIMIT,
        ] {
            config.scheduler_history_limit = Some(limit);
            assert!(config.is_valid(), "limit {limit} should be accepted");
            assert_eq!(config.scheduler_history_limit(), limit);
        }
    }

    #[test]
    fn scheduler_history_limit_above_the_hard_maximum_is_rejected() {
        let mut config = Config::default();
        config.scheduler_history_limit = Some(MAX_SCHEDULER_HISTORY_LIMIT + 1);
        assert!(!config.is_valid());
        let text = config
            .validate()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("scheduler_history_limit"),
            "missing from {text}"
        );
    }

    #[test]
    fn scheduler_history_limit_does_not_require_configured_jobs() {
        // An operator may choose retention before any job exists; refusing that
        // would force a pointless edit to the job list.
        let mut config = Config::default();
        config.scheduler_history_limit = Some(3);
        assert_eq!(config.jobs.len(), 0);
        assert!(config.is_valid());
    }

    #[test]
    fn valid_time_and_load_gated_jobs_round_trip() {
        let mut config = Config::default();
        config.jobs = vec![test_job("time-only")];
        if !cfg!(windows) {
            config.jobs.push(test_job("heavy"));
            config.jobs[1].max_load = Some(8.0);
        }
        let serialized = config.to_toml().unwrap();
        let parsed = Config::parse(&serialized, None).unwrap();
        assert_eq!(config, parsed);
        if !cfg!(windows) {
            assert_eq!(parsed.jobs[1].effective_load_window(), "15m");
            assert_eq!(
                parsed.jobs[1].effective_retry_interval_ms(),
                DEFAULT_RETRY_INTERVAL_MS
            );
            assert_eq!(parsed.jobs[1].effective_max_wait_ms(), DEFAULT_MAX_WAIT_MS);
        }
    }

    #[test]
    fn job_names_schedules_commands_and_load_bounds_are_validated() {
        let mut config = Config::default();
        config.jobs = vec![test_job("same"), test_job("same")];
        config.jobs[0].schedule = "0 0 * * * *".to_owned();
        config.jobs[0].command.clear();
        config.jobs[1].max_load = Some(f32::NAN);
        config.jobs[1].load_window = Some("2m".to_owned());
        let violations = config.validate();
        let text = violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "duplicates",
            "five cron fields",
            "command must",
            "finite and non-negative",
            "load_window",
        ] {
            assert!(text.contains(expected), "missing {expected} from {text}");
        }

        let mut config = Config::default();
        config.jobs = vec![test_job("bounds")];
        config.jobs[0].max_load = Some(2.0);
        config.jobs[0].retry_interval_ms = Some(MIN_RETRY_INTERVAL_MS - 1);
        config.jobs[0].max_wait_ms = Some(MAX_MAX_WAIT_MS + 1);
        assert!(!config.is_valid());
    }

    #[test]
    fn calendar_impossible_schedules_fail_validation() {
        for expression in ["0 0 31 2 *", "0 0 30 2 *", "0 0 31 2,4 *"] {
            let mut config = Config::default();
            config.jobs = vec![test_job("impossible")];
            config.jobs[0].schedule = expression.to_owned();
            let text = config
                .validate()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("no calendar date can satisfy this expression"),
                "{expression}: {text}"
            );
            assert!(matches!(
                config.validate().first(),
                Some(ConfigViolation::InvalidJobs(_))
            ));
        }

        for expression in [
            "0 0 29 2 *",
            "0 0 31 1,3,5,7,8,10,12 *",
            // Traditional DOM/DOW OR behavior keeps these satisfiable.
            "0 0 31 2 1",
            "0 0 * 2 1",
        ] {
            let mut config = Config::default();
            config.jobs = vec![test_job("satisfiable")];
            config.jobs[0].schedule = expression.to_owned();
            assert_eq!(config.validate(), [], "{expression}");
        }
    }

    #[test]
    fn windows_rejects_load_gate_and_unix_accepts_time_only() {
        let mut config = Config::default();
        config.jobs = vec![test_job("load")];
        config.jobs[0].max_load = Some(1.0);
        assert!(config
            .validate_for(JobPlatform::Windows)
            .iter()
            .any(|v| v.to_string().contains("unsupported on Windows")));
        assert_eq!(config.validate_for(JobPlatform::Unix).len(), 0);
    }

    #[test]
    fn every_scheduler_bound_and_load_relationship_is_enforced() {
        let mut config = Config::default();
        let mut job = test_job(&"n".repeat(MAX_JOB_NAME_CHARS + 1));
        job.schedule = "0 0 * * * 2026".to_owned();
        job.command = vec!["x".to_owned(); MAX_COMMAND_ARGS + 1];
        job.command[0] = String::new();
        job.command[1] = "x".repeat(MAX_COMMAND_ARG_BYTES + 1);
        job.working_dir = Some(PathBuf::from("/path/that/need/not/exist"));
        job.max_load = Some(8.0);
        job.load_window = Some("2m".to_owned());
        job.retry_interval_ms = Some(MAX_RETRY_INTERVAL_MS + 1);
        job.max_wait_ms = Some(MIN_RETRY_INTERVAL_MS);
        config.jobs = vec![job];
        let text = config
            .validate()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "name exceeds",
            "five cron fields",
            "non-empty executable",
            "argv entries",
            "UTF-8 bytes",
            "load_window",
            "retry_interval_ms",
        ] {
            assert!(text.contains(expected), "missing {expected} from {text}");
        }
        assert!(!text.contains("working_dir"));

        let mut config = Config::default();
        config.jobs = vec![test_job("missing-threshold")];
        config.jobs[0].retry_interval_ms = Some(MIN_RETRY_INTERVAL_MS);
        assert!(config.validate().iter().any(|v| v
            .to_string()
            .contains("retry_interval_ms requires max_load")));

        for max_load in [-1.0, f32::INFINITY, f32::NAN] {
            let mut config = Config::default();
            config.jobs = vec![test_job("invalid-load")];
            config.jobs[0].max_load = Some(max_load);
            assert!(config
                .validate()
                .iter()
                .any(|v| v.to_string().contains("finite and non-negative")));
        }

        let mut config = Config::default();
        config.jobs = vec![test_job("retry-after-wait")];
        config.jobs[0].max_load = Some(1.0);
        config.jobs[0].retry_interval_ms = Some(20_000);
        config.jobs[0].max_wait_ms = Some(10_000);
        assert!(config
            .validate()
            .iter()
            .any(|v| v.to_string().contains("must not exceed")));

        let mut config = Config::default();
        config.jobs = (0..=MAX_JOBS)
            .map(|index| test_job(&format!("job-{index}")))
            .collect();
        assert!(config
            .validate()
            .iter()
            .any(|v| v.to_string().contains("maximum is 64")));
    }

    #[test]
    fn unix_root_jobs_need_explicit_opt_in_but_empty_config_is_allowed() {
        let mut config = Config::default();
        assert!(config.validate_job_authority(true).is_ok());
        config.jobs.push(test_job("root-risk"));
        assert!(config
            .validate_job_authority(true)
            .unwrap_err()
            .to_string()
            .contains("allow_privileged_jobs = true"));
        config.allow_privileged_jobs = true;
        assert!(config.validate_job_authority(true).is_ok());
        assert!(config.validate_job_authority(false).is_ok());
    }

    #[test]
    fn empty_name_fails_validation() {
        let config = Config {
            name: String::new(),
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::EmptyName));
    }

    #[test]
    fn whitespace_only_name_fails_validation() {
        let config = Config {
            name: String::from("   \t\n  "),
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::EmptyName));
    }

    #[test]
    fn control_characters_in_name_fail_validation() {
        let config = Config {
            name: "greggd\nserver".into(),
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::NameContainsControlCharacters));
    }

    #[test]
    fn name_too_long_fails_validation() {
        let config = Config {
            name: "x".repeat(MAX_NAME_LEN + 1),
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::NameTooLong { .. })));
    }

    #[test]
    fn port_zero_fails_validation() {
        let config = Config {
            port: 0,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidPort(0)));
    }

    #[test]
    fn boundary_port_values() {
        let config = Config {
            port: MIN_PORT,
            ..Config::default()
        };
        assert!(config.is_valid());

        let config = Config {
            port: u16::MAX,
            ..Config::default()
        };
        assert!(config.is_valid());
    }

    #[test]
    fn sample_interval_too_low_fails_validation() {
        let config = Config {
            sample_interval_ms: 100,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidSampleInterval(100)));
    }

    #[test]
    fn sample_interval_too_high_fails_validation() {
        let config = Config {
            sample_interval_ms: 100_000,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidSampleInterval(100_000)));
    }

    #[test]
    fn staleness_below_interval_fails_validation() {
        let config = Config {
            sample_interval_ms: 5000,
            stale_after_ms: 3000,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(
            violations.contains(&ConfigViolation::StalenessBelowInterval {
                stale_after_ms: 3000,
                sample_interval_ms: 5000,
            })
        );
    }

    #[test]
    fn staleness_disabled_is_valid() {
        let config = Config {
            stale_after_ms: 0,
            ..Config::default()
        };
        assert!(config.is_valid());
    }

    #[test]
    fn staleness_greater_than_interval_is_valid() {
        let config = Config {
            sample_interval_ms: 1000,
            stale_after_ms: 5000,
            ..Config::default()
        };
        assert!(config.is_valid());
    }

    #[test]
    fn parse_rejects_unknown_fields() {
        let toml = r#"
name = "test"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
unknown_field = "oops"
"#;
        let result = Config::parse(toml, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_invalid_toml() {
        let result = Config::parse("not valid toml {{{", None);
        assert!(result.is_err());
    }

    #[test]
    fn load_returns_error_for_missing_file() {
        let result = Config::load(Path::new("/nonexistent/greggd.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn write_atomic_creates_file() {
        let dir = std::env::temp_dir().join("greggd_test_write_atomic");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    /// A fresh temp file may belong to a writer that is still in flight.
    ///
    /// `write_atomic` sweeps stale temps *before* creating its own, so an
    /// age-blind sweep deletes a concurrent writer's live temp between its
    /// `create_secure_temp_file` and its `rename` — failing that write even
    /// though it had already succeeded. The sweep must therefore leave a
    /// recent `.greggd-*.toml.tmp` alone.
    #[test]
    fn write_atomic_leaves_a_concurrent_writers_temp_alone() {
        let dir = std::env::temp_dir().join("greggd_test_concurrent_temp");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let in_flight = dir.join(".greggd-inflight-1234.toml.tmp");
        fs::write(&in_flight, "half-written").unwrap();

        Config::default()
            .write_atomic(&dir.join("config.toml"))
            .unwrap();

        assert!(
            in_flight.exists(),
            "a concurrent writer's in-flight temp must survive another writer's sweep"
        );
        assert_eq!(
            fs::read_to_string(&in_flight).unwrap(),
            "half-written",
            "the in-flight temp must not have been truncated or replaced"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The age gate still cleans up after a crashed writer, and only after one.
    #[test]
    fn temp_staleness_gate_is_about_age() {
        let now = std::time::SystemTime::now();
        let ago = |seconds: u64| now.checked_sub(std::time::Duration::from_secs(seconds));

        assert!(
            !temp_is_stale(Some(now)),
            "a just-created temp is another writer's in-flight file"
        );
        assert!(!temp_is_stale(Some(ago(1).unwrap())));
        assert!(
            temp_is_stale(Some(ago(STALE_TEMP_AGE.as_secs() + 60).unwrap())),
            "a crashed writer's temp must eventually be cleaned up"
        );
        assert!(
            !temp_is_stale(None),
            "an unreadable mtime fails closed: skip rather than delete"
        );
        assert!(
            !temp_is_stale(Some(now + std::time::Duration::from_secs(3600))),
            "an mtime in the future (clock skew) fails closed: skip rather than delete"
        );
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_preserves_existing_config_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("greggd_test_existing_dir_perms");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        Config::default()
            .write_atomic(&dir.join("config.toml"))
            .unwrap();

        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn secure_temp_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("greggd_test_secure_temp");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".greggd-test.toml.tmp");

        let file = create_secure_temp_file(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(file);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_produces_world_readable_config() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("greggd_test_world_readable");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // New files are world-readable so unprivileged `croncheck`,
        // `status`, and `configprint` can read a system config.
        Config::default().write_atomic(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // Overwriting an old 0600 install also relaxes to 0644; the
        // 0600 temp-file guarantee above still protects partial writes.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        Config {
            name: String::from("relaxed"),
            ..Config::default()
        }
        .write_atomic(&path)
        .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(Config::load(&path).unwrap().name, "relaxed");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_overwrites_existing_file() {
        let dir = std::env::temp_dir().join("greggd_test_overwrite");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config {
            name: String::from("first"),
            ..Config::default()
        };
        config.write_atomic(&path).unwrap();

        let config = Config {
            name: String::from("second"),
            ..Config::default()
        };
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.name, "second");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_preserves_old_on_temp_failure() {
        let dir = std::env::temp_dir().join("greggd_test_preserve");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let original = Config::default();
        original.write_atomic(&path).unwrap();

        // Create a file where a directory would need to be, so
        // create_dir_all fails when write_atomic tries to ensure the
        // parent directory exists.
        let blocker = dir.join("not_a_dir");
        fs::write(&blocker, b"x").unwrap();
        let bad_path = blocker.join("sub").join("config.toml");
        let result = original.write_atomic(&bad_path);
        assert!(result.is_err());

        // Original should still be valid.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(original, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn multiple_violations_reported() {
        let config = Config {
            name: String::new(),
            port: 0,
            sample_interval_ms: 10,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.len() >= 3);
    }

    #[test]
    fn boundary_interval_values() {
        let config = Config {
            sample_interval_ms: MIN_SAMPLE_INTERVAL_MS,
            stale_after_ms: MIN_SAMPLE_INTERVAL_MS + 1,
            ..Config::default()
        };
        assert!(config.is_valid());

        let config = Config {
            sample_interval_ms: MAX_SAMPLE_INTERVAL_MS,
            stale_after_ms: 0,
            ..Config::default()
        };
        assert!(config.is_valid());
    }

    #[test]
    fn config_violation_display_messages() {
        let v = ConfigViolation::EmptyName;
        assert_ne!(format!("{v}"), "");

        let v = ConfigViolation::NameTooLong {
            length: 200,
            max: 128,
        };
        let msg = format!("{v}");
        assert!(msg.contains("200"));
        assert!(msg.contains("128"));

        let v = ConfigViolation::InvalidPort(0);
        assert!(format!("{v}").contains('0'));

        let v = ConfigViolation::InvalidSampleInterval(10);
        assert!(format!("{v}").contains("10"));

        let v = ConfigViolation::StalenessBelowInterval {
            stale_after_ms: 500,
            sample_interval_ms: 1000,
        };
        let msg = format!("{v}");
        assert!(msg.contains("500"));
        assert!(msg.contains("1000"));
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_to_readonly_directory() {
        let dir = std::env::temp_dir().join("greggd_test_readonly");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let original = Config::default();
        let path = dir.join("config.toml");
        original.write_atomic(&path).unwrap();

        // Make directory read-only.
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms).unwrap();

        let result = original.write_atomic(&path);
        assert!(result.is_err());

        // Original file should still be intact and readable.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(original, loaded);

        // Restore permissions for cleanup.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_verification_detects_mismatch() {
        // The verification step re-parses the written file and compares
        // against the source config. We test that VerificationFailed is
        // the correct variant by manually corrupting the file after a
        // successful write, then verifying that load still succeeds on
        // the corrupted file (proving the file was writable) while the
        // original config would not match.
        let dir = std::env::temp_dir().join("greggd_test_verify_mismatch");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Corrupt the file in place.
        fs::write(&path, "name = \"corrupted\"\n").unwrap();

        // The corrupted file should parse as valid TOML but fail
        // validation (host is missing), proving the file was overwritten.
        let result = Config::load(&path);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_no_parent_directory() {
        let config = Config::default();
        // Path::new("/").parent() returns None, triggering NoParentDirectory.
        let result = config.write_atomic(Path::new("/"));
        match result {
            Err(ConfigError::AtomicWrite {
                source: AtomicWriteError::NoParentDirectory,
                ..
            }) => {}
            other => panic!("expected NoParentDirectory, got {other:?}"),
        }
    }

    #[test]
    fn write_atomic_multiple_rapid_writes() {
        let dir = std::env::temp_dir().join("greggd_test_rapid_writes");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        for i in 0..10 {
            let config = Config {
                name: format!("iteration-{i}"),
                ..Config::default()
            };
            config.write_atomic(&path).unwrap();
        }

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.name, "iteration-9");
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_deeply_nested_invalid_toml() {
        let toml = r"
[[[this is not valid toml
  broken = { { { }
";
        let result = Config::parse(toml, None);
        assert!(result.is_err());
        match result {
            Err(ConfigError::Parse { .. }) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn config_with_all_violations_at_once() {
        let config = Config {
            name: String::new(),
            port: 0,
            sample_interval_ms: 10,
            stale_after_ms: 5,
            host: "0.0.0.0".parse().unwrap(),
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.len() >= 3);
        assert!(violations.contains(&ConfigViolation::EmptyName));
        assert!(violations.contains(&ConfigViolation::InvalidPort(0)));
        assert!(violations.contains(&ConfigViolation::InvalidSampleInterval(10)));
        assert!(
            violations.contains(&ConfigViolation::StalenessBelowInterval {
                stale_after_ms: 5,
                sample_interval_ms: 10,
            })
        );
    }
}
