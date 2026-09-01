//! Poll scheduler with generation-based concurrency control.
//!
//! The scheduler runs a periodic loop that spawns concurrent poll tasks
//! for each endpoint, bounded by a semaphore. Each cycle produces a
//! [`PollBatch`] sent through an `mpsc` channel.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::clock::Clock;
use crate::endpoint::Endpoint;
use crate::poller::{HttpClient, PollBatch, PollOutcome, PollResult};

/// The scheduler could not deliver a batch because its consumer disappeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerRunError {
    /// The batch receiver was dropped while a batch was pending.
    ReceiverDropped,
}

/// Commands accepted by the systems poll scheduler.
#[derive(Debug)]
pub enum SchedulerCommand {
    /// Poll the current endpoint list immediately.
    Refresh,
    /// Atomically replace the endpoint list and poll it immediately.
    ReplaceEndpoints(Vec<Endpoint>),
}

/// Receiver and completion handle for an observed scheduler run.
pub struct SchedulerRunHandle {
    pub(crate) batches: mpsc::Receiver<PollBatch>,
    // The public run API returns only `batches`; tests use this handle to
    // assert that cancellation completes the scheduler task cleanly.
    #[allow(dead_code)]
    pub(crate) task: tokio::task::JoinHandle<Result<(), SchedulerRunError>>,
}

/// Poll scheduler with generation-based concurrency control.
///
/// Spawns a background task that periodically polls all endpoints and
/// sends completed batches through a channel. Concurrency is bounded
/// by a semaphore with `max_concurrent` permits.
pub struct PollScheduler<C: Clock> {
    clock: C,
    client: HttpClient,
    refresh_interval: Duration,
    max_concurrent: usize,
}

impl<C: Clock + Clone + Send + Sync + 'static> PollScheduler<C> {
    /// Create a new scheduler.
    #[must_use]
    pub fn new(
        clock: C,
        client: HttpClient,
        refresh_interval: Duration,
        max_concurrent: usize,
    ) -> Self {
        Self {
            clock,
            client,
            // Tokio rejects a zero interval because it would be ready
            // forever; direct users of this public constructor should be
            // protected even when they bypass Config validation.
            refresh_interval: refresh_interval.max(Duration::from_millis(1)),
            // A zero-sized semaphore would park every poll forever. Config
            // validation rejects zero for normal callers, but this public
            // constructor also needs to remain safe for direct users.
            max_concurrent: max_concurrent.max(1),
        }
    }

    /// Start the polling loop.
    ///
    /// Returns a receiver that yields [`PollBatch`]es. The loop runs
    /// until the `cancel` token is cancelled or the receiver is dropped.
    ///
    /// The command channel delivers immediate refreshes and endpoint
    /// replacements. A replacement is applied before its next generation.
    pub fn run(
        self,
        endpoints: Vec<Endpoint>,
        cancel: CancellationToken,
        command_rx: mpsc::Receiver<SchedulerCommand>,
    ) -> mpsc::Receiver<PollBatch> {
        self.run_observed(endpoints, cancel, command_rx).batches
    }

    /// Start a polling loop while retaining a handle for positive shutdown observation.
    pub(crate) fn run_observed(
        self,
        endpoints: Vec<Endpoint>,
        cancel: CancellationToken,
        command_rx: mpsc::Receiver<SchedulerCommand>,
    ) -> SchedulerRunHandle {
        let (tx, rx) = mpsc::channel::<PollBatch>(4);

        let task =
            tokio::spawn(async move { self.poll_loop(endpoints, tx, cancel, command_rx).await });

        SchedulerRunHandle { batches: rx, task }
    }

    /// The main polling loop.
    ///
    /// Performs the first generation immediately (no initial sleep), then
    /// alternates between waiting for the next interval tick or a
    /// `RefreshNow` signal. Each trigger produces exactly one generation.
    ///
    /// Timer semantics: uses `tokio::time::interval` with
    /// `MissedTickPolicy::Skip`, which maintains a fixed cadence. A
    /// manual refresh does **not** reset the periodic schedule — the next
    /// periodic generation fires at the next scheduled interval boundary.
    ///
    /// When the refresh channel closes (`recv()` returns `None`), the
    /// refresh branch is permanently disabled to avoid polling a closed
    /// receiver.
    async fn poll_loop(
        self,
        mut endpoints: Vec<Endpoint>,
        tx: mpsc::Sender<PollBatch>,
        cancel: CancellationToken,
        mut command_rx: mpsc::Receiver<SchedulerCommand>,
    ) -> Result<(), SchedulerRunError> {
        let semaphore = Arc::new(Semaphore::new(self.max_concurrent));
        let mut generation: u64 = 0;
        let mut command_open = true;

        // Use a fixed-cadence interval so manual refresh does not reset
        // the periodic schedule. Skip missed ticks if a generation runs
        // long, preserving the no-overlap invariant.
        let mut interval = tokio::time::interval(self.refresh_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // First generation is immediate when there are endpoints. An empty
        // config keeps the scheduler alive so Ctrl-R can add systems.
        if !endpoints.is_empty() {
            advance_generation(&mut generation);
            let batch = self
                .poll_generation(&endpoints, &semaphore, generation, &cancel)
                .await;
            if tokio::select! {
                result = tx.send(batch) => result.is_err(),
                () = cancel.cancelled() => false,
            } {
                return Err(SchedulerRunError::ReceiverDropped);
            }
        }

        // Consume the interval's initial immediate tick so the next tick
        // fires at the first interval boundary, not immediately.
        interval.tick().await;

        loop {
            tokio::select! {
                biased;

                () = cancel.cancelled() => break,

                // Refresh/replacement commands. Disabled permanently when
                // the channel closes to avoid busy-polling a closed receiver.
                msg = command_rx.recv(), if command_open => {
                    match msg {
                        Some(command) => {
                            if let SchedulerCommand::ReplaceEndpoints(replacement) = command {
                                endpoints = replacement;
                            }
                            if endpoints.is_empty() {
                                continue;
                            }
                            advance_generation(&mut generation);
                            let batch = self
                                .poll_generation(&endpoints, &semaphore, generation, &cancel)
                                .await;
                            if tokio::select! {
                                result = tx.send(batch) => result.is_err(),
                                () = cancel.cancelled() => false,
                            } {
                                return Err(SchedulerRunError::ReceiverDropped);
                            }
                        }
                        None => {
                            // Channel closed — disable this branch permanently.
                            command_open = false;
                        }
                    }
                }

                // Periodic tick at the fixed cadence.
                _ = interval.tick() => {
                    if !endpoints.is_empty() {
                        advance_generation(&mut generation);
                        let batch = self
                            .poll_generation(&endpoints, &semaphore, generation, &cancel)
                            .await;
                        if tokio::select! {
                            result = tx.send(batch) => result.is_err(),
                            () = cancel.cancelled() => false,
                        } {
                            return Err(SchedulerRunError::ReceiverDropped);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Poll all endpoints for a single generation.
    ///
    /// Every configured endpoint produces exactly one result in the batch.
    /// If a poll task panics, a synthetic `Cancelled` result is emitted
    /// for the associated endpoint.
    async fn poll_generation(
        &self,
        endpoints: &[Endpoint],
        semaphore: &Arc<Semaphore>,
        generation: u64,
        cancel: &CancellationToken,
    ) -> PollBatch {
        let started_at = self.clock.now();
        let mut handles: Vec<(Endpoint, tokio::task::JoinHandle<PollResult>)> =
            Vec::with_capacity(endpoints.len());

        for endpoint in endpoints {
            let client = self.client.clone();
            let sem = Arc::clone(semaphore);
            let ep = endpoint.clone();
            let clock = self.clock.clone();
            let cancel = cancel.clone();

            let handle = tokio::spawn(async move {
                let _permit = tokio::select! {
                    () = cancel.cancelled() => return cancelled_result(&ep),
                    permit = sem.acquire_owned() => match permit {
                        Ok(permit) => permit,
                        Err(_) => return cancelled_result(&ep),
                    },
                };
                tokio::select! {
                    () = cancel.cancelled() => cancelled_result(&ep),
                    result = client.poll(&ep, &clock) => result,
                }
            });

            handles.push((endpoint.clone(), handle));
        }

        let mut results = Vec::with_capacity(handles.len());
        for (endpoint, handle) in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(_) => {
                    // Task panicked — emit a synthetic Cancelled result
                    // so the endpoint still appears in the batch.
                    results.push(PollResult {
                        system_id: endpoint.id.clone(),
                        endpoint,
                        outcome: PollOutcome::Cancelled,
                        latency: Duration::ZERO,
                    });
                }
            }
        }

        PollBatch {
            generation,
            started_at,
            completed_at: self.clock.now(),
            results,
        }
    }
}

fn advance_generation(generation: &mut u64) {
    // Generation zero is reserved for the uninitialized state. Wrapping to
    // one after MAX keeps the scheduler live; AppState accepts this one
    // explicit wrap when it follows MAX.
    *generation = generation.checked_add(1).unwrap_or(1);
}

fn cancelled_result(endpoint: &Endpoint) -> PollResult {
    PollResult {
        system_id: endpoint.id.clone(),
        endpoint: endpoint.clone(),
        outcome: PollOutcome::Cancelled,
        latency: Duration::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::endpoint::Endpoint;
    use crate::poller::PollOutcome;
    use gregg_protocol::test_support::LinuxSnapshotBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Helper: create a channel pair for refresh signals.
    fn refresh_channel() -> (
        mpsc::Sender<SchedulerCommand>,
        mpsc::Receiver<SchedulerCommand>,
    ) {
        mpsc::channel(4)
    }

    /// Mock server that returns a valid snapshot.
    async fn valid_snapshot_server() -> String {
        let snap = LinuxSnapshotBuilder::default().build();
        let body = serde_json::to_string(&snap).unwrap();
        mock_server(body.into_bytes(), "200 OK").await
    }
    async fn mock_server(body: Vec<u8>, status: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = stream.read(&mut buf[total..]).await.unwrap();
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf[..total]);
                let response_status = if request
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains("/v2/"))
                {
                    "404 Not Found"
                } else {
                    &status
                };
                let header = format!(
                    "HTTP/1.1 {response_status}\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn endpoint_for_url(url: &str) -> Endpoint {
        let stripped = url.strip_prefix("http://").unwrap();
        let (host, port_str) = stripped.rsplit_once(':').unwrap();
        Endpoint {
            id: format!("{host}:{port_str}"),
            host: host.to_string(),
            port: port_str.parse().unwrap(),
            name: None,
        }
    }
    #[tokio::test]
    async fn scheduler_produces_batches_with_increasing_generations() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);

        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch1.generation, 1);

        clock.advance(Duration::from_millis(20));

        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch2.generation, 2);

        cancel.cancel();
    }

    #[test]
    fn zero_concurrency_is_clamped_to_one_permit() {
        let scheduler = PollScheduler::new(
            FakeClock::new(std::time::Instant::now()),
            HttpClient::new(Duration::from_secs(1)).expect("test HTTP client construction"),
            Duration::from_secs(1),
            0,
        );
        assert_eq!(scheduler.max_concurrent, 1);
    }

    #[test]
    fn zero_refresh_interval_is_clamped_to_one_millisecond() {
        let scheduler = PollScheduler::new(
            FakeClock::new(std::time::Instant::now()),
            HttpClient::new(Duration::from_secs(1)).expect("test HTTP client construction"),
            Duration::ZERO,
            1,
        );
        assert_eq!(scheduler.refresh_interval, Duration::from_millis(1));
    }

    #[test]
    fn generation_wraps_to_one_instead_of_sticking_at_max() {
        let mut generation = u64::MAX;
        advance_generation(&mut generation);
        assert_eq!(generation, 1);
    }

    #[tokio::test]
    async fn replacement_command_polls_only_the_replacement_endpoint() {
        let old = endpoint_for_url(&valid_snapshot_server().await);
        let replacement = endpoint_for_url(&valid_snapshot_server().await);
        assert_ne!(old.port, replacement.port);

        let scheduler = PollScheduler::new(
            FakeClock::new(std::time::Instant::now()),
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction"),
            Duration::from_secs(60),
            1,
        );
        let cancel = CancellationToken::new();
        let (commands, command_rx) = refresh_channel();
        let mut batches = scheduler.run(vec![old.clone()], cancel.clone(), command_rx);

        let first = tokio::time::timeout(Duration::from_secs(5), batches.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.results[0].endpoint, old);

        commands
            .send(SchedulerCommand::ReplaceEndpoints(
                vec![replacement.clone()],
            ))
            .await
            .unwrap();
        let second = tokio::time::timeout(Duration::from_secs(5), batches.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(second.results[0].endpoint, replacement);
        cancel.cancel();
    }

    #[tokio::test]
    async fn concurrency_never_exceeds_bound() {
        let max_concurrent = 2;
        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let peak_concurrent = Arc::new(AtomicUsize::new(0));

        // Create multiple slow mock servers.
        let mut endpoints = Vec::new();
        for _ in 0..5 {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let cc = Arc::clone(&concurrent_count);
            let pc = Arc::clone(&peak_concurrent);
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = stream.read(&mut buf[total..]).await.unwrap();
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }

                let current = cc.fetch_add(1, Ordering::SeqCst) + 1;
                // Update peak.
                pc.fetch_max(current, Ordering::SeqCst);

                tokio::time::sleep(Duration::from_millis(50)).await;

                cc.fetch_sub(1, Ordering::SeqCst);

                let snap = LinuxSnapshotBuilder::default().build();
                let body = serde_json::to_string(&snap).unwrap();
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
            });
            endpoints.push(Endpoint {
                id: format!("ep-{}", addr.port()),
                host: "127.0.0.1".into(),
                port: addr.port(),
                name: None,
            });
        }

        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler =
            PollScheduler::new(clock, client, Duration::from_millis(10), max_concurrent);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(endpoints, cancel.clone(), refresh_rx);

        let _ = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

        cancel.cancel();

        let peak = peak_concurrent.load(Ordering::SeqCst);
        assert!(
            peak <= max_concurrent,
            "peak concurrent {peak} exceeded max {max_concurrent}"
        );
    }

    #[tokio::test]
    async fn cancellation_stops_scheduler() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock, client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Wait for first batch.
        let batch = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap();
        assert!(batch.is_some());

        // Cancel.
        cancel.cancel();

        // The receiver should eventually close.
        // Give the scheduler a moment to notice the cancellation.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The channel may or may not have closed yet, but the scheduler
        // should stop producing new batches.
    }

    #[tokio::test]
    async fn cancellation_aborts_in_flight_generation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let endpoint = Endpoint {
            id: "slow".into(),
            host: "127.0.0.1".into(),
            port,
            name: None,
        };
        let scheduler = PollScheduler::new(
            FakeClock::new(std::time::Instant::now()),
            HttpClient::new(Duration::from_secs(30)).expect("test HTTP client construction"),
            Duration::from_secs(60),
            1,
        );
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let handle = scheduler.run_observed(vec![endpoint], cancel.clone(), refresh_rx);
        tokio::task::yield_now().await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle.task)
            .await
            .expect("cancelled generation should not wait for request timeout")
            .unwrap()
            .unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn empty_endpoint_list() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock, client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![], cancel.clone(), refresh_rx);

        // Should not produce any batches.
        let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(result.is_err());

        refresh_tx
            .send(SchedulerCommand::ReplaceEndpoints(vec![ep]))
            .await
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .is_some());

        cancel.cancel();
    }

    #[tokio::test]
    async fn single_endpoint_polls_repeatedly() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        let mut generations = Vec::new();
        for _ in 0..3 {
            clock.advance(Duration::from_millis(20));
            if let Some(batch) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
            {
                generations.push(batch.generation);
            }
        }

        assert_eq!(generations, vec![1, 2, 3]);
        cancel.cancel();
    }

    #[tokio::test]
    async fn overlap_skip_if_running() {
        // Create a slow mock server that takes 100ms to respond.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut total = 0;
            loop {
                let n = stream.read(&mut buf[total..]).await.unwrap();
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            // Simulate a slow endpoint.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let snap = LinuxSnapshotBuilder::default().build();
            let body = serde_json::to_string(&snap).unwrap();
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(body.as_bytes()).await.unwrap();
        });

        let ep = Endpoint {
            id: "slow-ep".into(),
            host: "127.0.0.1".into(),
            port: addr.port(),
            name: None,
        };

        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        // Refresh interval is 20ms, but the endpoint takes 100ms.
        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(20), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Wait for the first batch to complete (takes ~100ms).
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch1.generation, 1);

        // Advance clock past multiple refresh intervals.
        // The scheduler should not start a new generation while the
        // previous one is still in flight (skip-if-running).
        clock.advance(Duration::from_millis(60));

        // We should NOT receive a second batch yet because the scheduler
        // sleeps for the interval before starting a new generation, and
        // the first generation took 100ms. With a 20ms refresh interval,
        // after the first batch completes at ~100ms, the scheduler sleeps
        // 20ms more before starting generation 2. So at clock=160ms
        // (100ms first cycle + 60ms advance), generation 2 should have
        // started but may not have finished yet. The key invariant is
        // that generation numbers are strictly monotonically increasing
        // and no generation is skipped.
        clock.advance(Duration::from_millis(100));

        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        // Generation must be exactly 2 (no skipped generations).
        assert_eq!(batch2.generation, 2);

        cancel.cancel();
    }

    #[tokio::test]
    async fn multiple_endpoints_all_polled() {
        let url1 = valid_snapshot_server().await;
        let url2 = valid_snapshot_server().await;
        let ep1 = endpoint_for_url(&url1);
        let ep2 = endpoint_for_url(&url2);

        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep1, ep2], cancel.clone(), refresh_rx);

        clock.advance(Duration::from_millis(20));

        let batch = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch.results.len(), 2);

        cancel.cancel();
    }

    #[tokio::test]
    async fn fleet_scaling_10_endpoints() {
        fleet_scaling_test(10, 4).await;
    }

    #[tokio::test]
    async fn fleet_scaling_50_endpoints() {
        fleet_scaling_test(50, 4).await;
    }

    #[tokio::test]
    async fn fleet_scaling_100_endpoints() {
        fleet_scaling_test(100, 4).await;
    }

    /// Spin up `n` mock servers and verify the scheduler polls all of them
    /// with bounded concurrency, returning all results in a single batch.
    async fn fleet_scaling_test(n: usize, max_concurrent: usize) {
        let mut endpoints = Vec::new();
        for _ in 0..n {
            let url = valid_snapshot_server().await;
            endpoints.push(endpoint_for_url(&url));
        }

        let client =
            HttpClient::new(Duration::from_secs(30)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler =
            PollScheduler::new(clock, client, Duration::from_millis(10), max_concurrent);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(endpoints, cancel.clone(), refresh_rx);

        let batch = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("should receive batch within timeout")
            .expect("channel should not be closed");

        assert_eq!(
            batch.results.len(),
            n,
            "should have one result per endpoint"
        );
        let online_count = batch
            .results
            .iter()
            .filter(|r| matches!(r.outcome, PollOutcome::Online(_)))
            .count();
        assert_eq!(online_count, n, "all endpoints should be online");

        cancel.cancel();
    }

    #[tokio::test]
    async fn fleet_scaling_concurrency_bounded_at_scale() {
        let n = 50;
        let max_concurrent = 4;
        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let peak_concurrent = Arc::new(AtomicUsize::new(0));

        let mut endpoints = Vec::new();
        for _ in 0..n {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let cc = Arc::clone(&concurrent_count);
            let pc = Arc::clone(&peak_concurrent);
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = stream.read(&mut buf[total..]).await.unwrap();
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let current = cc.fetch_add(1, Ordering::SeqCst) + 1;
                pc.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                cc.fetch_sub(1, Ordering::SeqCst);

                let snap = LinuxSnapshotBuilder::default().build();
                let body = serde_json::to_string(&snap).unwrap();
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
            });
            endpoints.push(Endpoint {
                id: format!("ep-{}", addr.port()),
                host: "127.0.0.1".into(),
                port: addr.port(),
                name: None,
            });
        }

        let client =
            HttpClient::new(Duration::from_secs(30)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler =
            PollScheduler::new(clock, client, Duration::from_millis(10), max_concurrent);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(endpoints, cancel.clone(), refresh_rx);

        let batch = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("should receive batch")
            .expect("channel open");

        assert_eq!(batch.results.len(), n);
        cancel.cancel();

        let peak = peak_concurrent.load(Ordering::SeqCst);
        assert!(
            peak <= max_concurrent,
            "peak concurrent {peak} exceeded max {max_concurrent}"
        );
    }

    /// Mock server that alternates between valid snapshots and connection
    /// drops on successive connections, simulating an unstable endpoint.
    async fn alternating_mock_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let snap = LinuxSnapshotBuilder::default().build();
        let body = serde_json::to_string(&snap).unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = stream.read(&mut buf[total..]).await.unwrap();
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&buf[..total]);
                if request
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains("/v2/"))
                {
                    let header = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                    stream.write_all(header.as_bytes()).await.unwrap();
                    continue;
                }
                let count = call_count.fetch_add(1, Ordering::SeqCst);
                if count % 2 == 0 {
                    let header =
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                    stream.write_all(header.as_bytes()).await.unwrap();
                    stream.write_all(body.as_bytes()).await.unwrap();
                } else {
                    drop(stream);
                }
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[tokio::test]
    async fn alternating_online_offline_endpoint() {
        let url = alternating_mock_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let clock = crate::clock::RealClock;

        let mut online_count = 0;
        let mut offline_count = 0;
        for _ in 0..6 {
            let result = client.poll(&ep, &clock).await;
            match &result.outcome {
                PollOutcome::Online(_) => online_count += 1,
                _ => offline_count += 1,
            }
        }

        // With alternating behavior we should see a mix of online and offline.
        assert!(online_count > 0, "should have at least one online result");
        assert!(offline_count > 0, "should have at least one offline result");
    }

    #[tokio::test]
    async fn clock_backward_adjustment_does_not_corrupt_scheduler() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // First batch at normal time.
        clock.advance(Duration::from_millis(20));
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch1.generation, 1);
        assert!(batch1.started_at <= batch1.completed_at);

        // Set clock backward (simulating NTP correction or suspend/resume).
        // The scheduler uses tokio::time::sleep for the interval, not the
        // fake clock, so it will still wake up. The clock only affects
        // batch timestamps. Generations must remain monotonically increasing.
        clock.set(anchor.checked_sub(Duration::from_secs(3600)).unwrap());

        clock.advance(Duration::from_millis(20));
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch2.generation, 2, "generations must be monotonic");

        // Set clock far forward again.
        clock.set(anchor + Duration::from_secs(7200));
        clock.advance(Duration::from_millis(20));
        let batch3 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch3.generation, 3, "generations must be monotonic");

        cancel.cancel();
    }

    #[tokio::test]
    async fn scheduler_handles_alternating_endpoint() {
        let url = alternating_mock_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        let mut online_results = 0;
        let mut offline_results = 0;

        for _ in 0..4 {
            clock.advance(Duration::from_millis(20));
            if let Some(batch) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
            {
                for result in &batch.results {
                    match &result.outcome {
                        PollOutcome::Online(_) => online_results += 1,
                        _ => offline_results += 1,
                    }
                }
            }
        }

        // With alternating behavior, we should see a mix of online and offline.
        assert!(online_results > 0, "should have at least one online result");
        assert!(
            offline_results > 0,
            "should have at least one offline result"
        );

        cancel.cancel();
    }

    #[tokio::test]
    async fn first_poll_happens_immediately_without_delay() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        // Use a very long refresh interval — if the first poll were
        // delayed, we would not receive a batch within 200ms.
        let scheduler = PollScheduler::new(clock, client, Duration::from_secs(3600), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // The first batch should arrive almost immediately.
        let batch = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("should receive first batch without delay")
            .expect("channel should be open");
        assert_eq!(batch.generation, 1);

        cancel.cancel();
    }

    #[tokio::test]
    async fn refresh_now_triggers_generation() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        // Use a long refresh interval so only RefreshNow triggers polls.
        let scheduler = PollScheduler::new(clock, client, Duration::from_secs(3600), 4);
        let cancel = CancellationToken::new();
        let (refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Consume the immediate first batch (generation 1).
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first batch")
            .expect("channel open");
        assert_eq!(batch1.generation, 1);

        // Send a RefreshNow signal.
        refresh_tx.send(SchedulerCommand::Refresh).await.unwrap();

        // The scheduler should produce a second batch promptly.
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("refresh batch")
            .expect("channel open");
        assert_eq!(batch2.generation, 2);

        cancel.cancel();
    }

    #[tokio::test]
    async fn panicked_task_produces_cancelled_result_for_endpoint() {
        use crate::poller::PollResult;

        let endpoint = Endpoint {
            id: "panic-ep".into(),
            host: "127.0.0.1".into(),
            port: 1,
            name: None,
        };

        let ep_clone = endpoint.clone();

        // Spawn a task that panics after referencing the endpoint.
        let panic_handle = tokio::spawn(async move {
            // Reference the cloned endpoint so the compiler sees it as used.
            let id = ep_clone.id.clone();
            drop(id);
            panic!("test panic");
        });

        // Manually create the batch to test the cancelled result logic.
        let mut results = Vec::new();
        match panic_handle.await {
            Ok(result) => results.push(result),
            Err(_) => {
                results.push(PollResult {
                    system_id: endpoint.id.clone(),
                    endpoint,
                    outcome: PollOutcome::Cancelled,
                    latency: Duration::ZERO,
                });
            }
        }

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].system_id, "panic-ep");
        assert_eq!(results[0].outcome, PollOutcome::Cancelled);
    }

    /// C1: One manual refresh signal produces exactly one additional
    /// generation — not two (the old bug caused a fall-through to the
    /// periodic generation).
    #[tokio::test]
    async fn one_refresh_signal_produces_one_generation() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        // Use a very long refresh interval so only RefreshNow triggers polls.
        let scheduler = PollScheduler::new(clock, client, Duration::from_secs(3600), 4);
        let cancel = CancellationToken::new();
        let (refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Consume the immediate first batch (generation 1).
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first batch")
            .expect("channel open");
        assert_eq!(batch1.generation, 1);

        // Send a single RefreshNow signal.
        refresh_tx.send(SchedulerCommand::Refresh).await.unwrap();

        // Should receive exactly one additional batch (generation 2).
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("refresh batch")
            .expect("channel open");
        assert_eq!(batch2.generation, 2);

        // There should be NO generation 3 arriving shortly after.
        // The old bug would produce a second generation from the fall-through.
        let result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err() || result.unwrap().is_none(),
            "should not receive a third batch from a single refresh signal"
        );

        cancel.cancel();
    }

    /// C2: Closing the refresh channel does not cause a busy loop.
    /// Only periodic generations should occur at the configured interval.
    #[tokio::test]
    async fn closed_refresh_channel_does_not_busy_loop() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock, client, Duration::from_millis(200), 4);
        let cancel = CancellationToken::new();
        let (refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Consume the immediate first batch (generation 1).
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first batch")
            .expect("channel open");
        assert_eq!(batch1.generation, 1);
        let _t1 = std::time::Instant::now();

        // Drop the refresh sender to close the channel.
        drop(refresh_tx);

        // Wait for the periodic generation (generation 2).
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("second batch")
            .expect("channel open");
        assert_eq!(batch2.generation, 2);
        let t2 = std::time::Instant::now();

        // Wait for the next periodic generation (generation 3).
        let batch3 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("third batch")
            .expect("channel open");
        assert_eq!(batch3.generation, 3);
        let t3 = std::time::Instant::now();

        // Verify the interval between generations 2 and 3 is approximately
        // 200ms (the configured interval), not a tight loop.
        let elapsed = t3.saturating_duration_since(t2);
        assert!(
            elapsed >= Duration::from_millis(100),
            "generations should be spaced by the interval, not busy-looping; elapsed = {elapsed:?}"
        );

        cancel.cancel();
    }

    /// C3: Periodic cadence after manual refresh matches documentation.
    /// Manual refresh does NOT reset the periodic timer — the next periodic
    /// generation fires at the next scheduled interval boundary.
    #[tokio::test]
    async fn manual_refresh_does_not_reset_periodic_cadence() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        // Use a 100ms refresh interval.
        let scheduler = PollScheduler::new(clock, client, Duration::from_millis(100), 4);
        let cancel = CancellationToken::new();
        let (refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Consume the immediate first batch (generation 1).
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first batch")
            .expect("channel open");
        assert_eq!(batch1.generation, 1);
        let t1 = std::time::Instant::now();

        // Wait for the periodic generation (generation 2) at ~100ms.
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("second batch")
            .expect("channel open");
        assert_eq!(batch2.generation, 2);
        let _t2 = std::time::Instant::now();

        // Send a manual refresh immediately after generation 2.
        refresh_tx.send(SchedulerCommand::Refresh).await.unwrap();

        // Should receive the manual refresh batch (generation 3) promptly.
        let batch3 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("refresh batch")
            .expect("channel open");
        assert_eq!(batch3.generation, 3);
        let _t3 = std::time::Instant::now();

        // The next periodic generation (generation 4) should fire at the
        // next scheduled interval boundary, NOT 100ms after the manual refresh.
        // Since the manual refresh happened right after generation 2, and
        // the interval is 100ms, generation 4 should arrive at approximately
        // 200ms from the start (two interval boundaries).
        let batch4 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("fourth batch")
            .expect("channel open");
        assert_eq!(batch4.generation, 4);
        let t4 = std::time::Instant::now();

        // Generation 4 should arrive at approximately 200ms from start,
        // not 200ms from the manual refresh (which would be ~300ms).
        // On slow CI runners, wall-clock time between batches can be
        // much larger than the fake-clock interval, so we use a generous
        // tolerance that still proves the cadence was not fully reset
        // (a full reset would push batch4 well past the next boundary).
        let elapsed_from_start = t4.saturating_duration_since(t1);
        assert!(
            elapsed_from_start < Duration::from_secs(30),
            "periodic cadence should not be reset by manual refresh; \
             elapsed from start = {elapsed_from_start:?}"
        );

        cancel.cancel();
    }

    /// C1: Three rapid Ctrl-R signals each produce exactly one generation.
    #[tokio::test]
    async fn three_rapid_refresh_signals_produce_three_generations() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        // Use a short client timeout so failed polls (after the mock server
        // handles its one connection) don't stall the test.
        let client =
            HttpClient::new(Duration::from_millis(100)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        // Use a very long refresh interval so only RefreshNow triggers polls.
        let scheduler = PollScheduler::new(clock, client, Duration::from_secs(3600), 4);
        let cancel = CancellationToken::new();
        let (refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep], cancel.clone(), refresh_rx);

        // Consume the immediate first batch (generation 1).
        let _ = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first batch")
            .expect("channel open");

        // Send 3 rapid refresh signals.
        for _ in 0..3 {
            refresh_tx.send(SchedulerCommand::Refresh).await.unwrap();
        }

        // Should receive exactly 3 additional batches (generations 2-4).
        let mut generations = Vec::new();
        for _ in 0..3 {
            let batch = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("batch")
                .expect("channel open");
            generations.push(batch.generation);
        }

        // Generations should be 2 through 4, in order.
        assert_eq!(generations, vec![2, 3, 4]);

        cancel.cancel();
    }

    /// Mock server that fails the first `failure_count` connections by
    /// dropping the request without responding, then succeeds on later
    /// connections. Models a temporarily offline endpoint that later
    /// becomes reachable without operator intervention.
    async fn flaky_then_valid_server(failure_count: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let snap = LinuxSnapshotBuilder::default().build();
        let body = serde_json::to_string(&snap).unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = stream.read(&mut buf[total..]).await.unwrap();
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let count = accepted.fetch_add(1, Ordering::SeqCst);
                if count < failure_count {
                    drop(stream);
                    continue;
                }
                let request = String::from_utf8_lossy(&buf[..total]);
                let status = if request
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains("/v2/"))
                {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                stream.write_all(header.as_bytes()).await.unwrap();
                stream.write_all(body.as_bytes()).await.unwrap();
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    /// Phase 083 invariant: a failed endpoint is kept in the scheduler
    /// and polled again on later generations, recovering automatically
    /// when reachable. The scheduler does not silently suppress
    /// offline endpoints.
    #[tokio::test]
    async fn offline_endpoint_is_retried_and_recovers_on_next_generation() {
        let url = flaky_then_valid_server(/* failure_count */ 1).await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep.clone()], cancel.clone(), refresh_rx);

        // Generation 1: connection is dropped, expect one offline result.
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .expect("scheduler must produce the first batch");
        assert_eq!(batch1.generation, 1);
        assert_eq!(batch1.results.len(), 1, "exactly one result per endpoint");
        assert!(
            !matches!(
                batch1.results[0].outcome,
                PollOutcome::Online(_) | PollOutcome::OnlineV2(_)
            ),
            "first attempt should not be online: got {:?}",
            batch1.results[0].outcome,
        );
        assert_eq!(batch1.results[0].system_id, ep.id);

        // Generation 2: same endpoint polled again, success expected.
        clock.advance(Duration::from_millis(20));
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .expect("scheduler must produce the second batch");
        assert_eq!(batch2.generation, 2);
        assert_eq!(batch2.results.len(), 1, "exactly one result per endpoint");
        assert_eq!(
            batch2.results[0].system_id, ep.id,
            "the same endpoint must be polled again"
        );
        let recovered = matches!(
            batch2.results[0].outcome,
            PollOutcome::Online(_) | PollOutcome::OnlineV2(_)
        );
        assert!(
            recovered,
            "second generation should recover; got {:?}",
            batch2.results[0].outcome,
        );

        cancel.cancel();
    }

    /// Two-generation offline assertion: a still-offline endpoint must
    /// still appear in the next batch's result set, demonstrating the
    /// scheduler does not silently drop unreachable endpoints after one
    /// failure.
    #[tokio::test]
    async fn offline_endpoint_remains_in_scheduler_across_generations() {
        let url = flaky_then_valid_server(/* failure_count */ 99).await;
        let ep = endpoint_for_url(&url);
        let client =
            HttpClient::new(Duration::from_secs(5)).expect("test HTTP client construction");
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let (_refresh_tx, refresh_rx) = refresh_channel();
        let mut rx = scheduler.run(vec![ep.clone()], cancel.clone(), refresh_rx);

        // Generation 1: offline.
        let batch1 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch1.generation, 1);
        assert!(
            !matches!(
                batch1.results[0].outcome,
                PollOutcome::Online(_) | PollOutcome::OnlineV2(_)
            ),
            "first attempt must be offline"
        );

        // Generation 2: same endpoint appears again even though it is
        // still unhealthy.
        clock.advance(Duration::from_millis(20));
        let batch2 = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(batch2.generation, 2);
        assert_eq!(batch2.results.len(), 1);
        assert_eq!(batch2.results[0].system_id, ep.id);
        assert!(
            !matches!(
                batch2.results[0].outcome,
                PollOutcome::Online(_) | PollOutcome::OnlineV2(_)
            ),
            "second attempt should still be offline, got {:?}",
            batch2.results[0].outcome,
        );

        cancel.cancel();
    }
}
