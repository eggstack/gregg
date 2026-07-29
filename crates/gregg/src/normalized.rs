//! Normalized internal snapshot for the client.
//!
//! The client monitors mixed v1/v2 fleets. Rather than branching on wire
//! version throughout the codebase, we normalize both v1 and v2 snapshots
//! into a single internal type that the state reducer and UI consume.

use gregg_protocol::{LoadAverage, MemoryMetrics, SystemIdentity};

/// Normalized snapshot that the client uses internally.
///
/// Derived from either a v1 or v2 wire snapshot. Optional fields follow
/// v2 semantics: `None` means the metric is unsupported on the platform.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct NormalizedSnapshot {
    /// Schema version of the original wire snapshot.
    pub wire_version: u16,
    /// Unix epoch in milliseconds when the snapshot was produced.
    pub observed_at_unix_ms: u64,
    /// Sampling cadence in milliseconds.
    pub sample_interval_ms: u64,
    /// Whether the platform supports CPU I/O wait.
    pub cpu_iowait_supported: bool,
    /// Whether the platform supports load averages.
    pub load_supported: bool,
    /// Whether the platform supports swap metrics.
    pub swap_supported: bool,
    /// Whether the platform supports memory commit metrics.
    pub commit_supported: bool,
    /// Stable identity fields.
    pub system: SystemIdentity,
    /// Logical CPU core count.
    pub logical_cores: u32,
    /// CPU usage percentage.
    pub usage_pct: f32,
    /// CPU I/O wait percentage, if supported.
    pub iowait_pct: Option<f32>,
    /// Load averages, if supported.
    pub load: Option<LoadAverage>,
    /// Physical memory utilization.
    pub memory: MemoryMetrics,
    /// Swap utilization, if supported.
    pub swap: Option<SwapMetrics>,
    /// Commit charge metrics, if supported.
    pub commit: Option<CommitMetrics>,
}

/// Swap utilization (normalized from v1 or v2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapMetrics {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub usage_pct: f32,
}

/// Commit charge metrics (normalized from v2 only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommitMetrics {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub usage_pct: f32,
}

impl NormalizedSnapshot {
    /// Normalize a v1 wire snapshot into the internal representation.
    pub fn from_v1(snap: &gregg_protocol::StatusSnapshot) -> Self {
        Self {
            wire_version: gregg_protocol::SCHEMA_VERSION_V1,
            observed_at_unix_ms: snap.observed_at_unix_ms,
            sample_interval_ms: snap.sample_interval_ms,
            cpu_iowait_supported: snap.capabilities.cpu_iowait,
            load_supported: true,
            swap_supported: true,
            commit_supported: false,
            system: snap.system.clone(),
            logical_cores: snap.cpu.logical_cores,
            usage_pct: snap.cpu.usage_pct,
            iowait_pct: snap.cpu.iowait_pct,
            load: Some(snap.load),
            memory: snap.memory,
            swap: Some(SwapMetrics {
                used_bytes: snap.swap.used_bytes,
                total_bytes: snap.swap.total_bytes,
                usage_pct: snap.swap.usage_pct,
            }),
            commit: None,
        }
    }

    /// Normalize a v2 wire snapshot into the internal representation.
    pub fn from_v2(snap: &gregg_protocol::v2::StatusSnapshotV2) -> Self {
        Self {
            wire_version: gregg_protocol::v2::SCHEMA_VERSION_V2,
            observed_at_unix_ms: snap.observed_at_unix_ms,
            sample_interval_ms: snap.sample_interval_ms,
            cpu_iowait_supported: snap.capabilities.cpu_iowait,
            load_supported: snap.capabilities.load_average,
            swap_supported: snap.capabilities.swap,
            commit_supported: snap.capabilities.memory_commit,
            system: snap.system.clone(),
            logical_cores: snap.cpu.logical_cores,
            usage_pct: snap.cpu.usage_pct,
            iowait_pct: snap.cpu.iowait_pct,
            load: snap.load,
            memory: snap.memory,
            swap: snap.swap.as_ref().map(|s| SwapMetrics {
                used_bytes: s.used_bytes,
                total_bytes: s.total_bytes,
                usage_pct: s.usage_pct,
            }),
            commit: snap.commit.as_ref().map(|c| CommitMetrics {
                used_bytes: c.used_bytes,
                limit_bytes: c.limit_bytes,
                usage_pct: c.usage_pct,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg_protocol::test_support::{LinuxSnapshotBuilder, MacosSnapshotBuilder};
    use gregg_protocol::v2::SCHEMA_VERSION_V2;

    #[test]
    fn from_v1_preserves_load_and_swap() {
        let snap = LinuxSnapshotBuilder::default().build();
        let norm = NormalizedSnapshot::from_v1(&snap);
        assert_eq!(norm.wire_version, gregg_protocol::SCHEMA_VERSION_V1);
        assert!(norm.load.is_some());
        assert!(norm.swap.is_some());
        assert!(!norm.commit_supported);
    }

    #[test]
    fn from_v1_macos_iowait_unsupported() {
        let snap = MacosSnapshotBuilder::default().build();
        let norm = NormalizedSnapshot::from_v1(&snap);
        assert!(!norm.cpu_iowait_supported);
        assert!(norm.iowait_pct.is_none());
    }

    #[test]
    fn from_v2_linux_preserves_optional_fields() {
        let snap = gregg_protocol::test_support::LinuxSnapshotV2Builder::default().build();
        let norm = NormalizedSnapshot::from_v2(&snap);
        assert_eq!(norm.wire_version, SCHEMA_VERSION_V2);
        assert!(norm.load.is_some());
        assert!(norm.swap.is_some());
        assert!(!norm.commit_supported);
        assert!(norm.cpu_iowait_supported);
    }

    #[test]
    fn from_v2_windows_has_commit_no_swap_or_load() {
        let snap = gregg_protocol::test_support::WindowsSnapshotV2Builder::default().build();
        let norm = NormalizedSnapshot::from_v2(&snap);
        assert_eq!(norm.wire_version, SCHEMA_VERSION_V2);
        assert!(!norm.load_supported);
        assert!(!norm.swap_supported);
        assert!(norm.commit_supported);
        assert!(norm.load.is_none());
        assert!(norm.swap.is_none());
        assert!(norm.commit.is_some());
    }
}
