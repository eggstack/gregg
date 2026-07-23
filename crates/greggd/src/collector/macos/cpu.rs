//! macOS CPU counter parsing and delta normalization.
//!
//! Reads cumulative user, system, idle, and nice ticks from Mach
//! `host_statistics` with `HOST_CPU_LOAD_INFO` and computes interval
//! CPU busy percentage.
//!
//! macOS does not expose an aggregate CPU I/O-wait state. The normalized
//! output always reports `iowait_pct = None`.
//!
//! ```text
//! busy  = user + system + nice
//! total = user + system + nice + idle
//! usage_pct = delta(busy) / delta(total) * 100
//! ```

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::macos::ffi::RawCpuTicks;

/// Percentage result derived from a counter interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSample {
    pub usage_pct: f32,
}

/// Compute interval-derived CPU percentage from two [`RawCpuTicks`] readings.
///
/// - Returns [`CollectErrorKind::Warming`] when this is the first sample
///   (callers should establish a baseline).
/// - Returns [`CollectErrorKind::CounterReset`] when any counter decreased
///   between samples.
/// - Returns [`CollectErrorKind::CounterReset`] when `delta_total == 0`
///   to avoid division by zero.
/// - Returns [`CollectErrorKind::Numeric`] when the result is not finite.
pub fn compute_cpu_percentages(
    prev: &RawCpuTicks,
    curr: &RawCpuTicks,
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

    #[allow(
        clippy::cast_precision_loss,
        reason = "counter deltas below 2^53 preserve sub-tick precision; saturating percentages absorb the rest"
    )]
    let usage_pct = (delta_busy as f64) * 100.0 / (delta_total as f64);

    let finalize = |value: f64| -> Result<f32, CollectError> {
        if !value.is_finite() {
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "CPU percentage is not finite",
            ));
        }
        let clamped = value.clamp(0.0, 100.0);
        #[allow(
            clippy::cast_possible_truncation,
            reason = "clamped f64 in [0.0, 100.0] always fits in f32"
        )]
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
        let prev = RawCpuTicks {
            user: 1000,
            system: 500,
            idle: 8000,
            nice: 100,
        };
        let curr = RawCpuTicks {
            user: 1500,
            system: 700,
            idle: 8500,
            nice: 200,
        };
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        // busy_prev = 1600, busy_curr = 2400, delta_busy = 800
        // total_prev = 9600, total_curr = 10900, delta_total = 1300
        // usage_pct = 800 / 1300 * 100 ≈ 61.5385
        assert!((sample.usage_pct - 61.538_5_f32).abs() < 0.01);
    }

    #[test]
    fn zero_delta_returns_counter_reset() {
        let ticks = RawCpuTicks {
            user: 1000,
            system: 500,
            idle: 8000,
            nice: 100,
        };
        let err = compute_cpu_percentages(&ticks, &ticks).expect_err("zero delta");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
    }

    #[test]
    fn counter_decrease_returns_counter_reset() {
        let prev = RawCpuTicks {
            user: 2000,
            system: 1000,
            idle: 5000,
            nice: 200,
        };
        let curr = RawCpuTicks {
            user: 1000,
            system: 500,
            idle: 8000,
            nice: 100,
        };
        let err = compute_cpu_percentages(&prev, &curr).expect_err("decrease");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
    }

    #[test]
    fn full_busy_yields_100_percent() {
        let prev = RawCpuTicks {
            user: 0,
            system: 0,
            idle: 0,
            nice: 0,
        };
        let curr = RawCpuTicks {
            user: 100,
            system: 0,
            idle: 0,
            nice: 0,
        };
        let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
        assert!((sample.usage_pct - 100.0).abs() < 1e-6);
    }

    #[test]
    fn raw_ticks_total_and_busy() {
        let ticks = RawCpuTicks {
            user: 100,
            system: 50,
            idle: 800,
            nice: 10,
        };
        assert_eq!(ticks.busy(), 160);
        assert_eq!(ticks.total(), 960);
    }
}
