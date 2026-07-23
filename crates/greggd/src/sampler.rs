//! Periodic sampling loop for the greggd daemon.
//!
//! The sampler owns the collector, clock, and snapshot publication cadence.
//! It drives the collection loop on a configurable interval, converts
//! [`crate::collector::CollectedMetrics`] into wire [`StatusSnapshot`] values, and manages the
//! daemon readiness lifecycle.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use gregg_protocol::{
    HealthCategory, HealthResponse, ReadinessState, StatusSnapshot, SCHEMA_VERSION_V1,
};
use thiserror::Error;
use tokio::sync::broadcast;

#[cfg(test)]
use crate::collector::error::CollectError;
use crate::collector::error::CollectErrorKind;
use crate::collector::SystemCollector;

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

    /// Return a future that resolves after `dur` has elapsed.
    fn sleep(&self, dur: Duration) -> SleepFuture;
}

/// Wall-clock implementation using `std::time::SystemTime` and
/// `tokio::time::sleep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealClock;

impl Clock for RealClock {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "u128 millis from SystemTime is well within u64 range"
    )]
    fn now_unix_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
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

// ---------------------------------------------------------------------------
// Sampler
// ---------------------------------------------------------------------------

/// Periodic metrics sampler that owns a [`SystemCollector`] and a [`Clock`].
///
/// The sampler drives the collection loop, publishes immutable snapshots, and
/// tracks daemon readiness through the warming, ready, and failed lifecycle.
pub struct Sampler<C: SystemCollector, Clk: Clock> {
    collector: C,
    clock: Clk,
    interval_ms: u64,
    readiness: ReadinessState,
    snapshot: Option<Arc<StatusSnapshot>>,
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
            collector,
            clock,
            interval_ms: DEFAULT_INTERVAL_MS,
            readiness: ReadinessState::Warming,
            snapshot: None,
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
            collector,
            clock,
            interval_ms,
            readiness: ReadinessState::Warming,
            snapshot: None,
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

    /// Return the current readiness state.
    #[must_use]
    pub fn readiness(&self) -> ReadinessState {
        self.readiness
    }

    /// Return a health response reflecting the current readiness state.
    #[must_use]
    pub fn health_response(&self) -> HealthResponse {
        match self.readiness {
            ReadinessState::Ready => {
                let snap = self
                    .snapshot
                    .as_ref()
                    .expect("Ready implies snapshot is Some");
                HealthResponse::ready((**snap).clone())
            }
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
    pub async fn run(&mut self, mut shutdown: broadcast::Receiver<()>) {
        loop {
            self.sample_once();

            tokio::select! {
                () = self.clock.sleep(Duration::from_millis(self.interval_ms)) => {}
                _ = shutdown.recv() => {
                    tracing::info!("sampler shutting down");
                    break;
                }
            }
        }
    }

    /// Perform a single collection cycle.
    pub fn sample_once(&mut self) {
        match self.collector.sample() {
            Ok(metrics) => {
                if metrics.cpu_usage_pct.is_none() {
                    tracing::debug!(
                        kind = "warming",
                        "sample returned no CPU percentage; staying in warming state"
                    );
                    return;
                }

                let snap = metrics.into_snapshot(
                    SCHEMA_VERSION_V1,
                    self.clock.now_unix_ms(),
                    self.interval_ms,
                    self.collector.capabilities(),
                    self.collector
                        .identity()
                        .unwrap_or_else(|_| gregg_protocol::SystemIdentity {
                            name: String::new(),
                            hostname: String::new(),
                            os_name: String::new(),
                            os_version: String::new(),
                            kernel_name: String::new(),
                            kernel_release: String::new(),
                            architecture: String::new(),
                        }),
                );
                let arc_snap = Arc::new(snap);

                if self.readiness != ReadinessState::Ready {
                    tracing::info!(
                        from = ?self.readiness,
                        to = "ready",
                        "sampler state transition"
                    );
                }
                self.readiness = ReadinessState::Ready;
                self.consecutive_failures = 0;
                self.snapshot = Some(arc_snap);
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
            #[allow(
                clippy::cast_possible_truncation,
                reason = "test durations are small and fit in u64"
            )]
            self.advance(dur.as_millis() as u64);
            Box::pin(async move {
                tokio::time::sleep(dur).await;
            })
        }
    }

    /// A controllable collector that returns scripted results.
    struct SyntheticCollector {
        results: Mutex<VecDeque<Result<CollectedMetrics, CollectError>>>,
    }

    impl SyntheticCollector {
        fn from_results(results: Vec<Result<CollectedMetrics, CollectError>>) -> Self {
            Self {
                results: Mutex::new(VecDeque::from(results)),
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
    }

    impl SystemCollector for SyntheticCollector {
        fn identity(&self) -> Result<SystemIdentity, CollectError> {
            Ok(test_identity())
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

    // --- run loop integration tests ---

    #[tokio::test]
    async fn run_warms_then_becomes_ready() {
        let clock = SyntheticClock::new(0);
        let collector = SyntheticCollector::warming_then_success();
        let mut sampler = Sampler::with_interval(collector, clock, 250).unwrap();
        let (tx, shutdown) = broadcast::channel(1);

        let handle = tokio::spawn(async move {
            sampler.run(shutdown).await;
            sampler
        });

        tokio::time::sleep(Duration::from_millis(500)).await;
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
            sampler.run(shutdown).await;
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
            sampler.run(shutdown).await;
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
            sampler.run(shutdown).await;
            sampler
        });

        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = tx.send(());
        let sampler = handle.await.unwrap();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
    }
}
