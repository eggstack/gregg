//! Schema-version-2 wire types.
//!
//! Version 2 extends the version-1 snapshot with explicit capability flags
//! for load average, swap, and memory commit. This allows the protocol to
//! truthfully represent Linux, macOS, and Windows metric differences without
//! fabricating unsupported values.
//!
//! V2 snapshots are served from the daemon on a separate endpoint
//! (`/v2/status`). V1 endpoints remain unchanged. Clients prefer v2 but
//! fall back to v1 when the daemon does not support v2 (404 response).

use serde::{Deserialize, Serialize};

pub use crate::{LoadAverage, MemoryMetrics, SystemIdentity};

/// Schema major version 2.
pub const SCHEMA_VERSION_V2: u16 = 2;

/// Maximum number of drive records in a v2 status payload.
pub const MAX_DRIVE_ENTRIES: usize = 32;

/// Maximum UTF-8 byte length of a drive display name.
pub const MAX_DRIVE_NAME_BYTES: usize = 512;

/// Capacity metrics for one operator-visible mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DriveMetrics {
    /// Owned display name supplied by the platform collector.
    pub name: String,
    /// Bytes currently used.
    pub used_bytes: u64,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_bytes: Option<u64>,
}

/// Flat v2 status response with optional drive capacity data.
///
/// The base snapshot is flattened so the JSON shape remains compatible with
/// existing v2 clients. Keeping drives in this wrapper also preserves source
/// compatibility for downstream Rust code that constructs `StatusSnapshotV2`
/// literals. Missing or null `drives` means unavailable/legacy; an empty list
/// means enumeration succeeded and found no eligible filesystems.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusPayloadV2 {
    #[serde(flatten)]
    pub snapshot: StatusSnapshotV2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drives: Option<Vec<DriveMetrics>>,
}

impl StatusPayloadV2 {
    /// Validate the base snapshot and every optional drive record.
    pub fn validate(&self) -> Result<(), Vec<crate::ValidationViolationV2>> {
        crate::validate_v2::validate_payload_v2(self)
    }
}

/// Top-level daemon snapshot for schema version 2.
///
/// V2 extends v1 by making `load`, `swap`, and `commit` optional with
/// explicit capability flags. Platforms that do not support a metric report
/// `false` for the capability and `None` for the value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StatusSnapshotV2 {
    /// Schema major version. Must equal [`SCHEMA_VERSION_V2`].
    pub schema_version: u16,
    /// Unix epoch in milliseconds at which the snapshot was produced.
    pub observed_at_unix_ms: u64,
    /// Sampling cadence in milliseconds used to derive percentage metrics.
    pub sample_interval_ms: u64,
    /// Per-metric capability flags for v2.
    pub capabilities: MetricCapabilitiesV2,
    /// Stable identity fields.
    pub system: SystemIdentity,
    /// CPU utilization.
    pub cpu: CpuMetricsV2,
    /// Load averages. `None` when `capabilities.load_average` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<LoadAverage>,
    /// Physical memory utilization.
    pub memory: MemoryMetrics,
    /// Swap utilization. `None` when `capabilities.swap` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap: Option<SwapMetrics>,
    /// Windows commit charge (or similar commit accounting).
    /// `None` when `capabilities.memory_commit` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitMetrics>,
}

impl StatusSnapshotV2 {
    /// Validate that every field satisfies the version-2 protocol invariants.
    ///
    /// Returns `Ok(())` or a list of structured violations. Each violation
    /// carries a field path and a [`crate::ViolationKindV2`].
    pub fn validate(&self) -> Result<(), Vec<crate::ValidationViolationV2>> {
        crate::validate_v2::validate_v2(self)
    }
}

/// Per-metric capability flags for schema version 2.
///
/// A `false` flag means the metric is **unsupported on this platform**.
/// Servers must report `None` for the corresponding value rather than
/// fabricating a zero or placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::struct_excessive_bools)]
pub struct MetricCapabilitiesV2 {
    /// Whether aggregate CPU I/O wait is reported.
    pub cpu_iowait: bool,
    /// Whether one-/five-/fifteen-minute load averages are reported.
    pub load_average: bool,
    /// Whether swap utilization is reported.
    pub swap: bool,
    /// Whether memory commit charge is reported.
    pub memory_commit: bool,
}

/// CPU utilization snapshot for schema version 2.
///
/// Identical to v1 `CpuMetrics` but placed in the v2 module for
/// independent evolution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CpuMetricsV2 {
    /// Number of logical CPU cores available to the kernel.
    pub logical_cores: u32,
    /// Total CPU busy percentage, `0.0..=100.0`.
    pub usage_pct: f32,
    /// Aggregate CPU I/O-wait percentage, `0.0..=100.0`. `None` when
    /// [`MetricCapabilitiesV2::cpu_iowait`] is `false`.
    pub iowait_pct: Option<f32>,
}

/// Swap utilization for schema version 2.
///
/// When `total_bytes` is zero, `usage_pct` is `0.0` rather than `NaN`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SwapMetrics {
    /// Used swap in bytes. Never exceeds `total_bytes`.
    pub used_bytes: u64,
    /// Total swap in bytes.
    pub total_bytes: u64,
    /// Swap utilization percentage, `0.0..=100.0`.
    pub usage_pct: f32,
}

/// Commit charge metrics (Windows memory commit accounting).
///
/// Represents the system's commit charge: the total bytes committed by all
/// processes, the commit limit, and the derived percentage. This is
/// conceptually distinct from swap and must not be serialized as swap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CommitMetrics {
    /// Committed bytes in use.
    pub used_bytes: u64,
    /// Maximum commit limit in bytes.
    pub limit_bytes: u64,
    /// Commit utilization percentage, `0.0..=100.0`.
    pub usage_pct: f32,
}

/// Health and readiness response for schema version 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HealthResponseV2 {
    /// Daemon schema version, always [`SCHEMA_VERSION_V2`].
    pub schema_version: u16,
    /// Current readiness state.
    pub state: crate::ReadinessState,
    /// Coarse category for non-ready responses. `None` when `state == Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<crate::HealthCategory>,
    /// Short human-readable message. Never includes filesystem paths or
    /// internal error chains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Cached v2 snapshot, present only when `state == Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<StatusSnapshotV2>,
}

impl HealthResponseV2 {
    /// A `Ready` response wrapping the supplied v2 snapshot.
    #[must_use]
    pub fn ready(snapshot: StatusSnapshotV2) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V2,
            state: crate::ReadinessState::Ready,
            category: None,
            message: None,
            snapshot: Some(snapshot),
        }
    }

    /// A `Warming` response with a default message.
    #[must_use]
    pub fn warming() -> Self {
        Self::warming_with_message("collector warming up")
    }

    /// A `Warming` response with a custom message.
    #[must_use]
    pub fn warming_with_message(message: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V2,
            state: crate::ReadinessState::Warming,
            category: Some(crate::HealthCategory::Warming),
            message: Some(message.into()),
            snapshot: None,
        }
    }

    /// A `Failed` response with the given category and message.
    #[must_use]
    pub fn failed(category: crate::HealthCategory, message: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V2,
            state: crate::ReadinessState::Failed,
            category: Some(category),
            message: Some(message.into()),
            snapshot: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HealthCategory, ReadinessState};

    fn v2_identity() -> SystemIdentity {
        SystemIdentity {
            name: "test".into(),
            hostname: "test.local".into(),
            os_name: "linux".into(),
            os_version: "1.0".into(),
            kernel_name: "Linux".into(),
            kernel_release: "6.0.0".into(),
            architecture: "x86_64".into(),
        }
    }

    #[test]
    fn v2_linux_snapshot_round_trips() {
        let snap = StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1_716_460_800_000,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilitiesV2 {
                cpu_iowait: true,
                load_average: true,
                swap: true,
                memory_commit: false,
            },
            system: v2_identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 8,
                usage_pct: 25.2,
                iowait_pct: Some(0.4),
            },
            load: Some(LoadAverage {
                one: 1.32,
                five: 0.91,
                fifteen: 0.62,
            }),
            memory: crate::MemoryMetrics {
                used_bytes: 5_900_000_000,
                total_bytes: 15_600_000_000,
                usage_pct: 37.8,
            },
            swap: Some(SwapMetrics {
                used_bytes: 0,
                total_bytes: 4_000_000_000,
                usage_pct: 0.0,
            }),
            commit: None,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let parsed: StatusSnapshotV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, parsed);
    }

    #[test]
    fn v2_windows_snapshot_no_load_no_swap() {
        let snap = StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1_716_460_800_000,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilitiesV2 {
                cpu_iowait: false,
                load_average: false,
                swap: false,
                memory_commit: true,
            },
            system: v2_identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 4,
                usage_pct: 12.5,
                iowait_pct: None,
            },
            load: None,
            memory: crate::MemoryMetrics {
                used_bytes: 2_000_000_000,
                total_bytes: 8_000_000_000,
                usage_pct: 25.0,
            },
            swap: None,
            commit: Some(CommitMetrics {
                used_bytes: 3_000_000_000,
                limit_bytes: 8_000_000_000,
                usage_pct: 37.5,
            }),
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"load_average\":false"));
        assert!(json.contains("\"swap\":false"));
        assert!(json.contains("\"memory_commit\":true"));
        assert!(!json.contains("\"load\":"));
        assert!(!json.contains("\"swap_used_bytes\""));
        assert!(json.contains("\"commit\""));
        assert!(json.contains("\"used_bytes\""));

        let parsed: StatusSnapshotV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, parsed);
    }

    #[test]
    fn v2_health_ready_round_trips() {
        let snap = StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilitiesV2 {
                cpu_iowait: false,
                load_average: true,
                swap: false,
                memory_commit: true,
            },
            system: v2_identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 4,
                usage_pct: 10.0,
                iowait_pct: None,
            },
            load: Some(LoadAverage {
                one: 1.0,
                five: 0.5,
                fifteen: 0.3,
            }),
            memory: crate::MemoryMetrics {
                used_bytes: 1_000_000_000,
                total_bytes: 4_000_000_000,
                usage_pct: 25.0,
            },
            swap: None,
            commit: Some(CommitMetrics {
                used_bytes: 2_000_000_000,
                limit_bytes: 8_000_000_000,
                usage_pct: 25.0,
            }),
        };
        let health = HealthResponseV2::ready(snap);
        let json = serde_json::to_string(&health).unwrap();
        let parsed: HealthResponseV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(health, parsed);
        assert_eq!(parsed.state, ReadinessState::Ready);
        assert!(parsed.snapshot.is_some());
    }

    #[test]
    fn v2_health_warming_round_trips() {
        let health = HealthResponseV2::warming();
        let json = serde_json::to_string(&health).unwrap();
        let parsed: HealthResponseV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(health, parsed);
        assert_eq!(parsed.state, ReadinessState::Warming);
        assert!(parsed.snapshot.is_none());
    }

    #[test]
    fn v2_health_failed_round_trips() {
        let health = HealthResponseV2::failed(HealthCategory::CollectorFailure, "boom");
        let json = serde_json::to_string(&health).unwrap();
        let parsed: HealthResponseV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(health, parsed);
        assert_eq!(parsed.state, ReadinessState::Failed);
        assert_eq!(parsed.message.as_deref(), Some("boom"));
    }

    #[test]
    fn v2_schema_version_constant() {
        assert_eq!(SCHEMA_VERSION_V2, 2);
    }
}
