//! Windows identity collection.
//!
//! Reads hostname, OS version, kernel name/release, and architecture
//! from native Windows APIs. No external commands (`wmic`, `systeminfo`,
//! etc.) are invoked.
//!
//! Identity semantics:
//!
//! - `name`: configured display name or hostname.
//! - `hostname`: native computer/DNS hostname from `GetComputerNameExW`.
//! - `os_name`: `"windows"`.
//! - `os_version`: product version/build string from `RtlGetVersion` or
//!   `GetVersionExW`.
//! - `kernel_name`: `"Windows NT"`.
//! - `kernel_release`: numeric build/revision string.
//! - `architecture`: Rust target architecture normalization.

use gregg_protocol::SystemIdentity;

use crate::collector::error::CollectError;
use crate::collector::windows::source::WindowsSource;

/// Maximum input length for identity fields. Longer strings are clipped
/// on a valid UTF-8 boundary.
const MAX_IDENTITY_LEN: usize = 128;

/// Collect identity from a [`WindowsSource`] source.
///
/// `display_name`, when provided, overrides the user-facing `name` field.
pub fn collect_identity(
    source: &dyn WindowsSource,
    display_name: Option<&str>,
) -> Result<SystemIdentity, CollectError> {
    let raw = source.identity()?;

    let hostname = clip_identifier(&raw.hostname, MAX_IDENTITY_LEN);
    if hostname.is_empty() {
        return Err(CollectError::new(
            crate::collector::error::CollectErrorKind::Parse,
            "hostname is empty after normalization",
        ));
    }

    let name = match display_name {
        Some(value) => clip_identifier(value, MAX_IDENTITY_LEN),
        None => hostname.clone(),
    };

    Ok(SystemIdentity {
        name,
        hostname,
        os_name: "windows".to_string(),
        os_version: clip_identifier(&raw.os_version, MAX_IDENTITY_LEN),
        kernel_name: "Windows NT".to_string(),
        kernel_release: clip_identifier(&raw.os_version, MAX_IDENTITY_LEN),
        architecture: clip_identifier(&raw.architecture, MAX_IDENTITY_LEN),
    })
}

/// Clip an identifier string to the maximum length on a valid UTF-8
/// boundary.
fn clip_identifier(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.trim().to_string();
    }
    // Find a valid UTF-8 boundary at or before max_len.
    let mut end = max_len;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::windows::source::MockWindowsSource;

    #[test]
    fn normal_identity() {
        let mock = MockWindowsSource::success();
        let identity = collect_identity(&mock, None).expect("identity");
        assert_eq!(identity.hostname, "win-server");
        assert_eq!(identity.os_name, "windows");
        assert_eq!(identity.kernel_name, "Windows NT");
        assert_eq!(identity.architecture, "x86_64");
    }

    #[test]
    fn display_name_overrides() {
        let mock = MockWindowsSource::success();
        let identity = collect_identity(&mock, Some("my-server")).expect("identity");
        assert_eq!(identity.name, "my-server");
        assert_eq!(identity.hostname, "win-server");
    }

    #[test]
    fn fallback_to_hostname_when_no_display_name() {
        let mock = MockWindowsSource::success();
        let identity = collect_identity(&mock, None).expect("identity");
        assert_eq!(identity.name, identity.hostname);
    }

    #[test]
    fn error_propagated() {
        let mut mock = MockWindowsSource::success();
        mock.identity_error = true;
        let err = collect_identity(&mock, None).expect_err("must fail");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }

    #[test]
    fn unicode_hostname() {
        let mut mock = MockWindowsSource::success();
        mock.identity.hostname = "sv\u{00e4}r".to_string();
        let identity = collect_identity(&mock, None).expect("identity");
        assert_eq!(identity.hostname, "sv\u{00e4}r");
    }

    #[test]
    fn empty_hostname_rejected() {
        let mut mock = MockWindowsSource::success();
        mock.identity.hostname = String::new();
        let err = collect_identity(&mock, None).expect_err("empty hostname");
        assert_eq!(err.kind, crate::collector::error::CollectErrorKind::Parse);
    }

    #[test]
    fn whitespace_only_hostname_rejected() {
        let mut mock = MockWindowsSource::success();
        mock.identity.hostname = "   ".to_string();
        let err = collect_identity(&mock, None).expect_err("whitespace hostname");
        assert_eq!(err.kind, crate::collector::error::CollectErrorKind::Parse);
    }

    #[test]
    fn oversized_hostname_is_clipped() {
        let mut mock = MockWindowsSource::success();
        mock.identity.hostname = "a".repeat(200);
        let identity = collect_identity(&mock, None).expect("identity");
        assert!(identity.hostname.len() <= MAX_IDENTITY_LEN);
    }

    #[test]
    fn architecture_preserved() {
        let mut mock = MockWindowsSource::success();
        mock.identity.architecture = "x86_64".to_string();
        let identity = collect_identity(&mock, None).expect("identity");
        assert_eq!(identity.architecture, "x86_64");
    }

    #[test]
    fn clip_identifier_basic() {
        assert_eq!(clip_identifier("hello", 10), "hello");
        assert_eq!(clip_identifier("hello", 3), "hel");
    }

    #[test]
    fn clip_identifier_trims_whitespace() {
        assert_eq!(clip_identifier("  hello  ", 10), "hello");
    }
}
