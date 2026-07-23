//! macOS memory normalization from Mach VM statistics.
//!
//! Uses `host_statistics64` with `HOST_VM_INFO64` for page counts and
//! `host_page_size` for the page size. Physical memory total comes from
//! `hw.memsize`.
//!
//! Version-1 normalization:
//!
//! ```text
//! available_pages = free_count + inactive_count
//! available_bytes = available_pages * page_size
//! total_bytes = physical_memory (from hw.memsize)
//! used_bytes = total_bytes - min(available_bytes, total_bytes)
//! ```
//!
//! This definition favors availability-oriented semantics suitable for a
//! compact cross-platform utilization bar. It does not claim exact equality
//! with Activity Monitor's "Memory Used" or memory-pressure model.

use gregg_protocol::MemoryMetrics;

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::macos::ffi::RawVmStats;
use crate::collector::macos::normalize::percent;

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
        let usage_pct = percent(self.used_bytes, self.total_bytes);
        MemoryMetrics {
            used_bytes: self.used_bytes,
            total_bytes: self.total_bytes,
            usage_pct,
        }
    }
}

/// Compute memory metrics from raw VM statistics and physical memory total.
///
/// `total_bytes` is the physical memory from `hw.memsize`.
///
/// # Edge cases
///
/// - Available bytes transiently exceeding total: clamped to total.
/// - Zero total: returns zero used with zero percentage.
/// - Page size or count overflow: returns a `Numeric` error.
pub fn compute_memory(raw: &RawVmStats, total_bytes: u64) -> Result<MemorySample, CollectError> {
    if total_bytes == 0 {
        return Ok(MemorySample {
            used_bytes: 0,
            total_bytes: 0,
        });
    }

    let available_pages = raw.free_count.saturating_add(raw.inactive_count);

    let available_bytes = available_pages.checked_mul(raw.page_size).ok_or_else(|| {
        CollectError::new(
            CollectErrorKind::Numeric,
            "available pages * page_size overflowed u64",
        )
    })?;

    let available_bytes = available_bytes.min(total_bytes);
    let used_bytes = total_bytes - available_bytes;

    Ok(MemorySample {
        used_bytes,
        total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::macos::ffi::RawVmStats;

    fn sample_vm() -> RawVmStats {
        RawVmStats {
            free_count: 100_000,
            active_count: 200_000,
            inactive_count: 150_000,
            wire_count: 50_000,
            page_size: 16_384,
        }
    }

    #[test]
    fn normal_case() {
        let raw = sample_vm();
        let total = 16_000_000_000;
        let mem = compute_memory(&raw, total).expect("computes");
        // available = (100_000 + 150_000) * 16_384 = 250_000 * 16_384 = 4_096_000_000
        // used = 16_000_000_000 - 4_096_000_000 = 11_904_000_000
        assert_eq!(mem.used_bytes, 11_904_000_000);
        assert_eq!(mem.total_bytes, total);
    }

    #[test]
    fn zero_total() {
        let raw = sample_vm();
        let mem = compute_memory(&raw, 0).expect("zero total");
        assert_eq!(mem.used_bytes, 0);
        assert_eq!(mem.total_bytes, 0);
    }

    #[test]
    fn available_exceeding_total_is_clamped() {
        let raw = RawVmStats {
            free_count: u64::MAX / 16_384,
            active_count: 0,
            inactive_count: 0,
            wire_count: 0,
            page_size: 16_384,
        };
        let total = 1_000_000_000;
        let mem = compute_memory(&raw, total).expect("clamped");
        assert_eq!(mem.used_bytes, 0);
        assert_eq!(mem.total_bytes, total);
    }

    #[test]
    fn used_never_exceeds_total() {
        let raw = sample_vm();
        for total in [1, 1000, 16_000_000_000] {
            let mem = compute_memory(&raw, total).expect("computes");
            assert!(mem.used_bytes <= mem.total_bytes, "total={total}");
        }
    }

    #[test]
    fn into_metrics_produces_valid_percentage() {
        let raw = sample_vm();
        let mem = compute_memory(&raw, 16_000_000_000).expect("computes");
        let metrics = mem.into_metrics();
        assert!(metrics.usage_pct.is_finite());
        assert!((0.0..=100.0).contains(&metrics.usage_pct));
    }
}
