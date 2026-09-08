//! Configuration validation violations and percent/field checks.

use super::model::{
    EggpoolEntry, MAX_CONCURRENT_REQUESTS, MAX_EGGPOOL_NAME_LEN, MAX_ENV_NAME_LEN, MAX_PORT,
    MAX_REFRESH_SECONDS, MAX_REQUEST_TIMEOUT_MS, MIN_PORT, MIN_REFRESH_SECONDS,
    MIN_REQUEST_TIMEOUT_MS, SUPPORTED_CONFIG_VERSION,
};
use std::fmt;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigViolation {
    /// Config version is not supported.
    UnsupportedConfigVersion(u32),
    /// Refresh seconds is outside the valid range.
    InvalidRefreshSeconds(u64),
    /// Request timeout is outside the valid range.
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
    /// An `EggPool` endpoint is already configured and replacement was not requested.
    EggpoolAlreadyConfigured,
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
                    "request_timeout_ms {ms} is outside valid range {MIN_REQUEST_TIMEOUT_MS}..={MAX_REQUEST_TIMEOUT_MS}"
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
            Self::EggpoolAlreadyConfigured => {
                write!(f, "EggPool endpoint is already configured; use --replace")
            }
            Self::InvalidEggpoolApiKeyEnv { value, reason } => {
                write!(
                    f,
                    "invalid EggPool API-key environment variable {value:?}: {reason}"
                )
            }
        }
    }
}
pub(crate) fn validate_eggpool(violations: &mut Vec<ConfigViolation>, entry: &EggpoolEntry) {
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
    if entry.port == 0 {
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
    use super::super::model::{Config, EggpoolScheme, SystemEntry, DEFAULT_EGGPOOL_PORT};
    use super::*;
    use crate::endpoint::MAX_ENDPOINT_NAME_LEN;

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
    fn request_timeout_too_high_fails() {
        let config = Config {
            request_timeout_ms: MAX_REQUEST_TIMEOUT_MS + 1,
            ..Config::default()
        };
        let violations = config.validate();
        assert!(violations.contains(&ConfigViolation::InvalidRequestTimeout(
            MAX_REQUEST_TIMEOUT_MS + 1
        )));
    }
    #[test]
    fn request_timeout_boundaries_are_valid() {
        for request_timeout_ms in [MIN_REQUEST_TIMEOUT_MS, MAX_REQUEST_TIMEOUT_MS] {
            let config = Config {
                request_timeout_ms,
                ..Config::default()
            };
            assert!(
                config.is_valid(),
                "timeout {request_timeout_ms} should be valid"
            );
        }
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
    fn non_ascii_hosts_are_not_unicode_case_folded_for_deduplication() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "İ".into(),
            port: 80,
            name: None,
        });
        config.systems.push(SystemEntry {
            id: "id2".into(),
            host: "i\u{307}".into(),
            port: 80,
            name: None,
        });
        assert!(!config
            .validate()
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
    fn bracketed_system_host_fails_validation() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "[192.168.1.1]".into(),
            port: 80,
            name: None,
        });
        assert!(config
            .validate()
            .iter()
            .any(|v| matches!(v, ConfigViolation::InvalidHost { .. })));
    }
    #[test]
    fn malformed_host_is_not_inserted_into_duplicate_address_index() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "bad".into(),
            host: "fe80::1%25".into(),
            port: 8080,
            name: None,
        });
        let violations = config.validate();
        assert!(violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::InvalidHost { .. })));
        assert!(!violations
            .iter()
            .any(|v| matches!(v, ConfigViolation::DuplicateAddress { .. })));
    }
    #[test]
    fn parse_canonicalizes_recognized_ip_host_spellings() {
        let content = "\
config_version = 1\n\
refresh_seconds = 5\n\
request_timeout_ms = 1500\n\
max_concurrent_requests = 16\n\
default_port = 11310\n\
[[systems]]\n\
id = \"id1\"\n\
host = \"2001:0db8:0:0:0:0:0:1\"\n\
port = 11310\n";
        let config = Config::parse(content, None).expect("config should parse");
        assert_eq!(config.systems[0].host, "2001:db8::1");
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
}
