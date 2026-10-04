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
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

use crate::cron::{CronCache, CronFetchError};
use crate::endpoint::{Endpoint, EndpointError};
use crate::state::now_unix_ms;

/// Map a transport failure onto a scheduler error.
///
/// A body over the route's own cap is reported as such rather than as a
/// generic transport failure: "the daemon sent more than its contract allows"
/// is a real daemon problem, and hiding it behind "connection error" would send
/// the operator looking in the wrong place.
fn classify_failure(failure: &eggfetch_core::RequestFailure) -> CronFetchError {
    if matches!(failure.error(), eggfetch_core::Error::DecodedBodyTooLarge) {
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

/// One finished round of scheduler observation for a single endpoint.
///
/// The worker decides *whether* to fetch history and the engine decides *what to
/// do* with what came back. Splitting it this way keeps the cache single-owner
/// (the engine, which also projects it into the published document) without the
/// worker needing a lock, and without either side duplicating the other's
/// bookkeeping.
#[derive(Debug)]
pub struct CronObservation {
    /// Stable system id the observation belongs to.
    pub system_id: String,
    /// What the summary read produced.
    pub summary: SummaryOutcome,
    /// The history body, fetched only when the gate said it was needed.
    ///
    /// `None` alongside a `Supported` summary means the gate suppressed the
    /// fetch, which is the steady state and is not a failure.
    pub history: Option<SchedulerHistoryV2>,
    /// A problem with the *history* route while the summary was fine.
    ///
    /// Kept separate from the summary outcome on purpose. A remote that serves
    /// `/v2/scheduler` but not `/v2/scheduler/history` has still told us
    /// everything the at-a-glance job rows need; folding that into a failed
    /// summary would throw the job list away and leave the pane empty, which
    /// is strictly worse than showing the summary with a stale marker.
    pub history_error: Option<CronFetchError>,
    /// Unix milliseconds the round completed.
    pub now_unix_ms: u64,
}

impl CronObservation {
    /// Apply this observation to the shared cache.
    ///
    /// Returns whether the cache changed, so the caller publishes only on a real
    /// change. A scheduler failure is recorded but never erases retained data,
    /// and nothing here can reach system reachability.
    pub fn apply(self, cache: &mut CronCache) -> bool {
        let CronObservation {
            system_id,
            summary,
            history,
            history_error,
            now_unix_ms,
        } = self;

        match summary {
            SummaryOutcome::Supported(summary) => {
                let revision = summary.history_revision;
                let epoch = summary.epoch;
                // Compare against the pre-write state: "did this change anything
                // the operator can see" has to be judged before it is overwritten.
                let unchanged = cache.system(&system_id).is_some_and(|state| {
                    state.history_revision == Some(revision)
                        && state.epoch == Some(epoch)
                        && state.capability == crate::cron::CronCapability::Supported
                });
                // The summary is applied first and unconditionally: it is valid,
                // and `apply_summary` clears any previous error. A history
                // problem is then recorded on top of it rather than instead of
                // it, so the job list survives a broken history route.
                cache.apply_summary(&system_id, *summary, now_unix_ms);
                let mut changed = !unchanged;
                if let Some(history) = history {
                    changed |= cache.apply_history(&system_id, &history);
                }
                if let Some(error) = history_error {
                    cache.system_mut(&system_id).mark_failed(error, now_unix_ms);
                    changed = true;
                }
                changed
            }
            SummaryOutcome::Unsupported => {
                let state = cache.system_mut(&system_id);
                let was = state.capability != crate::cron::CronCapability::Unsupported;
                state.mark_unsupported();
                // `last_attempt_at` still moves, so the renderer can say the
                // answer is current rather than implying it was never asked.
                state.last_attempt_at_unix_ms = Some(now_unix_ms);
                was
            }
            SummaryOutcome::Failed(error) => {
                let state = cache.system_mut(&system_id);
                let was = state.last_error.as_ref() != Some(&error);
                state.mark_failed(error, now_unix_ms);
                was
            }
        }
    }
}

/// Read the shared endpoint list without holding the lock across an await.
fn lock_endpoints(endpoints: &Arc<std::sync::Mutex<Vec<Endpoint>>>) -> Vec<Endpoint> {
    endpoints
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The history-fetch gate the worker keeps for each system.
///
/// Deliberately worker-private. It answers one question — "have I already
/// downloaded the history for this remote epoch and revision?" — which is about
/// the worker's own requests rather than about fleet state, so the engine never
/// reads it and the two cannot drift into a disagreement that matters.
///
/// The gate advances only on a *valid* fetch. A failed or invalid round leaves
/// it where it was so the next cadence retries, which is the same
/// "never back off, never prune" rule the metrics scheduler follows.
#[derive(Debug, Default)]
struct HistoryGate {
    entries: std::collections::BTreeMap<String, (gregg_protocol::SchedulerEpochV2, u64)>,
}

impl HistoryGate {
    /// Whether a history fetch is needed for this system and summary.
    ///
    /// The epoch comparison is the load-bearing part: a restarted `greggd`
    /// resets `history_revision` and can reset it to the same small value it
    /// used before, so a revision-only gate would conclude "nothing changed" and
    /// never fetch the new epoch at all.
    fn needs(&self, system_id: &str, summary: &SchedulerSummaryV2) -> bool {
        match self.entries.get(system_id) {
            None => true,
            Some((epoch, revision)) => {
                *epoch != summary.epoch || *revision != summary.history_revision
            }
        }
    }

    /// Record a successfully fetched history document.
    fn record(&mut self, system_id: &str, history: &SchedulerHistoryV2) {
        self.entries.insert(
            system_id.to_owned(),
            (history.epoch, history.history_revision),
        );
    }

    /// Forget systems that left the fleet.
    fn forget_absent(&mut self, live: &[String]) {
        self.entries
            .retain(|id, _| live.iter().any(|kept| kept == id));
    }
}

/// The client daemon's scheduler-polling task.
///
/// Owns the HTTP client and the history gate; hands finished observations to the
/// engine. It polls whether or not any TUI is attached, because continuous
/// background observation is the point of the architecture, and opening a cron
/// pane must not change the fleet's remote request budget.
pub struct CronWorker {
    client: CronClient,
    gate: HistoryGate,
}

impl CronWorker {
    /// Create a worker with the given request deadline.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            client: CronClient::new(timeout),
            gate: HistoryGate::default(),
        }
    }

    /// Run until cancelled, sending one observation per endpoint per tick.
    ///
    /// Endpoints are walked sequentially rather than concurrently on purpose:
    /// the summary is small, the cadence is deliberately slow, and a bounded
    /// sequential walk cannot produce a burst that competes with the metrics
    /// scheduler for the daemon's budget.
    ///
    /// The endpoint list is shared rather than owned because a config reload can
    /// change it while this loop is mid-round. It is read under a short lock and
    /// the lock is never held across an await, so a reload cannot stall the
    /// scheduler or the engine.
    pub async fn run(
        mut self,
        endpoints: Arc<std::sync::Mutex<Vec<Endpoint>>>,
        reload: Arc<Notify>,
        updates: mpsc::Sender<CronObservation>,
        cancel: CancellationToken,
    ) {
        let mut tick = tokio::time::interval(CRON_SUMMARY_INTERVAL);
        // A round happens before the first wait, so a fresh daemon answers cron
        // questions on startup rather than after one full interval. `Delay`
        // keeps a slow round from being followed by a burst of catch-up rounds.
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let current = lock_endpoints(&endpoints);
            let live: Vec<String> = current.iter().map(|endpoint| endpoint.id.clone()).collect();
            self.gate.forget_absent(&live);
            for endpoint in &current {
                let observation = self.observe(endpoint).await;
                // A closed receiver means the engine is gone, which is the only
                // reason to stop.
                if updates.send(observation).await.is_err() {
                    return;
                }
            }
            tokio::select! {
                () = cancel.cancelled() => return,
                _ = tick.tick() => {}
                // A reload wakes the worker immediately rather than making the
                // operator wait out the remainder of the interval to see a
                // newly added system. `notify_one` stores a permit, so a reload
                // that lands mid-round is still observed on the next wait.
                () = reload.notified() => {}
            }
        }
    }

    /// One bounded observation for one endpoint.
    pub async fn observe(&mut self, endpoint: &Endpoint) -> CronObservation {
        let system_id = endpoint.id.clone();
        let now_unix_ms = now_unix_ms();
        let summary = self.client.summary(endpoint).await;
        let history = match &summary {
            SummaryOutcome::Supported(document) if self.gate.needs(&system_id, document) => {
                match self.client.history(endpoint).await {
                    Ok(Some(history)) => {
                        self.gate.record(&system_id, &history);
                        Some(history)
                    }
                    // A 404 here means the remote serves a summary but not its
                    // history: a real inconsistency, reported rather than passed
                    // off as "nothing to fetch". The summary itself is kept.
                    Ok(None) => {
                        return CronObservation {
                            system_id,
                            summary,
                            history: None,
                            history_error: Some(CronFetchError::Transport(
                                "the scheduler summary is served but the history route is not"
                                    .to_owned(),
                            )),
                            now_unix_ms,
                        };
                    }
                    // Leave the gate where it was so the next cadence retries,
                    // and report the reason without discarding the summary.
                    Err(error) => {
                        return CronObservation {
                            system_id,
                            summary,
                            history: None,
                            history_error: Some(error),
                            now_unix_ms,
                        };
                    }
                }
            }
            _ => None,
        };
        CronObservation {
            system_id,
            summary,
            history,
            history_error: None,
            now_unix_ms,
        }
    }
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
                                request.push_str(String::from_utf8_lossy(&chunk[..count]).as_ref());
                                if request.contains("\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let is_history = request.contains("/v2/scheduler/history");
                    let route = if is_history { history } else { summary };
                    let hits = if is_history {
                        history_hits
                    } else {
                        summary_hits
                    };
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
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(port);

            // Discovery: one summary, one history.
            worker.observe(&target).await.apply(&mut cache);
            assert_eq!(
                routes
                    .summary_hits
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(
                routes
                    .history_hits
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            assert_eq!(cache.system("sys").unwrap().job_records("backup").len(), 2);

            // Steady state: the summary is re-read every cadence, the history
            // body never is.
            worker.observe(&target).await.apply(&mut cache);
            worker.observe(&target).await.apply(&mut cache);
            assert_eq!(
                routes
                    .summary_hits
                    .load(std::sync::atomic::Ordering::SeqCst),
                3
            );
            assert_eq!(
                routes
                    .history_hits
                    .load(std::sync::atomic::Ordering::SeqCst),
                1,
                "the history body must not be downloaded on an unchanged revision"
            );
        });
    }

    #[test]
    fn a_revision_change_fetches_history_exactly_once() {
        runtime().block_on(async {
            let server = spawn_counting_server(4).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(server.port);

            worker.observe(&target).await.apply(&mut cache);
            assert_eq!(server.history_hits(), 1);
            assert_eq!(server.summary_hits(), 1);

            // Same revision twice more: the summary is still read on every
            // cadence, the history body never is.
            worker.observe(&target).await.apply(&mut cache);
            worker.observe(&target).await.apply(&mut cache);
            assert_eq!(server.history_hits(), 1);
            assert_eq!(server.summary_hits(), 3);

            server.set_revision(5);
            worker.observe(&target).await.apply(&mut cache);
            assert_eq!(
                server.history_hits(),
                2,
                "a revision change must trigger exactly one fetch"
            );
            worker.observe(&target).await.apply(&mut cache);
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
                                request.push_str(String::from_utf8_lossy(&chunk[..count]).as_ref());
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
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            worker
                .observe(&endpoint(server.port))
                .await
                .apply(&mut cache);
            assert_eq!(cache.system("sys").unwrap().job_records("backup").len(), 2);

            // Now the remote stops answering the scheduler route. Nothing
            // listens on this port, so the summary read fails.
            let unreachable = endpoint(1);
            let before_hits = server.history_hits();
            let changed = worker.observe(&unreachable).await.apply(&mut cache);

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
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            worker.observe(&endpoint(port)).await.apply(&mut cache);

            let state = cache.system("sys").expect("state");
            assert_eq!(state.capability, crate::cron::CronCapability::Supported);
            assert!(
                state.summary.as_ref().expect("summary").jobs.is_empty(),
                "supported with no jobs is not the same as unsupported"
            );
        });
    }

    #[test]
    fn a_daemon_restart_reseeds_from_the_remote_ring() {
        runtime().block_on(async {
            // The remote holds two records. A first client daemon caches them.
            let server = spawn_counting_server(3).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut first = CronCache::default();
            worker
                .observe(&endpoint(server.port))
                .await
                .apply(&mut first);
            assert_eq!(first.system("sys").unwrap().job_records("backup").len(), 2);

            // The client daemon restarts: the in-memory cache is gone, and the
            // new process knows nothing about the epoch or revision, so its
            // gate is empty and it fetches again. That is the reseed.
            let mut second_worker = CronWorker::new(Duration::from_secs(2));
            let mut second = CronCache::default();
            second_worker
                .observe(&endpoint(server.port))
                .await
                .apply(&mut second);
            assert_eq!(
                second.system("sys").unwrap().job_records("backup").len(),
                2,
                "a restarted client daemon must recover the remote ring"
            );
            assert_eq!(server.history_hits(), 2);
        });
    }

    #[test]
    fn many_frontends_cannot_add_a_single_remote_request() {
        runtime().block_on(async {
            // The reduced cron intents decide what is *published*. The worker
            // never sees them, so a window that opens the cron pane, ten
            // windows that open it, and no windows at all must produce exactly
            // the same remote request counts.
            let server = spawn_counting_server(6).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            for _ in 0..3 {
                worker
                    .observe(&endpoint(server.port))
                    .await
                    .apply(&mut cache);
            }
            assert_eq!(server.summary_hits(), 3);
            assert_eq!(
                server.history_hits(),
                1,
                "history is fetched on discovery and then only on revision change"
            );
        });
    }

    #[test]
    fn a_summary_served_without_its_history_route_is_reported() {
        runtime().block_on(async {
            // A remote that serves the summary but not the history route is a
            // real inconsistency. Passing it off as "nothing to fetch" would
            // leave the pane silently empty forever.
            let routes = Routes {
                summary: Some((200, summary_body(2))),
                history: None,
                ..Routes::default()
            };
            let port = serve(routes).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            worker.observe(&endpoint(port)).await.apply(&mut cache);

            let state = cache.system("sys").expect("state");
            assert!(
                matches!(state.last_error, Some(CronFetchError::Transport(_))),
                "{:?}",
                state.last_error
            );
            assert!(
                state.summary.is_some(),
                "the summary that did arrive is still retained"
            );
        });
    }

    #[test]
    fn a_failed_history_fetch_does_not_advance_the_gate() {
        runtime().block_on(async {
            // A revision that changed, whose history body is unreachable, must be
            // retried on the next cadence rather than being suppressed forever.
            let routes = Routes {
                summary: Some((200, summary_body(1))),
                history: None,
                ..Routes::default()
            };
            let port = serve(routes).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            for _ in 0..3 {
                worker.observe(&endpoint(port)).await.apply(&mut cache);
            }
            assert!(
                cache.system("sys").unwrap().history_revision.is_none(),
                "a history body that never arrived must not be recorded as applied"
            );
        });
    }

    #[test]
    fn a_repeated_observation_does_not_grow_the_cache() {
        runtime().block_on(async {
            let server = spawn_counting_server(9).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(server.port);
            for _ in 1..=5 {
                worker.observe(&target).await.apply(&mut cache);
            }
            assert_eq!(cache.total_records(), 2);
            assert_eq!(server.history_hits(), 1);
        });
    }
}
