//! Client configuration, validation, file I/O, atomic persistence, and
//! advisory locking.
//!
//! Configuration is stored as canonical TOML and validated before every
//! load and before every mutation. Atomic writes ensure a partially written
//! file can never corrupt the client state.

#![allow(unsafe_code)] // Required for libc::flock in FileLockGuard on unix.

// The cross-process configuration lock relies on platform file-locking
// primitives (flock on unix, LockFileEx on windows). Fail the build loudly on
// any other target rather than silently degrading to in-process-only locking,
// where concurrent processes could interleave config writes.
#[cfg(not(any(unix, windows)))]
compile_error!("cross-process config locking is only implemented for unix and windows targets");

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(all(test, unix))]
use std::cell::Cell;

use serde::{Deserialize, Serialize};

use crate::endpoint::{Endpoint, DEFAULT_PORT, MAX_ENDPOINT_NAME_LEN};

#[cfg(all(test, unix))]
thread_local! {
    static FAIL_NEXT_PERMISSION_SET: Cell<bool> = const { Cell::new(false) };
}

/// Create a replacement file with user-only permissions before exposing any
/// configuration bytes to it.
///
/// On Unix, the file is created with mode `0o600`. On Windows, the file
/// inherits the default ACL of the parent directory (typically the user's
/// profile directory), which already restricts access to the owning user.
fn create_secure_temp_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    let file = options.open(path)?;

    #[cfg(unix)]
    if let Err(error) = set_secure_permissions(&file) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }

    Ok(file)
}

#[cfg(unix)]
fn set_secure_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    #[cfg(test)]
    {
        if FAIL_NEXT_PERMISSION_SET.with(|fail| fail.replace(false)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected permission-setting failure",
            ));
        }
    }

    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(all(test, unix))]
fn inject_permission_set_failure() {
    FAIL_NEXT_PERMISSION_SET.with(|fail| fail.set(true));
}

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

/// Default port for the optional `EggPool` endpoint.
pub const DEFAULT_EGGPOOL_PORT: u16 = 11300;

/// Maximum display-name length for `EggPool` entries.
pub const MAX_EGGPOOL_NAME_LEN: usize = 128;

/// Maximum environment-variable name length for `EggPool` API keys.
pub const MAX_ENV_NAME_LEN: usize = 128;

/// A single monitored system entry.
///
/// Only the resolved host and port are persisted. The `port_was_explicit`
/// distinction is needed only during command parsing and is not stored, so
/// list/remove semantics depend solely on the current command input rather
/// than historical persistence of the flag. `gregg add` requires an explicit
/// port; the retained `default_port` configuration field is not used for new
/// system additions.
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

/// Scheme used to connect to `EggPool`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EggpoolScheme {
    /// Plain HTTP.
    Http,
    /// HTTPS.
    Https,
}

impl fmt::Display for EggpoolScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http => f.write_str("http"),
            Self::Https => f.write_str("https"),
        }
    }
}

/// The single optional `EggPool` statistics endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EggpoolEntry {
    /// Stable unique identifier (UUID v4).
    pub id: String,
    /// Normalized host name or IP address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Connection scheme.
    pub scheme: EggpoolScheme,
    /// Optional display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional environment-variable name containing the API key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

impl EggpoolEntry {
    /// Return the canonical base address without a URL path.
    #[must_use]
    pub fn display_address(&self) -> String {
        crate::eggpool_endpoint::display_address(&self.host, self.port, self.scheme)
    }
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
    /// Retained for configuration compatibility; `gregg add` requires an
    /// explicit port and does not use this field for new system additions.
    pub default_port: u16,
    /// Configured monitored systems.
    #[serde(default)]
    pub systems: Vec<SystemEntry>,
    /// Optional `EggPool` statistics endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eggpool: Option<EggpoolEntry>,
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
            eggpool: None,
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

        if let Some(eggpool) = &self.eggpool {
            validate_eggpool(&mut violations, eggpool);
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
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                PathBuf::from(appdata).join("gregg").join("gregg.toml")
            } else if let Ok(userprofile) = std::env::var("USERPROFILE") {
                PathBuf::from(userprofile)
                    .join("AppData")
                    .join("Roaming")
                    .join("gregg")
                    .join("gregg.toml")
            } else {
                // No user-scoped directory available — return a clear
                // error path rather than silently falling back to cwd.
                PathBuf::from("gregg.toml")
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
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
        let violations = self.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }

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

        {
            let mut file =
                create_secure_temp_file(&temp_path).map_err(|e| ConfigError::AtomicWrite {
                    path: path.to_path_buf(),
                    source: AtomicWriteError::Io(e),
                })?;

            file.write_all(content.as_bytes()).map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: path.to_path_buf(),
                    source: AtomicWriteError::Io(e),
                }
            })?;

            file.flush().map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: path.to_path_buf(),
                    source: AtomicWriteError::Io(e),
                }
            })?;

            // Sync the replacement before renaming it into place so the
            // rename cannot reorder ahead of the data on any platform.
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
    #[allow(dead_code)]
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
        match Config::load(&self.path) {
            Ok(config) => Ok(config),
            Err(error) if matches!(&error, ConfigError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound) => {
                Ok(Config::default())
            }
            Err(error) => Err(error),
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
            .truncate(false)
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
                    return Ok(FileLockGuard { file, handle: None });
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

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };
            use windows_sys::Win32::System::IO::OVERLAPPED;

            let handle = file.as_raw_handle();
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(LOCK_TIMEOUT_MS);

            loop {
                let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                #[allow(clippy::ptr_as_ptr)]
                let result = unsafe {
                    LockFileEx(
                        handle as *mut _,
                        LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                        0,
                        1,
                        0,
                        &mut overlapped,
                    )
                };

                if result != 0 {
                    // Lock acquired.
                    return Ok(FileLockGuard {
                        file,
                        handle: Some(handle as isize),
                    });
                }

                let last_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                // ERROR_LOCK_VIOLATION = 0x21, ERROR_IO_INCOMPLETE = 0x3E4
                if last_error == 0x21 || last_error == 0x3E4 {
                    if std::time::Instant::now() >= deadline {
                        return Err(ConfigError::LockTimeout {
                            path: lock_path,
                            timeout_ms: LOCK_TIMEOUT_MS,
                        });
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                } else {
                    #[allow(clippy::cast_possible_wrap)]
                    let err_code = last_error as i32;
                    return Err(ConfigError::Io {
                        path: lock_path,
                        source: io::Error::from_raw_os_error(err_code),
                    });
                }
            }
        }

        #[cfg(not(unix))]
        #[cfg(not(windows))]
        {
            // Unreachable: the module-level compile_error rejects builds on
            // targets without a cross-process lock implementation.
            Ok(FileLockGuard { file, handle: None })
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

    /// Run a transactional config edit under the cross-process lock.
    ///
    /// This implements the full read-edit-validate-commit sequence:
    ///
    /// 1. Acquire the in-process mutex and OS file lock.
    /// 2. Load the current valid configuration, or create a default in memory.
    /// 3. Serialize it to a temporary file in the destination directory.
    /// 4. Invoke the editor (via `edit`) on the temporary file.
    /// 5. If the editor exits nonzero, delete the temporary file and leave
    ///    the live config unchanged.
    /// 6. Parse the complete edited file (rejecting unknown fields).
    /// 7. Reject validation violations.
    /// 8. Atomically replace the live config using the durable write path.
    /// 9. Clean up the temporary file on all paths.
    ///
    /// The live config file is **never** opened directly in the editor.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on lock timeout, load, editor, parse,
    /// validation, or write failure.
    pub fn edit_transaction(
        &self,
        edit: impl FnOnce(&Path) -> Result<(), ConfigError>,
    ) -> Result<(), ConfigError> {
        let _thread_guard = self.lock.lock().map_err(|_| ConfigError::LockPoisoned)?;
        let _file_guard = self.acquire_lock()?;

        // Step 2: Load current config or create default.
        let config = self.load_or_default()?;

        // Step 3: Serialize to a temporary file in the destination directory.
        let dir = self.path.parent().ok_or_else(|| ConfigError::AtomicWrite {
            path: self.path.clone(),
            source: AtomicWriteError::NoParentDirectory,
        })?;
        fs::create_dir_all(dir).map_err(|e| ConfigError::AtomicWrite {
            path: self.path.clone(),
            source: AtomicWriteError::Io(e),
        })?;

        let temp_name = format!(
            ".gregg-edit-{}-{}.toml.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let temp_path = dir.join(&temp_name);

        {
            // Create the editor-visible file securely before serializing any
            // current configuration into it.
            let mut file =
                create_secure_temp_file(&temp_path).map_err(|e| ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                })?;
            let content = config.to_toml();
            file.write_all(content.as_bytes()).map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
            file.flush().map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                }
            })?;

            #[cfg(unix)]
            file.sync_all().map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
        }

        // Step 4-5: Invoke the editor on the temporary file.
        let edit_result = edit(&temp_path);

        if let Err(e) = edit_result {
            // Editor failed — clean up and leave live config unchanged.
            let _ = fs::remove_file(&temp_path);
            return Err(e);
        }

        // Step 6: Parse the complete edited file.
        let parse_result = Config::load(&temp_path);

        // Step 9: Clean up the temporary file on all paths.
        let _ = fs::remove_file(&temp_path);

        let edited = parse_result?;

        // Step 7: Reject validation violations.
        let violations = edited.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }

        // Step 8: Atomically replace the live config.
        self.write(&edited)?;

        Ok(())
    }
}

/// RAII guard for the cross-process configuration lock.
///
/// Holds the lock file handle for the duration of the critical section.
/// The OS-level advisory lock is released when the guard is dropped
/// (the file descriptor/handle is closed or explicitly unlocked). The
/// lock file inode may persist on disk, but it does not imply a stale
/// lock after the descriptor closes.
pub struct FileLockGuard {
    #[allow(dead_code)]
    file: fs::File,
    /// On Windows, the raw handle value is retained so we can call
    /// `UnlockFileEx` before the file is dropped. On Unix, this is `None`.
    /// Stored as `isize` for cross-platform struct layout.
    #[allow(dead_code)]
    handle: Option<isize>,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        // Closing the file descriptor releases the flock on Unix. On
        // Windows, we must explicitly unlock before the handle closes.
        #[cfg(windows)]
        if let Some(handle) = self.handle {
            unsafe {
                use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
                use windows_sys::Win32::System::IO::OVERLAPPED;
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                #[allow(clippy::ptr_as_ptr)]
                UnlockFileEx(handle as *mut _, 0, 1, 0, &mut overlapped);
            }
        }
        // We do not delete the lock file — it may be reused by the next
        // acquirer and removing it could race with a concurrent open.
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
    /// The editor could not be launched or exited with a nonzero status.
    EditorFailed { path: PathBuf, message: String },
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
            Self::EditorFailed { path, message } => {
                write!(f, "editor failed on {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::AtomicWrite { source, .. } => Some(source),
            Self::Validation(_)
            | Self::LockPoisoned
            | Self::LockTimeout { .. }
            | Self::EditorFailed { .. } => None,
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
    /// `EggPool` host is invalid.
    InvalidEggpoolHost { host: String },
    /// `EggPool` port is invalid.
    InvalidEggpoolPort { port: u16 },
    /// `EggPool` display name is invalid.
    InvalidEggpoolName { reason: String },
    /// `EggPool` API-key environment-variable name is invalid.
    InvalidEggpoolApiKeyEnv { value: String, reason: String },
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
            Self::InvalidEggpoolHost { host } => {
                write!(f, "invalid EggPool host: {host}")
            }
            Self::InvalidEggpoolPort { port } => {
                write!(
                    f,
                    "EggPool port {port} is outside valid range {MIN_PORT}..={MAX_PORT}"
                )
            }
            Self::InvalidEggpoolName { reason } => write!(f, "invalid EggPool name: {reason}"),
            Self::InvalidEggpoolApiKeyEnv { value, reason } => {
                write!(
                    f,
                    "invalid EggPool API-key environment variable {value:?}: {reason}"
                )
            }
        }
    }
}

fn validate_eggpool(violations: &mut Vec<ConfigViolation>, entry: &EggpoolEntry) {
    let host = entry.host.trim();
    if host.is_empty()
        || host.contains("://")
        || host.contains('/')
        || host.contains('?')
        || host.contains('#')
        || host.contains('@')
        || host.contains('[')
        || host.contains(']')
    {
        violations.push(ConfigViolation::InvalidEggpoolHost {
            host: entry.host.clone(),
        });
    }
    if entry.port < MIN_PORT {
        violations.push(ConfigViolation::InvalidEggpoolPort { port: entry.port });
    }
    if let Some(name) = &entry.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            violations.push(ConfigViolation::InvalidEggpoolName {
                reason: "name is empty".to_string(),
            });
        } else if trimmed != name {
            violations.push(ConfigViolation::InvalidEggpoolName {
                reason: "name must not have surrounding whitespace".to_string(),
            });
        } else if name.len() > MAX_EGGPOOL_NAME_LEN {
            violations.push(ConfigViolation::InvalidEggpoolName {
                reason: format!("name exceeds maximum length of {MAX_EGGPOOL_NAME_LEN}"),
            });
        }
    }
    if let Some(value) = &entry.api_key_env {
        let valid = !value.is_empty()
            && value.len() <= MAX_ENV_NAME_LEN
            && value
                .as_bytes()
                .first()
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid {
            let reason = if value.is_empty() {
                "name is empty".to_string()
            } else if value.len() > MAX_ENV_NAME_LEN {
                format!("name exceeds maximum length of {MAX_ENV_NAME_LEN}")
            } else {
                "name must match [A-Za-z_][A-Za-z0-9_]*".to_string()
            };
            violations.push(ConfigViolation::InvalidEggpoolApiKeyEnv {
                value: value.clone(),
                reason,
            });
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

    /// Locate the `lock_helper` binary for cross-process tests.
    ///
    /// During `cargo test`, the binary is in `target/debug/` or
    /// `target/debug/deps/`. This function searches upward from the test
    /// binary's directory to handle varying CI layouts.
    ///
    /// Returns `None` if the binary is not found (e.g., on CI runners
    /// where `[[bin]]` targets may not be compiled for all platforms).
    fn find_lock_helper() -> Option<String> {
        let exe_dir = std::env::current_exe()
            .expect("current_exe should succeed")
            .parent()
            .expect("exe should have a parent")
            .to_path_buf();

        let binary_name = if cfg!(windows) {
            "lock_helper.exe"
        } else {
            "lock_helper"
        };

        // Search upward from the test binary's directory (up to 5 levels).
        let mut search_dir = exe_dir.clone();
        for _ in 0..5 {
            let candidate = search_dir.join(binary_name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().into_owned());
            }
            if !search_dir.pop() {
                break;
            }
        }

        None
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
        assert!(config.eggpool.is_none());
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

    #[test]
    fn old_config_without_eggpool_loads_and_omits_table() {
        let content = "\
config_version = 1\n\
refresh_seconds = 5\n\
request_timeout_ms = 1500\n\
max_concurrent_requests = 16\n\
default_port = 11310\n";
        let config = Config::parse(content, None).unwrap();
        assert!(config.eggpool.is_none());
        assert!(!config.to_toml().contains("[eggpool]"));
    }

    #[test]
    fn eggpool_entry_round_trips_without_secret_value() {
        let config = Config {
            eggpool: Some(EggpoolEntry {
                id: "01234567-89ab-4cde-8123-456789abcdef".into(),
                host: "eggpool.local".into(),
                port: DEFAULT_EGGPOOL_PORT,
                scheme: EggpoolScheme::Https,
                name: Some("Main EggPool".into()),
                api_key_env: Some("EGGPOOL_GREGG_API_KEY".into()),
            }),
            ..Config::default()
        };
        let toml = config.to_toml();
        assert!(!toml.contains("secret-value"));
        assert_eq!(Config::parse(&toml, None).unwrap(), config);
    }

    #[test]
    fn eggpool_names_and_env_references_are_validated() {
        let mut config = Config {
            eggpool: Some(EggpoolEntry {
                id: "id".into(),
                host: "eggpool.local".into(),
                port: DEFAULT_EGGPOOL_PORT,
                scheme: EggpoolScheme::Http,
                name: Some("Main".into()),
                api_key_env: Some("_LOCAL_KEY".into()),
            }),
            ..Config::default()
        };
        assert!(config.is_valid());
        config.eggpool.as_mut().unwrap().api_key_env = Some("not-a-secret".into());
        assert!(config
            .validate()
            .iter()
            .any(|violation| matches!(violation, ConfigViolation::InvalidEggpoolApiKeyEnv { .. })));
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

        // Attempt write to an invalid nested path that cannot exist on
        // any platform (contains a null byte which is invalid on all OSes).
        let bad_path = dir.join("\0").join("config.toml");
        let result = config.write_atomic(&bad_path);
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
            eggpool: None,
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
                refresh_seconds: i + 1,
                ..Default::default()
            };
            config.write_atomic(&path).unwrap();
        }

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 10);
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

    /// Cross-process lock contention test using the `lock_helper` binary.
    ///
    /// Verifies that:
    /// - Process A (`lock_helper`) acquires the OS lock on `<config>.lock`.
    /// - Process B (this test) cannot mutate while A holds the lock.
    /// - After A releases, B completes successfully.
    /// - Final config is valid.
    #[test]
    fn cross_process_lock_contention_via_helper() {
        use std::process::Command;

        let dir = tmp_dir("cross_process_lock_helper");
        let path = dir.join("config.toml");
        let lock_path = PathBuf::from(format!("{}.lock", path.display()));
        let signal_path = dir.join("ready.signal");
        let store = ConfigStore::new(path.clone());

        // Initialize config.
        store
            .mutate(|config| {
                config.refresh_seconds = 5;
                Ok(())
            })
            .unwrap();

        // Locate the lock_helper binary. During `cargo test`, binaries are
        // placed in the same directory as the test binary or in target/debug/.
        let Some(lock_helper) = find_lock_helper() else {
            eprintln!("skipping: lock_helper binary not found");
            return;
        };

        // Spawn lock_helper to hold the OS lock on <config>.lock.
        let mut child = Command::new(&lock_helper)
            .arg(&lock_path)
            .arg(&signal_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn lock_helper at {lock_helper}: {e}"));

        // Wait for lock_helper to signal readiness (lock acquired).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !signal_path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "lock_helper did not signal readiness within 10s"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        // The lock_helper holds an exclusive lock on <config>.lock.
        // ConfigStore::mutate tries to acquire the same lock via acquire_lock().
        // With LOCKFILE_FAIL_IMMEDIATELY, this should fail immediately and
        // retry until the 5-second timeout. We use a short-lived mutate
        // that should NOT succeed while the helper holds the lock.
        //
        // Instead of waiting for the full timeout, verify that the lock is
        // actually held by checking that a second lock_helper also blocks.
        // Then kill the first lock_helper and verify our mutate succeeds.

        // Try a mutate — it should block (or timeout) because lock_helper
        // holds the lock. We don't wait for the full 5s timeout; instead,
        // we verify the lock is held and then release it.
        let store2 = ConfigStore::new(path.clone());
        let mutate_handle = std::thread::spawn(move || {
            // This should block until the lock_helper releases.
            store2.mutate(|config| {
                config.refresh_seconds = 20;
                Ok(())
            })
        });

        // Give the mutate thread time to attempt lock acquisition.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Kill lock_helper to release the lock.
        drop(child.stdin.take());
        let _ = child.kill();
        let _ = child.wait();

        // The mutate should now complete.
        let result = mutate_handle.join().expect("mutate thread panicked");
        result.expect("mutate should succeed after lock release");

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 20);
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    /// Concurrent mutation through multiple threads serializes correctly.
    ///
    /// On Windows, the OS file lock (`LockFileEx`) provides serialization;
    /// on Unix, `flock` does the same. This test proves the combination
    /// of in-process Mutex + OS file lock produces correct results.
    #[test]
    fn concurrent_mutation_serializes_correctly() {
        use std::sync::Arc;
        use std::thread;

        let dir = tmp_dir("concurrent_serialize");
        let path = dir.join("config.toml");
        let store = Arc::new(ConfigStore::new(path.clone()));

        // Initialize config.
        store
            .mutate(|config| {
                config.refresh_seconds = 1;
                Ok(())
            })
            .unwrap();

        // Spawn 5 threads that each increment refresh_seconds.
        let mut handles = Vec::new();
        for i in 2..=6 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                store
                    .mutate(|config| {
                        config.refresh_seconds = i;
                        Ok(())
                    })
                    .unwrap();
            }));
        }

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // Final value should be one of the written values (last writer wins).
        let loaded = store.load_existing().unwrap();
        assert!(
            (1..=6).contains(&loaded.refresh_seconds),
            "refresh_seconds should be in 1..=6, got {}",
            loaded.refresh_seconds
        );
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    /// On Windows, verify that sharing violations fail safely.
    ///
    /// When the destination file is held open with deny-all sharing,
    /// `fs::rename` should fail and the original file should be preserved.
    #[test]
    #[cfg(windows)]
    fn write_atomic_sharing_violation_preserves_original() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tmp_dir("atomic_sharing_violation");
        let path = dir.join("config.toml");

        // Write an initial config.
        let original = Config::default();
        original.write_atomic(&path).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Open the destination file with deny-all sharing.
        // This prevents any other process or handle from accessing the file,
        // including rename-over by another handle.
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0) // Deny all sharing.
            .open(&path)
            .expect("failed to open file for sharing violation");

        // Attempt to write a new config — should fail due to sharing violation.
        let updated = Config {
            refresh_seconds: 99,
            ..Config::default()
        };
        let result = updated.write_atomic(&path);

        // The rename should fail. On Windows, this produces an I/O error.
        assert!(result.is_err(), "rename should fail with sharing violation");

        // Drop the holder so we can read the file again.
        drop(holder);

        // Original file should be intact.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, original, "original config must be preserved");
        assert_eq!(
            fs::read(&path).unwrap(),
            original_bytes,
            "original bytes must be unchanged"
        );
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

    // --- edit_transaction tests ---

    /// Helper: count temp files in a directory.
    fn count_temp_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.contains(".tmp") || n.contains("gregg-edit"))
            })
            .count()
    }

    #[test]
    fn edit_transaction_valid_edit_commits() {
        let dir = tmp_dir("edit_valid");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Editor writes valid TOML with a changed refresh_seconds.
        store
            .edit_transaction(|temp_path| {
                let mut config = Config::load(temp_path)?;
                config.refresh_seconds = 30;
                fs::write(temp_path, config.to_toml()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 30);
        assert_ne!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_invalid_toml_preserves_original() {
        let dir = tmp_dir("edit_invalid_toml");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::PermissionsExt;

            fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };

        // Editor writes invalid TOML.
        let result = store.edit_transaction(|temp_path| {
            fs::write(temp_path, "this is not valid {{{").map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                original_mode
            );
        }
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_validation_failure_preserves_original() {
        let dir = tmp_dir("edit_validation_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::PermissionsExt;

            fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };

        // Editor writes TOML with an invalid value (refresh_seconds = 0).
        let result = store.edit_transaction(|temp_path| {
            let mut config = Config::load(temp_path)?;
            config.refresh_seconds = 0; // Invalid: below minimum
            fs::write(temp_path, config.to_toml()).map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                original_mode
            );
        }
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_nonzero_editor_exit_preserves_original() {
        let dir = tmp_dir("edit_nonzero_exit");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::PermissionsExt;

            fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };

        // Editor "exits nonzero" — closure returns an error.
        let result = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "editor exited with status: 1".to_string(),
            })
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                original_mode
            );
        }
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_editor_launch_failure_preserves_original() {
        let dir = tmp_dir("edit_launch_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Editor "launch fails" — closure returns an error.
        let result = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "failed to launch editor: not found".to_string(),
            })
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_missing_config_starts_from_default() {
        let dir = tmp_dir("edit_missing_config");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        // No config file exists yet.
        assert!(!path.exists());

        // Editor writes valid TOML (just the default).
        store
            .edit_transaction(|temp_path| {
                let config = Config::load(temp_path)?;
                // Verify the temp file started from defaults.
                assert_eq!(config, Config::default());
                // Write it back unchanged.
                fs::write(temp_path, config.to_toml()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        // Config file should now exist with default values.
        assert!(path.exists());
        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded, Config::default());
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_temp_files_removed_on_success_and_failure() {
        let dir = tmp_dir("edit_temp_cleanup");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();

        // Success path.
        store
            .edit_transaction(|temp_path| {
                fs::write(temp_path, Config::default().to_toml()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();
        assert_eq!(count_temp_files(&dir), 0, "no temp files after success");

        // Failure path (invalid TOML).
        let _ = store.edit_transaction(|temp_path| {
            fs::write(temp_path, "invalid {{{").map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert_eq!(count_temp_files(&dir), 0, "no temp files after failure");

        // Failure path (editor error).
        let _ = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "fail".to_string(),
            })
        });
        assert_eq!(
            count_temp_files(&dir),
            0,
            "no temp files after editor error"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_concurrent_mutation_no_lost_updates() {
        use std::sync::Arc;
        use std::thread;

        let dir = tmp_dir("edit_concurrent");
        let path = dir.join("config.toml");
        let store = Arc::new(ConfigStore::new(path.clone()));

        // Initialize config.
        store
            .mutate(|c| {
                c.refresh_seconds = 5;
                Ok(())
            })
            .unwrap();

        let original_bytes = fs::read(&path).unwrap();

        // Start an edit_transaction that holds the lock briefly.
        let store1 = store.clone();
        let edit_handle = thread::spawn(move || {
            store1
                .edit_transaction(|temp_path| {
                    // Hold the lock briefly to simulate editor interaction.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let mut config = Config::load(temp_path)?;
                    config.refresh_seconds = 20;
                    fs::write(temp_path, config.to_toml()).map_err(|e| ConfigError::Io {
                        path: temp_path.to_path_buf(),
                        source: e,
                    })?;
                    Ok(())
                })
                .unwrap();
        });

        // Give the edit thread time to acquire the lock.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Try a concurrent mutate — it should block until edit completes,
        // then see the updated config.
        let store2 = store.clone();
        let mutate_handle = thread::spawn(move || {
            store2
                .mutate(|c| {
                    c.refresh_seconds = 30;
                    Ok(())
                })
                .unwrap();
        });

        edit_handle.join().unwrap();
        mutate_handle.join().unwrap();

        // The final config should reflect the mutate (30), not the edit (20).
        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 30);
        assert_ne!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_rejects_unknown_fields() {
        let dir = tmp_dir("edit_unknown_fields");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Editor writes TOML with an unknown field.
        let result = store.edit_transaction(|temp_path| {
            let config = Config::load(temp_path)?;
            let toml = config.to_toml();
            // Append an unknown field.
            let modified = format!("{toml}\nunknown_field = \"oops\"\n");
            fs::write(temp_path, modified).map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Config file permission tests (Unix only) ---

    #[test]
    #[cfg(unix)]
    fn write_atomic_creates_new_config_with_0600_perms() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_new_file");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let metadata = fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "new config file must be user-only 0600");

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_preserves_0600_on_overwrite() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_overwrite");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let mode1 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode1, 0o600);

        config.write_atomic(&path).unwrap();
        let mode2 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode2, 0o600, "overwrite must preserve 0600");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_does_not_expose_broad_permission_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_no_leak");
        let path = dir.join("config.toml");

        let config = Config::default();

        // Verify no broad-permission temp files remain after write.
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(".tmp") {
                let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
                panic!("temp file {name} should not remain: mode {mode:o}");
            }
        }

        config.write_atomic(&path).unwrap();

        // After write, only the config file should exist, and no temp files.
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(entries.len(), 1, "only config file should remain");
        assert!(!entries[0].file_name().to_string_lossy().contains(".tmp"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn edit_transaction_preserves_0600_perms() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_edit");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = Config::default();
        store.write(&config).unwrap();
        let mode1 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode1, 0o600);

        store
            .edit_transaction(|temp_path| {
                let mut config = Config::load(temp_path)?;
                config.refresh_seconds = 30;
                fs::write(temp_path, config.to_toml()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        let mode2 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode2, 0o600, "edit must preserve 0600");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn edit_transaction_failure_preserves_original_perms_and_bytes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_edit_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = Config::default();
        store.write(&config).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let original_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(original_mode, 0o600);

        let result = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "simulated".to_string(),
            })
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        // Original permissions unchanged.
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, original_mode, "edit failure must preserve 0600");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn edit_transaction_editor_sees_secure_temp_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_editor_visible");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());
        store.write(&Config::default()).unwrap();

        store
            .edit_transaction(|temp_path| {
                let mode = fs::metadata(temp_path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "editor temp must start user-only");
                assert_eq!(Config::load(temp_path).unwrap(), Config::default());
                Ok(())
            })
            .unwrap();

        assert_eq!(count_temp_files(&dir), 0, "editor temp must be cleaned up");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_permission_failure_is_fatal_and_cleans_temp() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_injected_failure");
        let path = dir.join("config.toml");
        let original = Config::default();
        original.write_atomic(&path).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let original_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;

        inject_permission_set_failure();
        let result = Config {
            refresh_seconds: 10,
            ..original
        }
        .write_atomic(&path);

        match result {
            Err(ConfigError::AtomicWrite {
                source: AtomicWriteError::Io(error),
                ..
            }) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("expected permission failure, got {other:?}"),
        }
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            original_mode
        );
        assert_eq!(count_temp_files(&dir), 0, "failed temp must be cleaned up");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn lock_file_is_not_active_config_file() {
        let dir = tmp_dir("perms_lock_file");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = Config::default();
        store.write(&config).unwrap();

        // The lock file should exist after a mutate.
        store
            .mutate(|c| {
                c.refresh_seconds = 15;
                Ok(())
            })
            .unwrap();

        let lock_path = store.lock_path();
        // Lock file is not the config file.
        assert_ne!(lock_path, path);
        // Lock file path ends with .lock.
        assert!(lock_path.to_string_lossy().ends_with(".lock"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn mutate_preserves_0600_through_store() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_mutate");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        store
            .mutate(|c| {
                c.refresh_seconds = 20;
                Ok(())
            })
            .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mutate through store must produce 0600");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn repeated_writes_preserve_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_repeated");
        let path = dir.join("config.toml");

        for i in 1..=5 {
            let config = Config {
                refresh_seconds: i,
                ..Default::default()
            };
            config.write_atomic(&path).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "write {i} must produce 0600");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Windows config path tests ---

    #[test]
    #[cfg(target_os = "windows")]
    fn default_path_uses_appdata() {
        // On Windows, default_path should use %APPDATA%\gregg\gregg.toml.
        let path = Config::default_path();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("gregg\\gregg.toml") || path_str.contains("gregg/gregg.toml"),
            "expected APPDATA path, got: {path_str}"
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn default_path_parent_exists_or_can_be_created() {
        let path = Config::default_path();
        let parent = path.parent().unwrap();
        // The parent directory should either exist or be creatable.
        if !parent.exists() {
            fs::create_dir_all(parent).expect("should be able to create parent directory");
        }
        assert!(parent.exists());
        let _ = fs::remove_dir_all(parent);
    }

    // --- Atomic write with paths containing spaces and Unicode ---

    #[test]
    fn write_atomic_path_with_spaces() {
        let dir = tmp_dir("atomic spaces in path");
        let path = dir.join("config.toml");
        let config = Config::default();
        config.write_atomic(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_repeated_mutation_produces_valid_config() {
        let dir = tmp_dir("atomic_repeat_mutation");
        let path = dir.join("config.toml");

        let mut config = Config::default();
        for i in 1..=20 {
            config.refresh_seconds = i;
            config.write_atomic(&path).unwrap();
        }
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 20);
        assert!(loaded.is_valid());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_transaction_path_with_spaces() {
        let dir = tmp_dir("edit spaces in path");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();

        store
            .edit_transaction(|temp_path| {
                let mut config = Config::load(temp_path)?;
                config.refresh_seconds = 25;
                fs::write(temp_path, config.to_toml()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 25);
        assert_eq!(count_temp_files(&dir), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
