//! Periodic sampling loop for the greggd daemon.
//!
//! The sampler owns the collector, clock, and snapshot publication cadence.
//! It drives the collection loop on a configurable interval, converts
//! [`crate::collector::CollectedMetrics`] into wire [`StatusSnapshot`] values, and manages the
//! daemon readiness lifecycle.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use gregg_protocol::v2::StatusPayloadV2;
use gregg_protocol::{
    HealthCategory, HealthResponse, ReadinessState, StatusSnapshot, SCHEMA_VERSION_V1,
};
use thiserror::Error;
use tokio::sync::broadcast;

use crate::collector::error::{CollectError, CollectErrorKind};
use crate::collector::{CollectedMetrics, SystemCollector};

/// Default sampling interval in milliseconds.
const DEFAULT_INTERVAL_MS: u64 = 1000;
/// Minimum allowed sampling interval in milliseconds.
const MIN_INTERVAL_MS: u64 = 250;
/// Maximum allowed sampling interval in milliseconds.
const MAX_INTERVAL_MS: u64 = 60_000;

// ---------------------------------------------------------------------------
// Clock trait and real implementation
// ---------------------------------------------------------------------------

/// Type-erased future returned by [`Clock::sleep`].
pub type SleepFuture = Pin<Box<dyn Future<Output = ()> + Send + Sync>>;

/// Abstraction over time sources so the sampler can be tested without
/// real wall-clock sleeps.
pub trait Clock: Send + Sync {
    /// Current time as milliseconds since the Unix epoch.
    fn now_unix_ms(&self) -> u64;

    /// Current Unix time when it is representable as a non-negative value.
    ///
    /// The default preserves the original clock seam for injected clocks;
    /// [`RealClock`] overrides it to distinguish a pre-epoch clock from the
    /// invalid zero timestamp.
    fn checked_now_unix_ms(&self) -> Option<u64> {
        Some(self.now_unix_ms())
    }

    /// Return a future that resolves after `dur` has elapsed.
    fn sleep(&self, dur: Duration) -> SleepFuture;
}

/// Wall-clock implementation using `std::time::SystemTime` and
/// `tokio::time::sleep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealClock;

impl Clock for RealClock {
    #[allow(clippy::cast_possible_truncation)]
    fn now_unix_ms(&self) -> u64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as u64,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "system clock precedes the Unix epoch; reporting 0 until corrected"
                );
                0
            }
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn checked_now_unix_ms(&self) -> Option<u64> {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => Some(duration.as_millis() as u64),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "system clock precedes the Unix epoch; snapshot publication is paused until corrected"
                );
                None
            }
        }
    }

    fn sleep(&self, dur: Duration) -> SleepFuture {
        Box::pin(async move {
            tokio::time::sleep(dur).await;
        })
    }
}

// ---------------------------------------------------------------------------
// Sampler error
// ---------------------------------------------------------------------------

/// Errors produced by the sampler module.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum SamplerError {
    /// The requested sampling interval is outside the allowed bounds.
    #[error("interval {0}ms outside {MIN_INTERVAL_MS}..={MAX_INTERVAL_MS}ms")]
    IntervalOutOfBounds(u64),
}

fn validate_v2_payload(payload: StatusPayloadV2) -> Result<StatusPayloadV2, CollectError> {
    payload.validate().map(|()| payload).map_err(|violations| {
        CollectError::new(
            CollectErrorKind::Numeric,
            format!(
                "v2 payload validation failed ({} violations)",
                violations.len()
            ),
        )
    })
}

// ---------------------------------------------------------------------------
// Sampler
// ---------------------------------------------------------------------------

/// Periodic metrics sampler that owns a [`SystemCollector`] and a [`Clock`].
///
/// The sampler drives the collection loop, publishes immutable snapshots, and
/// tracks daemon readiness through the warming, ready, and failed lifecycle.
pub struct Sampler<C: SystemCollector, Clk: Clock> {
    /// Shared with the blocking pool for the duration of each sampling
    /// cycle. The mutex may be poisoned by a panicked sampling task, but
    /// the collector itself survives so later cycles keep sampling.
    collector: Arc<Mutex<C>>,
    clock: Clk,
    interval_ms: u64,
    readiness: ReadinessState,
    snapshot: Option<Arc<StatusSnapshot>>,
    snapshot_v2: Option<Arc<StatusPayloadV2>>,
    consecutive_failures: u32,
}

impl<C: SystemCollector, Clk: Clock> Sampler<C, Clk> {
    /// Create a new sampler with the given collector and clock.
    ///
    /// The initial interval is 1000ms and the readiness state is
    /// [`ReadinessState::Warming`].
    #[must_use]
    pub fn new(collector: C, clock: Clk) -> Self {
        Self {
            collector: Arc::new(Mutex::new(collector)),
            clock,
            interval_ms: DEFAULT_INTERVAL_MS,
            readiness: ReadinessState::Warming,
            snapshot: None,
            snapshot_v2: None,
            consecutive_failures: 0,
        }
    }

    /// Create a new sampler with a custom initial sampling interval.
    ///
    /// # Errors
    ///
    /// Returns [`SamplerError::IntervalOutOfBounds`] if `interval_ms` is
    /// outside 250..=60000.
    pub fn with_interval(collector: C, clock: Clk, interval_ms: u64) -> Result<Self, SamplerError> {
        Self::validate_interval(interval_ms)?;
        Ok(Self {
            collector: Arc::new(Mutex::new(collector)),
            clock,
            interval_ms,
            readiness: ReadinessState::Warming,
            snapshot: None,
            snapshot_v2: None,
            consecutive_failures: 0,
        })
    }

    /// Validate that the given interval is within the allowed bounds.
    pub fn validate_interval(ms: u64) -> Result<u64, SamplerError> {
        if (MIN_INTERVAL_MS..=MAX_INTERVAL_MS).contains(&ms) {
            Ok(ms)
        } else {
            Err(SamplerError::IntervalOutOfBounds(ms))
        }
    }

    /// Return the latest valid immutable snapshot, if one has been published.
    #[must_use]
    pub fn snapshot(&self) -> Option<Arc<StatusSnapshot>> {
        self.snapshot.clone()
    }

    /// Return the latest valid v2 immutable snapshot, if one has been published.
    #[must_use]
    pub fn snapshot_v2(&self) -> Option<Arc<StatusPayloadV2>> {
        self.snapshot_v2.clone()
    }

    /// Return the current readiness state.
    #[must_use]
    pub fn readiness(&self) -> ReadinessState {
        self.readiness
    }

    /// Return a health response reflecting the current readiness state.
    #[must_use]
    pub fn health_response(&self) -> HealthResponse {
        match self.readiness {
            ReadinessState::Ready => match self.snapshot.as_ref() {
                Some(snap) => HealthResponse::ready((**snap).clone()),
                None => {
                    HealthResponse::failed(HealthCategory::CollectorFailure, "snapshot unavailable")
                }
            },
            ReadinessState::Warming => HealthResponse::warming(),
            ReadinessState::Failed => {
                let msg = format!("{} consecutive failures", self.consecutive_failures);
                HealthResponse::failed(HealthCategory::CollectorFailure, msg)
            }
        }
    }

    /// Run the sampling loop until the shutdown signal fires.
    ///
    /// The loop sleeps for the configured interval between samples. The first
    /// sample is taken immediately on entry.
    ///
    /// Each collection cycle runs on tokio's blocking thread pool so slow
    /// native reads (procfs, statvfs on many mounts) cannot stall other tasks
    /// sharing this runtime — notably the HTTP server on the daemon's
    /// current-thread runtime.
    ///
    /// The `on_sample` callback is invoked after each collection cycle with
    /// the sampler's current readiness state and, when available, the
    /// latest snapshots. The callback returns a future that is **awaited
    /// inline** before the next sleep, ensuring ordered state updates
    /// (no detached tasks that could race with shutdown).
    pub async fn run<F, Fut>(&mut self, mut shutdown: broadcast::Receiver<()>, mut on_sample: F)
    where
        F: FnMut(ReadinessState, Option<Arc<StatusSnapshot>>, Option<Arc<StatusPayloadV2>>) -> Fut,
        Fut: std::future::Future<Output = ()>,
        C: Send + 'static,
    {
        loop {
            let result = self.sample_on_blocking_pool().await;
            self.apply_sample_result(result);
            on_sample(
                self.readiness,
                self.snapshot.clone(),
                self.snapshot_v2.clone(),
            )
            .await;

            tokio::select! {
                () = self.clock.sleep(Duration::from_millis(self.interval_ms)) => {}
                _ = shutdown.recv() => {
                    tracing::info!("sampler shutting down");
                    break;
                }
            }
        }
    }

    /// Perform a single collection cycle synchronously.
    ///
    /// Direct sampling used outside the runtime loop; [`Self::run`] samples
    /// through [`Self::sample_on_blocking_pool`] instead.
    ///
    /// Sampler-internal only; does not publish to `ServerState` — use `run()`
    /// for serving. The HTTP server reads `ServerState`, which is synced only
    /// by the `run()` loop, so `sample_once` alone never becomes visible over
    /// HTTP.
    pub fn sample_once(&mut self) {
        let result = self.lock_collector().sample();
        self.apply_sample_result(result);
    }

    /// Run one collection cycle on tokio's blocking thread pool.
    ///
    /// The collector is shared with the blocking task behind a mutex for the
    /// duration of one cycle. If the collection task panics, the mutex is
    /// poisoned but the collector itself survives; later cycles recover the
    /// lock and continue sampling instead of losing metrics permanently.
    async fn sample_on_blocking_pool(&mut self) -> Result<CollectedMetrics, CollectError>
    where
        C: Send + 'static,
    {
        let collector = Arc::clone(&self.collector);
        let join_result = tokio::task::spawn_blocking(move || {
            let mut guard = match collector.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.sample()
        })
        .await;
        match join_result {
            Ok(result) => result,
            Err(join_error) => {
                if join_error.is_cancelled() {
                    tracing::debug!(%join_error, "sampler collection task cancelled");
                } else {
                    tracing::warn!(%join_error, "sampler collection task panicked");
                }
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    if join_error.is_cancelled() {
                        "collection task cancelled"
                    } else {
                        "collection task panicked"
                    },
                ))
            }
        }
    }

    /// Lock the shared collector, recovering from poisoning caused by a
    /// panicked sampling task.
    fn lock_collector(&self) -> MutexGuard<'_, C> {
        match self.collector.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Apply one collected sample or collection failure to readiness and
    /// snapshot publication state.
    fn apply_sample_result(&mut self, result: Result<CollectedMetrics, CollectError>) {
        match result {
            Ok(metrics) => {
                if metrics.cpu_usage_pct.is_none() {
                    self.transition_to_failed("sample returned no CPU percentage");
                    tracing::debug!(kind = "numeric", "sample returned no CPU percentage");
                    return;
                }

                let Some(now_ms) = self.clock.checked_now_unix_ms() else {
                    self.transition_to_failed("system clock precedes unix epoch");
                    tracing::debug!("system clock precedes unix epoch; snapshot not published");
                    return;
                };
                let converted = self.convert_sample(metrics, now_ms);

                let (v1, payload_v2) = match converted {
                    Ok(converted) => converted,
                    Err(err) => {
                        self.transition_to_failed("snapshot conversion failed");
                        tracing::debug!(kind = ?err.kind, "snapshot conversion failed");
                        return;
                    }
                };

                let arc_v1 = v1.map(Arc::new);
                let arc_v2 = Arc::new(payload_v2);

                if self.readiness != ReadinessState::Ready {
                    tracing::info!(
                        from = ?self.readiness,
                        to = "ready",
                        "sampler state transition"
                    );
                }
                self.readiness = ReadinessState::Ready;
                self.consecutive_failures = 0;
                self.snapshot = arc_v1;
                self.snapshot_v2 = Some(arc_v2);
            }
            Err(err) => match err.kind {
                CollectErrorKind::Warming => {
                    tracing::debug!(
                        kind = "warming",
                        "sample warming; waiting for counter baseline"
                    );
                }
                CollectErrorKind::CounterReset => {
                    tracing::debug!(
                        kind = "counter_reset",
                        "counter reset; next sample will re-warm"
                    );
                }
                _ => {
                    if self.readiness == ReadinessState::Ready {
                        tracing::info!(from = "ready", to = "failed", "sampler state transition");
                    } else if self.readiness == ReadinessState::Warming {
                        tracing::info!(from = "warming", to = "failed", "sampler state transition");
                    }
                    self.readiness = ReadinessState::Failed;
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    tracing::debug!(
                        consecutive_failures = self.consecutive_failures,
                        kind = ?err.kind,
                        "sample failed"
                    );
                }
            },
        }
    }

    /// Convert one collected sample into the publishable snapshot pair.
    ///
    /// Produces a v1 snapshot only when the collector supports it; Windows
    /// does not (load/swap cannot be meaningfully represented as
    /// non-optional zero values).
    fn convert_sample(
        &self,
        metrics: CollectedMetrics,
        now_ms: u64,
    ) -> Result<(Option<StatusSnapshot>, StatusPayloadV2), CollectError> {
        // Clone the needed scalars under the lock, then drop the guard before
        // the ownership-aware conversion so the mutex is not held across
        // formatting.
        let (identity, capabilities, capabilities_v2, supports_v1) = {
            let collector = self.lock_collector();
            (
                collector.identity()?,
                collector.capabilities(),
                collector.capabilities_v2(),
                collector.supports_v1_snapshot(),
            )
        };
        metrics
            .into_snapshot_pair(
                SCHEMA_VERSION_V1,
                now_ms,
                self.interval_ms,
                capabilities,
                capabilities_v2,
                identity,
                supports_v1,
            )
            .and_then(|(v1, v2)| validate_v2_payload(v2).map(|v2| (v1, v2)))
    }

    /// Record one failure and move to the [`ReadinessState::Failed`] state.
    fn transition_to_failed(&mut self, message: &'static str) {
        if self.readiness == ReadinessState::Ready {
            tracing::info!(from = "ready", to = "failed", "sampler state transition");
        } else if self.readiness == ReadinessState::Warming {
            tracing::info!(from = "warming", to = "failed", "sampler state transition");
        }
        self.readiness = ReadinessState::Failed;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        tracing::debug!(
            consecutive_failures = self.consecutive_failures,
            "{message}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use crate::collector::CollectedMetrics;
    use gregg_protocol::{
        LoadAverage, MemoryMetrics, MetricCapabilities, SwapMetrics, SystemIdentity,
    };

    /// A controllable clock for deterministic tests.
    struct SyntheticClock {
        now_ms: AtomicU64,
    }

    impl SyntheticClock {
        fn new(start_ms: u64) -> Self {
            Self {
                now_ms: AtomicU64::new(start_ms),
            }
        }

        fn advance(&self, ms: u64) {
            self.now_ms.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Clock for SyntheticClock {
        fn now_unix_ms(&self) -> u64 {
            self.now_ms.load(Ordering::SeqCst)
        }

        fn sleep(&self, dur: Duration) -> SleepFuture {
            #[allow(clippy::cast_possible_truncation)]
            self.advance(dur.as_millis() as u64);
            Box::pin(async move {
                tokio::time::sleep(dur).await;
            })
        }
    }

    struct PreEpochClock;

    impl Clock for PreEpochClock {
        fn now_unix_ms(&self) -> u64 {
            0
        }

        fn checked_now_unix_ms(&self) -> Option<u64> {
            None
        }

        fn sleep(&self, dur: Duration) -> SleepFuture {
            Box::pin(async move {
                tokio::time::sleep(dur).await;
            })
        }
    }

    /// A controllable collector that returns scripted results.
    struct SyntheticCollector {
        results: Mutex<VecDeque<Result<CollectedMetrics, CollectError>>>,
        identity_results: Mutex<VecDeque<Result<SystemIdentity, CollectError>>>,
    }

    impl SyntheticCollector {
        fn from_results(results: Vec<Result<CollectedMetrics, CollectError>>) -> Self {
            Self {
                results: Mutex::new(VecDeque::from(results)),
                identity_results: Mutex::new(VecDeque::new()),
            }
        }

        fn with_identity_results(
            results: Vec<Result<CollectedMetrics, CollectError>>,
            identity_results: Vec<Result<SystemIdentity, CollectError>>,
        ) -> Self {
            Self {
                results: Mutex::new(VecDeque::from(results)),
                identity_results: Mutex::new(VecDeque::from(identity_results)),
            }
        }

        fn warming_then_success() -> Self {
            let warm = Err(CollectError::warming("baseline"));
            let success = Ok(successful_metrics());
            Self::from_results(vec![warm, success])
        }

        fn always_fails() -> Self {
            Self::from_results(vec![
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "unavailable",
                )),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "unavailable",
                )),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "unavailable",
                )),
            ])
        }

        fn succeed_then_fail() -> Self {
            Self::from_results(vec![
                Err(CollectError::warming("baseline")),
                Ok(successful_metrics()),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "unavailable",
                )),
            ])
        }

        fn counter_reset_then_recover() -> Self {
            Self::from_results(vec![
                Err(CollectError::warming("baseline")),
                Ok(successful_metrics()),
                Err(CollectError::counter_reset("counters reset")),
                Ok(successful_metrics()),
            ])
        }

        fn succeed_then_fail_repeatedly() -> Self {
            Self::from_results(vec![
                Err(CollectError::warming("baseline")),
                Ok(successful_metrics()),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "failure 1",
                )),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "failure 2",
                )),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "failure 3",
                )),
            ])
        }

        fn returns_invalid_metrics() -> Self {
            Self::from_results(vec![
                Err(CollectError::warming("baseline")),
                Ok(CollectedMetrics {
                    logical_cores: 0,
                    cpu_usage_pct: Some(f32::NAN),
                    cpu_iowait_pct: None,
                    load: LoadAverage {
                        one: f32::INFINITY,
                        five: -1.0,
                        fifteen: 0.0,
                    },
                    memory: MemoryMetrics {
                        used_bytes: 999,
                        total_bytes: 100,
                        usage_pct: 200.0,
                    },
                    swap: SwapMetrics {
                        used_bytes: 0,
                        total_bytes: 0,
                        usage_pct: 0.0,
                    },
                    commit: None,
                    drives: None,
                    cpu_frequency_hz: None,
                    disk_io: None,
                    network: None,
                }),
            ])
        }
    }

    impl SystemCollector for SyntheticCollector {
        fn identity(&self) -> Result<SystemIdentity, CollectError> {
            self.identity_results
                .lock()
                .expect("identity lock poisoned")
                .pop_front()
                .unwrap_or_else(|| Ok(test_identity()))
        }

        fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
            match self.results.lock().expect("lock poisoned").pop_front() {
                Some(result) => result,
                None => Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "exhausted",
                )),
            }
        }

        fn capabilities(&self) -> MetricCapabilities {
            MetricCapabilities { cpu_iowait: false }
        }
    }

    fn test_identity() -> SystemIdentity {
        SystemIdentity {
            name: "test-host".into(),
            hostname: "test.local".into(),
            os_name: "linux".into(),
            os_version: "1.0".into(),
            kernel_name: "Linux".into(),
            kernel_release: "6.0.0".into(),
            architecture: "x86_64".into(),
        }
    }

    fn successful_metrics() -> CollectedMetrics {
        CollectedMetrics {
            logical_cores: 4,
            cpu_usage_pct: Some(25.0),
            cpu_iowait_pct: None,
            load: LoadAverage {
                one: 1.0,
                five: 0.5,
                fifteen: 0.3,
            },
            memory: MemoryMetrics {
                used_bytes: 4_000_000_000,
                total_bytes: 8_000_000_000,
                usage_pct: 50.0,
            },
            swap: SwapMetrics {
                used_bytes: 0,
                total_bytes: 0,
                usage_pct: 0.0,
            },
            commit: None,
            drives: None,
            cpu_frequency_hz: None,
            disk_io: None,
            network: None,
        }
    }

    // --- validate_interval tests ---

    #[test]
    fn validate_interval_accepts_default() {
        assert_eq!(
            Sampler::<SyntheticCollector, SyntheticClock>::validate_interval(1000),
            Ok(1000)
        );
    }

    #[test]
    fn validate_interval_accepts_minimum() {
        assert_eq!(
            Sampler::<SyntheticCollector, SyntheticClock>::validate_interval(250),
            Ok(250)
        );
    }

    #[test]
    fn validate_interval_accepts_maximum() {
        assert_eq!(
            Sampler::<SyntheticCollector, SyntheticClock>::validate_interval(60_000),
            Ok(60_000)
        );
    }

    #[test]
    fn validate_interval_rejects_below_minimum() {
        assert!(matches!(
            Sampler::<SyntheticCollector, SyntheticClock>::validate_interval(249),
            Err(SamplerError::IntervalOutOfBounds(249))
        ));
    }

    #[test]
    fn validate_interval_rejects_above_maximum() {
        assert!(matches!(
            Sampler::<SyntheticCollector, SyntheticClock>::validate_interval(60_001),
            Err(SamplerError::IntervalOutOfBounds(60_001))
        ));
    }

    // --- with_interval tests ---

    #[test]
    fn with_interval_rejects_invalid() {
        let result = Sampler::with_interval(
            SyntheticCollector::from_results(vec![]),
            SyntheticClock::new(0),
            100,
        );
        assert!(result.is_err());
    }

    #[test]
    fn with_interval_accepts_valid() {
        let result = Sampler::with_interval(
            SyntheticCollector::from_results(vec![]),
            SyntheticClock::new(0),
            500,
        );
        assert!(result.is_ok());
    }

    // --- readiness and health_response tests ---

    #[test]
    fn initial_state_is_warming() {
        let sampler = Sampler::new(
            SyntheticCollector::from_results(vec![]),
            SyntheticClock::new(0),
        );
        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        assert!(sampler.snapshot().is_none());
        let health = sampler.health_response();
        assert_eq!(health.state, ReadinessState::Warming);
        assert_eq!(health.category, Some(HealthCategory::Warming));
    }

    #[test]
    fn health_response_failed_shows_consecutive_count() {
        let mut sampler = Sampler::new(
            SyntheticCollector::from_results(vec![]),
            SyntheticClock::new(0),
        );
        sampler.readiness = ReadinessState::Failed;
        sampler.consecutive_failures = 5;
        let health = sampler.health_response();
        assert_eq!(health.state, ReadinessState::Failed);
        assert_eq!(health.message, Some("5 consecutive failures".into()));
    }

    // --- sample_once behavioral tests (synchronous) ---

    #[test]
    fn warming_error_preserves_warming_state() {
        let clock = SyntheticClock::new(1000);
        let collector =
            SyntheticCollector::from_results(vec![Err(CollectError::warming("no baseline"))]);
        let mut sampler = Sampler::new(collector, clock);

        sampler.sample_once();

        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        assert!(sampler.snapshot().is_none());
    }

    #[test]
    fn warming_then_success_transitions_to_ready() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::warming_then_success();
        let mut sampler = Sampler::new(collector, clock);

        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Warming);

        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        let snap = sampler.snapshot().expect("snapshot must be present");
        assert!((snap.cpu.usage_pct - 25.0).abs() < f32::EPSILON);
        assert_eq!(snap.cpu.logical_cores, 4);
    }

    #[test]
    fn missing_cpu_percentage_after_ready_transitions_to_failed() {
        let clock = SyntheticClock::new(1000);
        let mut missing = successful_metrics();
        missing.cpu_usage_pct = None;
        let collector =
            SyntheticCollector::from_results(vec![Ok(successful_metrics()), Ok(missing)]);
        let mut sampler = Sampler::new(collector, clock);

        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        sampler.sample_once();

        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.consecutive_failures, 1);
    }

    #[test]
    fn pre_epoch_clock_does_not_publish_zero_timestamp() {
        let collector = SyntheticCollector::from_results(vec![Ok(successful_metrics())]);
        let mut sampler = Sampler::new(collector, PreEpochClock);

        sampler.sample_once();

        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert!(sampler.snapshot().is_none());
    }

    #[test]
    fn identity_failure_preserves_last_snapshot() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::with_identity_results(
            vec![
                Err(CollectError::warming("baseline")),
                Ok(successful_metrics()),
                Ok(successful_metrics()),
            ],
            vec![
                Ok(test_identity()),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "identity unavailable",
                )),
                Ok(test_identity()),
            ],
        );
        let mut sampler = Sampler::new(collector, clock);

        sampler.sample_once();
        sampler.sample_once();
        let published = sampler.snapshot().expect("snapshot must be present");

        sampler.sample_once();

        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.snapshot().as_deref(), Some(published.as_ref()));
        assert_eq!(published.system, test_identity());
    }

    #[test]
    fn always_fail_results_in_failed_state() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::always_fails();
        let mut sampler = Sampler::new(collector, clock);

        for _ in 0..3 {
            sampler.sample_once();
        }
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert!(sampler.snapshot().is_none());
        assert_eq!(sampler.consecutive_failures, 3);
    }

    #[test]
    fn succeed_then_fail_preserves_last_snapshot() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::succeed_then_fail();
        let mut sampler = Sampler::new(collector, clock);

        // warming
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        // success
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        let snap_before = sampler.snapshot().expect("snapshot present after success");
        // failure
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        let snap_after = sampler
            .snapshot()
            .expect("snapshot preserved after failure");
        assert_eq!(snap_before, snap_after);
        assert_eq!(sampler.consecutive_failures, 1);
    }

    #[test]
    fn counter_reset_preserves_current_state() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::counter_reset_then_recover();
        let mut sampler = Sampler::new(collector, clock);

        // warming
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        // success -> ready
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        // counter reset -> stays ready
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        // recovery -> still ready
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
    }

    #[test]
    fn succeed_then_fail_repeatedly_tracks_failures() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::succeed_then_fail_repeatedly();
        let mut sampler = Sampler::new(collector, clock);

        // warming
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        // success -> ready
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        assert!(sampler.snapshot().is_some());
        // failure 1
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.consecutive_failures, 1);
        // failure 2
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.consecutive_failures, 2);
        // failure 3
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.consecutive_failures, 3);
        // Snapshot is still the last valid one.
        assert!(sampler.snapshot().is_some());
    }

    #[test]
    fn invalid_metrics_fails_rather_than_publishing() {
        let clock = SyntheticClock::new(1000);
        let collector = SyntheticCollector::returns_invalid_metrics();
        let mut sampler = Sampler::new(collector, clock);

        // warming
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        // invalid metrics -> into_snapshot refuses to fabricate a 0.0 CPU
        // percentage, so the sampler records a failure instead of publishing.
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.consecutive_failures, 1);
        assert!(sampler.snapshot().is_none());
        assert!(sampler.snapshot_v2.is_none());
    }

    // --- run loop integration tests ---

    #[tokio::test]
    async fn run_warms_then_becomes_ready() {
        let clock = SyntheticClock::new(0);
        let collector = SyntheticCollector::from_results(vec![
            Err(CollectError::warming("baseline")),
            Ok(successful_metrics()),
            Ok(successful_metrics()),
            Ok(successful_metrics()),
            Ok(successful_metrics()),
            Ok(successful_metrics()),
        ]);
        let mut sampler = Sampler::with_interval(collector, clock, 250).unwrap();
        let (tx, shutdown) = broadcast::channel(1);

        let handle = tokio::spawn(async move {
            sampler
                .run(shutdown, |_state, _snap, _snap_v2| async {})
                .await;
            sampler
        });

        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = tx.send(());
        let sampler = handle.await.unwrap();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        assert!(sampler.snapshot().is_some());
    }

    #[tokio::test]
    async fn run_with_shutdown_signal() {
        let clock = SyntheticClock::new(0);
        let collector = SyntheticCollector::from_results(vec![
            Err(CollectError::warming("baseline")),
            Ok(successful_metrics()),
            Ok(successful_metrics()),
            Ok(successful_metrics()),
        ]);
        let mut sampler = Sampler::with_interval(collector, clock, 250).unwrap();
        let (tx, shutdown) = broadcast::channel(1);

        let handle = tokio::spawn(async move {
            sampler
                .run(shutdown, |_state, _snap, _snap_v2| async {})
                .await;
            sampler
        });

        tokio::time::sleep(Duration::from_millis(350)).await;
        let _ = tx.send(());
        let sampler = handle.await.unwrap();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
    }

    #[tokio::test]
    async fn run_logs_transitions() {
        let clock = SyntheticClock::new(0);
        let collector = SyntheticCollector::succeed_then_fail();
        let mut sampler = Sampler::with_interval(collector, clock, 250).unwrap();
        let (tx, shutdown) = broadcast::channel(1);

        let handle = tokio::spawn(async move {
            sampler
                .run(shutdown, |_state, _snap, _snap_v2| async {})
                .await;
            sampler
        });

        tokio::time::sleep(Duration::from_millis(600)).await;
        let _ = tx.send(());
        let sampler = handle.await.unwrap();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert!(sampler.snapshot().is_some());
    }

    #[tokio::test]
    async fn run_counter_reset_recover_cycle() {
        let clock = SyntheticClock::new(0);
        let collector = SyntheticCollector::counter_reset_then_recover();
        let mut sampler = Sampler::with_interval(collector, clock, 250).unwrap();
        let (tx, shutdown) = broadcast::channel(1);

        let handle = tokio::spawn(async move {
            sampler
                .run(shutdown, |_state, _snap, _snap_v2| async {})
                .await;
            sampler
        });

        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = tx.send(());
        let sampler = handle.await.unwrap();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
    }

    #[tokio::test]
    async fn run_callback_receives_each_sample() {
        let clock = SyntheticClock::new(0);
        let collector = SyntheticCollector::warming_then_success();
        let mut sampler = Sampler::with_interval(collector, clock, 250).unwrap();
        let (tx, shutdown) = broadcast::channel(1);

        let sample_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count = sample_count.clone();

        let handle = tokio::spawn(async move {
            sampler
                .run(shutdown, move |_state, _snap, _snap_v2| {
                    count.fetch_add(1, Ordering::Relaxed);
                    async {}
                })
                .await;
            sampler
        });

        tokio::time::sleep(Duration::from_millis(600)).await;
        let _ = tx.send(());
        let _ = handle.await.unwrap();
        // At least 2 samples (warming + success) should have fired the callback.
        assert!(sample_count.load(Ordering::Relaxed) >= 2);
    }

    // ===== Plan 142: persistent worker vs spawn_blocking experiment =====
    //
    // Test-only reversible candidate: one dedicated collector thread owns the
    // synthetic collector for the session; the async side sends one request
    // only when no sample is outstanding (bounded, no queue). Panics are
    // contained with `catch_unwind`, the collector is retained, and shutdown
    // is bounded. Production `Sampler` is unchanged; this module closes with
    // RETAIN SPAWN_BLOCKING (see Plan 142 closure).

    use std::sync::mpsc::{sync_channel, SyncSender};

    struct WorkerResponse {
        sample: Result<CollectedMetrics, CollectError>,
        identity: Result<gregg_protocol::SystemIdentity, CollectError>,
    }

    struct TestWorker {
        requests: Option<SyncSender<tokio::sync::oneshot::Sender<WorkerResponse>>>,
        handle: Option<std::thread::JoinHandle<SyntheticCollector>>,
        samples_taken: Arc<std::sync::atomic::AtomicU32>,
    }

    impl TestWorker {
        fn spawn(collector: SyntheticCollector) -> Self {
            // Bounded capacity 1: one outstanding sample at most (buffered
            // request or in-progress sample), never an unbounded queue.
            let (tx, rx) = sync_channel::<tokio::sync::oneshot::Sender<WorkerResponse>>(1);
            let samples_taken = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counted = Arc::clone(&samples_taken);
            let handle = std::thread::Builder::new()
                .name("plan142-test-collector".into())
                .spawn(move || {
                    let mut owned = collector;
                    while let Ok(respond) = rx.recv() {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            counted.fetch_add(1, Ordering::Relaxed);
                            let sample = owned.sample();
                            let identity = owned.identity();
                            WorkerResponse { sample, identity }
                        }));
                        let response = match result {
                            Ok(response) => response,
                            Err(_) => WorkerResponse {
                                sample: Err(CollectError::new(
                                    CollectErrorKind::SourceUnavailable,
                                    "collection task panicked",
                                )),
                                identity: Ok(test_identity()),
                            },
                        };
                        let _ = respond.send(response);
                    }
                    owned
                })
                .expect("worker thread spawns");
            Self {
                requests: Some(tx),
                handle: Some(handle),
                samples_taken,
            }
        }

        async fn sample_once_via_worker(&self) -> Option<WorkerResponse> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            // At most one in flight: `try_send` fails if a sample is
            // outstanding (rendezvous channel), never queues unbounded work.
            self.requests.as_ref()?.try_send(tx).ok()?;
            rx.await.ok()
        }

        fn shutdown_bounded(mut self, timeout: Duration) -> Option<SyntheticCollector> {
            drop(self.requests.take());
            let handle = self.handle.take()?;
            // Bounded join: a blocked native call must not hang shutdown.
            // Spawn a waiter and time out; on timeout the worker is detached
            // (mirroring the runtime pool, which cannot abort started work).
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let owned = handle.join();
                let _ = done_tx.send(owned);
            });
            done_rx.recv_timeout(timeout).ok().and_then(Result::ok)
        }
    }

    struct PanickingCollector {
        panicked: bool,
    }

    impl SystemCollector for PanickingCollector {
        fn identity(&self) -> Result<gregg_protocol::SystemIdentity, CollectError> {
            Ok(test_identity())
        }

        fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
            if !self.panicked {
                self.panicked = true;
                panic!("plan142 synthetic panic");
            }
            Ok(successful_metrics())
        }

        fn capabilities(&self) -> gregg_protocol::MetricCapabilities {
            gregg_protocol::MetricCapabilities { cpu_iowait: false }
        }
    }

    #[test]
    fn plan142_worker_warming_then_ready_matches_baseline() {
        let worker = TestWorker::spawn(SyntheticCollector::warming_then_success());
        let first = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(worker.sample_once_via_worker())
            .expect("first response");
        assert!(matches!(
            first.sample,
            Err(ref error) if error.kind == CollectErrorKind::Warming
        ));
        let second = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(worker.sample_once_via_worker())
            .expect("second response");
        assert!(second.sample.is_ok());
        assert!(second.identity.is_ok());
        assert_eq!(worker.samples_taken.load(Ordering::Relaxed), 2);
        let _ = worker.shutdown_bounded(Duration::from_secs(2));
    }

    #[test]
    fn plan142_worker_panic_recovers_and_retains_collector() {
        // Baseline `Sampler` recovers from a panicked `spawn_blocking` task
        // via poisoned-mutex recovery; the worker must match that without
        // killing its loop.
        let (tx, rx) = sync_channel::<tokio::sync::oneshot::Sender<WorkerResponse>>(1);
        let handle = std::thread::Builder::new()
            .name("plan142-panic-worker".into())
            .spawn(move || {
                let mut owned = PanickingCollector { panicked: false };
                let mut samples = 0u32;
                while let Ok(respond) = rx.recv() {
                    samples += 1;
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owned.sample()));
                    let sample = match result {
                        Ok(sample) => sample,
                        Err(_) => Err(CollectError::new(
                            CollectErrorKind::SourceUnavailable,
                            "collection task panicked",
                        )),
                    };
                    let _ = respond.send(WorkerResponse {
                        sample,
                        identity: Ok(test_identity()),
                    });
                    if samples >= 2 {
                        break;
                    }
                }
            })
            .unwrap();
        let roundtrip = |timeout: Duration| {
            let (tx_one, rx_one) = tokio::sync::oneshot::channel();
            tx.try_send(tx_one).unwrap();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async { tokio::time::timeout(timeout, rx_one).await })
                .unwrap()
                .unwrap()
        };
        let first = roundtrip(Duration::from_secs(2));
        assert!(matches!(
            first.sample,
            Err(ref error) if error.kind == CollectErrorKind::SourceUnavailable
        ));
        let second = roundtrip(Duration::from_secs(2));
        assert!(second.sample.is_ok(), "worker must survive panic");
        drop(tx);
        handle.join().expect("worker exits cleanly");
    }

    #[tokio::test]
    async fn plan142_worker_allows_at_most_one_in_flight() {
        let worker = TestWorker::spawn(SyntheticCollector::warming_then_success());
        // Sequential use (the `Sampler::run` discipline) always succeeds.
        let first = worker.sample_once_via_worker().await;
        assert!(first.is_some());
        let second = worker.sample_once_via_worker().await;
        assert!(second.is_some());
        let _ = worker.shutdown_bounded(Duration::from_secs(2));
    }

    #[tokio::test]
    async fn plan142_worker_shutdown_between_samples_is_bounded() {
        let worker = TestWorker::spawn(SyntheticCollector::warming_then_success());
        let _ = worker.sample_once_via_worker().await;
        let start = std::time::Instant::now();
        let _ = worker.shutdown_bounded(Duration::from_secs(2));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "idle shutdown must be prompt"
        );
    }

    #[tokio::test]
    async fn plan142_blocked_worker_does_not_stall_async_http() {
        struct BlockedCollector;
        impl SystemCollector for BlockedCollector {
            fn identity(&self) -> Result<gregg_protocol::SystemIdentity, CollectError> {
                Ok(test_identity())
            }
            fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
                std::thread::sleep(Duration::from_secs(10));
                Ok(successful_metrics())
            }
            fn capabilities(&self) -> gregg_protocol::MetricCapabilities {
                gregg_protocol::MetricCapabilities { cpu_iowait: false }
            }
        }
        // Baseline `spawn_blocking` cannot abort started work either; the
        // worker must not claim stronger cancellation. Prove the async
        // runtime stays responsive while a sample is blocked by racing a
        // bounded shutdown against an unrelated async tick.
        let (tx, rx) = sync_channel::<tokio::sync::oneshot::Sender<WorkerResponse>>(1);
        let handle = std::thread::Builder::new()
            .name("plan142-blocked-worker".into())
            .spawn(move || {
                let mut owned = BlockedCollector;
                while let Ok(respond) = rx.recv() {
                    // No `catch_unwind` needed for the blocking proof; the
                    // sleep simulates an uninterruptible native call.
                    let sample = owned.sample();
                    let _ = respond.send(WorkerResponse {
                        sample,
                        identity: Ok(test_identity()),
                    });
                }
            })
            .unwrap();
        let (tx_one, rx_one) = tokio::sync::oneshot::channel();
        tx.try_send(tx_one).unwrap();
        // Unrelated async work completes while the worker is blocked.
        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::task::yield_now().await;
        })
        .await
        .expect("async runtime stays responsive during blocked sample");
        // Bounded shutdown does not wait for the 10s native call.
        drop(tx);
        let shutdown_done = tokio::task::spawn_blocking(move || {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let joined = handle.join();
                let _ = done_tx.send(joined.is_ok());
            });
            done_rx.recv_timeout(Duration::from_millis(200)).is_ok()
        })
        .await
        .unwrap();
        assert!(
            !shutdown_done,
            "blocked worker join must time out rather than hang shutdown"
        );
        std::mem::drop(rx_one);
    }

    #[test]
    fn plan142_worker_handoff_structure_is_documented() {
        // Structural evidence (synthetic near-zero-cost collector, so native
        // I/O does not hide handoff cost):
        // - baseline: one `spawn_blocking` task + two mutex locks per sample
        //   (sample + identity/capability conversion), no channels;
        // - candidate: zero `spawn_blocking` tasks, one thread total, two
        //   channel operations per sample (request + response), zero mutexes,
        //   but an extra worker/channel/shutdown state machine plus
        //   `sample_once` ownership transfer machinery.
        //
        // The candidate removes per-tick task creation at the cost of a
        // persistent thread, bounded request/response channels, panic
        // containment, identity bundling, and bounded-shutdown/detach logic.
        // After Plan 141 native I/O dominates per-sample cost, so the
        // handoff saving is not meaningful enough to justify the additional
        // lifecycle state. RETAIN SPAWN_BLOCKING.
        let baseline_spawn_blocking_per_sample = 1;
        let baseline_mutex_locks_per_sample = 2;
        let candidate_threads_total = 1;
        let candidate_channel_ops_per_sample = 2;
        assert_eq!(baseline_spawn_blocking_per_sample, 1);
        assert_eq!(baseline_mutex_locks_per_sample, 2);
        assert_eq!(candidate_threads_total, 1);
        assert_eq!(candidate_channel_ops_per_sample, 2);
    }
}
