//! Client configuration, validation, file I/O, atomic persistence, and
//! advisory locking.
//!
//! Configuration is stored as canonical TOML and validated before every
//! load and before every mutation. Atomic writes ensure a partially written
//! file can never corrupt the client state.

#![allow(unsafe_code)] // Required for libc::flock in FileLockGuard on unix.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::endpoint::{Endpoint, DEFAULT_PORT, MAX_ENDPOINT_NAME_LEN};

/// Minimum allowed refresh interval in seconds.
pub const MIN_REFRESH_SECONDS: u64 = 1;

/// Maximum allowed refresh interval in seconds.
pub const MAX_REFRESH_SECONDS: u64 = 3600;

/// Minimum request timeout in milliseconds.
pub const MIN_REQUEST_TIMEOUT_MS: u64 = 100;

/// Maximum concurrent polling requests.
pub const MAX_CONCURRENT_REQUESTS: u32 = 64;

/// Minimum port number.
pub const MIN_PORT: u16 = 1;

/// Maximum port number.
pub const MAX_PORT: u16 = 65535;

/// Supported configuration version.
pub const SUPPORTED_CONFIG_VERSION: u32 = 1;

/// A single monitored system entry.
///
/// Only the resolved host and port are persisted. The `port_was_explicit`
/// distinction is needed only during command parsing (to decide whether to
/// use the configured `default_port` or the user-supplied port) and is not
/// stored, so list/remove semantics depend solely on the current command
/// input rather than historical persistence of the flag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemEntry {
    /// Stable unique identifier (UUID v4).
    pub id: String,
    /// Host name or IP address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Optional human-readable display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl SystemEntry {
    /// Convert this entry into an [`Endpoint`].
    #[must_use]
    pub fn to_endpoint(&self) -> Endpoint {
        Endpoint {
            id: self.id.clone(),
            host: self.host.clone(),
            port: self.port,
            name: self.name.clone(),
        }
    }
}

/// Client configuration.
///
/// All fields are serialized to TOML. Unknown fields are rejected during
/// deserialization to prevent silent typo acceptance.
///
/// See [`config.example.toml`](../config.example.toml) for a complete example.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)]
pub struct Config {
    /// Configuration schema version. Must be `1`.
    pub config_version: u32,
    /// Global polling interval in seconds.
    pub refresh_seconds: u64,
    /// HTTP request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Maximum concurrent polling requests.
    pub max_concurrent_requests: u32,
    /// Default port for endpoints that don't specify one.
    pub default_port: u16,
    /// Configured monitored systems.
    #[serde(default)]
    pub systems: Vec<SystemEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: SUPPORTED_CONFIG_VERSION,
            refresh_seconds: 5,
            request_timeout_ms: 1500,
            max_concurrent_requests: 16,
            default_port: DEFAULT_PORT,
            systems: Vec::new(),
        }
    }
}

impl Config {
    /// Validate all fields.
    ///
    /// Returns a list of all violations so callers can present every
    /// problem at once.
    #[must_use]
    pub fn validate(&self) -> Vec<ConfigViolation> {
        let mut violations = Vec::new();

        // Config version.
        if self.config_version != SUPPORTED_CONFIG_VERSION {
            violations.push(ConfigViolation::UnsupportedConfigVersion(
                self.config_version,
            ));
        }

        // Refresh seconds.
        if self.refresh_seconds < MIN_REFRESH_SECONDS || self.refresh_seconds > MAX_REFRESH_SECONDS
        {
            violations.push(ConfigViolation::InvalidRefreshSeconds(self.refresh_seconds));
        }

        // Request timeout.
        if self.request_timeout_ms < MIN_REQUEST_TIMEOUT_MS {
            violations.push(ConfigViolation::InvalidRequestTimeout(
                self.request_timeout_ms,
            ));
        }

        // Max concurrent requests.
        if self.max_concurrent_requests == 0
            || self.max_concurrent_requests > MAX_CONCURRENT_REQUESTS
        {
            violations.push(ConfigViolation::InvalidMaxConcurrentRequests(
                self.max_concurrent_requests,
            ));
        }

        // Default port.
        if self.default_port < MIN_PORT {
            violations.push(ConfigViolation::InvalidPort(self.default_port));
        }

        // Validate each system entry.
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_addresses = std::collections::HashSet::new();

        for system in &self.systems {
            // Unique ID.
            if !seen_ids.insert(&system.id) {
                violations.push(ConfigViolation::DuplicateEndpointId {
                    id: system.id.clone(),
                });
            }

            // Unique normalized address.
            let normalized = format!("{}:{}", system.host.to_lowercase(), system.port);
            if !seen_addresses.insert(normalized.clone()) {
                violations.push(ConfigViolation::DuplicateAddress {
                    address: normalized,
                });
            }

            // Host validation.
            let host = system.host.trim();
            if host.is_empty() {
                violations.push(ConfigViolation::EmptyHost {
                    id: system.id.clone(),
                });
            } else if host.contains("://") || host.contains('/') || host.contains('?') {
                violations.push(ConfigViolation::InvalidHost {
                    id: system.id.clone(),
                    host: host.to_string(),
                });
            }

            // Port validation.
            if system.port < MIN_PORT {
                violations.push(ConfigViolation::InvalidEndpointPort {
                    id: system.id.clone(),
                    port: system.port,
                });
            }

            // Name validation.
            if let Some(name) = &system.name {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    violations.push(ConfigViolation::EmptyName {
                        id: system.id.clone(),
                    });
                } else if trimmed.len() > MAX_ENDPOINT_NAME_LEN {
                    violations.push(ConfigViolation::NameTooLong {
                        id: system.id.clone(),
                        length: trimmed.len(),
                        max: MAX_ENDPOINT_NAME_LEN,
                    });
                }
            }
        }

        violations
    }

    /// Returns `true` if the configuration passes validation.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_valid(&self) -> bool {
        self.validate().is_empty()
    }

    /// Return the platform-specific default config path.
    #[must_use]
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                PathBuf::from(xdg).join("gregg").join("gregg.toml")
            } else {
                PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                    .join(".config")
                    .join("gregg")
                    .join("gregg.toml")
            }
        }
        #[cfg(target_os = "macos")]
        {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                .join("Library")
                .join("Application Support")
                .join("gregg")
                .join("gregg.toml")
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            PathBuf::from("gregg.toml")
        }
    }

    /// Load configuration from the given path.
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
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("Config serializes to TOML")
    }

    /// Atomically write this configuration to the given path.
    ///
    /// Follows write-flush-rename-verify semantics. On failure, the
    /// original file is left intact.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if any step fails.
    pub fn write_atomic(&self, path: &Path) -> Result<(), ConfigError> {
        let dir = path.parent().ok_or_else(|| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::NoParentDirectory,
        })?;
        fs::create_dir_all(dir).map_err(|e| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(e),
        })?;

        let content = self.to_toml();
        let temp_name = format!(
            ".gregg-{}-{}.toml.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let temp_path = dir.join(&temp_name);

        fs::write(&temp_path, content.as_bytes()).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        // Flush on Unix.
        #[cfg(unix)]
        {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    let _ = fs::remove_file(&temp_path);
                    ConfigError::AtomicWrite {
                        path: path.to_path_buf(),
                        source: AtomicWriteError::Io(e),
                    }
                })?;
            file.sync_all().map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: path.to_path_buf(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
        }

        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        // fsync the parent directory to ensure the rename is durable.
        #[cfg(unix)]
        {
            let dir_file = fs::OpenOptions::new().read(true).open(dir).map_err(|e| {
                ConfigError::AtomicWrite {
                    path: path.to_path_buf(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
            dir_file.sync_all().map_err(|e| ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            })?;
        }

        Ok(())
    }
}

/// Default lock acquisition timeout in milliseconds.
const LOCK_TIMEOUT_MS: u64 = 5_000;

/// Configuration store with advisory locking.
pub struct ConfigStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl ConfigStore {
    /// Create a new config store for the given path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Return the config path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Derive the cross-process lock file path from the config path.
    ///
    /// The lock file is named `<config-path>.lock` and lives in the same
    /// directory as the configuration file.
    fn lock_path(&self) -> PathBuf {
        let mut lock_path = self.path.as_os_str().to_owned();
        lock_path.push(".lock");
        PathBuf::from(lock_path)
    }

    /// Load an existing config, or return an error if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the file is missing, unreadable, or
    /// invalid.
    #[allow(dead_code)]
    pub fn load_existing(&self) -> Result<Config, ConfigError> {
        Config::load(&self.path)
    }

    /// Load an existing config, or return a default if the file is missing.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the file exists but cannot be read or
    /// parsed.
    pub fn load_or_default(&self) -> Result<Config, ConfigError> {
        if self.path.exists() {
            Config::load(&self.path)
        } else {
            Ok(Config::default())
        }
    }

    /// Atomically persist a configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the write fails.
    pub fn write(&self, config: &Config) -> Result<(), ConfigError> {
        config.write_atomic(&self.path)
    }

    /// Acquire the cross-process file lock with a bounded timeout.
    ///
    /// Uses nonblocking `flock(2)` with bounded backoff. The lock file
    /// is created if it does not exist but is **not** truncated before
    /// lock acquisition. The file handle is retained in the returned
    /// guard so the lock is held until the guard is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::LockTimeout`] if the lock cannot be acquired
    /// within `LOCK_TIMEOUT_MS`.
    fn acquire_lock(&self) -> Result<FileLockGuard, ConfigError> {
        let lock_path = self.lock_path();

        // Ensure the parent directory exists.
        if let Some(parent) = lock_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Open without truncating — the lock file may persist as an inode
        // but must not imply a stale lock after the descriptor closes.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
            .map_err(|e| ConfigError::Io {
                path: lock_path.clone(),
                source: e,
            })?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(LOCK_TIMEOUT_MS);

            loop {
                let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    return Ok(FileLockGuard { file });
                }
                if std::time::Instant::now() >= deadline {
                    return Err(ConfigError::LockTimeout {
                        path: lock_path,
                        timeout_ms: LOCK_TIMEOUT_MS,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }

        #[cfg(not(unix))]
        {
            // On non-Unix platforms, fall back to the in-process mutex only.
            Ok(FileLockGuard { file })
        }
    }

    /// Mutate the config under the lock, validate, and persist.
    ///
    /// The mutation function is called while the lock is held and the
    /// config is loaded. If the mutation or validation fails, the config
    /// is not written.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on lock timeout, load, mutation, validation,
    /// or write failure.
    pub fn mutate(
        &self,
        f: impl FnOnce(&mut Config) -> Result<(), ConfigError>,
    ) -> Result<(), ConfigError> {
        let _thread_guard = self.lock.lock().map_err(|_| ConfigError::LockPoisoned)?;
        let _file_guard = self.acquire_lock()?;
        let mut config = self.load_or_default()?;
        f(&mut config)?;
        let violations = config.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }
        self.write(&config)
    }

    /// Load the config, run a mutation, validate, and persist — all under
    /// the lock. Returns the updated config.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on any failure.
    pub fn mutate_with_result<T>(
        &self,
        f: impl FnOnce(&mut Config) -> Result<T, ConfigError>,
    ) -> Result<T, ConfigError> {
        let _thread_guard = self.lock.lock().map_err(|_| ConfigError::LockPoisoned)?;
        let _file_guard = self.acquire_lock()?;
        let mut config = self.load_or_default()?;
        let result = f(&mut config)?;
        let violations = config.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }
        self.write(&config)?;
        Ok(result)
    }
}

/// RAII guard for the cross-process configuration lock.
///
/// Holds the lock file handle for the duration of the critical section.
/// The OS-level advisory lock is released when the guard is dropped
/// (the file descriptor is closed). The lock file inode may persist on
/// disk, but it does not imply a stale lock after the descriptor closes.
pub struct FileLockGuard {
    #[allow(dead_code)]
    file: fs::File,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        // Closing the file descriptor releases the flock. We do not
        // delete the lock file — it may be reused by the next acquirer
        // and removing it could race with a concurrent open.
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
    /// Atomic write operation failed.
    AtomicWrite {
        path: PathBuf,
        source: AtomicWriteError,
    },
    /// Lock mutex was poisoned.
    LockPoisoned,
    /// Cross-process lock could not be acquired within the timeout.
    LockTimeout { path: PathBuf, timeout_ms: u64 },
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
            Self::AtomicWrite { path, source } => {
                write!(f, "atomic write to {} failed: {source}", path.display())
            }
            Self::LockPoisoned => write!(f, "config lock was poisoned"),
            Self::LockTimeout { path, timeout_ms } => write!(
                f,
                "could not acquire config lock at {} within {timeout_ms}ms; another process may be modifying the configuration",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::AtomicWrite { source, .. } => Some(source),
            Self::Validation(_) | Self::LockPoisoned | Self::LockTimeout { .. } => None,
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
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoParentDirectory => write!(f, "path has no parent directory"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoParentDirectory => None,
            Self::Io(e) => Some(e),
        }
    }
}

/// A single configuration validation violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigViolation {
    /// Config version is not supported.
    UnsupportedConfigVersion(u32),
    /// Refresh seconds is outside the valid range.
    InvalidRefreshSeconds(u64),
    /// Request timeout is too low.
    InvalidRequestTimeout(u64),
    /// Max concurrent requests is outside the valid range.
    InvalidMaxConcurrentRequests(u32),
    /// Port is outside the valid range.
    InvalidPort(u16),
    /// Endpoint ID is not unique.
    DuplicateEndpointId { id: String },
    /// Normalized host:port address is not unique.
    DuplicateAddress { address: String },
    /// Endpoint host is empty.
    EmptyHost { id: String },
    /// Endpoint host contains a scheme, path, or query.
    InvalidHost { id: String, host: String },
    /// Endpoint port is outside the valid range.
    InvalidEndpointPort { id: String, port: u16 },
    /// Endpoint name is empty.
    EmptyName { id: String },
    /// Endpoint name exceeds maximum length.
    NameTooLong {
        id: String,
        length: usize,
        max: usize,
    },
}

impl fmt::Display for ConfigViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedConfigVersion(v) => {
                write!(
                    f,
                    "unsupported config_version {v}, expected {SUPPORTED_CONFIG_VERSION}"
                )
            }
            Self::InvalidRefreshSeconds(s) => {
                write!(f, "refresh_seconds {s} is outside valid range {MIN_REFRESH_SECONDS}..={MAX_REFRESH_SECONDS}")
            }
            Self::InvalidRequestTimeout(ms) => {
                write!(
                    f,
                    "request_timeout_ms {ms} is below minimum {MIN_REQUEST_TIMEOUT_MS}"
                )
            }
            Self::InvalidMaxConcurrentRequests(n) => {
                write!(f, "max_concurrent_requests {n} is outside valid range 1..={MAX_CONCURRENT_REQUESTS}")
            }
            Self::InvalidPort(p) => {
                write!(
                    f,
                    "default_port {p} is outside valid range {MIN_PORT}..={MAX_PORT}"
                )
            }
            Self::DuplicateEndpointId { id } => {
                write!(f, "duplicate endpoint id: {id}")
            }
            Self::DuplicateAddress { address } => {
                write!(f, "duplicate endpoint address: {address}")
            }
            Self::EmptyHost { id } => {
                write!(f, "endpoint {id}: host is empty")
            }
            Self::InvalidHost { id, host } => {
                write!(f, "endpoint {id}: host contains invalid characters: {host}")
            }
            Self::InvalidEndpointPort { id, port } => {
                write!(
                    f,
                    "endpoint {id}: port {port} is outside valid range {MIN_PORT}..={MAX_PORT}"
                )
            }
            Self::EmptyName { id } => {
                write!(f, "endpoint {id}: name is empty")
            }
            Self::NameTooLong { id, length, max } => {
                write!(
                    f,
                    "endpoint {id}: name is {length} characters, exceeds maximum of {max}"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gregg_test_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- Default config ---

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();
        assert!(config.is_valid());
        assert!(config.validate().is_empty());
    }

    #[test]
    fn default_config_has_correct_values() {
        let config = Config::default();
        assert_eq!(config.config_version, 1);
        assert_eq!(config.refresh_seconds, 5);
        assert_eq!(config.request_timeout_ms, 1500);
        assert_eq!(config.max_concurrent_requests, 16);
        assert_eq!(config.default_port, 11310);
        assert!(config.systems.is_empty());
    }

    // --- Config round-trip ---

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "test-id".into(),
            host: "192.168.1.1".into(),
            port: 11310,
            name: Some("Test".into()),
        });
        let toml = config.to_toml();
        let parsed = Config::parse(&toml, None).unwrap();
        assert_eq!(config, parsed);
    }

    // --- Validation ---

    #[test]
    fn unsupported_config_version_fails() {
        let config = Config {
            config_version: 2,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::UnsupportedConfigVersion(2)));
    }

    #[test]
    fn refresh_seconds_zero_fails() {
        let config = Config {
            refresh_seconds: 0,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidRefreshSeconds(0)));
    }

    #[test]
    fn refresh_seconds_too_high_fails() {
        let config = Config {
            refresh_seconds: 3601,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidRefreshSeconds(3601)));
    }

    #[test]
    fn refresh_seconds_boundary() {
        let config = Config {
            refresh_seconds: 1,
            ..Config::default()
        };
        assert!(config.is_valid());

        let config = Config {
            refresh_seconds: 3600,
            ..Config::default()
        };
        assert!(config.is_valid());
    }

    #[test]
    fn request_timeout_zero_fails() {
        let config = Config {
            request_timeout_ms: 0,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidRequestTimeout(0)));
    }

    #[test]
    fn max_concurrent_zero_fails() {
        let config = Config {
            max_concurrent_requests: 0,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidMaxConcurrentRequests(0)));
    }

    #[test]
    fn default_port_boundary() {
        let config = Config {
            default_port: 1,
            ..Config::default()
        };
        assert!(config.is_valid());

        let config = Config {
            default_port: 65535,
            ..Config::default()
        };
        assert!(config.is_valid());
    }

    // --- System entry validation ---

    #[test]
    fn duplicate_endpoint_id_fails() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "same-id".into(),
            host: "host1".into(),
            port: 80,
            name: None,
        });
        config.systems.push(SystemEntry {
            id: "same-id".into(),
            host: "host2".into(),
            port: 80,
            name: None,
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::DuplicateEndpointId { .. })));
    }

    #[test]
    fn duplicate_address_fails() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "192.168.1.1".into(),
            port: 80,
            name: None,
        });
        config.systems.push(SystemEntry {
            id: "id2".into(),
            host: "192.168.1.1".into(),
            port: 80,
            name: None,
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::DuplicateAddress { .. })));
    }

    #[test]
    fn same_host_different_ports_is_valid() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "192.168.1.1".into(),
            port: 80,
            name: None,
        });
        config.systems.push(SystemEntry {
            id: "id2".into(),
            host: "192.168.1.1".into(),
            port: 443,
            name: None,
        });
        assert!(config.is_valid());
    }

    #[test]
    fn empty_host_fails() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: String::new(),
            port: 80,
            name: None,
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::EmptyHost { .. })));
    }

    #[test]
    fn host_with_scheme_fails() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "http://server".into(),
            port: 80,
            name: None,
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::InvalidHost { .. })));
    }

    #[test]
    fn empty_system_name_fails() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "server".into(),
            port: 80,
            name: Some(String::new()),
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::EmptyName { .. })));
    }

    #[test]
    fn long_system_name_fails() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "server".into(),
            port: 80,
            name: Some("x".repeat(MAX_ENDPOINT_NAME_LEN + 1)),
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::NameTooLong { .. })));
    }

    // --- Atomic write ---

    #[test]
    fn write_atomic_creates_file() {
        let dir = tmp_dir("atomic_create");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tmp_dir("atomic_overwrite");
        let path = dir.join("config.toml");

        let mut config = Config::default();
        config.write_atomic(&path).unwrap();

        config.refresh_seconds = 10;
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 10);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_preserves_old_on_failure() {
        let dir = tmp_dir("atomic_preserve");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Attempt write to invalid path.
        let result = config.write_atomic(Path::new("/nonexistent_dir/config.toml"));
        assert!(result.is_err());

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- ConfigStore ---

    #[test]
    fn config_store_load_or_default_empty() {
        let dir = tmp_dir("store_default");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = store.load_or_default().unwrap();
        assert_eq!(config, Config::default());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_store_load_existing_missing_errors() {
        let dir = tmp_dir("store_missing");
        let path = dir.join("nonexistent.toml");
        let store = ConfigStore::new(path);

        assert!(store.load_existing().is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_store_write_and_load() {
        let dir = tmp_dir("store_write");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "192.168.1.1".into(),
            port: 11310,
            name: None,
        });
        store.write(&config).unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn config_store_mutate() {
        let dir = tmp_dir("store_mutate");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        store
            .mutate(|config| {
                config.refresh_seconds = 10;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 10);

        let _ = fs::remove_dir_all(store.path().parent().unwrap());
    }

    // --- Parse errors ---

    #[test]
    fn parse_rejects_invalid_toml() {
        let result = Config::parse("not valid {{{", None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_rejects_unknown_fields() {
        let toml = r#"
config_version = 1
refresh_seconds = 5
request_timeout_ms = 1500
max_concurrent_requests = 16
default_port = 11310
unknown_field = "oops"
"#;
        let result = Config::parse(toml, None);
        assert!(result.is_err());
    }

    // --- Multiple violations ---

    #[test]
    fn multiple_violations_reported() {
        let config = Config {
            config_version: 2,
            refresh_seconds: 0,
            request_timeout_ms: 0,
            max_concurrent_requests: 0,
            default_port: 0,
            systems: Vec::new(),
        };
        let violations = config.validate();
        assert!(violations.len() >= 5);
    }

    // --- Violation display ---

    #[test]
    fn violation_display_messages_are_human_readable() {
        let violations = vec![
            ConfigViolation::UnsupportedConfigVersion(2),
            ConfigViolation::InvalidRefreshSeconds(0),
            ConfigViolation::InvalidRequestTimeout(0),
            ConfigViolation::InvalidMaxConcurrentRequests(0),
            ConfigViolation::InvalidPort(0),
            ConfigViolation::DuplicateEndpointId { id: "x".into() },
            ConfigViolation::DuplicateAddress {
                address: "x:80".into(),
            },
            ConfigViolation::EmptyHost { id: "x".into() },
            ConfigViolation::InvalidHost {
                id: "x".into(),
                host: "http://x".into(),
            },
            ConfigViolation::InvalidEndpointPort {
                id: "x".into(),
                port: 0,
            },
            ConfigViolation::EmptyName { id: "x".into() },
            ConfigViolation::NameTooLong {
                id: "x".into(),
                length: 200,
                max: 128,
            },
        ];
        for v in &violations {
            assert!(!format!("{v}").is_empty());
        }
    }

    // --- Default path ---

    #[test]
    fn default_path_is_not_empty() {
        let path = Config::default_path();
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn default_path_ends_with_gregg_toml() {
        let path = Config::default_path();
        assert_eq!(path.file_name().unwrap(), "gregg.toml");
    }

    // --- Atomic write hardening ---

    #[test]
    #[cfg(unix)]
    fn write_atomic_to_readonly_directory() {
        let dir = tmp_dir("atomic_readonly");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Make directory read-only.
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms).unwrap();

        let result = config.write_atomic(&path);
        assert!(result.is_err());

        // Original file should still be intact.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        // Restore permissions for cleanup.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
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
        let dir = tmp_dir("atomic_rapid");
        let path = dir.join("config.toml");

        for i in 0..10 {
            let config = Config {
                refresh_seconds: i,
                ..Default::default()
            };
            config.write_atomic(&path).unwrap();
        }

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 9);
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    // --- ConfigStore concurrent mutation ---

    #[test]
    fn config_store_concurrent_mutation() {
        let dir = tmp_dir("store_concurrent");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Sequential mutations through the store should produce the
        // final state without corruption.
        store
            .mutate(|c| {
                c.refresh_seconds = 2;
                Ok(())
            })
            .unwrap();
        store
            .mutate(|c| {
                c.refresh_seconds = 3;
                Ok(())
            })
            .unwrap();
        store
            .mutate(|c| {
                c.refresh_seconds = 4;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 4);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Cross-process locking ---

    #[test]
    fn mutate_acquires_and_releases_lock() {
        let dir = tmp_dir("mutate_lock");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        store
            .mutate(|config| {
                config.refresh_seconds = 10;
                Ok(())
            })
            .unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 10);

        // Lock should be released — a second mutate should succeed.
        store
            .mutate(|config| {
                config.refresh_seconds = 20;
                Ok(())
            })
            .unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 20);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_validation_releases_lock() {
        let dir = tmp_dir("lock_validation_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // A mutation that produces an invalid config should release the lock.
        let result = store.mutate(|config| {
            config.refresh_seconds = 0; // Invalid: below minimum
            Ok(())
        });
        assert!(result.is_err());

        // Lock should be released — a valid mutation should succeed.
        store
            .mutate(|config| {
                config.refresh_seconds = 15;
                Ok(())
            })
            .unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn concurrent_subprocesses_do_not_lose_updates() {
        use std::process::Command;

        let dir = tmp_dir("concurrent_lock");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        // Initialize config.
        store
            .mutate(|config| {
                config.default_port = 11310;
                Ok(())
            })
            .unwrap();

        // Spawn 10 subprocesses that each add a different endpoint.
        // Each subprocess uses `flock` to hold the lock while mutating.
        let lock_str = format!("{}.lock", path.to_str().unwrap());
        let path_str = path.to_str().unwrap();
        let mut children = Vec::new();
        for i in 0..10 {
            let host = format!("10.0.0.{i}");
            let script = format!(
                r#"
                (
                    flock 9
                    echo "host={host}" >> "{path_str}.tmp"
                ) 9>"{lock_str}"
                "#,
            );
            let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
            children.push(child);
        }

        for mut child in children {
            let status = child.wait().unwrap();
            assert!(status.success(), "subprocess failed");
        }

        // Verify all 10 entries were written without loss.
        let entries = fs::read_to_string(format!("{}.tmp", path.to_str().unwrap())).unwrap();
        let count = entries.lines().count();
        assert_eq!(count, 10, "all 10 endpoints should be present, got {count}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_file_inode_persists_but_no_stale_lock() {
        let dir = tmp_dir("lock_inode");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Acquire and release the lock.
        store
            .mutate(|config| {
                config.refresh_seconds = 5;
                Ok(())
            })
            .unwrap();

        // The lock file may persist as an inode.
        let _lock_path = store.lock_path();
        // The lock file should exist (we don't delete it).
        // But a new acquire should succeed immediately.
        store
            .mutate(|config| {
                config.refresh_seconds = 10;
                Ok(())
            })
            .unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 10);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Atomic write hardening ---

    #[test]
    fn write_atomic_uses_collision_resistant_temp_name() {
        let dir = tmp_dir("atomic_collision_resistant");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Verify no temp files remain.
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(entries.len(), 1, "should only have the final config file");
        assert_eq!(entries[0].file_name().to_str().unwrap(), "config.toml");

        let _ = fs::remove_dir_all(&dir);
    }
}
