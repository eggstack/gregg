//! Daemon configuration, validation, file I/O, and atomic persistence.
//!
//! Configuration is stored as canonical TOML and validated before every
//! load and before every mutation. Atomic writes ensure a partially written
//! file can never corrupt a running service.

use std::fmt;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: String::from("greggd"),
            host: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 11310,
            sample_interval_ms: 1000,
            stale_after_ms: 10_000,
        }
    }
}

impl Config {
    /// Validate all fields.
    ///
    /// Returns a list of all violations so callers can present every
    /// problem at once rather than fixing them one at a time.
    #[must_use]
    pub fn validate(&self) -> Vec<ConfigViolation> {
        let mut violations = Vec::new();

        // Name validation.
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            violations.push(ConfigViolation::EmptyName);
        } else if trimmed.len() > MAX_NAME_LEN {
            violations.push(ConfigViolation::NameTooLong {
                length: trimmed.len(),
                max: MAX_NAME_LEN,
            });
        }

        // Port validation. u16 cannot exceed 65535, so only check for zero.
        if self.port < MIN_PORT {
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
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("Config serializes to TOML")
    }

    /// Atomically write this configuration to the given path.
    ///
    /// This follows the write-flush-rename-verify pattern:
    /// 1. Write to a unique temporary file in the same directory.
    /// 2. Flush the file.
    /// 3. Rename over the destination.
    /// 4. Reopen and re-parse as verification.
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
        fs::create_dir_all(dir).map_err(|e| ConfigError::AtomicWrite {
            path: path.to_path_buf(),
            source: AtomicWriteError::Io(e),
        })?;

        // 2. Serialize the complete config.
        let content = self.to_toml();

        // 3. Write to a uniquely named temporary file.
        let temp_name = format!(".greggd-{}.toml.tmp", std::process::id());
        let temp_path = dir.join(&temp_name);

        fs::write(&temp_path, content.as_bytes()).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        // 4. Flush the file.
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

        // 5. Rename atomically over the destination.
        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::Io(e),
            }
        })?;

        // 6. Reopen and re-parse as verification.
        let verified = Self::load(path)?;
        if *self != verified {
            return Err(ConfigError::AtomicWrite {
                path: path.to_path_buf(),
                source: AtomicWriteError::VerificationFailed,
            });
        }

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
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Validation(_) => None,
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
    /// The file was written but verification re-parse failed.
    VerificationFailed,
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoParentDirectory => write!(f, "path has no parent directory"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::VerificationFailed => write!(f, "verification re-parse failed"),
        }
    }
}

impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// A single configuration validation violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigViolation {
    /// Display name is empty after trimming.
    EmptyName,
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
}

impl fmt::Display for ConfigViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "name is empty after trimming"),
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
        }
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
    fn config_round_trips_through_toml() {
        let config = Config::default();
        let toml = config.to_toml();
        let parsed = Config::parse(&toml, None).unwrap();
        assert_eq!(config, parsed);
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

        // Attempt to write to an invalid path (no parent directory).
        let result = original.write_atomic(Path::new("/nonexistent_dir/config.toml"));
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
        assert!(!format!("{v}").is_empty());

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
}
