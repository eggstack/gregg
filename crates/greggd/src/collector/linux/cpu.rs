//! Linux CPU counter parsing and delta normalization.
//!
//! Reads the aggregate `cpu` row of `/proc/stat`, retains the previous
//! sample inside the collector, and computes CPU busy / I/O-wait percentages
//! from nonnegative deltas between samples.
//!
//! The definition follows `man 5 proc`:
//!
//! ```text
//! busy   = user + nice + system + irq + softirq + steal
//! iowait = iowait
//! total  = user + nice + system + idle + iowait + irq + softirq + steal
//! usage_pct  = delta(busy)   / delta(total) * 100
//! iowait_pct = delta(iowait) / delta(total) * 100
//! ```
//!
//! Guest time (`guest`, `guest_nice`) is **not** added to `busy` again.
//! Linux already accounts guest time inside `user` and `nice`, per the kernel
//! docs. Adding it here would double-count guest work.

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::linux::source::ParsedProcStat;

/// A single cumulative counter reading from `/proc/stat`.
///
/// All fields are in `USER_HZ` units. `idle` and `iowait` are tracked even
/// though they do not contribute to `busy` so that `total` can be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuCounters {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuCounters {
    /// Sum of every field, used to compute the denominator.
    pub fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    /// Sum of the "busy" fields used in the numerator.
    pub fn busy(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }
}

/// Percentage result derived from a counter interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSample {
    pub usage_pct: f32,
    pub iowait_pct: f32,
}

/// Parse the raw `/proc/stat` content into a [`ParsedProcStat`].
///
/// Returns `Err(Parse)` if the aggregate `cpu` row is missing or has an
/// unexpected number of fields. Per-CPU rows (`cpu0`, `cpu1`, ...) are
/// accepted but ignored; the version-1 collector samples aggregate counters
/// only.
pub fn parse_proc_stat(raw: &str) -> Result<ParsedProcStat, CollectError> {
    let mut aggregate = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            aggregate = Some(parse_cpu_row("cpu", rest)?);
            break;
        }
    }
    Ok(ParsedProcStat { aggregate })
}

fn parse_cpu_row(label: &str, rest: &str) -> Result<CpuCounters, CollectError> {
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `/proc/stat` documents 10 user/sys/idle/.../guest/guest_nice fields for
    // the aggregate `cpu` row. Older kernels may report only 7; we accept
    // both and treat absent trailing fields as zero.
    let expected_min = 7;
    if fields.len() < expected_min {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            format!(
                "expected at least {expected_min} fields on `{label}` row, found {}",
                fields.len()
            ),
        ));
    }
    let mut values = [0u64; 10];
    for (idx, field) in fields.iter().take(10).enumerate() {
        values[idx] = field.parse::<u64>().map_err(|e| {
            CollectError::new(
                CollectErrorKind::Parse,
                format!("non-numeric field on `{label}` row at index {idx}"),
            )
            .with_source(e)
        })?;
    }
    Ok(CpuCounters {
        user: values[0],
        nice: values[1],
        system: values[2],
        idle: values[3],
        iowait: values[4],
        irq: values[5],
        softirq: values[6],
        steal: values[7],
    })
}

/// Compute interval-derived percentages from two [`CpuCounters`] readings.
///
/// - Returns [`CollectErrorKind::Warming`] when `prev` is `None`; the
///   collector establishes a baseline on the first call.
/// - Returns [`CollectErrorKind::CounterReset`] when any counter decreased
///   between samples; the caller is expected to discard the baseline and
///   wait for a subsequent sample.
/// - Returns [`CollectErrorKind::CounterReset`] when `delta_total == 0` so a
///   caller cannot divide by zero. This is the conservative choice: a zero
///   delta is rare in practice but indicates a sampling cadence faster than
///   the kernel clock granularity, in which case the safest response is to
///   restart the baseline.
/// - Returns [`CollectErrorKind::Numeric`] when the computed percentages are
///   not finite or outside the allowed closed interval.
pub fn compute_percentages(
    prev: &CpuCounters,
    curr: &CpuCounters,
) -> Result<CpuSample, CollectError> {
    let delta = |before: u64, after: u64| -> Result<u64, CollectError> {
        if after < before {
            return Err(CollectError::counter_reset(
                "CPU counter decreased between samples; baseline discarded",
            ));
        }
        let diff = after - before;
        Ok(diff)
    };

    let busy_curr = curr.busy();
    let busy_prev = prev.busy();
    let total_curr = curr.total();
    let total_prev = prev.total();

    let delta_busy = delta(busy_prev, busy_curr)?;
    let delta_iowait = delta(prev.iowait, curr.iowait)?;
    let delta_total = delta(total_prev, total_curr)?;

    if delta_total == 0 {
        return Err(CollectError::counter_reset(
            "CPU total delta is zero; baseline discarded to avoid division by zero",
        ));
    }

    // Counter deltas are bounded by the kernel's USER_HZ * interval time
    // (typically `100 * 1_000_000` for a 1 s interval). Counts beyond
    // `2^53` would lose sub-tick precision, but the percentage computation
    // saturates and the validator rejects out-of-range results, so the
    // truncation is benign.
    #[allow(
        clippy::cast_precision_loss,
        reason = "counter deltas below 2^53 USER_HZ preserve sub-tick precision; saturating percentages absorb the rest"
    )]
    let usage_pct = (delta_busy as f64) * 100.0 / (delta_total as f64);
    #[allow(
        clippy::cast_precision_loss,
        reason = "see usage_pct rationale; same arithmetic"
    )]
    let iowait_pct = (delta_iowait as f64) * 100.0 / (delta_total as f64);

    let finalize = |value: f64| -> Result<f32, CollectError> {
        if !value.is_finite() {
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "CPU percentage is not finite",
            ));
        }
        // Clamp tiny overshoot from rounding; tolerate `1e-9` to keep the
        // result within `0.0..=100.0` after `f64 -> f32` conversion.
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
        iowait_pct: finalize(iowait_pct)?,
    })
}
