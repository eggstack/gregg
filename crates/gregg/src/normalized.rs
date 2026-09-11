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

/// Client-owned disk-I/O device record independent of the wire schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedDiskIoDevice {
    pub id: String,
    pub name: String,
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
    pub drive_name: Option<String>,
}

/// Compatibility name for callers that prefer the wire-model terminology.
pub type NormalizedDiskIoMetrics = NormalizedDiskIoDevice;

/// Normalized aggregate and per-device disk throughput.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedDiskIo {
    pub aggregate_read_bytes_per_sec: u64,
    pub aggregate_write_bytes_per_sec: u64,
    pub devices: Vec<NormalizedDiskIoDevice>,
}

/// Client-owned network interface record independent of the wire schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedNetworkInterface {
    pub id: String,
    pub name: String,
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
    pub rx_capacity_bps: Option<u64>,
    pub tx_capacity_bps: Option<u64>,
    pub is_loopback: bool,
    pub aggregate_member: bool,
}

/// Compatibility name for callers that prefer the wire-model terminology.
pub type NormalizedNetworkInterfaceMetrics = NormalizedNetworkInterface;

/// Normalized aggregate and per-interface network telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedNetwork {
    pub aggregate_rx_bytes_per_sec: u64,
    pub aggregate_tx_bytes_per_sec: u64,
    pub aggregate_rx_capacity_bps: Option<u64>,
    pub aggregate_tx_capacity_bps: Option<u64>,
    pub interfaces: Vec<NormalizedNetworkInterface>,
}

impl NormalizedNetwork {
    /// Derive aggregate link-capacity utilization from directional values.
    ///
    /// Receive and transmit directions are evaluated separately and the
    /// larger valid percentage is returned. This keeps simultaneous full
    /// duplex traffic at 100%, rather than incorrectly adding the two
    /// directions. Throughput remains available through the raw fields when
    /// one or both capacities are unknown.
    #[must_use]
    pub fn aggregate_utilization_pct(&self) -> Option<f32> {
        network_utilization_pct(
            self.aggregate_rx_bytes_per_sec,
            self.aggregate_tx_bytes_per_sec,
            self.aggregate_rx_capacity_bps,
            self.aggregate_tx_capacity_bps,
        )
    }
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
    /// Host-level current CPU frequency in Hz, when available.
    pub cpu_frequency_hz: Option<u64>,
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
    /// Optional disk-I/O throughput.
    pub disk_io: Option<NormalizedDiskIo>,
    /// Optional network throughput and capacity.
    pub network: Option<NormalizedNetwork>,
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
            cpu_frequency_hz: None,
            load: Some(snap.load),
            memory: snap.memory,
            swap: Some(SwapMetrics {
                used_bytes: snap.swap.used_bytes,
                total_bytes: snap.swap.total_bytes,
                usage_pct: snap.swap.usage_pct,
            }),
            commit: None,
            drives: None,
            disk_io: None,
            network: None,
        }
    }

    /// Normalize a v2 wire snapshot into the internal representation.
    pub fn from_v2(snap: &gregg_protocol::v2::StatusSnapshotV2) -> Self {
        Self::from_v2_parts(snap, None, None, None, None)
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
        let disk_io = payload.disk_io.as_ref().map(normalize_disk_io);
        let network = payload.network.as_ref().map(normalize_network);
        Self::from_v2_parts(
            &payload.snapshot,
            drives,
            payload.cpu_frequency_hz,
            disk_io,
            network,
        )
    }

    fn from_v2_parts(
        snap: &gregg_protocol::v2::StatusSnapshotV2,
        drives: Option<Vec<NormalizedDrive>>,
        cpu_frequency_hz: Option<u64>,
        disk_io: Option<NormalizedDiskIo>,
        network: Option<NormalizedNetwork>,
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
            cpu_frequency_hz,
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
            disk_io,
            network,
        }
    }
}

fn normalize_disk_io(payload: &gregg_protocol::v2::DiskIoPayload) -> NormalizedDiskIo {
    NormalizedDiskIo {
        aggregate_read_bytes_per_sec: payload.aggregate_read_bytes_per_sec,
        aggregate_write_bytes_per_sec: payload.aggregate_write_bytes_per_sec,
        devices: payload
            .devices
            .iter()
            .map(|device| NormalizedDiskIoDevice {
                id: device.id.clone(),
                name: device.name.clone(),
                read_bytes_per_sec: device.read_bytes_per_sec,
                write_bytes_per_sec: device.write_bytes_per_sec,
                drive_name: device.drive_name.clone(),
            })
            .collect(),
    }
}

fn normalize_network(payload: &gregg_protocol::v2::NetworkPayload) -> NormalizedNetwork {
    NormalizedNetwork {
        aggregate_rx_bytes_per_sec: payload.aggregate_rx_bytes_per_sec,
        aggregate_tx_bytes_per_sec: payload.aggregate_tx_bytes_per_sec,
        aggregate_rx_capacity_bps: payload.aggregate_rx_capacity_bps,
        aggregate_tx_capacity_bps: payload.aggregate_tx_capacity_bps,
        interfaces: payload
            .interfaces
            .iter()
            .map(|interface| NormalizedNetworkInterface {
                id: interface.id.clone(),
                name: interface.name.clone(),
                rx_bytes_per_sec: interface.rx_bytes_per_sec,
                tx_bytes_per_sec: interface.tx_bytes_per_sec,
                rx_capacity_bps: interface.rx_capacity_bps,
                tx_capacity_bps: interface.tx_capacity_bps,
                is_loopback: interface.is_loopback,
                aggregate_member: interface.aggregate_member,
            })
            .collect(),
    }
}

/// Convert a byte rate to a bit rate without allowing `u64` overflow.
#[must_use]
pub const fn bytes_per_second_to_bits_per_second(bytes_per_sec: u64) -> Option<u64> {
    bytes_per_sec.checked_mul(8)
}

/// Derive full-duplex-safe aggregate network utilization.
///
/// Each valid direction compares bits per second against its own capacity;
/// the result is the maximum valid direction, clamped to `0..=100`. A zero
/// or missing capacity makes that direction unavailable. If both directions
/// lack capacity, the result is `None` even when throughput is present.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#[must_use]
pub fn network_utilization_pct(
    rx_bytes_per_sec: u64,
    tx_bytes_per_sec: u64,
    rx_capacity_bps: Option<u64>,
    tx_capacity_bps: Option<u64>,
) -> Option<f32> {
    let rx = directional_utilization_pct(rx_bytes_per_sec, rx_capacity_bps);
    let tx = directional_utilization_pct(tx_bytes_per_sec, tx_capacity_bps);
    match (rx, tx) {
        (Some(rx), Some(tx)) => Some(rx.max(tx)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn directional_utilization_pct(bytes_per_sec: u64, capacity_bps: Option<u64>) -> Option<f32> {
    let capacity = capacity_bps?;
    if capacity == 0 {
        return None;
    }
    let bits_per_sec = bytes_per_second_to_bits_per_second(bytes_per_sec)?;
    let percentage = (bits_per_sec as f64 / capacity as f64 * 100.0) as f32;
    Some(percentage.clamp(0.0, 100.0))
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
        assert!(norm.cpu_frequency_hz.is_none());
        assert!(norm.disk_io.is_none());
        assert!(norm.network.is_none());
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
        assert!(norm.cpu_frequency_hz.is_none());
        assert!(norm.disk_io.is_none());
        assert!(norm.network.is_none());
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
        assert!(v1.cpu_frequency_hz.is_none());
        assert!(v1.disk_io.is_none());
        assert!(v1.network.is_none());
        let v2 = NormalizedSnapshot::from_v2_payload(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default().build_payload(),
        );
        assert!(v2.drives.is_none());
        assert!(v2.cpu_frequency_hz.is_none());
        assert!(v2.disk_io.is_none());
        assert!(v2.network.is_none());
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

    #[test]
    fn new_v2_telemetry_is_preserved() {
        let payload = gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
            .cpu_frequency_hz(Some(2_400_000_000))
            .disk_io(Some(gregg_protocol::v2::DiskIoPayload {
                aggregate_read_bytes_per_sec: 10,
                aggregate_write_bytes_per_sec: 20,
                devices: vec![gregg_protocol::v2::DiskIoMetrics {
                    id: "nvme0".into(),
                    name: "nvme0".into(),
                    read_bytes_per_sec: 10,
                    write_bytes_per_sec: 20,
                    drive_name: Some("/".into()),
                }],
            }))
            .network(Some(gregg_protocol::v2::NetworkPayload {
                aggregate_rx_bytes_per_sec: 100_000_000,
                aggregate_tx_bytes_per_sec: 100_000_000,
                aggregate_rx_capacity_bps: Some(100_000_000 * 8),
                aggregate_tx_capacity_bps: Some(100_000_000 * 8),
                interfaces: vec![gregg_protocol::v2::NetworkInterfaceMetrics {
                    id: "eth0".into(),
                    name: "eth0".into(),
                    rx_bytes_per_sec: 100_000_000,
                    tx_bytes_per_sec: 100_000_000,
                    rx_capacity_bps: Some(100_000_000 * 8),
                    tx_capacity_bps: Some(100_000_000 * 8),
                    is_loopback: false,
                    aggregate_member: true,
                }],
            }))
            .build_payload();
        let normalized = NormalizedSnapshot::from_v2_payload(&payload);
        assert_eq!(normalized.cpu_frequency_hz, Some(2_400_000_000));
        assert_eq!(normalized.disk_io.as_ref().unwrap().devices.len(), 1);
        assert_eq!(normalized.network.as_ref().unwrap().interfaces.len(), 1);
        assert_eq!(
            normalized.network.unwrap().aggregate_utilization_pct(),
            Some(100.0)
        );
    }

    #[test]
    fn network_utilization_uses_max_direction_not_sum() {
        assert_eq!(
            network_utilization_pct(
                100_000_000,
                100_000_000,
                Some(100_000_000 * 8),
                Some(100_000_000 * 8),
            ),
            Some(100.0)
        );
    }

    #[test]
    fn network_utilization_preserves_throughput_when_capacity_is_missing() {
        assert_eq!(network_utilization_pct(100, 200, None, None), None);
        assert_eq!(
            network_utilization_pct(100, 200, Some(800), None),
            Some(100.0)
        );
        assert_eq!(bytes_per_second_to_bits_per_second(u64::MAX), None);
    }
}
