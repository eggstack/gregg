//! Endpoint parsing, normalization, and identity.
//!
//! An endpoint represents a single `host:port` pair that identifies a
//! remote `greggd` instance. Canonical endpoint parsing is strict and
//! deterministic; the `gregg add` command has a separate HTTP URL adapter.

use std::fmt;
use std::net::IpAddr;

/// Default port for greggd endpoints.
pub const DEFAULT_PORT: u16 = 11310;

/// Maximum length for an endpoint display name.
pub const MAX_ENDPOINT_NAME_LEN: usize = 128;

/// A parsed and normalized remote endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    /// Stable unique identifier (UUID v4).
    pub id: String,
    /// Normalized host (IP literal or DNS name).
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Optional human-readable display name.
    pub name: Option<String>,
}

impl Endpoint {
    /// Create a new endpoint with a generated UUID.
    #[allow(dead_code)]
    pub fn new(host: String, port: u16, name: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            host,
            port,
            name,
        }
    }

    /// Return the canonical `host:port` display form.
    ///
    /// IPv6 addresses are bracketed: `[::1]:8080`.
    #[must_use]
    pub fn display_address(&self) -> String {
        display_address(&self.host, self.port)
    }

    /// Return `true` if this endpoint matches the given host string.
    ///
    /// For host-only matching, the host comparison is case-insensitive
    /// for DNS names and uses normalized IP literals.
    #[must_use]
    #[allow(dead_code)]
    pub fn matches_host(&self, host: &str) -> bool {
        if let Ok(ip) = host.parse::<IpAddr>() {
            // Compare as IP addresses for normalized comparison.
            if let Ok(self_ip) = self.host.parse::<IpAddr>() {
                return ip == self_ip;
            }
            return false;
        }
        // DNS comparison: case-insensitive.
        self.host.eq_ignore_ascii_case(host)
    }

    /// Return `true` if this endpoint matches the given host:port string.
    #[must_use]
    #[allow(dead_code)]
    pub fn matches_full(&self, spec: &str) -> bool {
        match EndpointSpec::parse(spec) {
            Ok(parsed) => self.host == parsed.host && self.port == parsed.port,
            Err(_) => false,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = &self.name {
            write!(f, "{name}  {}", self.display_address())
        } else {
            write!(f, "  {}", self.display_address())
        }
    }
}

/// A partially parsed endpoint specification before ID assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointSpec {
    /// Normalized host.
    pub host: String,
    /// TCP port (may be the default).
    pub port: u16,
    /// Whether the port was explicitly provided in the input.
    pub port_was_explicit: bool,
    /// Optional display name.
    pub name: Option<String>,
}

/// Errors from endpoint parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    /// Input is empty or whitespace only.
    EmptyInput,
    /// Input contains a URL scheme (e.g. `http://`).
    HasScheme { input: String },
    /// Input contains a URL scheme other than HTTP.
    UnsupportedScheme { scheme: String, input: String },
    /// Input looked like an HTTP URL but was not syntactically valid.
    MalformedUrl { input: String },
    /// Input contains a path separator.
    HasPath { input: String },
    /// Input contains credentials.
    HasCredentials { input: String },
    /// Port is zero.
    PortZero,
    /// Port exceeds u16 range.
    PortOverflow { input: String },
    /// Port is not a valid number.
    PortNotANumber { input: String },
    /// Malformed bracket syntax for IPv6.
    MalformedBrackets { input: String },
    /// Host is empty after parsing.
    EmptyHost,
    /// Display name is empty or too long.
    InvalidName { reason: String },
    /// `gregg add` was given input without a port; the add command
    /// requires an explicit nonzero port.
    ExplicitPortRequired,
    /// `gregg add` was given a nickname in addition to `--name`.
    AmbiguousName { input: String },
    /// Input contained more than one `@`, leaving the nickname
    /// separator ambiguous.
    MultipleAtSeparators { input: String },
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "endpoint is empty"),
            Self::HasScheme { input } => {
                write!(f, "endpoint must not include a URL scheme: {input}")
            }
            Self::UnsupportedScheme { scheme, input } => {
                write!(f, "unsupported endpoint URL scheme '{scheme}': {input}")
            }
            Self::MalformedUrl { input } => write!(f, "malformed endpoint URL: {input}"),
            Self::HasPath { input } => {
                write!(f, "endpoint must not include a path: {input}")
            }
            Self::HasCredentials { input } => {
                write!(f, "endpoint must not include credentials: {input}")
            }
            Self::PortZero => write!(f, "port must be greater than 0"),
            Self::PortOverflow { input } => {
                write!(f, "port exceeds valid range: {input}")
            }
            Self::PortNotANumber { input } => {
                write!(f, "port is not a valid number: {input}")
            }
            Self::MalformedBrackets { input } => {
                write!(f, "malformed IPv6 bracket syntax: {input}")
            }
            Self::EmptyHost => write!(f, "host is empty"),
            Self::InvalidName { reason } => {
                write!(f, "invalid display name: {reason}")
            }
            Self::ExplicitPortRequired => write!(
                f,
                "gregg add requires an explicit port; specify host:port instead of host"
            ),
            Self::AmbiguousName { input } => {
                write!(
                    f,
                    "gregg add received both an inline nickname and --name; pick one: {input}"
                )
            }
            Self::MultipleAtSeparators { input } => {
                write!(
                    f,
                    "endpoint contains multiple '@' separators; use at most one 'nickname@' prefix: {input}"
                )
            }
        }
    }
}

impl std::error::Error for EndpointError {}

impl EndpointSpec {
    /// Parse a host:port specification string.
    ///
    /// Supports:
    /// - IPv4 with optional port: `192.168.1.1` or `192.168.1.1:8080`
    /// - DNS hostname with optional port: `server.local` or `server.local:8080`
    /// - Bracketed IPv6 with port: `[::1]:8080`
    /// - Bare IPv6 without port: `::1` (uses default port)
    ///
    /// Rejects schemes, paths, credentials, whitespace-only, port zero.
    ///
    /// # Errors
    ///
    /// Returns [`EndpointError`] if the input is malformed.
    pub fn parse(input: &str) -> Result<Self, EndpointError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(EndpointError::EmptyInput);
        }

        let input_str = trimmed;

        // Reject URL schemes.
        if input_str.contains("://") {
            return Err(EndpointError::HasScheme {
                input: input_str.to_string(),
            });
        }

        // Reject paths (contain / but not inside brackets).
        if !input_str.starts_with('[') && input_str.contains('/') {
            return Err(EndpointError::HasPath {
                input: input_str.to_string(),
            });
        }

        // Reject credentials (@).
        if input_str.contains('@') {
            return Err(EndpointError::HasCredentials {
                input: input_str.to_string(),
            });
        }

        // Bracketed IPv6: [addr]:port
        if input_str.starts_with('[') {
            return Self::parse_bracketed_ipv6(input_str);
        }

        // Count colons to disambiguate.
        let colon_count = input_str.matches(':').count();

        match colon_count {
            0 => {
                // Bare hostname or IPv6 without brackets (unambiguous — no colons).
                Ok(Self {
                    host: normalize_host(input_str)?,
                    port: DEFAULT_PORT,
                    port_was_explicit: false,
                    name: None,
                })
            }
            1 => {
                // host:port — split on the last colon.
                let (host_part, port_part) = rsplit_once_colon(input_str);
                let port = parse_port(port_part, input_str)?;
                Ok(Self {
                    host: normalize_host(host_part)?,
                    port,
                    port_was_explicit: true,
                    name: None,
                })
            }
            _ => {
                // Multiple colons: could be bare IPv6 (e.g., `::1`, `fe80::1`).
                // Bare IPv6 without brackets uses default port.
                // Check for host:port form with IPv6 (e.g., `::1:8080` is ambiguous).
                // Strategy: if it parses as a valid IPv6 address, treat as host-only.
                // If not, try splitting on last colon as host:port.
                if input_str.parse::<IpAddr>().is_ok() || is_ipv6_with_zone_id(input_str) {
                    // Valid IPv6 literal — use default port.
                    Ok(Self {
                        host: normalize_host(input_str)?,
                        port: DEFAULT_PORT,
                        port_was_explicit: false,
                        name: None,
                    })
                } else {
                    // A zone-ID IPv6 address with an explicit port is not
                    // accepted by `IpAddr`, so recognize the host portion
                    // explicitly before using the general fallback. This
                    // keeps malformed zone IDs from being treated as DNS
                    // names by accident.
                    let (host_part, port_part) = rsplit_once_colon(input_str);
                    if input_str.contains('%') && !is_ipv6_with_zone_id(host_part) {
                        return Err(EndpointError::MalformedBrackets {
                            input: input_str.to_string(),
                        });
                    }
                    let port = parse_port(port_part, input_str)?;
                    Ok(Self {
                        host: normalize_host(host_part)?,
                        port,
                        port_was_explicit: true,
                        name: None,
                    })
                }
            }
        }
    }

    /// Parse the endpoint input accepted by `gregg add`.
    ///
    /// Canonical host and port input is delegated to [`Self::parse`]. An HTTP
    /// URL is reduced to its authority first; path, query, and fragment are
    /// validated by the URL parser and then discarded. URL parsing retains
    /// whether an explicit port was supplied so `:80` is not confused with a
    /// missing port.
    pub fn parse_add_input(input: &str) -> Result<Self, EndpointError> {
        let trimmed = input.trim();
        if !trimmed.contains("://") {
            return Self::parse(trimmed);
        }

        let url = reqwest::Url::parse(trimmed).map_err(|_| EndpointError::MalformedUrl {
            input: trimmed.to_string(),
        })?;
        let scheme = url.scheme();
        if !scheme.eq_ignore_ascii_case("http") {
            return Err(EndpointError::UnsupportedScheme {
                scheme: scheme.to_string(),
                input: trimmed.to_string(),
            });
        }

        let authority = trimmed
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(rest))
            .unwrap_or_default();
        if authority.contains('@') || !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::HasCredentials {
                input: trimmed.to_string(),
            });
        }

        let parsed_host = url.host_str().ok_or(EndpointError::EmptyHost)?;
        // Use the raw authority for IPv6 zone identifiers so a literal zone
        // beginning with "25" is not mistaken for an already-decoded `%25`
        // separator by the URL parser.
        let host = raw_authority_host(authority)
            .filter(|host| host.contains('%'))
            .unwrap_or(parsed_host);
        let host = unbracketed_host(host);
        let authority_host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        // `Url::port()` omits an explicitly written default port. Preserve
        // that distinction from the raw authority while still letting the
        // URL parser validate the complete input.
        let explicit_port = url
            .port()
            .map(|port| port.to_string())
            .or_else(|| raw_authority_port(authority).map(std::string::ToString::to_string));
        let canonical = match explicit_port {
            Some(port) => format!("{authority_host}:{port}"),
            None => authority_host,
        };
        Self::parse(&canonical)
    }

    fn parse_bracketed_ipv6(input: &str) -> Result<Self, EndpointError> {
        let close = input
            .find(']')
            .ok_or_else(|| EndpointError::MalformedBrackets {
                input: input.to_string(),
            })?;

        let addr_part = &input[1..close];

        if addr_part.is_empty() {
            return Err(EndpointError::EmptyHost);
        }

        // Must be followed by :port
        let rest = &input[close + 1..];
        if rest.is_empty() {
            return Err(EndpointError::MalformedBrackets {
                input: input.to_string(),
            });
        }
        if !rest.starts_with(':') {
            return Err(EndpointError::MalformedBrackets {
                input: input.to_string(),
            });
        }

        let port_str = &rest[1..];
        let port = parse_port(port_str, input)?;

        if addr_part.parse::<std::net::Ipv6Addr>().is_err() && !is_ipv6_with_zone_id(addr_part) {
            return Err(EndpointError::MalformedBrackets {
                input: input.to_string(),
            });
        }

        let host = normalize_host(addr_part).map_err(|_| EndpointError::MalformedBrackets {
            input: input.to_string(),
        })?;
        Ok(Self {
            host,
            port,
            port_was_explicit: true,
            name: None,
        })
    }

    /// Consume the spec and produce an [`Endpoint`] with a generated UUID.
    #[allow(dead_code)]
    #[must_use]
    pub fn into_endpoint(self) -> Endpoint {
        Endpoint::new(self.host, self.port, self.name)
    }
}

/// Validate a display name.
///
/// Returns `Ok(())` if the name is valid, or an error describing why it
/// is not.
pub fn validate_name(name: &str) -> Result<(), EndpointError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(EndpointError::InvalidName {
            reason: "name is empty".to_string(),
        });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(EndpointError::InvalidName {
            reason: "name contains control characters".to_string(),
        });
    }
    if trimmed.len() > MAX_ENDPOINT_NAME_LEN {
        return Err(EndpointError::InvalidName {
            reason: format!(
                "name is {} bytes, exceeds maximum of {MAX_ENDPOINT_NAME_LEN}",
                trimmed.len()
            ),
        });
    }
    Ok(())
}

pub(crate) fn normalize_host(host: &str) -> Result<String, EndpointError> {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return Err(EndpointError::EmptyHost);
    }

    // URL authorities encode the zone separator as `%25`. Accept the
    // operator-friendly bare `%zone` spelling, but persist the URL-safe form
    // so the poller can construct a valid request URL.
    if let Some((address, zone)) = trimmed.split_once('%') {
        if address.parse::<std::net::Ipv6Addr>().is_ok() && !zone.contains(':') {
            let zone = zone.strip_prefix("25").unwrap_or(zone);
            if zone.is_empty() {
                return Err(EndpointError::EmptyHost);
            }
            return Ok(format!("{address}%25{zone}"));
        }
    }

    // If it parses as an IP address, normalize through the standard library.
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return Ok(ip.to_string());
    }

    // DNS name: preserve case as configured. Matching is case-insensitive
    // (RFC 1035) in [`Endpoint::matches_host`], [`Config::validate`], and
    // the `cmd_add`/`cmd_remove` duplicate checks — see those sites for
    // the canonicalization rules.
    Ok(trimmed.to_string())
}

fn raw_authority_host(authority: &str) -> Option<&str> {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map(|(host, _)| host);
    }
    match authority.rsplit_once(':') {
        Some((host, _)) if !host.contains(':') => Some(host),
        _ => Some(authority),
    }
}

fn raw_authority_port(authority: &str) -> Option<&str> {
    if let Some(close) = authority.find(']') {
        return authority
            .get(close + 1..)
            .and_then(|rest| rest.strip_prefix(':'));
    }
    authority
        .rsplit_once(':')
        .filter(|(host, _)| !host.contains(':'))
        .map(|(_, port)| port)
}

fn is_ipv6_with_zone_id(host: &str) -> bool {
    let Some((address, zone)) = host.split_once('%') else {
        return false;
    };
    let zone = zone.strip_prefix("25").unwrap_or(zone);
    !zone.is_empty() && !zone.contains(':') && address.parse::<std::net::Ipv6Addr>().is_ok()
}

fn parse_port(port_str: &str, full_input: &str) -> Result<u16, EndpointError> {
    let trimmed = port_str.trim();
    if trimmed.is_empty() {
        return Err(EndpointError::MalformedBrackets {
            input: full_input.to_string(),
        });
    }
    let port: u64 = trimmed.parse().map_err(|_| EndpointError::PortNotANumber {
        input: full_input.to_string(),
    })?;
    if port == 0 {
        return Err(EndpointError::PortZero);
    }
    if port > u64::from(u16::MAX) {
        return Err(EndpointError::PortOverflow {
            input: full_input.to_string(),
        });
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(port as u16)
}

fn rsplit_once_colon(s: &str) -> (&str, &str) {
    // Safety: caller guarantees at least one colon.
    debug_assert!(s.contains(':'));
    let idx = s.rfind(':').unwrap();
    (&s[..idx], &s[idx + 1..])
}

/// Canonical display address for a host and port.
#[must_use]
pub fn display_address(host: &str, port: u16) -> String {
    let host = bracketed_host(host).unwrap_or_else(|_| unbracketed_host(host).to_string());
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Normalize and bracket a host for an authority or display address.
pub(crate) fn bracketed_host(host: &str) -> Result<String, EndpointError> {
    let host = unbracketed_host(host);
    normalize_host(host)
}

fn unbracketed_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Valid IPv4 parsing ---

    #[test]
    fn ipv4_with_default_port() {
        let spec = EndpointSpec::parse("192.168.1.1").unwrap();
        assert_eq!(spec.host, "192.168.1.1");
        assert_eq!(spec.port, DEFAULT_PORT);
        assert!(!spec.port_was_explicit);
        assert!(spec.name.is_none());
    }

    #[test]
    fn ipv6_with_empty_encoded_zone_is_rejected() {
        assert!(matches!(
            EndpointSpec::parse("[fe80::1%25]:8080"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
        assert!(normalize_host("fe80::1%25").is_err());
    }

    #[test]
    fn bracketed_ipv6_zone_is_normalized_once_for_persistence() {
        let spec = EndpointSpec::parse("[fe80::1%2525thfloor]:8080").unwrap();
        assert_eq!(spec.host, "fe80::1%2525thfloor");
    }

    #[test]
    fn ipv4_with_explicit_port() {
        let spec = EndpointSpec::parse("192.168.1.1:8080").unwrap();
        assert_eq!(spec.host, "192.168.1.1");
        assert_eq!(spec.port, 8080);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn ipv4_localhost() {
        let spec = EndpointSpec::parse("127.0.0.1:11310").unwrap();
        assert_eq!(spec.host, "127.0.0.1");
        assert_eq!(spec.port, 11310);
        assert!(spec.port_was_explicit);
    }

    // --- Valid DNS parsing ---

    #[test]
    fn dns_hostname_with_default_port() {
        let spec = EndpointSpec::parse("server.local").unwrap();
        assert_eq!(spec.host, "server.local");
        assert_eq!(spec.port, DEFAULT_PORT);
        assert!(!spec.port_was_explicit);
    }

    #[test]
    fn dns_hostname_with_port() {
        let spec = EndpointSpec::parse("server.local:9090").unwrap();
        assert_eq!(spec.host, "server.local");
        assert_eq!(spec.port, 9090);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn mdns_hostname() {
        let spec = EndpointSpec::parse("macmini.local:11320").unwrap();
        assert_eq!(spec.host, "macmini.local");
        assert_eq!(spec.port, 11320);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn add_input_accepts_http_urls_and_discards_path() {
        let spec =
            EndpointSpec::parse_add_input("http://192.168.183.143:11310/v2/status?x=1#fragment")
                .unwrap();
        assert_eq!(spec.host, "192.168.183.143");
        assert_eq!(spec.port, 11310);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn add_input_preserves_missing_and_explicit_default_http_ports() {
        let missing = EndpointSpec::parse_add_input("http://192.168.183.143/").unwrap();
        assert_eq!(missing.port, DEFAULT_PORT);
        assert!(!missing.port_was_explicit);

        let explicit = EndpointSpec::parse_add_input("http://192.168.183.143:80/").unwrap();
        assert_eq!(explicit.port, 80);
        assert!(explicit.port_was_explicit);
    }

    #[test]
    fn add_input_accepts_bracketed_ipv6_and_rejects_non_http_urls() {
        let spec = EndpointSpec::parse_add_input("http://[fd00::10]:11310/").unwrap();
        assert_eq!(spec.host, "fd00::10");
        assert_eq!(spec.port, 11310);

        assert!(matches!(
            EndpointSpec::parse_add_input("https://host:11310/"),
            Err(EndpointError::UnsupportedScheme { .. })
        ));
        assert!(matches!(
            EndpointSpec::parse_add_input("http://user:password@host:11310/"),
            Err(EndpointError::HasCredentials { .. })
        ));
    }

    #[test]
    fn add_input_preserves_a_zone_name_starting_with_25() {
        let spec = EndpointSpec::parse("[fe80::1%2525thfloor]:11310")
            .expect("endpoint with an encoded zone should parse");
        assert_eq!(spec.host, "fe80::1%2525thfloor");
    }

    // --- Valid IPv6 parsing ---

    #[test]
    fn bracketed_ipv6_with_port() {
        let spec = EndpointSpec::parse("[::1]:8080").unwrap();
        assert_eq!(spec.host, "::1");
        assert_eq!(spec.port, 8080);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn bracketed_ipv6_full_address() {
        let spec = EndpointSpec::parse("[fe80::1%25eth0]:8080").unwrap();
        assert_eq!(spec.host, "fe80::1%25eth0");
        assert_eq!(spec.port, 8080);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn bracketed_non_ipv6_literal_is_rejected() {
        assert!(matches!(
            EndpointSpec::parse("[not-an-ip]:8080"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
    }

    #[test]
    fn bare_ipv6_with_default_port() {
        let spec = EndpointSpec::parse("::1").unwrap();
        assert_eq!(spec.host, "::1");
        assert_eq!(spec.port, DEFAULT_PORT);
        assert!(!spec.port_was_explicit);
    }

    #[test]
    fn bare_ipv6_zone_id_with_default_port() {
        for input in ["fe80::1%eth0", "fe80::1%25eth0"] {
            let spec = EndpointSpec::parse(input).unwrap();
            assert_eq!(spec.host, "fe80::1%25eth0");
            assert_eq!(spec.port, DEFAULT_PORT);
            assert!(!spec.port_was_explicit);
        }
    }

    #[test]
    fn unbracketed_ipv6_zone_id_with_port_is_parsed_explicitly() {
        let spec = EndpointSpec::parse("fe80::1%eth0:8080").unwrap();
        assert_eq!(spec.host, "fe80::1%25eth0");
        assert_eq!(spec.port, 8080);
        assert!(spec.port_was_explicit);
    }

    // --- Rejection tests ---

    #[test]
    fn empty_input_rejected() {
        assert!(matches!(
            EndpointSpec::parse(""),
            Err(EndpointError::EmptyInput)
        ));
    }

    #[test]
    fn whitespace_only_rejected() {
        assert!(matches!(
            EndpointSpec::parse("   "),
            Err(EndpointError::EmptyInput)
        ));
    }

    #[test]
    fn scheme_rejected() {
        assert!(matches!(
            EndpointSpec::parse("http://192.168.1.1:8080"),
            Err(EndpointError::HasScheme { .. })
        ));
    }

    #[test]
    fn https_scheme_rejected() {
        assert!(matches!(
            EndpointSpec::parse("https://server.local"),
            Err(EndpointError::HasScheme { .. })
        ));
    }

    #[test]
    fn path_rejected() {
        assert!(matches!(
            EndpointSpec::parse("server.local/status"),
            Err(EndpointError::HasPath { .. })
        ));
    }

    #[test]
    fn credentials_rejected() {
        assert!(matches!(
            EndpointSpec::parse("user:pass@server.local"),
            Err(EndpointError::HasCredentials { .. })
        ));
    }

    #[test]
    fn port_zero_rejected() {
        assert!(matches!(
            EndpointSpec::parse("192.168.1.1:0"),
            Err(EndpointError::PortZero)
        ));
    }

    #[test]
    fn port_overflow_rejected() {
        assert!(matches!(
            EndpointSpec::parse("192.168.1.1:99999"),
            Err(EndpointError::PortOverflow { .. })
        ));
    }

    #[test]
    fn port_not_a_number_rejected() {
        assert!(matches!(
            EndpointSpec::parse("192.168.1.1:abc"),
            Err(EndpointError::PortNotANumber { .. })
        ));
    }

    #[test]
    fn malformed_bracket_no_close() {
        assert!(matches!(
            EndpointSpec::parse("[::1"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
    }

    #[test]
    fn malformed_bracket_no_port() {
        assert!(matches!(
            EndpointSpec::parse("[::1]"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
    }

    #[test]
    fn malformed_bracket_no_colon_after_close() {
        assert!(matches!(
            EndpointSpec::parse("[::1]8080"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
    }

    #[test]
    fn empty_bracket_host_rejected() {
        assert!(matches!(
            EndpointSpec::parse("[]:8080"),
            Err(EndpointError::EmptyHost)
        ));
    }

    #[test]
    fn empty_port_after_bracket_rejected() {
        assert!(matches!(
            EndpointSpec::parse("[::1]:"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
    }

    #[test]
    fn empty_ipv6_zone_id_is_rejected() {
        assert!(matches!(
            EndpointSpec::parse("fe80::1%"),
            Err(EndpointError::MalformedBrackets { .. })
        ));
    }

    // --- Normalization ---

    #[test]
    fn ipv4_normalized() {
        let spec = EndpointSpec::parse("0.0.0.1:80").unwrap();
        assert_eq!(spec.host, "0.0.0.1");
    }

    #[test]
    fn ipv6_loopback_normalized() {
        let spec = EndpointSpec::parse("[0000::0001]:80").unwrap();
        assert_eq!(spec.host, "::1");
    }

    // --- Endpoint display ---

    #[test]
    fn ipv4_display() {
        let ep = Endpoint::new("192.168.1.1".into(), 11310, None);
        assert_eq!(ep.display_address(), "192.168.1.1:11310");
    }

    #[test]
    fn ipv6_display() {
        let ep = Endpoint::new("::1".into(), 8080, None);
        assert_eq!(ep.display_address(), "[::1]:8080");
    }

    #[test]
    fn display_address_normalizes_bare_ipv6_zone_ids() {
        assert_eq!(
            display_address("fe80::1%eth0", 8080),
            "[fe80::1%25eth0]:8080"
        );
    }

    #[test]
    fn endpoint_with_name_display() {
        let ep = Endpoint::new("192.168.1.1".into(), 11310, Some("Server".into()));
        assert_eq!(format!("{ep}"), "Server  192.168.1.1:11310");
    }

    #[test]
    fn endpoint_without_name_display() {
        let ep = Endpoint::new("192.168.1.1".into(), 11310, None);
        assert_eq!(format!("{ep}"), "  192.168.1.1:11310");
    }

    // --- Host matching ---

    #[test]
    fn matches_host_exact_ip() {
        let ep = Endpoint::new("192.168.1.1".into(), 11310, None);
        assert!(ep.matches_host("192.168.1.1"));
    }

    #[test]
    fn matches_host_dns_case_insensitive() {
        let ep = Endpoint::new("Server.Local".into(), 11310, None);
        assert!(ep.matches_host("server.local"));
        assert!(ep.matches_host("SERVER.LOCAL"));
    }

    #[test]
    fn matches_full_exact() {
        let ep = Endpoint::new("192.168.1.1".into(), 11310, None);
        assert!(ep.matches_full("192.168.1.1:11310"));
    }

    #[test]
    fn matches_full_wrong_port() {
        let ep = Endpoint::new("192.168.1.1".into(), 11310, None);
        assert!(!ep.matches_full("192.168.1.1:8080"));
    }

    // --- Display address helper ---

    #[test]
    fn display_address_ipv4() {
        assert_eq!(display_address("192.168.1.1", 8080), "192.168.1.1:8080");
    }

    #[test]
    fn display_address_ipv6() {
        assert_eq!(display_address("::1", 8080), "[::1]:8080");
    }

    // --- Name validation ---

    #[test]
    fn valid_name() {
        assert!(validate_name("My Server").is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn control_characters_in_name_rejected() {
        assert!(validate_name("server\nname").is_err());
        assert!(validate_name("server\u{7f}name").is_err());
    }

    #[test]
    fn long_name_rejected() {
        assert!(validate_name(&"x".repeat(MAX_ENDPOINT_NAME_LEN + 1)).is_err());
    }

    // --- Endpoint ID is unique ---

    #[test]
    fn endpoint_ids_are_unique() {
        let ep1 = Endpoint::new("192.168.1.1".into(), 11310, None);
        let ep2 = Endpoint::new("192.168.1.1".into(), 11310, None);
        assert_ne!(ep1.id, ep2.id);
    }

    // --- Error display messages ---

    #[test]
    fn error_messages_are_human_readable() {
        let cases: Vec<Box<dyn std::error::Error>> = vec![
            Box::new(EndpointError::EmptyInput),
            Box::new(EndpointError::HasScheme {
                input: "http://x".into(),
            }),
            Box::new(EndpointError::PortZero),
        ];
        for err in cases {
            assert!(!format!("{err}").is_empty());
        }
    }

    // --- Default port constant ---

    #[test]
    fn default_port_matches_daemon() {
        assert_eq!(DEFAULT_PORT, 11310);
    }

    // --- into_endpoint ---

    #[test]
    fn into_endpoint_assigns_uuid() {
        let spec = EndpointSpec::parse("192.168.1.1:8080").unwrap();
        let ep = spec.into_endpoint();
        assert_eq!(ep.host, "192.168.1.1");
        assert_eq!(ep.port, 8080);
        assert!(!ep.id.is_empty());
    }

    // --- Edge cases ---

    #[test]
    fn trailing_whitespace_trimmed() {
        let spec = EndpointSpec::parse("192.168.1.1:8080  ").unwrap();
        assert_eq!(spec.host, "192.168.1.1");
        assert_eq!(spec.port, 8080);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn leading_whitespace_trimmed() {
        let spec = EndpointSpec::parse("  192.168.1.1:8080").unwrap();
        assert_eq!(spec.host, "192.168.1.1");
        assert_eq!(spec.port, 8080);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn bare_ipv6_loopback() {
        let spec = EndpointSpec::parse("::1").unwrap();
        assert_eq!(spec.host, "::1");
        assert_eq!(spec.port, DEFAULT_PORT);
        assert!(!spec.port_was_explicit);
    }

    #[test]
    fn ipv6_with_zone_id_bracketed() {
        let spec = EndpointSpec::parse("[fe80::1%25eth0]:443").unwrap();
        assert_eq!(spec.host, "fe80::1%25eth0");
        assert_eq!(spec.port, 443);
        assert!(spec.port_was_explicit);
    }

    #[test]
    fn scheme_with_path_rejected() {
        // Double-check: scheme detection happens before path detection.
        let result = EndpointSpec::parse("http://user:pass@server.local/path");
        assert!(result.is_err());
    }

    #[test]
    fn single_colon_in_hostname_rejected_as_port() {
        // "server:local" — this will be parsed as host:port with non-numeric port.
        let result = EndpointSpec::parse("server:local");
        assert!(matches!(result, Err(EndpointError::PortNotANumber { .. })));
    }
}
