//! macOS collector entry point.
//!
//! Gathers identity, CPU, memory, swap, and load-average samples from
//! Mach host statistics and sysctl APIs. Platform-specific code lives
//! in this module; the shared collector contract is defined in
//! [`crate::collector`].

use gregg_protocol::{LoadAverage, MetricCapabilities, SystemIdentity};

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::{CollectedMetrics, SystemCollector};

pub mod cpu;
pub mod ffi;
pub mod identity;
pub mod memory;
pub mod normalize;
pub mod swap;

#[cfg(test)]
mod tests;

/// A macOS native collector.
///
/// Constructed once per daemon process. Identity and static fields are read
/// eagerly during construction so the first [`Self::sample`] returns a
/// warming error rather than blocking on identity I/O.
pub struct MacOsCollector<S: ffi::MacNativeQueries = ffi::FfiNativeQueries> {
    source: S,
    identity: SystemIdentity,
    capabilities: MetricCapabilities,
    previous_cpu: Option<ffi::RawCpuTicks>,
    logical_cores: u32,
    physical_memory_bytes: u64,
}

impl MacOsCollector<ffi::FfiNativeQueries> {
    /// Create a collector using the production FFI implementation.
    ///
    /// `display_name` overrides the user-facing `name` field only; the actual
    /// `hostname` continues to come from the host.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        Self::with_source(ffi::FfiNativeQueries, display_name)
    }
}

impl<S: ffi::MacNativeQueries> MacOsCollector<S> {
    /// Create a collector with an injected source. Intended for tests so
    /// synthetic values can be exercised without touching the host.
    pub fn with_source(source: S, display_name: Option<&str>) -> Result<Self, CollectError> {
        let raw_identity = source.identity()?;
        let logical_cores = raw_identity.logical_cores.max(1);
        let physical_memory_bytes = raw_identity.physical_memory_bytes;

        let system_identity = identity::collect_identity(&source, display_name)?;

        Ok(Self {
            source,
            identity: system_identity,
            capabilities: MetricCapabilities { cpu_iowait: false },
            previous_cpu: None,
            logical_cores,
            physical_memory_bytes,
        })
    }

    /// Borrow the underlying source mutably. Tests use this to swap values
    /// between samples; production code does not need it.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }
}

impl<S: ffi::MacNativeQueries> SystemCollector for MacOsCollector<S> {
    fn identity(&self) -> Result<SystemIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
        let raw_cpu = self.source.cpu_load_info()?;
        let raw_vm = self.source.vm_info64()?;
        let raw_swap = self.source.swap_usage()?;
        let raw_load = self.source.load_averages()?;

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

        Ok(CollectedMetrics {
            logical_cores: self.logical_cores,
            cpu_usage_pct: Some(cpu.usage_pct),
            cpu_iowait_pct: None,
            load,
            memory: memory.into_metrics(),
            swap: swap.into_metrics(),
        })
    }

    fn capabilities(&self) -> MetricCapabilities {
        self.capabilities
    }
}

/// Parse a `[f64; 3]` load average into the protocol [`LoadAverage`].
fn parse_loadavgs(raw: &[f64; 3]) -> Result<LoadAverage, CollectError> {
    let parse_one = |value: f64, label: &str| -> Result<f32, CollectError> {
        if !value.is_finite() || value < 0.0 {
            return Err(CollectError::new(
                CollectErrorKind::Parse,
                format!("loadavg {label} is not finite/non-negative"),
            ));
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "load averages bounded by host CPU count and probe interval; f32 has ample headroom"
        )]
        Ok(value as f32)
    };

    Ok(LoadAverage {
        one: parse_one(raw[0], "1")?,
        five: parse_one(raw[1], "5")?,
        fifteen: parse_one(raw[2], "15")?,
    })
}
