//! macOS collector entry point.
//!
//! Gathers identity, CPU, memory, swap, and load-average samples from
//! Mach host statistics and sysctl APIs. Moved without semantic change
//! from `greggd::collector::macos` (Plan 134); types are protocol-neutral.

use std::time::Instant;

use crate::error::{CollectError, CollectErrorKind};
use crate::model::{
    CollectionLimits, DiskIoMetrics, DiskIoPayload, HostCapabilities, HostIdentity, HostSample,
    LoadAverage, NetworkInterfaceMetrics, NetworkPayload,
};
use crate::rate::CounterBaselines;
use crate::slow_probe::DriveRefreshCache;
use crate::{clamped_usage_pct, HostCollector};

pub mod cpu;
pub mod ffi;
pub mod identity;
pub mod memory;
pub mod normalize;
pub mod swap;

fn collect_drives<S: ffi::MacNativeQueries>(
    source: &S,
    limits: &CollectionLimits,
) -> Result<Vec<crate::model::DriveMetrics>, CollectError> {
    let mounted = source.mounted_filesystems()?;
    let candidates = mounted
        .into_iter()
        .filter(|record| {
            record.flags & ffi::MNT_LOCAL != 0
                && record.flags & ffi::MNT_DONTBROWSE == 0
                && !record.mount_point.is_empty()
                && record.filesystem_type != "devfs"
                && record.filesystem_type != "autofs"
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

#[cfg(test)]
mod tests;

/// Bounded availability tracker for optional macOS telemetry families.
#[derive(Debug, Default)]
struct OptionalFamilyAvailability {
    drives: Option<bool>,
    disk_io: Option<bool>,
    network: Option<bool>,
}

impl OptionalFamilyAvailability {
    fn observe(&mut self, family: &'static str, available: bool, error: Option<&CollectError>) {
        let slot = match family {
            "drives" => &mut self.drives,
            "disk_io" => &mut self.disk_io,
            _ => &mut self.network,
        };
        let previous = *slot;
        *slot = Some(available);
        if previous == Some(available) {
            return;
        }
        if available {
            tracing::debug!(family, "macOS optional telemetry available");
        } else if previous.is_none() {
            if let Some(error) = error {
                tracing::debug!(
                    family,
                    kind = ?error.kind,
                    error = %error.message,
                    "macOS optional telemetry unavailable"
                );
            } else {
                tracing::debug!(family, "macOS optional telemetry unavailable");
            }
        } else if let Some(error) = error {
            tracing::warn!(
                family,
                kind = ?error.kind,
                error = %error.message,
                "macOS optional telemetry unavailable"
            );
        } else {
            tracing::warn!(family, "macOS optional telemetry unavailable");
        }
    }
}

/// A macOS native collector.
pub struct MacOsCollector<S: ffi::MacNativeQueries = ffi::FfiNativeQueries> {
    source: S,
    identity: HostIdentity,
    capabilities: HostCapabilities,
    limits: CollectionLimits,
    previous_cpu: Option<ffi::RawCpuTicks>,
    logical_cores: u32,
    physical_memory_bytes: u64,
    drive_refresh: Option<DriveRefreshCache>,
    disk_baselines: CounterBaselines,
    network_baselines: CounterBaselines,
    optional_availability: OptionalFamilyAvailability,
}

impl MacOsCollector<ffi::FfiNativeQueries> {
    /// Create a collector using the production FFI implementation.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        Self::with_source(ffi::FfiNativeQueries, display_name)
    }
}

impl<S: ffi::MacNativeQueries + Clone> MacOsCollector<S> {
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
        let physical_memory_bytes = raw_identity.physical_memory_bytes;

        let system_identity = identity::collect_identity(&source, display_name)?;

        Ok(Self {
            source,
            identity: system_identity,
            capabilities: HostCapabilities {
                cpu_iowait: false,
                load_average: true,
                swap: true,
                memory_commit: false,
                drives: true,
                cpu_frequency: false,
                disk_io: true,
                network: true,
            },
            limits,
            previous_cpu: None,
            logical_cores,
            physical_memory_bytes,
            drive_refresh: None,
            disk_baselines: CounterBaselines::default(),
            network_baselines: CounterBaselines::default(),
            optional_availability: OptionalFamilyAvailability::default(),
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

impl<S: ffi::MacNativeQueries + Clone + 'static> MacOsCollector<S> {
    fn refresh_drives(&mut self) -> Option<Vec<crate::model::DriveMetrics>> {
        if self.drive_refresh.is_none() {
            let limits = self.limits;
            self.drive_refresh = Some(DriveRefreshCache::new(self.source.clone(), move |source| {
                collect_drives(source, &limits)
            }));
        }
        let drives = self
            .drive_refresh
            .as_mut()
            .and_then(DriveRefreshCache::poll);
        self.optional_availability
            .observe("drives", drives.is_some(), None);
        drives
    }

    fn collect_disk_io(&mut self, now: Instant) -> Option<DiskIoPayload> {
        let records = match self.source.disk_io() {
            Ok(records) => {
                self.optional_availability.observe("disk_io", true, None);
                records
            }
            Err(error) => {
                self.optional_availability
                    .observe("disk_io", false, Some(&error));
                self.disk_baselines.clear();
                return None;
            }
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
        let records = match self.source.network_interfaces() {
            Ok(records) => {
                self.optional_availability.observe("network", true, None);
                records
            }
            Err(error) => {
                self.optional_availability
                    .observe("network", false, Some(&error));
                self.network_baselines.clear();
                return None;
            }
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

impl<S: ffi::MacNativeQueries + Clone + 'static> HostCollector for MacOsCollector<S> {
    fn identity(&self) -> Result<HostIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<HostSample, CollectError> {
        let raw_cpu = self.source.cpu_load_info()?;
        let raw_vm = self.source.vm_info64()?;
        let raw_swap = self.source.swap_usage()?;
        let raw_load = self.source.load_averages()?;
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

        let load = parse_loadavgs(&raw_load)?;
        let memory = memory::compute_memory(&raw_vm, self.physical_memory_bytes)?;
        let swap = swap::compute_swap(&raw_swap);

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
            load: Some(load),
            memory: crate::model::MemoryMetrics {
                used_bytes: memory.used_bytes,
                total_bytes: memory.total_bytes,
                usage_pct: clamped_usage_pct(memory.used_bytes, memory.total_bytes),
            },
            swap: Some(crate::model::SwapMetrics {
                used_bytes: swap.used_bytes,
                total_bytes: swap.total_bytes,
                usage_pct: clamped_usage_pct(swap.used_bytes, swap.total_bytes),
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

/// Parse a `[f64; 3]` load average into [`LoadAverage`].
fn parse_loadavgs(raw: &[f64; 3]) -> Result<LoadAverage, CollectError> {
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
