//! Windows physical memory normalization from `GlobalMemoryStatusEx`.
//!
//! ```text
//! used_bytes = total_bytes - available_bytes
//! usage_pct  = 100 * used_bytes / total_bytes
//! ```
//!
//! The pagefile fields from `GlobalMemoryStatusEx` are **not** used as
//! swap. They reflect system commit semantics and are distinct from the
//! swap metric Gregg exposes on Unix.

use gregg_protocol::MemoryMetrics;

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::windows::source::RawPhysicalMemory;

/// Parsed memory information normalized into the collector's wire shape.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySample {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl MemorySample {
    /// Convert into the wire [`MemoryMetrics`].
    #[must_use]
    pub fn into_metrics(self) -> MemoryMetrics {
        let usage_pct = if self.total_bytes == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let pct = (self.used_bytes as f64) * 100.0 / (self.total_bytes as f64);
            (pct as f32).clamp(0.0, 100.0)
        };
        MemoryMetrics {
            used_bytes: self.used_bytes,
            total_bytes: self.total_bytes,
            usage_pct,
        }
    }
}

/// Compute memory metrics from raw `GlobalMemoryStatusEx` values.
///
/// # Edge cases
///
/// - Available exceeding total: clamped to total.
/// - Zero total: returns zero used with zero percentage.
/// - API failure: propagated as `SourceUnavailable`.
pub fn compute_memory(raw: &RawPhysicalMemory) -> Result<MemorySample, CollectError> {
    if raw.total_bytes == 0 {
        return Ok(MemorySample {
            used_bytes: 0,
            total_bytes: 0,
        });
    }

    let available = raw.available_bytes.min(raw.total_bytes);
    let used_bytes = raw.total_bytes - available;

    Ok(MemorySample {
        used_bytes,
        total_bytes: raw.total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::windows::source::RawPhysicalMemory;

    #[test]
    fn normal_case() {
        let raw = RawPhysicalMemory {
            total_bytes: 16_000_000_000,
            available_bytes: 10_000_000_000,
        };
        let mem = compute_memory(&raw).expect("computes");
        assert_eq!(mem.used_bytes, 6_000_000_000);
        assert_eq!(mem.total_bytes, 16_000_000_000);
        let metrics = mem.into_metrics();
        assert!((metrics.usage_pct - 37.5).abs() < 0.01);
    }

    #[test]
    fn zero_usage() {
        let raw = RawPhysicalMemory {
            total_bytes: 8_000_000_000,
            available_bytes: 8_000_000_000,
        };
        let mem = compute_memory(&raw).expect("computes");
        assert_eq!(mem.used_bytes, 0);
        let metrics = mem.into_metrics();
        assert!((metrics.usage_pct - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn full_usage() {
        let raw = RawPhysicalMemory {
            total_bytes: 8_000_000_000,
            available_bytes: 0,
        };
        let mem = compute_memory(&raw).expect("computes");
        assert_eq!(mem.used_bytes, 8_000_000_000);
        let metrics = mem.into_metrics();
        assert!((metrics.usage_pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn available_exceeding_total_is_clamped() {
        let raw = RawPhysicalMemory {
            total_bytes: 1_000_000_000,
            available_bytes: u64::MAX,
        };
        let mem = compute_memory(&raw).expect("clamped");
        assert_eq!(mem.used_bytes, 0);
        assert_eq!(mem.total_bytes, 1_000_000_000);
    }

    #[test]
    fn zero_total() {
        let raw = RawPhysicalMemory {
            total_bytes: 0,
            available_bytes: 0,
        };
        let mem = compute_memory(&raw).expect("zero total");
        assert_eq!(mem.used_bytes, 0);
        assert_eq!(mem.total_bytes, 0);
        let metrics = mem.into_metrics();
        assert!((metrics.usage_pct - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn used_never_exceeds_total() {
        for total in [1, 1000, 16_000_000_000] {
            let raw = RawPhysicalMemory {
                total_bytes: total,
                available_bytes: total / 3,
            };
            let mem = compute_memory(&raw).expect("computes");
            assert!(mem.used_bytes <= mem.total_bytes, "total={total}");
        }
    }

    #[test]
    fn into_metrics_produces_valid_percentage() {
        let raw = RawPhysicalMemory {
            total_bytes: 16_000_000_000,
            available_bytes: 10_000_000_000,
        };
        let mem = compute_memory(&raw).expect("computes");
        let metrics = mem.into_metrics();
        assert!(metrics.usage_pct.is_finite());
        assert!((0.0..=100.0).contains(&metrics.usage_pct));
    }

    #[test]
    fn near_u64_max_values() {
        let raw = RawPhysicalMemory {
            total_bytes: u64::MAX,
            available_bytes: u64::MAX / 2,
        };
        let mem = compute_memory(&raw).expect("computes");
        assert!(mem.used_bytes <= mem.total_bytes);
    }
}
