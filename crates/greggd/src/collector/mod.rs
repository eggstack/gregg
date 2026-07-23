//! Native metrics collection.
//!
//! The collector boundary isolates platform-specific sampling from the daemon
//! sampler and the HTTP surface. The shared trait is implemented by per-OS
//! modules that read their own native kernel or user-space interfaces and
//! return a normalized, daemon-internal sample. The sampler in phase 4 owns
//! cadence, clock, and snapshot publication.
//!
//! # Design rules
//!
//! - The collector never spawns external commands. Linux uses procfs and
//!   sysinfo interfaces; macOS uses Mach and sysctl APIs behind a contained
//!   FFI module added in phase 3.
//! - The collector never owns a clock. The daemon samples call
//!   [`SystemCollector::sample`] and stamp [`StatusSnapshot::observed_at_unix_ms`]
//!   in the sampler.
//! - All percentage normalization, counter-delta handling, and warming-up
//!   state live behind the trait, not in the protocol crate.
//! - Errors are typed so the daemon can distinguish a warming baseline from a
//!   hard collector failure when reporting health.

use gregg_protocol::{
    CpuMetrics, LoadAverage, MemoryMetrics, MetricCapabilities, StatusSnapshot, SwapMetrics,
    SystemIdentity,
};

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

pub mod error;

/// Normalized metric sample produced by a [`SystemCollector`].
///
/// The struct is daemon-internal: it carries fields that do not appear on the
/// wire so collectors can express transient states (warming, counter reset)
/// without polluting the protocol. The daemon sampler maps it losslessly into
/// a [`StatusSnapshot`] once it is ready for publication.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectedMetrics {
    /// Logical CPU core count. Always `> 0` for a successfully collected
    /// identity snapshot.
    pub logical_cores: u32,
    /// Aggregate CPU busy percentage derived from a counter interval. `None`
    /// while warming up or immediately after a counter reset.
    pub cpu_usage_pct: Option<f32>,
    /// Aggregate Linux CPU I/O-wait percentage. Always `None` for non-Linux
    /// collectors; on Linux it is `Some` once a valid interval exists.
    pub cpu_iowait_pct: Option<f32>,
    /// Load averages parsed verbatim from the platform source.
    pub load: LoadAverage,
    /// Physical memory utilization.
    pub memory: MemoryMetrics,
    /// Swap utilization.
    pub swap: SwapMetrics,
}

impl CollectedMetrics {
    /// Convert this sample into a wire [`StatusSnapshot`].
    ///
    /// The caller (the daemon sampler) is responsible for filling in
    /// `schema_version`, `observed_at_unix_ms`, and `sample_interval_ms`. CPU
    /// `usage_pct` and `iowait_pct` are coalesced from `Option` into either a
    /// concrete value or a protocol-compatible zero with the right capability
    /// flag set. Callers should not publish a snapshot while
    /// [`Self::cpu_usage_pct`] is `None`; the function performs the coalesce
    /// defensively so the result is always wire-valid for the daemon's
    /// platform capabilities.
    #[must_use]
    pub fn into_snapshot(
        self,
        schema_version: u16,
        observed_at_unix_ms: u64,
        sample_interval_ms: u64,
        capabilities: MetricCapabilities,
        system: SystemIdentity,
    ) -> StatusSnapshot {
        let cpu_usage_pct = self.cpu_usage_pct.unwrap_or(0.0);
        let cpu_iowait_pct = if capabilities.cpu_iowait {
            Some(self.cpu_iowait_pct.unwrap_or(0.0))
        } else {
            None
        };
        StatusSnapshot {
            schema_version,
            observed_at_unix_ms,
            sample_interval_ms,
            capabilities,
            system,
            cpu: CpuMetrics {
                logical_cores: self.logical_cores,
                usage_pct: cpu_usage_pct,
                iowait_pct: cpu_iowait_pct,
            },
            load: self.load,
            memory: self.memory,
            swap: self.swap,
        }
    }
}

/// Shared collector contract implemented by every platform-specific collector.
///
/// The contract is intentionally minimal: it owns identity collection and one
/// incremental sample. The daemon sampler owns cadence and clock.
pub trait SystemCollector: Send {
    /// Read identity fields once and cache them inside the collector.
    ///
    /// Identity is expected to be stable for the lifetime of the daemon, but
    /// re-reading is permitted if the host's identity changes (for example a
    /// hostname rename).
    fn identity(&self) -> Result<SystemIdentity, error::CollectError>;

    /// Take one native sample.
    ///
    /// The first call after construction is expected to return
    /// [`error::CollectErrorKind::Warming`] because percentage metrics
    /// require a second reading. Once two valid samples exist the collector
    /// returns normalized [`CollectedMetrics`].
    fn sample(&mut self) -> Result<CollectedMetrics, error::CollectError>;

    /// Per-platform metric capability flags.
    fn capabilities(&self) -> MetricCapabilities;
}
