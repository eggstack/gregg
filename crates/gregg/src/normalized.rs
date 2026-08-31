//! Normalized internal snapshot for the client.
//!
//! The client monitors mixed v1/v2 fleets. Rather than branching on wire
//! version throughout the codebase, we normalize both v1 and v2 snapshots
//! into a single internal type that the state reducer and UI consume.

use gregg_protocol::{LoadAverage, MemoryMetrics, SystemIdentity};

/// Client-owned drive record independent of wire schema version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedDrive {
    pub name: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: Option<u64>,
}

/// Derived aggregate capacity for a normalized drive list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriveAggregate {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub usage_pct: f32,
}

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
    /// `None` means unavailable/legacy; `Some(empty)` means successful empty enumeration.
    pub drives: Option<Vec<NormalizedDrive>>,
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
            drives: None,
        }
    }

    /// Normalize a v2 wire snapshot into the internal representation.
    pub fn from_v2(snap: &gregg_protocol::v2::StatusSnapshotV2) -> Self {
        Self::from_v2_parts(snap, None)
    }

    /// Normalize a v2 status payload including its optional drive data.
    pub fn from_v2_payload(payload: &gregg_protocol::v2::StatusPayloadV2) -> Self {
        let drives = payload.drives.as_ref().map(|drives| {
            drives
                .iter()
                .map(|drive| NormalizedDrive {
                    name: drive.name.clone(),
                    used_bytes: drive.used_bytes,
                    total_bytes: drive.total_bytes,
                    available_bytes: drive.available_bytes,
                })
                .collect()
        });
        Self::from_v2_parts(&payload.snapshot, drives)
    }

    fn from_v2_parts(
        snap: &gregg_protocol::v2::StatusSnapshotV2,
        drives: Option<Vec<NormalizedDrive>>,
    ) -> Self {
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
            drives,
        }
    }
}

/// Aggregate normalized drives without allowing integer sums to wrap.
///
/// Individual drives that would overflow the running totals are skipped
/// rather than poisoning the whole-fleet aggregate, so a single corrupt
/// entry cannot blank the displayed totals.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn aggregate_drives(drives: &[NormalizedDrive]) -> Option<DriveAggregate> {
    if drives.is_empty() {
        return None;
    }
    let mut used_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut available_bytes: u64 = 0;
    let mut accumulated = false;
    for drive in drives {
        if drive.total_bytes == 0 || drive.used_bytes > drive.total_bytes {
            continue;
        }
        let available = drive
            .available_bytes
            .unwrap_or(drive.total_bytes - drive.used_bytes);
        if available > drive.total_bytes {
            continue;
        }
        let (Some(new_used), Some(new_total), Some(new_available)) = (
            used_bytes.checked_add(drive.used_bytes),
            total_bytes.checked_add(drive.total_bytes),
            available_bytes.checked_add(available),
        ) else {
            // Adding this drive would overflow; skip it and continue
            // accumulating the remaining drives.
            continue;
        };
        used_bytes = new_used;
        total_bytes = new_total;
        available_bytes = new_available;
        accumulated = true;
    }
    if total_bytes == 0 || !accumulated {
        return None;
    }
    let usage_pct = if used_bytes >= total_bytes {
        100.0
    } else {
        (used_bytes as f64 / total_bytes as f64 * 100.0) as f32
    };
    Some(DriveAggregate {
        used_bytes,
        total_bytes,
        available_bytes,
        usage_pct: usage_pct.clamp(0.0, 100.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg_protocol::test_support::{LinuxSnapshotBuilder, MacosSnapshotBuilder};
    use gregg_protocol::v2::SCHEMA_VERSION_V2;

    fn drive(name: &str, used_bytes: u64, total_bytes: u64) -> NormalizedDrive {
        NormalizedDrive {
            name: name.into(),
            used_bytes,
            total_bytes,
            available_bytes: None,
        }
    }

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

    #[test]
    fn v1_and_old_v2_have_unavailable_drives() {
        let v1 = NormalizedSnapshot::from_v1(&LinuxSnapshotBuilder::default().build());
        assert!(v1.drives.is_none());
        let v2 = NormalizedSnapshot::from_v2_payload(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default().build_payload(),
        );
        assert!(v2.drives.is_none());
    }

    #[test]
    fn v2_drive_order_and_empty_state_are_preserved() {
        let payload = gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
            .drives(Some(vec![
                gregg_protocol::v2::DriveMetrics {
                    name: "/".into(),
                    used_bytes: 1,
                    total_bytes: 2,
                    available_bytes: None,
                },
                gregg_protocol::v2::DriveMetrics {
                    name: "/home".into(),
                    used_bytes: 3,
                    total_bytes: 4,
                    available_bytes: None,
                },
            ]))
            .build_payload();
        let norm = NormalizedSnapshot::from_v2_payload(&payload);
        assert_eq!(
            norm.drives
                .as_ref()
                .unwrap()
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            vec!["/", "/home"]
        );

        let empty = gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
            .drives(Some(Vec::new()))
            .build_payload();
        assert_eq!(
            NormalizedSnapshot::from_v2_payload(&empty).drives,
            Some(Vec::new())
        );
    }

    #[test]
    fn aggregate_drives_computes_exact_totals() {
        let aggregate = aggregate_drives(&[drive("/", 2, 10), drive("/home", 3, 20)]).unwrap();
        assert_eq!(aggregate.used_bytes, 5);
        assert_eq!(aggregate.total_bytes, 30);
        assert_eq!(aggregate.available_bytes, 25);
        assert!((aggregate.usage_pct - 16.666_666).abs() < 0.0001);
    }

    #[test]
    fn aggregate_drives_sums_explicit_availability_independently() {
        let aggregate = aggregate_drives(&[
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 6,
                total_bytes: 10,
                available_bytes: Some(2),
            },
            NormalizedDrive {
                name: "/home".into(),
                used_bytes: 3,
                total_bytes: 10,
                available_bytes: Some(4),
            },
        ])
        .unwrap();
        assert_eq!(aggregate.used_bytes, 9);
        assert_eq!(aggregate.total_bytes, 20);
        assert_eq!(aggregate.available_bytes, 6);
    }

    #[test]
    fn aggregate_drives_rejects_empty_and_invalid_input() {
        assert!(aggregate_drives(&[]).is_none());
        assert!(aggregate_drives(&[drive("/", 0, 0)]).is_none());
        assert!(aggregate_drives(&[drive("/", 2, 1)]).is_none());
    }

    #[test]
    fn aggregate_drives_skips_overflowing_entry_and_keeps_valid_drives() {
        // First drive is valid and is accumulated; second drive would
        // overflow every running total, so it is skipped instead of
        // poisoning the aggregate.
        let aggregate =
            aggregate_drives(&[drive("/", 1, 10), drive("/home", u64::MAX, u64::MAX)]).unwrap();
        assert_eq!(aggregate.used_bytes, 1);
        assert_eq!(aggregate.total_bytes, 10);
        assert_eq!(aggregate.available_bytes, 9);
    }

    #[test]
    fn aggregate_drives_skips_invalid_entries() {
        let aggregate = aggregate_drives(&[
            drive("/", 2, 10),
            drive("/invalid", 2, 1),
            NormalizedDrive {
                name: "/also-invalid".into(),
                used_bytes: 1,
                total_bytes: 10,
                available_bytes: Some(11),
            },
        ])
        .unwrap();
        assert_eq!(aggregate.used_bytes, 2);
        assert_eq!(aggregate.total_bytes, 10);
    }

    #[test]
    fn aggregate_drives_handles_maximum_byte_counts() {
        let aggregate = aggregate_drives(&[drive("/", u64::MAX - 1, u64::MAX)]).unwrap();

        assert_eq!(aggregate.used_bytes, u64::MAX - 1);
        assert_eq!(aggregate.total_bytes, u64::MAX);
        assert!((aggregate.usage_pct - 100.0).abs() < f32::EPSILON);
    }
}
