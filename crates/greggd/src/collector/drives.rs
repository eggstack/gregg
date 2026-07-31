//! Shared validation and bounding for native drive results.

use gregg_protocol::v2::{DriveMetrics, MAX_DRIVE_ENTRIES, MAX_DRIVE_NAME_BYTES};

/// An owned, platform-neutral candidate before wire normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DriveCandidate {
    pub(crate) identity: String,
    pub(crate) name: String,
    pub(crate) total_bytes: u64,
    pub(crate) free_bytes: u64,
}

/// Convert valid native candidates into deterministic, bounded wire records.
pub(crate) fn normalize(mut candidates: Vec<DriveCandidate>) -> Vec<DriveMetrics> {
    candidates.retain(|candidate| {
        !candidate.identity.is_empty()
            && !candidate.name.is_empty()
            && candidate.name.len() <= MAX_DRIVE_NAME_BYTES
            && candidate.total_bytes > 0
            && candidate.free_bytes <= candidate.total_bytes
    });

    candidates.sort_by(|left, right| {
        left.identity
            .cmp(&right.identity)
            .then_with(|| left.name.cmp(&right.name))
    });
    candidates.dedup_by(|left, right| left.identity == right.identity);
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.truncate(MAX_DRIVE_ENTRIES);

    candidates
        .into_iter()
        .map(|candidate| DriveMetrics {
            name: candidate.name,
            used_bytes: candidate.total_bytes - candidate.free_bytes,
            total_bytes: candidate.total_bytes,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_rejects_invalid_values_deduplicates_and_bounds() {
        let mut candidates = vec![
            DriveCandidate {
                identity: "same".to_string(),
                name: "/z".to_string(),
                total_bytes: 10,
                free_bytes: 2,
            },
            DriveCandidate {
                identity: "same".to_string(),
                name: "/a".to_string(),
                total_bytes: 10,
                free_bytes: 3,
            },
            DriveCandidate {
                identity: "bad".to_string(),
                name: "/bad".to_string(),
                total_bytes: 1,
                free_bytes: 2,
            },
        ];
        candidates.extend((0..(MAX_DRIVE_ENTRIES + 2)).map(|index| DriveCandidate {
            identity: format!("id-{index}"),
            name: format!("/{index}"),
            total_bytes: 10,
            free_bytes: 1,
        }));

        let normalized = normalize(candidates);
        assert_eq!(normalized.len(), MAX_DRIVE_ENTRIES);
        assert_eq!(normalized[0].name, "/0");
        assert!(normalized
            .iter()
            .all(|drive| drive.used_bytes <= drive.total_bytes));
        assert!(!normalized.iter().any(|drive| drive.name == "/bad"));
    }
}
