//! macOS identity collection.
//!
//! Reads hostname, product name/version, Darwin kernel name/release,
//! architecture, and logical core count from native sysctl APIs. No
//! external commands (`sw_vers`, `sysctl`, etc.) are invoked.
//!
//! Product version comes from `/System/Library/CoreServices/SystemVersion.plist`
//! which is read as a plain file. If the plist is unavailable, the version
//! falls back to `"unknown"` rather than fabricating a value.

use gregg_protocol::SystemIdentity;

use crate::collector::error::CollectError;
use crate::collector::macos::ffi::MacNativeQueries;
use crate::collector::macos::normalize::clip_identifier;

/// Maximum input length for identity fields. Longer strings are clipped
/// on a valid UTF-8 boundary.
const MAX_IDENTITY_LEN: usize = 128;

/// Collect identity from a [`MacNativeQueries`] source.
///
/// `display_name`, when provided, overrides the user-facing `name` field.
pub fn collect_identity(
    source: &dyn MacNativeQueries,
    display_name: Option<&str>,
) -> Result<SystemIdentity, CollectError> {
    let raw = source.identity()?;

    let name = match display_name {
        Some(value) => clip_identifier(value, MAX_IDENTITY_LEN),
        None => clip_identifier(&raw.hostname, MAX_IDENTITY_LEN),
    };

    Ok(SystemIdentity {
        name,
        hostname: clip_identifier(&raw.hostname, MAX_IDENTITY_LEN),
        os_name: clip_identifier(&raw.os_name, MAX_IDENTITY_LEN),
        os_version: clip_identifier(&raw.os_version, MAX_IDENTITY_LEN),
        kernel_name: clip_identifier(&raw.kernel_name, MAX_IDENTITY_LEN),
        kernel_release: clip_identifier(&raw.kernel_release, MAX_IDENTITY_LEN),
        architecture: clip_identifier(&raw.architecture, MAX_IDENTITY_LEN),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::macos::ffi::MockNativeQueries;

    #[test]
    fn normal_identity() {
        let mock = MockNativeQueries::success();
        let identity = collect_identity(&mock, None).expect("identity");
        assert_eq!(identity.hostname, "test-mac.local");
        assert_eq!(identity.os_name, "macos");
        assert_eq!(identity.kernel_name, "Darwin");
        assert_eq!(identity.architecture, "arm64");
    }

    #[test]
    fn display_name_overrides() {
        let mock = MockNativeQueries::success();
        let identity = collect_identity(&mock, Some("my-mac")).expect("identity");
        assert_eq!(identity.name, "my-mac");
        assert_eq!(identity.hostname, "test-mac.local");
    }

    #[test]
    fn fallback_to_hostname_when_no_display_name() {
        let mock = MockNativeQueries::success();
        let identity = collect_identity(&mock, None).expect("identity");
        assert_eq!(identity.name, identity.hostname);
    }

    #[test]
    fn error_propagated() {
        let mut mock = MockNativeQueries::success();
        mock.identity_error = true;
        let err = collect_identity(&mock, None).expect_err("must fail");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }
}
