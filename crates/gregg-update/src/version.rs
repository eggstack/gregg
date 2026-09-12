//! Stable-version parsing and comparison.
//!
//! The version authority is crates.io `max_stable_version`; only strict
//! `MAJOR.MINOR.PATCH` versions participate in update decisions.

/// Parse a stable `MAJOR.MINOR.PATCH` version. Rejects prerelease/build
/// metadata and any non-numeric component. Returns the three numeric
/// components.
#[must_use]
pub fn parse_stable_version(input: &str) -> Option<(u64, u64, u64)> {
    if input.is_empty() || input.contains('-') || input.contains('+') {
        return None;
    }
    let mut parts = input.split('.');
    let major_str = parts.next()?;
    let minor_str = parts.next()?;
    let patch_str = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    // SemVer 2.0 §2: numeric identifiers must not include leading zeroes.
    for part in [major_str, minor_str, patch_str] {
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
    }
    let major = major_str.parse::<u64>().ok()?;
    let minor = minor_str.parse::<u64>().ok()?;
    let patch = patch_str.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// Compare two stable versions SemVer-safely. Returns an ordering or `None`
/// if either version is not a valid stable `MAJOR.MINOR.PATCH`.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let av = parse_stable_version(a)?;
    let bv = parse_stable_version(b)?;
    Some(av.cmp(&bv))
}

/// Returns `true` when `latest` is strictly newer than `current`.
/// Returns `None` when either side is not a valid stable version.
#[must_use]
pub fn is_update_available(current: &str, latest: &str) -> Option<bool> {
    compare_versions(current, latest).map(|ordering| ordering == std::cmp::Ordering::Less)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stable_versions() {
        assert_eq!(parse_stable_version("1.0.11"), Some((1, 0, 11)));
        assert_eq!(parse_stable_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_stable_version("10.20.30"), Some((10, 20, 30)));
        assert_eq!(parse_stable_version("1.0.0-alpha"), None);
        assert_eq!(parse_stable_version("1.0.0+build"), None);
        assert_eq!(parse_stable_version("1.0"), None);
        assert_eq!(parse_stable_version("1.0.0.0"), None);
        assert_eq!(parse_stable_version(""), None);
        assert_eq!(parse_stable_version("a.b.c"), None);
        assert_eq!(parse_stable_version("01.02.03"), None);
        assert_eq!(parse_stable_version("1.02.3"), None);
        assert_eq!(parse_stable_version("0.0.0"), Some((0, 0, 0)));
    }

    #[test]
    fn version_comparison() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.0.11", "1.0.11"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.0.10", "1.0.11"), Some(Ordering::Less));
        assert_eq!(
            compare_versions("1.0.11", "1.0.10"),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_versions("1.0.9", "1.0.11"), Some(Ordering::Less));
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Some(Ordering::Greater));
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.0.0", "1.0.0-alpha"), None);
    }

    #[test]
    fn update_availability() {
        assert_eq!(is_update_available("1.0.11", "1.0.12"), Some(true));
        assert_eq!(is_update_available("1.0.12", "1.0.12"), Some(false));
        assert_eq!(is_update_available("1.0.13", "1.0.12"), Some(false));
        assert_eq!(is_update_available("1.0.12", "bogus"), None);
    }

    #[test]
    fn crates_io_json_parsing() {
        let json = r#"{"crate":{"max_stable_version":"1.0.11","max_version":"1.0.11"}}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let ver = v["crate"]["max_stable_version"].as_str().unwrap();
        assert_eq!(ver, "1.0.11");
        assert!(parse_stable_version(ver).is_some());
    }
}
