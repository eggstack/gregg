//! Shared validation and bounding for native drive results.
//!
//! Moved without behavioral change from `greggd::collector::drives`
//! (Plan 134). Bounds come from [`CollectionLimits`](crate::model::CollectionLimits)
//! instead of `gregg-protocol` constants.

use crate::model::{CollectionLimits, DriveMetrics};

/// An owned, platform-neutral candidate before wire normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveCandidate {
    /// Stable identity used for deduplication.
    pub identity: String,
    /// Display mount/drive name.
    pub name: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Total free bytes (not caller-available).
    pub total_free_bytes: u64,
    /// Caller-available bytes.
    pub available_bytes: u64,
}

/// Convert valid native candidates into deterministic, bounded records
/// using Gregg's default limits.
pub fn normalize(candidates: Vec<DriveCandidate>) -> Vec<DriveMetrics> {
    normalize_with_limits(candidates, &CollectionLimits::gregg_defaults())
}

/// Convert valid native candidates into deterministic, bounded records
/// using explicit limits.
pub fn normalize_with_limits(
    mut candidates: Vec<DriveCandidate>,
    limits: &CollectionLimits,
) -> Vec<DriveMetrics> {
    candidates.retain(|candidate| {
        !candidate.identity.is_empty()
            && !candidate.name.is_empty()
            && candidate.name.len() <= limits.max_drive_name_bytes
            && candidate.total_bytes > 0
            && candidate.total_free_bytes <= candidate.total_bytes
            && candidate.available_bytes <= candidate.total_bytes
    });

    candidates.sort_by(|left, right| {
        left.identity
            .cmp(&right.identity)
            .then_with(|| left.name.cmp(&right.name))
    });
    candidates.dedup_by(|left, right| left.identity == right.identity);
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.truncate(limits.max_drive_entries);

    candidates
        .into_iter()
        .map(|candidate| DriveMetrics {
            name: candidate.name,
            used_bytes: candidate.total_bytes - candidate.total_free_bytes,
            total_bytes: candidate.total_bytes,
            available_bytes: Some(candidate.available_bytes),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_rejects_invalid_values_deduplicates_and_bounds() {
        let limits = CollectionLimits::gregg_defaults();
        let mut candidates = vec![
            DriveCandidate {
                identity: "same".to_string(),
                name: "/z".to_string(),
                total_bytes: 10,
                total_free_bytes: 2,
                available_bytes: 2,
            },
            DriveCandidate {
                identity: "same".to_string(),
                name: "/a".to_string(),
                total_bytes: 10,
                total_free_bytes: 3,
                available_bytes: 3,
            },
            DriveCandidate {
                identity: "bad".to_string(),
                name: "/bad".to_string(),
                total_bytes: 1,
                total_free_bytes: 2,
                available_bytes: 2,
            },
        ];
        candidates.extend(
            (0..(limits.max_drive_entries + 2)).map(|index| DriveCandidate {
                identity: format!("id-{index}"),
                name: format!("/{index}"),
                total_bytes: 10,
                total_free_bytes: 1,
                available_bytes: 1,
            }),
        );

        let normalized = normalize(candidates);
        assert_eq!(normalized.len(), limits.max_drive_entries);
        assert_eq!(normalized[0].name, "/0");
        assert!(normalized
            .iter()
            .all(|drive| drive.used_bytes <= drive.total_bytes));
        assert!(!normalized.iter().any(|drive| drive.name == "/bad"));
    }
}
