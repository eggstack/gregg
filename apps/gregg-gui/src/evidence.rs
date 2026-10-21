//! Plan 190: the counted multi-frontend qualification harness.
//!
//! # What this proves
//!
//! One question, answered with counts instead of prose: **does attaching more
//! frontends to one config change what the client daemon asks the remotes
//! for?**
//!
//! The architecture says it does not — the client daemon owns remote polling,
//! and a frontend (TUI or GUI) is a *subscriber* of the documents that daemon
//! already publishes. Prose is not evidence, so this harness stands up:
//!
//! - two counted `greggd`-shaped remotes: a **modern** one that serves the
//!   scheduler summary and history routes, and an **old** one that answers
//!   `404` for both;
//! - a **real** client daemon (`gregg::clientd::daemon::run_daemon`), not a
//!   stub;
//! - a **TUI-like** frontend attached through the same public client-daemon
//!   API the terminal client uses, and a **GUI** frontend attached through the
//!   real [`Bridge`];
//! - per-route hit counters on both remotes, read at defined points.
//!
//! # How the counters are compared
//!
//! A poll can land anywhere inside a measurement window, so every comparison
//! is between two **differences** over two comparable windows, never between
//! absolute counts. The metrics plane runs on the configured `refresh_seconds`
//! cadence, so a window that spans several intervals measures a *rate*; the
//! scheduler planes run on their own, much slower cadences, so their counts
//! are compared with the slack those cadences allow.
//!
//! # What this deliberately does not re-prove
//!
//! Process ownership — the foreign-peer refusal, the directional version
//! rotation, the "never stop an unowned process" rule, and the daemon restart
//! path — is already covered by the client daemon's own launcher tests in
//! `crates/gregg/src/clientd/launch.rs`. This harness proves what only the GUI
//! line can prove: that *the GUI* reaches the same daemon, sees the same
//! documents, and adds no request of its own. `EVIDENCE.md` lists the split.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use gregg::clientd::identity::ClientDaemonIdentity;
use gregg::clientd::snapshot::FrontendSnapshot;
use gregg::clientd::{DaemonExecutable, FrontendFrame, FrontendSender};
use gregg::config::ConfigStore;
use gregg::cron::CronCapability;
use gregg::eggpool::EggpoolPeriod;
use gregg::state::{EggpoolWorkerState, Reachability};

use crate::bridge::{Bridge, BridgeMessage, CronDesired, EggpoolDesired};

/// The config's polling interval, in seconds.
///
/// Short but non-zero: the whole point is to count requests, and a zero
/// interval would make the count meaningless rather than cheap.
const REFRESH_SECONDS: u64 = 1;

/// How long one measurement window lasts.
///
/// Four refresh intervals. Long enough that the count over it is a rate
/// rather than a single sample, short enough that the whole suite stays quick.
const WINDOW: Duration = Duration::from_millis(4_000);

/// Slack allowed when comparing the metrics counts of two windows.
///
/// The daemon's poll cadence is anchored, so a window of exactly four refresh
/// intervals holds four polls — except when a poll lands within a millisecond
/// of a window edge, in which case it falls on either side of the cut, and a
/// loaded machine can skip a tick. Two polls of slack covers that boundary
/// jitter while staying far below the eight-or-more polls a frontend that
/// opened its own HTTP client would produce.
const COUNT_TOLERANCE: usize = 2;

/// Slack allowed when comparing the scheduler summary counts of two windows.
///
/// The scheduler summary is polled on a thirty-second cadence, so a four-second
/// window contains one summary request or none. One extra request is the most
/// that can differ between two such windows.
const SCHEDULER_TOLERANCE: usize = 1;

/// How long a bounded wait for a condition may take.
///
/// Every wait in this harness is a bounded poll against a deadline, never an
/// arbitrary sleep standing in for a condition.
const WAIT_DEADLINE: Duration = Duration::from_secs(10);

/// The smallest growth the cron-history publication must produce.
///
/// The scheduler history record is a few hundred bytes of JSON; the metrics
/// payload around it is a fixed document whose only length jitter is a
/// latency digit. Sixty-four bytes therefore separates "the history rode
/// along" from "the metrics jittered" without depending on either.
const MIN_HISTORY_GROWTH_BYTES: usize = 64;

/// The fixture's modern system id.
const MODERN_SYSTEM_ID: &str = "11111111-2222-4333-8444-555555555555";

/// The fixture's old system id.
const OLD_SYSTEM_ID: &str = "66666666-7777-4888-8999-aaaaaaaaaaaa";

/// The fixture's `EggPool` entry id.
const EGGPOOL_ENTRY_ID: &str = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";

// ---------------------------------------------------------------------------
// Counted remotes
// ---------------------------------------------------------------------------

/// One remote's per-route hit counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Hits {
    /// `/v2/status` requests.
    status: usize,
    /// `/v2/scheduler` requests.
    scheduler: usize,
    /// `/v2/scheduler/history` requests.
    history: usize,
}

impl Hits {
    /// The hits that happened between two readings.
    fn since(self, earlier: Self) -> Self {
        Self {
            status: self.status.saturating_sub(earlier.status),
            scheduler: self.scheduler.saturating_sub(earlier.scheduler),
            history: self.history.saturating_sub(earlier.history),
        }
    }
}

/// A `greggd`-shaped remote that counts the requests it receives, per route.
struct CountedRemote {
    /// The port it listens on.
    port: u16,
    /// `/v2/status` hits.
    status: Arc<AtomicUsize>,
    /// `/v2/scheduler` hits.
    scheduler: Arc<AtomicUsize>,
    /// `/v2/scheduler/history` hits.
    history: Arc<AtomicUsize>,
}

impl CountedRemote {
    /// The current counters.
    fn hits(&self) -> Hits {
        Hits {
            status: self.status.load(Ordering::SeqCst),
            scheduler: self.scheduler.load(Ordering::SeqCst),
            history: self.history.load(Ordering::SeqCst),
        }
    }
}

/// Serve one counted `greggd`-shaped remote.
///
/// `scheduler_capable` decides whether the scheduler routes exist. A modern
/// remote serves the summary and the history body; an old one answers `404`
/// for both, which is exactly what a `greggd` from before the scheduler plane
/// did — a healthy daemon with an additive route missing.
async fn spawn_counted_remote(scheduler_capable: bool) -> CountedRemote {
    let status_body = serde_json::to_vec(
        &gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
            .sample_interval_ms(1_000)
            .build_payload(),
    )
    .expect("the status document serializes");
    let summary_body = scheduler_summary_body();
    let history_body = scheduler_history_body();

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let status = Arc::new(AtomicUsize::new(0));
    let scheduler = Arc::new(AtomicUsize::new(0));
    let history = Arc::new(AtomicUsize::new(0));

    let task_status = Arc::clone(&status);
    let task_scheduler = Arc::clone(&scheduler);
    let task_history = Arc::clone(&history);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let status = Arc::clone(&task_status);
            let scheduler = Arc::clone(&task_scheduler);
            let history = Arc::clone(&task_history);
            let status_body = status_body.clone();
            let summary_body = summary_body.clone();
            let history_body = history_body.clone();
            tokio::spawn(async move {
                let Some(target) = read_request_target(&mut stream).await else {
                    return;
                };
                let (counter, body) = if target.starts_with("/v2/status") {
                    (status, Some(status_body))
                } else if target.starts_with("/v2/scheduler/history") {
                    (history, scheduler_capable.then_some(history_body))
                } else if target.starts_with("/v2/scheduler") {
                    (scheduler, scheduler_capable.then_some(summary_body))
                } else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let response = match body {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        String::from_utf8_lossy(&body)
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_owned()
                    }
                };
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });

    CountedRemote {
        port,
        status,
        scheduler,
        history,
    }
}

/// The fixture's scheduler epoch. Fixed, so the history gate never advances.
fn scheduler_epoch() -> gregg_protocol::SchedulerEpochV2 {
    gregg_protocol::SchedulerEpochV2 {
        started_at_unix_ms: 1_700_000_000_000,
        nonce: 7,
    }
}

/// The fixture's scheduler summary document.
fn scheduler_summary_body() -> Vec<u8> {
    let summary = gregg_protocol::SchedulerSummaryV2 {
        schema_version: 2,
        generated_at_unix_ms: 1_700_000_000_000,
        epoch: scheduler_epoch(),
        history_revision: 4,
        jobs: vec![gregg_protocol::SchedulerJobV2 {
            name: "backup".to_owned(),
            schedule: "0 3 * * *".to_owned(),
            next_due_unix_ms: 1_700_100_000_000,
            state: gregg_protocol::SchedulerJobStateV2::Idle,
            load: None,
            pending_since_unix_ms: None,
            next_retry_unix_ms: None,
            running_since_unix_ms: None,
            last: Some(gregg_protocol::SchedulerRunSummaryV2 {
                sequence: 1,
                scheduled_unix_ms: 1_700_000_000_000,
                finished_unix_ms: 1_700_000_001_000,
                outcome: gregg_protocol::SchedulerOutcomeV2::Success,
                exit_code: Some(0),
                signal: None,
                duration_ms: Some(1_000),
                delay_ms: 0,
                coalesced: false,
            }),
        }],
    };
    serde_json::to_vec(&summary).expect("the scheduler summary serializes")
}

/// The fixture's scheduler history document.
///
/// Its epoch and revision match the summary's, so the pair is coherent and
/// the daemon's history gate advances exactly once.
fn scheduler_history_body() -> Vec<u8> {
    let history = gregg_protocol::SchedulerHistoryV2 {
        schema_version: 2,
        generated_at_unix_ms: 1_700_000_000_000,
        epoch: scheduler_epoch(),
        history_revision: 4,
        jobs: vec![gregg_protocol::SchedulerJobHistoryV2 {
            name: "backup".to_owned(),
            records: vec![gregg_protocol::SchedulerRunRecordV2 {
                sequence: 1,
                scheduled_unix_ms: 1_700_000_000_000,
                started_unix_ms: Some(1_700_000_000_100),
                finished_unix_ms: 1_700_000_001_000,
                outcome: gregg_protocol::SchedulerOutcomeV2::Success,
                exit_code: Some(0),
                signal: None,
                duration_ms: Some(900),
                delay_ms: 0,
                coalesced: false,
                stdout: gregg_protocol::SchedulerOutputV2::new("ok\n".to_owned(), false),
                stderr: gregg_protocol::SchedulerOutputV2::new(String::new(), false),
            }],
        }],
    };
    serde_json::to_vec(&history).expect("the scheduler history serializes")
}

/// A counted `EggPool`-shaped remote.
struct CountedEggpoolRemote {
    /// The port it listens on.
    port: u16,
    /// `/api/stats/summary` hits.
    summary: Arc<AtomicUsize>,
    /// `/api/status` hits.
    status: Arc<AtomicUsize>,
    /// The period each summary request asked for, in order.
    periods: Arc<Mutex<Vec<String>>>,
}

impl CountedEggpoolRemote {
    /// The summary requests seen so far, in order.
    fn periods(&self) -> Vec<String> {
        self.periods
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How many summary requests have been served.
    fn summary_hits(&self) -> usize {
        self.summary.load(Ordering::SeqCst)
    }

    /// How many status requests have been served.
    fn status_hits(&self) -> usize {
        self.status.load(Ordering::SeqCst)
    }
}

/// Serve one counted `EggPool`-shaped remote.
///
/// It answers the summary route with the period the request asked for, so the
/// window the daemon drove the worker with is observable from the outside,
/// and the schema-version-1 status route with a valid snapshot.
async fn spawn_counted_eggpool_remote() -> CountedEggpoolRemote {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let summary = Arc::new(AtomicUsize::new(0));
    let status = Arc::new(AtomicUsize::new(0));
    let periods = Arc::new(Mutex::new(Vec::new()));

    let task_summary = Arc::clone(&summary);
    let task_status = Arc::clone(&status);
    let task_periods = Arc::clone(&periods);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let summary = Arc::clone(&task_summary);
            let status = Arc::clone(&task_status);
            let periods = Arc::clone(&task_periods);
            tokio::spawn(async move {
                let Some(target) = read_request_target(&mut stream).await else {
                    return;
                };
                let body = if target.starts_with("/api/stats/summary") {
                    summary.fetch_add(1, Ordering::SeqCst);
                    let period = query_value(&target, "period").unwrap_or_else(|| "1h".to_owned());
                    periods
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(period.clone());
                    format!(
                        "{{\"period\":\"{period}\",\"accounted_tokens\":100,\"cache_read_ratio\":null,\
                         \"tokens_per_second\":1.5,\"avg_ttft_ms\":12.0,\"streamed_requests\":0}}"
                    )
                } else if target.starts_with("/api/status") {
                    status.fetch_add(1, Ordering::SeqCst);
                    eggpool_status_body()
                } else {
                    return;
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });

    CountedEggpoolRemote {
        port,
        summary,
        status,
        periods,
    }
}

/// A valid schema-version-1 `EggPool` status snapshot.
fn eggpool_status_body() -> String {
    "{\"schema_version\":1,\"proxy\":{\"status\":\"ready\",\"available\":true,\"uptime_seconds\":10.0,\
     \"model_count\":2,\"routable_accounts\":1,\"enabled_accounts\":1},\"providers\":[]}"
        .to_owned()
}

/// Read one HTTP request head and return its request target.
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut request = String::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        request.push_str(&String::from_utf8_lossy(&chunk[..count]));
        if request.contains("\r\n\r\n") {
            break;
        }
    }
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .map(str::to_owned)
}

/// One query parameter's value.
fn query_value(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_owned())
    })
}

// ---------------------------------------------------------------------------
// Fixture configuration
// ---------------------------------------------------------------------------

/// One monitored system in the fixture config.
struct FixtureSystem {
    /// The system's stable id.
    id: &'static str,
    /// The port it is polled on.
    port: u16,
    /// Its display name.
    name: &'static str,
}

/// Write a valid `gregg.toml` for the fixture.
///
/// `config_version` is required and a system's id is a UUID v4, so both are
/// spelled the way the real CLI writes them.
fn write_config(path: &Path, systems: &[FixtureSystem], eggpool_port: Option<u16>) {
    let mut config = String::new();
    writeln!(config, "config_version = 1").expect("writing to a string cannot fail");
    writeln!(config, "request_timeout_ms = 1000").expect("writing to a string cannot fail");
    writeln!(config, "refresh_seconds = {REFRESH_SECONDS}")
        .expect("writing to a string cannot fail");
    writeln!(config, "max_concurrent_requests = 4").expect("writing to a string cannot fail");
    writeln!(config, "default_port = 11310").expect("writing to a string cannot fail");
    for system in systems {
        writeln!(config).expect("writing to a string cannot fail");
        writeln!(config, "[[systems]]").expect("writing to a string cannot fail");
        writeln!(config, "id = \"{}\"", system.id).expect("writing to a string cannot fail");
        writeln!(config, "host = \"127.0.0.1\"").expect("writing to a string cannot fail");
        writeln!(config, "port = {}", system.port).expect("writing to a string cannot fail");
        writeln!(config, "name = \"{}\"", system.name).expect("writing to a string cannot fail");
    }
    if let Some(port) = eggpool_port {
        writeln!(config).expect("writing to a string cannot fail");
        writeln!(config, "[eggpool]").expect("writing to a string cannot fail");
        writeln!(config, "id = \"{EGGPOOL_ENTRY_ID}\"").expect("writing to a string cannot fail");
        writeln!(config, "host = \"127.0.0.1\"").expect("writing to a string cannot fail");
        writeln!(config, "port = {port}").expect("writing to a string cannot fail");
        writeln!(config, "scheme = \"http\"").expect("writing to a string cannot fail");
        writeln!(config, "name = \"pool\"").expect("writing to a string cannot fail");
    }
    std::fs::write(path, config).expect("writes the fixture config");
}

/// A unique temporary directory for one fixture.
fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gregg-ev-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

/// An executable that exists, is not this test binary, and is never spawned.
///
/// The daemon under test is already running, so the launcher's probe finds it
/// and the executable is only ever named, never run.
#[cfg(unix)]
fn harmless_executable() -> PathBuf {
    PathBuf::from("/bin/true")
}

/// An executable that exists, is not this test binary, and is never spawned.
#[cfg(windows)]
fn harmless_executable() -> PathBuf {
    PathBuf::from("cmd.exe")
}

// ---------------------------------------------------------------------------
// The daemon and its frontends
// ---------------------------------------------------------------------------

/// The client daemon under test.
struct DaemonHandle {
    /// The config-specific identity it serves.
    identity: ClientDaemonIdentity,
    /// The token that stops it.
    cancel: CancellationToken,
    /// Its task.
    task: Option<JoinHandle<()>>,
}

impl DaemonHandle {
    /// Stop the daemon and wait for it to release its endpoint.
    async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Start the real client daemon for `config_path` and wait until it serves.
async fn start_daemon(config_path: &Path) -> DaemonHandle {
    let identity = ClientDaemonIdentity::for_path(config_path);
    let store = ConfigStore::new(config_path.to_path_buf());
    let cancel = CancellationToken::new();
    let daemon_cancel = cancel.clone();
    let daemon_identity = identity.clone();
    let task = tokio::spawn(async move {
        if let Err(error) =
            gregg::clientd::daemon::run_daemon(store, daemon_identity, daemon_cancel).await
        {
            eprintln!("the client daemon stopped: {error:?}");
        }
    });
    wait_for_daemon(&identity, &task).await;
    DaemonHandle {
        identity,
        cancel,
        task: Some(task),
    }
}

/// Wait until the daemon's endpoint is being served.
async fn wait_for_daemon(identity: &ClientDaemonIdentity, task: &JoinHandle<()>) {
    for _ in 0..400 {
        let running = gregg::clientd::daemon::status(identity)
            .await
            .is_ok_and(|status| status.running);
        if running {
            return;
        }
        assert!(
            !task.is_finished(),
            "the client daemon exited before it could serve"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the client daemon did not come up");
}

/// A second frontend, attached exactly the way the terminal client attaches.
///
/// It uses the same public client-daemon API the TUI uses — the same
/// handshake, the same frame stream, the same control requests — so the
/// harness exercises a genuine two-frontend situation rather than two copies
/// of the GUI.
struct TuiFrontend {
    /// The control-request side.
    sender: FrontendSender,
    /// The endpoint the attachment reached.
    endpoint: PathBuf,
    /// Every document this frontend has received.
    documents: Arc<Mutex<Vec<FrontendSnapshot>>>,
    /// The reader task.
    pump: Option<JoinHandle<()>>,
}

impl TuiFrontend {
    /// Attach to the daemon for `identity`.
    async fn attach(identity: &ClientDaemonIdentity) -> Self {
        let attachment = gregg::clientd::daemon::attach(identity, env!("CARGO_PKG_VERSION"))
            .await
            .expect("the terminal-style frontend attaches");
        let gregg::clientd::Attachment {
            frontend,
            frames,
            endpoint,
            ..
        } = attachment;
        let documents = Arc::new(Mutex::new(Vec::new()));
        let pump = spawn_collector(frames, Arc::clone(&documents));
        Self {
            sender: frontend,
            endpoint,
            documents,
            pump: Some(pump),
        }
    }

    /// Every document received so far.
    fn documents(&self) -> Vec<FrontendSnapshot> {
        self.documents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The endpoint this frontend reached.
    fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    /// Close the connection, as a closed window would.
    fn close(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
    }
}

impl Drop for TuiFrontend {
    fn drop(&mut self) {
        self.close();
    }
}

/// Read frames until the connection ends, keeping every document.
fn spawn_collector(
    mut frames: gregg::clientd::FrameStream,
    documents: Arc<Mutex<Vec<FrontendSnapshot>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(frame) = frames.next().await {
            if let FrontendFrame::Snapshot(snapshot) = frame {
                documents
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(*snapshot);
            }
        }
    })
}

/// The documents a GUI bridge has published since the last drain.
fn drain_gui(gui: &Bridge) -> Vec<FrontendSnapshot> {
    gui.drain()
        .into_iter()
        .filter_map(|message| match message {
            BridgeMessage::Snapshot { snapshot } => Some(*snapshot),
            _ => None,
        })
        .collect()
}

/// Wait until one of the values a source produces satisfies `predicate`, or the
/// deadline passes.
async fn wait_for<T, F, P>(source: F, predicate: P) -> Option<T>
where
    F: Fn() -> Vec<T>,
    P: Fn(&T) -> bool,
{
    let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
    loop {
        for value in source() {
            if predicate(&value) {
                return Some(value);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait until the whole collection a source produces satisfies `predicate`, or
/// the deadline passes.
async fn wait_for_all<T, F, P>(source: F, predicate: P) -> Option<Vec<T>>
where
    F: Fn() -> Vec<T>,
    P: Fn(&[T]) -> bool,
{
    let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
    loop {
        let values = source();
        if predicate(&values) {
            return Some(values);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether every system in a document is online.
fn all_online(snapshot: &FrontendSnapshot) -> bool {
    !snapshot.systems.is_empty()
        && snapshot
            .systems
            .iter()
            .all(|system| system.reachability == Reachability::Online)
}

/// The scheduler entry for one system, if the document carries one.
fn cron_entry<'a>(
    snapshot: &'a FrontendSnapshot,
    system_id: &str,
) -> Option<&'a gregg::clientd::snapshot::SystemCronDto> {
    snapshot
        .cron
        .iter()
        .find(|entry| entry.system_id == system_id)
}

/// Whether a document advertises the given converged `EggPool` window.
fn advertises_eggpool_period(snapshot: &FrontendSnapshot, period: EggpoolPeriod) -> bool {
    snapshot
        .eggpool
        .as_ref()
        .is_some_and(|eggpool| eggpool.period == period)
}

/// Whether a document reports the `EggPool` worker as idle.
fn eggpool_worker_is_idle(snapshot: &FrontendSnapshot) -> bool {
    snapshot
        .eggpool
        .as_ref()
        .is_some_and(|eggpool| eggpool.worker_state == EggpoolWorkerState::Idle)
}

/// Assert that the `EggPool` remote was never asked for the month window.
///
/// A worker per frontend would have asked for it; one converged worker only
/// ever asks for the shortest window any active frontend asked for.
fn assert_only_the_converged_window_was_fetched(pool: &CountedEggpoolRemote) {
    assert!(
        !pool.periods().iter().any(|period| period == "30d"),
        "a worker per frontend would have asked for the month window: {:?}",
        pool.periods()
    );
}

/// The size of the local IPC document for one snapshot.
///
/// The daemon encodes exactly this frame, so the number is the document's
/// real size rather than an estimate of it.
fn document_bytes(snapshot: &FrontendSnapshot) -> usize {
    serde_json::to_vec(&FrontendFrame::Snapshot(Box::new(snapshot.clone())))
        .expect("the document serializes")
        .len()
}

// ---------------------------------------------------------------------------
// The counted multi-frontend proof
// ---------------------------------------------------------------------------

/// One measurement window's worth of per-remote hits.
#[derive(Debug, Clone, Copy)]
struct Window {
    /// The modern remote's hits.
    modern: Hits,
    /// The old remote's hits.
    old: Hits,
}

/// Measure both remotes over one window.
async fn measure_window(modern: &CountedRemote, old: &CountedRemote) -> Window {
    let modern_before = modern.hits();
    let old_before = old.hits();
    tokio::time::sleep(WINDOW).await;
    Window {
        modern: modern.hits().since(modern_before),
        old: old.hits().since(old_before),
    }
}

/// Assert that one phase's counts sit inside the baseline's slack.
fn assert_phase_inside_baseline(phase: &str, baseline: &Window, measured: &Window) {
    assert!(
        measured.modern.status <= baseline.modern.status + COUNT_TOLERANCE,
        "{phase} must not add metrics polls: {:?} then {:?}",
        baseline.modern,
        measured.modern
    );
    assert!(
        measured.modern.status >= 1,
        "{phase} must still be polled: {:?}",
        measured.modern
    );
    assert!(
        measured.old.status <= baseline.old.status + COUNT_TOLERANCE,
        "{phase} must not add metrics polls to the old remote: {:?} then {:?}",
        baseline.old,
        measured.old
    );
    assert!(
        measured.old.status >= 1,
        "{phase} must still poll the old remote: {:?}",
        measured.old
    );
    assert!(
        measured.modern.scheduler <= baseline.modern.scheduler + SCHEDULER_TOLERANCE,
        "{phase} must not add scheduler summary polls: {:?} then {:?}",
        baseline.modern,
        measured.modern
    );
    assert_eq!(
        measured.modern.history, 0,
        "{phase} must not fetch the scheduler history body again: {:?} then {:?}",
        baseline.modern, measured.modern
    );
}

/// The frontend count does not change what the daemon asks the remotes for.
///
/// One measurement window is taken four times over the same daemon and the
/// same two remotes: the daemon alone, then with a terminal-style frontend
/// attached, then with a GUI attached as well, then with both frontends closed
/// again. The metrics counts must stay inside the cadence's own slack in
/// every phase, the scheduler summary must not grow beyond its thirty-second
/// cadence, and the scheduler history body must not be fetched again at all:
/// it is fetched once per `(epoch, revision)`, and nothing here changes
/// either.
#[tokio::test]
async fn two_frontends_do_not_multiply_the_fleet_request_budget() {
    let modern = spawn_counted_remote(true).await;
    let old = spawn_counted_remote(false).await;
    let dir = temp_dir("budget");
    let config_path = dir.join("gregg.toml");
    write_config(
        &config_path,
        &[
            FixtureSystem {
                id: MODERN_SYSTEM_ID,
                port: modern.port,
                name: "modern",
            },
            FixtureSystem {
                id: OLD_SYSTEM_ID,
                port: old.port,
                name: "old",
            },
        ],
        None,
    );

    let daemon = start_daemon(&config_path).await;

    // The daemon must have polled before anything is measured, or the counts
    // would be measuring a daemon that never started.
    assert!(
        wait_for(|| vec![modern.hits().status], |hits| *hits > 0)
            .await
            .is_some(),
        "the daemon must poll its fleet"
    );

    // --- phase one: the daemon alone --------------------------------------
    let baseline = measure_window(&modern, &old).await;

    // --- phase two: one frontend -------------------------------------------
    let mut tui = TuiFrontend::attach(&daemon.identity).await;
    let tui_endpoint = tui.endpoint().to_path_buf();
    assert!(
        wait_for(|| tui.documents(), all_online).await.is_some(),
        "the terminal-style frontend must see the fleet"
    );
    let with_tui = measure_window(&modern, &old).await;

    // --- phase three: two frontends ----------------------------------------
    let gui = Bridge::spawn(
        config_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );
    let gui_ready = wait_for(
        || drain_gui(&gui),
        |snapshot| {
            all_online(snapshot)
                && cron_entry(snapshot, MODERN_SYSTEM_ID).is_some()
                && cron_entry(snapshot, OLD_SYSTEM_ID).is_some()
        },
    )
    .await;
    assert!(
        gui_ready.is_some(),
        "the GUI must see the fleet and both scheduler capabilities"
    );
    let with_two = measure_window(&modern, &old).await;

    // --- phase four: both frontends closed ---------------------------------
    drop(gui);
    tui.close();
    drop(tui);
    let after_close = measure_window(&modern, &old).await;

    // The metrics plane: one poll per refresh interval, whatever the number
    // of frontends — including none.
    assert!(
        baseline.modern.status >= 2,
        "the daemon must poll several times during the baseline window: {:?}",
        baseline.modern
    );
    assert_phase_inside_baseline("with one frontend", &baseline, &with_tui);
    assert_phase_inside_baseline("with two frontends", &baseline, &with_two);
    assert_phase_inside_baseline("after the frontends closed", &baseline, &after_close);

    // The scheduler history body: fetched once per `(epoch, revision)`, and
    // neither changes while the frontends come and go.
    assert_eq!(
        modern.hits().history,
        1,
        "the scheduler history body is fetched exactly once, at discovery"
    );
    assert_eq!(
        old.hits().history,
        0,
        "an old remote with no scheduler routes is never asked for history"
    );

    // The two remotes are told apart by their capability, not by an error: an
    // old daemon is a healthy daemon with an additive route missing.
    let snapshot = gui_ready.expect("the GUI must see the fleet");
    assert_eq!(
        cron_entry(&snapshot, MODERN_SYSTEM_ID).map(|entry| entry.capability),
        Some(CronCapability::Supported),
        "the modern remote must be reported as scheduler-capable"
    );
    assert_eq!(
        cron_entry(&snapshot, OLD_SYSTEM_ID).map(|entry| entry.capability),
        Some(CronCapability::Unsupported),
        "the old remote must be reported as scheduler-incapable, not as an error"
    );

    print_budget_table(&baseline, &with_tui, &with_two, &after_close, &modern, &old);
    println!(
        "daemon id {} on {}\ntest process pid {}",
        daemon.identity.id(),
        tui_endpoint.display(),
        std::process::id(),
    );

    daemon.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Print the counted table the evidence record quotes.
fn print_budget_table(
    baseline: &Window,
    with_tui: &Window,
    with_two: &Window,
    after_close: &Window,
    modern: &CountedRemote,
    old: &CountedRemote,
) {
    println!(
        "request budget over {WINDOW:?} windows\n\
         remote  route                 daemon-alone  with-tui  tui+gui  closed\n\
         modern  /v2/status            {:>11}  {:>8}  {:>6}  {:>6}\n\
         modern  /v2/scheduler         {:>11}  {:>8}  {:>6}  {:>6}\n\
         modern  /v2/scheduler/history {:>11}  {:>8}  {:>6}  {:>6}\n\
         old     /v2/status            {:>11}  {:>8}  {:>6}  {:>6}\n\
         old     /v2/scheduler         {:>11}  {:>8}  {:>6}  {:>6}\n\
         old     /v2/scheduler/history {:>11}  {:>8}  {:>6}  {:>6}\n\
         totals for the daemon's whole life: modern {modern_total:?} old {old_total:?}",
        baseline.modern.status,
        with_tui.modern.status,
        with_two.modern.status,
        after_close.modern.status,
        baseline.modern.scheduler,
        with_tui.modern.scheduler,
        with_two.modern.scheduler,
        after_close.modern.scheduler,
        baseline.modern.history,
        with_tui.modern.history,
        with_two.modern.history,
        after_close.modern.history,
        baseline.old.status,
        with_tui.old.status,
        with_two.old.status,
        after_close.old.status,
        baseline.old.scheduler,
        with_tui.old.scheduler,
        with_two.old.scheduler,
        after_close.old.scheduler,
        baseline.old.history,
        with_tui.old.history,
        with_two.old.history,
        after_close.old.history,
        modern_total = modern.hits(),
        old_total = old.hits(),
    );
}

/// An opened cron job grows the local document but not the remote request
/// budget.
///
/// The GUI's cron intent governs what the daemon *transmits*, never what it
/// fetches: the summary is polled on the daemon's own cadence and the history
/// body only when the remote's `(epoch, revision)` changes. So opening a job
/// must make the local IPC document carry that job's records, while the
/// remote's history counter stays exactly where the discovery left it.
#[tokio::test]
async fn an_opened_cron_job_grows_the_local_document_but_not_the_remote_budget() {
    let modern = spawn_counted_remote(true).await;
    let dir = temp_dir("cron");
    let config_path = dir.join("gregg.toml");
    write_config(
        &config_path,
        &[FixtureSystem {
            id: MODERN_SYSTEM_ID,
            port: modern.port,
            name: "modern",
        }],
        None,
    );

    let daemon = start_daemon(&config_path).await;
    let gui = Bridge::spawn(
        config_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );

    // The scheduler plane must have been read once before the intent can
    // publish anything.
    let ready = wait_for(
        || drain_gui(&gui),
        |snapshot| {
            cron_entry(snapshot, MODERN_SYSTEM_ID)
                .and_then(|entry| entry.summary.as_ref())
                .is_some()
        },
    )
    .await;
    let before = ready.expect("the daemon must publish the scheduler summary");
    let before_bytes = document_bytes(&before);
    let history_before = modern.hits().history;
    assert_eq!(
        history_before, 1,
        "the history body is fetched once, at discovery"
    );

    // Open the cron job in the GUI.
    gui.set_cron_intent(Some(CronDesired {
        system_id: MODERN_SYSTEM_ID.to_owned(),
        job: "backup".to_owned(),
        display_history: 5,
    }));

    let opened = wait_for(
        || drain_gui(&gui),
        |snapshot| {
            cron_entry(snapshot, MODERN_SYSTEM_ID)
                .is_some_and(|entry| entry.history.iter().any(|job| job.job == "backup"))
        },
    )
    .await;
    let opened = opened.expect("the opened job's records must be published");
    let opened_bytes = document_bytes(&opened);
    assert!(
        opened_bytes >= before_bytes + MIN_HISTORY_GROWTH_BYTES,
        "the local document must grow to carry the job's records: {before_bytes} then {opened_bytes}"
    );
    assert_eq!(
        modern.hits().history,
        history_before,
        "opening a cron job must not fetch the history body again"
    );
    assert!(
        modern.hits().scheduler <= 2,
        "opening a cron job must not add a scheduler summary poll: {:?}",
        modern.hits()
    );

    // Close the view: the records stop being transmitted.
    gui.set_cron_intent(None);
    let closed = wait_for(
        || drain_gui(&gui),
        |snapshot| {
            cron_entry(snapshot, MODERN_SYSTEM_ID).is_some_and(|entry| entry.history.is_empty())
        },
    )
    .await;
    assert!(
        closed.is_some(),
        "closing the cron view must stop the records being transmitted"
    );
    assert_eq!(
        modern.hits().history,
        history_before,
        "closing a cron job must not fetch the history body either"
    );

    println!(
        "cron intent: local document {before_bytes} -> {opened_bytes} bytes; \
         remote counters after the whole test: {hits:?}",
        hits = modern.hits()
    );

    daemon.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `EggPool` view converges one daemon-owned worker, not one per
/// frontend.
///
/// The daemon owns a single `EggPool` worker and reduces every attached
/// frontend's `(active, period)` intent into one converged desired state:
/// active when any frontend has the pane open, and the **shortest** window any
/// of them asked for.
///
/// The counting proof is the window the remote is asked for. The GUI asks for
/// the week window and the terminal-style frontend asks for the month window;
/// a worker per frontend would ask the remote for *both* windows, while one
/// converged worker only ever asks for the shortest one. So the remote must
/// never see the month window — and when the last pane closes, the request
/// stream must stop rather than one worker per frontend keeping half of them
/// alive.
#[tokio::test]
async fn the_eggpool_view_converges_one_daemon_owned_worker() {
    let pool = spawn_counted_eggpool_remote().await;
    let dir = temp_dir("egg");
    let config_path = dir.join("gregg.toml");
    write_config(
        &config_path,
        &[FixtureSystem {
            id: MODERN_SYSTEM_ID,
            port: 1,
            name: "modern",
        }],
        Some(pool.port),
    );

    let daemon = start_daemon(&config_path).await;
    let tui = TuiFrontend::attach(&daemon.identity).await;
    let gui = Bridge::spawn(
        config_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );

    // The GUI opens the `EggPool` view on the week window.
    gui.set_eggpool_intent(Some(EggpoolDesired {
        active: true,
        period: EggpoolPeriod::Week,
        refresh: false,
    }));
    let fetched = wait_for_all(
        || pool.periods(),
        |periods| !periods.is_empty() && periods.iter().all(|period| period == "7d"),
    )
    .await;
    assert!(
        fetched.is_some(),
        "the daemon must fetch the window the GUI asked for: {:?}",
        pool.periods()
    );
    assert!(
        pool.status_hits() >= 1,
        "the worker fetches the health plane alongside the summary"
    );

    // The terminal-style frontend opens the same view on the *month* window.
    // The converged window is still the week one, so the remote must not be
    // asked for the month window at all.
    tui.sender
        .request_eggpool_intent(true, EggpoolPeriod::Month, false, 1)
        .expect("the terminal-style frontend publishes its intent");
    let joined = wait_for_all(
        || pool.periods(),
        |periods| periods.len() >= 2 && periods.iter().all(|period| period == "7d"),
    )
    .await;
    assert!(
        joined.is_some(),
        "one converged worker must only ever ask for the shortest window: {:?}",
        pool.periods()
    );
    assert_only_the_converged_window_was_fetched(&pool);

    // The daemon advertises the converged window to every frontend.
    let advertised = wait_for(
        || tui.documents(),
        |snapshot| advertises_eggpool_period(snapshot, EggpoolPeriod::Week),
    )
    .await;
    assert!(
        advertised.is_some(),
        "the daemon must advertise the converged window, not one frontend's request"
    );
    let gui_advertised = wait_for(
        || drain_gui(&gui),
        |snapshot| advertises_eggpool_period(snapshot, EggpoolPeriod::Week),
    )
    .await;
    assert!(
        gui_advertised.is_some(),
        "every frontend must be advertised the same converged window"
    );

    // The terminal-style frontend closes its view. The GUI still holds the
    // pane open, so the worker stays active and keeps the same window.
    let before_release = pool.summary_hits();
    tui.sender
        .request_eggpool_intent(false, EggpoolPeriod::Month, false, 2)
        .expect("the terminal-style frontend releases its intent");
    let released = wait_for_all(|| pool.periods(), |periods| periods.len() > before_release).await;
    assert!(
        released.is_some(),
        "the released frontend's transition must be driven by the same worker"
    );
    assert_only_the_converged_window_was_fetched(&pool);

    // The last pane closes: the worker converges to inactive and stops asking
    // the remote for anything.
    gui.set_eggpool_intent(None);
    let idle = wait_for(|| tui.documents(), eggpool_worker_is_idle).await;
    assert!(
        idle.is_some(),
        "the last pane leaving must converge the worker to inactive"
    );
    let hits = pool.summary_hits();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        pool.summary_hits(),
        hits,
        "an inactive worker must not keep asking the remote: {:?}",
        pool.periods()
    );

    println!(
        "eggpool: windows the remote was asked for: {periods:?}; \
         summary requests {summary}, status requests {status}",
        periods = pool.periods(),
        summary = pool.summary_hits(),
        status = pool.status_hits()
    );

    daemon.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The daemon outlives its frontends and hands a reattaching one the newest
/// document.
///
/// Closing every frontend must not stop the daemon: it keeps polling on its
/// configured cadence, and a frontend that comes back is handed the newest
/// coherent document rather than an empty one — and the daemon does not
/// rebuild its scheduler history from zero to serve it.
#[tokio::test]
async fn the_daemon_outlives_its_frontends_and_reattaches_with_the_newest_document() {
    let modern = spawn_counted_remote(true).await;
    let dir = temp_dir("lifecycle");
    let config_path = dir.join("gregg.toml");
    write_config(
        &config_path,
        &[FixtureSystem {
            id: MODERN_SYSTEM_ID,
            port: modern.port,
            name: "modern",
        }],
        None,
    );

    let daemon = start_daemon(&config_path).await;
    let mut tui = TuiFrontend::attach(&daemon.identity).await;
    let gui = Bridge::spawn(
        config_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );
    let seen = wait_for(|| tui.documents(), all_online).await;
    let seen = seen.expect("the terminal-style frontend must see the fleet");
    let last_generation = seen.generation;
    let first_endpoint = tui.endpoint().to_path_buf();
    let history_before = modern.hits().history;

    // Both frontends close.
    drop(gui);
    tui.close();
    drop(tui);

    // The daemon is still there, and still polling.
    let status = gregg::clientd::daemon::status(&daemon.identity)
        .await
        .expect("the daemon must answer");
    assert!(
        status.running,
        "the daemon must survive its frontends closing"
    );

    let before = modern.hits().status;
    tokio::time::sleep(WINDOW).await;
    let polled = modern.hits().status.saturating_sub(before);
    assert!(
        polled >= 2,
        "the daemon must keep polling on its cadence after the frontends close: {polled}"
    );

    // A new frontend attaches and is handed the newest document.
    let again = TuiFrontend::attach(&daemon.identity).await;
    assert_eq!(
        again.endpoint(),
        first_endpoint,
        "a reattaching frontend must reach the same daemon endpoint"
    );
    let fresh = wait_for(
        || again.documents(),
        |snapshot| snapshot.poll_initialized && all_online(snapshot),
    )
    .await;
    let fresh = fresh.expect("a reattaching frontend must receive the fleet");
    assert!(
        fresh.generation >= last_generation,
        "the reattaching frontend must receive the newest document: {last_generation} then {}",
        fresh.generation
    );
    assert!(
        fresh.produced_at_unix_ms >= seen.produced_at_unix_ms,
        "the reattaching frontend must receive a document produced after the previous one"
    );
    assert_eq!(
        modern.hits().history,
        history_before,
        "the daemon must not rebuild its scheduler history for a reattaching frontend"
    );

    daemon.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two config paths reach two daemons, and two spellings of one config path
/// reach one.
///
/// The identity is derived from the *normalized* config path, so a relative
/// segment or a `..` cannot produce a second daemon for one file. Each daemon
/// serves only its own fleet, which is what makes the two daemons observably
/// separate rather than merely differently named.
#[tokio::test]
async fn distinct_config_paths_reach_distinct_daemons_and_one_spelling_reaches_one() {
    let remote = spawn_counted_remote(false).await;
    let first_dir = temp_dir("alpha");
    let second_dir = temp_dir("beta");
    let first_path = first_dir.join("gregg.toml");
    let second_path = second_dir.join("gregg.toml");
    write_config(
        &first_path,
        &[FixtureSystem {
            id: MODERN_SYSTEM_ID,
            port: remote.port,
            name: "alpha",
        }],
        None,
    );
    write_config(
        &second_path,
        &[FixtureSystem {
            id: OLD_SYSTEM_ID,
            port: remote.port,
            name: "beta",
        }],
        None,
    );

    let first = start_daemon(&first_path).await;
    let second = start_daemon(&second_path).await;
    assert_ne!(
        first.identity.id(),
        second.identity.id(),
        "two config paths must be two identities"
    );

    let first_gui = Bridge::spawn(
        first_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );
    let second_gui = Bridge::spawn(
        second_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );
    // The same file, spelled differently.
    let spelled_gui = Bridge::spawn(
        first_dir.join(".").join("gregg.toml"),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );

    let first_id = gui_daemon_id(&first_gui).await;
    let second_id = gui_daemon_id(&second_gui).await;
    let spelled_id = gui_daemon_id(&spelled_gui).await;
    assert_ne!(
        first_id, second_id,
        "two config paths must reach two different daemons"
    );
    assert_eq!(
        first_id, spelled_id,
        "two spellings of one config path must reach one daemon"
    );

    // Each daemon serves only its own fleet.
    let first_fleet = wait_for(|| drain_gui(&first_gui), all_online)
        .await
        .expect("the first GUI must see its fleet");
    let second_fleet = wait_for(|| drain_gui(&second_gui), all_online)
        .await
        .expect("the second GUI must see its fleet");
    assert_eq!(
        first_fleet.systems.len(),
        1,
        "the first daemon serves one system"
    );
    assert_eq!(
        second_fleet.systems.len(),
        1,
        "the second daemon serves one system"
    );
    assert_eq!(
        first_fleet.systems[0].configured_name.as_deref(),
        Some("alpha"),
        "the first daemon must serve its own system"
    );
    assert_eq!(
        second_fleet.systems[0].configured_name.as_deref(),
        Some("beta"),
        "the second daemon must serve its own system"
    );

    first.stop().await;
    second.stop().await;
    let _ = std::fs::remove_dir_all(&first_dir);
    let _ = std::fs::remove_dir_all(&second_dir);
}

/// Wait for the GUI to report which daemon it attached to.
async fn gui_daemon_id(gui: &Bridge) -> String {
    let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
    loop {
        for message in gui.drain() {
            if let BridgeMessage::Attached { daemon_id, .. } = message {
                return daemon_id;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the GUI never reported the daemon it attached to"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The GUI refuses a daemon that speaks another local protocol, and leaves it
/// running.
///
/// A version mismatch is not a lost connection: the GUI must say so instead
/// of rendering documents whose meaning it cannot interpret, and it must not
/// stop or replace the peer it collided with. The peer here is a stand-in
/// that completes the handshake and then reports a newer protocol version —
/// the only shape in which the GUI would otherwise treat the daemon as
/// usable.
///
/// "Not stopped" is counted, not assumed: the stand-in counts the connections
/// it accepts, and a stop or a replacement would have to come back over the
/// same endpoint. One connection is the whole story — the GUI's own.
#[cfg(unix)]
#[tokio::test]
async fn the_gui_refuses_an_incompatible_daemon_without_stopping_it() {
    let dir = temp_dir("incompatible");
    let config_path = dir.join("gregg.toml");
    write_config(
        &config_path,
        &[FixtureSystem {
            id: MODERN_SYSTEM_ID,
            port: 1,
            name: "modern",
        }],
        None,
    );
    let identity = ClientDaemonIdentity::for_path(&config_path);
    let listener = bind_stand_in_daemon(&config_path);
    let connections = Arc::new(AtomicUsize::new(0));

    let stand_in = tokio::spawn({
        let connections = Arc::clone(&connections);
        async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                connections.fetch_add(1, Ordering::SeqCst);
                if read_frame(&mut stream).await.is_none() {
                    continue;
                }
                let hello =
                    gregg::clientd::FrontendFrame::Hello(Box::new(gregg::clientd::HelloPayload {
                        protocol_version: gregg::clientd::PROTOCOL_VERSION,
                        version: "9.9.9".to_owned(),
                        daemon_id: identity.id().to_owned(),
                        current_generation: 0,
                    }));
                let mismatch = gregg::clientd::FrontendFrame::VersionMismatch(
                    gregg::clientd::VersionMismatchPayload {
                        frontend: gregg::clientd::PROTOCOL_VERSION,
                        daemon: gregg::clientd::PROTOCOL_VERSION + 1,
                        daemon_id: identity.id().to_owned(),
                    },
                );
                let hello = gregg::clientd::encode_frame(&hello).expect("the hello frame encodes");
                let mismatch =
                    gregg::clientd::encode_frame(&mismatch).expect("the mismatch frame encodes");
                let _ = stream.write_all(&hello).await;
                let _ = stream.write_all(&mismatch).await;
                let _ = stream.flush().await;
                // Hold the connection open: a stop request would arrive here.
                let mut rest = Vec::new();
                let _ = stream.read_to_end(&mut rest).await;
            }
        }
    });

    let gui = Bridge::spawn(
        config_path.clone(),
        DaemonExecutable::Path(harmless_executable()),
        Arc::new(|| {}),
    );

    let reported = wait_for_gui_message(&gui, |message| {
        matches!(message, BridgeMessage::Incompatible { .. })
    })
    .await;
    assert!(
        reported.is_some(),
        "the GUI must report an incompatible daemon instead of rendering it"
    );
    // A stop or a replacement would arrive asynchronously relative to the
    // incompatibility report, so the count is read after a short settle.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        1,
        "the GUI must not have stopped or replaced the peer it collided with"
    );

    stand_in.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Wait for the GUI to publish a message the predicate accepts.
async fn wait_for_gui_message(
    gui: &Bridge,
    predicate: impl Fn(&BridgeMessage) -> bool,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;
    loop {
        for message in gui.drain() {
            if predicate(&message) {
                return Some(format!("{message:?}"));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Bind the stand-in daemon on the endpoint the client daemon would use.
#[cfg(unix)]
fn bind_stand_in_daemon(config_path: &Path) -> tokio::net::UnixListener {
    for candidate in gregg::clientd::socket_candidates(config_path) {
        let _ = std::fs::remove_file(&candidate);
        if let Ok(listener) = tokio::net::UnixListener::bind(&candidate) {
            return listener;
        }
    }
    panic!("no client-daemon endpoint could be bound for the stand-in daemon");
}

/// Read one length-prefixed frame.
#[cfg(unix)]
async fn read_frame(stream: &mut tokio::net::UnixStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        if buffer.len() >= gregg::clientd::LEN_PREFIX_BYTES {
            let length =
                gregg::clientd::parse_length(&buffer[..gregg::clientd::LEN_PREFIX_BYTES]).ok()?;
            let end = gregg::clientd::LEN_PREFIX_BYTES + length;
            if buffer.len() >= end {
                return Some(buffer[gregg::clientd::LEN_PREFIX_BYTES..end].to_vec());
            }
        }
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..count]);
    }
}
