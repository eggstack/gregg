//! Linux collector entry point.
//!
//! Gathers identity, CPU, memory, swap, and load-average samples from
//! procfs and kernel interfaces. Moved without semantic change from
//! `greggd::collector::linux` (Plan 134); types are protocol-neutral.

use std::time::Instant;

use crate::error::{CollectError, CollectErrorKind};
use crate::model::{
    CollectionLimits, DiskIoMetrics, DiskIoPayload, HostCapabilities, HostIdentity, HostSample,
    LoadAverage, NetworkInterfaceMetrics, NetworkPayload,
};
use crate::rate::CounterBaselines;
use crate::slow_probe::DriveRefreshCache;
use crate::{clamped_usage_pct, HostCollector};

mod cpu;
mod drives;
mod fixtures;
mod identity;
mod memory;
mod source;

const MAX_LOGICAL_CORES: usize = 8192;

#[cfg(test)]
mod tests;

pub use cpu::{compute_percentages, parse_proc_stat, CpuCounters, CpuSample as CpuSampleView};
pub use identity::{collect_identity, os_release_sample, synthetic_identity};
pub use memory::{
    compute_memory as parse_meminfo, compute_swap as parse_swap,
    parse_meminfo as parse_meminfo_raw, MemorySample as MeminfoSample,
    SwapSample as SwapInfoSample,
};
pub use source::{
    FileSource, MemorySource, ParsedMeminfo, ParsedProcStat, ProcSource, RawDiskIo,
    RawNetworkInterface,
};

/// A Linux native collector.
pub struct LinuxCollector {
    source: ProcSource,
    identity: HostIdentity,
    capabilities: HostCapabilities,
    limits: CollectionLimits,
    previous_cpu: Option<cpu::CpuCounters>,
    drive_refresh: Option<DriveRefreshCache>,
    disk_baselines: CounterBaselines,
    network_baselines: CounterBaselines,
}

impl LinuxCollector {
    /// Create a collector that reads from the production procfs paths.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        let source = ProcSource::production();
        Self::with_source(source, display_name)
    }

    /// Create a collector with an injected source.
    pub fn with_source(
        source: ProcSource,
        display_name: Option<&str>,
    ) -> Result<Self, CollectError> {
        Self::with_source_and_limits(source, display_name, CollectionLimits::gregg_defaults())
    }

    /// Create a collector with an injected source and explicit limits.
    pub fn with_source_and_limits(
        source: ProcSource,
        display_name: Option<&str>,
        limits: CollectionLimits,
    ) -> Result<Self, CollectError> {
        let identity = identity::collect_identity(&source, display_name)?;
        Ok(Self {
            source,
            identity,
            capabilities: HostCapabilities {
                cpu_iowait: true,
                load_average: true,
                swap: true,
                memory_commit: false,
                drives: true,
                cpu_frequency: true,
                disk_io: true,
                network: true,
            },
            limits,
            previous_cpu: None,
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

    /// Borrow the underlying [`ProcSource`] mutably.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut ProcSource {
        &mut self.source
    }
}

impl LinuxCollector {
    fn refresh_drives(&mut self) -> Option<Vec<crate::model::DriveMetrics>> {
        if self.drive_refresh.is_none() {
            let limits = self.limits;
            self.drive_refresh = Some(DriveRefreshCache::new(self.source.clone(), move |source| {
                drives::collect_with_limits(source, &limits)
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
            .retain_ids(records.iter().map(|record| record.id.as_str()));
        let mut devices = Vec::new();
        let mut aggregate_read = 0u64;
        let mut aggregate_write = 0u64;
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
            aggregate_read = aggregate_read.saturating_add(rate.first_per_sec);
            aggregate_write = aggregate_write.saturating_add(rate.second_per_sec);
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
            aggregate_read_bytes_per_sec: aggregate_read,
            aggregate_write_bytes_per_sec: aggregate_write,
            devices,
        })
    }

    fn collect_network(&mut self, now: Instant) -> Option<NetworkPayload> {
        let Ok(records) = self.source.network_interfaces() else {
            self.network_baselines.clear();
            return None;
        };
        self.network_baselines
            .retain_ids(records.iter().map(|record| record.id.as_str()));
        let mut interfaces = Vec::new();
        let mut aggregate_rx = 0u64;
        let mut aggregate_tx = 0u64;
        let mut rx_capacity_total: Option<u64> = None;
        let mut tx_capacity_total: Option<u64> = None;
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
                aggregate_rx = aggregate_rx.saturating_add(rate.first_per_sec);
                aggregate_tx = aggregate_tx.saturating_add(rate.second_per_sec);
                if record.operational && !record.is_loopback {
                    if let Some(capacity) = record.rx_capacity_bps {
                        rx_capacity_total =
                            Some(rx_capacity_total.unwrap_or(0).saturating_add(capacity));
                    }
                    if let Some(capacity) = record.tx_capacity_bps {
                        tx_capacity_total =
                            Some(tx_capacity_total.unwrap_or(0).saturating_add(capacity));
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
            aggregate_rx_bytes_per_sec: aggregate_rx,
            aggregate_tx_bytes_per_sec: aggregate_tx,
            aggregate_rx_capacity_bps: rx_capacity_total,
            aggregate_tx_capacity_bps: tx_capacity_total,
            interfaces,
        })
    }
}

impl HostCollector for LinuxCollector {
    fn identity(&self) -> Result<HostIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<HostSample, CollectError> {
        let stat = self.source.read_proc_stat()?;
        let loadavg = self.source.read_proc_loadavg()?;
        let meminfo = self.source.read_proc_meminfo()?;

        let cpu_sample = if let (Some(prev), Some(curr)) =
            (self.previous_cpu.as_ref(), stat.aggregate.as_ref())
        {
            match cpu::compute_percentages(prev, curr) {
                Ok(sample) => sample,
                Err(CollectError {
                    kind: CollectErrorKind::CounterReset,
                    ..
                }) => {
                    self.previous_cpu = stat.aggregate;
                    return Err(CollectError::counter_reset(
                        "aggregate CPU counters reset; baseline re-established",
                    ));
                }
                Err(other) => return Err(other),
            }
        } else {
            self.previous_cpu = stat.aggregate;
            return Err(CollectError::warming(
                "first CPU sample establishes the counter baseline",
            ));
        };

        let memory_sample = memory::compute_memory(&meminfo)?;
        let swap_sample = memory::compute_swap(&meminfo)?;
        let load = parse_loadavg(&loadavg)?;
        let now = Instant::now();

        self.previous_cpu = stat.aggregate;

        let logical_cores = u32::try_from(
            self.source
                .logical_core_count()
                .unwrap_or(1)
                .clamp(1, MAX_LOGICAL_CORES),
        )
        .unwrap_or(1);

        Ok(HostSample {
            logical_cores,
            cpu_usage_pct: Some(cpu_sample.usage_pct),
            cpu_iowait_pct: Some(cpu_sample.iowait_pct),
            load: Some(load),
            memory: crate::model::MemoryMetrics {
                used_bytes: memory_sample.used_bytes,
                total_bytes: memory_sample.total_bytes,
                usage_pct: clamped_usage_pct(memory_sample.used_bytes, memory_sample.total_bytes),
            },
            swap: Some(crate::model::SwapMetrics {
                used_bytes: swap_sample.used_bytes,
                total_bytes: swap_sample.total_bytes,
                usage_pct: clamped_usage_pct(swap_sample.used_bytes, swap_sample.total_bytes),
            }),
            commit: None,
            drives: self.refresh_drives(),
            cpu_frequency_hz: self.source.cpu_frequency_hz(),
            disk_io: self.collect_disk_io(now),
            network: self.collect_network(now),
        })
    }

    fn capabilities(&self) -> HostCapabilities {
        self.capabilities
    }
}

/// Parse `/proc/loadavg` content.
pub fn parse_loadavg(raw: &str) -> Result<LoadAverage, CollectError> {
    let trimmed = raw.trim();
    let mut parts = trimmed.split_whitespace();
    let one = parts
        .next()
        .ok_or_else(|| CollectError::new(CollectErrorKind::Parse, "missing load.1 field"))?;
    let five = parts
        .next()
        .ok_or_else(|| CollectError::new(CollectErrorKind::Parse, "missing load.5 field"))?;
    let fifteen = parts
        .next()
        .ok_or_else(|| CollectError::new(CollectErrorKind::Parse, "missing load.15 field"))?;

    let parse_one = |s: &str, label: &str| {
        let parsed: f64 = s.parse().map_err(|e: std::num::ParseFloatError| {
            CollectError::new(
                CollectErrorKind::Parse,
                format!("loadavg {label} not a float"),
            )
            .with_source(e)
        })?;
        if !parsed.is_finite() || parsed < 0.0 {
            return Err(CollectError::new(
                CollectErrorKind::Parse,
                format!("loadavg {label} is not finite/non-negative"),
            ));
        }
        #[allow(clippy::cast_possible_truncation)]
        let as_f32 = parsed as f32;
        Ok(as_f32)
    };

    Ok(LoadAverage {
        one: parse_one(one, "1")?,
        five: parse_one(five, "5")?,
        fifteen: parse_one(fifteen, "15")?,
    })
}
