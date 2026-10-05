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

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use gregg_protocol::{SchedulerHistoryV2, SchedulerSummaryV2};
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::cron::{CronCache, CronFetchError, CronSystemState};
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

/// Maximum number of endpoint scheduler reads in flight at once.
///
/// Small, fixed, and deliberately not configuration. The summary plane is
/// slower and smaller than metrics by design, so this is not about throughput:
/// a strictly sequential walk let one endpoint's request deadline delay every
/// system behind it, so the effective fleet cadence became a multiple of the
/// nominal 30 seconds — the pane looked stale for reasons that had nothing to do
/// with the remote. Four removes that head-of-line wait while capping concurrent
/// scheduler requests far below the metrics scheduler's own in-flight bound, so
/// the two planes still cannot add up to a burst.
///
/// Nothing here is spawned: the round runs as one bounded set of futures on the
/// worker's own task, so a reload or a shutdown drops at most four in-flight
/// reads and no read outlives the worker.
const CRON_MAX_IN_FLIGHT: usize = 4;

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
    /// The endpoint this read actually polled.
    ///
    /// Carried for the same reason a metrics result carries its endpoint: a
    /// `Ctrl-R` can repoint a stable id at a different host while a scheduler
    /// request for the old host is still in flight, and that late answer belongs
    /// to a target the configuration no longer contains. Keyed by id alone it
    /// would be accepted as the new target's answer.
    pub host: String,
    /// The port this read actually polled.
    pub port: u16,
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
    /// Unix milliseconds the round completed — after every request it made, not
    /// before the first one.
    pub now_unix_ms: u64,
}

impl CronObservation {
    /// Apply this observation to the shared cache.
    ///
    /// Returns whether the operator-visible state changed, so the caller
    /// publishes only on a real change. A scheduler failure is recorded but
    /// never erases retained data, and nothing here can reach system
    /// reachability.
    ///
    /// The change decision is a comparison of what a cron row can *draw* —
    /// capability, job rows, `(epoch, revision)`, and the stale marker — taken
    /// around the write rather than guessed from a revision counter.
    /// `history_revision` is a *history* revision: it does not move for an
    /// ordinary live transition such as idle → waiting, a job starting, a load
    /// gate appearing, or a next-due advancing, and judging "changed" from it
    /// alone let a valid newer summary sit in the cache unpublished until some
    /// unrelated event forced a document. The same blindness hid error
    /// recovery, because a successful read clears `last_error` while the
    /// revision and epoch can both be unchanged.
    ///
    /// Local attempt/success timestamps are excluded from that comparison, so an
    /// unchanged successful poll still publishes nothing.
    pub fn apply(self, cache: &mut CronCache) -> bool {
        let CronObservation {
            system_id,
            host: _,
            port: _,
            summary,
            history,
            history_error,
            now_unix_ms,
        } = self;

        let before = cache
            .system(&system_id)
            .map(CronSystemState::rendered_state);
        // The summary is applied first and unconditionally: it is valid, and
        // `apply_summary` clears any previous error. A history problem is then
        // recorded on top of it rather than instead of it, so the job list
        // survives a broken history route.
        match summary {
            SummaryOutcome::Supported(summary) => {
                cache.apply_summary(&system_id, *summary, now_unix_ms);
                // Belt and braces: a history merge that retained a record is
                // drawn, so it is a change even in the future shape where the
                // summary identity alone would not have said so.
                let merged = if let Some(history) = history {
                    cache.apply_history(&system_id, &history)
                } else {
                    false
                };
                if let Some(error) = history_error {
                    cache.system_mut(&system_id).mark_failed(error, now_unix_ms);
                }
                if merged {
                    return true;
                }
            }
            SummaryOutcome::Unsupported => {
                let state = cache.system_mut(&system_id);
                state.mark_unsupported();
                // `last_attempt_at` still moves, so the renderer can say the
                // answer is current rather than implying it was never asked.
                state.last_attempt_at_unix_ms = Some(now_unix_ms);
            }
            SummaryOutcome::Failed(error) => {
                cache.system_mut(&system_id).mark_failed(error, now_unix_ms);
            }
        }
        let after = cache
            .system(&system_id)
            .map(CronSystemState::rendered_state);
        after != before
    }
}

/// The identity one scheduler read was bound to.
///
/// The system id alone is not enough: a stable id can be repointed at a
/// different endpoint, and a gate keyed only by id would let the new target
/// inherit the previous target's "already fetched" answer whenever the two
/// happened to report the same epoch and revision. The host is normalized and
/// lowercased with the same equivalence the metrics reducer applies, so an
/// equivalent spelling of one endpoint is one key and an actual repoint is a
/// different one.
///
/// The separator is a unit separator so an id can never forge another entry's
/// key by containing the delimiter.
fn target_key(system_id: &str, host: &str, port: u16) -> String {
    let normalized = crate::endpoint::normalize_host(host)
        .map_or_else(|_| host.to_owned(), |host| host.to_ascii_lowercase());
    format!("{system_id}\u{1f}{normalized}\u{1f}{port}")
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
#[derive(Debug, Default, Clone)]
struct HistoryGate {
    entries: std::collections::BTreeMap<String, (gregg_protocol::SchedulerEpochV2, u64)>,
}

impl HistoryGate {
    /// Whether a history fetch is needed for this target and summary.
    ///
    /// The epoch comparison is the load-bearing part: a restarted `greggd`
    /// resets `history_revision` and can reset it to the same small value it
    /// used before, so a revision-only gate would conclude "nothing changed" and
    /// never fetch the new epoch at all.
    fn needs(&self, key: &str, summary: &SchedulerSummaryV2) -> bool {
        match self.entries.get(key) {
            None => true,
            Some((epoch, revision)) => {
                *epoch != summary.epoch || *revision != summary.history_revision
            }
        }
    }

    /// Record a successfully fetched *and coherent* history document.
    ///
    /// Only a document that matched the summary it was fetched with may get
    /// here. Advancing the gate from a mismatched pair would tell the next
    /// summary for the newer lifetime that its history was already downloaded,
    /// which is exactly how a new epoch's records go missing indefinitely.
    fn record(&mut self, key: &str, history: &SchedulerHistoryV2) {
        self.entries
            .insert(key.to_owned(), (history.epoch, history.history_revision));
    }

    /// Forget targets that are no longer polled.
    fn forget_absent(&mut self, live: &[String]) {
        self.entries
            .retain(|key, _| live.iter().any(|kept| kept == key));
    }
}

/// What one endpoint's HTTP work produced, before any gate or coherence
/// decision.
///
/// Deliberately carries the endpoint back with it: the round runs several of
/// these concurrently, so the identity has to travel with the answer rather than
/// be recovered from a shared slot the next poll would overwrite.
#[derive(Debug)]
struct EndpointFetch {
    endpoint: Endpoint,
    summary: SummaryOutcome,
    history: HistoryAttempt,
}

/// What became of the history request for one endpoint.
#[derive(Debug)]
enum HistoryAttempt {
    /// The gate suppressed the fetch: the steady state, not a failure.
    Suppressed,
    /// A fetch was made. `Ok(None)` is a remote that serves the summary but not
    /// the history route, which is a real inconsistency rather than "nothing to
    /// fetch".
    Fetched(Result<Option<SchedulerHistoryV2>, CronFetchError>),
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
    /// Each round observes endpoints with a fixed bound of
    /// [`CRON_MAX_IN_FLIGHT`] reads in flight, all on this one task. Nothing is
    /// spawned, so a reload or shutdown drops at most those few in-flight reads
    /// and none of them can outlive the worker; and because the window is
    /// bounded, a large fleet cannot turn into a burst competing with the
    /// metrics scheduler for the daemon's budget.
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
        // One round at startup, then one per interval. `interval`'s first tick
        // is already due, so awaiting it straight after the startup round fired
        // a second back-to-back round: a fresh daemon asked every endpoint twice
        // before it had learned anything, for no benefit. `interval_at` puts the
        // first deadline one interval out instead. `Delay` keeps a slow round
        // from being followed by a burst of catch-up rounds.
        let mut tick = tokio::time::interval_at(
            Instant::now() + CRON_SUMMARY_INTERVAL,
            CRON_SUMMARY_INTERVAL,
        );
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let current = lock_endpoints(&endpoints);
            let live: Vec<String> = current
                .iter()
                .map(|endpoint| target_key(&endpoint.id, &endpoint.host, endpoint.port))
                .collect();
            self.gate.forget_absent(&live);
            // A closed receiver means the engine is gone, which is the only
            // reason to stop.
            if !self.round(current, &updates).await {
                return;
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

    /// Observe every endpoint once, at most [`CRON_MAX_IN_FLIGHT`] at a time.
    ///
    /// Returns `false` when the receiver closed, which is the only reason to
    /// stop. The HTTP work overlaps on one task with a fixed window, while the
    /// gate is read through one immutable round snapshot and mutated serially as
    /// results land, so history decisions stay single owner.
    async fn round(
        &mut self,
        endpoints: Vec<Endpoint>,
        updates: &mpsc::Sender<CronObservation>,
    ) -> bool {
        let Self { client, gate } = self;
        // One snapshot per round rather than one per endpoint. It is read-only
        // inside the concurrent phase and never shared between endpoints — each
        // entry belongs to exactly one configured system — so a snapshot taken
        // at round start cannot make a later endpoint's decision stale.
        let snapshot = gate.clone();
        let mut pending = endpoints.into_iter();
        let mut in_flight = FuturesUnordered::new();
        while in_flight.len() < CRON_MAX_IN_FLIGHT {
            let Some(endpoint) = pending.next() else {
                break;
            };
            in_flight.push(fetch(&*client, endpoint, &snapshot));
        }
        while let Some(finished) = in_flight.next().await {
            let observation = settle(gate, finished);
            if updates.send(observation).await.is_err() {
                return false;
            }
            if let Some(endpoint) = pending.next() {
                in_flight.push(fetch(&*client, endpoint, &snapshot));
            }
        }
        true
    }

    /// One bounded observation for one endpoint.
    pub async fn observe(&mut self, endpoint: &Endpoint) -> CronObservation {
        let finished = fetch(&self.client, endpoint.clone(), &self.gate).await;
        settle(&mut self.gate, finished)
    }
}

/// Fetch one endpoint's summary, and its history only when the gate asks.
///
/// Pure HTTP plus a read-only gate check: everything that decides *what to do*
/// with the pair happens later, in [`settle`], so no decision is made twice.
async fn fetch(client: &CronClient, endpoint: Endpoint, gate: &HistoryGate) -> EndpointFetch {
    let summary = client.summary(&endpoint).await;
    let history = match &summary {
        SummaryOutcome::Supported(document) => {
            let key = target_key(&endpoint.id, &endpoint.host, endpoint.port);
            if gate.needs(&key, document) {
                HistoryAttempt::Fetched(client.history(&endpoint).await)
            } else {
                HistoryAttempt::Suppressed
            }
        }
        SummaryOutcome::Unsupported | SummaryOutcome::Failed(_) => HistoryAttempt::Suppressed,
    };
    EndpointFetch {
        endpoint,
        summary,
        history,
    }
}

/// Turn one finished fetch into an observation: coherence first, then the gate.
///
/// The coherence check is the load-bearing part. `/v2/scheduler` and
/// `/v2/scheduler/history` are two independent requests, so a `greggd` restart
/// or a history change between them yields summary A with history B. That pair
/// is not history: merging it would attribute records to the wrong lifetime, and
/// advancing the gate from it would let the next summary *for B* look already
/// fetched — so B's own records could stay missing until some later revision
/// happened to change. A mismatch is therefore reported as a scheduler-scoped
/// diagnostic, the gate is left alone, and the next cadence retries.
///
/// `generated_at_unix_ms` equality is deliberately not required: a live-state
/// publication can legitimately rebuild the pair without changing the retained
/// history.
fn settle(gate: &mut HistoryGate, finished: EndpointFetch) -> CronObservation {
    let EndpointFetch {
        endpoint,
        summary,
        history,
    } = finished;
    let system_id = endpoint.id.clone();
    let host = endpoint.host.clone();
    let port = endpoint.port;
    let key = target_key(&system_id, &host, port);

    let mut applied = None;
    let mut history_error = None;
    if let SummaryOutcome::Supported(document) = &summary {
        match history {
            HistoryAttempt::Suppressed => {}
            HistoryAttempt::Fetched(Ok(Some(found))) => {
                if found.epoch == document.epoch
                    && found.history_revision == document.history_revision
                {
                    gate.record(&key, &found);
                    applied = Some(found);
                } else {
                    history_error = Some(CronFetchError::Incoherent(format!(
                        "summary is epoch {:?} revision {} but history is epoch {:?} revision {}",
                        (document.epoch.started_at_unix_ms, document.epoch.nonce),
                        document.history_revision,
                        (found.epoch.started_at_unix_ms, found.epoch.nonce),
                        found.history_revision
                    )));
                }
            }
            // A 404 here means the remote serves a summary but not its history:
            // a real inconsistency, reported rather than passed off as "nothing
            // to fetch". The summary itself is kept.
            HistoryAttempt::Fetched(Ok(None)) => {
                history_error = Some(CronFetchError::Transport(
                    "the scheduler summary is served but the history route is not".to_owned(),
                ));
            }
            // Leave the gate where it was so the next cadence retries, and
            // report the reason without discarding the summary.
            HistoryAttempt::Fetched(Err(error)) => {
                history_error = Some(error);
            }
        }
    }
    CronObservation {
        system_id,
        host,
        port,
        summary,
        history: applied,
        history_error,
        // The round's completion time: captured after every request, so the
        // documented meaning is true rather than aspirational.
        now_unix_ms: now_unix_ms(),
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

    /// How long the deliberately slow endpoint stalls.
    ///
    /// Four times the budget the fast endpoints get, so a sequential walk could
    /// not have delivered them inside it, and short enough to keep the test
    /// quick.
    const SLOW_ENDPOINT_DELAY: Duration = Duration::from_secs(4);

    /// How long a fast endpoint stalls before answering.
    ///
    /// A loopback answer is written in a single socket write, so without a stall
    /// every request would start and finish inside one poll and concurrent reads
    /// would be indistinguishable from a serial walk. The meter only ever
    /// overlaps genuinely pending requests, so the stall has to happen *before*
    /// the response, not after it.
    const FAST_ENDPOINT_DELAY: Duration = Duration::from_millis(40);

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
    // ---------------------------------------------------------------------
    // Plan 174: live-summary publication, coherence, target binding, cadence.
    // ---------------------------------------------------------------------

    /// A supported summary with one job, as the observation layer sees it.
    fn supported_summary(job_state: gregg_protocol::SchedulerJobStateV2) -> SummaryOutcome {
        SummaryOutcome::Supported(Box::new(SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch: epoch(),
            // The revision never moves in these cases: that is the point. It is
            // a *history* revision, so a live transition must be judged on the
            // operator-visible state instead.
            history_revision: 4,
            jobs: vec![SchedulerJobV2 {
                name: "backup".to_owned(),
                schedule: "0 3 * * *".to_owned(),
                next_due_unix_ms: 1_700_100_000_000,
                state: job_state,
                load: None,
                pending_since_unix_ms: None,
                next_retry_unix_ms: None,
                running_since_unix_ms: None,
                last: None,
            }],
        }))
    }

    fn observation(summary: SummaryOutcome, now: u64) -> CronObservation {
        CronObservation {
            system_id: "sys".to_owned(),
            host: "127.0.0.1".to_owned(),
            port: 11_310,
            summary,
            history: None,
            history_error: None,
            now_unix_ms: now,
        }
    }

    #[test]
    fn a_live_transition_with_an_unchanged_history_revision_is_still_a_visible_change() {
        // `history_revision` does not move for a live-state transition. Judging
        // "changed" from it alone let a valid newer summary sit in the cache
        // unpublished, and the TUI kept drawing the previous state until some
        // unrelated metrics event happened to force a document.
        let mut cache = CronCache::default();
        assert!(
            observation(
                supported_summary(gregg_protocol::SchedulerJobStateV2::Idle),
                1_000,
            )
            .apply(&mut cache),
            "the first observation is a change"
        );
        assert!(
            observation(
                supported_summary(gregg_protocol::SchedulerJobStateV2::Running),
                31_000,
            )
            .apply(&mut cache),
            "idle -> running with an unchanged history revision must publish"
        );
        let state = cache.system("sys").expect("state");
        assert_eq!(state.history_revision, None);
        assert_eq!(
            state
                .summary
                .as_ref()
                .map(|summary| summary.history_revision),
            Some(4),
            "the summary's revision really is unchanged across the transition"
        );
    }

    #[test]
    fn clearing_a_scheduler_error_with_an_otherwise_identical_summary_is_a_visible_change() {
        // A successful read clears the error, so the document that removes a
        // stale warning must be published. An unchanged epoch and revision made
        // that document optional, which left the operator looking at a warning
        // for a route that now answers.
        let mut cache = CronCache::default();
        observation(
            supported_summary(gregg_protocol::SchedulerJobStateV2::Idle),
            1_000,
        )
        .apply(&mut cache);
        let mut failed = observation(
            supported_summary(gregg_protocol::SchedulerJobStateV2::Idle),
            31_000,
        );
        failed.history_error = Some(CronFetchError::Transport("boom".to_owned()));
        assert!(failed.apply(&mut cache), "the failure is visible");
        assert!(cache.system("sys").expect("state").last_error.is_some());

        assert!(
            observation(
                supported_summary(gregg_protocol::SchedulerJobStateV2::Idle),
                61_000,
            )
            .apply(&mut cache),
            "recovering from an error must publish the document that clears it"
        );
        assert!(cache.system("sys").expect("state").last_error.is_none());
    }

    #[test]
    fn an_identical_successful_poll_does_not_publish_just_because_a_timestamp_moved() {
        // The opposite failure: if the attempt/success timestamps participated in
        // the decision, every 30-second cadence would rebuild every frontend's
        // document for no visible change.
        let mut cache = CronCache::default();
        observation(
            supported_summary(gregg_protocol::SchedulerJobStateV2::Idle),
            1_000,
        )
        .apply(&mut cache);
        for step in 2..=5 {
            assert!(
                !observation(
                    supported_summary(gregg_protocol::SchedulerJobStateV2::Idle),
                    1_000 * step,
                )
                .apply(&mut cache),
                "an unchanged successful poll must not force a publication"
            );
        }
        let state = cache.system("sys").expect("state");
        assert_eq!(
            state.last_attempt_at_unix_ms,
            Some(5_000),
            "the attempt timestamp still advances, it just is not operator-visible"
        );
        assert_eq!(state.last_success_at_unix_ms, Some(5_000));
    }

    /// A summary and a history document that must be treated as one pair only
    /// when their epoch and revision agree.
    fn summary_document(revision: u64, epoch: SchedulerEpochV2) -> SchedulerSummaryV2 {
        SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch,
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
                last: None,
            }],
        }
    }

    fn history_document(
        revision: u64,
        epoch: SchedulerEpochV2,
        sequences: &[u64],
    ) -> SchedulerHistoryV2 {
        SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch,
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
        }
    }

    fn other_epoch(nonce: u64) -> SchedulerEpochV2 {
        SchedulerEpochV2 {
            started_at_unix_ms: 9_000,
            nonce,
        }
    }

    /// Settle one round where the summary and the history document disagree,
    /// and report what the observation carried.
    fn settle_incoherent(
        summary: SchedulerSummaryV2,
        history: SchedulerHistoryV2,
    ) -> (CronObservation, bool) {
        let needs = {
            let gate = HistoryGate::default();
            gate.needs(&target_key("sys", "127.0.0.1", 11_310), &summary)
        };
        assert!(needs, "first discovery always needs history");
        let mut observation = observation(SummaryOutcome::Supported(Box::new(summary)), 5_000);
        let mut gate = HistoryGate::default();
        let finished = EndpointFetch {
            endpoint: endpoint(11_310),
            summary: observation.summary.clone(),
            history: HistoryAttempt::Fetched(Ok(Some(history))),
        };
        observation = settle(&mut gate, finished);
        let SummaryOutcome::Supported(summary) = &observation.summary else {
            panic!("a supported summary stays supported");
        };
        let still_needs = gate.needs(&target_key("sys", "127.0.0.1", 11_310), summary);
        (observation, still_needs)
    }

    #[test]
    fn a_history_from_another_epoch_is_rejected_rather_than_merged() {
        // greggd restarted between the two requests: the pair is summary A with
        // history B. Merging it attributes records to the wrong lifetime, and
        // advancing the gate from it would tell the next summary *for B* that its
        // history was already downloaded.
        let (observation, still_needs) = settle_incoherent(
            summary_document(1, epoch()),
            history_document(1, other_epoch(99), &[7, 8]),
        );
        assert!(
            observation.history.is_none(),
            "an incoherent history document is not history"
        );
        assert!(
            matches!(
                observation.history_error,
                Some(CronFetchError::Incoherent(_))
            ),
            "{:?}",
            observation.history_error
        );
        assert!(
            matches!(observation.summary, SummaryOutcome::Supported(_)),
            "the valid summary is retained"
        );
        assert!(
            still_needs,
            "the gate must not advance, or the new epoch's records never arrive"
        );
    }

    #[test]
    fn a_history_at_a_different_revision_is_rejected_the_same_way() {
        let (observation, still_needs) = settle_incoherent(
            summary_document(2, epoch()),
            history_document(3, epoch(), &[7, 8]),
        );
        assert!(observation.history.is_none());
        assert!(matches!(
            observation.history_error,
            Some(CronFetchError::Incoherent(_))
        ));
        assert!(still_needs);
    }

    #[test]
    fn the_next_coherent_poll_after_a_mismatch_refetches_and_applies_history() {
        runtime().block_on(async {
            // The whole point of refusing the pair: the next cadence must be able
            // to fetch and apply it. A remote that restarted mid-pair therefore
            // costs one round, not the history forever.
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let port = listener.local_addr().expect("addr").port();
            let phase = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let task_phase = std::sync::Arc::clone(&phase);
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let phase = std::sync::Arc::clone(&task_phase);
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
                        // Round 1 answers the summary with one epoch and the
                        // history with another. Round 2 answers coherently.
                        let round = phase.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let epoch = if round == 0 {
                            epoch()
                        } else {
                            other_epoch(99)
                        };
                        let history_epoch = if round == 0 {
                            other_epoch(99)
                        } else {
                            epoch
                        };
                        let body = if request.contains("/v2/scheduler/history") {
                            serde_json::to_vec(&history_document(5, history_epoch, &[7, 8]))
                                .expect("serializes")
                        } else {
                            serde_json::to_vec(&summary_document(5, epoch)).expect("serializes")
                        };
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

            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();
            let target = endpoint(port);
            worker.observe(&target).await.apply(&mut cache);
            let state = cache.system("sys").expect("state");
            assert!(
                matches!(state.last_error, Some(CronFetchError::Incoherent(_))),
                "{:?}",
                state.last_error
            );
            assert_eq!(
                state.job_records("backup").len(),
                0,
                "the mismatched records must not be retained"
            );

            // The next poll is coherent, so the gate still wants it and it lands.
            let changed = worker.observe(&target).await.apply(&mut cache);
            let state = cache.system("sys").expect("state");
            assert!(state.last_error.is_none(), "recovery clears the error");
            assert!(
                changed,
                "applying the recovered history is a visible change"
            );
            assert_eq!(
                state.job_records("backup").len(),
                2,
                "the coherent document is applied on the next cadence"
            );
        });
    }

    #[test]
    fn a_target_key_distinguishes_a_repoint_from_an_equivalent_spelling() {
        let original = target_key("sys", "BOX", 11_310);
        assert_eq!(
            original,
            target_key("sys", "box", 11_310),
            "DNS case is not a different machine"
        );
        assert_ne!(
            original,
            target_key("sys", "box", 11_311),
            "a different port is a different target"
        );
        assert_ne!(original, target_key("sys", "other", 11_310));
        assert_ne!(
            original,
            target_key("other", "box", 11_310),
            "the id is part of the key, so two systems cannot collide"
        );
    }

    #[test]
    fn a_repointed_target_whose_numbers_collide_still_performs_first_discovery() {
        runtime().block_on(async {
            // A and B both report epoch 1 revision 1. If the gate were keyed by
            // system id alone, B's first read would look already fetched and its
            // records would never be requested.
            let server = spawn_counting_server(1).await;
            let mut worker = CronWorker::new(Duration::from_secs(2));
            let mut cache = CronCache::default();

            let mut old_target = endpoint(server.port);
            old_target.id = "sys".to_owned();
            worker.observe(&old_target).await.apply(&mut cache);
            assert_eq!(server.history_hits(), 1);

            // The same daemon, reached as a different target for the same id.
            let mut new_target = old_target.clone();
            new_target.host = "127.0.0.2".to_owned();
            let key = target_key(&new_target.id, &new_target.host, new_target.port);
            assert!(
                worker.gate.needs(&key, &summary_document(1, epoch())),
                "a repointed target must re-discover its history"
            );
        });
    }

    #[test]
    fn startup_runs_exactly_one_round_before_the_first_period() {
        runtime().block_on(async {
            // `tokio::time::interval`'s first tick is already due, so awaiting it
            // straight after the startup round fired a second back-to-back round:
            // a fresh daemon asked every endpoint twice before learning anything.
            let server = spawn_counting_server(1).await;
            let endpoints = Arc::new(std::sync::Mutex::new(vec![endpoint(server.port)]));
            let (tx, mut rx) = mpsc::channel(8);
            let cancel = CancellationToken::new();
            let worker = CronWorker::new(Duration::from_secs(2));
            let handle =
                tokio::spawn(worker.run(endpoints, Arc::new(Notify::new()), tx, cancel.clone()));

            // The startup round arrives...
            let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("the startup round runs")
                .expect("an observation");
            assert_eq!(first.system_id, "sys");
            // ...and the next one does not arrive until the period elapses.
            let early = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await;
            assert!(
                early.is_err(),
                "a second round fired before the cron period elapsed"
            );
            assert_eq!(
                server.summary_hits(),
                1,
                "one startup round means one summary request"
            );

            // The period still elapses: the cadence is delayed, not cancelled.
            let later = tokio::time::timeout(Duration::from_secs(40), rx.recv())
                .await
                .expect("the cadence continues")
                .expect("an observation");
            assert_eq!(later.system_id, "sys");
            assert!(server.summary_hits() >= 2);
            cancel.cancel();
            let _ = handle.await;
        });
    }

    /// Fleet-wide request meter.
    ///
    /// Shared by every loopback server in a test, because the number that matters
    /// is how many scheduler reads are open *at once across the fleet* — one
    /// server's peak can only ever be one connection per endpoint, which proves
    /// nothing about the round's window.
    #[derive(Clone, Default)]
    struct RequestMeter {
        in_flight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RequestMeter {
        fn enter(&self) {
            let active = self
                .in_flight
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            self.peak
                .fetch_max(active, std::sync::atomic::Ordering::SeqCst);
        }

        fn leave(&self) {
            self.in_flight
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn peak(&self) -> usize {
            self.peak.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn in_flight(&self) -> usize {
            self.in_flight.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// A loopback greggd that stalls for `delay` before answering
    /// `/v2/scheduler`.
    ///
    /// The deliberately slow variant stalls long enough that a sequential walk
    /// could not have delivered the fast endpoints behind it.
    async fn spawn_gated_server(meter: RequestMeter, delay: Duration) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let meter = meter.clone();
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
                    meter.enter();
                    tokio::time::sleep(delay).await;
                    let body = summary_body(1);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream
                        .write_all(format!("{head}{}", String::from_utf8_lossy(&body)).as_bytes())
                        .await;
                    let _ = stream.flush().await;
                    // Counted out the instant the response is on the wire, so the
                    // meter measures requests awaiting an answer and never a
                    // connection still being drained.
                    meter.leave();
                });
            }
        });
        port
    }

    fn gated_endpoint(id: &str, port: u16) -> Endpoint {
        Endpoint {
            id: id.to_owned(),
            host: "127.0.0.1".to_owned(),
            port,
            name: None,
        }
    }

    #[tokio::test]
    async fn one_slow_endpoint_does_not_stall_the_rest_of_the_fleet() {
        let meter = RequestMeter::default();
        let fast_port = spawn_gated_server(meter.clone(), FAST_ENDPOINT_DELAY).await;
        let slow_port = spawn_gated_server(meter.clone(), SLOW_ENDPOINT_DELAY).await;
        let mut endpoints: Vec<Endpoint> = (0..7)
            .map(|index| gated_endpoint(&format!("fast-{index}"), fast_port))
            .collect();
        // The slow endpoint is first on purpose. In a sequential walk every fast
        // system behind it would wait out its request deadline, which is how the
        // effective fleet cadence became a multiple of the nominal one.
        endpoints.insert(0, gated_endpoint("slow", slow_port));

        let (tx, mut rx) = mpsc::channel::<CronObservation>(16);
        let mut worker = CronWorker::new(Duration::from_secs(30));
        let collect = async {
            let mut fast = 0;
            let deadline = Instant::now() + SLOW_ENDPOINT_DELAY / 4;
            while fast < 7 {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let Ok(Some(observation)) = tokio::time::timeout(remaining, rx.recv()).await else {
                    break;
                };
                if observation.system_id.starts_with("fast-") {
                    fast += 1;
                }
            }
            fast
        };
        let (_round, fast) = tokio::join!(worker.round(endpoints, &tx), collect);

        assert_eq!(
            fast, 7,
            "a deliberately slow endpoint must not delay the systems behind it"
        );
        assert!(
            meter.peak() <= CRON_MAX_IN_FLIGHT,
            "the round ran {} requests at once, above the {} bound",
            meter.peak(),
            CRON_MAX_IN_FLIGHT
        );
        assert_eq!(meter.in_flight(), 0, "no read outlives the round");
    }

    #[tokio::test]
    async fn the_in_flight_request_count_never_exceeds_the_cron_bound() {
        // Twelve distinct remotes, because the HTTP client pools per origin: a
        // fleet pointed at one port would serialize behind a single pooled
        // connection and prove nothing about the window.
        let meter = RequestMeter::default();
        let mut endpoints = Vec::new();
        for index in 0..12 {
            let port = spawn_gated_server(meter.clone(), FAST_ENDPOINT_DELAY).await;
            endpoints.push(gated_endpoint(&format!("sys-{index}"), port));
        }
        let (tx, mut rx) = mpsc::channel::<CronObservation>(32);
        let mut worker = CronWorker::new(Duration::from_secs(10));
        let round = worker.round(endpoints, &tx);
        let collect = async {
            let mut seen = 0;
            while let Ok(Some(_)) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                seen += 1;
            }
            seen
        };
        let (_round, seen) = tokio::join!(round, collect);

        assert_eq!(seen, 12, "every endpoint is observed exactly once");
        assert_eq!(meter.in_flight(), 0, "no read outlives the round");
        assert!(
            meter.peak() <= CRON_MAX_IN_FLIGHT,
            "the fleet ran {} requests at once, above the {} bound",
            meter.peak(),
            CRON_MAX_IN_FLIGHT
        );
        assert!(
            meter.peak() > 1,
            "the window must overlap requests for this to mean anything"
        );
    }
}
