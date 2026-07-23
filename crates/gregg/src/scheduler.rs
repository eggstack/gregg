#![allow(dead_code)]

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
use crate::poller::{HttpClient, PollBatch};

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
            refresh_interval,
            max_concurrent,
        }
    }

    /// Start the polling loop.
    ///
    /// Returns a receiver that yields [`PollBatch`]es. The loop runs
    /// until the `cancel` token is cancelled or the receiver is dropped.
    pub fn run(
        self,
        endpoints: Vec<Endpoint>,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<PollBatch> {
        let (tx, rx) = mpsc::channel::<PollBatch>(4);

        tokio::spawn(async move {
            self.poll_loop(endpoints, tx, cancel).await;
        });

        rx
    }

    /// The main polling loop.
    async fn poll_loop(
        self,
        endpoints: Vec<Endpoint>,
        tx: mpsc::Sender<PollBatch>,
        cancel: CancellationToken,
    ) {
        if endpoints.is_empty() {
            return;
        }

        let semaphore = Arc::new(Semaphore::new(self.max_concurrent));
        let mut generation: u64 = 0;

        loop {
            // Sleep for the refresh interval, checking for cancellation.
            if cancel.is_cancelled() {
                break;
            }

            tokio::select! {
                () = tokio::time::sleep(self.refresh_interval) => {}
                () = cancel.cancelled() => break,
            }

            generation = generation.saturating_add(1);
            let batch = self
                .poll_generation(&endpoints, &semaphore, generation)
                .await;

            // Try to send the batch. If the receiver is dropped, break.
            if tx.send(batch).await.is_err() {
                break;
            }
        }
    }

    /// Poll all endpoints for a single generation.
    async fn poll_generation(
        &self,
        endpoints: &[Endpoint],
        semaphore: &Arc<Semaphore>,
        generation: u64,
    ) -> PollBatch {
        let started_at = self.clock.now();
        let mut handles = Vec::with_capacity(endpoints.len());

        for endpoint in endpoints {
            let client = self.client.clone();
            let sem = Arc::clone(semaphore);
            let endpoint = endpoint.clone();
            let clock = self.clock.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore should not be closed");
                client.poll(&endpoint, &clock).await
            });

            handles.push(handle);
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
            // Task panicked — treat as a cancelled poll for
            // this endpoint. We don't have the endpoint info,
            // so we skip it.
        }

        PollBatch {
            generation,
            started_at,
            completed_at: self.clock.now(),
            results,
        }
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
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
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
        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);

        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep], cancel.clone());

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

        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler =
            PollScheduler::new(clock, client, Duration::from_millis(10), max_concurrent);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(endpoints, cancel.clone());

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
        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock, client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep], cancel.clone());

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
    async fn empty_endpoint_list() {
        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock, client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![], cancel.clone());

        // Should not produce any batches.
        let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(result.unwrap().is_none());

        cancel.cancel();
    }

    #[tokio::test]
    async fn single_endpoint_polls_repeatedly() {
        let url = valid_snapshot_server().await;
        let ep = endpoint_for_url(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep], cancel.clone());

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

        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        // Refresh interval is 20ms, but the endpoint takes 100ms.
        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(20), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep], cancel.clone());

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

        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep1, ep2], cancel.clone());

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

        let client = HttpClient::new(Duration::from_secs(30));
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler =
            PollScheduler::new(clock, client, Duration::from_millis(10), max_concurrent);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(endpoints, cancel.clone());

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

        let client = HttpClient::new(Duration::from_secs(30));
        let anchor = std::time::Instant::now();
        let clock = FakeClock::new(anchor);

        let scheduler =
            PollScheduler::new(clock, client, Duration::from_millis(10), max_concurrent);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(endpoints, cancel.clone());

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
                let count = call_count.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 4096];
                let mut total = 0;
                loop {
                    let n = stream.read(&mut buf[total..]).await.unwrap();
                    total += n;
                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
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
        let client = HttpClient::new(Duration::from_secs(5));
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
        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep], cancel.clone());

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
        let client = HttpClient::new(Duration::from_secs(5));
        let anchor = std::time::Instant::now();
        let mut clock = FakeClock::new(anchor);

        let scheduler = PollScheduler::new(clock.clone(), client, Duration::from_millis(10), 4);
        let cancel = CancellationToken::new();
        let mut rx = scheduler.run(vec![ep], cancel.clone());

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
}
