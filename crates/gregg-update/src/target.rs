//! Supported-target mapping and release asset naming.
//!
//! The public asset contract (Plan 099, unchanged by Plan 104):
//!
//! ```text
//! tag: vX.Y.Z
//! asset: <program>-<target>[.exe]
//! checksum: <asset>.sha256
//! ```
//!
//! These constants must stay in sync with `scripts/release-targets.txt`,
//! the Unix/Windows bootstrap installers, and the release workflow. The
//! `supported_targets_match_release_table` test enforces that drift fails
//! loudly instead of shipping divergent names.

/// GitHub repository owning the releases, without URL scheme.
pub const GITHUB_REPO: &str = "eggstack/gregg";

/// Supported binary targets (Plan 099 public contract).
pub const SUPPORTED_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// Detect the current host's Rust target suffix using `std::env::consts`.
/// Returns `Some(target)` for the five supported prebuilt targets, `None`
/// for source-only/unknown hosts (`ARMv7`, FreeBSD, etc.) which should use
/// the Cargo fallback.
#[must_use]
pub fn detect_target() -> Option<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    detect_target_for(os, arch)
}

/// Pure helper for tests: map OS/ARCH strings to a target.
#[must_use]
pub fn detect_target_for(os: &str, arch: &str) -> Option<String> {
    match (os, arch) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu".to_string()),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu".to_string()),
        ("macos", "x86_64") => Some("x86_64-apple-darwin".to_string()),
        ("macos", "aarch64") => Some("aarch64-apple-darwin".to_string()),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc".to_string()),
        _ => None,
    }
}

/// Whether a target is part of the supported binary matrix.
#[must_use]
pub fn is_supported_binary_target(target: &str) -> bool {
    SUPPORTED_TARGETS.contains(&target)
}

/// Asset name for a program+target (no version in filename).
#[must_use]
pub fn asset_name(program: &str, target: &str) -> String {
    if target == "x86_64-pc-windows-msvc" {
        format!("{program}-{target}.exe")
    } else {
        format!("{program}-{target}")
    }
}

/// Construct exact tagged GitHub Release URLs for an asset and its checksum.
#[must_use]
pub fn github_urls(program: &str, target: &str, version: &str) -> (String, String) {
    let asset = asset_name(program, target);
    let base = format!("https://github.com/{GITHUB_REPO}/releases/download/v{version}/{asset}");
    let sha = format!("{base}.sha256");
    (base, sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_mapping() {
        assert_eq!(
            detect_target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu".to_string())
        );
        assert_eq!(
            detect_target_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu".to_string())
        );
        assert_eq!(
            detect_target_for("macos", "x86_64"),
            Some("x86_64-apple-darwin".to_string())
        );
        assert_eq!(
            detect_target_for("macos", "aarch64"),
            Some("aarch64-apple-darwin".to_string())
        );
        assert_eq!(
            detect_target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc".to_string())
        );
        assert_eq!(detect_target_for("linux", "arm"), None);
        assert_eq!(detect_target_for("freebsd", "x86_64"), None);
    }

    #[test]
    fn supported_targets() {
        assert!(is_supported_binary_target("x86_64-unknown-linux-gnu"));
        assert!(is_supported_binary_target("aarch64-unknown-linux-gnu"));
        assert!(is_supported_binary_target("x86_64-apple-darwin"));
        assert!(is_supported_binary_target("aarch64-apple-darwin"));
        assert!(is_supported_binary_target("x86_64-pc-windows-msvc"));
        assert!(!is_supported_binary_target("armv7-unknown-linux-gnueabihf"));
        assert!(!is_supported_binary_target("unknown"));
    }

    #[test]
    fn asset_names() {
        assert_eq!(
            asset_name("gregg", "x86_64-unknown-linux-gnu"),
            "gregg-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            asset_name("gregg", "x86_64-pc-windows-msvc"),
            "gregg-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            asset_name("greggd", "aarch64-apple-darwin"),
            "greggd-aarch64-apple-darwin"
        );
        assert_eq!(
            asset_name("greggd", "x86_64-pc-windows-msvc"),
            "greggd-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn github_urls_format() {
        let (url, sha) = github_urls("gregg", "x86_64-unknown-linux-gnu", "1.0.12");
        assert_eq!(
            url,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/gregg-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            sha,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/gregg-x86_64-unknown-linux-gnu.sha256"
        );
        let (url2, _) = github_urls("greggd", "x86_64-pc-windows-msvc", "1.0.12");
        assert_eq!(
            url2,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/greggd-x86_64-pc-windows-msvc.exe"
        );
    }

    /// Drift prevention (Plan 104): the Rust target table must match the
    /// machine-readable release table consumed by the release scripts and
    /// installers. A new target must be added in both places at once.
    #[test]
    fn supported_targets_match_release_table() {
        let table = include_str!("../../../scripts/release-targets.txt");
        let mut from_table = Vec::new();
        for line in table.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let target = line.split_whitespace().next().unwrap_or("");
            assert!(
                !target.is_empty(),
                "malformed release-targets.txt line: {line:?}"
            );
            from_table.push(target.to_string());
        }
        let mut from_rust: Vec<String> =
            SUPPORTED_TARGETS.iter().map(ToString::to_string).collect();
        from_table.sort();
        from_rust.sort();
        assert_eq!(
            from_rust, from_table,
            "SUPPORTED_TARGETS diverged from scripts/release-targets.txt"
        );
    }
}
