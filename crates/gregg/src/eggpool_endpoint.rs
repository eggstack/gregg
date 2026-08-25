//! Strict host/port parsing for the optional `EggPool` endpoint.

use std::fmt;
use std::net::IpAddr;

use crate::config::{EggpoolScheme, DEFAULT_EGGPOOL_PORT};

/// A parsed `EggPool` endpoint before configuration persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EggpoolEndpointSpec {
    /// Normalized host.
    pub host: String,
    /// Port, defaulting to [`DEFAULT_EGGPOOL_PORT`].
    pub port: u16,
    /// Whether the port was explicitly provided.
    pub port_was_explicit: bool,
}

/// Errors from `EggPool` endpoint parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EggpoolEndpointError {
    /// Input is empty.
    EmptyInput,
    /// A URL scheme was supplied.
    HasScheme,
    /// A path, query, or fragment was supplied.
    HasPath,
    /// Credentials were supplied.
    HasCredentials,
    /// Brackets are malformed.
    MalformedBrackets,
    /// Host is empty.
    EmptyHost,
    /// Port is invalid.
    InvalidPort,
}

impl fmt::Display for EggpoolEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyInput => "EggPool endpoint is empty",
            Self::HasScheme => "EggPool endpoint must not include a URL scheme",
            Self::HasPath => "EggPool endpoint must not include a path, query, or fragment",
            Self::HasCredentials => "EggPool endpoint must not include credentials",
            Self::MalformedBrackets => "malformed EggPool IPv6 bracket syntax",
            Self::EmptyHost => "EggPool host is empty",
            Self::InvalidPort => "EggPool port must be a number from 1 to 65535",
        };
        f.write_str(message)
    }
}

impl std::error::Error for EggpoolEndpointError {}

impl EggpoolEndpointSpec {
    /// Parse `HOST`, `HOST:PORT`, `[IPv6]:PORT`, or bare IPv6.
    pub fn parse(input: &str) -> Result<Self, EggpoolEndpointError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(EggpoolEndpointError::EmptyInput);
        }
        if input.contains("://") {
            return Err(EggpoolEndpointError::HasScheme);
        }
        if input.contains('/') || input.contains('?') || input.contains('#') {
            return Err(EggpoolEndpointError::HasPath);
        }
        if input.contains('@') {
            return Err(EggpoolEndpointError::HasCredentials);
        }

        if input.starts_with('[') {
            let close = input
                .find(']')
                .ok_or(EggpoolEndpointError::MalformedBrackets)?;
            let host = &input[1..close];
            if host.is_empty() || input.get(close + 1..close + 2) != Some(":") {
                return Err(EggpoolEndpointError::MalformedBrackets);
            }
            if host.parse::<IpAddr>().is_err() {
                return Err(EggpoolEndpointError::MalformedBrackets);
            }
            let port = parse_port(input.get(close + 2..).unwrap_or_default())?;
            return Ok(Self {
                host: normalize_host(host)?,
                port,
                port_was_explicit: true,
            });
        }

        if input.parse::<IpAddr>().is_ok() {
            return Ok(Self {
                host: normalize_host(input)?,
                port: DEFAULT_EGGPOOL_PORT,
                port_was_explicit: false,
            });
        }

        let colon_count = input.matches(':').count();
        if colon_count > 1 {
            // Bare multi-colon input (e.g. `::1:8080`) is ambiguous;
            // bracketed `[ipv6]:port` is the required form.
            return Err(EggpoolEndpointError::MalformedBrackets);
        }
        if colon_count == 1 {
            let (host, port) = input
                .split_once(':')
                .ok_or(EggpoolEndpointError::InvalidPort)?;
            return Ok(Self {
                host: normalize_host(host)?,
                port: parse_port(port)?,
                port_was_explicit: true,
            });
        }
        Ok(Self {
            host: normalize_host(input)?,
            port: DEFAULT_EGGPOOL_PORT,
            port_was_explicit: false,
        })
    }
}

fn normalize_host(host: &str) -> Result<String, EggpoolEndpointError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(EggpoolEndpointError::EmptyHost);
    }
    Ok(host
        .parse::<IpAddr>()
        .map_or_else(|_| host.to_ascii_lowercase(), |ip| ip.to_string()))
}

fn parse_port(port: &str) -> Result<u16, EggpoolEndpointError> {
    let port = port
        .parse::<u32>()
        .map_err(|_| EggpoolEndpointError::InvalidPort)?;
    if port == 0 || port > u32::from(u16::MAX) {
        return Err(EggpoolEndpointError::InvalidPort);
    }
    u16::try_from(port).map_err(|_| EggpoolEndpointError::InvalidPort)
}

/// Format an `EggPool` base address, including scheme and IPv6 brackets.
#[must_use]
pub fn display_address(host: &str, port: u16, scheme: EggpoolScheme) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    format!("{scheme}://{host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_common_hosts() {
        assert_eq!(
            EggpoolEndpointSpec::parse("EGGPOOL.Local").unwrap().host,
            "eggpool.local"
        );
        assert_eq!(
            EggpoolEndpointSpec::parse("127.0.0.1:443").unwrap().port,
            443
        );
        assert_eq!(
            EggpoolEndpointSpec::parse("::1").unwrap().port,
            DEFAULT_EGGPOOL_PORT
        );
        let ipv6 = EggpoolEndpointSpec::parse("[2001:DB8::1]:443").unwrap();
        assert_eq!(ipv6.host, "2001:db8::1");
        assert!(ipv6.port_was_explicit);
    }

    #[test]
    fn rejects_urls_credentials_and_bad_ports() {
        for input in [
            "https://host",
            "host/path",
            "user@host",
            "host:0",
            "host:nope",
            "host:65536",
        ] {
            assert!(EggpoolEndpointSpec::parse(input).is_err(), "{input}");
        }
        assert!(EggpoolEndpointSpec::parse("[::1]").is_err());
    }

    #[test]
    fn formats_schemes_and_ipv6() {
        assert_eq!(
            display_address("eggpool.local", 11300, EggpoolScheme::Http),
            "http://eggpool.local:11300"
        );
        assert_eq!(
            display_address("2001:db8::1", 443, EggpoolScheme::Https),
            "https://[2001:db8::1]:443"
        );
    }
}
