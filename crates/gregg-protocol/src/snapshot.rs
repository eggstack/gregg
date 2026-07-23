//! Snapshot and identity wire types.

use serde::{Deserialize, Serialize};

use crate::ValidationViolation;

/// Top-level daemon snapshot returned by the status endpoint.
///
/// Every numeric field uses raw units. CPU and memory percentages are reported
/// in the closed interval `0.0..=100.0`. Bytes are unsigned 64-bit counts.
/// `observed_at_unix_ms` is the Unix epoch in milliseconds at which the
/// underlying counters were sampled.
///
/// CPU percentage values are derived from sampling-interval deltas, not from
/// instantaneous single reads, so a freshly started daemon may legitimately
/// report a snapshot whose CPU usage is still unknown. Such snapshots surface
/// through the [`HealthResponse`](crate::HealthResponse) instead of through
/// this endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusSnapshot {
    /// Schema major version. Must equal
    /// [`crate::SCHEMA_VERSION_V1`] for this endpoint.
    pub schema_version: u16,
    /// Unix epoch in milliseconds at which the snapshot was produced.
    pub observed_at_unix_ms: u64,
    /// Sampling cadence in milliseconds used to derive percentage metrics.
    pub sample_interval_ms: u64,
    /// Per-metric capability flags.
    pub capabilities: MetricCapabilities,
    /// Stable identity fields reported separately so clients can degrade by
    /// width priority.
    pub system: SystemIdentity,
    /// CPU utilization, with optional Linux aggregate I/O wait.
    pub cpu: CpuMetrics,
    /// One-, five-, and fifteen-minute load averages.
    pub load: LoadAverage,
    /// Physical memory utilization.
    pub memory: MemoryMetrics,
    /// Swap utilization.
    pub swap: SwapMetrics,
}

/// Per-metric capability flags.
///
/// A `false` flag means the metric is **unsupported on this platform**.
/// Servers report `None` for unsupported values; clients render those values
/// as absent rather than as zero.
///
/// A `true` flag means the metric is supported; the corresponding value must
/// still be present in a `Ready` snapshot but may be absent during warmup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MetricCapabilities {
    /// Whether aggregate CPU I/O wait is reported.
    ///
    /// `false` on macOS, where no equivalent accounting state exists.
    /// `true` on Linux.
    pub cpu_iowait: bool,
}

/// Stable identity fields. Each field is transported separately so the TUI
/// can degrade by width priority without parsing a combined string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemIdentity {
    /// User-facing system name (typically a configured alias).
    pub name: String,
    /// Network hostname as reported by the operating system.
    pub hostname: String,
    /// Operating-system family name (e.g. `"linux"`, `"macos"`).
    pub os_name: String,
    /// Operating-system version string.
    pub os_version: String,
    /// Kernel name (e.g. `"Linux"`, `"Darwin"`).
    pub kernel_name: String,
    /// Kernel release string.
    pub kernel_release: String,
    /// Target architecture (e.g. `"x86_64"`, `"aarch64"`).
    pub architecture: String,
}

/// CPU utilization snapshot.
///
/// `usage_pct` is total CPU busy over the most recent sampling interval.
/// `iowait_pct` is the aggregate CPU I/O-wait time over the same interval;
/// it is `Some(_)` only when [`MetricCapabilities::cpu_iowait`] is `true`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CpuMetrics {
    /// Number of logical CPU cores available to the kernel.
    pub logical_cores: u32,
    /// Total CPU busy percentage, `0.0..=100.0`, derived from delta samples.
    pub usage_pct: f32,
    /// Aggregate CPU I/O-wait percentage, `0.0..=100.0`. `None` when the
    /// platform does not expose this state.
    pub iowait_pct: Option<f32>,
}

/// One-, five-, and fifteen-minute load averages as reported by the
/// operating system.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LoadAverage {
    /// One-minute load average.
    pub one: f32,
    /// Five-minute load average.
    pub five: f32,
    /// Fifteen-minute load average.
    pub fifteen: f32,
}

/// Physical memory utilization.
///
/// `usage_pct` is computed as `100.0 * used_bytes / total_bytes` and clamped
/// to the closed interval `0.0..=100.0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MemoryMetrics {
    /// Used physical memory in bytes. Never exceeds `total_bytes`.
    pub used_bytes: u64,
    /// Total physical memory in bytes.
    pub total_bytes: u64,
    /// Memory utilization percentage, `0.0..=100.0`.
    pub usage_pct: f32,
}

/// Swap utilization.
///
/// When `total_bytes` is zero, `usage_pct` is `0.0` rather than `NaN`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SwapMetrics {
    /// Used swap in bytes. Never exceeds `total_bytes`.
    pub used_bytes: u64,
    /// Total swap in bytes.
    pub total_bytes: u64,
    /// Swap utilization percentage, `0.0..=100.0`. Zero when
    /// `total_bytes == 0`.
    pub usage_pct: f32,
}

impl StatusSnapshot {
    /// Validate that every field satisfies the version-1 protocol invariants.
    ///
    /// The returned [`ValidationViolation`] list is structured so callers can
    /// log individual fields and decide whether to reject the snapshot,
    /// surface it as a warning, or fall back to a warming-up health response.
    ///
    /// # Invariants
    ///
    /// - `schema_version == SCHEMA_VERSION_V1`.
    /// - `observed_at_unix_ms > 0` and `sample_interval_ms > 0`.
    /// - `cpu.logical_cores > 0`.
    /// - All percentages are finite and in `0.0..=100.0`.
    /// - `used_bytes <= total_bytes` for memory and swap.
    /// - When `total_bytes == 0`, the corresponding `usage_pct` is `0.0`.
    /// - `iowait_pct` is `None` exactly when `cpu_iowait` capability is
    ///   `false`. A `true` capability with `None` is rejected because a
    ///   `Ready` snapshot must report every supported metric.
    pub fn validate(&self) -> Result<(), Vec<ValidationViolation>> {
        crate::validate::validate(self)
    }
}
