//! Linux collector entry point.
//!
//! Gathers identity, CPU, memory, swap, and load-average samples from
//! procfs and kernel interfaces. Platform-specific code lives in this
//! module; the shared collector contract is defined in
//! [`crate::collector`].

use gregg_protocol::{LoadAverage, MetricCapabilities, SystemIdentity};

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::{CollectedMetrics, SystemCollector};

mod cpu;
mod fixtures;
mod identity;
mod memory;
mod source;

#[cfg(test)]
mod tests;

pub use cpu::{compute_percentages, parse_proc_stat, CpuCounters, CpuSample as CpuSampleView};
pub use identity::{collect_identity, os_release_sample, synthetic_identity};
pub use memory::{
    compute_memory as parse_meminfo, compute_swap as parse_swap,
    parse_meminfo as parse_meminfo_raw, MemorySample as MeminfoSample,
    SwapSample as SwapInfoSample,
};
pub use source::{FileSource, MemorySource, ParsedMeminfo, ParsedProcStat, ProcSource};

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
    logical_cores: u32,
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
        let logical_cores = source
            .logical_core_count()
            .unwrap_or(1)
            .max(1)
            .try_into()
            .unwrap_or(u32::MAX);
        Ok(Self {
            source,
            identity,
            capabilities: MetricCapabilities { cpu_iowait: true },
            previous_cpu: None,
            logical_cores,
        })
    }

    /// Borrow the underlying [`ProcSource`] mutably. Tests use this to swap
    /// fixture content between samples; production code does not need it.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut ProcSource {
        &mut self.source
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

        self.previous_cpu = stat.aggregate;

        Ok(CollectedMetrics {
            logical_cores: self.logical_cores,
            cpu_usage_pct: Some(cpu_sample.usage_pct),
            cpu_iowait_pct: Some(cpu_sample.iowait_pct),
            load,
            memory: memory_sample.into_metrics(),
            swap: swap_sample.into_metrics(),
        })
    }

    fn capabilities(&self) -> MetricCapabilities {
        self.capabilities
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
        #[allow(
            clippy::cast_possible_truncation,
            reason = "load averages bounded by host CPU count and probe interval; f32 has ample headroom"
        )]
        let as_f32 = parsed as f32;
        Ok(as_f32)
    };

    Ok(LoadAverage {
        one: parse_one(one, "1")?,
        five: parse_one(five, "5")?,
        fifteen: parse_one(fifteen, "15")?,
    })
}
