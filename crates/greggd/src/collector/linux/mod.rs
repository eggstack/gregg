//! Linux collector entry point.
//!
//! Gathers identity, CPU, memory, swap, and load-average samples from
//! procfs and kernel interfaces. Platform-specific code lives in this
//! module; the shared collector contract is defined in
//! [`crate::collector`].

use std::time::Instant;

use gregg_protocol::v2::{
    DiskIoMetrics, DiskIoPayload, MetricCapabilitiesV2, NetworkInterfaceMetrics, NetworkPayload,
    MAX_DISK_IO_ENTRIES, MAX_NETWORK_INTERFACE_ENTRIES,
};
use gregg_protocol::{LoadAverage, MetricCapabilities, SystemIdentity};

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::rate::CounterBaselines;
use crate::collector::{CollectedMetrics, DriveRefreshCache, SystemCollector};

mod cpu;
mod drives;
mod fixtures;
mod identity;
mod memory;
mod source;

/// Sane upper bound on the reported core count. Far above any real or
/// planned Linux machine, so a bogus sysinfo read cannot surface a
/// sentinel-scale value to clients.
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
///
/// Constructed once per daemon process. Identity and static fields are read
/// eagerly during [`LinuxCollector::new`] so the first [`Self::sample`]
/// returns a warming error rather than blocking on identity I/O.
pub struct LinuxCollector {
    source: ProcSource,
    identity: SystemIdentity,
    capabilities: MetricCapabilities,
    previous_cpu: Option<cpu::CpuCounters>,
    drive_refresh: Option<DriveRefreshCache>,
    disk_baselines: CounterBaselines,
    network_baselines: CounterBaselines,
}

impl LinuxCollector {
    /// Create a collector that reads from the production procfs paths.
    ///
    /// `display_name` overrides the user-facing `name` field only; the actual
    /// `hostname` continues to come from the host.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        let source = ProcSource::production();
        Self::with_source(source, display_name)
    }

    /// Create a collector with an injected source. Intended for tests so
    /// fixtures can be replayed without touching the host `/proc` filesystem.
    pub fn with_source(
        source: ProcSource,
        display_name: Option<&str>,
    ) -> Result<Self, CollectError> {
        let identity = identity::collect_identity(&source, display_name)?;
        Ok(Self {
            source,
            identity,
            capabilities: MetricCapabilities { cpu_iowait: true },
            previous_cpu: None,
            drive_refresh: None,
            disk_baselines: CounterBaselines::default(),
            network_baselines: CounterBaselines::default(),
        })
    }

    /// Borrow the underlying [`ProcSource`] mutably. Tests use this to swap
    /// fixture content between samples; production code does not need it.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut ProcSource {
        &mut self.source
    }
}

impl LinuxCollector {
    fn refresh_drives(&mut self) -> Option<Vec<gregg_protocol::v2::DriveMetrics>> {
        if self.drive_refresh.is_none() {
            self.drive_refresh = Some(DriveRefreshCache::new(self.source.clone(), drives::collect));
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
            if devices.len() < MAX_DISK_IO_ENTRIES {
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
            if interfaces.len() < MAX_NETWORK_INTERFACE_ENTRIES {
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

impl SystemCollector for LinuxCollector {
    fn identity(&self) -> Result<SystemIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
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

        // Re-read the core count on every sample so CPU hotplug events are
        // reflected instead of freezing the count from construction time.
        let logical_cores = u32::try_from(
            self.source
                .logical_core_count()
                .unwrap_or(1)
                .clamp(1, MAX_LOGICAL_CORES),
        )
        .unwrap_or(1);

        Ok(CollectedMetrics {
            logical_cores,
            cpu_usage_pct: Some(cpu_sample.usage_pct),
            cpu_iowait_pct: Some(cpu_sample.iowait_pct),
            load,
            memory: memory_sample.into_metrics(),
            swap: swap_sample.into_metrics(),
            commit: None,
            drives: self.refresh_drives(),
            cpu_frequency_hz: self.source.cpu_frequency_hz(),
            disk_io: self.collect_disk_io(now),
            network: self.collect_network(now),
        })
    }

    fn capabilities(&self) -> MetricCapabilities {
        self.capabilities
    }

    fn capabilities_v2(&self) -> MetricCapabilitiesV2 {
        MetricCapabilitiesV2 {
            cpu_iowait: true,
            load_average: true,
            swap: true,
            memory_commit: false,
        }
    }
}

fn parse_loadavg(raw: &str) -> Result<LoadAverage, CollectError> {
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
