//! Windows collector entry point.
//!
//! Gathers identity, CPU, memory, commit, and logical processor count
//! from native Windows APIs. Platform-specific code lives in this
//! module; the shared collector contract is defined in
//! [`crate::collector`].
//!
//! Windows does not expose Unix load average, Unix swap, or CPU I/O-wait
//! state. These are reported as unsupported with explicit capability
//! flags.

use gregg_protocol::v2::MetricCapabilitiesV2;
use gregg_protocol::{LoadAverage, MetricCapabilities, SystemIdentity};

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::windows::cpu::CpuSample;
use crate::collector::windows::source::{RawCpuTimes, WindowsSource};
use crate::collector::{CollectedMetrics, SystemCollector};

pub mod commit;
pub mod cpu;
pub mod identity;
pub mod memory;
pub mod source;

/// A Windows native collector.
///
/// Constructed once per daemon process. Identity and static fields are read
/// eagerly during construction so the first [`Self::sample`] returns a
/// warming error rather than blocking on identity I/O.
pub struct WindowsCollector<S: WindowsSource = source::NativeWindowsSource> {
    source: S,
    identity: SystemIdentity,
    capabilities: MetricCapabilities,
    capabilities_v2: MetricCapabilitiesV2,
    previous_cpu: Option<RawCpuTimes>,
    logical_cores: u32,
}

impl WindowsCollector<source::NativeWindowsSource> {
    /// Create a collector using the production FFI implementation.
    ///
    /// `display_name` overrides the user-facing `name` field only; the actual
    /// `hostname` continues to come from the host.
    ///
    /// # Errors
    ///
    /// Returns [`CollectError`] if identity or topology cannot be read.
    pub fn new(display_name: Option<&str>) -> Result<Self, CollectError> {
        Self::with_source(source::NativeWindowsSource, display_name)
    }
}

impl<S: WindowsSource> WindowsCollector<S> {
    /// Create a collector with an injected source. Intended for tests so
    /// synthetic values can be exercised without touching the host.
    ///
    /// # Errors
    ///
    /// Returns [`CollectError`] if identity or topology cannot be read.
    pub fn with_source(source: S, display_name: Option<&str>) -> Result<Self, CollectError> {
        let raw_identity = source.identity()?;
        let topology = source.processor_topology()?;

        // Guard: reject multi-group topologies or single-group counts above
        // the supported limit for `GetSystemTimes` aggregation.
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
            capabilities: MetricCapabilities { cpu_iowait: false },
            capabilities_v2: MetricCapabilitiesV2 {
                cpu_iowait: false,
                load_average: false,
                swap: false,
                memory_commit: true,
            },
            previous_cpu: None,
            logical_cores,
        })
    }

    /// Borrow the underlying source mutably. Tests use this to swap values
    /// between samples; production code does not need it.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }
}

impl<S: WindowsSource> SystemCollector for WindowsCollector<S> {
    fn identity(&self) -> Result<SystemIdentity, CollectError> {
        Ok(self.identity.clone())
    }

    fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
        let raw_cpu = self.source.cpu_times()?;
        let raw_memory = self.source.physical_memory()?;
        let raw_commit = self.source.commit()?;

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

        Ok(CollectedMetrics {
            logical_cores: self.logical_cores,
            cpu_usage_pct: Some(cpu.usage_pct),
            cpu_iowait_pct: None,
            load: LoadAverage {
                one: 0.0,
                five: 0.0,
                fifteen: 0.0,
            },
            memory: mem_sample.into_metrics(),
            swap: gregg_protocol::SwapMetrics {
                used_bytes: 0,
                total_bytes: 0,
                usage_pct: 0.0,
            },
            commit: Some(commit_sample.into_metrics()),
        })
    }

    fn capabilities(&self) -> MetricCapabilities {
        self.capabilities
    }

    fn capabilities_v2(&self) -> MetricCapabilitiesV2 {
        self.capabilities_v2
    }
}
