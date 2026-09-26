//! Windows collector entry point.
//!
//! Gathers identity, CPU, memory, and commit samples from native Windows
//! APIs. Moved without semantic change from `greggd::collector::windows`
//! (Plan 134); types are protocol-neutral. Windows has no Unix load
//! average, Unix swap, or CPU I/O-wait state.

use std::time::Instant;

use crate::error::{CollectError, CollectErrorKind};
use crate::model::{
    CollectionLimits, DiskIoMetrics, DiskIoPayload, HostCapabilities, HostIdentity, HostSample,
    NetworkInterfaceMetrics, NetworkPayload,
};
use crate::rate::CounterBaselines;
use crate::slow_probe::DriveRefreshCache;
use crate::windows::source::{RawCpuTimes, WindowsSource};
use crate::{clamped_usage_pct, HostCollector};

pub mod commit;
pub mod cpu;
pub mod identity;
pub mod memory;
pub mod source;

fn collect_drives<S: WindowsSource>(
    source: &S,
    limits: &CollectionLimits,
) -> Result<Vec<crate::model::DriveMetrics>, CollectError> {
    let raw = source.logical_drives()?;
    let candidates = raw
        .into_iter()
        .filter(|drive| {
            (drive.drive_type == source::DRIVE_FIXED || drive.drive_type == source::DRIVE_REMOVABLE)
                && !drive.root.is_empty()
        })
        .map(|drive| crate::drives::DriveCandidate {
            identity: drive.root.clone(),
            name: drive.root,
            total_bytes: drive.total_bytes,
            total_free_bytes: drive.total_free_bytes,
            available_bytes: drive.available_bytes,
        })
        .collect();
    Ok(crate::drives::normalize_with_limits(candidates, limits))
}

/// A Windows native collector.
#[derive(Debug)]
pub struct WindowsCollector<S: WindowsSource = source::NativeWindowsSource> {
    source: S,
    identity: HostIdentity,
    capabilities: HostCapabilities,
    limits: CollectionLimits,
    previous_cpu: Option<RawCpuTimes>,
    logical_cores: u32,
    drive_refresh: Option<DriveRefreshCache>,
    disk_baselines: CounterBaselines,
    network_baselines: CounterBaselines,
}

impl WindowsCollector<source::NativeWindowsSource> {
    /// Create a collector using the production FFI implementation.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        Self::with_source(source::NativeWindowsSource, display_name)
    }
}

impl<S: WindowsSource + Clone> WindowsCollector<S> {
    /// Create a collector with an injected source.
    pub fn with_source(source: S, display_name: Option<&str>) -> Result<Self, CollectError> {
        Self::with_source_and_limits(source, display_name, CollectionLimits::gregg_defaults())
    }

    /// Create a collector with an injected source and explicit limits.
    pub fn with_source_and_limits(
        source: S,
        display_name: Option<&str>,
        limits: CollectionLimits,
    ) -> Result<Self, CollectError> {
        let raw_identity = source.identity()?;
        let topology = source.processor_topology()?;

        if topology.group_count > 1 {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                "multiple processor groups are not supported; \
                 GetSystemTimes only covers one group",
            ));
        }
        if topology.active_logical_processors > cpu::MAX_SINGLE_GROUP_LOGICAL_PROCESSORS {
            return Err(CollectError::new(
                CollectErrorKind::SourceUnavailable,
                format!(
                    "logical processor count {} exceeds supported limit of {}",
                    topology.active_logical_processors,
                    cpu::MAX_SINGLE_GROUP_LOGICAL_PROCESSORS
                ),
            ));
        }

        let logical_cores = raw_identity.logical_cores.max(1);
        let system_identity = identity::collect_identity(&source, display_name)?;

        Ok(Self {
            source,
            identity: system_identity,
            capabilities: HostCapabilities {
                cpu_iowait: false,
                load_average: false,
                swap: false,
                memory_commit: true,
                drives: true,
                cpu_frequency: true,
                disk_io: true,
                network: true,
            },
            limits,
            previous_cpu: None,
            logical_cores,
            drive_refresh: None,
            disk_baselines: CounterBaselines::default(),
            network_baselines: CounterBaselines::default(),
        })
    }

    /// Override collection limits after construction.
    #[must_use]
    pub fn with_limits(mut self, limits: CollectionLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Borrow the underlying source mutably.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }
}

impl<S: WindowsSource + Clone + 'static> WindowsCollector<S> {
    fn refresh_drives(&mut self) -> Option<Vec<crate::model::DriveMetrics>> {
        if self.drive_refresh.is_none() {
            let limits = self.limits;
            self.drive_refresh = Some(DriveRefreshCache::new(self.source.clone(), move |source| {
                collect_drives(source, &limits)
            }));
        }
        self.drive_refresh
            .as_mut()
            .and_then(DriveRefreshCache::poll)
    }

    fn collect_disk_io(&mut self, now: Instant) -> Option<DiskIoPayload> {
        let Ok(records) = self.source.disk_io() else {
            self.disk_baselines.clear();
            return None;
        };
        self.disk_baselines
            .retain_ids(records.iter().map(|r| r.id.as_str()));
        let mut devices = Vec::new();
        let mut read_total = 0u64;
        let mut write_total = 0u64;
        for record in records {
            if record.id.is_empty()
                || record.name.is_empty()
                || record.id.contains('\0')
                || record.name.contains('\0')
            {
                continue;
            }
            let Some(rate) =
                self.disk_baselines
                    .observe(&record.id, now, record.read_bytes, record.write_bytes)
            else {
                continue;
            };
            read_total = read_total.saturating_add(rate.first_per_sec);
            write_total = write_total.saturating_add(rate.second_per_sec);
            if devices.len() < self.limits.max_disk_io_entries {
                devices.push(DiskIoMetrics {
                    id: record.id,
                    name: record.name,
                    read_bytes_per_sec: rate.first_per_sec,
                    write_bytes_per_sec: rate.second_per_sec,
                    drive_name: None,
                });
            }
        }
        (!devices.is_empty()).then_some(DiskIoPayload {
            aggregate_read_bytes_per_sec: read_total,
            aggregate_write_bytes_per_sec: write_total,
            devices,
        })
    }

    fn collect_network(&mut self, now: Instant) -> Option<NetworkPayload> {
        let Ok(records) = self.source.network_interfaces() else {
            self.network_baselines.clear();
            return None;
        };
        self.network_baselines
            .retain_ids(records.iter().map(|r| r.id.as_str()));
        let mut interfaces = Vec::new();
        let mut rx_total = 0u64;
        let mut tx_total = 0u64;
        let mut rx_capacity: Option<u64> = None;
        let mut tx_capacity: Option<u64> = None;
        for record in records {
            if record.id.is_empty()
                || record.name.is_empty()
                || record.id.contains('\0')
                || record.name.contains('\0')
            {
                continue;
            }
            let Some(rate) =
                self.network_baselines
                    .observe(&record.id, now, record.rx_bytes, record.tx_bytes)
            else {
                continue;
            };
            let aggregate_member = record.aggregate_member && !record.is_loopback;
            if aggregate_member {
                rx_total = rx_total.saturating_add(rate.first_per_sec);
                tx_total = tx_total.saturating_add(rate.second_per_sec);
                if record.operational && !record.is_loopback {
                    if let Some(capacity) = record.rx_capacity_bps {
                        rx_capacity = Some(rx_capacity.unwrap_or(0).saturating_add(capacity));
                    }
                    if let Some(capacity) = record.tx_capacity_bps {
                        tx_capacity = Some(tx_capacity.unwrap_or(0).saturating_add(capacity));
                    }
                }
            }
            if interfaces.len() < self.limits.max_network_interface_entries {
                interfaces.push(NetworkInterfaceMetrics {
                    id: record.id,
                    name: record.name,
                    rx_bytes_per_sec: rate.first_per_sec,
                    tx_bytes_per_sec: rate.second_per_sec,
                    rx_capacity_bps: record.rx_capacity_bps,
                    tx_capacity_bps: record.tx_capacity_bps,
                    is_loopback: record.is_loopback,
                    aggregate_member,
                });
            }
        }
        (!interfaces.is_empty()).then_some(NetworkPayload {
            aggregate_rx_bytes_per_sec: rx_total,
            aggregate_tx_bytes_per_sec: tx_total,
            aggregate_rx_capacity_bps: rx_capacity,
            aggregate_tx_capacity_bps: tx_capacity,
            interfaces,
        })
    }
}

impl<S: WindowsSource + Clone + 'static> HostCollector for WindowsCollector<S> {
    fn identity(&self) -> Result<HostIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<HostSample, CollectError> {
        let raw_cpu = self.source.cpu_times()?;
        let raw_memory = self.source.physical_memory()?;
        let raw_commit = self.source.commit()?;
        let now = Instant::now();

        let cpu_sample = if let Some(prev) = self.previous_cpu.as_ref() {
            match cpu::compute_cpu_percentages(prev, &raw_cpu) {
                Ok(sample) => Some(sample),
                Err(CollectError {
                    kind: CollectErrorKind::CounterReset,
                    ..
                }) => {
                    self.previous_cpu = Some(raw_cpu);
                    return Err(CollectError::counter_reset(
                        "CPU counters reset; baseline re-established",
                    ));
                }
                Err(other) => return Err(other),
            }
        } else {
            self.previous_cpu = Some(raw_cpu);
            return Err(CollectError::warming(
                "first CPU sample establishes the counter baseline",
            ));
        };

        self.previous_cpu = Some(raw_cpu);

        let mem_sample = memory::compute_memory(&raw_memory)?;
        let commit_sample = commit::compute_commit(&raw_commit)?;

        let cpu = cpu_sample.ok_or_else(|| {
            CollectError::new(
                CollectErrorKind::Numeric,
                "cpu_sample should be Some after baseline established",
            )
        })?;

        Ok(HostSample {
            logical_cores: self.logical_cores,
            cpu_usage_pct: Some(cpu.usage_pct),
            cpu_iowait_pct: None,
            load: None,
            memory: crate::model::MemoryMetrics {
                used_bytes: mem_sample.used_bytes,
                total_bytes: mem_sample.total_bytes,
                usage_pct: clamped_usage_pct(mem_sample.used_bytes, mem_sample.total_bytes),
            },
            swap: None,
            commit: Some(crate::model::CommitMetrics {
                used_bytes: commit_sample.used_bytes,
                limit_bytes: commit_sample.limit_bytes,
                usage_pct: clamped_usage_pct(commit_sample.used_bytes, commit_sample.limit_bytes),
            }),
            drives: self.refresh_drives(),
            cpu_frequency_hz: self.source.cpu_frequency_hz().ok().flatten(),
            disk_io: self.collect_disk_io(now),
            network: self.collect_network(now),
        })
    }

    fn capabilities(&self) -> HostCapabilities {
        self.capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::source::{MockWindowsSource, RawIdentity, RawProcessorTopology};

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

    #[test]
    fn first_sample_warms_then_ready() {
        let mut collector = WindowsCollector::with_source(mock_source(), None).expect("constructs");
        let err = collector.sample().expect_err("first warms");
        assert_eq!(err.kind, CollectErrorKind::Warming);
        let metrics = collector.sample().expect("second succeeds");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.cpu_iowait_pct.is_none());
        assert!(metrics.load.is_none());
        assert!(metrics.swap.is_none());
        assert!(metrics.commit.is_some());
        let caps = collector.capabilities();
        assert!(!caps.cpu_iowait && !caps.load_average && !caps.swap);
        assert!(caps.memory_commit);
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
    fn commit_bounded_by_limit() {
        let mock = mock_source();
        let mut collector = WindowsCollector::with_source(mock, None).expect("constructs");
        let _ = collector.sample().expect_err("warming");
        let metrics = collector.sample().expect("sample");
        let commit = metrics.commit.expect("commit");
        assert!(commit.used_bytes <= commit.limit_bytes);
    }
}
