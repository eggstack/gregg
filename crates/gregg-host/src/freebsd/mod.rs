//! FreeBSD collector entry point.
//!
//! First post-extraction native backend for `gregg-host` (Plan 136).
//! Gathers identity, CPU, load, memory, and local filesystems from FreeBSD
//! sysctl/libc interfaces, with disk I/O from base `libdevstat` and network
//! from `ifmib(4)`. No shell commands, no privilege escalation, no generic
//! Unix/BSD abstraction: this backend has its own explicit source seam.
//!
//! Memory semantics (documented from FreeBSD VM accounting, not transplanted
//! from Linux/macOS):
//!
//! ```text
//! available = (free + inactive + cache + laundry) * page_size
//! used = total - min(available, total)
//! ```
//!
//! Swap and CPU frequency are truthfully unsupported in the first backend:
//! `kvm_getswapinfo` needs unprivileged validation across the supported
//! floor first, and no validated unprivileged frequency source was found.
//! Follow-ups are recorded in Plan 136 rather than scraping commands.

use std::time::Instant;

use crate::error::{CollectError, CollectErrorKind};
use crate::model::{
    CollectionLimits, DiskIoMetrics, DiskIoPayload, HostCapabilities, HostIdentity, HostSample,
    LoadAverage, NetworkInterfaceMetrics, NetworkPayload,
};
use crate::rate::CounterBaselines;
use crate::slow_probe::DriveRefreshCache;
use crate::{clamped_usage_pct, HostCollector};

pub mod source;

use source::{FreeBsdSource, RawCpuTimes};

#[cfg(test)]
mod tests;

/// A FreeBSD native collector.
pub struct FreeBsdCollector<S: FreeBsdSource = source::NativeFreeBsdSource> {
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

impl FreeBsdCollector<source::NativeFreeBsdSource> {
    /// Create a collector using the production native implementation.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        Self::with_source(source::NativeFreeBsdSource, display_name)
    }
}

impl<S: FreeBsdSource + Clone> FreeBsdCollector<S> {
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
        let logical_cores = raw_identity.logical_cores.max(1);
        let system_identity = collect_identity(&raw_identity, display_name);
        Ok(Self {
            source,
            identity: system_identity,
            capabilities: HostCapabilities {
                cpu_iowait: false,
                load_average: true,
                swap: false,
                memory_commit: false,
                drives: true,
                cpu_frequency: false,
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

/// Build the stable host identity (non-empty, NUL-free fields).
fn collect_identity(raw: &source::RawIdentity, display_name: Option<&str>) -> HostIdentity {
    let clip = |value: &str| -> String {
        let trimmed = value.trim().replace('\0', "");
        if trimmed.is_empty() {
            "unknown".to_string()
        } else {
            trimmed.chars().take(128).collect()
        }
    };
    HostIdentity {
        name: display_name.map_or_else(|| raw.hostname.clone(), |name| clip(name)),
        hostname: clip(&raw.hostname),
        os_name: "freebsd".to_string(),
        os_version: clip(&raw.os_version),
        kernel_name: "FreeBSD".to_string(),
        kernel_release: clip(&raw.kernel_release),
        architecture: clip(&raw.architecture),
    }
}

/// Compute aggregate CPU percentages from two `kern.cp_time` readings.
///
/// ```text
/// busy = user + nice + sys + intr
/// total = busy + idle
/// usage_pct = delta(busy) / delta(total) * 100
/// ```
///
/// No state maps to Linux `iowait`; `cpu_iowait` stays `false`.
pub fn compute_cpu_percentages(
    prev: &RawCpuTimes,
    curr: &RawCpuTimes,
) -> Result<f32, CollectError> {
    for (before, after) in [
        (prev.user, curr.user),
        (prev.nice, curr.nice),
        (prev.sys, curr.sys),
        (prev.intr, curr.intr),
        (prev.idle, curr.idle),
    ] {
        if after < before {
            return Err(CollectError::counter_reset(
                "FreeBSD CPU counter decreased; baseline discarded",
            ));
        }
    }
    let delta_busy = curr.busy().saturating_sub(prev.busy());
    let delta_total = curr.total().saturating_sub(prev.total());
    if delta_total == 0 {
        return Err(CollectError::counter_reset(
            "FreeBSD CPU total delta is zero; baseline discarded",
        ));
    }
    #[allow(clippy::cast_precision_loss)]
    let pct = (delta_busy as f64) * 100.0 / (delta_total as f64);
    crate::finalize_percentage(pct)
}

/// Compute memory metrics from FreeBSD VM inputs.
pub fn compute_memory(
    raw: &source::RawPhysicalMemory,
) -> Result<crate::model::MemoryMetrics, CollectError> {
    if raw.total_bytes == 0 {
        return Ok(crate::model::MemoryMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        });
    }
    if raw.page_size == 0 {
        return Err(CollectError::new(
            CollectErrorKind::Parse,
            "FreeBSD page size is zero",
        ));
    }
    let pages = raw
        .free_count
        .checked_add(raw.inactive_count)
        .and_then(|sum| sum.checked_add(raw.cache_count))
        .and_then(|sum| sum.checked_add(raw.laundry_count))
        .ok_or_else(|| {
            CollectError::new(CollectErrorKind::Numeric, "FreeBSD page count overflowed")
        })?;
    let available = pages.checked_mul(raw.page_size).ok_or_else(|| {
        CollectError::new(
            CollectErrorKind::Numeric,
            "FreeBSD available-bytes overflowed",
        )
    })?;
    let available = available.min(raw.total_bytes);
    let used = raw.total_bytes - available;
    Ok(crate::model::MemoryMetrics {
        used_bytes: used,
        total_bytes: raw.total_bytes,
        usage_pct: clamped_usage_pct(used, raw.total_bytes),
    })
}

fn parse_load(raw: &[f64; 3]) -> Result<LoadAverage, CollectError> {
    let parse_one = |value: f64, label: &str| -> Result<f32, CollectError> {
        if !value.is_finite() || value < 0.0 {
            return Err(CollectError::new(
                CollectErrorKind::Parse,
                format!("FreeBSD loadavg {label} is not finite/non-negative"),
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

fn collect_drives<S: FreeBsdSource>(
    source: &S,
    limits: &CollectionLimits,
) -> Result<Vec<crate::model::DriveMetrics>, CollectError> {
    let mounted = source.mounted_filesystems()?;
    let candidates = mounted
        .into_iter()
        .filter(|record| {
            record.flags & source::MNT_LOCAL != 0
                && !record.mount_point.is_empty()
                && record.filesystem_type != "devfs"
                && !record.filesystem_type.starts_with("autofs")
        })
        .filter_map(|record| {
            let unit = (record.block_size > 0).then_some(record.block_size)?;
            let total = record.total_blocks.checked_mul(unit)?;
            let free = record.free_blocks.checked_mul(unit)?;
            let available = record.available_blocks.checked_mul(unit)?;
            (total > 0 && free <= total && available <= total).then_some(
                crate::drives::DriveCandidate {
                    identity: format!("{}:{}", record.fsid.0, record.fsid.1),
                    name: record.mount_point,
                    total_bytes: total,
                    total_free_bytes: free,
                    available_bytes: available,
                },
            )
        })
        .collect();
    Ok(crate::drives::normalize_with_limits(candidates, limits))
}

impl<S: FreeBsdSource + Clone + 'static> FreeBsdCollector<S> {
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
        let records = match self.source.disk_io() {
            Ok(records) => records,
            Err(_) => {
                self.disk_baselines.clear();
                return None;
            }
        };
        self.disk_baselines
            .retain_ids(records.iter().map(|record| record.id.as_str()));
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
        let records = match self.source.network_interfaces() {
            Ok(records) => records,
            Err(_) => {
                self.network_baselines.clear();
                return None;
            }
        };
        self.network_baselines
            .retain_ids(records.iter().map(|record| record.id.as_str()));
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

impl<S: FreeBsdSource + Clone + 'static> HostCollector for FreeBsdCollector<S> {
    fn identity(&self) -> Result<HostIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<HostSample, CollectError> {
        let raw_cpu = self.source.cpu_times()?;
        let raw_load = self.source.load_averages()?;
        let raw_memory = self.source.physical_memory()?;
        let now = Instant::now();

        let cpu_pct = if let Some(prev) = self.previous_cpu.as_ref() {
            match compute_cpu_percentages(prev, &raw_cpu) {
                Ok(pct) => Some(pct),
                Err(error) if error.kind == CollectErrorKind::CounterReset => {
                    self.previous_cpu = Some(raw_cpu);
                    return Err(CollectError::counter_reset(
                        "FreeBSD CPU counters reset; baseline re-established",
                    ));
                }
                Err(other) => return Err(other),
            }
        } else {
            self.previous_cpu = Some(raw_cpu);
            return Err(CollectError::warming(
                "first FreeBSD CPU sample establishes the counter baseline",
            ));
        };
        self.previous_cpu = Some(raw_cpu);

        let load = parse_load(&raw_load)?;
        let memory = compute_memory(&raw_memory)?;

        Ok(HostSample {
            logical_cores: self.logical_cores,
            cpu_usage_pct: cpu_pct,
            cpu_iowait_pct: None,
            load: Some(load),
            memory,
            swap: None,
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
