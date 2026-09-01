//! Native metrics collection.
//!
//! The collector boundary isolates platform-specific sampling from the daemon
//! sampler and the HTTP surface. The shared trait is implemented by per-OS
//! modules that read their own native kernel or user-space interfaces and
//! return a normalized, daemon-internal sample. The sampler in phase 4 owns
//! cadence, clock, and snapshot publication.
//!
//! # Design rules
//!
//! - The collector never spawns external commands. Linux uses procfs and
//!   sysinfo interfaces; macOS uses Mach and sysctl APIs behind a contained
//!   FFI module added in phase 3.
//! - The collector never owns a clock. The daemon samples call
//!   [`SystemCollector::sample`] and stamp [`StatusSnapshot::observed_at_unix_ms`]
//!   in the sampler.
//! - All percentage normalization, counter-delta handling, and warming-up
//!   state live behind the trait, not in the protocol crate.
//! - Errors are typed so the daemon can distinguish a warming baseline from a
//!   hard collector failure when reporting health.

use gregg_protocol::v2::{
    CommitMetrics, CpuMetricsV2, DriveMetrics, MetricCapabilitiesV2, StatusPayloadV2,
    StatusSnapshotV2, SwapMetrics as SwapMetricsV2, SCHEMA_VERSION_V2,
};
use gregg_protocol::{
    CpuMetrics, LoadAverage, MemoryMetrics, MetricCapabilities, StatusSnapshot, SwapMetrics,
    SystemIdentity,
};

mod drives;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

pub mod error;

use error::{CollectError, CollectErrorKind};

const DRIVE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct DriveRefreshCache {
    request_tx: Option<std::sync::mpsc::SyncSender<()>>,
    result_rx: std::sync::mpsc::Receiver<Result<Vec<DriveMetrics>, CollectError>>,
    latest: Option<Vec<DriveMetrics>>,
}

impl DriveRefreshCache {
    pub(crate) fn new<S, F>(source: S, collect: F) -> Self
    where
        S: Send + 'static,
        F: Fn(&S) -> Result<Vec<DriveMetrics>, CollectError> + Send + 'static,
    {
        let (request_tx, request_rx) = std::sync::mpsc::sync_channel(1);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || loop {
            let request = request_rx.recv_timeout(DRIVE_REFRESH_INTERVAL);
            if matches!(
                request,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ) {
                break;
            }
            let Ok(result) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collect(&source)))
            else {
                tracing::warn!("drive refresh worker collector panicked; continuing");
                continue;
            };
            // Do not discard a completed refresh merely because the sampler
            // has not drained the previous result yet. A bounded send keeps
            // the worker from racing ahead while still allowing cache drop
            // to disconnect it without blocking the owner.
            if result_tx.send(result).is_err() {
                break;
            }
        });
        drop(worker);
        let _ = request_tx.try_send(());
        Self {
            request_tx: Some(request_tx),
            result_rx,
            latest: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn request(&self) {
        if let Some(sender) = &self.request_tx {
            let _ = sender.try_send(());
        }
    }

    pub(crate) fn poll(&mut self) -> Option<Vec<DriveMetrics>> {
        while let Ok(result) = self.result_rx.try_recv() {
            match result {
                Ok(drives) => self.latest = Some(drives),
                Err(error) => tracing::debug!(kind = ?error.kind),
            }
        }
        self.latest.clone()
    }
}

impl Drop for DriveRefreshCache {
    fn drop(&mut self) {
        let _ = self.request_tx.take();
    }
}

/// Shared clamped percentage normalization for byte ratios.
///
/// Zero total yields `0.0` rather than a division by zero; the result is
/// clamped to the closed `0.0..=100.0` interval. Every collector path that
/// derives a percentage from used/total bytes must go through this helper so
/// v1 and v2 snapshots can never diverge.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub(crate) fn clamped_usage_pct(used_bytes: u64, total_bytes: u64) -> f32 {
    if total_bytes == 0 {
        0.0
    } else if used_bytes >= total_bytes {
        100.0
    } else {
        let pct = (used_bytes as f64 / total_bytes as f64) * 100.0;
        // Re-check finiteness after the narrowing cast so a non-finite
        // intermediate can never reach the wire, mirroring the CPU
        // percentage finalizers in `collector/linux/cpu.rs`.
        let value = pct as f32;
        if value.is_finite() {
            value.clamp(0.0, 100.0)
        } else {
            0.0
        }
    }
}

/// Normalized metric sample produced by a [`SystemCollector`].
///
/// The struct is daemon-internal: it carries fields that do not appear on the
/// wire so collectors can express transient states (warming, counter reset)
/// without polluting the protocol. The daemon sampler maps it losslessly into
/// a [`StatusSnapshot`] once it is ready for publication.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectedMetrics {
    /// Logical CPU core count. Always `> 0` for a successfully collected
    /// identity snapshot.
    pub logical_cores: u32,
    /// Aggregate CPU busy percentage derived from a counter interval. `None`
    /// while warming up or immediately after a counter reset.
    pub cpu_usage_pct: Option<f32>,
    /// Aggregate Linux CPU I/O-wait percentage. Always `None` for non-Linux
    /// collectors; on Linux it is `Some` once a valid interval exists.
    pub cpu_iowait_pct: Option<f32>,
    /// Load averages parsed verbatim from the platform source.
    pub load: LoadAverage,
    /// Physical memory utilization.
    pub memory: MemoryMetrics,
    /// Swap utilization.
    pub swap: SwapMetrics,
    /// Windows commit charge. `None` on Linux/macOS; `Some` on Windows
    /// when the collector reports commit metrics.
    pub commit: Option<CommitMetrics>,
    /// Optional bounded native drive capacity data. `None` means enumeration
    /// was unavailable; an empty list means it succeeded with no eligible
    /// local filesystems.
    pub drives: Option<Vec<DriveMetrics>>,
}

impl CollectedMetrics {
    /// Convert this sample into a wire [`StatusSnapshot`].
    ///
    /// The caller (the daemon sampler) is responsible for filling in
    /// `schema_version`, `observed_at_unix_ms`, and `sample_interval_ms`.
    /// Optional metrics are set according to the platform capability flags.
    ///
    /// # Errors
    ///
    /// Returns [`CollectErrorKind::Numeric`] rather than fabricating a
    /// `0.0` placeholder when [`Self::cpu_usage_pct`] is missing or
    /// non-finite, or — when the platform reports I/O wait — when
    /// [`Self::cpu_iowait_pct`] is missing or non-finite. Callers should not
    /// publish a snapshot while [`Self::cpu_usage_pct`] is `None`.
    pub fn into_snapshot(
        self,
        schema_version: u16,
        observed_at_unix_ms: u64,
        sample_interval_ms: u64,
        capabilities: MetricCapabilities,
        system: SystemIdentity,
    ) -> Result<StatusSnapshot, CollectError> {
        let Some(cpu_usage_pct) = self.cpu_usage_pct.filter(|v| v.is_finite()) else {
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "cpu usage percentage is missing or non-finite",
            ));
        };
        let cpu_iowait_pct = if capabilities.cpu_iowait {
            let Some(iowait_pct) = self.cpu_iowait_pct.filter(|v| v.is_finite()) else {
                return Err(CollectError::new(
                    CollectErrorKind::Numeric,
                    "cpu iowait percentage is missing or non-finite",
                ));
            };
            Some(iowait_pct)
        } else {
            None
        };
        Ok(StatusSnapshot {
            schema_version,
            observed_at_unix_ms,
            sample_interval_ms,
            capabilities,
            system,
            cpu: CpuMetrics {
                logical_cores: self.logical_cores,
                usage_pct: cpu_usage_pct,
                iowait_pct: cpu_iowait_pct,
            },
            load: self.load,
            memory: self.memory,
            swap: self.swap,
        })
    }

    /// Convert this sample into a wire [`StatusSnapshotV2`].
    ///
    /// The caller (the daemon sampler) is responsible for filling in
    /// `observed_at_unix_ms` and `sample_interval_ms`. Optional metrics
    /// (load, swap, commit) are set according to the v2 capability flags.
    ///
    /// # Errors
    ///
    /// Returns [`CollectErrorKind::Numeric`] rather than fabricating a
    /// `0.0` placeholder when [`Self::cpu_usage_pct`] is missing or
    /// non-finite, or — when the platform reports I/O wait — when
    /// [`Self::cpu_iowait_pct`] is missing or non-finite.
    pub fn into_snapshot_v2(
        self,
        observed_at_unix_ms: u64,
        sample_interval_ms: u64,
        capabilities: MetricCapabilitiesV2,
        system: SystemIdentity,
    ) -> Result<StatusSnapshotV2, CollectError> {
        let Some(cpu_usage_pct) = self.cpu_usage_pct.filter(|v| v.is_finite()) else {
            return Err(CollectError::new(
                CollectErrorKind::Numeric,
                "cpu usage percentage is missing or non-finite",
            ));
        };
        let cpu_iowait_pct = if capabilities.cpu_iowait {
            let Some(iowait_pct) = self.cpu_iowait_pct.filter(|v| v.is_finite()) else {
                return Err(CollectError::new(
                    CollectErrorKind::Numeric,
                    "cpu iowait percentage is missing or non-finite",
                ));
            };
            Some(iowait_pct)
        } else {
            None
        };

        let load = if capabilities.load_average {
            Some(self.load)
        } else {
            None
        };

        let swap = if capabilities.swap {
            Some(SwapMetricsV2 {
                used_bytes: self.swap.used_bytes,
                total_bytes: self.swap.total_bytes,
                usage_pct: clamped_usage_pct(self.swap.used_bytes, self.swap.total_bytes),
            })
        } else {
            None
        };

        Ok(StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms,
            sample_interval_ms,
            capabilities,
            system,
            cpu: CpuMetricsV2 {
                logical_cores: self.logical_cores,
                usage_pct: cpu_usage_pct,
                iowait_pct: cpu_iowait_pct,
            },
            load,
            memory: self.memory,
            swap,
            commit: self.commit,
        })
    }

    /// Convert this sample into the flat v2 status payload, preserving drive
    /// availability semantics for the client.
    ///
    /// # Errors
    ///
    /// Returns [`CollectErrorKind::Numeric`] under the same conditions as
    /// [`Self::into_snapshot_v2`].
    pub fn into_status_payload_v2(
        self,
        observed_at_unix_ms: u64,
        sample_interval_ms: u64,
        capabilities: MetricCapabilitiesV2,
        system: SystemIdentity,
    ) -> Result<StatusPayloadV2, CollectError> {
        let mut this = self;
        let drives = this.drives.take();
        let snapshot = this.into_snapshot_v2(
            observed_at_unix_ms,
            sample_interval_ms,
            capabilities,
            system,
        )?;
        Ok(StatusPayloadV2 { snapshot, drives })
    }
}

/// Shared collector contract implemented by every platform-specific collector.
///
/// The contract is intentionally minimal: it owns identity collection and one
/// incremental sample. The daemon sampler owns cadence and clock.
pub trait SystemCollector: Send {
    /// Read identity fields once and cache them inside the collector.
    ///
    /// Identity is expected to be stable for the lifetime of the daemon, but
    /// re-reading is permitted if the host's identity changes (for example a
    /// hostname rename).
    fn identity(&self) -> Result<SystemIdentity, error::CollectError>;

    /// Take one native sample.
    ///
    /// The first call after construction is expected to return
    /// [`error::CollectErrorKind::Warming`] because percentage metrics
    /// require a second reading. Once two valid samples exist the collector
    /// returns normalized [`CollectedMetrics`].
    fn sample(&mut self) -> Result<CollectedMetrics, error::CollectError>;

    /// Per-platform metric capability flags.
    fn capabilities(&self) -> MetricCapabilities;

    /// Per-platform metric capability flags for schema version 2.
    ///
    /// The default implementation derives v2 capabilities from v1
    /// capabilities. Platform collectors may override this if v2
    /// capabilities differ from v1.
    fn capabilities_v2(&self) -> MetricCapabilitiesV2 {
        let v1 = self.capabilities();
        MetricCapabilitiesV2 {
            cpu_iowait: v1.cpu_iowait,
            load_average: true,
            swap: true,
            memory_commit: false,
        }
    }

    /// Whether this collector supports producing a v1 `StatusSnapshot`.
    ///
    /// Returns `true` by default. Windows returns `false` because v1
    /// requires non-optional `load` and `swap` fields that have no
    /// meaningful representation on Windows. The sampler skips v1 snapshot
    /// production when this returns `false`, causing `/v1/status` to
    /// return 404.
    fn supports_v1_snapshot(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::error::{CollectError, CollectErrorKind};
    use super::{clamped_usage_pct, DriveRefreshCache};
    use gregg_protocol::v2::DriveMetrics;

    #[test]
    fn large_byte_ratios_remain_finite_and_clamped() {
        assert!((clamped_usage_pct(0, u64::MAX) - 0.0).abs() < f32::EPSILON);
        assert!((clamped_usage_pct(u64::MAX, u64::MAX) - 100.0).abs() < f32::EPSILON);
        assert!((clamped_usage_pct(u64::MAX, u64::MAX - 1) - 100.0).abs() < f32::EPSILON);
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        for _ in 0..10_000 {
            if condition() {
                return;
            }
            std::thread::yield_now();
        }
        assert!(condition(), "worker did not reach expected state");
    }

    #[test]
    fn blocked_drive_refresh_does_not_block_cache_drop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let started_for_worker = Arc::clone(&started);
        let release_for_worker = Arc::clone(&release);
        let mut cache = DriveRefreshCache::new((), move |()| {
            started_for_worker.store(true, Ordering::Release);
            while !release_for_worker.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok(Vec::new())
        });

        wait_until(|| started.load(Ordering::Acquire));
        assert_eq!(cache.poll(), None);
        let before = std::time::Instant::now();
        drop(cache);
        assert!(before.elapsed() < std::time::Duration::from_millis(100));
        release.store(true, Ordering::Release);
    }

    #[test]
    fn drive_refresh_retains_last_success_after_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_worker = Arc::clone(&calls);
        let mut cache = DriveRefreshCache::new((), move |()| {
            let call = calls_for_worker.fetch_add(1, Ordering::AcqRel);
            if call == 0 {
                Ok(vec![DriveMetrics {
                    name: "root".to_string(),
                    used_bytes: 1,
                    total_bytes: 2,
                    available_bytes: Some(1),
                }])
            } else {
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "refresh failed",
                ))
            }
        });

        wait_until(|| cache.poll().is_some());
        let first = cache.poll().expect("first drive result");
        assert_eq!(first[0].name, "root");
        cache.request();
        wait_until(|| calls.load(Ordering::Acquire) >= 2);
        assert_eq!(
            cache.poll().expect("last good drive result")[0].used_bytes,
            1
        );
    }

    #[test]
    fn drive_refresh_does_not_drop_a_new_result_while_previous_is_queued() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_worker = Arc::clone(&calls);
        let mut cache = DriveRefreshCache::new((), move |()| {
            let call = calls_for_worker.fetch_add(1, Ordering::AcqRel);
            Ok(vec![DriveMetrics {
                name: format!("drive-{call}"),
                used_bytes: call as u64,
                total_bytes: 10,
                available_bytes: Some(10 - call as u64),
            }])
        });

        wait_until(|| calls.load(Ordering::Acquire) >= 1);
        cache.request();
        wait_until(|| calls.load(Ordering::Acquire) >= 2);
        wait_until(|| {
            cache
                .poll()
                .is_some_and(|drives| drives[0].name == "drive-1")
        });
    }

    #[test]
    fn drive_refresh_recovers_after_collector_panic() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_worker = Arc::clone(&calls);
        let mut cache = DriveRefreshCache::new((), move |()| {
            assert_ne!(
                calls_for_worker.fetch_add(1, Ordering::AcqRel),
                0,
                "injected drive refresh panic"
            );
            Ok(Vec::new())
        });

        wait_until(|| calls.load(Ordering::Acquire) >= 1);
        cache.request();
        wait_until(|| calls.load(Ordering::Acquire) >= 2);
        wait_until(|| cache.poll().is_some());
        assert_eq!(cache.poll(), Some(Vec::new()));
    }
}
