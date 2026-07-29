//! Windows CPU counter parsing and delta normalization.
//!
//! Reads cumulative idle, kernel, and user time counters from
//! `GetSystemTimes` and computes interval CPU busy percentage.
//!
//! Windows aggregate system-time semantics include idle time within kernel
//! time. The normalization uses:
//!
//! ```text
//! total = kernel + user
//! busy  = total - idle
//! usage_pct = delta(busy) / delta(total) * 100
//! ```
//!
//! CPU utilization requires two valid samples. The first sample returns
//! [`CollectErrorKind::Warming`].

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::windows::source::RawCpuTimes;

/// Percentage result derived from a counter interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSample {
    pub usage_pct: f32,
}

/// Maximum supported logical processors for a single-group CPU
/// aggregation. Systems with more processors than this in a single group
/// are rejected because `GetSystemTimes` only covers one processor group.
pub const MAX_SINGLE_GROUP_LOGICAL_PROCESSORS: u32 = 64;

/// Compute interval-derived CPU percentage from two [`RawCpuTimes`] readings.
///
/// # Behavior
///
/// - Returns [`CollectErrorKind::Warming`] when `prev == curr` (identical
///   samples).
/// - Returns [`CollectErrorKind::CounterReset`] when any counter decreased.
/// - Returns [`CollectErrorKind::CounterReset`] when `delta_total == 0`.
/// - Returns [`CollectErrorKind::Numeric`] when the result is not finite.
///
/// # Formula
///
/// Windows kernel time includes idle time:
///
/// ```text
/// total = kernel + user
/// busy  = total - idle = kernel - idle + user
/// ```
///
/// Both `delta(busy)` and `delta(total)` are computed from the raw
/// counters using checked arithmetic.
pub fn compute_cpu_percentages(
    prev: &RawCpuTimes,
    curr: &RawCpuTimes,
) -> Result<CpuSample, CollectError> {
    let delta = |before: u64, after: u64| -> Result<u64, CollectError> {
        if after < before {
            return Err(CollectError::counter_reset(
                "CPU counter decreased between samples; baseline discarded",
            ));
        }
        Ok(after - before)
    };

    let delta_busy = delta(prev.busy(), curr.busy())?;
    let delta_total = delta(prev.total(), curr.total())?;

    if delta_total == 0 {
        return Err(CollectError::counter_reset(
            "CPU total delta is zero; baseline discarded to avoid division by zero",
        ));
    }

    #[allow(clippy::cast_precision_loss)]
    let usage_pct = (delta_busy as f64) * 100.0 / (delta_total as f64);

    let finalize = |value: f64| -> Result<f32, CollectError> {
        if !value.is_finite() {
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "CPU percentage is not finite",
            ));
        }
        let clamped = value.clamp(0.0, 100.0);
        #[allow(clippy::cast_possible_truncation)]
        let as_f32 = clamped as f32;
        if !as_f32.is_finite() || !(0.0..=100.0).contains(&as_f32) {
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "CPU percentage outside closed 0..=100 interval after conversion",
            ));
        }
        Ok(as_f32)
    };

    Ok(CpuSample {
        usage_pct: finalize(usage_pct)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_delta() {
        let prev = RawCpuTimes {
            idle: 8_000,
            kernel: 8_500,
            user: 1_000,
        };
        let curr = RawCpuTimes {
            idle: 8_500,
            kernel: 9_200,
            user: 1_300,
        };
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        // prev: total=9500, busy=1500; curr: total=10500, busy=2000
        // delta_busy=500, delta_total=1000 => 50.0%
        assert!((sample.usage_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn zero_busy_yields_0_percent() {
        let prev = RawCpuTimes {
            idle: 8_000,
            kernel: 8_000,
            user: 0,
        };
        let curr = RawCpuTimes {
            idle: 9_000,
            kernel: 9_000,
            user: 0,
        };
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        assert!((sample.usage_pct - 0.0).abs() < 1e-6);
    }

    #[test]
    fn full_busy_yields_100_percent() {
        let prev = RawCpuTimes {
            idle: 0,
            kernel: 0,
            user: 0,
        };
        let curr = RawCpuTimes {
            idle: 0,
            kernel: 1_000,
            user: 0,
        };
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        assert!((sample.usage_pct - 100.0).abs() < 1e-6);
    }

    #[test]
    fn identical_counters_return_warming() {
        let ticks = RawCpuTimes {
            idle: 8_000,
            kernel: 8_500,
            user: 1_000,
        };
        let err = compute_cpu_percentages(&ticks, &ticks).expect_err("identical");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
    }

    #[test]
    fn counter_decrease_returns_counter_reset() {
        let prev = RawCpuTimes {
            idle: 2_000,
            kernel: 5_000,
            user: 1_000,
        };
        let curr = RawCpuTimes {
            idle: 1_000,
            kernel: 4_000,
            user: 500,
        };
        let err = compute_cpu_percentages(&prev, &curr).expect_err("decrease");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
    }

    #[test]
    fn twenty_five_percent() {
        let prev = RawCpuTimes {
            idle: 0,
            kernel: 0,
            user: 0,
        };
        let curr = RawCpuTimes {
            idle: 750,
            kernel: 1_000,
            user: 0,
        };
        // total=1000, busy=1000-750=250 => 25%
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        assert!((sample.usage_pct - 25.0).abs() < 0.01);
    }

    #[test]
    fn seventy_five_percent() {
        let prev = RawCpuTimes {
            idle: 0,
            kernel: 0,
            user: 0,
        };
        let curr = RawCpuTimes {
            idle: 250,
            kernel: 1_000,
            user: 0,
        };
        // total=1000, busy=750 => 75%
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        assert!((sample.usage_pct - 75.0).abs() < 0.01);
    }

    #[test]
    fn raw_times_total_and_busy() {
        let ticks = RawCpuTimes {
            idle: 800,
            kernel: 850,
            user: 100,
        };
        assert_eq!(ticks.total(), 950);
        assert_eq!(ticks.busy(), 150);
    }

    #[test]
    fn large_counter_values() {
        let prev = RawCpuTimes {
            idle: u64::MAX / 2,
            kernel: u64::MAX / 2 + 100,
            user: u64::MAX / 4,
        };
        let curr = RawCpuTimes {
            idle: u64::MAX / 2 + 100,
            kernel: u64::MAX / 2 + 200,
            user: u64::MAX / 4 + 100,
        };
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        assert!(sample.usage_pct.is_finite());
        assert!((0.0..=100.0).contains(&sample.usage_pct));
    }
}
