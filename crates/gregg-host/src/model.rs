//! Protocol-neutral host telemetry model.
//!
//! These types mirror the fields Gregg collects today without importing
//! `gregg-protocol`. Unsupported metrics are `None`, never fabricated zeros
//! (the Windows zeroed load/swap v1 convention lives in the `greggd`
//! adapter, not here).

/// Stable host identity collected once per process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIdentity {
    /// User-facing display name (configured name or hostname).
    pub name: String,
    /// Native hostname.
    pub hostname: String,
    /// Operating system name.
    pub os_name: String,
    /// Operating system version.
    pub os_version: String,
    /// Kernel name.
    pub kernel_name: String,
    /// Kernel release.
    pub kernel_release: String,
    /// Machine architecture.
    pub architecture: String,
}

/// One/five/fifteen-minute load averages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadAverage {
    /// One-minute average.
    pub one: f32,
    /// Five-minute average.
    pub five: f32,
    /// Fifteen-minute average.
    pub fifteen: f32,
}

/// Physical memory utilization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryMetrics {
    /// Used bytes.
    pub used_bytes: u64,
    /// Total bytes.
    pub total_bytes: u64,
    /// Usage percentage in `0.0..=100.0`.
    pub usage_pct: f32,
}

/// Swap utilization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapMetrics {
    /// Used bytes.
    pub used_bytes: u64,
    /// Total bytes.
    pub total_bytes: u64,
    /// Usage percentage in `0.0..=100.0`.
    pub usage_pct: f32,
}

/// Windows commit charge (Windows-specific; other platforms use `None`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommitMetrics {
    /// Committed bytes.
    pub used_bytes: u64,
    /// Commit limit bytes.
    pub limit_bytes: u64,
    /// Usage percentage in `0.0..=100.0`.
    pub usage_pct: f32,
}

/// Capacity for one operator-visible mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveMetrics {
    /// Display name supplied by the platform collector.
    pub name: String,
    /// Bytes currently used (`total - total_free`).
    pub used_bytes: u64,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Caller-available bytes, when known.
    pub available_bytes: Option<u64>,
}

/// Disk throughput for one stable device identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskIoMetrics {
    /// Stable identity used for baseline handling.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Read throughput in bytes per second.
    pub read_bytes_per_sec: u64,
    /// Write throughput in bytes per second.
    pub write_bytes_per_sec: u64,
    /// Optional association with an existing drive name.
    pub drive_name: Option<String>,
}

/// Aggregate and per-device disk throughput.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskIoPayload {
    /// De-duplicated aggregate read throughput in bytes per second.
    pub aggregate_read_bytes_per_sec: u64,
    /// De-duplicated aggregate write throughput in bytes per second.
    pub aggregate_write_bytes_per_sec: u64,
    /// Bounded device detail records.
    pub devices: Vec<DiskIoMetrics>,
}

/// Network throughput and capacity for one interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterfaceMetrics {
    /// Stable native interface identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Receive throughput in bytes per second.
    pub rx_bytes_per_sec: u64,
    /// Transmit throughput in bytes per second.
    pub tx_bytes_per_sec: u64,
    /// Receive capacity in bits per second, when known.
    pub rx_capacity_bps: Option<u64>,
    /// Transmit capacity in bits per second, when known.
    pub tx_capacity_bps: Option<u64>,
    /// Whether this interface is loopback.
    pub is_loopback: bool,
    /// Whether this interface joins the aggregate.
    pub aggregate_member: bool,
}

/// Aggregate and per-interface network throughput and capacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPayload {
    /// Aggregate receive throughput in bytes per second.
    pub aggregate_rx_bytes_per_sec: u64,
    /// Aggregate transmit throughput in bytes per second.
    pub aggregate_tx_bytes_per_sec: u64,
    /// Aggregate receive capacity in bits per second, when known.
    pub aggregate_rx_capacity_bps: Option<u64>,
    /// Aggregate transmit capacity in bits per second, when known.
    pub aggregate_tx_capacity_bps: Option<u64>,
    /// Bounded interface detail records.
    pub interfaces: Vec<NetworkInterfaceMetrics>,
}

/// What the host backend actually supports.
///
/// Capability flags describe support; transient source failure is distinct
/// from permanent unsupported status and is represented by `None` payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct HostCapabilities {
    /// Aggregate CPU I/O-wait percentage available.
    pub cpu_iowait: bool,
    /// Load averages available.
    pub load_average: bool,
    /// Swap accounting available.
    pub swap: bool,
    /// Windows commit charge available.
    pub memory_commit: bool,
    /// Drive enumeration available.
    pub drives: bool,
    /// CPU frequency available.
    pub cpu_frequency: bool,
    /// Disk-I/O counters available.
    pub disk_io: bool,
    /// Network counters available.
    pub network: bool,
}

/// One normalized host sample.
///
/// Optional families are `None` when unsupported or transiently unavailable.
/// The sampler/adapter decides readiness; this type never fabricates zeros.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSample {
    /// Logical CPU core count, always `> 0` on success.
    pub logical_cores: u32,
    /// Aggregate CPU busy percentage. `None` while warming or after reset.
    pub cpu_usage_pct: Option<f32>,
    /// Aggregate CPU I/O-wait percentage, when supported and warmed.
    pub cpu_iowait_pct: Option<f32>,
    /// Load averages, when supported.
    pub load: Option<LoadAverage>,
    /// Physical memory utilization.
    pub memory: MemoryMetrics,
    /// Swap utilization, when supported.
    pub swap: Option<SwapMetrics>,
    /// Windows commit charge, when supported.
    pub commit: Option<CommitMetrics>,
    /// Bounded drive capacity. `None` means unavailable; `Some(empty)` means
    /// a successful enumeration with no eligible filesystems.
    pub drives: Option<Vec<DriveMetrics>>,
    /// Current CPU frequency in Hz, when supported.
    pub cpu_frequency_hz: Option<u64>,
    /// Disk throughput rates, when a valid interval exists.
    pub disk_io: Option<DiskIoPayload>,
    /// Network throughput rates, when a valid interval exists.
    pub network: Option<NetworkPayload>,
}

/// Protocol-neutral collection bounds.
///
/// `greggd` constructs these from the current `gregg-protocol` constants so
/// wire behavior does not change. Defaults match those constants for
/// external consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectionLimits {
    /// Maximum drive records.
    pub max_drive_entries: usize,
    /// Maximum drive-name bytes.
    pub max_drive_name_bytes: usize,
    /// Maximum disk-I/O detail records.
    pub max_disk_io_entries: usize,
    /// Maximum disk stable-identity bytes.
    pub max_disk_id_bytes: usize,
    /// Maximum disk display-name bytes.
    pub max_disk_name_bytes: usize,
    /// Maximum network-interface records.
    pub max_network_interface_entries: usize,
    /// Maximum interface stable-identity bytes.
    pub max_network_id_bytes: usize,
    /// Maximum interface display-name bytes.
    pub max_network_name_bytes: usize,
}

impl Default for CollectionLimits {
    fn default() -> Self {
        Self {
            max_drive_entries: 32,
            max_drive_name_bytes: 512,
            max_disk_io_entries: 32,
            max_disk_id_bytes: 512,
            max_disk_name_bytes: 512,
            max_network_interface_entries: 32,
            max_network_id_bytes: 512,
            max_network_name_bytes: 512,
        }
    }
}

impl CollectionLimits {
    /// Bounds matching Gregg's current wire constants.
    #[must_use]
    pub const fn gregg_defaults() -> Self {
        Self {
            max_drive_entries: 32,
            max_drive_name_bytes: 512,
            max_disk_io_entries: 32,
            max_disk_id_bytes: 512,
            max_disk_name_bytes: 512,
            max_network_interface_entries: 32,
            max_network_id_bytes: 512,
            max_network_name_bytes: 512,
        }
    }
}
