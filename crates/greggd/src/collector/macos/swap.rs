//! macOS swap normalization from sysctl `vm.swapusage`.
//!
//! A host with no configured swap returns zero total and zero used with
//! `usage_pct = 0.0`. Compressed memory is not treated as swap unless
//! the native interface reports it as such.

use gregg_protocol::SwapMetrics;

use crate::collector::macos::ffi::RawSwapUsage;
use crate::collector::macos::normalize::percent;

/// Parsed swap information normalized into the collector's wire shape.
#[derive(Debug, Clone, PartialEq)]
pub struct SwapSample {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl SwapSample {
    /// Convert into the wire [`SwapMetrics`], handling zero total swap by
    /// returning `usage_pct == 0.0` rather than dividing by zero.
    #[must_use]
    pub fn into_metrics(self) -> SwapMetrics {
        let usage_pct = percent(self.used_bytes, self.total_bytes);
        SwapMetrics {
            used_bytes: self.used_bytes,
            total_bytes: self.total_bytes,
            usage_pct,
        }
    }
}

/// Compute swap metrics from raw sysctl values.
///
/// Normalizes used to never exceed total.
pub fn compute_swap(raw: &RawSwapUsage) -> SwapSample {
    let used = raw.used_bytes.min(raw.total_bytes);
    SwapSample {
        used_bytes: used,
        total_bytes: raw.total_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_swap() {
        let raw = RawSwapUsage {
            total_bytes: 0,
            used_bytes: 0,
        };
        let swap = compute_swap(&raw);
        assert_eq!(swap.used_bytes, 0);
        assert_eq!(swap.total_bytes, 0);
        let metrics = swap.into_metrics();
        assert!((metrics.usage_pct - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn normal_swap() {
        let raw = RawSwapUsage {
            total_bytes: 4_000_000_000,
            used_bytes: 1_000_000_000,
        };
        let swap = compute_swap(&raw);
        assert_eq!(swap.used_bytes, 1_000_000_000);
        assert_eq!(swap.total_bytes, 4_000_000_000);
        let metrics = swap.into_metrics();
        assert!((metrics.usage_pct - 25.0).abs() < 0.01);
    }

    #[test]
    fn used_exceeding_total_is_clamped() {
        let raw = RawSwapUsage {
            total_bytes: 1_000_000_000,
            used_bytes: 2_000_000_000,
        };
        let swap = compute_swap(&raw);
        assert_eq!(swap.used_bytes, 1_000_000_000);
        assert_eq!(swap.total_bytes, 1_000_000_000);
        let metrics = swap.into_metrics();
        assert!((metrics.usage_pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn used_never_exceeds_total() {
        for (total, used) in [(0, 0), (100, 50), (100, 100), (100, 200)] {
            let raw = RawSwapUsage {
                total_bytes: total,
                used_bytes: used,
            };
            let swap = compute_swap(&raw);
            assert!(swap.used_bytes <= swap.total_bytes);
        }
    }
}
