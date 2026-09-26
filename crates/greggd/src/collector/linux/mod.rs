//! Linux collector compatibility facade (Plan 135).
//!
//! Production telemetry comes from [`gregg_host::linux`]. This module
//! preserves the `greggd::collector::linux::*` paths used by the sampler,
//! `run`, and tests, and adapts the protocol-neutral host sample into the
//! Gregg-owned [`CollectedMetrics`](crate::collector::CollectedMetrics).

use gregg_host::linux::LinuxCollector as HostLinuxCollector;
use gregg_host::model::CollectionLimits;

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::{CollectedMetrics, SystemCollector};
use gregg_host::HostCollector;
use gregg_protocol::v2::{
    DiskIoMetrics, DiskIoPayload, DriveMetrics, MetricCapabilitiesV2, NetworkInterfaceMetrics,
    NetworkPayload,
};
use gregg_protocol::{LoadAverage, MemoryMetrics, MetricCapabilities, SwapMetrics, SystemIdentity};

// --- Re-exports: protocol-neutral items with identical semantics ------------

pub use gregg_host::linux::{
    collect_identity as collect_host_identity, os_release_sample,
    synthetic_identity as synthetic_host_identity,
};
pub use gregg_host::linux::{
    compute_percentages, parse_proc_stat, CpuCounters, CpuSampleView, FileSource, MemorySource,
    ParsedMeminfo, ParsedProcStat, ProcSource, RawDiskIo, RawNetworkInterface,
};
pub use gregg_host::linux::{
    parse_meminfo as compute_memory, parse_meminfo_raw as parse_meminfo, parse_swap as compute_swap,
};
pub use gregg_host::linux::{MeminfoSample, SwapInfoSample};

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;

// --- Identity / metric conversion --------------------------------------------

fn convert_identity(identity: gregg_host::model::HostIdentity) -> SystemIdentity {
    SystemIdentity {
        name: identity.name,
        hostname: identity.hostname,
        os_name: identity.os_name,
        os_version: identity.os_version,
        kernel_name: identity.kernel_name,
        kernel_release: identity.kernel_release,
        architecture: identity.architecture,
    }
}

fn convert_drive(drive: gregg_host::model::DriveMetrics) -> DriveMetrics {
    DriveMetrics {
        name: drive.name,
        used_bytes: drive.used_bytes,
        total_bytes: drive.total_bytes,
        available_bytes: drive.available_bytes,
    }
}

fn convert_disk_io(payload: gregg_host::model::DiskIoPayload) -> DiskIoPayload {
    DiskIoPayload {
        aggregate_read_bytes_per_sec: payload.aggregate_read_bytes_per_sec,
        aggregate_write_bytes_per_sec: payload.aggregate_write_bytes_per_sec,
        devices: payload
            .devices
            .into_iter()
            .map(|device| DiskIoMetrics {
                id: device.id,
                name: device.name,
                read_bytes_per_sec: device.read_bytes_per_sec,
                write_bytes_per_sec: device.write_bytes_per_sec,
                drive_name: device.drive_name,
            })
            .collect(),
    }
}

fn convert_network(payload: gregg_host::model::NetworkPayload) -> NetworkPayload {
    NetworkPayload {
        aggregate_rx_bytes_per_sec: payload.aggregate_rx_bytes_per_sec,
        aggregate_tx_bytes_per_sec: payload.aggregate_tx_bytes_per_sec,
        aggregate_rx_capacity_bps: payload.aggregate_rx_capacity_bps,
        aggregate_tx_capacity_bps: payload.aggregate_tx_capacity_bps,
        interfaces: payload
            .interfaces
            .into_iter()
            .map(|interface| NetworkInterfaceMetrics {
                id: interface.id,
                name: interface.name,
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

fn convert_sample(sample: gregg_host::model::HostSample) -> Result<CollectedMetrics, CollectError> {
    let load = sample.load.ok_or_else(|| {
        CollectError::new(
            CollectErrorKind::Numeric,
            "linux host sample missing load averages",
        )
    })?;
    let swap = sample.swap.ok_or_else(|| {
        CollectError::new(CollectErrorKind::Numeric, "linux host sample missing swap")
    })?;
    Ok(CollectedMetrics {
        logical_cores: sample.logical_cores,
        cpu_usage_pct: sample.cpu_usage_pct,
        cpu_iowait_pct: sample.cpu_iowait_pct,
        load: LoadAverage {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        },
        memory: MemoryMetrics {
            used_bytes: sample.memory.used_bytes,
            total_bytes: sample.memory.total_bytes,
            usage_pct: sample.memory.usage_pct,
        },
        swap: SwapMetrics {
            used_bytes: swap.used_bytes,
            total_bytes: swap.total_bytes,
            usage_pct: swap.usage_pct,
        },
        commit: None,
        drives: sample
            .drives
            .map(|drives| drives.into_iter().map(convert_drive).collect()),
        cpu_frequency_hz: sample.cpu_frequency_hz,
        disk_io: sample.disk_io.map(convert_disk_io),
        network: sample.network.map(convert_network),
    })
}

// --- Compatibility collector --------------------------------------------------

/// Linux collector facade: production sampling delegates to `gregg-host`.
pub struct LinuxCollector {
    inner: HostLinuxCollector,
}

/// Collection limits matching the current `gregg-protocol` constants exactly.
fn gregg_limits() -> CollectionLimits {
    CollectionLimits {
        max_drive_entries: gregg_protocol::v2::MAX_DRIVE_ENTRIES,
        max_drive_name_bytes: gregg_protocol::v2::MAX_DRIVE_NAME_BYTES,
        max_disk_io_entries: gregg_protocol::v2::MAX_DISK_IO_ENTRIES,
        max_disk_id_bytes: gregg_protocol::v2::MAX_LIVE_METRIC_ID_BYTES,
        max_disk_name_bytes: gregg_protocol::v2::MAX_LIVE_METRIC_NAME_BYTES,
        max_network_interface_entries: gregg_protocol::v2::MAX_NETWORK_INTERFACE_ENTRIES,
        max_network_id_bytes: gregg_protocol::v2::MAX_LIVE_METRIC_ID_BYTES,
        max_network_name_bytes: gregg_protocol::v2::MAX_LIVE_METRIC_NAME_BYTES,
    }
}

impl LinuxCollector {
    /// Create a collector that reads from the production procfs paths.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        let inner = HostLinuxCollector::with_source_and_limits(
            ProcSource::production(),
            display_name,
            gregg_limits(),
        )?;
        Ok(Self { inner })
    }

    /// Create a collector with an injected source.
    pub fn with_source(
        source: ProcSource,
        display_name: Option<&str>,
    ) -> Result<Self, CollectError> {
        let inner =
            HostLinuxCollector::with_source_and_limits(source, display_name, gregg_limits())?;
        Ok(Self { inner })
    }

    /// Borrow the underlying source mutably.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut ProcSource {
        self.inner.source_mut()
    }
}

impl SystemCollector for LinuxCollector {
    fn identity(&self) -> Result<SystemIdentity, CollectError> {
        self.inner.identity().map(convert_identity)
    }

    fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
        self.inner.sample().and_then(convert_sample)
    }

    fn capabilities(&self) -> MetricCapabilities {
        MetricCapabilities {
            cpu_iowait: self.inner.capabilities().cpu_iowait,
        }
    }

    fn capabilities_v2(&self) -> MetricCapabilitiesV2 {
        let caps = self.inner.capabilities();
        MetricCapabilitiesV2 {
            cpu_iowait: caps.cpu_iowait,
            load_average: caps.load_average,
            swap: caps.swap,
            memory_commit: caps.memory_commit,
        }
    }
}

/// Collect identity through the host backend and adapt to [`SystemIdentity`].
pub fn collect_identity(
    source: &ProcSource,
    display_name: Option<&str>,
) -> Result<SystemIdentity, CollectError> {
    collect_host_identity(source, display_name).map(convert_identity)
}

/// Parse `/proc/loadavg` content (host parser, protocol-typed result).
pub fn parse_loadavg(raw: &str) -> Result<LoadAverage, CollectError> {
    let host = gregg_host::linux::parse_loadavg(raw)?;
    Ok(LoadAverage {
        one: host.one,
        five: host.five,
        fifteen: host.fifteen,
    })
}

/// Synthetic identity helper preserved for tests.
pub fn synthetic_identity(
    name: &str,
    hostname: &str,
    os_name: &str,
    os_version: &str,
    kernel_name: &str,
    kernel_release: &str,
    architecture: &str,
) -> SystemIdentity {
    let host = synthetic_host_identity(
        name,
        hostname,
        os_name,
        os_version,
        kernel_name,
        kernel_release,
        architecture,
    );
    convert_identity(host)
}
