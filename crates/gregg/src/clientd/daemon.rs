//! Plan 164: the per-config Gregg client daemon.
//!
//! `gregg daemon run` is a foreground process that owns everything derived
//! from the network for one configuration file: the endpoint HTTP client, the
//! poll scheduler and its generation loop, normalized fleet state, the
//! `EggPool` worker, and the `Ctrl-R` config-reload boundary. Frontends attach
//! over local IPC and receive complete state documents.
//!
//! # Why the split exists
//!
//! A TUI that owns its own poller multiplies the fleet's request budget by the
//! number of open windows, and closing one window silently stops a fraction of
//! what the operator is watching. Moving ownership here means closing every TUI
//! changes nothing about polling, and a second window costs a socket rather
//! than a poll generation.
//!
//! # Fan-out discipline
//!
//! State is published through a [`watch`] channel, which is a *latest value*
//! slot rather than a queue. A frontend that cannot keep up observes a newer
//! generation and skips the ones it missed, which is safe because every
//! document is complete and self-contained. The alternative — queueing each
//! generation per frontend — would let one slow window hold memory
//! proportional to fleet size times poll cadence, and would let it delay
//! publication for everyone else.
//!
//! Each document is serialized **once**, at publication time, and every
//! frontend is handed the same bytes. Serialization therefore costs the same
//! with one TUI open as with five, which is the same discipline `greggd`
//! already uses for its scheduler publication cell.
//!
//! Control acknowledgements are the one non-latest-state message, because a
//! caller needs a definite answer. They ride a small per-connection channel
//! rather than the shared slot, so one frontend's request cannot displace
//! another's state.
//!
//! # Failure semantics
//!
//! Losing every frontend is not a reason to stop. The daemon keeps polling
//! with no TUI attached, because continuous background observation is the
//! point of the architecture. A dead poll or `EggPool` task *is* a reason to
//! stop: continuing would publish stale state indefinitely, which is
//! indistinguishable from a healthy fleet that happens not to be changing.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, watch, Notify};
use tokio_util::sync::CancellationToken;

use crate::clientd::identity::ClientDaemonIdentity;
use crate::clientd::ipc::{self, BoundEndpoint, Connection, TransportError};
use crate::clientd::protocol::{
    encode_frame, DaemonRequest, FrontendFrame, HelloPayload, PROTOCOL_VERSION,
};
use crate::clientd::snapshot::FrontendSnapshot;
use crate::config::{ConfigError, ConfigStore};
use crate::eggpool::{self, EggpoolDesiredState, EggpoolPeriod};
use crate::scheduler::SchedulerCommand;
use crate::state::{
    now_unix_ms, CronIntent, CronIntents, EggpoolIntent, EggpoolIntents, FleetState,
};

/// How often a connection looks for inbound bytes.
///
/// The connection loop is a single task per frontend, so it polls for
/// readability rather than splitting the socket into separate async read and
/// write halves. That keeps one transport implementation for both platforms.
/// The cost is one timer wake per attached frontend every 20 ms, and the only
/// latency it adds is to a control request the operator just pressed a key
/// for. State delivery is unaffected: it is driven by the watch channel.
const READ_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How often the engine re-reduces frontend `EggPool` intents.
///
/// A disconnect changes the converged worker state even though nobody sends a
/// request, so the reduction cannot live only on the request path.
const INTENT_REDUCE_INTERVAL: Duration = Duration::from_millis(250);

/// Capacity of the per-connection control-ack channel.
///
/// A frontend with this many unacknowledged requests outstanding is not
/// reading. Draining is not an option, because the daemon must not block, so
/// the frame is dropped: a frontend that stopped reading will be closed by the
/// failed state write that follows.
const ACK_CHANNEL_CAPACITY: usize = 8;

/// Capacity of the shared inbound-request channel.
///
/// Requests are rare and small (`Ctrl-R`, an intent update, a stop). A full
/// channel means the engine is wedged, so the sending connection is closed
/// rather than parked behind a queue.
const REQUEST_CHANNEL_CAPACITY: usize = 64;

/// Read buffer for one connection.
const READ_BUFFER_BYTES: usize = 8192;

/// How long the accept loop may take to unwind once its wait is interrupted.
///
/// Generous, and only ever reached on failure: a Unix accept returns to its
/// cancellation check within milliseconds, and an interrupted Windows
/// `ConnectNamedPipe` returns as soon as the cancellation is marked. The bound
/// exists so a transport regression fails as a bounded error instead of a hang.
const ACCEPT_STOP_GRACE: Duration = Duration::from_secs(5);

/// How many buffered state documents `stop` skips while looking for the
/// acknowledgement to its own request.
///
/// A document is never an answer to a stop, but one the handshake left buffered
/// is legitimate traffic rather than a protocol violation, so it is skipped
/// rather than reported. The bound keeps a daemon that keeps publishing from
/// keeping the read alive indefinitely.
const MAX_STOP_SKIPPED_DOCUMENTS: usize = 8;

/// Capacity of the scheduler observation channel.
///
/// Bounded: a full channel means the engine is not draining, and an unbounded
/// one would let a fleet of endpoints accumulate observations faster than they
/// can be applied.
const CRON_CHANNEL_CAPACITY: usize = 64;

/// One publication: the encoded frame plus the generation it carries.
struct Document {
    /// Local IPC generation, mirrored from the encoded document so a
    /// connection can report it without decoding.
    #[allow(dead_code)]
    generation: u64,
    /// Encoded `FrontendFrame::Snapshot`, produced once per publication.
    encoded: Vec<u8>,
}

/// The shared publication cell and control surface.
struct Hub {
    /// Latest complete state document. `watch` is a replacement slot, so
    /// `subscribe` immediately yields the current value and a slow reader
    /// observes the newest generation on its next `changed`.
    snapshots: watch::Sender<Arc<Document>>,
    /// Where connections hand decoded requests to the engine.
    requests: mpsc::Sender<Inbound>,
    /// Per-frontend `EggPool` intents, reduced into one converged worker state.
    intents: Arc<Mutex<EggpoolIntents>>,
    /// Per-frontend cron-detail intents, reduced into the set of history
    /// records the published document carries.
    ///
    /// Separate from the polling plane on purpose: this decides what is
    /// *transmitted*, never what is *fetched*.
    cron_intents: Arc<Mutex<CronIntents>>,
    /// The endpoint list the scheduler worker polls, shared with it.
    ///
    /// A config reload pushes the reconciled list here so the scheduler plane
    /// follows the same single reload boundary as the metrics plane, rather than
    /// discovering endpoints independently.
    cron_endpoints: Arc<Mutex<Vec<crate::endpoint::Endpoint>>>,
    /// Wakes the scheduler worker when the endpoint list changes, so a newly
    /// added system is observed without waiting out the whole interval.
    cron_reload: Arc<Notify>,
    /// Config identity this daemon serves.
    daemon_id: String,
    /// Fires when a frontend asks the daemon to stop.
    shutdown: CancellationToken,
}

/// One decoded request plus the frontend that sent it.
struct Inbound {
    /// Subscriber identity, used to replace that frontend's intent.
    subscriber: u64,
    /// The request itself.
    request: DaemonRequest,
    /// Where to deliver an acknowledgement for this connection.
    acks: mpsc::Sender<FrontendFrame>,
}

/// What a request asked the engine loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    /// Keep running; publication already happened.
    Continue,
    /// Unwind now. The acknowledgement is written by the connection before it
    /// observes the cancellation, so `gregg daemon stop` always sees its own
    /// reply.
    Stop,
}

/// Why the client daemon could not start or could not continue.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// The local endpoint could not be bound.
    #[error("could not bind the client-daemon endpoint: {0}")]
    Bind(#[source] TransportError),
    /// The configuration could not be read.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A published state document could not be encoded.
    #[error("could not encode a state document: {0}")]
    Encode(String),
    /// The poll scheduler task died, so the daemon can no longer observe the
    /// fleet.
    #[error("the poll scheduler stopped unexpectedly")]
    SchedulerStopped,
    /// The `EggPool` worker task died.
    #[error("the EggPool worker stopped unexpectedly")]
    EggpoolWorkerStopped,
}

impl DaemonError {
    /// Whether the failure justifies exiting the whole process.
    ///
    /// A dead poller or a dead `EggPool` worker is not recoverable in place.
    /// A config that cannot be read is the one failure that is *not* fatal to
    /// the process, because the daemon can be retried once the operator
    /// fixes the file and a later attach can restart it.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        !matches!(self, Self::Config(_))
    }
}

/// Run the client daemon in the foreground until `cancel` fires.
///
/// This never forks or self-daemonizes: a supervisor (or Plan 165's user
/// startup registration) owns the process lifetime, and a test harness can
/// drive it directly.
pub async fn run_daemon(
    store: ConfigStore,
    identity: ClientDaemonIdentity,
    cancel: CancellationToken,
) -> Result<(), DaemonError> {
    let config = store.load_or_default()?;
    let config_path = store.path().to_path_buf();
    let mut fleet = FleetState::from_config(&config);

    let timeout = Duration::from_millis(config.request_timeout_ms);
    let refresh = Duration::from_secs(config.refresh_seconds);
    let max_concurrent = config.max_concurrent_requests as usize;
    let endpoints: Vec<crate::endpoint::Endpoint> = config
        .systems
        .iter()
        .map(crate::config::SystemEntry::to_endpoint)
        .collect();

    // A token dedicated to the poll and worker tasks, so a stop request
    // unwinds them even while the main loop is still acknowledging the caller
    // that asked for it.
    let tasks = CancellationToken::new();
    let (scheduler_tx, scheduler_rx) = mpsc::channel::<SchedulerCommand>(4);
    let mut batch_rx = Some(
        crate::scheduler::PollScheduler::new(
            crate::clock::RealClock,
            crate::poller::HttpClient::new(timeout),
            refresh,
            max_concurrent,
        )
        .run(endpoints.clone(), tasks.clone(), scheduler_rx),
    );

    let eggpool_client = eggpool::EggpoolClient::new(timeout);
    // The worker itself is wired by `Engine::sync_eggpool_worker`, so a reload
    // that adds or removes the entry is honored instead of being frozen into
    // this process's startup config.
    let mut eggpool_results: Option<mpsc::Receiver<eggpool::EggpoolResult>> = None;

    // Bind before the first document is published, so a frontend can never
    // observe a daemon that is not yet serving.
    let candidates = identity.candidates();
    let listener = ipc::bind(&candidates).map_err(DaemonError::Bind)?;
    let endpoint: BoundEndpoint = listener.endpoint().clone();

    let (snapshots, _) = watch::channel(encode_document(&fleet, 1, &CronIntents::default())?);
    let (requests, mut request_rx) = mpsc::channel(REQUEST_CHANNEL_CAPACITY);
    // Capacity of the scheduler observation channel. Bounded because a full
    // channel means the engine is not draining, and an unbounded one would let
    // a slow engine accumulate observations faster than they can be applied.
    let (cron_tx, mut cron_rx) = mpsc::channel(CRON_CHANNEL_CAPACITY);
    let cron_endpoints = Arc::new(Mutex::new(endpoints));
    let cron_reload = Arc::new(Notify::new());

    let hub = Arc::new(Hub {
        snapshots,
        requests,
        intents: Arc::new(Mutex::new(EggpoolIntents::default())),
        cron_intents: Arc::new(Mutex::new(CronIntents::default())),
        cron_endpoints: Arc::clone(&cron_endpoints),
        cron_reload: Arc::clone(&cron_reload),
        daemon_id: identity.id().to_owned(),
        shutdown: tasks.clone(),
    });

    // Taken before the listener is moved into the accept task: that task is the
    // only other owner of the listener, and shutdown is driven from here.
    let accept_stop = listener.stop_handle();
    let mut accept_task = tokio::spawn({
        let hub = Arc::clone(&hub);
        let cancel = tasks.clone();
        async move { accept_loop(listener, hub, cancel).await }
    });

    // The scheduler plane runs whether or not a TUI is attached, because
    // continuous background observation is the point of the architecture.
    let cron_task = tokio::spawn(crate::clientd::cron::CronWorker::new(timeout).run(
        cron_endpoints,
        cron_reload,
        cron_tx,
        tasks.clone(),
    ));

    let mut engine = Engine {
        // The initial document was already published at generation 1 below, so
        // the counter starts there rather than producing a duplicate 1 on the
        // first real change.
        generation: 1,
        converged: fleet.eggpool_desired_state(&EggpoolIntents::default()),
        eggpool_worker: None,
        eggpool_client,
        cron_dirty: false,
        cron_revision: 0,
        store: ConfigStore::new(config_path),
    };
    // The entry the config already names is wired before the loop, so a
    // configured `EggPool` endpoint is live from the first document.
    engine.sync_eggpool_worker(&fleet, &tasks, &mut eggpool_results);

    let result = engine
        .run(
            &hub,
            &mut fleet,
            &mut batch_rx,
            &mut eggpool_results,
            &scheduler_tx,
            &mut request_rx,
            &mut cron_rx,
            &cancel,
            &tasks,
        )
        .await;

    tasks.cancel();
    cron_task.abort();
    // Stop accepting *before* the endpoint is released, so no client can be
    // handed a connection by a listener that is on its way out.
    //
    // The interrupt matters on Windows, where the accept wait is parked in a
    // blocking-pool thread that `abort` cannot reach: requesting the stop ends
    // that wait, so the accept loop unwinds and the runtime has nothing left to
    // wait for when it is dropped. Aborting alone would drop the task's future
    // while the thread kept waiting for a client that is never coming, and the
    // process would hang instead of exiting. On Unix the request is a no-op and
    // the loop reaches its own cancellation check in milliseconds.
    accept_stop.request();
    if tokio::time::timeout(ACCEPT_STOP_GRACE, &mut accept_task)
        .await
        .is_err()
    {
        // Last resort, and the old defective path on Windows: it cannot cancel a
        // `spawn_blocking` job that is already running. Kept so a transport
        // regression surfaces as a bounded failure rather than a silent hang.
        accept_task.abort();
    }
    ipc::cleanup(&endpoint.path);
    result
}

impl Hub {
    /// Apply one change to the frontend intent set.
    ///
    /// A poisoned lock still holds usable data: the guard is only poisoned if
    /// a previous holder panicked mid-update, and every mutation here is a
    /// single push or retain. Failing closed on a panic would stop the
    /// `EggPool` worker from ever converging again, which is strictly worse
    /// than converging from a possibly-stale set.
    fn with_cron_intents<T>(&self, edit: impl FnOnce(&mut CronIntents) -> T) -> T {
        let mut guard = self
            .cron_intents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        edit(&mut guard)
    }

    /// The reduced cron-detail intents the next document will honour.
    fn cron_intents(&self) -> CronIntents {
        self.cron_intents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn with_intents<T>(&self, edit: impl FnOnce(&mut EggpoolIntents) -> T) -> T {
        let mut guard = self
            .intents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        edit(&mut guard)
    }
}

/// The live `EggPool` worker together with the entry it was spawned for.
struct EggpoolWorkerHandle {
    endpoint: crate::config::EggpoolEntry,
    control: eggpool::EggpoolControl,
}

/// Mutable engine bookkeeping that is not part of the fleet.
struct Engine {
    /// Local IPC generation of the most recent publication.
    generation: u64,
    /// The `EggPool` desired state the worker currently holds.
    converged: Option<EggpoolDesiredState>,
    /// The worker currently serving the configured `EggPool` entry.
    ///
    /// Owned by the engine and re-checked against the config on every reload,
    /// rather than derived once at startup. `Ctrl-R` is the daemon's only
    /// config boundary and an operator can add or remove `[eggpool]` at it;
    /// derived only at startup, *adding* one produced a document carrying a
    /// pane with no worker behind it, so every request moved it to
    /// `Refreshing` and nothing ever resolved it.
    eggpool_worker: Option<EggpoolWorkerHandle>,
    /// The client the worker is spawned with, reused across reloads so a
    /// rewire keeps the daemon's configured timeout.
    eggpool_client: eggpool::EggpoolClient,
    /// Whether a cron-detail intent changed since the last publication.
    ///
    /// Tracked separately from the cache because an intent change alters what
    /// the *document* carries without altering the cache at all.
    cron_dirty: bool,
    /// The intent-set revision this engine last published at.
    ///
    /// A departing frontend's intent is removed by its connection task, which
    /// has no engine handle and cannot raise `cron_dirty` itself. Comparing the
    /// revision on the reduce tick covers every mutation, wherever it was made,
    /// so a disconnect stops transmitting unread history instead of waiting for
    /// an unrelated change to republish.
    cron_revision: u64,
    /// The config file this daemon reloads.
    ///
    /// Resolved from the daemon's own startup path, never from a
    /// frontend-supplied one, so no frontend can redirect the daemon onto a
    /// different configuration.
    store: ConfigStore,
}

impl Engine {
    /// The daemon's main loop: mutate fleet state, publish on real change.
    #[allow(clippy::too_many_arguments)]
    async fn run(
        &mut self,
        hub: &Arc<Hub>,
        fleet: &mut FleetState,
        batch_rx: &mut Option<mpsc::Receiver<crate::poller::PollBatch>>,
        eggpool_results: &mut Option<mpsc::Receiver<eggpool::EggpoolResult>>,
        scheduler_tx: &mpsc::Sender<SchedulerCommand>,
        request_rx: &mut mpsc::Receiver<Inbound>,
        cron_rx: &mut mpsc::Receiver<crate::clientd::cron::CronObservation>,
        cancel: &CancellationToken,
        tasks: &CancellationToken,
    ) -> Result<(), DaemonError> {
        let mut stop = Control::Continue;
        let mut tick = tokio::time::interval(INTENT_REDUCE_INTERVAL);

        while stop == Control::Continue {
            let mut dirty = false;
            tokio::select! {
                () = cancel.cancelled() => {
                    stop = Control::Stop;
                }
                () = hub.shutdown.cancelled() => {
                    // A frontend asked the daemon to stop. Its acknowledgement
                    // is already queued on its own connection, so exiting here
                    // cannot lose it.
                    stop = Control::Stop;
                }

                // An empty system list produces no batches at all, so the
                // receiver is retired rather than left pending forever.
                maybe = recv_opt(batch_rx), if batch_rx.is_some() => {
                    let was_initialized = fleet.last_applied_generation != 0;
                            match maybe {
                        Some(batch) => dirty |= fleet.apply_batch_owned_changed(batch),
                        None => *batch_rx = None,
                    }
                    // The transition from "never polled" to "polled at least
                    // once" is published even when it changed nothing visible.
                    // A frontend places its selection once, on the first
                    // document that carries reachability; if that document
                    // were suppressed, the placement would instead fire on
                    // some later, unrelated change and undo whatever the
                    // operator had selected in the meantime. This costs one
                    // publication in the daemon's entire life, not one per
                    // poll.
                    dirty |= was_initialized != (fleet.last_applied_generation != 0);
                }

                maybe = recv_opt(eggpool_results), if eggpool_results.is_some() => {
                    if let Some(result) = maybe {
                        dirty |= fleet.apply_eggpool_result_changed(&result);
                    } else {
                        fleet.mark_eggpool_worker_unavailable();
                        *eggpool_results = None;
                        dirty = true;
                    }
                }

                // A finished scheduler observation. It is applied to the cron
                // cache only, and it can never turn a system offline: the
                // metrics plane owns reachability, and a cron route failure on
                // an otherwise healthy system must not say otherwise.
                //
                // A late answer for a replaced endpoint is dropped here rather
                // than in the worker, because this is where the current
                // configuration lives: the worker knows what it polled, and only
                // the fleet knows what is still configured.
                Some(observation) = cron_rx.recv() => {
                    // Silently on purpose, like every other best-effort nudge
                    // in this daemon: a discarded answer is not an operator
                    // problem, and the frontend never learns that a repoint
                    // happened.
                    if fleet.accepts_cron_observation(&observation) {
                        dirty |= observation.apply(&mut fleet.cron);
                    }
                }

                Some(inbound) = request_rx.recv() => {
                    // Synchronous by design: this runs on the single task that
                    // owns the fleet, applies cron/EggPool results, and writes
                    // every frontend's document, so it must never park.
                    stop = self.handle_request(fleet, hub, scheduler_tx, inbound);
                    // A reload may have added, removed, or repointed the
                    // `EggPool` entry, so the worker is re-checked against the
                    // config it now holds rather than the one this process
                    // started with.
                    self.sync_eggpool_worker(fleet, tasks, eggpool_results);
                    dirty = true;
                }

                _ = tick.tick() => {
                    dirty |= self.reduce_intents(fleet, hub);
                    dirty |= self.note_cron_intent_revision(hub);
                }
            }

            // An intent change alters the document without touching the cache,
            // so it is a publication reason in its own right.
            dirty |= std::mem::take(&mut self.cron_dirty);
            if dirty && stop == Control::Continue {
                self.generation = self.generation.saturating_add(1);
                let document = encode_document(fleet, self.generation, &hub.cron_intents())?;
                // `send_replace`, not `send`: `send` fails when the channel has
                // no receivers and **discards the value** when it does. A
                // document published while no TUI is attached would therefore
                // be dropped, and a frontend attaching later would be handed
                // the stale document from bind time and then never told again,
                // because a quiet fleet produces no further publications. The
                // value is stored unconditionally here, so "attach later" and
                // "attach now" see the same state.
                hub.snapshots.send_replace(document);
            }
        }
        Ok(())
    }

    /// Recompute the converged `EggPool` worker state from every attached
    /// frontend's intent, publishing only on a real change.
    fn reduce_intents(&mut self, fleet: &mut FleetState, hub: &Arc<Hub>) -> bool {
        let Some(control) = self.eggpool_worker.as_ref().map(|worker| &worker.control) else {
            return false;
        };
        // A poisoned lock still holds usable data: the guard is only poisoned
        // if a previous holder panicked mid-update, and `set`/`remove` are
        // single pushes. Losing the whole daemon over that would be a worse
        // outcome than converging from a possibly-stale intent set.
        let guard = hub
            .intents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let intents: EggpoolIntents = guard.clone();
        let Some(desired) = fleet.eggpool_desired_state(&intents) else {
            return false;
        };
        if self.converged == Some(desired) {
            return false;
        }
        // The fleet must carry the window the worker is actually driven with on
        // *every* path that changes it, not only on the request path. A
        // disconnect has no request of its own, so this is where a departing
        // frontend's removal actually lands: without it the converged period
        // moves while `fleet.eggpool.period` stays on the old one, and the
        // reducer then rejects every result the worker fetches — the pane
        // freezes and the worker burns a request per interval, all discarded.
        if fleet.set_eggpool_period(desired.period) {
            // A new window needs a fresh worker generation, or the worker
            // returns the previous generation's result and the reducer rejects
            // it as stale.
            fleet.begin_eggpool_request();
        }
        // Derive what to publish *after* the fleet updates, so the worker is
        // driven with the generation the reducer will accept. Publishing the
        // pre-bump generation instead made the worker fetch a result that was
        // rejected on arrival, and the next tick then republished at the real
        // generation — which the worker treats as superseding, so it aborted
        // the fetch it had just started and issued a second one. One wasted
        // round trip per event, on every pane.
        let desired = fleet.eggpool_desired_state(&intents).unwrap_or(desired);
        if control.publish(desired).is_err() {
            // The worker's control channel is closed, and no amount of
            // re-publishing reopens it. Recording the converged state anyway
            // bounds this to one publication: without it, every 250ms tick
            // re-detected the same failure, marked the pane unavailable again,
            // and re-encoded and broadcast a full fleet document forever. A
            // *different* intent still retries, because it is a different
            // desired state, so this is not a permanent mute.
            fleet.mark_eggpool_worker_unavailable();
            self.converged = Some(desired);
            return true;
        }
        self.converged = Some(desired);
        // The fleet must carry the window the worker is actually driven with on
        // *every* path that changes it, not only on the request path. A
        // disconnect has no request of its own, so this is where a departing
        // frontend's removal actually lands: without it the converged period
        // moves while `fleet.eggpool.period` stays on the old one, and the
        // reducer then rejects every result the worker fetches — the pane
        // freezes and the worker burns a request per interval, all discarded.
        if fleet.set_eggpool_period(desired.period) {
            // A new window needs a fresh worker generation, or the worker
            // returns the previous generation's result and the reducer rejects
            // it as stale.
            fleet.begin_eggpool_request();
        }
        false
    }

    /// Republish when the cron-intent set changed outside a frontend request.
    ///
    /// A departing frontend's intent is removed by its connection task, which
    /// has no engine handle and cannot raise `cron_dirty` itself. Comparing the
    /// set's revision covers every mutation wherever it was made, so the
    /// departed window's history stops being transmitted instead of riding
    /// along until some unrelated change republishes.
    fn note_cron_intent_revision(&mut self, hub: &Arc<Hub>) -> bool {
        let revision = hub.cron_intents().revision();
        if revision == self.cron_revision {
            return false;
        }
        self.cron_revision = revision;
        self.cron_dirty = true;
        true
    }

    /// Wire or drop the `EggPool` worker to match the current configuration.
    ///
    /// Called after every frontend request, because `Ctrl-R` is the daemon's
    /// only config boundary and the operator may add, remove, or repoint the
    /// entry there. Removing the entry already worked by accident — the worker
    /// simply stopped being driven — while *adding* one left a pane that no
    /// worker could ever answer, which is a far worse failure than none at all.
    ///
    /// Dropping the control handle closes the worker's watch channel, which is
    /// how the old worker is told to stop, and its result receiver is retired
    /// with it. A fresh worker starts inactive, so the converged memo is
    /// cleared as well: the next intent tick republishes and drives it.
    fn sync_eggpool_worker(
        &mut self,
        fleet: &FleetState,
        tasks: &CancellationToken,
        eggpool_results: &mut Option<mpsc::Receiver<eggpool::EggpoolResult>>,
    ) {
        let wanted = fleet.eggpool.as_ref().map(|state| state.endpoint.clone());
        if self.eggpool_worker.as_ref().map(|w| &w.endpoint) == wanted.as_ref() {
            return;
        }
        self.eggpool_worker = None;
        *eggpool_results = None;
        self.converged = None;
        let Some(entry) = wanted else {
            return;
        };
        let worker =
            eggpool::spawn_worker(self.eggpool_client.clone(), entry.clone(), tasks.clone());
        *eggpool_results = Some(worker.results);
        self.eggpool_worker = Some(EggpoolWorkerHandle {
            endpoint: entry,
            control: worker.control,
        });
    }

    /// Apply one frontend request, returning whether the daemon should stop.
    fn handle_request(
        &mut self,
        fleet: &mut FleetState,
        hub: &Arc<Hub>,
        scheduler_tx: &mpsc::Sender<SchedulerCommand>,
        inbound: Inbound,
    ) -> Control {
        let Inbound {
            subscriber,
            request,
            acks,
        } = inbound;

        let mut stop = Control::Continue;
        let (ack_generation, accepted, detail) = match request {
            // Handled by the connection before it ever reaches the engine.
            DaemonRequest::Handshake { .. } => return stop,
            DaemonRequest::ReloadConfig { generation } => {
                let (accepted, detail) = self.reload_config(fleet, hub, scheduler_tx);
                (generation, accepted, detail)
            }
            DaemonRequest::SetEggpoolIntent {
                active,
                period,
                refresh,
                generation,
            } => {
                let previous = hub.with_intents(|guard| guard.set(subscriber, active, period));
                // A new window, a newly opened pane, or an explicit manual
                // refresh each need a fresh worker generation; without one the
                // worker would return the previous generation's result and the
                // reducer would reject it as stale. An identical re-send of
                // the same intent is a no-op, which is what keeps a
                // reconnecting frontend from minting generations forever.
                let changed = previous != Some(EggpoolIntent { active, period });
                // Fleet-global state must carry the window the worker is
                // actually driven with: the converged reduction over every
                // attached frontend, not this one frontend's request. Writing
                // a single frontend's period here would make the reducer
                // reject every result the worker fetched as soon as two
                // frontends disagreed, leaving both panes in `Refreshing`
                // with no error.
                let converged = hub.with_intents(|intents| fleet.eggpool_desired_state(intents));
                let window_changed =
                    converged.is_some_and(|desired| fleet.set_eggpool_period(desired.period));
                if window_changed || changed || refresh {
                    fleet.begin_eggpool_request();
                }
                (generation, true, None)
            }
            DaemonRequest::SetCronIntent {
                system_id,
                job,
                display_history,
                generation,
            } => {
                // A replacement, never a delta: a stale intent composed on top
                // of a newer one would keep the daemon transmitting records for
                // a pane the operator has already closed.
                let intent = CronIntent {
                    system_id,
                    job,
                    display_history,
                };
                let previous =
                    hub.with_cron_intents(|intents| intents.set(subscriber, intent.clone()));
                if previous.as_ref() != Some(&intent) {
                    self.cron_dirty = true;
                }
                // Keep the memo in step so the tick does not republish a
                // change this request already accounted for.
                self.cron_revision = hub.cron_intents().revision();
                (generation, true, None)
            }
            DaemonRequest::Shutdown { generation } => {
                stop = Control::Stop;
                (generation, true, None)
            }
        };

        let frame = FrontendFrame::ControlAck {
            generation: ack_generation,
            accepted,
            detail,
        };
        // `try_send` rather than `send`: a frontend that stopped reading must
        // not be able to stall the engine loop. The connection's own state
        // write is what will notice and close it.
        let _ = acks.try_send(frame);
        stop
    }

    /// Re-read the config and reconcile, or keep the last-known-good fleet.
    fn reload_config(
        &mut self,
        fleet: &mut FleetState,
        hub: &Arc<Hub>,
        scheduler_tx: &mpsc::Sender<SchedulerCommand>,
    ) -> (bool, Option<String>) {
        match self.store.load_existing() {
            Ok(config) => {
                let endpoints: Vec<crate::endpoint::Endpoint> = config
                    .systems
                    .iter()
                    .map(crate::config::SystemEntry::to_endpoint)
                    .collect();
                fleet.reconcile_systems(&config);
                match config.eggpool.clone() {
                    Some(entry) => fleet.adopt_eggpool_endpoint(entry),
                    None => fleet.clear_eggpool(),
                }
                fleet.clear_config_reload_error();
                // A system that left the fleet must not keep its cron history
                // alive for the daemon's whole life, and the global bound is
                // better spent on live systems.
                let live: Vec<String> = fleet
                    .systems
                    .iter()
                    .map(|system| system.id.clone())
                    .collect();
                fleet.cron.retain_systems(&live);
                // The scheduler plane follows the same reload boundary as the
                // metrics plane, so it never discovers endpoints independently.
                // The list is needed by both the scheduler plane and the
                // metrics plane, so one copy is genuinely required rather than
                // incidental.
                let for_cron = endpoints.clone();
                *hub.cron_endpoints
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = for_cron;
                hub.cron_reload.notify_one();
                // Accepted config that changes endpoints polls immediately
                // rather than waiting out the remaining cadence.
                request_scheduler_poll(scheduler_tx, SchedulerCommand::ReplaceEndpoints(endpoints));
                (true, None)
            }
            Err(error) => {
                let message = format!("config reload failed: {error}");
                fleet.set_config_reload_error(message.clone());
                // Still poll: a temporarily invalid file must not freeze
                // metrics that were already being collected.
                request_scheduler_poll(scheduler_tx, SchedulerCommand::Refresh);
                (false, Some(message))
            }
        }
    }
}

/// Ask the poll task for an early poll, without ever parking the caller.
///
/// A `SchedulerCommand` is a *request*, never state: the poll task keeps the
/// endpoint list it was given and applies the command when it next receives
/// one, and `Refresh` only asks to skip ahead to the next poll. The channel is
/// bounded and the scheduler reads commands only *between* generations, so a
/// full channel simply means a generation is in flight — and the fixed cadence
/// polls anyway.
///
/// A blocking `send().await` on this path would park the single engine task
/// that owns the fleet, applies cron and `EggPool` results, and writes every
/// frontend's document. That freezes every attached TUI for the rest of the
/// generation and leaves a `Shutdown` request unread, so a refusal here costs
/// at most the early poll — never the fan-out.
///
/// Silent by construction: the client daemon has no subscriber, so a dropped
/// early poll is indistinguishable in the product from the cadence poll it
/// replaces.
fn request_scheduler_poll(
    scheduler_tx: &mpsc::Sender<SchedulerCommand>,
    command: SchedulerCommand,
) {
    let _ = scheduler_tx.try_send(command);
}

fn encode_document(
    fleet: &FleetState,
    generation: u64,
    cron_intents: &CronIntents,
) -> Result<Arc<Document>, DaemonError> {
    let dto = fleet.to_dto_for(
        std::time::Instant::now(),
        now_unix_ms(),
        generation,
        cron_intents,
    );
    let frame = FrontendFrame::Snapshot(Box::new(dto));
    let encoded = encode_frame(&frame).map_err(|error| DaemonError::Encode(error.to_string()))?;
    Ok(Arc::new(Document {
        generation,
        encoded,
    }))
}

async fn recv_opt<T>(receiver: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Accept connections until cancelled.
async fn accept_loop(mut listener: ipc::Listener, hub: Arc<Hub>, cancel: CancellationToken) {
    let mut next_subscriber = 1_u64;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match listener.accept().await {
            Ok(connection) => {
                let subscriber = next_subscriber;
                next_subscriber = next_subscriber.wrapping_add(1).max(1);
                let hub = Arc::clone(&hub);
                tokio::spawn(async move {
                    serve(connection, hub, subscriber).await;
                });
            }
            // No connection is waiting. That is the normal state, not an error.
            Err(TransportError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // A pipe instance can legitimately be taken by a client that
            // disconnects between instance creation and the connect call.
            Err(TransportError::Disconnected) => {}
            Err(_) => return,
        }
    }
}

/// Serve one frontend for its whole lifetime.
async fn serve(mut connection: Connection, hub: Arc<Hub>, subscriber: u64) {
    if let Err(error) = serve_inner(&mut connection, &hub, subscriber).await {
        // One frontend's protocol failure is that frontend's problem. The
        // daemon keeps polling and other frontends keep their state.
        //
        // The write is best-effort: a frontend that has already gone away
        // cannot be told anything, and that is not a second failure worth
        // reporting over the first.
        let _ = connection.write_frame(&FrontendFrame::ProtocolError {
            message: error.to_string(),
        });
    }
    // A departed frontend must stop shaping the `EggPool` worker, or the
    // worker would stay activated for a window nobody is watching.
    hub.with_intents(|intents| intents.remove(subscriber));
    // Same for cron: a departed window that left its intent behind would keep
    // the daemon transmitting history records nobody is reading. This task has
    // no engine handle, so it cannot raise a publication trigger itself; the
    // revision this bumps is what makes the engine's reduce tick republish.
    hub.with_cron_intents(|intents| intents.remove(subscriber));
}

async fn serve_inner(
    connection: &mut Connection,
    hub: &Arc<Hub>,
    subscriber: u64,
) -> Result<(), TransportError> {
    let mut snapshots = hub.snapshots.subscribe();
    let (ack_tx, mut ack_rx) = mpsc::channel(ACK_CHANNEL_CAPACITY);
    let mut handshake_done = false;
    let mut read_buffer = vec![0_u8; READ_BUFFER_BYTES];

    loop {
        if !handshake_done {
            if let Some(request) = connection.try_read_request()? {
                match request {
                    DaemonRequest::Handshake { .. } => {
                        // `try_read_request` already refused a protocol
                        // version this daemon does not speak; this is the
                        // config-identity check, so a TUI cannot attach to a
                        // daemon serving a different configuration.
                        connection.verify_daemon_id(&hub.daemon_id)?;
                        handshake_done = true;
                        connection.write_frame(&FrontendFrame::Hello(Box::new(HelloPayload {
                            protocol_version: PROTOCOL_VERSION,
                            version: env!("CARGO_PKG_VERSION").to_owned(),
                            daemon_id: hub.daemon_id.clone(),
                            current_generation: snapshots.borrow().generation,
                        })))?;
                        // A new subscriber gets the complete current state
                        // immediately: an empty TUI must never have to wait for
                        // the next poll generation to learn what is configured.
                        let current = Arc::clone(&snapshots.borrow());
                        connection.write_encoded(&current.encoded)?;
                        hub.with_intents(|intents| {
                            intents.set(subscriber, false, EggpoolPeriod::Hour);
                        });
                        // A new window starts with the cron pane closed. A
                        // reconnected frontend must not inherit the pane state
                        // of the connection it replaced.
                        hub.with_cron_intents(|intents| {
                            intents.set(
                                subscriber,
                                CronIntent {
                                    system_id: None,
                                    job: None,
                                    display_history: 0,
                                },
                            );
                        });
                    }
                    other => {
                        return Err(TransportError::Protocol(format!(
                            "expected a handshake, got {other:?}"
                        )));
                    }
                }
            }
        } else if let Some(request) = connection.try_read_request()? {
            // `try_send`, never `send`: a full channel means the engine is
            // wedged, and blocking here would let one frontend hold the
            // accept loop hostage.
            if hub
                .requests
                .try_send(Inbound {
                    subscriber,
                    request,
                    acks: ack_tx.clone(),
                })
                .is_err()
            {
                return Err(TransportError::Protocol(
                    "the client daemon is not accepting requests".to_owned(),
                ));
            }
        }

        let mut write_state = false;
        tokio::select! {
            changed = snapshots.changed() => {
                if changed.is_err() {
                    // The daemon is gone. Report the reason rather than
                    // looking like a network failure.
                    let _ = connection.write_frame(&FrontendFrame::ShuttingDown);
                    return Ok(());
                }
                // Do not publish before the handshake has been answered. The
                // handshake reply already carries the current state, and a
                // frontend identifies its daemon by the *first* frame, so a
                // document overtaking the `Hello` would make a healthy daemon
                // look like a refusing one. Dropping the notification is safe
                // rather than lossy: the handshake reads the latest value
                // itself, and `borrow` does not mark it seen, so the next real
                // publication still wakes this branch.
                write_state = handshake_done;
            }
            Some(frame) = ack_rx.recv() => {
                // A failed ack write means the peer is gone; there is no
                // partial frame to salvage, so the connection ends here.
                connection.write_frame(&frame)?;
            }
            () = tokio::time::sleep(READ_POLL_INTERVAL) => {
                // Push whatever the peer can take. A frontend that stalled and
                // came back then finishes the document it was mid-way through
                // receiving, instead of waiting for the next publication — the
                // unsent tail is the only thing that may not be dropped.
                connection.flush_outbound()?;
                match connection.read_available(&mut read_buffer) {
                    Ok(count) => connection.push_bytes(&read_buffer[..count])?,
                    // Nothing yet. The loop retries on its next tick.
                    Err(TransportError::WouldBlock) => {}
                    Err(TransportError::Disconnected) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        }

        if write_state {
            let latest = Arc::clone(&snapshots.borrow_and_update());
            connection.write_encoded(&latest.encoded)?;
        }
    }
}

/// Why a frontend could not attach to a client daemon.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// No compatible daemon answered on any candidate endpoint.
    #[error("no client daemon is listening for this config: {0}")]
    Transport(#[source] TransportError),
    /// The endpoint accepted a connection but never identified itself.
    #[error("the client daemon did not complete a handshake in time")]
    HandshakeTimeout,
    /// The endpoint closed during the handshake.
    #[error("the client daemon closed the connection during the handshake")]
    Disconnected,
    /// The daemon speaks a different local protocol version.
    #[error(
        "the client daemon for this config speaks local protocol {daemon}, but this frontend speaks {frontend}; stop it with `gregg daemon stop` and start it again with the current binary",
        daemon = .daemon,
        frontend = .frontend
    )]
    VersionMismatch {
        /// Local protocol version the daemon speaks.
        daemon: u16,
        /// Local protocol version this frontend speaks.
        frontend: u16,
    },
    /// The daemon refused this frontend for a stated reason.
    #[error("the client daemon refused this frontend: {0}")]
    Refused(String),
}

impl AttachError {
    /// Whether the endpoint is simply not there.
    ///
    /// Only a connect-level failure counts as "absent". A version mismatch, an
    /// identity mismatch, or a malformed peer is a *live* endpoint that must
    /// never authorize a competing daemon.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Transport(_))
    }
}

/// An established attachment to a running client daemon.
pub struct Attachment {
    /// The send side for control requests.
    pub frontend: crate::clientd::frontend::FrontendSender,
    /// The read side of the connection, and the only socket owner.
    pub frames: crate::clientd::frontend::FrameStream,
    /// What the daemon said about itself.
    pub hello: HelloPayload,
}

/// Attach to the client daemon for one configuration.
///
/// Plan 164's contract is narrow on purpose. A frontend that cannot reach a
/// compatible daemon must **fail** rather than quietly resume direct remote
/// polling: a silent fallback would recreate the duplicate ownership this
/// architecture removes, and would hide the daemon's failure from the operator
/// entirely. Plan 165 turns an *absent* endpoint into a bounded lazy launch; it
/// does not relax anything here.
pub async fn attach(
    identity: &ClientDaemonIdentity,
    version: &str,
) -> Result<Attachment, AttachError> {
    let candidates = identity.candidates();
    let mut link = crate::clientd::frontend::FrontendLink::connect(&candidates)
        .map_err(AttachError::Transport)?;
    link.handshake(identity, version)
        .map_err(AttachError::Transport)?;
    let (sender, mut stream) = link.split();

    // Wait for the daemon to identify itself before trusting anything else.
    let first = stream.next_terminal().await.map_err(|error| match error {
        crate::clientd::frontend::FrontError::Timeout => AttachError::HandshakeTimeout,
        crate::clientd::frontend::FrontError::Disconnected => AttachError::Disconnected,
        other => AttachError::Refused(other.to_string()),
    })?;
    match first {
        FrontendFrame::Hello(hello) => Ok(Attachment {
            frontend: sender,
            frames: stream,
            hello: *hello,
        }),
        FrontendFrame::VersionMismatch(payload) => Err(AttachError::VersionMismatch {
            daemon: payload.daemon,
            frontend: payload.frontend,
        }),
        FrontendFrame::ProtocolError { message } => Err(AttachError::Refused(message)),
        other => Err(AttachError::Refused(format!(
            "expected a hello frame, got {other:?}"
        ))),
    }
}

/// What `gregg daemon status` reports about one configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    /// The config identity the answer is about.
    pub daemon_id: String,
    /// Whether a compatible daemon answered the handshake.
    pub running: bool,
    /// The daemon's workspace version, when it answered.
    pub version: Option<String>,
    /// The local protocol version the daemon speaks.
    pub protocol_version: Option<u16>,
    /// The endpoint a client would dial, for diagnostics.
    pub endpoint: PathBuf,
    /// Why the daemon is not usable, when it is not running.
    pub detail: Option<String>,
}

impl DaemonStatus {
    /// Render one bounded, operator-readable line.
    #[must_use]
    pub fn render(&self) -> String {
        if self.running {
            format!(
                "client daemon {} running (protocol {}, version {}) at {}",
                self.daemon_id,
                self.protocol_version.unwrap_or_default(),
                self.version.as_deref().unwrap_or("unknown"),
                self.endpoint.display()
            )
        } else {
            format!(
                "client daemon {} not running at {}: {}",
                self.daemon_id,
                self.endpoint.display(),
                self.detail
                    .as_deref()
                    .unwrap_or("no client daemon answered")
            )
        }
    }
}

/// Report whether a matching client daemon is running.
///
/// Read-only and bounded: one dial, one handshake, one answer. Identity comes
/// from the protocol handshake, never from a process name or a PID file, so the
/// answer cannot be a guess about an unrelated process that happens to share a
/// name.
pub async fn status(identity: &ClientDaemonIdentity) -> Result<DaemonStatus, AttachError> {
    let endpoint = identity
        .candidates()
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("<unresolved>"));
    match attach(identity, env!("CARGO_PKG_VERSION")).await {
        Ok(attachment) => Ok(DaemonStatus {
            daemon_id: attachment.hello.daemon_id.clone(),
            running: true,
            version: Some(attachment.hello.version.clone()),
            protocol_version: Some(attachment.hello.protocol_version),
            endpoint,
            detail: None,
        }),
        Err(error) => Ok(DaemonStatus {
            daemon_id: identity.id().to_owned(),
            running: false,
            version: None,
            protocol_version: None,
            endpoint,
            detail: Some(error.to_string()),
        }),
    }
}

/// Stop the matching client daemon.
///
/// Only a positively identified daemon serving this same configuration is
/// asked to stop. A foreign endpoint, a different config's daemon, and a
/// protocol mismatch are all reported rather than forced: killing something
/// that merely looks similar is exactly the failure mode ownership checks
/// exist to prevent.
pub async fn stop(identity: &ClientDaemonIdentity, version: &str) -> Result<(), AttachError> {
    let mut attachment = attach(identity, version).await?;
    attachment
        .frontend
        .request_shutdown(1)
        .map_err(|error| AttachError::Refused(error.to_string()))?;
    // A state document the handshake left buffered is not an answer, and
    // neither is a protocol violation or a timeout: only the acknowledgement
    // or a clean disconnect is. Anything else used to be reported as a
    // successful stop, which is the unsound signal the endpoint check below
    // exists to backstop. The skip is bounded so a daemon that keeps
    // publishing cannot keep this read alive forever.
    let deadline = tokio::time::Instant::now() + crate::clientd::frontend::TERMINAL_FRAME_TIMEOUT;
    let mut skipped = 0;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(frame) = tokio::time::timeout(remaining, attachment.frames.next()).await else {
            return Err(AttachError::Refused(
                "the client daemon did not answer the stop request in time".to_owned(),
            ));
        };
        match frame {
            Ok(FrontendFrame::Snapshot(_)) if skipped < MAX_STOP_SKIPPED_DOCUMENTS => {
                skipped += 1;
            }
            Ok(FrontendFrame::ControlAck {
                accepted, detail, ..
            }) => {
                break if accepted {
                    Ok(())
                } else {
                    Err(AttachError::Refused(detail.unwrap_or_else(|| {
                        "the daemon refused the stop request".to_owned()
                    })))
                };
            }
            // The daemon closes after honouring a stop, so a clean disconnect
            // immediately after the request is a success, not a failure.
            Err(error) if error.is_disconnect() => break Ok(()),
            Err(error) => break Err(AttachError::Refused(error.to_string())),
            Ok(other) => {
                break Err(AttachError::Refused(format!(
                    "the client daemon answered the stop request with {other:?} instead of an acknowledgement"
                )));
            }
        }
    }?;

    // Wait for the endpoint to go away. Returning on the acknowledgement alone
    // would report success for a daemon that is still unwinding, and the very
    // next command would find it still listening.
    for _ in 0..200 {
        if identity
            .candidates()
            .iter()
            .all(|path| !ipc::endpoint_is_live(path))
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(AttachError::Refused(
        "the client daemon accepted the stop but is still listening".to_owned(),
    ))
}

/// The state a frontend starts from when no daemon has answered yet.
///
/// A TUI is never constructed from a configuration file, so this exists only
/// so the first frame has something structurally valid to be compared against
/// in tests.
#[must_use]
pub fn empty_document() -> FrontendSnapshot {
    FrontendSnapshot::empty(Vec::new())
}

/// The daemon test harness, shared by every platform.
///
/// Plan 169 found the Windows half of this module had never been compiled,
/// because the whole module sat behind `#[cfg(unix)]` for a reason that turned
/// out to be three lines: a raw-`UnixStream` byte-ordering probe, a socket-file
/// unlink assertion, and nothing else. Everything here — the temporary config,
/// the running daemon, the document reader, and the counting remote — is
/// platform-neutral and drives the real `ipc` layer, so on Windows it drives
/// the real `\\.\pipe\gregg-client-<id>` transport.
#[cfg(test)]
mod test_support {
    use super::*;
    use crate::clientd::snapshot::FrontendSnapshot;
    use crate::config::{Config, SystemEntry};
    use std::time::Duration;

    /// A per-test temporary directory that cleans itself up.
    pub(super) struct TempDir(std::path::PathBuf);

    impl TempDir {
        pub(super) fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gregg-clientd-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        pub(super) fn config_path(&self) -> std::path::PathBuf {
            self.0.join("gregg.toml")
        }

        /// The socket file the daemon must unlink on an orderly exit.
        ///
        /// Only Unix has one: a Windows pipe has no filesystem entry, so the
        /// cleanup assertion is not meaningful there.
        #[cfg(unix)]
        pub(super) fn socket_path(&self) -> std::path::PathBuf {
            self.0.join("gregg-client.sock")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A config with no systems and a one-second cadence.
    ///
    /// An empty system list means the scheduler produces no batches at all,
    /// which is exactly what the ownership and fan-out tests need: they are
    /// about the local channel, not about metrics, and must not depend on a
    /// reachable host. It also makes publications fully deterministic, because
    /// a `Ctrl-R` becomes the only thing that can mint a new generation.
    pub(super) fn write_empty_config(dir: &TempDir) -> ConfigStore {
        let config = Config {
            refresh_seconds: 1,
            ..Config::default()
        };
        let store = ConfigStore::new(dir.config_path());
        store.write(&config).expect("writes config");
        store
    }

    /// A config pointing at one loopback port, with a one-second cadence.
    pub(super) fn write_single_system_config(dir: &TempDir, port: u16) -> ConfigStore {
        let config = Config {
            refresh_seconds: 1,
            systems: vec![SystemEntry {
                id: "sys-a".to_owned(),
                host: "127.0.0.1".to_owned(),
                port,
                name: Some("remote".to_owned()),
            }],
            ..Config::default()
        };
        let store = ConfigStore::new(dir.config_path());
        store.write(&config).expect("writes config");
        store
    }

    /// A running daemon plus the identity needed to reach it.
    pub(super) struct Running {
        pub(super) cancel: CancellationToken,
        pub(super) handle: tokio::task::JoinHandle<Result<(), DaemonError>>,
        pub(super) identity: ClientDaemonIdentity,
    }

    impl Running {
        pub(super) fn start(dir: &TempDir, store: ConfigStore) -> Self {
            let identity = ClientDaemonIdentity::for_path(&dir.config_path());
            let cancel = CancellationToken::new();
            let handle = {
                let cancel = cancel.clone();
                let identity = ClientDaemonIdentity::for_path(&dir.config_path());
                tokio::spawn(async move { run_daemon(store, identity, cancel).await })
            };
            Self {
                cancel,
                handle,
                identity,
            }
        }

        /// Wait for the endpoint to appear, bounded.
        pub(super) async fn ready(&self) {
            for _ in 0..200 {
                if self
                    .identity
                    .candidates()
                    .iter()
                    .any(|path| ipc::endpoint_is_live(path))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("the daemon never bound its endpoint");
        }

        /// Stop the daemon and wait for it to actually finish.
        ///
        /// Bounded on purpose: if the accept wait ever became uncancellable
        /// again this would fail the test rather than wedge the CI job until its
        /// own timeout, which is the failure this harness exists to catch.
        pub(super) async fn shutdown(self) {
            self.cancel.cancel();
            tokio::time::timeout(Duration::from_secs(10), self.handle)
                .await
                .expect("the daemon must exit after a stop request")
                .expect("the daemon task must not panic")
                .expect("a clean stop must not be an error");
        }
    }

    /// How long a single expected document may take to arrive.
    ///
    /// Generous, because it is only ever reached on failure: the point is that a
    /// daemon which stops publishing fails a test instead of wedging the CI job
    /// until its own timeout.
    pub(super) const DOCUMENT_BUDGET: Duration = Duration::from_secs(20);

    /// The next state document.
    ///
    /// Note that the bound is a *count of frames*, not a deadline:
    /// [`FrameStream::next`] waits indefinitely for a frame, so a daemon that
    /// stops publishing hangs this helper rather than failing it. Tests that
    /// wait for state which may never arrive must use [`await_documents`].
    pub(super) async fn next_snapshot(
        frames: &mut crate::clientd::frontend::FrameStream,
    ) -> FrontendSnapshot {
        for _ in 0..200 {
            if let FrontendFrame::Snapshot(document) = frames.next().await.expect("reads") {
                return *document;
            }
        }
        panic!("no state document arrived");
    }

    /// A deadline-bounded [`next_snapshot`].
    ///
    /// [`next_snapshot`] bounds the *frame count* it will read, not how long it
    /// will wait, because `FrameStream::next` waits for a frame indefinitely.
    /// That is fine inside [`await_documents`], which owns a deadline, but not
    /// for a test that waits on a specific generation: a daemon that stops
    /// publishing would wedge the whole CI job instead of failing it. Every
    /// direct wait in the Windows tests goes through this.
    pub(super) async fn next_snapshot_within(
        frames: &mut crate::clientd::frontend::FrameStream,
        budget: Duration,
    ) -> FrontendSnapshot {
        match tokio::time::timeout(budget, next_snapshot(frames)).await {
            Ok(document) => document,
            Err(elapsed) => panic!("no state document arrived within {budget:?}: {elapsed}"),
        }
    }

    /// Read documents until `wanted` is satisfied, or fail after `budget`.
    ///
    /// Deadline-bounded because "the daemon published nothing" is exactly the
    /// regression these tests exist to catch, and it must fail the test rather
    /// than hang the suite.
    ///
    /// Documents are *accumulated* rather than discarded: a fact the daemon
    /// published in an earlier document is just as true as one in the latest,
    /// and a test that only inspected the final read would report a failure for
    /// state that arrived perfectly correctly one read earlier.
    pub(super) async fn await_documents(
        frames: &mut crate::clientd::frontend::FrameStream,
        budget: Duration,
        mut wanted: impl FnMut(&FrontendSnapshot) -> bool,
    ) -> Vec<FrontendSnapshot> {
        let deadline = tokio::time::Instant::now() + budget;
        let mut seen = Vec::new();
        while tokio::time::Instant::now() < deadline {
            let Ok(document) = tokio::time::timeout_at(deadline, next_snapshot(frames)).await
            else {
                break;
            };
            let satisfied = wanted(&document);
            seen.push(document);
            if satisfied {
                return seen;
            }
        }
        panic!("the expected state did not arrive within {budget:?}");
    }

    /// A fully-counting remote: serves metrics *and* both scheduler routes and
    /// records how many times each was asked.
    ///
    /// Plan 167's primary architectural proof needs request counts, not
    /// publication counts. Two subscribers receiving the same generation only
    /// shows the fan-out is shared; it does not show the *remote* was not
    /// polled twice.
    pub(super) struct CountingGreggd {
        pub(super) port: u16,
        status: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        summary: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        history: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl CountingGreggd {
        pub(super) fn status_hits(&self) -> usize {
            self.status.load(std::sync::atomic::Ordering::SeqCst)
        }

        pub(super) fn summary_hits(&self) -> usize {
            self.summary.load(std::sync::atomic::Ordering::SeqCst)
        }

        pub(super) fn history_hits(&self) -> usize {
            self.history.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    pub(super) async fn spawn_counting_greggd() -> CountingGreggd {
        use gregg_protocol::test_support::LinuxSnapshotV2Builder;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let status_body = serde_json::to_vec(
            &LinuxSnapshotV2Builder::default()
                .sample_interval_ms(1_000)
                .build_payload(),
        )
        .expect("status serializes");
        let (summary_body, history_body) = scheduler_documents();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let status = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let summary = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let history = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (t_status, t_summary, t_history) = (
            std::sync::Arc::clone(&status),
            std::sync::Arc::clone(&summary),
            std::sync::Arc::clone(&history),
        );
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (status_body, summary_body, history_body) = (
                    status_body.clone(),
                    summary_body.clone(),
                    history_body.clone(),
                );
                let (t_status, t_summary, t_history) = (
                    std::sync::Arc::clone(&t_status),
                    std::sync::Arc::clone(&t_summary),
                    std::sync::Arc::clone(&t_history),
                );
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
                    // This remote serves every route, so there is no 404 arm:
                    // any request that is not a scheduler route is a metrics
                    // request, which is exactly what the counts are asserting.
                    let body = if request.contains("/v2/scheduler/history") {
                        t_history.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        history_body
                    } else if request.contains("/v2/scheduler") {
                        t_summary.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        summary_body
                    } else {
                        t_status.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        status_body
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        String::from_utf8_lossy(&body)
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });
        CountingGreggd {
            port,
            status,
            summary,
            history,
        }
    }

    /// The scheduler documents the counting remote serves.
    #[allow(clippy::type_complexity)]
    fn scheduler_documents() -> (Vec<u8>, Vec<u8>) {
        use gregg_protocol::{
            SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobStateV2,
            SchedulerJobV2, SchedulerOutcomeV2, SchedulerOutputV2, SchedulerRunRecordV2,
            SchedulerRunSummaryV2, SchedulerSummaryV2,
        };
        let epoch = SchedulerEpochV2 {
            started_at_unix_ms: 1_700_000_000_000,
            nonce: 11,
        };
        let summary = serde_json::to_vec(&SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch,
            history_revision: 1,
            jobs: vec![SchedulerJobV2 {
                name: "backup".to_owned(),
                schedule: "0 3 * * *".to_owned(),
                next_due_unix_ms: 1_700_100_000_000,
                state: SchedulerJobStateV2::Idle,
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
        })
        .expect("summary serializes");
        let history = serde_json::to_vec(&SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch,
            history_revision: 1,
            jobs: vec![SchedulerJobHistoryV2 {
                name: "backup".to_owned(),
                records: vec![SchedulerRunRecordV2 {
                    sequence: 1,
                    scheduled_unix_ms: 1_700_000_000_000,
                    started_unix_ms: Some(1_700_000_000_100),
                    finished_unix_ms: 1_700_000_001_000,
                    outcome: SchedulerOutcomeV2::Success,
                    exit_code: Some(0),
                    signal: None,
                    duration_ms: Some(900),
                    delay_ms: 0,
                    coalesced: false,
                    stdout: SchedulerOutputV2::new("ok\n".to_owned(), false),
                    stderr: SchedulerOutputV2::new(String::new(), false),
                }],
            }],
        })
        .expect("history serializes");
        (summary, history)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::test_support::{
        await_documents, next_snapshot_within, spawn_counting_greggd, write_empty_config,
        write_single_system_config, Running, TempDir, DOCUMENT_BUDGET,
    };
    use super::*;
    use crate::clientd::frontend::FrontendLink;
    use crate::config::SystemEntry;
    use std::time::Duration;

    /// A full scheduler channel must not stall the engine's own work.
    ///
    /// The poll task only reads commands *between* generations, so a bounded
    /// channel fills routinely while a generation is in flight. `Ctrl-R` plus a
    /// run of mutating CLI nudges is enough to fill it. A blocking send there
    /// parked the single engine task, freezing every attached TUI and leaving a
    /// `Shutdown` unread for the rest of the generation.
    #[test]
    fn a_full_scheduler_channel_does_not_stop_the_reload_path() {
        let dir = TempDir::new("scheduler-backpressure");
        let store = write_single_system_config(&dir, 1);
        let (snapshots, _) = watch::channel(
            encode_document(
                &FleetState::from_config(&store.load_existing().expect("loads")),
                1,
                &CronIntents::default(),
            )
            .expect("encodes"),
        );
        let (requests, _request_rx) = mpsc::channel(1);
        let hub = Arc::new(Hub {
            snapshots,
            requests,
            intents: Arc::new(Mutex::new(EggpoolIntents::default())),
            cron_intents: Arc::new(Mutex::new(CronIntents::default())),
            cron_endpoints: Arc::new(Mutex::new(Vec::new())),
            cron_reload: Arc::new(Notify::new()),
            daemon_id: "test".to_owned(),
            shutdown: CancellationToken::new(),
        });
        let mut fleet = FleetState::from_config(&store.load_existing().expect("loads"));
        let mut engine = Engine {
            generation: 0,
            converged: None,
            eggpool_worker: None,
            eggpool_client: eggpool::EggpoolClient::new(Duration::from_secs(1)),
            cron_dirty: false,
            cron_revision: 0,
            store: ConfigStore::new(dir.config_path()),
        };

        // A channel with no receiver: the scheduler task is gone and every
        // send is refused. This is the worst case a real full channel has.
        let (scheduler_tx, scheduler_rx) = mpsc::channel::<SchedulerCommand>(1);
        drop(scheduler_rx);
        let (accepted, detail) = engine.reload_config(&mut fleet, &hub, &scheduler_tx);
        assert!(accepted, "the reload itself still succeeds: {detail:?}");
        assert_eq!(
            fleet.systems.len(),
            1,
            "the fleet must be reconciled even when the poll request is dropped"
        );

        // And a *full* (not closed) channel is equally harmless.
        let (scheduler_tx, _scheduler_rx) = mpsc::channel::<SchedulerCommand>(1);
        scheduler_tx
            .try_send(SchedulerCommand::Refresh)
            .expect("the single slot is free");
        request_scheduler_poll(
            &scheduler_tx,
            SchedulerCommand::ReplaceEndpoints(Vec::new()),
        );
        assert_eq!(
            fleet.systems.len(),
            1,
            "a refused early poll must not cost the daemon its own work"
        );
    }

    /// A window change publishes one generation, not two.
    ///
    /// `reduce_intents` used to record the converged memo and then bump the
    /// worker generation, so the next tick saw a difference and republished the
    /// same window one generation later. The worker treats that as superseding:
    /// it aborted the fetch it had just started and issued a second one. The
    /// pane still converged — at the cost of a wasted round trip per event.
    #[tokio::test]
    async fn a_window_change_publishes_the_generation_the_reducer_accepts() {
        let dir = TempDir::new("one-generation");
        let store = write_empty_config(&dir);
        let entry = crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            // A closed port: the worker runs and fails fast, so this stays a
            // test of the daemon's bookkeeping rather than of the network.
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        };
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(entry.clone());
        store.write(&config).expect("writes");
        let (snapshots, _) = watch::channel(
            encode_document(
                &FleetState::from_config(&config),
                1,
                &CronIntents::default(),
            )
            .expect("encodes"),
        );
        let (requests, _request_rx) = mpsc::channel(1);
        let hub = Arc::new(Hub {
            snapshots,
            requests,
            intents: Arc::new(Mutex::new(EggpoolIntents::default())),
            cron_intents: Arc::new(Mutex::new(CronIntents::default())),
            cron_endpoints: Arc::new(Mutex::new(Vec::new())),
            cron_reload: Arc::new(Notify::new()),
            daemon_id: "test".to_owned(),
            shutdown: CancellationToken::new(),
        });
        let mut fleet = FleetState::from_config(&config);
        let worker = eggpool::spawn_worker(
            eggpool::EggpoolClient::new(Duration::from_secs(1)),
            entry.clone(),
            CancellationToken::new(),
        );
        let mut engine = Engine {
            generation: 0,
            converged: None,
            eggpool_worker: Some(EggpoolWorkerHandle {
                endpoint: entry,
                control: worker.control,
            }),
            eggpool_client: eggpool::EggpoolClient::new(Duration::from_secs(1)),
            cron_dirty: false,
            cron_revision: 0,
            store: ConfigStore::new(dir.config_path()),
        };

        // Open the pane on a one-hour window, then ask for a longer one.
        hub.with_intents(|intents| intents.set(1, true, EggpoolPeriod::Hour));
        engine.reduce_intents(&mut fleet, &hub);
        hub.with_intents(|intents| intents.set(1, true, EggpoolPeriod::Day));
        engine.reduce_intents(&mut fleet, &hub);

        let intents = hub.with_intents(|guard| guard.clone());
        assert_eq!(
            engine.converged,
            fleet.eggpool_desired_state(&intents),
            "the memo must describe the generation the worker was just driven with"
        );
        assert_eq!(
            engine.converged.map(|state| state.generation),
            fleet.eggpool.as_ref().map(|state| state.request_generation),
            "the reducer would otherwise reject every result the worker fetches"
        );
        assert!(
            !engine.reduce_intents(&mut fleet, &hub),
            "the next tick must have nothing to republish"
        );
    }

    /// A dead `EggPool` worker is published once, not once per tick.
    ///
    /// The reduce tick runs every 250ms. With the converged memo left unset
    /// after a failed publish, one dead worker re-detected its own failure on
    /// every tick and re-encoded and broadcast a full fleet document forever.
    #[tokio::test]
    async fn a_dead_eggpool_worker_publishes_once_and_then_settles() {
        let dir = TempDir::new("dead-worker");
        let store = write_empty_config(&dir);
        let entry = crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        };
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(entry.clone());
        store.write(&config).expect("writes");

        let (snapshots, _) = watch::channel(
            encode_document(
                &FleetState::from_config(&config),
                1,
                &CronIntents::default(),
            )
            .expect("encodes"),
        );
        let (requests, _request_rx) = mpsc::channel(1);
        let hub = Arc::new(Hub {
            snapshots,
            requests,
            intents: Arc::new(Mutex::new(EggpoolIntents::default())),
            cron_intents: Arc::new(Mutex::new(CronIntents::default())),
            cron_endpoints: Arc::new(Mutex::new(Vec::new())),
            cron_reload: Arc::new(Notify::new()),
            daemon_id: "test".to_owned(),
            shutdown: CancellationToken::new(),
        });
        let mut fleet = FleetState::from_config(&config);
        hub.with_intents(|intents| intents.set(1, true, EggpoolPeriod::Hour));

        // A real worker, then stopped: cancelling its token ends the task, so
        // the control channel closes and publishing fails exactly as a dead
        // worker's does.
        let tasks = CancellationToken::new();
        let worker = eggpool::spawn_worker(
            eggpool::EggpoolClient::new(Duration::from_secs(1)),
            entry.clone(),
            tasks.clone(),
        );
        tasks.cancel();
        let probe = EggpoolDesiredState {
            active: true,
            period: EggpoolPeriod::Hour,
            generation: 1,
        };
        for _ in 0..8 {
            tokio::task::yield_now().await;
            if worker.control.publish(probe).is_err() {
                break;
            }
        }
        worker
            .control
            .publish(probe)
            .expect_err("the worker is stopped, so its channel is closed");

        let mut engine = Engine {
            generation: 0,
            converged: None,
            eggpool_worker: Some(EggpoolWorkerHandle {
                endpoint: entry,
                control: worker.control,
            }),
            eggpool_client: eggpool::EggpoolClient::new(Duration::from_secs(1)),
            cron_dirty: false,
            cron_revision: 0,
            store: ConfigStore::new(dir.config_path()),
        };

        assert!(
            engine.reduce_intents(&mut fleet, &hub),
            "the dead worker is reported once"
        );
        assert_eq!(
            fleet.eggpool.as_ref().map(|state| state.worker_state),
            Some(crate::state::EggpoolWorkerState::WorkerUnavailable),
            "a pane with no worker says so rather than pretending to load"
        );
        assert!(
            !engine.reduce_intents(&mut fleet, &hub),
            "an unchanged dead worker must not re-publish on the next tick"
        );
    }

    /// Read one length-prefixed frame straight off a raw socket.
    ///
    /// Hand-rolled on purpose: the ordering test below is about the *first
    /// bytes on the wire*, so it must not go through the client-side frame
    /// reader, which is exactly the thing that would hide a violated ordering.
    /// `tokio` has no named-pipe stream, so this probe is Unix-only; the
    /// Windows equivalent is the `Hello`-first contract asserted through the
    /// production `attach` path in `windows_tests`.
    async fn read_raw_frame(stream: &mut tokio::net::UnixStream) -> FrontendFrame {
        use tokio::io::AsyncReadExt;

        let mut prefix = [0_u8; 9];
        stream.read_exact(&mut prefix).await.expect("reads prefix");
        let length = usize::from_str_radix(
            std::str::from_utf8(&prefix[..8])
                .expect("prefix is ascii")
                .trim(),
            16,
        )
        .expect("hex length");
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.expect("reads body");
        serde_json::from_slice(&body).expect("decodes")
    }

    /// A loopback remote that serves **only** the two scheduler routes.
    ///
    // Long because the wire documents are spelled out inline: a shared builder
    // would hide exactly the shape these tests are asserting against.
    #[allow(clippy::too_many_lines)]
    ///
    /// `/v2/status` is deliberately not served, so the metrics plane reports
    /// the system offline while the cron plane is fully healthy. That is the
    /// exact independence the plan requires, and it is far easier to assert
    /// from a real end-to-end run than from a unit test on a reducer.
    struct CountingRemote {
        port: u16,
        summary_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        history_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl CountingRemote {
        fn summary_hits(&self) -> usize {
            self.summary_hits.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn history_hits(&self) -> usize {
            self.history_hits.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    async fn spawn_scheduler_only_remote() -> u16 {
        spawn_counting_remote().await.port
    }

    // Long because the wire documents are spelled out inline: a shared builder
    // would hide exactly the shape these tests are asserting against.
    #[allow(clippy::too_many_lines)]
    async fn spawn_counting_remote() -> CountingRemote {
        use gregg_protocol::{
            SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobStateV2,
            SchedulerJobV2, SchedulerOutcomeV2, SchedulerOutputV2, SchedulerRunRecordV2,
            SchedulerRunSummaryV2, SchedulerSummaryV2,
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let epoch = SchedulerEpochV2 {
            started_at_unix_ms: 1_700_000_000_000,
            nonce: 11,
        };
        let job = SchedulerJobV2 {
            name: "backup".to_owned(),
            schedule: "0 3 * * *".to_owned(),
            next_due_unix_ms: 1_700_100_000_000,
            state: SchedulerJobStateV2::Idle,
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
        };
        let summary = serde_json::to_vec(&SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch,
            history_revision: 1,
            jobs: vec![job],
        })
        .expect("summary serializes");
        let history = serde_json::to_vec(&SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch,
            history_revision: 1,
            jobs: vec![SchedulerJobHistoryV2 {
                name: "backup".to_owned(),
                records: vec![SchedulerRunRecordV2 {
                    sequence: 1,
                    scheduled_unix_ms: 1_700_000_000_000,
                    started_unix_ms: Some(1_700_000_000_100),
                    finished_unix_ms: 1_700_000_001_000,
                    outcome: SchedulerOutcomeV2::Success,
                    exit_code: Some(0),
                    signal: None,
                    duration_ms: Some(900),
                    delay_ms: 0,
                    coalesced: false,
                    stdout: SchedulerOutputV2::new("ok\n".to_owned(), false),
                    stderr: SchedulerOutputV2::new(String::new(), false),
                }],
            }],
        })
        .expect("history serializes");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let summary_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let history_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task_summary_hits = std::sync::Arc::clone(&summary_hits);
        let task_history_hits = std::sync::Arc::clone(&history_hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let summary = summary.clone();
                let history = history.clone();
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
                    let body = if request.contains("/v2/scheduler/history") {
                        history_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Some(history)
                    } else if request.contains("/v2/scheduler") {
                        summary_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Some(summary)
                    } else {
                        None
                    };
                    let response = match body {
                        Some(body) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            String::from_utf8_lossy(&body)
                        ),
                        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });
        CountingRemote {
            port,
            summary_hits,
            history_hits,
        }
    }

    #[tokio::test]
    async fn a_document_never_overtakes_the_hello_a_frontend_identifies_its_daemon_by() {
        use tokio::io::AsyncWriteExt;

        let dir = TempDir::new("hello-order");
        // A dead loopback port: the metrics plane is refused immediately, so the
        // config boundary below can force a publication without a reachable
        // host standing in for one.
        let running = Running::start(&dir, write_single_system_config(&dir, 9));
        running.ready().await;

        // A normal frontend, used only to *force and observe* a publication.
        let mut observer = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("observer attaches");
        let baseline = next_snapshot_within(&mut observer.frames, DOCUMENT_BUDGET)
            .await
            .generation;

        // A second connection that connects and then stays deliberately silent,
        // so it is subscribed to the publication slot without ever having sent
        // a handshake. `serve_inner` is parked in its `select!` for this
        // connection with `handshake_done` still false; the only question is
        // whether the `changed()` arm is allowed to write there.
        let mut stream = tokio::net::UnixStream::connect(
            running
                .identity
                .candidates()
                .into_iter()
                .find(|path| path.exists())
                .expect("the daemon bound an endpoint"),
        )
        .await
        .expect("connects");

        // `Ctrl-R` is the one guaranteed publication boundary: a reload
        // reconciles the fleet and polls immediately, so a newer generation
        // exists without waiting on the cadence. Once the observer has seen it,
        // the daemon has definitely published, and the silent connection's
        // `changed()` is therefore armed. This is what makes the ordering
        // below deterministic rather than a race that mostly goes the right way.
        observer
            .frontend
            .request_reload(baseline + 1)
            .expect("reload is accepted");
        await_documents(&mut observer.frames, Duration::from_secs(20), |document| {
            document.generation > baseline
        })
        .await;

        stream
            .write_all(
                &encode_frame(&DaemonRequest::Handshake {
                    protocol_version: PROTOCOL_VERSION,
                    version: "test".to_owned(),
                    daemon_id: running.identity.id().to_owned(),
                })
                .expect("encodes"),
            )
            .await
            .expect("sends handshake");

        // A frontend identifies its daemon by the first frame it receives, so
        // a document arriving ahead of the `Hello` makes a healthy daemon
        // indistinguishable from a refusing one. `attach` rejects exactly this,
        // which is how the ordering bug presented as two unrelated cron tests
        // failing under load.
        match read_raw_frame(&mut stream).await {
            FrontendFrame::Hello(hello) => assert_eq!(
                hello.daemon_id,
                running.identity.id().to_owned(),
                "the hello must name the config it serves"
            ),
            other => panic!("the first frame must be a hello, got {other:?}"),
        }

        // Withholding the premature write must not cost the frontend its first
        // document: the handshake reply carries the current state, so nothing
        // is lost by refusing to publish ahead of the `Hello`.
        assert!(
            matches!(
                read_raw_frame(&mut stream).await,
                FrontendFrame::Snapshot(_)
            ),
            "the hello must be followed by the current document"
        );

        running.shutdown().await;
    }

    #[tokio::test]
    async fn cron_observability_survives_a_system_whose_metrics_route_is_absent() {
        let port = spawn_scheduler_only_remote().await;
        let dir = TempDir::new("cron-offline-metrics");
        let running = Running::start(&dir, write_single_system_config(&dir, port));
        running.ready().await;

        let mut attachment = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");

        // The daemon stores every publication unconditionally, so a frontend
        // attaching after the fleet has already settled is handed the current
        // state rather than the document from bind time, and a frontend that
        // attaches early is brought up to date by the next publication.
        let documents = await_documents(
            &mut attachment.frames,
            Duration::from_secs(20),
            |document| {
                document.systems[0].reachability != crate::state::Reachability::Pending
                    && document.cron_for("sys-a").is_some_and(|cron| {
                        cron.capability == crate::cron::CronCapability::Supported
                    })
            },
        )
        .await;

        // The metrics plane owns reachability, and its verdict is independent
        // of whether the scheduler routes exist.
        assert_ne!(
            documents.last().expect("at least one document").systems[0].reachability,
            crate::state::Reachability::Pending,
            "an attach after a settled poll must see the current verdict"
        );

        // The scheduler plane is nonetheless fully healthy, and that fact is
        // published separately rather than being folded into reachability.
        let cron = documents
            .iter()
            .rev()
            .find_map(|document| document.cron_for("sys-a").cloned())
            .expect("every system has a cron entry");
        assert_eq!(cron.capability, crate::cron::CronCapability::Supported);
        assert!(cron.summary.is_some(), "the job list arrived");
        assert!(cron.last_error.is_none());
        assert!(
            cron.history.is_empty(),
            "nobody has the pane open, so no records are published"
        );

        running.shutdown().await;
    }

    #[tokio::test]
    async fn an_open_cron_pane_publishes_only_the_selected_job() {
        let port = spawn_scheduler_only_remote().await;
        let dir = TempDir::new("cron-intent");
        let running = Running::start(&dir, write_single_system_config(&dir, port));
        running.ready().await;

        let mut attachment = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");

        // The frontend asks for one job on one system.
        attachment
            .frontend
            .request_cron_intent(Some("sys-a"), Some("backup"), 5, 1)
            .expect("queues the intent");

        let opened = await_documents(
            &mut attachment.frames,
            Duration::from_secs(20),
            |document| {
                document
                    .cron_for("sys-a")
                    .is_some_and(|entry| !entry.history.is_empty())
            },
        )
        .await;
        let entry = opened
            .last()
            .and_then(|document| document.cron_for("sys-a"))
            .expect("a cron entry");
        assert_eq!(entry.history.len(), 1, "only the selected job");
        assert_eq!(entry.history[0].job, "backup");
        assert_eq!(entry.history[0].records.len(), 1);

        // Closing the pane stops the transmission. A departed view must not
        // keep the daemon publishing records nobody is reading.
        attachment
            .frontend
            .request_cron_intent(None, None, 0, 2)
            .expect("queues the close");
        await_documents(
            &mut attachment.frames,
            Duration::from_secs(20),
            |document| {
                document
                    .cron_for("sys-a")
                    .is_some_and(|entry| entry.history.is_empty())
            },
        )
        .await;

        running.shutdown().await;
    }

    #[tokio::test]
    async fn several_windows_asking_for_cron_add_no_remote_requests() {
        // The scheduler plane is the daemon's, not the window's. Ten windows
        // with the cron pane open must cost the fleet exactly what one closed
        // window costs, because the intent governs *publication* and never
        // fetching. A per-TUI poller would fail this by a factor of ten.
        let remote = spawn_counting_remote().await;
        let dir = TempDir::new("cron-multiclient");
        let running = Running::start(&dir, write_single_system_config(&dir, remote.port));
        running.ready().await;

        // One window, pane closed. This is the baseline.
        let mut first = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("first attaches");
        await_documents(&mut first.frames, Duration::from_secs(20), |document| {
            document
                .cron_for("sys-a")
                .is_some_and(|cron| cron.capability == crate::cron::CronCapability::Supported)
        })
        .await;
        let baseline_summary = remote.summary_hits();
        let baseline_history = remote.history_hits();
        assert_eq!(
            baseline_history, 1,
            "discovery fetches history exactly once"
        );

        // More windows, all with the cron pane open.
        let mut others = Vec::new();
        for _ in 0..3 {
            let attachment = attach(&running.identity, env!("CARGO_PKG_VERSION"))
                .await
                .expect("attaches");
            attachment
                .frontend
                .request_cron_intent(Some("sys-a"), Some("backup"), 5, 1)
                .expect("queues the intent");
            others.push(attachment);
        }

        // Let the intents land and the publications settle. The remote must not
        // be touched: nothing about attaching or opening a pane is a reason to
        // read it again.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            remote.summary_hits(),
            baseline_summary,
            "a window opening the cron pane must not add a scheduler read"
        );
        assert_eq!(
            remote.history_hits(),
            baseline_history,
            "a window opening the cron pane must not add a history read"
        );

        // The intents still take effect: the pane's records are published.
        for attachment in &mut others {
            await_documents(
                &mut attachment.frames,
                Duration::from_secs(20),
                |document| {
                    document
                        .cron_for("sys-a")
                        .is_some_and(|entry| !entry.history.is_empty())
                },
            )
            .await;
        }

        running.shutdown().await;
    }

    #[tokio::test]
    async fn one_config_identity_gets_one_endpoint_and_two_configs_never_share_one() {
        let first = TempDir::new("identity-a");
        let second = TempDir::new("identity-b");
        let a = ClientDaemonIdentity::for_path(&first.config_path());
        let b = ClientDaemonIdentity::for_path(&second.config_path());
        assert_ne!(a.id(), b.id(), "distinct configs must not collide");
        assert_ne!(a.candidates()[0], b.candidates()[0]);

        // The same file reached through an equivalent spelling converges.
        let direct = ClientDaemonIdentity::for_path(&first.config_path());
        assert_eq!(a.id(), direct.id());
    }

    #[tokio::test]
    async fn a_new_subscriber_receives_complete_current_state_immediately() {
        let dir = TempDir::new("first-subscriber");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        link.handshake(&running.identity, "test")
            .expect("handshakes");
        let (sender, mut frames) = link.split();

        // The first frame after the handshake is the complete current state,
        // not a delta: a fresh TUI never has to wait for a poll.
        let document = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
        assert_eq!(document.systems.len(), 0);
        assert_eq!(
            document.generation, 1,
            "the initial document is generation 1"
        );

        drop(sender);
        running.shutdown().await;
    }

    #[tokio::test]
    async fn two_subscribers_share_one_publication_instead_of_one_poll_each() {
        let dir = TempDir::new("fanout");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        let mut first = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        first
            .handshake(&running.identity, "test")
            .expect("handshakes");
        let (first_sender, mut first_frames) = first.split();
        let mut second = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        second
            .handshake(&running.identity, "test")
            .expect("handshakes");
        let (second_sender, mut second_frames) = second.split();

        let initial_first = next_snapshot_within(&mut first_frames, DOCUMENT_BUDGET).await;
        let initial_second = next_snapshot_within(&mut second_frames, DOCUMENT_BUDGET).await;
        assert_eq!(
            initial_first.generation, initial_second.generation,
            "both subscribers observe the same publication, not two polls"
        );

        // One reload produces exactly one publication, and both subscribers
        // see the same generation of it.
        first_sender.request_reload(1).expect("queues");
        let reloaded_first = next_snapshot_within(&mut first_frames, DOCUMENT_BUDGET).await;
        let reloaded_second = next_snapshot_within(&mut second_frames, DOCUMENT_BUDGET).await;
        assert!(reloaded_first.generation > initial_first.generation);
        assert_eq!(reloaded_first.generation, reloaded_second.generation);

        drop(first_sender);
        drop(second_sender);
        running.shutdown().await;
    }

    #[tokio::test]
    async fn several_windows_cost_the_fleet_exactly_one_polling_plane() {
        // Plan 167's primary architectural proof. The remote is counted, not the
        // publication, because a shared generation is necessary but not
        // sufficient: two frontends polling in parallel would still share
        // generations while doubling the fleet's requests.
        let remote = spawn_counting_greggd().await;
        let dir = TempDir::new("one-plane");
        // A one-second cadence so several generations elapse inside the test.
        let running = Running::start(&dir, write_single_system_config(&dir, remote.port));
        running.ready().await;

        let mut first = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        // Let the first frontend settle on the metrics and scheduler planes.
        await_documents(&mut first.frames, Duration::from_secs(20), |document| {
            document.systems[0].reachability == crate::state::Reachability::Online
                && document
                    .cron_for("sys-a")
                    .is_some_and(|cron| cron.capability == crate::cron::CronCapability::Supported)
        })
        .await;
        let baseline_status = remote.status_hits();
        assert_eq!(remote.history_hits(), 1, "discovery fetches history once");
        let baseline_summary = remote.summary_hits();

        // Attach a second window, with the cron pane open, and let both run for
        // several metrics generations.
        let second = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        second
            .frontend
            .request_cron_intent(Some("sys-a"), Some("backup"), 5, 1)
            .expect("queues the intent");
        tokio::time::sleep(Duration::from_millis(1_200)).await;

        // Two windows, one fleet. Both planes are shared, and the history body
        // is still fetched only on discovery.
        assert!(
            remote.status_hits() >= baseline_status,
            "metrics must keep flowing for both windows"
        );
        assert!(
            remote.status_hits() < baseline_status * 2 + 4,
            "a second window must not double the metrics cadence: {} -> {}",
            baseline_status,
            remote.status_hits()
        );
        assert_eq!(
            remote.summary_hits(),
            baseline_summary,
            "a second window must not double the scheduler cadence"
        );
        assert_eq!(
            remote.history_hits(),
            1,
            "a second window, and an open cron pane, must not refetch history"
        );

        // Closing one window changes nothing about the remote.
        let after_second = remote.status_hits();
        drop(second);
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(
            remote.status_hits() > after_second,
            "the fleet keeps polling after one window closes"
        );
        assert_eq!(remote.history_hits(), 1);

        // Closing the final window must not stop the daemon. This is the whole
        // reason observation lives in a separate process.
        drop(first);
        let after_all = remote.status_hits();
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(
            remote.status_hits() > after_all,
            "the daemon must keep polling with no window attached"
        );
        assert_eq!(remote.history_hits(), 1, "and still not refetch history");

        // The daemon is still the same one: `stop` is the only way down.
        running.shutdown().await;
    }

    #[tokio::test]
    async fn a_tui_restart_keeps_the_daemons_cron_history() {
        // Restarting a window loses only presentation state. The daemon's
        // retained history is the reason "close the TUI and reopen it" does not
        // reset what Gregg has observed.
        let remote = spawn_counting_greggd().await;
        let dir = TempDir::new("tui-restart");
        let running = Running::start(&dir, write_single_system_config(&dir, remote.port));
        running.ready().await;

        let mut first = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        first
            .frontend
            .request_cron_intent(Some("sys-a"), Some("backup"), 5, 1)
            .expect("queues the intent");
        await_documents(&mut first.frames, Duration::from_secs(20), |document| {
            document
                .cron_for("sys-a")
                .is_some_and(|entry| !entry.history.is_empty())
        })
        .await;
        assert_eq!(remote.history_hits(), 1);

        // The window goes away entirely.
        drop(first);
        tokio::time::sleep(Duration::from_millis(300)).await;

        // A brand-new window reopens the same cron detail. The records come
        // from the daemon's retained cache, not from the remote: the departed
        // window's intent is gone, so transmission is re-established by asking
        // again rather than by inheriting a dead window's intent.
        let mut second = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        second
            .frontend
            .request_cron_intent(Some("sys-a"), Some("backup"), 5, 2)
            .expect("queues the reopened intent");
        let documents = await_documents(&mut second.frames, Duration::from_secs(20), |document| {
            document
                .cron_for("sys-a")
                .is_some_and(|entry| !entry.history.is_empty())
        })
        .await;
        let entry = documents
            .last()
            .and_then(|document| document.cron_for("sys-a"))
            .expect("a cron entry");
        assert_eq!(
            entry.history[0].records.len(),
            1,
            "the restarted window sees the daemon's retained history"
        );
        assert_eq!(
            remote.history_hits(),
            1,
            "and the daemon did not refetch it from the remote"
        );

        running.shutdown().await;
    }

    /// A departing frontend's cleanup is a publication trigger in its own right.
    ///
    /// `serve` removes the intent but cannot raise `cron_dirty`, so without this
    /// the removal waited for an unrelated change: attached frontends kept
    /// receiving the departed window's history, and a new attach was handed
    /// history it never asked for.
    #[test]
    fn a_cron_intent_removed_by_a_disconnect_is_a_publication_trigger() {
        let dir = TempDir::new("cron-intent-trigger");
        let store = write_empty_config(&dir);
        let (snapshots, _) = watch::channel(
            encode_document(
                &FleetState::from_config(&store.load_existing().expect("loads")),
                1,
                &CronIntents::default(),
            )
            .expect("encodes"),
        );
        let (requests, _request_rx) = mpsc::channel(1);
        let hub = Arc::new(Hub {
            snapshots,
            requests,
            intents: Arc::new(Mutex::new(EggpoolIntents::default())),
            cron_intents: Arc::new(Mutex::new(CronIntents::default())),
            cron_endpoints: Arc::new(Mutex::new(Vec::new())),
            cron_reload: Arc::new(Notify::new()),
            daemon_id: "test".to_owned(),
            shutdown: CancellationToken::new(),
        });
        let mut engine = Engine {
            generation: 0,
            converged: None,
            eggpool_worker: None,
            eggpool_client: eggpool::EggpoolClient::new(Duration::from_secs(1)),
            cron_dirty: false,
            cron_revision: hub.cron_intents().revision(),
            store: ConfigStore::new(dir.config_path()),
        };

        assert!(
            !engine.note_cron_intent_revision(&hub),
            "an unchanged intent set is not a publication trigger"
        );

        // Exactly what `serve` does when a frontend disconnects.
        hub.with_cron_intents(|intents| {
            intents.set(
                1,
                crate::clientd::daemon::CronIntent {
                    system_id: Some("sys-a".to_owned()),
                    job: Some("backup".to_owned()),
                    display_history: 5,
                },
            )
        });
        assert!(engine.note_cron_intent_revision(&hub));
        assert!(engine.cron_dirty, "an added intent is dirty");
        // The engine's publication consumes the flag.
        assert!(std::mem::take(&mut engine.cron_dirty));
        assert!(!engine.note_cron_intent_revision(&hub));

        hub.with_cron_intents(|intents| intents.remove(1));
        assert!(
            engine.note_cron_intent_revision(&hub),
            "a disconnect's removal must republish, not wait for an unrelated change"
        );
        assert!(engine.cron_dirty);
    }

    /// A departed frontend's cron intent stops being transmitted.
    ///
    /// The connection task removes the departing intent but has no engine
    /// handle, so nothing raised a publication trigger: attached frontends kept
    /// receiving that history and a newly attached one was handed history it
    /// never asked for, until some unrelated change happened to republish.
    #[tokio::test]
    async fn a_departed_frontends_cron_intent_is_no_longer_transmitted() {
        let remote = spawn_counting_greggd().await;
        let dir = TempDir::new("departed-cron-intent");
        let running = Running::start(&dir, write_single_system_config(&dir, remote.port));
        running.ready().await;

        let mut watcher = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        watcher
            .frontend
            .request_cron_intent(Some("sys-a"), Some("backup"), 5, 1)
            .expect("queues the intent");
        await_documents(&mut watcher.frames, Duration::from_secs(20), |document| {
            document
                .cron_for("sys-a")
                .is_some_and(|entry| !entry.history.is_empty())
        })
        .await;

        // The window closes without asking for anything on its way out.
        drop(watcher);
        // Let the connection task finish its own cleanup.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // A brand-new window asked for no cron detail at all, so it must not be
        // handed the departed window's history.
        let mut fresh = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        let documents = next_snapshot_within(&mut fresh.frames, DOCUMENT_BUDGET).await;
        assert!(
            documents
                .cron_for("sys-a")
                .is_none_or(|entry| entry.history.is_empty()),
            "a departing frontend's cron intent must not keep transmitting history: {documents:?}"
        );

        running.shutdown().await;
    }

    #[tokio::test]
    async fn a_disconnected_subscriber_does_not_stop_the_daemon() {
        let dir = TempDir::new("disconnect");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        {
            let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
            link.handshake(&running.identity, "test")
                .expect("handshakes");
            let (_sender, mut frames) = link.split();
            next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
        }
        // Give the accept loop a moment to notice the closed socket.
        tokio::time::sleep(Duration::from_millis(80)).await;

        // The endpoint is still served, which is the whole point: losing a
        // window must not stop background polling.
        let status = status(&running.identity).await.expect("status answers");
        assert!(status.running, "the daemon must outlive its last frontend");

        // And it still answers a fresh attach.
        let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        link.handshake(&running.identity, "test")
            .expect("handshakes");
        let (_sender, mut frames) = link.split();
        next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;

        running.shutdown().await;
    }

    #[tokio::test]
    async fn a_stop_request_from_an_identified_daemon_stops_it() {
        let dir = TempDir::new("stop");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;
        stop(&running.identity, "test")
            .await
            .expect("stop acknowledged");

        let status = status(&running.identity).await.expect("status answers");
        assert!(!status.running, "the daemon must be gone after a stop");
        assert!(
            !dir.socket_path().exists(),
            "the endpoint must be unlinked on an orderly exit"
        );
    }

    #[tokio::test]
    async fn a_handshake_for_a_different_config_is_refused() {
        let dir = TempDir::new("mismatch");
        let other = TempDir::new("mismatch-other");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        // A TUI that believes it is talking to a different config must not be
        // handed this daemon's fleet.
        let foreign = ClientDaemonIdentity::for_path(&other.config_path());
        let mut framed = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        framed
            .send(&crate::clientd::protocol::DaemonRequest::Handshake {
                protocol_version: PROTOCOL_VERSION,
                version: "test".to_owned(),
                daemon_id: foreign.id().to_owned(),
            })
            .expect("writes");
        let (foreign_sender, mut framed_frames) = framed.split();
        let _ = foreign_sender;
        // The refused handshake produces a protocol error, never a snapshot.
        let mut saw_snapshot = false;
        for _ in 0..50 {
            match framed_frames.next().await {
                Ok(FrontendFrame::Snapshot(_)) => {
                    saw_snapshot = true;
                    break;
                }
                // A refusal, a close, or any other non-snapshot frame ends
                // the wait: the assertion is only that no state was served.
                Ok(FrontendFrame::ProtocolError { .. }) | Err(_) => break,
                // Neither frame is a snapshot; keep reading.
                Ok(_) => {}
            }
        }
        assert!(
            !saw_snapshot,
            "a frontend for another config must never receive this daemon's state"
        );

        running.shutdown().await;
    }

    #[tokio::test]
    async fn an_invalid_reload_keeps_the_last_known_good_fleet_and_reports_it() {
        let dir = TempDir::new("reload-failure");
        let store = write_empty_config(&dir);
        let mut config = store.load_existing().expect("loads");
        config.systems.push(SystemEntry {
            id: "kept".into(),
            host: "127.0.0.1".into(),
            port: 11310,
            name: None,
        });
        store.write(&config).expect("writes");
        let running = Running::start(&dir, store);
        running.ready().await;

        let mut link = FrontendLink::connect(&running.identity.candidates()).expect("connects");
        link.handshake(&running.identity, "test")
            .expect("handshakes");
        let (sender, mut frames) = link.split();
        let initial = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
        assert_eq!(initial.systems.len(), 1);
        assert!(initial.config_reload_error.is_none());

        // Corrupt the file behind the daemon's back.
        let store = ConfigStore::new(dir.config_path());
        std::fs::write(dir.config_path(), "this is not = valid = toml [[[")
            .expect("writes broken config");
        sender.request_reload(1).expect("queues");

        let mut saw_error = false;
        for _ in 0..10 {
            let document = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
            if let Some(error) = document.config_reload_error {
                assert!(
                    error.contains("config reload failed"),
                    "unexpected diagnostic: {error}"
                );
                saw_error = true;
                // The last-known-good fleet survives an invalid file.
                assert_eq!(
                    document.systems.len(),
                    1,
                    "an invalid reload must not erase the systems already being polled"
                );
                break;
            }
        }
        assert!(saw_error, "a rejected reload must publish a diagnostic");

        // Repairing the file clears it.
        store.write(&config).expect("rewrites");
        sender.request_reload(2).expect("queues");
        let mut cleared = false;
        for _ in 0..10 {
            let document = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
            if document.config_reload_error.is_none() {
                cleared = true;
                break;
            }
        }
        assert!(cleared, "a successful reload must clear the diagnostic");

        drop(sender);
        running.shutdown().await;
    }

    #[tokio::test]
    async fn eggpool_intents_converge_deterministically_and_free_on_disconnect() {
        let dir = TempDir::new("intents");
        let store = write_empty_config(&dir);
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        store.write(&config).expect("writes");

        let identity = ClientDaemonIdentity::for_path(&dir.config_path());

        // Two frontends, each asking for a different window. The reduction is
        // the shortest *active* window, and it does not depend on the order
        // the intents arrived in.
        let mut intents = EggpoolIntents::default();
        intents.set(1, true, EggpoolPeriod::Week);
        intents.set(2, true, EggpoolPeriod::Day);
        assert_eq!(
            intents.converged_period(EggpoolPeriod::Hour),
            EggpoolPeriod::Day
        );
        assert!(intents.any_active());

        // Reversing the insertion order changes nothing.
        let mut reversed = EggpoolIntents::default();
        reversed.set(2, true, EggpoolPeriod::Day);
        reversed.set(1, true, EggpoolPeriod::Week);
        assert_eq!(
            reversed.converged_period(EggpoolPeriod::Hour),
            intents.converged_period(EggpoolPeriod::Hour)
        );

        // An inactive frontend's window must not win: it is not being shown.
        reversed.set(2, false, EggpoolPeriod::Month);
        assert_eq!(
            reversed.converged_period(EggpoolPeriod::Hour),
            EggpoolPeriod::Week
        );

        // The last pane leaving converges the worker to inactive.
        reversed.set(1, false, EggpoolPeriod::Week);
        assert!(!reversed.any_active());
        assert_eq!(
            reversed.converged_period(EggpoolPeriod::Hour),
            EggpoolPeriod::Hour
        );

        // Re-sending the same intent replaces the entry rather than adding one,
        // so a frontend that refreshes often cannot leak an active entry that
        // keeps the worker running after it disconnected.
        let mut repeated = EggpoolIntents::default();
        for _ in 0..100 {
            repeated.set(7, true, EggpoolPeriod::Hour);
        }
        assert_eq!(repeated.len(), 1);
        repeated.remove(7);
        assert!(repeated.is_empty());

        let running = Running::start(&dir, store);
        running.ready().await;
        // A real attach registers an inactive intent, so the worker starts
        // inactive even though `EggPool` is configured.
        let mut link = FrontendLink::connect(&identity.candidates()).expect("connects");
        link.handshake(&identity, "test").expect("handshakes");
        let (_sender, mut frames) = link.split();
        let document = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
        assert!(document.eggpool.is_some());
        assert_eq!(
            document.eggpool.as_ref().map(|e| e.worker_state),
            Some(crate::state::EggpoolWorkerState::Idle),
            "a frontend that has not opened the pane leaves the worker inactive"
        );
        running.shutdown().await;
    }

    /// An `[eggpool]` entry *added* by `Ctrl-R` must come up live.
    ///
    /// The worker used to be spawned once from the startup config, so adding the
    /// entry published a pane no worker could answer: any request moved it to
    /// `Refreshing` and nothing ever resolved it, which reads as a permanent
    /// "still loading" rather than as "unavailable".
    #[tokio::test]
    async fn adding_an_eggpool_entry_by_reload_brings_up_a_live_worker() {
        let dir = TempDir::new("reload-adds-eggpool");
        let store = write_empty_config(&dir);
        let identity = ClientDaemonIdentity::for_path(&dir.config_path());

        let running = Running::start(&dir, ConfigStore::new(dir.config_path()));
        running.ready().await;
        let mut link = FrontendLink::connect(&identity.candidates()).expect("connects");
        link.handshake(&identity, "test").expect("handshakes");
        let (sender, mut frames) = link.split();
        let initial = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
        assert!(
            initial.eggpool.is_none(),
            "the daemon starts with no `EggPool` entry at all"
        );

        // Add the entry and press the one config-reload boundary.
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            // A closed port: the worker runs and fails to fetch, which is the
            // observable difference from a pane with no worker behind it.
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        store.write(&config).expect("writes");
        sender.request_reload(1).expect("queues");

        let mut added = None;
        for _ in 0..10 {
            let document = next_snapshot_within(&mut frames, DOCUMENT_BUDGET).await;
            if document.eggpool.is_some() {
                added = Some(document);
                break;
            }
        }
        let added = added.expect("the added entry must be published");
        let eggpool = added.eggpool.as_ref().expect("the entry is present");
        assert_eq!(eggpool.endpoint.id, "pool");

        // A live worker answers the pane: the request leaves the daemon and a
        // result comes back, so an attempt is recorded. A pane with no worker
        // behind it never records one — it sits `Idle`, or moves to
        // `Refreshing` on a key press and never leaves, which reads as a
        // permanent "still loading" rather than as "unavailable".
        sender
            .request_eggpool_intent(true, EggpoolPeriod::Hour, false, 2)
            .expect("queues");
        await_documents(&mut frames, DOCUMENT_BUDGET, |document| {
            document
                .eggpool
                .as_ref()
                .is_some_and(|entry| entry.last_attempt_at_unix_ms.is_some())
        })
        .await;

        drop(sender);
        running.shutdown().await;
    }

    /// Two frontends that disagree about the window must never make the
    /// advertised period drift from the window the worker fetches.
    ///
    /// The worker is driven with the converged shortest *active* window, so
    /// fleet-global state has to carry that same value. If one frontend's
    /// request were written there instead, the reducer would reject every
    /// result the worker actually returned (`result.period != eggpool.period`)
    /// and both panes would sit in `Refreshing` with no error.
    #[tokio::test]
    async fn divergent_frontends_advertise_the_converged_eggpool_period() {
        let dir = TempDir::new("converged");
        let store = write_empty_config(&dir);
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        store.write(&config).expect("writes");

        let running = Running::start(&dir, store);
        running.ready().await;
        let identity = running.identity.clone();

        let mut hour_frontend = FrontendLink::connect(&identity.candidates()).expect("connects");
        hour_frontend
            .handshake(&identity, "test")
            .expect("handshakes");
        let mut day_frontend = FrontendLink::connect(&identity.candidates()).expect("connects");
        day_frontend
            .handshake(&identity, "test")
            .expect("handshakes");

        // Frontend A asks for `Hour`, frontend B for `Day`. The worker is
        // driven with the shortest active window, so `Hour` is the only period
        // the daemon is ever going to fetch.
        hour_frontend
            .send(&DaemonRequest::SetEggpoolIntent {
                active: true,
                period: EggpoolPeriod::Hour,
                refresh: false,
                generation: 1,
            })
            .expect("writes");
        day_frontend
            .send(&DaemonRequest::SetEggpoolIntent {
                active: true,
                period: EggpoolPeriod::Day,
                refresh: false,
                generation: 1,
            })
            .expect("writes");
        // B then re-sends its own intent, which is what a pane move, a manual
        // refresh, or the next key press looks like on the wire.
        day_frontend
            .send(&DaemonRequest::SetEggpoolIntent {
                active: true,
                period: EggpoolPeriod::Day,
                refresh: true,
                generation: 2,
            })
            .expect("writes");

        let (_sender, mut frames) = hour_frontend.split();
        // A result has to arrive for the window to be meaningful; the pane is
        // active, so the worker is driven and its failed fetch publishes.
        let documents = await_documents(&mut frames, DOCUMENT_BUDGET, |document| {
            document
                .eggpool
                .as_ref()
                .is_some_and(|eggpool| eggpool.last_attempt_at_unix_ms.is_some())
        })
        .await;
        for document in &documents {
            let eggpool = document.eggpool.as_ref().expect("eggpool is configured");
            assert_eq!(
                eggpool.period,
                EggpoolPeriod::Hour,
                "the advertised window must be the converged one the worker fetches"
            );
        }
        running.shutdown().await;
    }

    /// A departing frontend must move the fleet's window, not just the worker's.
    ///
    /// The disconnect path removes an intent with no request of its own, so the
    /// converged window changes there and nowhere else. If the fleet kept the
    /// old period, the reducer's period check would reject every result the
    /// worker fetched from then on: the surviving pane would freeze on its
    /// pre-disconnect summary while the worker kept burning a request per
    /// interval, every one of them discarded.
    #[tokio::test]
    async fn a_disconnect_reconverges_the_fleet_window_and_keeps_results_landing() {
        let dir = TempDir::new("reconverge");
        let store = write_empty_config(&dir);
        let mut config = store.load_existing().expect("loads");
        config.eggpool = Some(crate::config::EggpoolEntry {
            id: "pool".into(),
            host: "127.0.0.1".into(),
            port: 1,
            scheme: crate::config::EggpoolScheme::Http,
            name: None,
            api_key_env: None,
        });
        store.write(&config).expect("writes");

        let running = Running::start(&dir, store);
        running.ready().await;
        let identity = running.identity.clone();

        let mut hour_frontend = FrontendLink::connect(&identity.candidates()).expect("connects");
        hour_frontend
            .handshake(&identity, "test")
            .expect("handshakes");
        let mut day_frontend = FrontendLink::connect(&identity.candidates()).expect("connects");
        day_frontend
            .handshake(&identity, "test")
            .expect("handshakes");
        hour_frontend
            .send(&DaemonRequest::SetEggpoolIntent {
                active: true,
                period: EggpoolPeriod::Hour,
                refresh: false,
                generation: 1,
            })
            .expect("writes");
        day_frontend
            .send(&DaemonRequest::SetEggpoolIntent {
                active: true,
                period: EggpoolPeriod::Day,
                refresh: false,
                generation: 1,
            })
            .expect("writes");

        let (hour_sender, mut hour_frames) = hour_frontend.split();
        let (_day_sender, mut day_frames) = day_frontend.split();
        // Both attached: the converged window is the shorter one, and results
        // for it are landing.
        await_documents(&mut hour_frames, DOCUMENT_BUDGET, |document| {
            document.eggpool.as_ref().is_some_and(|eggpool| {
                eggpool.period == EggpoolPeriod::Hour && eggpool.last_attempt_at_unix_ms.is_some()
            })
        })
        .await;

        // A quits, so `Day` — the surviving pane's own request — becomes the
        // window the worker is driven with.
        drop(hour_sender);
        drop(hour_frames);
        await_documents(&mut day_frames, DOCUMENT_BUDGET, |document| {
            document.eggpool.as_ref().is_some_and(|eggpool| {
                eggpool.period == EggpoolPeriod::Day
                    // A result has to be *applied*, not merely fetched: the
                    // summary only moves when the reducer's period check
                    // accepts what the worker returned.
                    && eggpool.last_attempt_at_unix_ms.is_some()
            })
        })
        .await;
        running.shutdown().await;
    }
}

/// The Windows half of the fan-out proof.
///
/// Plan 168 closed the Windows transport but left "two TUIs on one Windows
/// client daemon" argued from the shared code path rather than demonstrated,
/// because the whole daemon harness sat behind `#[cfg(unix)]`. The harness is
/// now platform-neutral, so these tests drive `run_daemon` and `attach` for
/// real: on this platform that means `CreateNamedPipeW` with the owner-only
/// SDDL on the daemon side and `CreateFileW` plus `PeekNamedPipe` on each
/// frontend side.
///
/// No interactive terminal windows are involved, and nothing here reaches for
/// a mock transport: the assertions are about the real `\\.\pipe\` endpoint.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::test_support::{
        await_documents, next_snapshot_within, spawn_counting_greggd, write_empty_config,
        write_single_system_config, Running, TempDir, DOCUMENT_BUDGET,
    };
    use super::*;
    use std::time::Duration;

    /// Two real named-pipe frontends over one real client daemon.
    ///
    /// The config has no systems, which is what makes this deterministic rather
    /// than a timing race: the daemon polls nothing, so the only things that
    /// can mint a new generation are an explicit `Ctrl-R`. Each bullet the plan
    /// asks for therefore lands on a specific, awaited generation instead of on
    /// "whatever the cadence happened to produce".
    #[tokio::test]
    async fn two_frontends_share_one_daemon_and_one_publication_over_real_named_pipes() {
        let dir = TempDir::new("win-fanout");
        let running = Running::start(&dir, write_empty_config(&dir));
        running.ready().await;

        // The endpoint really is the Windows pipe namespace, not a socket path.
        let endpoint = &running.identity.candidates()[0];
        let endpoint_text = endpoint.to_string_lossy().into_owned();
        assert!(
            endpoint_text.starts_with(r"\\.\pipe\gregg-client-"),
            "the endpoint must be a named pipe in the gregg-client namespace, got \
             {endpoint_text}"
        );

        // Two independent frontend connections, each completing the version and
        // config handshake through the production `attach` path.
        let first = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("the first frontend attaches");
        let second = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("the second frontend attaches");
        assert_eq!(
            first.hello.daemon_id,
            running.identity.id(),
            "the hello must name the config this daemon serves"
        );
        assert_eq!(second.hello.daemon_id, running.identity.id());
        assert_eq!(
            first.hello.protocol_version, second.hello.protocol_version,
            "both frontends negotiated against the same daemon"
        );

        let (first_frontend, mut first_frames) = (first.frontend, first.frames);
        let (second_frontend, mut second_frames) = (second.frontend, second.frames);

        // Both receive the same published generation. The daemon serializes one
        // document for every frontend, so sharing a generation is the observable
        // form of "one polling plane, two readers".
        let initial_first = next_snapshot_within(&mut first_frames, DOCUMENT_BUDGET).await;
        let initial_second = next_snapshot_within(&mut second_frames, DOCUMENT_BUDGET).await;
        assert_eq!(initial_first.systems.len(), 0);
        assert_eq!(
            initial_first.generation, initial_second.generation,
            "both frontends observe the same publication, not two polls"
        );

        // One `Ctrl-R` from one frontend is one publication, and both frontends
        // see the same generation of it.
        first_frontend.request_reload(1).expect("queues the reload");
        let reloaded_first = next_snapshot_within(&mut first_frames, DOCUMENT_BUDGET).await;
        let reloaded_second = next_snapshot_within(&mut second_frames, DOCUMENT_BUDGET).await;
        assert!(reloaded_first.generation > initial_first.generation);
        assert_eq!(
            reloaded_first.generation, reloaded_second.generation,
            "one reload must reach both frontends as one generation"
        );

        // Losing one frontend must not stop the daemon or the other frontend.
        drop(second_frontend);
        drop(second_frames);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = status(&running.identity).await.expect("status answers");
        assert!(status.running, "one window closing must not stop clientd");
        assert_eq!(status.daemon_id, running.identity.id());

        // And a later publication still reaches the window that stayed.
        first_frontend
            .request_reload(2)
            .expect("queues the second reload");
        let later = next_snapshot_within(&mut first_frames, DOCUMENT_BUDGET).await;
        assert!(
            later.generation > reloaded_first.generation,
            "the surviving frontend must keep receiving publications: {} -> {}",
            reloaded_first.generation,
            later.generation
        );

        drop(first_frontend);
        drop(first_frames);
        running.shutdown().await;
    }

    /// The transport-level echo of the plan's conditional request-count check.
    ///
    /// Plan 167 proved one polling plane on Unix. This repeats the count on the
    /// named-pipe transport so the claim is not carried across platforms by
    /// argument alone. The metrics bound is deliberately loose — the point is
    /// to catch a *second poll plane* (a 2x jump), not to measure the cadence.
    #[tokio::test]
    async fn a_second_windows_frontend_does_not_add_a_second_polling_plane() {
        let remote = spawn_counting_greggd().await;
        let dir = TempDir::new("win-one-plane");
        let running = Running::start(&dir, write_single_system_config(&dir, remote.port));
        running.ready().await;

        let mut first = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        // Let the first window settle on both the metrics and the scheduler
        // plane, so the baselines below are the one-window steady state.
        await_documents(&mut first.frames, Duration::from_secs(20), |document| {
            document.systems[0].reachability == crate::state::Reachability::Online
                && document
                    .cron_for("sys-a")
                    .is_some_and(|cron| cron.capability == crate::cron::CronCapability::Supported)
        })
        .await;
        assert_eq!(
            remote.history_hits(),
            1,
            "discovery fetches history exactly once"
        );
        let baseline_status = remote.status_hits();
        let baseline_summary = remote.summary_hits();
        assert!(baseline_status > 0, "the fleet must actually be polled");

        // A second window, with the cron pane open, over the same pipe transport.
        let second = attach(&running.identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("attaches");
        second
            .frontend
            .request_cron_intent(Some("sys-a"), Some("backup"), 5, 1)
            .expect("queues the intent");
        tokio::time::sleep(Duration::from_millis(1_200)).await;

        // The metrics cadence keeps flowing for both windows and is not
        // doubled, and neither scheduler plane is re-read for the new window.
        assert!(
            remote.status_hits() >= baseline_status,
            "metrics must keep flowing for both windows"
        );
        assert!(
            remote.status_hits() < baseline_status * 2 + 4,
            "a second window must not double the metrics cadence: {baseline_status} -> {}",
            remote.status_hits()
        );
        assert_eq!(
            remote.summary_hits(),
            baseline_summary,
            "a second window must not double the scheduler cadence"
        );
        assert_eq!(
            remote.history_hits(),
            1,
            "a second window, and an open cron pane, must not refetch history"
        );

        // Closing one of the two windows changes nothing about the remote: the
        // daemon keeps polling for the window that stayed.
        let before_close = remote.status_hits();
        drop(first);
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        assert!(
            remote.status_hits() > before_close,
            "the daemon keeps polling after one of two windows closes"
        );
        assert_eq!(remote.history_hits(), 1);

        drop(second);
        running.shutdown().await;
    }

    /// Plan 171's regression test: the daemon process must actually exit.
    ///
    /// `run_daemon` *returning* is not the claim. The claim is that the
    /// **runtime is released**, because `dispatch_daemon` drops the runtime as
    /// the last thing `gregg daemon run` does, and a blocking-pool job that is
    /// still parked makes that drop block forever. So this test builds the same
    /// current-thread runtime the command builds, drives a real daemon over a
    /// real named pipe on that runtime, and stops it the way an operator does.
    ///
    /// No client ever connects, which is the whole design of the test: the
    /// accept wait is therefore parked in `ConnectNamedPipe` when the stop
    /// arrives, which is exactly the state that used to leave a thread waiting
    /// for a client that was never coming. There is no "eventually" for it to
    /// recover on, so a prompt release is the only passing outcome.
    ///
    /// The drop is what is being asserted, so it cannot simply be awaited: a
    /// drop that never returns *is* the defect, and waiting on one in-line is
    /// indistinguishable from a hung test. Handing it to a thread and reporting
    /// a named failure on timeout turns a wedged CI job into an assertion.
    #[test]
    fn a_stopped_daemon_releases_its_runtime() {
        let dir = TempDir::new("win-runtime");
        let store = write_empty_config(&dir);
        let identity = ClientDaemonIdentity::for_path(&dir.config_path());
        let endpoint = identity.candidates()[0].clone();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();

        let (released, reported) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("gregg-win-runtime".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("builds the current-thread runtime `dispatch_daemon` uses");
                runtime.block_on(async move {
                    let daemon = run_daemon(store, identity, cancel);
                    tokio::pin!(daemon);
                    tokio::select! {
                        result = &mut daemon => {
                            result.expect("an unsolicited daemon exit is not this test");
                        }
                        // Wait for the endpoint to be really bound before
                        // stopping, so the accept wait is parked rather than
                        // merely about to start.
                        () = async {
                            while !ipc::endpoint_is_live(&endpoint) {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        } => {
                            stop.cancel();
                            daemon
                                .await
                                .expect("a clean stop must not be reported as an error");
                        }
                    }
                });
                // The assertion. This drop joins the blocking pool, so it is
                // where an uncancellable accept wait used to park forever.
                let elapsed = std::time::Instant::now();
                drop(runtime);
                let _ = released.send(elapsed.elapsed());
            })
            .expect("spawns the runtime thread");

        match reported.recv_timeout(Duration::from_secs(20)) {
            Ok(_) => {
                worker.join().expect("the runtime thread must not panic");
            }
            Err(error) => panic!(
                "the runtime was not released within the budget ({error}): a parked \
                 accept wait is blocking `gregg daemon run` from exiting"
            ),
        }
    }
}
