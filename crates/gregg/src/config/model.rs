//! Client configuration model: entries, limits, defaults, load/validate/write primitives.

use super::store::{
    cleanup_stale_temps, create_secure_temp_file, sync_parent_directory, AtomicWriteError,
    ConfigError,
};
use super::validation::{validate_eggpool, ConfigViolation};
use crate::endpoint::{Endpoint, DEFAULT_PORT, MAX_ENDPOINT_NAME_LEN};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Minimum allowed refresh interval in seconds.
pub const MIN_REFRESH_SECONDS: u64 = 1;

/// Maximum allowed refresh interval in seconds.
pub const MAX_REFRESH_SECONDS: u64 = 3600;

/// Minimum request timeout in milliseconds.
pub const MIN_REQUEST_TIMEOUT_MS: u64 = 100;

/// Maximum request timeout in milliseconds.
pub const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;

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
        if self.request_timeout_ms < MIN_REQUEST_TIMEOUT_MS
            || self.request_timeout_ms > MAX_REQUEST_TIMEOUT_MS
        {
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
        if self.default_port == 0 {
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

            // Host validation.
            let host = system.host.trim();
            let normalized_host = if host.is_empty() {
                violations.push(ConfigViolation::EmptyHost {
                    id: system.id.clone(),
                });
                None
            } else if host.contains("://")
                || host.contains('/')
                || host.contains('?')
                || host.contains('[')
                || host.contains(']')
            {
                violations.push(ConfigViolation::InvalidHost {
                    id: system.id.clone(),
                    host: host.to_string(),
                });
                None
            } else if let Ok(normalized_host) = crate::endpoint::normalize_host(host) {
                Some(normalized_host)
            } else {
                violations.push(ConfigViolation::InvalidHost {
                    id: system.id.clone(),
                    host: host.to_string(),
                });
                None
            };

            // Unique normalized address. Invalid hosts are already reported
            // above and must not also create misleading duplicate diagnostics.
            if let Some(normalized_host) = normalized_host {
                let normalized =
                    format!("{}:{}", normalized_host.to_ascii_lowercase(), system.port);
                if !seen_addresses.insert(normalized.clone()) {
                    violations.push(ConfigViolation::DuplicateAddress {
                        address: normalized,
                    });
                }
            }

            // Port validation.
            if system.port == 0 {
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
    ///
    /// When no user-scoped config directory is available (missing `HOME`,
    /// `XDG_CONFIG_HOME`, `APPDATA`, or `USERPROFILE`, or an unrecognized
    /// target OS), falls back to a bare `gregg.toml` filename. The bare
    /// fallback relies on [`Self::write_atomic`] treating an empty parent
    /// path as the current working directory.
    #[must_use]
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                if !xdg.trim().is_empty() {
                    return PathBuf::from(xdg).join("gregg").join("gregg.toml");
                }
            }
            match std::env::var("HOME") {
                Ok(home) if !home.trim().is_empty() => PathBuf::from(home)
                    .join(".config")
                    .join("gregg")
                    .join("gregg.toml"),
                _ => PathBuf::from("gregg.toml"),
            }
        }
        #[cfg(target_os = "macos")]
        {
            match std::env::var("HOME") {
                Ok(home) if !home.trim().is_empty() => PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("gregg")
                    .join("gregg.toml"),
                _ => PathBuf::from("gregg.toml"),
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                if !appdata.trim().is_empty() {
                    return PathBuf::from(appdata).join("gregg").join("gregg.toml");
                }
            }
            if let Ok(userprofile) = std::env::var("USERPROFILE") {
                if !userprofile.trim().is_empty() {
                    return PathBuf::from(userprofile)
                        .join("AppData")
                        .join("Roaming")
                        .join("gregg")
                        .join("gregg.toml");
                }
            }
            // No user-scoped directory available; bare fallback.
            PathBuf::from("gregg.toml")
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
        let mut config: Self = toml::from_str(content).map_err(|e| ConfigError::Parse {
            path: path.map(PathBuf::from),
            source: e,
        })?;

        // Config files are a public input boundary. Canonicalize recognized
        // IP literals and IPv6 zone identifiers before validation so a
        // hand-edited spelling cannot create a second logical endpoint.
        for system in &mut config.systems {
            system.host = crate::endpoint::normalize_host(&system.host).map_err(|_error| {
                ConfigError::Validation(vec![ConfigViolation::InvalidHost {
                    id: system.id.clone(),
                    host: system.host.clone(),
                }])
            })?;
        }

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
        // An empty parent path (e.g. a bare `gregg.toml` fallback from
        // `default_path`) means the current working directory; skip
        // `create_dir_all` because the empty string is not a valid path.
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).map_err(|e| ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            })?;
        }

        cleanup_stale_temps(if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        })
        .map_err(|source| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(source),
        })?;

        let content = self.to_toml().map_err(|source| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Serialization(source),
        })?;
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

        // Verify the bytes that were actually written before exposing the
        // replacement. This catches truncated or otherwise corrupt temp
        // files before the atomic rename can replace a valid config.
        let Ok(verified) = Config::load(&temp_path) else {
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::VerificationFailed,
            });
        };
        if verified != *self {
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::VerificationFailed,
            });
        }

        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        sync_parent_directory(dir).map_err(|e| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(e),
        })?;

        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

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
        let toml = config.to_toml().unwrap();
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
        assert!(!config.to_toml().unwrap().contains("[eggpool]"));
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
        let toml = config.to_toml().unwrap();
        assert!(!toml.contains("secret-value"));
        assert_eq!(Config::parse(&toml, None).unwrap(), config);
    }
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
}
