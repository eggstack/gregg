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
use crate::collector::windows::source::{RawCpuTimes, WindowsSource};
use crate::collector::{CollectedMetrics, DriveRefreshCache, SystemCollector};

pub mod commit;
pub mod cpu;
pub mod identity;
pub mod memory;
pub mod source;

fn collect_drives<S: WindowsSource>(
    source: &S,
) -> Result<Vec<gregg_protocol::v2::DriveMetrics>, CollectError> {
    let raw = source.logical_drives()?;
    let candidates = raw
        .into_iter()
        .filter(|drive| {
            (drive.drive_type == source::DRIVE_FIXED || drive.drive_type == source::DRIVE_REMOVABLE)
                && !drive.root.is_empty()
        })
        .map(|drive| crate::collector::drives::DriveCandidate {
            identity: drive.root.clone(),
            name: drive.root,
            total_bytes: drive.total_bytes,
            total_free_bytes: drive.total_free_bytes,
            available_bytes: drive.available_bytes,
        })
        .collect();
    Ok(crate::collector::drives::normalize(candidates))
}

/// A Windows native collector.
///
/// Constructed once per daemon process. Identity and static fields are read
/// eagerly during construction so the first [`Self::sample`] returns a
/// warming error rather than blocking on identity I/O.
#[derive(Debug)]
pub struct WindowsCollector<S: WindowsSource = source::NativeWindowsSource> {
    source: S,
    identity: SystemIdentity,
    capabilities: MetricCapabilities,
    capabilities_v2: MetricCapabilitiesV2,
    previous_cpu: Option<RawCpuTimes>,
    logical_cores: u32,
    drive_refresh: Option<DriveRefreshCache>,
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

impl<S: WindowsSource + Clone> WindowsCollector<S> {
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
            drive_refresh: None,
        })
    }

    /// Borrow the underlying source mutably. Tests use this to swap values
    /// between samples; production code does not need it.
    #[must_use]
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }
}

impl<S: WindowsSource + Clone + 'static> WindowsCollector<S> {
    fn refresh_drives(&mut self) -> Option<Vec<gregg_protocol::v2::DriveMetrics>> {
        if self.drive_refresh.is_none() {
            self.drive_refresh = Some(DriveRefreshCache::new(
                self.source.clone(),
                collect_drives::<S>,
            ));
        }
        self.drive_refresh
            .as_mut()
            .and_then(DriveRefreshCache::poll)
    }
}

impl<S: WindowsSource + Clone + 'static> SystemCollector for WindowsCollector<S> {
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
            drives: self.refresh_drives(),
        })
    }

    fn capabilities(&self) -> MetricCapabilities {
        self.capabilities
    }

    fn capabilities_v2(&self) -> MetricCapabilitiesV2 {
        self.capabilities_v2
    }

    fn supports_v1_snapshot(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::windows::source::{MockWindowsSource, RawIdentity, RawProcessorTopology};
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
        for _ in 0..100 {
            let metrics = collector.sample().expect("core sample succeeds");
            if metrics.drives.is_some() {
                return metrics;
            }
            std::thread::yield_now();
        }
        panic!("drive refresh did not complete");
    }

    // --- Topology guard tests (Workstream C) ---

    #[test]
    fn single_group_within_limit_succeeds() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 64,
            group_count: 1,
        };
        let result = WindowsCollector::with_source(mock, None);
        assert!(
            result.is_ok(),
            "64 processors in 1 group should be accepted"
        );
    }

    #[test]
    fn single_group_exceeding_limit_rejected() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 65,
            group_count: 1,
        };
        let err = WindowsCollector::with_source(mock, None).expect_err("65 should be rejected");
        assert!(err.message.contains("exceeds supported limit"));
    }

    #[test]
    fn multi_group_rejected() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 8,
            group_count: 2,
        };
        let err = WindowsCollector::with_source(mock, None).expect_err("2 groups should fail");
        assert!(err.message.contains("multiple processor groups"));
    }

    #[test]
    fn single_processor_accepted() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 1,
            group_count: 1,
        };
        assert!(WindowsCollector::with_source(mock, None).is_ok());
    }

    #[test]
    fn boundary_sixty_four_accepted() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 64,
            group_count: 1,
        };
        assert!(WindowsCollector::with_source(mock, None).is_ok());
    }

    #[test]
    fn boundary_sixty_five_rejected() {
        let mut mock = mock_source();
        mock.topology = RawProcessorTopology {
            active_logical_processors: 65,
            group_count: 1,
        };
        assert!(WindowsCollector::with_source(mock, None).is_err());
    }

    #[test]
    fn topology_error_propagated() {
        let mut mock = mock_source();
        mock.topology_error = true;
        let err = WindowsCollector::with_source(mock, None).expect_err("topology error");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }

    #[test]
    fn identity_error_propagated() {
        let mut mock = mock_source();
        mock.identity_error = true;
        let err = WindowsCollector::with_source(mock, None).expect_err("identity error");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }

    // --- Structural invariant tests (Workstream I) ---

    #[test]
    fn identity_fields_are_nonempty() {
        let mock = mock_source();
        let collector = WindowsCollector::with_source(mock, None).expect("collector");
        let identity = collector.identity().expect("identity");
        assert!(!identity.hostname.is_empty());
        assert!(!identity.os_name.is_empty());
        assert_eq!(identity.os_name, "windows");
        assert!(!identity.kernel_name.is_empty());
        assert!(!identity.architecture.is_empty());
    }

    #[test]
    fn logical_cores_is_positive() {
        let mock = mock_source();
        let collector = WindowsCollector::with_source(mock, None).expect("collector");
        assert!(collector.logical_cores > 0);
    }

    #[test]
    fn memory_total_positive_used_not_exceeding_total() {
        let mock = mock_source();
        let collector = WindowsCollector::with_source(mock, None).expect("collector");
        let mut collector = collector;
        // First sample warms
        let _ = collector.sample();
        // Second sample produces metrics
        let metrics = collector.sample().expect("second sample");
        assert!(metrics.memory.total_bytes > 0);
        assert!(metrics.memory.used_bytes <= metrics.memory.total_bytes);
    }

    #[test]
    fn drive_capacity_preserves_total_free_and_caller_available() {
        let mut collector = WindowsCollector::with_source(mock_source(), None).expect("collector");
        let _ = collector.sample();
        let metrics = sample_until_drives(&mut collector);
        let drive = &metrics.drives.expect("drives")[0];

        assert_eq!(drive.used_bytes, 75);
        assert_eq!(drive.total_bytes, 100);
        assert_eq!(drive.available_bytes, Some(20));
    }

    #[test]
    fn first_sample_warms() {
        let mock = mock_source();
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let err = collector.sample().expect_err("first sample should warm");
        assert_eq!(err.kind, crate::collector::error::CollectErrorKind::Warming);
    }

    #[test]
    fn second_sample_becomes_ready() {
        let mut mock = mock_source();
        mock.auto_increment_cpu = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample(); // warm
        let metrics = collector.sample().expect("second sample");
        assert!(metrics.cpu_usage_pct.is_some());
        let cpu = metrics.cpu_usage_pct.unwrap();
        assert!(cpu.is_finite());
        assert!((0.0..=100.0).contains(&cpu));
    }

    #[test]
    fn commit_is_some() {
        let mock = mock_source();
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample();
        let metrics = collector.sample().expect("second sample");
        assert!(metrics.commit.is_some());
        let commit = metrics.commit.unwrap();
        assert!(commit.used_bytes <= commit.limit_bytes);
        assert!(commit.usage_pct.is_finite());
        assert!((0.0..=100.0).contains(&commit.usage_pct));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn unsupported_metrics_are_absent() {
        let mock = mock_source();
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample();
        let metrics = collector.sample().expect("second sample");
        // iowait should be None
        assert!(metrics.cpu_iowait_pct.is_none());
        // load should be zero (v1 convention for unsupported)
        assert_eq!(metrics.load.one, 0.0);
        assert_eq!(metrics.load.five, 0.0);
        assert_eq!(metrics.load.fifteen, 0.0);
        // swap should be zero (v1 convention for unsupported)
        assert_eq!(metrics.swap.used_bytes, 0);
        assert_eq!(metrics.swap.total_bytes, 0);
    }

    #[test]
    fn v2_capabilities_match_plan() {
        let mock = mock_source();
        let collector = WindowsCollector::with_source(mock, None).expect("collector");
        let v2 = collector.capabilities_v2();
        assert!(!v2.cpu_iowait);
        assert!(!v2.load_average);
        assert!(!v2.swap);
        assert!(v2.memory_commit);
    }

    #[test]
    fn supports_v1_snapshot_returns_false() {
        let mock = mock_source();
        let collector = WindowsCollector::with_source(mock, None).expect("collector");
        assert!(!collector.supports_v1_snapshot());
    }

    // --- CPU error handling tests ---

    #[test]
    fn cpu_error_propagated() {
        let mut mock = mock_source();
        mock.cpu_error = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample(); // warm
        let err = collector.sample().expect_err("cpu error");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }

    #[test]
    fn memory_error_propagated() {
        let mut mock = mock_source();
        mock.memory_error = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample(); // warm
        let err = collector.sample().expect_err("memory error");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }

    #[test]
    fn commit_error_propagated() {
        let mut mock = mock_source();
        mock.commit_error = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample(); // warm
        let err = collector.sample().expect_err("commit error");
        assert_eq!(
            err.kind,
            crate::collector::error::CollectErrorKind::SourceUnavailable
        );
    }

    #[test]
    fn drive_enumeration_failure_preserves_core_metrics_and_omits_drives() {
        let mut mock = mock_source();
        mock.drives_error = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample().expect_err("warming");
        let metrics = collector.sample().expect("core metrics remain available");

        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.memory.total_bytes > 0);
        assert!(metrics.drives.is_none());
    }

    #[test]
    fn filtered_drive_enumeration_is_successful_empty() {
        let mut mock = mock_source();
        mock.drives.clear();
        let mut collector = WindowsCollector::with_source(mock, None).expect("collector");
        let _ = collector.sample().expect_err("warming");
        let metrics = sample_until_drives(&mut collector);

        assert_eq!(metrics.drives, Some(Vec::new()));
    }
}
