//! Windows commit charge normalization from `GetPerformanceInfo`.
//!
//! ```text
//! used_bytes  = commit_total_pages * page_size_bytes
//! limit_bytes = commit_limit_pages  * page_size_bytes
//! usage_pct   = 100 * used_bytes / limit_bytes
//! ```
//!
//! Commit charge is conceptually distinct from swap. The pagefile fields
//! from `GlobalMemoryStatusEx` reflect commit semantics and must not be
//! conflated with Unix swap.

use gregg_protocol::v2::CommitMetrics;

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::windows::source::RawCommit;

/// Parsed commit information normalized into the collector's wire shape.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitSample {
    pub used_bytes: u64,
    pub limit_bytes: u64,
}

impl CommitSample {
    /// Convert into the wire [`CommitMetrics`].
    #[must_use]
    pub fn into_metrics(self) -> CommitMetrics {
        let usage_pct = if self.limit_bytes == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let pct = (self.used_bytes as f64) * 100.0 / (self.limit_bytes as f64);
            #[allow(clippy::cast_possible_truncation)]
            let as_f32 = pct as f32;
            as_f32.clamp(0.0, 100.0)
        };
        CommitMetrics {
            used_bytes: self.used_bytes,
            limit_bytes: self.limit_bytes,
            usage_pct,
        }
    }
}

/// Compute commit metrics from raw `GetPerformanceInfo` values.
///
/// A transient commit charge above the commit limit (pagefile resize
/// windows, kernel over-commit before expansion) is clamped to the limit
/// for that sample so `usage_pct` saturates at 100 % instead of failing
/// the whole collection cycle. This also preserves the wire invariant
/// that committed bytes never exceed the reported limit.
///
/// # Errors
///
/// Returns [`CollectErrorKind::Parse`] when `page_size_bytes` is zero,
/// and [`CollectErrorKind::Numeric`] on multiplication overflow.
pub fn compute_commit(raw: &RawCommit) -> Result<CommitSample, CollectError> {
    if raw.page_size_bytes == 0 {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            "page size is zero",
        ));
    }

    let commit_total_pages = raw.commit_total_pages.min(raw.commit_limit_pages);

    let used_bytes = commit_total_pages
        .checked_mul(raw.page_size_bytes)
        .ok_or_else(|| {
            CollectError::new(
                CollectErrorKind::Numeric,
                "commit total pages * page_size overflowed u64",
            )
        })?;

    let limit_bytes = raw
        .commit_limit_pages
        .checked_mul(raw.page_size_bytes)
        .ok_or_else(|| {
            CollectError::new(
                CollectErrorKind::Numeric,
                "commit limit pages * page_size overflowed u64",
            )
        })?;

    Ok(CommitSample {
        used_bytes,
        limit_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::windows::source::RawCommit;

    fn sample_commit() -> RawCommit {
        RawCommit {
            commit_total_pages: 200_000,
            commit_limit_pages: 800_000,
            page_size_bytes: 4096,
        }
    }

    #[test]
    fn normal_case() {
        let raw = sample_commit();
        let commit = compute_commit(&raw).expect("computes");
        assert_eq!(commit.used_bytes, 200_000 * 4096);
        assert_eq!(commit.limit_bytes, 800_000 * 4096);
        let metrics = commit.into_metrics();
        assert!((metrics.usage_pct - 25.0).abs() < 0.01);
    }

    #[test]
    fn zero_commit() {
        let raw = RawCommit {
            commit_total_pages: 0,
            commit_limit_pages: 800_000,
            page_size_bytes: 4096,
        };
        let commit = compute_commit(&raw).expect("computes");
        assert_eq!(commit.used_bytes, 0);
        let metrics = commit.into_metrics();
        assert!((metrics.usage_pct - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn full_commit_limit() {
        let raw = RawCommit {
            commit_total_pages: 800_000,
            commit_limit_pages: 800_000,
            page_size_bytes: 4096,
        };
        let commit = compute_commit(&raw).expect("computes");
        let metrics = commit.into_metrics();
        assert!((metrics.usage_pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn total_greater_than_limit_clamps_to_limit() {
        let raw = RawCommit {
            commit_total_pages: 900_000,
            commit_limit_pages: 800_000,
            page_size_bytes: 4096,
        };
        let commit = compute_commit(&raw).expect("over-commit clamps instead of failing");
        assert_eq!(commit.used_bytes, 800_000 * 4096);
        assert_eq!(commit.limit_bytes, 800_000 * 4096);
        let metrics = commit.into_metrics();
        assert!((metrics.usage_pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn page_size_zero_fails() {
        let raw = RawCommit {
            commit_total_pages: 200_000,
            commit_limit_pages: 800_000,
            page_size_bytes: 0,
        };
        let err = compute_commit(&raw).expect_err("should fail");
        assert_eq!(err.kind, CollectErrorKind::Parse);
    }

    #[test]
    fn multiplication_overflow() {
        let raw = RawCommit {
            commit_total_pages: u64::MAX,
            commit_limit_pages: u64::MAX,
            page_size_bytes: 4096,
        };
        let err = compute_commit(&raw).expect_err("overflow");
        assert_eq!(err.kind, CollectErrorKind::Numeric);
    }

    #[test]
    fn large_valid_values() {
        let raw = RawCommit {
            commit_total_pages: 1_000_000_000,
            commit_limit_pages: 2_000_000_000,
            page_size_bytes: 4096,
        };
        let commit = compute_commit(&raw).expect("computes");
        let metrics = commit.into_metrics();
        assert!((metrics.usage_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn into_metrics_produces_valid_percentage() {
        let raw = sample_commit();
        let commit = compute_commit(&raw).expect("computes");
        let metrics = commit.into_metrics();
        assert!(metrics.usage_pct.is_finite());
        assert!((0.0..=100.0).contains(&metrics.usage_pct));
    }
}
