//! Plan 166: the client daemon's scheduler-polling plane.
//!
//! This is the *only* place Gregg fetches the Plan-162 scheduler routes, and it
//! lives in the client daemon for the same reason metrics polling does: a
//! per-TUI poller would multiply the fleet's request budget by the number of
//! open windows, and closing one window would silently stop a fraction of what
//! the operator is watching.
//!
//! # Two planes, deliberately unequal
//!
//! - The **summary** (`/v2/scheduler`) is small, carries no command output, and
//!   is read on a bounded cadence. It is what the at-a-glance job rows need.
//! - The **history** (`/v2/scheduler/history`) carries bounded stdout/stderr
//!   tails, so it is fetched only on first support discovery and when the
//!   summary's `history_revision` changes. Downloading it on every metrics poll
//!   would put the largest response in the system on the hot path to draw five
//!   job rows.
//!
//! # Scheduler failure is not reachability failure
//!
//! Every outcome here is scoped to the scheduler routes. An old `greggd` is
//! `Unsupported`, a flapping network is `Failed`, and neither is ever allowed to
//! become "this system is offline" — the metrics plane owns that fact, and a
//! healthy daemon with no cron jobs must not be reported as down.
//!
//! # Why there is no log or config fallback
//!
//! The plan rules it out and the reason is sound: parsing a remote's scheduler
//! logs or configuration files would be a second, undocumented contract with no
//! version negotiation and no way to tell a stale file from a live one. If the
//! route is absent, the capability is absent.

use std::time::Duration;

use gregg_protocol::{SchedulerHistoryV2, SchedulerSummaryV2};

use crate::cron::{CronCache, CronFetchError};
use crate::endpoint::{Endpoint, EndpointError};

/// Map a transport failure onto a scheduler error.
///
/// A body over the route's own cap is reported as such rather than as a
/// generic transport failure: "the daemon sent more than its contract allows"
/// is a real daemon problem, and hiding it behind "connection error" would send
/// the operator looking in the wrong place.
fn classify_failure(failure: &eggfetch_core::RequestFailure) -> CronFetchError {
    if matches!(
        failure.error(),
        eggfetch_core::Error::DecodedBodyTooLarge
    ) {
        return CronFetchError::BodyTooLarge;
    }
    let message = if failure.is_timeout() {
        "the scheduler request timed out".to_owned()
    } else {
        format!("could not reach the scheduler route: {}", failure.error())
    };
    CronFetchError::Transport(message)
}

/// How often the scheduler summary is read per endpoint.
///
/// Deliberately slower than the metrics cadence. A load-delayed transition is
/// visible on the order of a minute, which is fast enough for an operator
/// watching Gregg and slow enough that scheduler observability does not
/// double a five-second fleet's request count.
pub const CRON_SUMMARY_INTERVAL: Duration = Duration::from_secs(30);

/// Construct the scheduler summary URL for an endpoint.
///
/// IPv6 hosts are bracketed per RFC 2732.
///
/// # Errors
///
/// Returns [`EndpointError`] when `host` cannot be normalized.
pub fn scheduler_url(host: &str, port: u16) -> Result<String, EndpointError> {
    crate::poller::bracketed_host(host).map(|host| format!("http://{host}:{port}/v2/scheduler"))
}

/// Construct the scheduler history URL for an endpoint.
///
/// # Errors
///
/// Returns [`EndpointError`] when `host` cannot be normalized.
pub fn scheduler_history_url(host: &str, port: u16) -> Result<String, EndpointError> {
    crate::poller::bracketed_host(host)
        .map(|host| format!("http://{host}:{port}/v2/scheduler/history"))
}

/// What one summary read told us.
///
/// `Unsupported` is deliberately not a [`CronFetchError`]: a pre-scheduler
/// `greggd` is a healthy daemon, and reporting it as a failure would put a
/// permanent "error" badge on every old system in a mixed fleet.
#[derive(Debug, Clone, PartialEq)]
pub enum SummaryOutcome {
    /// The remote serves the routes and produced a valid summary.
    Supported(Box<SchedulerSummaryV2>),
    /// The remote answered but does not serve `/v2/scheduler`.
    Unsupported,
    /// The read failed; retained data stays usable but is not known to be current.
    Failed(CronFetchError),
}

/// HTTP client for the two scheduler routes.
///
/// Separate from [`crate::poller::HttpClient`] because the body caps differ by
/// two orders of magnitude, and because mixing the two documents in one type
/// would invite a caller to use the wrong cap. The per-route caps come from
/// `gregg-protocol`, so the client cannot drift from the wire contract.
#[derive(Clone)]
pub struct CronClient {
    client: eggfetch_core::Client,
    timeout: Duration,
}

impl CronClient {
    /// Create a client with the same whole-request deadline shape the metrics
    /// poller uses.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let client = eggfetch_core::Client::builder()
            .timeout(eggfetch_core::Timeout {
                pool: Some(timeout),
                connect: Some(timeout),
                write: Some(timeout),
                read: Some(timeout),
                total: Some(timeout),
            })
            .max_idle_connections_per_host(2)
            .max_decoded_body_size(gregg_protocol::MAX_SCHEDULER_SUMMARY_BODY_BYTES)
            .build();
        Self { client, timeout }
    }

    /// The configured whole-request deadline.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Read `/v2/scheduler` for one endpoint.
    pub async fn summary(&self, endpoint: &Endpoint) -> SummaryOutcome {
        let url = match scheduler_url(&endpoint.host, endpoint.port) {
            Ok(url) => url,
            Err(error) => {
                return SummaryOutcome::Failed(CronFetchError::Transport(format!(
                    "could not build a scheduler URL: {error}"
                )))
            }
        };
        let cap = gregg_protocol::MAX_SCHEDULER_SUMMARY_BODY_BYTES;
        match self.get_json(&url, cap).await {
            Ok(Some(body)) => match serde_json::from_slice::<SchedulerSummaryV2>(&body) {
                Ok(summary) => {
                    if summary.validate().is_err() {
                        // A document that fails its own contract is a different
                        // problem from a flaky network, and conflating them
                        // hides a real daemon bug.
                        SummaryOutcome::Failed(CronFetchError::Invalid(
                            "the scheduler summary failed its own wire validation".to_owned(),
                        ))
                    } else {
                        SummaryOutcome::Supported(Box::new(summary))
                    }
                }
                Err(error) => SummaryOutcome::Failed(CronFetchError::Invalid(error.to_string())),
            },
            // A 404 is the documented "old daemon" signal, not a failure.
            Ok(None) => SummaryOutcome::Unsupported,
            Err(error) => SummaryOutcome::Failed(error),
        }
    }

    /// Read `/v2/scheduler/history` for one endpoint.
    ///
    /// Returns `Ok(None)` for a 404 so the caller can distinguish "this daemon
    /// has no scheduler" from "this read failed", which is the same distinction
    /// the summary outcome makes.
    pub async fn history(
        &self,
        endpoint: &Endpoint,
    ) -> Result<Option<SchedulerHistoryV2>, CronFetchError> {
        let url = scheduler_history_url(&endpoint.host, endpoint.port).map_err(|error| {
            CronFetchError::Transport(format!("could not build a scheduler URL: {error}"))
        })?;
        let cap = gregg_protocol::MAX_SCHEDULER_HISTORY_BODY_BYTES;
        let Some(body) = self.get_json(&url, cap).await? else {
            return Ok(None);
        };
        let history: SchedulerHistoryV2 = serde_json::from_slice(&body)
            .map_err(|error| CronFetchError::Invalid(error.to_string()))?;
        if history.validate().is_err() {
            return Err(CronFetchError::Invalid(
                "the scheduler history failed its own wire validation".to_owned(),
            ));
        }
        Ok(Some(history))
    }

    /// GET one JSON body, or `None` for a 404.
    async fn get_json(&self, url: &str, cap: usize) -> Result<Option<Vec<u8>>, CronFetchError> {
        let builder = self
            .client
            .get(url)
            .map_err(|error| CronFetchError::Transport(error.to_string()))?
            .max_decoded_body_size(cap);
        let mut response = builder
            .send_detailed()
            .await
            .map_err(|failure| classify_failure(&failure))?;
        if response.status() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(CronFetchError::Transport(format!(
                "the scheduler route answered HTTP {}",
                response.status().as_u16()
            )));
        }
        // `bytes()` surfaces the decoded-body limit as an ordinary error, so
        // the cap is enforced by the transport rather than by a second
        // accumulation loop here.
        response
            .bytes()
            .await
            .map(|body| Some(body.to_vec()))
            .map_err(|error| {
                if matches!(error, eggfetch_core::Error::DecodedBodyTooLarge) {
                    CronFetchError::BodyTooLarge
                } else {
                    CronFetchError::Transport(error.to_string())
                }
            })
    }
}

/// One round of scheduler observation for a single endpoint.
///
/// The two-plane discipline lives here rather than in the caller: the history
/// fetch is gated on `needs_history`, so it is structurally impossible for a
/// steady-state metrics cadence to download the history body.
#[derive(Debug)]
pub struct CronObservation {
    /// Stable system id the observation belongs to.
    pub system_id: String,
    /// What the summary read produced.
    pub summary: SummaryOutcome,
    /// Records added by this round, if a history fetch happened.
    pub records_added: usize,
}

/// Run one bounded observation for one endpoint against the shared cache.
///
/// Returns whether the cache changed, so the caller publishes only on a real
/// change. A scheduler failure is recorded but never erases retained data, and
/// never touches system reachability.
pub async fn observe(
    client: &CronClient,
    cache: &mut CronCache,
    endpoint: &Endpoint,
    now_unix_ms: u64,
) -> bool {
    let system_id = endpoint.id.clone();
    let outcome = client.summary(endpoint).await;

    let wants_history = match &outcome {
        SummaryOutcome::Supported(summary) => cache
            .system(&system_id)
            .is_none_or(|state| state.needs_history(summary)),
        // Nothing to fetch history for: the remote has no scheduler, or the
        // summary failed and the last known revision still governs.
        SummaryOutcome::Unsupported | SummaryOutcome::Failed(_) => false,
    };

    let mut changed = false;
    match outcome {
        SummaryOutcome::Supported(summary) => {
            let revision = summary.history_revision;
            let epoch = summary.epoch;
            // Compare before overwrite: the fetch decision above was made
            // against the pre-write state.
            let before = cache
                .system(&system_id)
                .and_then(|state| state.history_revision)
                .is_some_and(|previous| previous == revision)
                && cache
                    .system(&system_id)
                    .and_then(|state| state.epoch)
                    .is_some_and(|previous| previous == epoch);
            cache.apply_summary(&system_id, *summary, now_unix_ms);
            changed |= !before;

            if wants_history {
                match client.history(endpoint).await {
                    Ok(Some(history)) => {
                        changed |= cache.apply_history(&system_id, &history);
                    }
                    Ok(None) => {
                        // The summary said supported and the history route is
                        // absent. That is a real inconsistency in the remote, so
                        // it is reported rather than silently ignored.
                        let state = cache.system_mut(&system_id);
                        state.mark_failed(
                            CronFetchError::Transport(
                                "the scheduler summary is served but the history route is not"
                                    .to_owned(),
                            ),
                            now_unix_ms,
                        );
                        changed = true;
                    }
                    Err(error) => {
                        let state = cache.system_mut(&system_id);
                        state.mark_failed(error, now_unix_ms);
                        changed = true;
                    }
                }
            }
        }
        SummaryOutcome::Unsupported => {
            let state = cache.system_mut(&system_id);
            let was = state.capability != crate::cron::CronCapability::Unsupported;
            state.mark_unsupported();
            changed |= was;
            // `last_attempt_at` still moves so the renderer can say the answer
            // is current rather than implying it was never asked.
            state.last_attempt_at_unix_ms = Some(now_unix_ms);
        }
        SummaryOutcome::Failed(error) => {
            let state = cache.system_mut(&system_id);
            let was = state.last_error.as_ref() != Some(&error);
            state.mark_failed(error, now_unix_ms);
            changed |= was;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg_protocol::{
        SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobV2,
        SchedulerOutcomeV2, SchedulerOutputV2, SchedulerRunRecordV2, SchedulerRunSummaryV2,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn endpoint(port: u16) -> Endpoint {
        Endpoint {
            id: "sys".to_owned(),
            host: "127.0.0.1".to_owned(),
            port,
            name: None,
        }
    }

    fn epoch() -> SchedulerEpochV2 {
        SchedulerEpochV2 {
            started_at_unix_ms: 1_000,
            nonce: 7,
        }
    }

    fn summary_body(revision: u64) -> Vec<u8> {
        let summary = SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch: epoch(),
            history_revision: revision,
            jobs: vec![SchedulerJobV2 {
                name: "backup".to_owned(),
                schedule: "0 3 * * *".to_owned(),
                next_due_unix_ms: 1_700_100_000_000,
                state: gregg_protocol::SchedulerJobStateV2::Idle,
                load: None,
                pending_since_unix_ms: None,
                next_retry_unix_ms: None,
                running_since_unix_ms: None,
                last: Some(SchedulerRunSummaryV2 {
                    sequence: 1,
                    scheduled_unix_ms: 1_700_000_000_000,
                    finished_unix_ms: 1_700_000_001_000,
                    outcome: SchedulerOutcomeV2::Success,
                    exit_code: Some(0),
                    signal: None,
                    duration_ms: Some(1_000),
                    delay_ms: 0,
                    coalesced: false,
                }),
            }],
        };
        serde_json::to_vec(&summary).expect("serializes")
    }

    fn history_body(revision: u64, sequences: &[u64]) -> Vec<u8> {
        let history = SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch: epoch(),
            history_revision: revision,
            jobs: vec![SchedulerJobHistoryV2 {
                name: "backup".to_owned(),
                records: sequences
                    .iter()
                    .map(|sequence| SchedulerRunRecordV2 {
                        sequence: *sequence,
                        scheduled_unix_ms: 1_700_000_000_000,
                        started_unix_ms: Some(1_700_000_000_100),
                        finished_unix_ms: 1_700_000_001_000 + *sequence,
                        outcome: SchedulerOutcomeV2::Success,
                        exit_code: Some(0),
                        signal: None,
                        duration_ms: Some(900),
                        delay_ms: 0,
                        coalesced: false,
                        stdout: SchedulerOutputV2::new("ok\n".to_owned(), false),
                        stderr: SchedulerOutputV2::new(String::new(), false),
                    })
                    .collect(),
            }],
        };
        serde_json::to_vec(&history).expect("serializes")
    }

    /// What a loopback greggd should answer, and how many times.
    #[derive(Clone, Default)]
    struct Routes {
        summary: Option<(u16, Vec<u8>)>,
        history: Option<(u16, Vec<u8>)>,
        summary_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        history_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn serve(routes: Routes) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let summary_hits = std::sync::Arc::clone(&routes.summary_hits);
        let history_hits = std::sync::Arc::clone(&routes.history_hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let summary = routes.summary.clone();
                let history = routes.history.clone();
                let summary_hits = std::sync::Arc::clone(&summary_hits);
                let history_hits = std::sync::Arc::clone(&history_hits);
                tokio::spawn(async move {
                    let mut request = String::new();
                    let mut chunk = [0_u8; 1024];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => {
                                request.push_str(
                                    String::from_utf8_lossy(&chunk[..count]).as_ref(),
                                );
                                if request.contains("\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let is_history = request.contains("/v2/scheduler/history");
                    let route = if is_history { history } else { summary };
                    let hits = if is_history { history_hits } else { summary_hits };
                    hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let response = match route {
                        Some((status, body)) => {
                            let head = format!(
                                "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            format!("{head}{}", String::from_utf8_lossy(&body))
                        }
                        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_owned(),
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });
        port
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn urls_bracket_ipv6_and_use_the_planned_paths() {
        assert_eq!(
            scheduler_url("box", 11310).expect("url"),
            "http://box:11310/v2/scheduler"
        );
        assert_eq!(
            scheduler_history_url("box", 11310).expect("url"),
            "http://box:11310/v2/scheduler/history"
        );
        assert_eq!(
            scheduler_url("::1", 11310).expect("url"),
            "http://[::1]:11310/v2/scheduler"
        );
        assert_eq!(
            scheduler_history_url("::1", 11310).expect("url"),
            "http://[::1]:11310/v2/scheduler/history"
        );
    }

    #[test]
    fn an_old_daemon_is_unsupported_rather_than_a_failure() {
        runtime().block_on(async {
            let port = serve(Routes::default()).await;
            let client = CronClient::new(Duration::from_secs(2));
            let outcome = client.summary(&endpoint(port)).await;
            assert_eq!(outcome, SummaryOutcome::Unsupported);
        });
    }

    #[test]
    fn an_unchanged_revision_suppresses_the_history_body_entirely() {
        runtime().block_on(async {
            let routes = Routes {
                summary: Some((200, summary_body(4))),
                history: Some((200, history_body(4, &[1, 2]))),
                ..Routes::default()
            };
            let port = serve(routes.clone()).await;
            let client = CronClient::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(port);

            // Discovery: one summary, one history.
            observe(&client, &mut cache, &target, 1_700_000_000_000).await;
            assert_eq!(routes.summary_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(routes.history_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(cache.system("sys").unwrap().job_records("backup").len(), 2);

            // Steady state: the summary is re-read every cadence, the history
            // body never is.
            observe(&client, &mut cache, &target, 1_700_000_030_000).await;
            observe(&client, &mut cache, &target, 1_700_000_060_000).await;
            assert_eq!(routes.summary_hits.load(std::sync::atomic::Ordering::SeqCst), 3);
            assert_eq!(
                routes.history_hits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "the history body must not be downloaded on an unchanged revision"
            );
        });
    }

    #[test]
    fn a_revision_change_fetches_history_exactly_once() {
        runtime().block_on(async {
            let server = spawn_counting_server(4).await;
            let client = CronClient::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(server.port);

            observe(&client, &mut cache, &target, 1).await;
            assert_eq!(server.history_hits(), 1);
            assert_eq!(server.summary_hits(), 1);

            // Same revision twice more: the summary is still read on every
            // cadence, the history body never is.
            observe(&client, &mut cache, &target, 2).await;
            observe(&client, &mut cache, &target, 3).await;
            assert_eq!(server.history_hits(), 1);
            assert_eq!(server.summary_hits(), 3);

            server.set_revision(5);
            observe(&client, &mut cache, &target, 4).await;
            assert_eq!(
                server.history_hits(),
                2,
                "a revision change must trigger exactly one fetch"
            );
            observe(&client, &mut cache, &target, 5).await;
            assert_eq!(server.history_hits(), 2);
        });
    }

    /// A loopback server whose summary revision the test can advance, plus its
    /// own per-route hit counters.
    ///
    /// The counters are returned rather than being process-wide statics: tests
    /// run in parallel, so a shared counter would let one test's request count
    /// decide another test's assertion.
    struct CountingServer {
        port: u16,
        revision: std::sync::Arc<std::sync::Mutex<u64>>,
        summary_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        history_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl CountingServer {
        fn set_revision(&self, revision: u64) {
            *self.revision.lock().expect("lock") = revision;
        }

        fn summary_hits(&self) -> usize {
            self.summary_hits.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn history_hits(&self) -> usize {
            self.history_hits.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    async fn spawn_counting_server(revision: u64) -> CountingServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let revision = std::sync::Arc::new(std::sync::Mutex::new(revision));
        let summary_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let history_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task_revision = std::sync::Arc::clone(&revision);
        let task_summary_hits = std::sync::Arc::clone(&summary_hits);
        let task_history_hits = std::sync::Arc::clone(&history_hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let revision = std::sync::Arc::clone(&task_revision);
                let summary_hits = std::sync::Arc::clone(&task_summary_hits);
                let history_hits = std::sync::Arc::clone(&task_history_hits);
                tokio::spawn(async move {
                    let mut request = String::new();
                    let mut chunk = [0_u8; 1024];
                    loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => {
                                request
                                    .push_str(String::from_utf8_lossy(&chunk[..count]).as_ref());
                                if request.contains("\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let current = *revision.lock().expect("lock");
                    let (body, hits) = if request.contains("/v2/scheduler/history") {
                        (history_body(current, &[1, 2]), history_hits)
                    } else {
                        (summary_body(current), summary_hits)
                    };
                    hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream
                        .write_all(format!("{head}{}", String::from_utf8_lossy(&body)).as_bytes())
                        .await;
                    let _ = stream.flush().await;
                });
            }
        });
        CountingServer {
            port,
            revision,
            summary_hits,
            history_hits,
        }
    }

    #[test]
    fn a_scheduler_failure_keeps_the_last_known_data() {
        runtime().block_on(async {
            // First a good read so there is something to retain.
            let server = spawn_counting_server(1).await;
            let client = CronClient::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            observe(&client, &mut cache, &endpoint(server.port), 1).await;
            assert_eq!(cache.system("sys").unwrap().job_records("backup").len(), 2);

            // Now the remote stops answering the scheduler route. Nothing
            // listens on this port, so the summary read fails.
            let unreachable = endpoint(1);
            let before_hits = server.history_hits();
            let changed = observe(&client, &mut cache, &unreachable, 2).await;

            let state = cache.system_mut("sys");
            assert!(changed, "the failure itself is a visible change");
            assert!(state.last_error.is_some());
            assert!(
                state.summary.is_some(),
                "a transient scheduler failure must not erase the retained summary"
            );
            assert_eq!(state.job_records("backup").len(), 2);
            assert_eq!(
                server.history_hits(),
                before_hits,
                "a failed summary must not also download the history body"
            );
            // The failure is scoped to the scheduler plane: nothing in this
            // module can reach Reachability, which is what keeps a cron route
            // error from turning a healthy system offline.
            assert_eq!(state.capability, crate::cron::CronCapability::Supported);
        });
    }

    #[test]
    fn a_malformed_document_is_reported_as_invalid_not_as_a_network_error() {
        runtime().block_on(async {
            let routes = Routes {
                summary: Some((200, b"{not json".to_vec())),
                ..Routes::default()
            };
            let port = serve(routes).await;
            let client = CronClient::new(Duration::from_secs(2));
            match client.summary(&endpoint(port)).await {
                SummaryOutcome::Failed(CronFetchError::Invalid(_)) => {}
                other => panic!("expected Invalid, got {other:?}"),
            }
        });
    }

    #[test]
    fn a_summary_that_fails_its_own_validation_is_rejected() {
        runtime().block_on(async {
            // A well-formed JSON body whose schema_version is wrong is exactly
            // what a mismatched daemon would produce.
            let mut value: serde_json::Value =
                serde_json::from_slice(&summary_body(1)).expect("json");
            value["schema_version"] = serde_json::json!(9);
            let routes = Routes {
                summary: Some((200, serde_json::to_vec(&value).expect("json"))),
                ..Routes::default()
            };
            let port = serve(routes).await;
            let client = CronClient::new(Duration::from_secs(2));
            match client.summary(&endpoint(port)).await {
                SummaryOutcome::Failed(CronFetchError::Invalid(_)) => {}
                other => panic!("expected Invalid, got {other:?}"),
            }
        });
    }

    #[test]
    fn an_empty_but_supported_scheduler_is_supported_with_no_jobs() {
        runtime().block_on(async {
            let summary = SchedulerSummaryV2 {
                schema_version: 2,
                generated_at_unix_ms: 1_700_000_000_000,
                epoch: epoch(),
                history_revision: 0,
                jobs: Vec::new(),
            };
            let routes = Routes {
                summary: Some((200, serde_json::to_vec(&summary).expect("json"))),
                history: Some((200, history_body(0, &[]))),
                ..Routes::default()
            };
            let port = serve(routes).await;
            let client = CronClient::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            observe(&client, &mut cache, &endpoint(port), 1).await;

            let state = cache.system("sys").expect("state");
            assert_eq!(state.capability, crate::cron::CronCapability::Supported);
            assert!(
                state.summary.as_ref().expect("summary").jobs.is_empty(),
                "supported with no jobs is not the same as unsupported"
            );
        });
    }

    #[test]
    fn a_repeated_observation_does_not_grow_the_cache() {
        runtime().block_on(async {
            let server = spawn_counting_server(9).await;
            let client = CronClient::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(server.port);
            for tick in 1..=5 {
                observe(&client, &mut cache, &target, tick).await;
            }
            assert_eq!(cache.total_records(), 2);
            assert_eq!(server.history_hits(), 1);
        });
    }
}
