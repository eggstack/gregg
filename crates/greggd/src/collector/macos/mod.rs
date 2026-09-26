//! macOS collector compatibility facade (Plan 135).
//!
//! Production telemetry comes from [`gregg_host::macos`]. This module
//! preserves the `greggd::collector::macos::*` paths and adapts the
//! protocol-neutral host sample into [`CollectedMetrics`](crate::collector::CollectedMetrics).

use gregg_host::model::CollectionLimits;

use crate::collector::error::CollectError;
use crate::collector::{CollectedMetrics, SystemCollector};
use gregg_host::HostCollector;
use gregg_protocol::v2::{
    DiskIoMetrics, DiskIoPayload, DriveMetrics, MetricCapabilitiesV2, NetworkInterfaceMetrics,
    NetworkPayload,
};
use gregg_protocol::{LoadAverage, MemoryMetrics, MetricCapabilities, SwapMetrics, SystemIdentity};

// --- Re-exports ---------------------------------------------------------------

pub use gregg_host::macos::ffi::{MacNativeQueries, MockNativeQueries};
pub use gregg_host::macos::MacOsCollector as HostMacOsCollector;
pub use gregg_host::macos::{cpu, ffi, identity, memory, normalize, swap};

#[cfg(test)]
mod tests;

// --- Conversion -----------------------------------------------------------------

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
    use crate::collector::error::CollectErrorKind;
    let load = sample.load.ok_or_else(|| {
        CollectError::new(
            CollectErrorKind::Numeric,
            "macos host sample missing load averages",
        )
    })?;
    let swap = sample.swap.ok_or_else(|| {
        CollectError::new(CollectErrorKind::Numeric, "macos host sample missing swap")
    })?;
    Ok(CollectedMetrics {
        logical_cores: sample.logical_cores,
        cpu_usage_pct: sample.cpu_usage_pct,
        cpu_iowait_pct: None,
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

// --- Compatibility collector -----------------------------------------------------

/// macOS collector facade: production sampling delegates to `gregg-host`.
pub struct MacOsCollector<S: MacNativeQueries = gregg_host::macos::ffi::FfiNativeQueries> {
    inner: gregg_host::macos::MacOsCollector<S>,
}

impl MacOsCollector<gregg_host::macos::ffi::FfiNativeQueries> {
    /// Create a collector using the production FFI implementation.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        let inner = gregg_host::macos::MacOsCollector::with_source_and_limits(
            gregg_host::macos::ffi::FfiNativeQueries,
            display_name,
            gregg_limits(),
        )?;
        Ok(Self { inner })
    }
}

impl<S: MacNativeQueries + Clone> MacOsCollector<S> {
    /// Create a collector with an injected source.
    pub fn with_source(source: S, display_name: Option<&str>) -> Result<Self, CollectError> {
        let inner = gregg_host::macos::MacOsCollector::with_source_and_limits(
            source,
            display_name,
            gregg_limits(),
        )?;
        Ok(Self { inner })
    }

    /// Borrow the underlying source mutably.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut S {
        self.inner.source_mut()
    }
}

impl<S: MacNativeQueries + Clone + 'static> SystemCollector for MacOsCollector<S> {
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

/// Parse `[f64; 3]` load averages (host parser, protocol-typed result).
pub fn parse_loadavgs(raw: &[f64; 3]) -> Result<LoadAverage, CollectError> {
    use crate::collector::error::CollectErrorKind;
    let parse_one = |value: f64, label: &str| -> Result<f32, CollectError> {
        if !value.is_finite() || value < 0.0 {
            return Err(CollectError::new(
                CollectErrorKind::Parse,
                format!("loadavg {label} is not finite/non-negative"),
            ));
        }
        #[allow(clippy::cast_possible_truncation)]
        Ok(value as f32)
    };
    Ok(LoadAverage {
        one: parse_one(raw[0], "1")?,
        five: parse_one(raw[1], "5")?,
        fifteen: parse_one(raw[2], "15")?,
    })
}
