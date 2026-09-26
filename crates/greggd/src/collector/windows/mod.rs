//! Windows collector compatibility facade (Plan 135).
//!
//! Production telemetry comes from [`gregg_host::windows`]. This module
//! preserves the `greggd::collector::windows::*` paths and adapts the
//! protocol-neutral host sample into [`CollectedMetrics`](crate::collector::CollectedMetrics).
//!
//! Windows does not expose Unix load average, Unix swap, or CPU I/O-wait
//! state. These are reported as unsupported with explicit capability flags.

use gregg_host::model::CollectionLimits;

use crate::collector::error::CollectError;
use crate::collector::{CollectedMetrics, SystemCollector};
use gregg_host::windows::source::WindowsSource;
use gregg_host::HostCollector;
use gregg_protocol::v2::{
    DiskIoMetrics, DiskIoPayload, DriveMetrics, MetricCapabilitiesV2, NetworkInterfaceMetrics,
    NetworkPayload,
};
use gregg_protocol::{LoadAverage, MetricCapabilities, SwapMetrics, SystemIdentity};

pub use gregg_host::windows::source::{
    MockWindowsSource, NativeWindowsSource, RawCommit, RawCpuTimes, RawDiskIo, RawIdentity,
    RawLogicalDrive, RawNetworkInterface, RawPhysicalMemory, RawProcessorTopology,
    WindowsSource as HostWindowsSource,
};
pub use gregg_host::windows::WindowsCollector as HostWindowsCollector;
pub use gregg_host::windows::{commit, cpu, identity, memory, source};

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
    let commit = sample.commit.ok_or_else(|| {
        CollectError::new(
            crate::collector::error::CollectErrorKind::Numeric,
            "windows host sample missing commit",
        )
    })?;
    Ok(CollectedMetrics {
        logical_cores: sample.logical_cores,
        cpu_usage_pct: sample.cpu_usage_pct,
        cpu_iowait_pct: None,
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: gregg_protocol::MemoryMetrics {
            used_bytes: sample.memory.used_bytes,
            total_bytes: sample.memory.total_bytes,
            usage_pct: sample.memory.usage_pct,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
        commit: Some(gregg_protocol::v2::CommitMetrics {
            used_bytes: commit.used_bytes,
            limit_bytes: commit.limit_bytes,
            usage_pct: commit.usage_pct,
        }),
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

/// Windows collector facade: production sampling delegates to `gregg-host`.
#[derive(Debug)]
pub struct WindowsCollector<S: WindowsSource = NativeWindowsSource> {
    inner: gregg_host::windows::WindowsCollector<S>,
}

impl WindowsCollector<NativeWindowsSource> {
    /// Create a collector using the production FFI implementation.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        let inner = gregg_host::windows::WindowsCollector::with_source_and_limits(
            NativeWindowsSource,
            display_name,
            gregg_limits(),
        )?;
        Ok(Self { inner })
    }
}

impl<S: WindowsSource + Clone> WindowsCollector<S> {
    /// Create a collector with an injected source.
    pub fn with_source(source: S, display_name: Option<&str>) -> Result<Self, CollectError> {
        let inner = gregg_host::windows::WindowsCollector::with_source_and_limits(
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

impl<S: WindowsSource + Clone + 'static> SystemCollector for WindowsCollector<S> {
    fn identity(&self) -> Result<SystemIdentity, CollectError> {
        self.inner.identity().map(convert_identity)
    }

    fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
        self.inner.sample().and_then(convert_sample)
    }

    fn capabilities(&self) -> MetricCapabilities {
        MetricCapabilities { cpu_iowait: false }
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

    fn supports_v1_snapshot(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::SystemCollector;

    fn default_identity() -> RawIdentity {
        RawIdentity {
            hostname: "test-host".to_string(),
            os_version: "10.0.22631".to_string(),
            architecture: "x86_64".to_string(),
            logical_cores: 4,
            physical_memory_bytes: 8_000_000_000,
            processor_group_count: 1,
        }
    }

    fn mock_source() -> MockWindowsSource {
        let mut m = MockWindowsSource::success();
        m.identity = default_identity();
        m.topology = RawProcessorTopology {
            active_logical_processors: 4,
            group_count: 1,
        };
        m.auto_increment_cpu = true;
        m
    }

    fn sample_until_drives(
        collector: &mut WindowsCollector<MockWindowsSource>,
    ) -> crate::collector::CollectedMetrics {
        for _ in 0..200 {
            let metrics = collector.sample().expect("core sample succeeds");
            if metrics.drives.is_some() {
                return metrics;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("drive refresh did not complete");
    }

    #[test]
    fn single_group_within_limit_succeeds() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 64,
            group_count: 1,
        };
        assert!(WindowsCollector::with_source(mock, None).is_ok());
    }

    #[test]
    fn multi_group_rejected() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 8,
            group_count: 2,
        };
        let err = WindowsCollector::with_source(mock, None).expect_err("2 groups fail");
        assert!(err.message.contains("multiple processor groups"));
    }

    #[test]
    fn first_sample_warms_then_ready_with_commit() {
        let mut collector = WindowsCollector::with_source(mock_source(), None).expect("constructs");
        let err = collector.sample().expect_err("first warms");
        assert_eq!(err.kind, crate::collector::error::CollectErrorKind::Warming);
        let metrics = collector.sample().expect("second succeeds");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.cpu_iowait_pct.is_none());
        assert!(metrics.commit.is_some());
        assert!(!collector.supports_v1_snapshot());
        let caps_v2 = collector.capabilities_v2();
        assert!(!caps_v2.cpu_iowait && !caps_v2.load_average && !caps_v2.swap);
        assert!(caps_v2.memory_commit);
    }

    #[test]
    fn drive_failure_preserves_core_sample() {
        let mut mock = mock_source();
        mock.drives_error = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("constructs");
        let _ = collector.sample().expect_err("warming");
        let metrics = collector.sample().expect("core succeeds");
        assert!(metrics.cpu_usage_pct.is_some());
        assert_eq!(metrics.drives, None);
    }

    #[test]
    fn successful_empty_drive_enumeration_is_preserved() {
        let mut mock = mock_source();
        mock.drives.clear();
        let mut collector = WindowsCollector::with_source(mock, None).expect("constructs");
        let _ = collector.sample().expect_err("warming");
        let metrics = sample_until_drives(&mut collector);
        assert_eq!(metrics.drives, Some(Vec::new()));
    }
}
