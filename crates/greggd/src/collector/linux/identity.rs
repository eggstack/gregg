//! Linux identity collection.
//!
//! Reads hostname, kernel name/release, architecture, and `/etc/os-release`
//! from procfs/sysfs only. No external commands are invoked.
//!
//! Missing `/etc/os-release` is non-fatal: identity falls back to a truthful
//! generic Linux identity (`os_name = "linux"`, `os_version = "unknown"`)
//! rather than failing the whole collector. Kernel-identity failures are
//! surfaced because they indicate a deeper platform problem.

use gregg_protocol::SystemIdentity;

use crate::collector::error::CollectError;
use crate::collector::linux::source::ProcSource;

/// Maximum input length we accept for any parsed identifier. Strings longer
/// than this are clipped and the remainder discarded; the protocol treats
/// identity fields as display metadata, not as semantic input.
const MAX_IDENTITY_LEN: usize = 128;

/// Collect identity from a [`ProcSource`].
///
/// `display_name`, when provided, replaces the user-facing `name` field. The
/// actual `hostname` always comes from `/proc/sys/kernel/hostname`.
pub fn collect_identity(
    source: &ProcSource,
    display_name: Option<&str>,
) -> Result<SystemIdentity, CollectError> {
    let hostname = source.hostname()?;
    let kernel = source.kernel_identity()?;
    let architecture = source.architecture();
    let _ = source.logical_core_count().unwrap_or(1).max(1);

    let os_release = source.read_os_release()?;
    let (os_name, os_version) = match os_release {
        Some(content) => parse_os_release(&content),
        None => ("linux".to_string(), "unknown".to_string()),
    };

    let name = match display_name {
        Some(value) => clip_identifier(value),
        None => hostname.clone(),
    };

    Ok(SystemIdentity {
        name,
        hostname: clip_identifier(&hostname),
        os_name: clip_identifier(&os_name),
        os_version: clip_identifier(&os_version),
        kernel_name: clip_identifier(&kernel.sysname),
        kernel_release: clip_identifier(&kernel.release),
        architecture: clip_identifier(&architecture),
    })
}

fn clip_identifier(input: &str) -> String {
    if input.len() <= MAX_IDENTITY_LEN {
        input.to_string()
    } else {
        // Keep at most MAX_IDENTITY_LEN bytes by truncating on the largest
        // valid UTF-8 boundary at or before the limit.
        let mut end = MAX_IDENTITY_LEN;
        while end > 0 && !input.is_char_boundary(end) {
            end -= 1;
        }
        input[..end].to_string()
    }
}

/// Parse the standard `/etc/os-release` sh-like syntax used by systemd and
/// most distributions. Returns `(pretty_name_or_name, version)`.
///
/// Quotes and backslash escapes are stripped. Unknown keys are ignored.
fn parse_os_release(content: &str) -> (String, String) {
    let mut pretty_name: Option<String> = None;
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut version_id: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "PRETTY_NAME" => pretty_name = Some(value),
            "NAME" => name = Some(value),
            "VERSION" => version = Some(value),
            "VERSION_ID" => version_id = Some(value),
            _ => {}
        }
    }

    let chosen = pretty_name
        .or(name.clone())
        .unwrap_or_else(|| "linux".to_string());
    let version = version
        .or(version_id)
        .or(name)
        .unwrap_or_else(|| "unknown".to_string());
    (chosen, version)
}

/// Strip surrounding single or double quotes and decode common backslash
/// escapes. Falls back to the input string when quote stripping fails.
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        let inner = &trimmed[1..trimmed.len() - 1];
        inner
            .replace("\\\"", "\"")
            .replace("\\'", "'")
            .replace("\\\\", "\\")
    } else {
        trimmed.to_string()
    }
}

/// Convenience for source-level tests: build a minimal [`SystemIdentity`]
/// from raw identity fields without going through the procfs paths.
#[allow(dead_code, reason = "used by integration tests")]
pub fn synthetic_identity(
    name: &str,
    hostname: &str,
    os_name: &str,
    os_version: &str,
    kernel_name: &str,
    kernel_release: &str,
    architecture: &str,
) -> SystemIdentity {
    SystemIdentity {
        name: name.to_string(),
        hostname: hostname.to_string(),
        os_name: os_name.to_string(),
        os_version: os_version.to_string(),
        kernel_name: kernel_name.to_string(),
        kernel_release: kernel_release.to_string(),
        architecture: architecture.to_string(),
    }
}

/// Light-weight `os-release` fixture loader for source-level tests.
#[allow(dead_code, reason = "used by integration tests")]
pub fn os_release_sample() -> &'static str {
    r#"NAME="Ubuntu"
VERSION="24.04 LTS (Noble Numbat)"
ID=ubuntu
VERSION_ID="24.04"
PRETTY_NAME="Ubuntu 24.04 LTS"
"#
}
