//! Bounded, local-only maintenance scheduler.

pub(crate) mod observation;
pub(crate) mod schedule;

use std::process::{ExitStatus, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use gregg_protocol::{
    LoadAverage, ReadinessState, SchedulerJobV2, SchedulerLoadGateV2, SchedulerOutcomeV2,
};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;

use crate::config::{ScheduledJobConfig, MAX_JOBS};
use observation::{
    drain_into, drain_step, job_state, OutputTail, SchedulerObserver, SchedulerPublisher,
    TerminalRecord,
};
use schedule::LocalSchedule;

const CHILD_SHUTDOWN_BOUND: Duration = Duration::from_secs(2);

/// Fixed, non-configurable grace period for draining output after the direct
/// child has exited.
///
/// The direct child's wait result is the terminal execution event. A descendant
/// that merely inherited stdout or stderr can hold a pipe write end open long
/// after the scheduled child is gone, and greggd has exactly one global child
/// slot: waiting for pipe EOF would let a process greggd never scheduled block
/// every later maintenance job. Bytes already in flight get this long to land
/// in the fixed-capacity tails, and then the read handles close, the record is
/// finalized, and the slot is freed.
///
/// Fixed by design — not a configuration field — and it never expands into
/// process-group ownership or descendant termination: greggd does not own
/// anything it did not spawn, so a descendant that writes after this boundary is
/// outside the direct-child execution/history contract.
const POST_EXIT_OUTPUT_SETTLE: Duration = Duration::from_millis(250);

/// Scheduler-internal civil-clock reconciliation bound.
///
/// Long civil-time sleeps are never trusted for longer than one
/// cron-resolution minute without re-reading wall time. The semantic
/// cron/retry/max-wait deadline stays truthful inside [`Engine::next_deadline`];
/// this cap applies only to the actual `sleep_until` wake in [`run`], so a
/// large forward wall-clock jump is observed promptly (and coalesced) while
/// monotonic retry/max-wait/child semantics are unchanged.
const MAX_CIVIL_RECHECK: Duration = Duration::from_secs(60);

/// Cap a semantic monotonic deadline at one reconciliation interval.
///
/// Pure monotonic calculation: no allocation, no async work, no telemetry,
/// filesystem, or process effects. Unit-testable independently of the event
/// loop.
fn bounded_wake_deadline(semantic_deadline: Instant, now: Instant) -> Instant {
    semantic_deadline.min(now + MAX_CIVIL_RECHECK)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Current wall clock in Unix milliseconds, saturating a pre-epoch clock at 0.
///
/// Only ever used for the read-only observation model, so a backwards clock
/// degrades the published timestamp rather than affecting scheduling, which
/// stays on its own civil-time and monotonic clocks.
fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

/// Latest cached sampler load state. It is borrowed only when a job is due
/// or a deferred job becomes eligible for retry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoadGateState {
    pub(crate) readiness: ReadinessState,
    pub(crate) load: Option<LoadAverage>,
}

impl LoadGateState {
    pub(crate) const UNAVAILABLE: Self = Self {
        readiness: ReadinessState::Warming,
        load: None,
    };
}

#[derive(Debug)]
struct PendingOccurrence {
    since: Instant,
    /// Wall time the occurrence first became pending, for the wire model.
    since_unix_ms: u64,
    /// Civil occurrence this pending run was scheduled for.
    scheduled: DateTime<Local>,
    retry_at: Instant,
    /// Wall time of the current load-gate retry, for the wire model.
    retry_at_unix_ms: u64,
    coalesced: bool,
    waiting_logged: bool,
}

/// Per-job runtime state. Operator configuration stays borrowed from the
/// validated configuration so a launch decision never copies it.
#[derive(Debug)]
struct JobState {
    schedule: LocalSchedule,
    next_due: DateTime<Local>,
    pending: Option<PendingOccurrence>,
    /// The most recent load-gate decision for the current occurrence.
    ///
    /// Stored rather than recomputed at publication time so the published
    /// `observed` value is the reading behind the actual decision, and so an
    /// idle job never appears to carry a live load reading.
    last_gate: Option<SchedulerLoadGateV2>,
}

#[derive(Debug)]
struct Engine<'a> {
    configs: &'a [ScheduledJobConfig],
    states: Vec<JobState>,
    /// Plan 163: bounded history plus the published read-only snapshot. The
    /// engine owns it because it is the only thing that knows when an
    /// externally visible transition happened.
    observer: SchedulerObserver,
}

/// A selected job, identified by its configuration index so the direct child
/// can be spawned from the borrowed configuration before the next await.
#[derive(Debug)]
struct Launch {
    index: usize,
    pending_age: Duration,
    observed_load: Option<f32>,
    load_window: Option<&'static str>,
    /// Civil occurrence this run was scheduled for.
    scheduled: DateTime<Local>,
    /// Whether later civil occurrences were folded into this run.
    coalesced: bool,
}

/// Cached-load gate evaluation for a load-gated job: the inclusive verdict,
/// the selected window, the observed value, and the configured threshold. A
/// time-only job has no gate. Warming, failed, or missing load fails closed.
fn load_gate(
    config: &ScheduledJobConfig,
    load: LoadGateState,
) -> Option<(bool, &'static str, Option<f32>, f32)> {
    let threshold = config.max_load?;
    let (window, observed) = config.max_load.map(|_| {
        let window = config.effective_load_window();
        let value = match (load.readiness, load.load) {
            (ReadinessState::Ready, Some(values)) => Some(match window {
                "1m" => values.one,
                "5m" => values.five,
                _ => values.fifteen,
            }),
            _ => None,
        };
        (window, value)
    })?;
    Some((
        observed.is_some_and(|value| value.is_finite() && value <= threshold),
        window,
        observed,
        threshold,
    ))
}

impl<'a> Engine<'a> {
    fn new(
        configs: &'a [ScheduledJobConfig],
        wall_now: DateTime<Local>,
        history_limit: usize,
        publisher: SchedulerPublisher,
    ) -> Result<Self, String> {
        if configs.len() > MAX_JOBS {
            return Err(format!("scheduler has more than {MAX_JOBS} jobs"));
        }
        let mut states = Vec::with_capacity(configs.len());
        for config in configs {
            let schedule = LocalSchedule::parse(&config.schedule)?;
            let next_due = schedule
                .next_after(&wall_now)
                .map_err(|error| format!("job {:?}: {error}", config.name))?;
            states.push(JobState {
                schedule,
                next_due,
                pending: None,
                last_gate: None,
            });
        }
        let names: Vec<String> = configs.iter().map(|config| config.name.clone()).collect();
        let observer = SchedulerObserver::new(&names, history_limit, now_unix_ms(), publisher);
        Ok(Self {
            configs,
            states,
            observer,
        })
    }

    /// Publish current live state, skipping an unchanged state.
    ///
    /// Called after every externally visible transition and after each
    /// scheduler wake. Because it compares against the last publication, the
    /// Plan-160 one-minute reconciliation wake publishes nothing when no
    /// externally visible state changed.
    async fn publish(&mut self, wall_now: DateTime<Local>, running: Option<(usize, u64)>) {
        let jobs: Vec<SchedulerJobV2> = self
            .states
            .iter()
            .enumerate()
            .map(|(index, state)| {
                let config = &self.configs[index];
                let running_here = running.filter(|(active, _)| *active == index);
                // A load gate is holding the occurrence back exactly when a
                // gate decision exists, nothing is running for this job, and
                // the occurrence is still waiting. The retry instant is then
                // always in the future, so the published civil time is a real
                // countdown rather than the second the tick started in.
                let deferred =
                    state.last_gate.is_some() && running_here.is_none() && state.pending.is_some();
                SchedulerJobV2 {
                    name: config.name.clone(),
                    schedule: config.schedule.clone(),
                    next_due_unix_ms: state.next_due.timestamp_millis().max(0).unsigned_abs(),
                    state: job_state(
                        running_here.is_some(),
                        state.pending.is_some(),
                        if deferred {
                            state.last_gate.as_ref()
                        } else {
                            None
                        },
                    ),
                    load: state.last_gate.clone(),
                    pending_since_unix_ms: state.pending.as_ref().map(|p| p.since_unix_ms),
                    // A retry time is only meaningful while a load gate is
                    // actually holding the occurrence back.
                    next_retry_unix_ms: deferred
                        .then(|| {
                            state
                                .pending
                                .as_ref()
                                .map(|pending| pending.retry_at_unix_ms)
                        })
                        .flatten(),
                    running_since_unix_ms: running_here.map(|(_, started)| started),
                    last: self.observer.last_summary(index),
                }
            })
            .collect();
        self.observer
            .publish(jobs, wall_now.timestamp_millis().max(0).unsigned_abs())
            .await;
    }

    /// Push the retry deadline out for every load-blocked candidate that lost
    /// the free global slot to `winner`, recording the reading behind each
    /// decision so the published load context is the one that actually
    /// delayed the job.
    ///
    /// `winner` is `None` when no candidate was eligible at all. That case is
    /// not an early return: a load-gated occurrence created in the same tick
    /// keeps its already-elapsed `retry_at`, which would otherwise become the
    /// `sleep_until` deadline in [`Engine::next_deadline`] and spin the loop
    /// until the gate opens. With `None` every load-blocked candidate is
    /// deferred, and with `Some(winner)` the ordering filter keeps the
    /// documented "visited before the winner" boundary.
    fn defer_blocked_before(
        &mut self,
        winner: Option<(Instant, usize)>,
        now: Instant,
        unix_now: u64,
        load: LoadGateState,
    ) {
        let configs = self.configs;
        for (blocked, state) in self.states.iter_mut().enumerate() {
            let Some(pending) = state.pending.as_ref() else {
                continue;
            };
            if pending.retry_at > now
                || winner.is_some_and(|winner| (pending.since, blocked) >= winner)
            {
                continue;
            }
            let config = &configs[blocked];
            let Some((allowed, window, observed, threshold)) = load_gate(config, load) else {
                continue;
            };
            if allowed {
                continue;
            }
            let pending = state.pending.as_mut().expect("candidate is pending");
            if !pending.waiting_logged {
                tracing::info!(
                    job = %config.name,
                    load_window = window,
                    observed_load = observed,
                    max_load = threshold,
                    pending_age_ms = duration_millis(now.saturating_duration_since(pending.since)),
                    "scheduled job pending: load gate"
                );
                pending.waiting_logged = true;
            }
            pending.retry_at = now + Duration::from_millis(config.effective_retry_interval_ms());
            // The retry deadline is monotonic; the published copy is civil
            // time so a client can render "next retry in ..." truthfully.
            pending.retry_at_unix_ms =
                unix_now.saturating_add(config.effective_retry_interval_ms());
            state.last_gate = Some(SchedulerLoadGateV2 {
                window: window.to_owned(),
                threshold,
                observed,
            });
        }
    }

    /// Expire a load-deferred occurrence once it has waited its full
    /// `max_wait`, recording the terminal outcome.
    ///
    /// Only an occurrence the load gate is refusing *right now* can expire:
    /// `tick` expires before the global child-slot check, so a `max_load` job
    /// the host is comfortably under would otherwise be dropped with a
    /// `load_expired` record blaming load for a slot the gate never saw. A
    /// time-only job has no gate and can never expire this way.
    ///
    /// A load-expired occurrence is a terminal record even though no child ever
    /// ran; omitting it would make the recent-runs display quietly dishonest.
    /// Returns the pending age for the existing log line.
    fn take_expired(
        &mut self,
        index: usize,
        now: Instant,
        config: &ScheduledJobConfig,
        load: LoadGateState,
    ) -> Option<u64> {
        let state = &mut self.states[index];
        let load_blocked = load_gate(config, load).is_some_and(|(allowed, ..)| !allowed);
        let expired = load_blocked
            && state.pending.as_ref().is_some_and(|pending| {
                now.saturating_duration_since(pending.since)
                    >= Duration::from_millis(config.effective_max_wait_ms())
            });
        if !expired {
            return None;
        }
        let pending = state.pending.take().expect("pending checked above");
        let pending_age_ms = duration_millis(now.saturating_duration_since(pending.since));
        self.observer.record_terminal(
            index,
            TerminalRecord::without_child(
                pending.scheduled.timestamp_millis().max(0).unsigned_abs(),
                now_unix_ms(),
                pending_age_ms,
                pending.coalesced,
                SchedulerOutcomeV2::LoadExpired,
            ),
            SchedulerOutcomeV2::LoadExpired,
        );
        Some(pending_age_ms)
    }

    fn tick(
        &mut self,
        wall_now: DateTime<Local>,
        now: Instant,
        load: LoadGateState,
        slot_available: bool,
    ) -> Result<Option<Launch>, String> {
        let configs = self.configs;
        let unix_now = wall_now.timestamp_millis().max(0).unsigned_abs();
        for (index, state) in self.states.iter_mut().enumerate() {
            let config = &configs[index];
            if state.next_due <= wall_now {
                if let Some(pending) = &mut state.pending {
                    pending.coalesced = true;
                } else {
                    state.pending = Some(PendingOccurrence {
                        since: now,
                        since_unix_ms: unix_now,
                        scheduled: state.next_due,
                        retry_at: now,
                        retry_at_unix_ms: unix_now,
                        coalesced: false,
                        waiting_logged: false,
                    });
                }
                // A fresh occurrence has no load decision yet.
                state.last_gate = None;
                // Configuration validation already rejects calendar-impossible
                // expressions, so a failure here is an internal time-domain
                // error. It reaches the existing scheduler fatal boundary
                // instead of fabricating a replacement schedule.
                state.next_due = state
                    .schedule
                    .next_after(&wall_now)
                    .map_err(|error| format!("job {:?}: {error}", config.name))?;
            }
        }
        // Expiry needs the observer, which is a separate field from `states`.
        for (index, config) in configs.iter().enumerate() {
            if let Some(pending_age_ms) = self.take_expired(index, now, config, load) {
                tracing::info!(
                    job = %config.name,
                    pending_age_ms,
                    "scheduled job expired waiting for load"
                );
            }
        }

        if !slot_available {
            return Ok(None);
        }

        // Bounded selection over at most MAX_JOBS jobs: the oldest eligible
        // candidate wins with config order as the stable tie breaker. No
        // candidate vector is built merely to choose the next job.
        let mut selected: Option<(Instant, usize)> = None;
        for (index, state) in self.states.iter().enumerate() {
            let Some(pending) = state.pending.as_ref() else {
                continue;
            };
            if pending.retry_at > now
                || load_gate(&configs[index], load).is_some_and(|(allowed, ..)| !allowed)
            {
                continue;
            }
            let key = (pending.since, index);
            if selected.is_none_or(|current| key < current) {
                selected = Some(key);
            }
        }
        let Some((since, index)) = selected else {
            // Nothing was eligible. Every load-blocked candidate is still
            // rescheduled here so no pending keeps an elapsed `retry_at`.
            self.defer_blocked_before(None, now, unix_now, load);
            return Ok(None);
        };

        // Defer every load-blocked candidate that would have been visited
        // before the winner, so an older blocked job neither hides nor is
        // hidden by the job that takes the free global slot.
        self.defer_blocked_before(Some((since, index)), now, unix_now, load);

        let pending = self.states[index]
            .pending
            .take()
            .expect("selected candidate is pending");
        let gate = load_gate(&configs[index], load);
        // Remember the decision that let this job through, so an operator can
        // see the load it ran under.
        self.states[index].last_gate = gate.map(|(allowed, window, observed, threshold)| {
            let _ = allowed;
            SchedulerLoadGateV2 {
                window: window.to_owned(),
                threshold,
                observed,
            }
        });
        Ok(Some(Launch {
            index,
            pending_age: now.saturating_duration_since(pending.since),
            observed_load: gate.and_then(|(_, _, observed, _)| observed),
            load_window: gate.map(|(_, window, _, _)| window),
            scheduled: pending.scheduled,
            coalesced: pending.coalesced,
        }))
    }

    fn next_deadline(
        &self,
        wall_now: DateTime<Local>,
        now: Instant,
        slot_available: bool,
    ) -> Instant {
        let mut deadline = None;
        for (index, state) in self.states.iter().enumerate() {
            let config = &self.configs[index];
            let until_due = (state.next_due - wall_now)
                .to_std()
                .unwrap_or(Duration::ZERO);
            let candidate = now + until_due;
            deadline = Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
            if let Some(pending) = &state.pending {
                // An elapsed `retry_at` describes work the tick in front of
                // this call already decided against, so folding it in would
                // park `sleep_until` on the past and spin the loop. Only a
                // retry that is genuinely still in the future is a deadline.
                if slot_available && pending.retry_at > now {
                    deadline = Some(deadline.map_or(pending.retry_at, |current: Instant| {
                        current.min(pending.retry_at)
                    }));
                }
                if config.max_load.is_some() {
                    let expiry =
                        pending.since + Duration::from_millis(config.effective_max_wait_ms());
                    deadline = Some(deadline.map_or(expiry, |current| current.min(expiry)));
                }
            }
        }
        deadline.expect("a scheduler is started only with at least one job")
    }

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.states
            .iter()
            .filter(|job| job.pending.is_some())
            .count()
    }
}

/// A running child plus everything needed to finalize its terminal record.
struct RunningChild<'a> {
    child: tokio::process::Child,
    /// Piped only when history capture is enabled; otherwise both are `None`
    /// and the child keeps the original null streams.
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    /// Accumulated bounded tail per piped stream, owned here rather than by the
    /// drain future.
    ///
    /// The drain future is rebuilt on every scheduler wake, and a wake is
    /// capped at [`MAX_CIVIL_RECHECK`] even while a child keeps running. A
    /// per-wake accumulator would therefore be dropped — together with every
    /// byte already read — on the first deadline wake, publishing an empty (or
    /// post-wake-only) tail that still claimed `truncated: false`. Owning the
    /// tail here makes a cancelled drain lose nothing it had folded in.
    stdout_tail: OutputTail,
    stderr_tail: OutputTail,
    /// Index into the configured job set, so every publication can name the
    /// job that actually holds the global child slot — and every terminal
    /// record is attributed to the job that was actually launched.
    index: usize,
    job_name: &'a str,
    started: Instant,
    started_unix_ms: u64,
    scheduled_unix_ms: u64,
    delay_ms: u64,
    coalesced: bool,
    /// The direct child's terminal execution event, frozen when its wait
    /// result resolves.
    ///
    /// It lives here, not in the completion future, because that future is
    /// rebuilt on every scheduler wake: an exit observed during one wake has to
    /// keep its own instant, exit status, and settle budget when the next wake
    /// rebuilds the future. Recording `finished_unix_ms` from the moment the
    /// completion is *assembled* would describe the settle that followed the
    /// exit instead of the exit itself.
    exit: Option<ChildExit>,
    /// When the post-exit settle bound expires, fixed at the exit instant so a
    /// wake inside the settle window cannot restart or extend it.
    settle_deadline: Option<Instant>,
    /// Whether each piped stream can produce no further bytes (EOF, read error,
    /// or never piped). Retained across wakes for the same reason as the tails.
    stdout_done: bool,
    stderr_done: bool,
}

impl RunningChild<'_> {
    /// Freeze the direct child's terminal execution event and open its
    /// bounded post-exit output settle window.
    ///
    /// Idempotent: a rebuild after a cancelled wake keeps the first instant.
    fn freeze_exit(&mut self, status: std::io::Result<ExitStatus>) {
        if self.exit.is_some() {
            return;
        }
        let finished = Instant::now();
        self.exit = Some(ChildExit {
            status,
            finished,
            finished_unix_ms: now_unix_ms(),
        });
        self.settle_deadline = Some(finished + POST_EXIT_OUTPUT_SETTLE);
    }

    /// Whether both piped streams are finished, so the post-exit settle has
    /// nothing left to wait for.
    ///
    /// Only the Unix inherited-descriptor regression asserts this directly; the
    /// settle itself no longer needs the predicate because it owns the loop that
    /// would have called it.
    #[cfg(all(test, unix))]
    fn output_drained(&self) -> bool {
        self.stdout_done && self.stderr_done
    }
}

/// The direct child's terminal execution event.
struct ChildExit {
    status: std::io::Result<ExitStatus>,
    /// Monotonic instant the wait result resolved.
    finished: Instant,
    /// Wall clock captured at that same instant.
    finished_unix_ms: u64,
}

/// A child that has exited, with its own terminal instant and both bounded
/// tails.
#[derive(Debug)]
struct ChildCompletion {
    status: std::io::Result<ExitStatus>,
    /// When the direct child exited — not when the post-exit drain settled.
    finished_unix_ms: u64,
    /// Direct-child wall duration, measured to the wait result.
    duration_ms: u64,
    stdout: OutputTail,
    stderr: OutputTail,
}

/// Wait for the direct child to exit, then settle its output within a fixed
/// bound.
///
/// Two phases, both cancellation-safe across a rebuilt future:
///
/// 1. **While the child runs**, its wait and both bounded drains are polled on
///    the same task, so a child that writes more than one pipe buffer makes
///    progress on its writes as the drains make progress. Awaiting the child
///    *before* the drains would let such a child block in `write(2)` forever.
///    The streams are borrowed rather than handed to drain tasks, so a
///    cancelled select simply stops draining and leaves the handles in place
///    for the next wake — no drain task can outlive the scheduler or delay
///    shutdown — and the already-folded tails survive on the `RunningChild`.
/// 2. **Once the direct child has exited**, its finish instant and exit status
///    are frozen first, then both streams keep making bounded progress for at
///    most [`POST_EXIT_OUTPUT_SETTLE`] — ending early the moment both reach
///    EOF. This is what keeps an inherited writer from retaining the one global
///    child slot after the scheduled child is gone, while never blocking the
///    scheduler to capture a descendant's later output. Within that budget the
///    two streams progress independently: see [`settle_output`].
async fn await_child_completion(child: &mut RunningChild<'_>) -> ChildCompletion {
    if child.exit.is_none() {
        // The inner block scopes the drain borrows so the frozen exit can be
        // installed as soon as the select resolves.
        let status = {
            let wait = child.child.wait();
            tokio::pin!(wait);
            let stdout = drain_into(
                child.stdout.as_mut(),
                &mut child.stdout_tail,
                &mut child.stdout_done,
            );
            tokio::pin!(stdout);
            let stderr = drain_into(
                child.stderr.as_mut(),
                &mut child.stderr_tail,
                &mut child.stderr_done,
            );
            tokio::pin!(stderr);
            tokio::select! {
                status = &mut wait => Some(status),
                // Both pipes finished first (capture disabled, or the child
                // closed its own descriptors). The direct child's wait is still
                // the terminal event, so keep waiting for it with no drain in
                // flight. The two drains are joined *here* rather than run in
                // sequence: awaiting stdout to EOF before touching stderr would
                // let a child blocked writing to a full stderr pipe never
                // reach the EOF that releases the first one.
                () = async {
                    let _ = tokio::join!(&mut stdout, &mut stderr);
                } => None,
            }
        };
        if let Some(status) = status {
            child.freeze_exit(status);
        } else {
            let status = child.child.wait().await;
            child.freeze_exit(status);
        }
    }
    let settle_deadline = child
        .settle_deadline
        .expect("a frozen exit carries its settle bound");
    settle_output(
        child.stdout.as_mut(),
        &mut child.stdout_tail,
        &mut child.stdout_done,
        child.stderr.as_mut(),
        &mut child.stderr_tail,
        &mut child.stderr_done,
        settle_deadline,
    )
    .await;
    let exit = child
        .exit
        .take()
        .expect("the direct child reached a terminal wait result");
    // Dropping the read ends here is the boundary: a writer that merely
    // inherited the descriptor now gets `EPIPE` instead of holding greggd's one
    // global child slot. greggd does not kill it.
    child.stdout = None;
    child.stderr = None;
    ChildCompletion {
        status: exit.status,
        finished_unix_ms: exit.finished_unix_ms,
        duration_ms: duration_millis(exit.finished.saturating_duration_since(child.started)),
        stdout: std::mem::take(&mut child.stdout_tail),
        stderr: std::mem::take(&mut child.stderr_tail),
    }
}

/// Spend the post-exit settle on both streams **independently**.
///
/// After the direct child has exited, one stream can stay open with nothing to
/// read at all: a descendant inherited that descriptor and is still alive.
/// Awaiting that read inside a `join!` with the other stream's read means the
/// busy stream performs exactly one chunk and then stalls until the idle one
/// produces bytes or the deadline wins — so output that was already available is
/// silently dropped from the retained tail.
///
/// So each iteration selects among three independent futures: one `drain_step`
/// per stream that is not finished, and the frozen deadline. Whichever stream is
/// ready first wins that iteration, so a pending read on one never withholds
/// progress from the other. The preferred branch alternates, so when both streams
/// are continuously ready neither can starve the other for the whole settle.
///
/// A read that loses the select is cancelled having consumed nothing, and the
/// tails are borrowed from the `RunningChild`, so losing a cancelled read — or
/// losing this whole future to a scheduler wake — loses nothing already folded
/// in. Each iteration strictly reduces what remains: a `drain_step` either folds
/// bytes or marks its own stream finished.
///
/// `settle_deadline` is passed in rather than recomputed, so neither a wake nor
/// a per-stream read can extend the bound frozen at the child's exit. Nothing is
/// spawned: the deadline returns exactly when the idle inherited writer has had
/// its budget, and greggd never waits for descendant EOF.
async fn settle_output<O, E>(
    mut stdout: Option<&mut O>,
    stdout_tail: &mut OutputTail,
    stdout_done: &mut bool,
    mut stderr: Option<&mut E>,
    stderr_tail: &mut OutputTail,
    stderr_done: &mut bool,
    settle_deadline: Instant,
) where
    O: AsyncReadExt + Unpin,
    E: AsyncReadExt + Unpin,
{
    let mut prefer_stdout = true;
    loop {
        if *stdout_done && *stderr_done {
            return;
        }
        if prefer_stdout {
            tokio::select! {
                biased;
                _ = drain_step(stdout.as_deref_mut(), stdout_tail, stdout_done), if !*stdout_done => {}
                _ = drain_step(stderr.as_deref_mut(), stderr_tail, stderr_done), if !*stderr_done => {}
                () = tokio::time::sleep_until(settle_deadline) => return,
            }
        } else {
            tokio::select! {
                biased;
                _ = drain_step(stderr.as_deref_mut(), stderr_tail, stderr_done), if !*stderr_done => {}
                _ = drain_step(stdout.as_deref_mut(), stdout_tail, stdout_done), if !*stdout_done => {}
                () = tokio::time::sleep_until(settle_deadline) => return,
            }
        }
        prefer_stdout = !prefer_stdout;
    }
}

enum SchedulerWake {
    Child(Option<ChildCompletion>),
    Deadline,
    Shutdown,
}

#[cfg(unix)]
fn exit_signal(status: ExitStatus) -> Option<u32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(i32::unsigned_abs)
}

#[cfg(not(unix))]
fn exit_signal(_status: ExitStatus) -> Option<u32> {
    None
}

/// What one completed direct child contributed to its terminal record.
#[derive(Debug)]
struct FinishedChild {
    sequence: u64,
    outcome: SchedulerOutcomeV2,
    exit_code: Option<i32>,
    signal: Option<u32>,
    /// Direct-child wall duration, frozen at the wait result.
    duration_ms: u64,
}

/// Record one completed direct child under the configuration index it carries.
///
/// The running child already owns the authoritative index it was launched with,
/// so attribution never searches the configured jobs by name: a duplicate or
/// reordered name can never move a terminal record onto a sibling, and there is
/// no index-zero fallback for a failed search. The published finish instant and
/// duration come from the completion's frozen exit event, never from the moment
/// the record is assembled.
fn finish_child(
    engine: &mut Engine<'_>,
    child: &RunningChild<'_>,
    completion: ChildCompletion,
) -> FinishedChild {
    let (outcome, exit_code, signal) = match completion.status {
        Ok(status) => {
            let code = status.code();
            let signal = exit_signal(status);
            let outcome = if code == Some(0) {
                SchedulerOutcomeV2::Success
            } else {
                SchedulerOutcomeV2::Failed
            };
            (outcome, code, signal)
        }
        // The child was created but its status could not be observed.
        // Publishing it as a failure would be a guess.
        Err(error) => {
            tracing::info!(job = %child.job_name, %error, "scheduled job wait failed");
            (SchedulerOutcomeV2::WaitFailed, None, None)
        }
    };
    let sequence = engine.observer.record_terminal(
        child.index,
        TerminalRecord {
            scheduled_unix_ms: child.scheduled_unix_ms,
            started_unix_ms: Some(child.started_unix_ms),
            finished_unix_ms: completion.finished_unix_ms,
            delay_ms: child.delay_ms,
            coalesced: child.coalesced,
            exit_code,
            signal,
            duration_ms: Some(completion.duration_ms),
            stdout: completion.stdout,
            stderr: completion.stderr,
        },
        outcome,
    );
    FinishedChild {
        sequence,
        outcome,
        exit_code,
        signal,
        duration_ms: completion.duration_ms,
    }
}

fn start_child(
    job: &ScheduledJobConfig,
    index: usize,
    capture_output: bool,
) -> Result<RunningChild<'_>, String> {
    let Some(executable) = job.command.first() else {
        return Err("validated command has no executable".to_owned());
    };
    let mut command = Command::new(executable);
    command
        .args(&job.command[1..])
        .stdin(Stdio::null())
        .stdout(if capture_output {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(if capture_output {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .kill_on_drop(true);
    if let Some(working_dir) = &job.working_dir {
        command.current_dir(working_dir);
    }
    // Take both pipe handles immediately after the spawn. Leaving them in the
    // child would risk the classic deadlock where a child blocks writing to a
    // full pipe nobody drains.
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // A stream that was never piped has nothing to drain and nothing to wait
    // for, so its post-exit settle is already satisfied.
    let stdout_done = stdout.is_none();
    let stderr_done = stderr.is_none();
    Ok(RunningChild {
        child,
        stdout,
        stderr,
        stdout_tail: OutputTail::new(),
        stderr_tail: OutputTail::new(),
        index,
        job_name: &job.name,
        started: Instant::now(),
        started_unix_ms: now_unix_ms(),
        scheduled_unix_ms: 0,
        delay_ms: 0,
        coalesced: false,
        exit: None,
        settle_deadline: None,
        stdout_done,
        stderr_done,
    })
}

/// Run the daemon's configured job set until its shutdown signal arrives.
///
/// `publisher` is the same shared cell the HTTP server already serves, so the
/// live job set replaces the initial empty document without re-wiring the
/// server state.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(
    jobs: Vec<ScheduledJobConfig>,
    history_limit: usize,
    publisher: SchedulerPublisher,
    mut load_rx: watch::Receiver<LoadGateState>,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), String> {
    let mut engine = Engine::new(&jobs, Local::now(), history_limit, publisher)?;
    let capture_output = engine.observer.capture_enabled();
    let mut active: Option<RunningChild<'_>> = None;
    // Publish the initial idle state so a configured job is visible before its
    // first run, and a daemon with jobs is never observably empty.
    engine.publish(Local::now(), None).await;
    loop {
        let now = Instant::now();
        let wall_now = Local::now();
        // Copy the cached load state and drop the watch guard before any
        // await, so the borrow never spans a suspension point.
        let load_state = *load_rx.borrow_and_update();
        if let Some(launch) = engine.tick(wall_now, now, load_state, active.is_none())? {
            let job = &jobs[launch.index];
            match start_child(job, launch.index, capture_output) {
                Ok(mut child) => {
                    child.scheduled_unix_ms =
                        launch.scheduled.timestamp_millis().max(0).unsigned_abs();
                    child.delay_ms = duration_millis(launch.pending_age);
                    child.coalesced = launch.coalesced;
                    tracing::info!(
                        job = %job.name,
                        pending_age_ms = duration_millis(launch.pending_age),
                        load_window = launch.load_window,
                        observed_load = launch.observed_load,
                        "scheduled job started"
                    );
                    active = Some(child);
                    engine
                        .publish(
                            Local::now(),
                            active.as_ref().map(|c| (c.index, c.started_unix_ms)),
                        )
                        .await;
                }
                Err(error) => {
                    tracing::info!(job = %job.name, %error, "scheduled job completed");
                    // A job that could not even be created is a terminal
                    // occurrence. Silently dropping it would hide a broken
                    // command behind an empty recent-runs list.
                    let finished = now_unix_ms();
                    let sequence = engine.observer.record_terminal(
                        launch.index,
                        TerminalRecord::without_child(
                            launch.scheduled.timestamp_millis().max(0).unsigned_abs(),
                            finished,
                            duration_millis(launch.pending_age),
                            launch.coalesced,
                            SchedulerOutcomeV2::SpawnFailed,
                        ),
                        SchedulerOutcomeV2::SpawnFailed,
                    );
                    tracing::info!(job = %job.name, sequence, "scheduled job spawn failed");
                    engine.publish(Local::now(), None).await;
                }
            }
            continue;
        }
        // The running child is named here too: a long run otherwise leaves the
        // job published as idle with no `running_since_unix_ms` for its whole
        // duration.
        engine
            .publish(
                wall_now,
                active.as_ref().map(|c| (c.index, c.started_unix_ms)),
            )
            .await;

        let semantic_deadline = engine.next_deadline(wall_now, Instant::now(), active.is_none());
        // Re-reading civil time at least once per minute bounds how long a
        // forward wall-clock jump can hide behind a stale monotonic sleep.
        // The wake itself performs the existing bounded scan only: no host
        // telemetry, HTTP request, filesystem scan, or child spawn, and it
        // stays silent (the `Deadline` branch below logs nothing).
        let wake_at = bounded_wake_deadline(semantic_deadline, Instant::now());
        let wake = {
            let child_wait = async {
                match active.as_mut() {
                    Some(child) => Some(await_child_completion(child).await),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                result = child_wait => SchedulerWake::Child(result),
                () = tokio::time::sleep_until(wake_at) => SchedulerWake::Deadline,
                _ = shutdown.recv() => SchedulerWake::Shutdown,
            }
        };
        match wake {
            SchedulerWake::Child(Some(completion)) => {
                let child = active.take().expect("completed child remains active");
                let finished = finish_child(&mut engine, &child, completion);
                tracing::info!(
                    job = %child.job_name,
                    sequence = finished.sequence,
                    outcome = ?finished.outcome,
                    exit_code = finished.exit_code,
                    signal = finished.signal,
                    elapsed_ms = finished.duration_ms,
                    "scheduled job completed"
                );
                engine.publish(Local::now(), None).await;
            }
            SchedulerWake::Child(None) | SchedulerWake::Deadline => {}
            SchedulerWake::Shutdown => {
                if let Some(active_child) = active.as_mut() {
                    tracing::info!(job = %active_child.job_name, "scheduler shutting down with active command");
                    if let Err(error) = active_child.child.start_kill() {
                        tracing::warn!(job = %active_child.job_name, %error, "scheduled child kill request failed");
                    }
                    match tokio::time::timeout(CHILD_SHUTDOWN_BOUND, active_child.child.wait())
                        .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => {
                            tracing::warn!(job = %active_child.job_name, %error, "scheduled child wait failed during shutdown");
                        }
                        Err(_) => {
                            tracing::warn!(job = %active_child.job_name, "scheduled child cleanup exceeded its bound");
                        }
                    }
                    // No `Cancelled` record is published: history is memory-only
                    // and the process is exiting, so the record could never be
                    // served. The vocabulary keeps the variant so an old
                    // client can render it if a future driver reports one.
                }
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
impl<'a> Engine<'a> {
    /// Build an engine whose jobs bypass configuration validation so a
    /// calendar-impossible schedule can reach the runtime failure path.
    fn new_unchecked(configs: &'a [ScheduledJobConfig], wall_now: DateTime<Local>) -> Self {
        let states = configs
            .iter()
            .map(|config| {
                let schedule =
                    LocalSchedule::parse(&config.schedule).expect("test schedule parses");
                let next_due = schedule.next_after(&wall_now).unwrap_or(wall_now);
                JobState {
                    schedule,
                    next_due,
                    pending: None,
                    last_gate: None,
                }
            })
            .collect();
        let names: Vec<String> = configs.iter().map(|config| config.name.clone()).collect();
        Self {
            configs,
            states,
            observer: SchedulerObserver::new(&names, 5, now_unix_ms(), SchedulerPublisher::empty()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;
    use chrono::{TimeZone, Utc};
    use gregg_protocol::{SchedulerJobStateV2, SchedulerSummaryV2};

    fn job(name: &str, max_load: Option<f32>) -> ScheduledJobConfig {
        ScheduledJobConfig {
            name: name.to_owned(),
            schedule: "* * * * *".to_owned(),
            command: vec!["/bin/true".to_owned(), "--flag".to_owned()],
            working_dir: None,
            max_load,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        }
    }

    fn wall_time() -> DateTime<Local> {
        Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0)
            .unwrap()
            .with_timezone(&Local)
    }

    const fn ready(one: f32, five: f32, fifteen: f32) -> LoadGateState {
        LoadGateState {
            readiness: ReadinessState::Ready,
            load: Some(LoadAverage { one, five, fifteen }),
        }
    }

    #[test]
    fn startup_is_strictly_after_reference_and_argv_is_preserved() {
        let wall = wall_time();
        let config = job("argv", None);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        assert!(engine.states[0].next_due > wall);
        let due = engine.states[0].next_due;
        let launch = engine
            .tick(
                due,
                Instant::now() + Duration::from_secs(60),
                LoadGateState::UNAVAILABLE,
                true,
            )
            .unwrap()
            .unwrap();
        assert_eq!(launch.index, 0);
        assert_eq!(config.command, ["/bin/true", "--flag"]);
    }

    #[test]
    fn deferral_is_bounded_coalesced_and_expires_once() {
        let wall = wall_time();
        let mut config = job("heavy", Some(1.0));
        config.retry_interval_ms = Some(10_000);
        config.max_wait_ms = Some(120_000);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let since = Instant::now();
        let due = engine.states[0].next_due;
        assert!(engine
            .tick(due, since, LoadGateState::UNAVAILABLE, false)
            .unwrap()
            .is_none());
        let retry_due = engine.states[0].next_due;
        let retry_wall = retry_due;
        assert!(engine
            .tick(
                retry_wall,
                since + Duration::from_secs(60),
                LoadGateState::UNAVAILABLE,
                false,
            )
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        assert_eq!(engine.states[0].pending.as_ref().unwrap().since, since);
        assert!(engine.states[0].pending.as_ref().unwrap().coalesced);
        assert!(engine
            .tick(
                retry_wall,
                since + Duration::from_secs(120),
                LoadGateState::UNAVAILABLE,
                false,
            )
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn a_slot_delayed_occurrence_is_not_blamed_on_the_load_gate() {
        // A threshold this host is comfortably under: the gate would have
        // allowed the job the moment the global child slot freed.
        let mut config = job("quick", Some(50.0));
        config.retry_interval_ms = Some(10_000);
        config.max_wait_ms = Some(1_000);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall_time(),
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let since = Instant::now();
        let due = engine.states[0].next_due;
        let idle = ready(0.5, 0.5, 0.5);
        // Due while another job holds the only child slot: the occurrence is
        // pending, and past its whole `max_wait` it must still be pending.
        assert!(engine.tick(due, since, idle, false).unwrap().is_none());
        assert!(engine
            .tick(due, since + Duration::from_secs(30), idle, false)
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        // Nothing ran, so nothing can have blamed load for it.
        assert!(engine.observer.last_summary(0).is_none());
        // The slot frees: the occurrence is still there and launches.
        let launch = engine
            .tick(due, since + Duration::from_secs(30), idle, true)
            .unwrap()
            .expect("a slot-delayed occurrence must not be dropped");
        assert_eq!(launch.index, 0);
    }

    #[test]
    fn load_threshold_is_inclusive_and_selected_window_is_exact() {
        let wall = wall_time();
        for (window, values, expected) in [
            ("1m", (7.9, 8.1, 8.2), 7.9),
            ("5m", (7.9, 8.0, 8.2), 8.0),
            ("15m", (7.9, 8.1, 8.0), 8.0),
        ] {
            let mut config = job(window, Some(8.0));
            config.load_window = Some(window.to_owned());
            let mut engine = Engine::new(
                std::slice::from_ref(&config),
                wall,
                5,
                SchedulerPublisher::empty(),
            )
            .unwrap();
            let due = engine.states[0].next_due;
            let launch = engine
                .tick(
                    due,
                    Instant::now(),
                    ready(values.0, values.1, values.2),
                    true,
                )
                .unwrap()
                .unwrap();
            assert_eq!(launch.observed_load, Some(expected));
            assert_eq!(launch.load_window, Some(window));
        }

        let mut config = job("high", Some(8.0));
        config.retry_interval_ms = Some(10_000);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let due = engine.states[0].next_due;
        let since = Instant::now();
        assert!(engine
            .tick(due, since, ready(8.01, 8.01, 8.01), true)
            .unwrap()
            .is_none());
        let retry_at = engine.states[0].pending.as_ref().unwrap().retry_at;
        let launch = engine
            .tick(due, retry_at, ready(8.0, 8.0, 8.0), true)
            .unwrap()
            .unwrap();
        assert_eq!(launch.observed_load, Some(8.0));
    }

    #[test]
    fn global_slot_and_oldest_stable_selection_prevent_herding() {
        let wall = wall_time();
        let configs = [job("first", None), job("second", None), job("third", None)];
        let mut engine = Engine::new(&configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        let first = engine
            .tick(due, mono, LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .unwrap();
        assert_eq!(configs[first.index].name, "first");
        assert!(engine
            .tick(due, mono, LoadGateState::UNAVAILABLE, false)
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 2);
        assert!(engine.next_deadline(due, mono, false) > mono);
        let second = engine
            .tick(due, mono, LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .unwrap();
        assert_eq!(configs[second.index].name, "second");
    }

    #[test]
    fn five_deferred_jobs_still_produce_one_global_launch_each() {
        let wall = wall_time();
        let configs: Vec<_> = (0..5)
            .map(|index| job(&format!("job-{index}"), Some(8.0)))
            .collect();
        let mut engine = Engine::new(&configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        let mut launched = Vec::new();
        for _ in 0..5 {
            let launch = engine
                .tick(due, mono, ready(1.0, 1.0, 1.0), true)
                .unwrap()
                .expect("one job per free slot");
            launched.push(configs[launch.index].name.clone());
        }
        assert_eq!(launched, ["job-0", "job-1", "job-2", "job-3", "job-4"]);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn high_load_candidate_does_not_block_time_only_job() {
        let wall = wall_time();
        let mut heavy = job("heavy", Some(0.0));
        heavy.retry_interval_ms = Some(10_000);
        let configs = [heavy, job("time-only", None)];
        let mut engine = Engine::new(&configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        let due = engine.states[0].next_due;
        let launch = engine
            .tick(due, Instant::now(), ready(1.0, 1.0, 1.0), true)
            .unwrap()
            .unwrap();
        assert_eq!(configs[launch.index].name, "time-only");
        // The older blocked job is still pending and was deferred, not dropped.
        let blocked = engine.states[0].pending.as_ref().unwrap();
        assert!(blocked.retry_at > Instant::now());
    }

    #[test]
    fn only_candidates_older_than_the_winner_are_deferred() {
        let wall = wall_time();
        let mut older = job("older-blocked", Some(8.0));
        older.retry_interval_ms = Some(10_000);
        let mut younger = job("younger-blocked", Some(8.0));
        younger.retry_interval_ms = Some(30_000);
        let configs = [older, job("time-only", None), younger];
        let mut engine = Engine::new(&configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        let launch = engine
            .tick(due, mono, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .unwrap();
        assert_eq!(configs[launch.index].name, "time-only");
        // The older blocked job is deferred and stays pending.
        let older = engine.states[0].pending.as_ref().unwrap();
        assert_eq!(older.retry_at, mono + Duration::from_millis(10_000));
        // The younger blocked job sorts after the winner and is left untouched.
        let younger = engine.states[2].pending.as_ref().unwrap();
        assert_eq!(younger.retry_at, mono);
    }

    /// A load-gated occurrence that never finds a free candidate must still be
    /// rescheduled, with the child slot idle the whole time.
    ///
    /// The wake deadline is only correct while `retry_at` is in the future: an
    /// elapsed one makes `sleep_until` return immediately and the loop spins
    /// on a core until the gate opens.
    #[test]
    fn a_gated_occurrence_with_a_free_slot_is_deferred_instead_of_spinning() {
        let wall = wall_time();
        let mut config = job("gated", Some(8.0));
        config.retry_interval_ms = Some(10_000);
        let publisher = SchedulerPublisher::empty();
        let mut engine =
            Engine::new(std::slice::from_ref(&config), wall, 5, publisher.clone()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        // The occurrence becomes due and is gated in the same tick, so no
        // candidate is ever selected and `defer_blocked_before` used to be
        // unreachable.
        assert!(engine
            .tick(due, mono, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .is_none());
        let pending = engine.states[0]
            .pending
            .as_ref()
            .expect("a gated occurrence stays pending");
        assert_eq!(pending.retry_at, mono + Duration::from_millis(10_000));
        assert!(engine.next_deadline(due, mono, true) > mono);
        // The gate is still closed long after that retry elapsed: the retry is
        // pushed forward again instead of parking the wake on the past.
        let later = mono + Duration::from_secs(30);
        assert!(engine
            .tick(due, later, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .is_none());
        assert!(engine.states[0].pending.as_ref().unwrap().retry_at > later);
        assert!(engine.next_deadline(due, later, true) > later);
    }

    /// The published retry is a real countdown, not the second the tick began.
    #[tokio::test]
    async fn a_deferred_job_publishes_a_future_retry_and_a_load_state() {
        let wall = wall_time();
        let mut config = job("gated", Some(8.0));
        config.retry_interval_ms = Some(60_000);
        let publisher = SchedulerPublisher::empty();
        let mut engine =
            Engine::new(std::slice::from_ref(&config), wall, 5, publisher.clone()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        assert!(engine
            .tick(due, mono, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .is_none());
        engine.publish(wall, None).await;
        let publication = publisher.current().await;
        let summary: SchedulerSummaryV2 =
            serde_json::from_slice(&publication.summary_bytes).expect("valid summary");
        summary.validate().expect("summary validates");
        let job = &summary.jobs[0];
        assert_eq!(job.state, SchedulerJobStateV2::LoadHigh);
        let retry = job
            .next_retry_unix_ms
            .expect("a load-deferred job publishes its retry");
        assert!(
            retry > summary.generated_at_unix_ms,
            "retry {retry} must be in the future, not the current second"
        );
    }

    /// The job holding the global child slot is published as running for its
    /// whole duration, not only at the instant it started.
    #[tokio::test]
    async fn the_job_holding_the_child_slot_is_published_as_running() {
        let wall = wall_time();
        let mut config = job("running", Some(8.0));
        config.retry_interval_ms = Some(10_000);
        let publisher = SchedulerPublisher::empty();
        let mut engine =
            Engine::new(std::slice::from_ref(&config), wall, 5, publisher.clone()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        let launch = engine
            .tick(due, mono, ready(1.0, 1.0, 1.0), true)
            .unwrap()
            .expect("an open gate launches");
        // A later wake, while the child still holds the slot.
        let started = now_unix_ms();
        engine.publish(wall, Some((launch.index, started))).await;
        let publication = publisher.current().await;
        let summary: SchedulerSummaryV2 =
            serde_json::from_slice(&publication.summary_bytes).expect("valid summary");
        summary.validate().expect("summary validates");
        let job = &summary.jobs[launch.index];
        assert_eq!(job.state, SchedulerJobStateV2::Running);
        assert_eq!(job.running_since_unix_ms, Some(started));
        assert!(job.pending_since_unix_ms.is_none());
    }

    #[test]
    fn load_is_rechecked_after_a_previous_child_releases_the_slot() {
        let wall = wall_time();
        let configs = [job("first", Some(8.0)), job("second", Some(8.0))];
        let mut engine = Engine::new(&configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        let first = engine
            .tick(due, mono, ready(7.9, 7.9, 7.9), true)
            .unwrap()
            .unwrap();
        assert_eq!(configs[first.index].name, "first");
        assert!(engine
            .tick(due, mono, ready(7.9, 7.9, 7.9), false)
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        assert!(engine
            .tick(due, mono, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        let retry = engine.states[1].pending.as_ref().unwrap().retry_at;
        let second = engine
            .tick(due, retry, ready(8.0, 8.0, 8.0), true)
            .unwrap()
            .unwrap();
        assert_eq!(configs[second.index].name, "second");
    }

    #[tokio::test]
    async fn spawn_failure_is_terminal_for_the_occurrence() {
        let wall = wall_time();
        let mut config = job("bad-exe", None);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let due = engine.states[0].next_due;
        let launch = engine
            .tick(due, Instant::now(), LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .unwrap();
        assert_eq!(launch.index, 0);
        assert_eq!(engine.pending_count(), 0);
        config.command[0] = "/greggd-test-does-not-exist".to_owned();
        assert!(start_child(&config, 0, true).is_err());
    }

    #[test]
    fn schedule_failure_propagates_instead_of_fabricating_a_fallback() {
        let wall = wall_time();
        let mut impossible = job("impossible", None);
        impossible.schedule = "0 0 31 2 *".to_owned();
        let configs = [job("valid", None), impossible];
        let mut engine = Engine::new_unchecked(&configs, wall);
        let due = wall;
        let error = engine
            .tick(due, Instant::now(), LoadGateState::UNAVAILABLE, true)
            .expect_err("a calendar-impossible schedule is a runtime failure");
        assert!(error.contains("impossible"), "{error}");
        assert_eq!(engine.states[1].next_due, due);
    }

    #[tokio::test]
    async fn impossible_schedule_reaches_the_run_error_boundary() {
        let mut impossible = job("impossible", None);
        impossible.schedule = "0 0 30 2 *".to_owned();
        let (load_tx, load_rx) = watch::channel(LoadGateState::UNAVAILABLE);
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
        drop(load_tx);
        let error = run(
            vec![impossible],
            5,
            SchedulerPublisher::empty(),
            load_rx,
            shutdown_rx.resubscribe(),
        )
        .await
        .expect_err("the scheduler reports an unsatisfiable schedule");
        assert!(error.contains("impossible"), "{error}");
        drop(shutdown_tx);
    }

    fn daily_job(name: &str) -> ScheduledJobConfig {
        ScheduledJobConfig {
            name: name.to_owned(),
            schedule: "0 3 * * *".to_owned(),
            command: vec!["/bin/true".to_owned()],
            working_dir: None,
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        }
    }

    #[test]
    fn civil_recheck_cap_is_one_minute_and_pure() {
        assert_eq!(MAX_CIVIL_RECHECK, Duration::from_secs(60));
        let now = Instant::now();
        // A ten-hour semantic deadline wakes within one reconciliation bound.
        assert_eq!(
            bounded_wake_deadline(now + Duration::from_secs(10 * 3600), now),
            now + MAX_CIVIL_RECHECK
        );
        // Near semantic deadlines are never delayed by the recheck.
        assert_eq!(
            bounded_wake_deadline(now + Duration::from_secs(15), now),
            now + Duration::from_secs(15)
        );
        assert_eq!(
            bounded_wake_deadline(now + Duration::from_secs(10), now),
            now + Duration::from_secs(10)
        );
        assert_eq!(
            bounded_wake_deadline(now + Duration::from_secs(20), now),
            now + Duration::from_secs(20)
        );
        assert_eq!(bounded_wake_deadline(now, now), now);
    }

    #[test]
    fn semantic_deadline_stays_truthful_while_the_wake_is_capped() {
        let wall = wall_time();
        let config = daily_job("daily");
        let engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let mono = Instant::now();
        let semantic = engine.next_deadline(wall, mono, true);
        assert!(semantic > mono, "a daily job is not due immediately");
        let wake = bounded_wake_deadline(semantic, mono);
        assert!(wake <= mono + MAX_CIVIL_RECHECK);
        // The engine itself still reports the true civil deadline; only the
        // event-loop sleep is capped.
        assert_eq!(engine.next_deadline(wall, mono, true), semantic);
    }

    #[test]
    fn empty_job_list_builds_no_engine_state() {
        // Production `run_with_shutdown` never spawns the scheduler task when
        // `config.jobs` is empty (`run.rs` keeps the `is_empty` branch), so a
        // jobless daemon pays zero reconciliation cost. At the engine layer
        // this means an empty configuration carries no deadlines at all.
        let wall = wall_time();
        let configs: &[ScheduledJobConfig] = &[];
        let engine = Engine::new(configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        assert_eq!(engine.pending_count(), 0);
        assert!(engine.states.is_empty());
    }

    #[test]
    fn forward_wall_jump_coalesces_to_one_pending_and_advances_next_due() {
        let wall = wall_time();
        let config = daily_job("daily");
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let stored = engine.states[0].next_due;
        assert!(stored > wall);
        let mono = Instant::now();
        let semantic = engine.next_deadline(wall, mono, true);
        // The long monotonic sleep is capped at one reconciliation interval.
        assert_eq!(
            bounded_wake_deadline(semantic, mono),
            mono + MAX_CIVIL_RECHECK
        );
        // Only the capped interval passes monotonically while wall time jumps
        // beyond the stored occurrence.
        let jumped_wall = stored + chrono::Duration::minutes(65);
        let launch = engine
            .tick(
                jumped_wall,
                mono + MAX_CIVIL_RECHECK,
                LoadGateState::UNAVAILABLE,
                true,
            )
            .unwrap()
            .expect("a jumped-over occurrence becomes exactly one launch");
        assert_eq!(launch.index, 0);
        assert_eq!(engine.pending_count(), 0);
        assert!(
            engine.states[0].next_due > jumped_wall,
            "next_due advances strictly after the jumped wall time"
        );
    }

    #[test]
    fn forward_jump_of_every_minute_job_does_not_build_a_backlog() {
        let wall = wall_time();
        let config = job("minutely", None);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let mono = Instant::now();
        let jumped_wall = engine.states[0].next_due + chrono::Duration::hours(3);
        let launch = engine
            .tick(jumped_wall, mono, LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .expect("several skipped minutes coalesce to one launch");
        assert_eq!(launch.index, 0);
        assert_eq!(engine.pending_count(), 0);
        assert!(engine.states[0].next_due > jumped_wall);
    }

    #[test]
    fn backward_wall_jump_does_not_launch_before_the_stored_occurrence() {
        let wall = wall_time();
        let config = job("minutely", None);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        // Shortly before the occurrence the bounded wake retains the near
        // semantic deadline instead of rounding out to the cap.
        let before = due - chrono::Duration::seconds(30);
        let semantic = engine.next_deadline(before, mono, true);
        assert_eq!(semantic, mono + Duration::from_secs(30));
        assert_eq!(bounded_wake_deadline(semantic, mono), semantic);
        // The old monotonic estimate fires while wall time has moved backward:
        // the stored occurrence is not due, so nothing launches and nothing
        // is consumed.
        let moved_back = before - chrono::Duration::minutes(5);
        let fired = mono + Duration::from_secs(30);
        assert!(engine
            .tick(moved_back, fired, LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(engine.states[0].next_due, due);
        // Subsequent wakes from the moved-back wall stay capped at the
        // reconciliation bound even though the semantic deadline is minutes
        // away.
        let semantic_after = engine.next_deadline(moved_back, fired, true);
        assert!(semantic_after > fired + MAX_CIVIL_RECHECK);
        assert_eq!(
            bounded_wake_deadline(semantic_after, fired),
            fired + MAX_CIVIL_RECHECK
        );
        // Reaching the actual civil occurrence launches exactly once.
        let launch = engine
            .tick(due, fired, LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .expect("the real civil occurrence still launches");
        assert_eq!(launch.index, 0);
        assert_eq!(engine.pending_count(), 0);
        // Moving wall time back over the consumed occurrence does not
        // recreate it.
        let recheck = due - chrono::Duration::minutes(1);
        assert!(engine
            .tick(
                recheck,
                fired + Duration::from_secs(5),
                LoadGateState::UNAVAILABLE,
                true
            )
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn load_retry_stays_monotonic_across_wall_jumps() {
        let wall = wall_time();
        let mut heavy = job("heavy", Some(0.0));
        heavy.retry_interval_ms = Some(10_000);
        heavy.max_wait_ms = Some(300_000);
        let configs = [heavy, job("time-only", None)];
        let mut engine = Engine::new(&configs, wall, 5, SchedulerPublisher::empty()).unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        // The heavy job blocks on load while the time-only job wins the free
        // slot, deferring the heavy retry ten monotonic seconds out.
        let first = engine
            .tick(due, mono, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .expect("the time-only job takes the free slot");
        assert_eq!(configs[first.index].name, "time-only");
        assert_eq!(
            engine.states[0].pending.as_ref().unwrap().retry_at,
            mono + Duration::from_millis(10_000)
        );
        // Same wall, monotonic retry not yet crossed: nothing is eligible.
        assert!(engine
            .tick(
                due,
                mono + Duration::from_secs(5),
                ready(0.0, 0.0, 0.0),
                true
            )
            .unwrap()
            .is_none());
        // Wall time jumps hours forward while monotonic time stays short of
        // the retry: the heavy retry is still not due. Only the time-only
        // job's fresh occurrence may launch.
        let far_forward = due + chrono::Duration::hours(3);
        let launch = engine
            .tick(
                far_forward,
                mono + Duration::from_secs(5),
                ready(0.0, 0.0, 0.0),
                true,
            )
            .unwrap()
            .expect("the fresh time-only occurrence launches");
        assert_eq!(configs[launch.index].name, "time-only");
        assert!(engine.states[0].pending.is_some(), "heavy stays pending");
        // Wall time moves backward while monotonic time crosses the retry:
        // the heavy job becomes due on its monotonic schedule.
        let moved_back = due - chrono::Duration::hours(1);
        let launch = engine
            .tick(
                moved_back,
                mono + Duration::from_secs(10),
                ready(0.0, 0.0, 0.0),
                true,
            )
            .unwrap()
            .expect("crossing the monotonic retry launches the heavy job");
        assert_eq!(configs[launch.index].name, "heavy");
    }

    #[test]
    fn max_wait_expiry_stays_monotonic_across_wall_jumps() {
        let wall = wall_time();
        let mut config = job("heavy", Some(0.0));
        config.retry_interval_ms = Some(10_000);
        config.max_wait_ms = Some(120_000);
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall,
            5,
            SchedulerPublisher::empty(),
        )
        .unwrap();
        let due = engine.states[0].next_due;
        let mono = Instant::now();
        assert!(engine
            .tick(due, mono, ready(9.0, 9.0, 9.0), true)
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        // A forward wall jump cannot shorten the monotonic max-wait: sixty
        // monotonic seconds into a 120-second bound the occurrence survives.
        let far_forward = due + chrono::Duration::hours(3);
        assert!(engine
            .tick(
                far_forward,
                mono + Duration::from_secs(60),
                ready(9.0, 9.0, 9.0),
                true
            )
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        // A backward wall jump cannot extend it either: at 120 monotonic
        // seconds the pending occurrence expires once.
        let moved_back = due - chrono::Duration::hours(1);
        assert!(engine
            .tick(
                moved_back,
                mono + Duration::from_secs(120),
                ready(9.0, 9.0, 9.0),
                true
            )
            .unwrap()
            .is_none());
        assert_eq!(engine.pending_count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn active_direct_child_is_terminated_and_reaped_on_shutdown() {
        let job = ScheduledJobConfig {
            name: "long-test-child".to_owned(),
            schedule: "* * * * *".to_owned(),
            command: vec!["/bin/sleep".to_owned(), "30".to_owned()],
            working_dir: None,
            max_load: None,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        };
        let mut child = start_child(&job, 0, true).unwrap();
        child.child.start_kill().unwrap();
        let result = tokio::time::timeout(CHILD_SHUTDOWN_BOUND, child.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(result.code().is_none());
    }
}

// ===== Plan 163: deterministic scheduler observability tests =====

#[cfg(test)]
mod observation_tests {
    use super::*;
    use crate::scheduler::observation::{SchedulerObserver, TerminalRecord, DRAIN_CHUNK};
    use gregg_protocol::{
        SchedulerHistoryV2, SchedulerJobStateV2, SchedulerSummaryV2, MAX_SCHEDULER_HISTORY_LIMIT,
        MAX_SCHEDULER_OUTPUT_TEXT_BYTES,
    };

    fn job_config(name: &str, command: &[&str], max_load: Option<f32>) -> ScheduledJobConfig {
        job_config_owned(
            name,
            command.iter().map(|part| (*part).to_owned()).collect(),
            max_load,
        )
    }

    fn job_config_owned(
        name: &str,
        command: Vec<String>,
        max_load: Option<f32>,
    ) -> ScheduledJobConfig {
        ScheduledJobConfig {
            name: name.to_owned(),
            schedule: "* * * * *".to_owned(),
            command,
            working_dir: None,
            max_load,
            load_window: None,
            retry_interval_ms: None,
            max_wait_ms: None,
        }
    }

    fn wall_time() -> DateTime<Local> {
        chrono::TimeZone::with_ymd_and_hms(&Local, 2026, 10, 4, 12, 0, 0).unwrap()
    }

    /// Spawn a child and return the terminal record the scheduler would
    /// record.
    ///
    /// The wait itself is the production [`await_child_completion`] future, not
    /// a reimplementation of it: a helper that joined the three futures its own
    /// way would keep passing while production awaited `wait()` first and
    /// wedged on any child that writes more than one pipe buffer. The record's
    /// finish instant and duration are the completion's frozen exit event,
    /// exactly as [`finish_child`] takes them.
    ///
    /// Only the Unix tests below drive a real child, so this helper is Unix
    /// only: on Windows it would otherwise be dead code, and CI builds with
    /// `-D warnings`.
    #[cfg(unix)]
    async fn run_child_to_completion(
        job: &ScheduledJobConfig,
        launched: Launch,
    ) -> (TerminalRecord, SchedulerOutcomeV2) {
        let mut child = start_child(job, 0, true).expect("child spawns");
        child.scheduled_unix_ms = launched.scheduled.timestamp_millis().max(0).unsigned_abs();
        child.delay_ms = duration_millis(launched.pending_age);
        child.coalesced = launched.coalesced;
        let completion = await_child_completion(&mut child).await;
        let status = completion.status.expect("child exit observed");
        let (stdout, stderr) = (completion.stdout, completion.stderr);
        let exit_code = status.code();
        let signal = exit_signal(status);
        let outcome = if exit_code == Some(0) {
            SchedulerOutcomeV2::Success
        } else {
            SchedulerOutcomeV2::Failed
        };
        (
            TerminalRecord {
                scheduled_unix_ms: child.scheduled_unix_ms,
                started_unix_ms: Some(child.started_unix_ms),
                finished_unix_ms: completion.finished_unix_ms,
                delay_ms: child.delay_ms,
                coalesced: child.coalesced,
                exit_code,
                signal,
                duration_ms: Some(completion.duration_ms),
                stdout,
                stderr,
            },
            outcome,
        )
    }

    /// Unix only, for the same reason as [`run_child_to_completion`].
    #[cfg(unix)]
    fn launch_for(engine: &Engine<'_>, index: usize, pending_age: Duration) -> Launch {
        let _ = engine;
        Launch {
            index,
            pending_age,
            observed_load: None,
            load_window: None,
            scheduled: wall_time(),
            coalesced: false,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_child_records_exit_zero_and_bounded_output() {
        let job = job_config(
            "echo",
            &["/bin/sh", "-c", "printf 'hello'; printf 'warn' 1>&2"],
            None,
        );
        let configs = std::slice::from_ref(&job);
        let engine = Engine::new(configs, wall_time(), 5, SchedulerPublisher::empty()).unwrap();
        let launch = launch_for(&engine, 0, Duration::ZERO);
        let (record, outcome) = run_child_to_completion(&job, launch).await;
        assert_eq!(outcome, SchedulerOutcomeV2::Success);

        let mut observer = SchedulerObserver::new(
            std::slice::from_ref(&job.name),
            5,
            1_700_000_000_000,
            SchedulerPublisher::empty(),
        );
        observer.record_terminal(0, record, outcome);
        let document = published_history(&mut observer, 1).await;
        document.validate().expect("history validates");
        let stored = &document.jobs[0].records[0];
        assert_eq!(stored.outcome, SchedulerOutcomeV2::Success);
        assert_eq!(stored.exit_code, Some(0));
        assert_eq!(stored.stdout.text, "hello");
        assert_eq!(stored.stderr.text, "warn");
        assert!(!stored.stdout.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failing_child_records_status_and_stderr() {
        let job = job_config(
            "failing",
            &["/bin/sh", "-c", "printf 'boom' 1>&2; exit 3"],
            None,
        );
        let configs = std::slice::from_ref(&job);
        let engine = Engine::new(configs, wall_time(), 5, SchedulerPublisher::empty()).unwrap();
        let launch = launch_for(&engine, 0, Duration::ZERO);
        let (record, outcome) = run_child_to_completion(&job, launch).await;
        assert_eq!(outcome, SchedulerOutcomeV2::Failed);
        assert_eq!(record.exit_code, Some(3));
        assert!(record.duration_ms.is_some());
        let stdout = record.stdout.into_wire();
        assert_eq!(stdout.text, "");
    }

    /// Output a child wrote before a deadline wake must still be published.
    ///
    /// `run` caps every wake at [`MAX_CIVIL_RECHECK`], so any child that
    /// outlives one wake has its completion future — and with it the whole
    /// drain — cancelled and rebuilt. The tail has to live on the child for the
    /// record to describe the run instead of only its last minute. The
    /// intervals here are injected and short; the production 60s cap is
    /// exercised in `bounded_wake_deadline` above.
    #[cfg(unix)]
    #[tokio::test]
    async fn output_written_before_a_deadline_wake_still_reaches_the_record() {
        let job = job_config(
            "slow-writer",
            &["/bin/sh", "-c", "echo the-log-line; sleep 0.5"],
            None,
        );
        let mut child = start_child(&job, 0, true).expect("child spawns");

        // First wake: cancel the completion future while the child still runs,
        // exactly as the civil-clock cap does.
        let wake_at = Instant::now() + Duration::from_millis(150);
        let cancelled = tokio::select! {
            result = async { await_child_completion(&mut child).await.status } => {
                panic!("child outlived the injected wake: {result:?}")
            }
            () = tokio::time::sleep_until(wake_at) => true,
        };
        assert!(cancelled);

        // Second wake: the rebuilt drain continues into the same tail.
        let completion = await_child_completion(&mut child).await;
        assert_eq!(
            completion.status.expect("child exit observed").code(),
            Some(0)
        );
        let stdout = completion.stdout.into_wire();
        assert_eq!(
            stdout.text, "the-log-line\n",
            "output read before the wake must not be discarded"
        );
        assert!(
            !stdout.truncated,
            "a short complete run must not claim its output was truncated"
        );
    }

    /// The output-flood proof: a child writing far beyond both caps must not
    /// block, must not grow retention with total bytes, and must still record a
    /// truthful terminal outcome.
    #[cfg(unix)]
    #[tokio::test]
    async fn output_flood_stays_bounded_and_never_deadlocks() {
        // 4 MiB on each stream: 4096x the 1024-byte raw cap and 8192x the
        // 512-byte published cap.
        let job = job_config(
            "flood",
            &[
                "/bin/sh",
                "-c",
                "i=0; while [ $i -lt 1024 ]; do printf '%01000d' 0; i=$((i+1)); done; \
                 i=0; while [ $i -lt 1024 ]; do printf 'E%01000d' 0 1>&2; i=$((i+1)); done",
            ],
            None,
        );
        let configs = std::slice::from_ref(&job);
        let engine = Engine::new(configs, wall_time(), 5, SchedulerPublisher::empty()).unwrap();
        let launch = launch_for(&engine, 0, Duration::ZERO);
        // A generous bound: the point is that the child finishes at all, not
        // that it finishes quickly.
        let (record, outcome) = tokio::time::timeout(
            Duration::from_secs(30),
            run_child_to_completion(&job, launch),
        )
        .await
        .expect("a flooding child must not deadlock the scheduler");
        assert_eq!(outcome, SchedulerOutcomeV2::Success);
        let stdout = record.stdout.into_wire();
        let stderr = record.stderr.into_wire();
        assert!(stdout.truncated, "stdout past the cap must be marked");
        assert!(stderr.truncated, "stderr past the cap must be marked");
        assert!(
            stdout.text.len() <= gregg_protocol::MAX_SCHEDULER_OUTPUT_TEXT_BYTES,
            "stdout retained {} bytes",
            stdout.text.len()
        );
        assert!(stderr.text.len() <= gregg_protocol::MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn disabled_capture_keeps_the_original_null_streams() {
        let job = job_config("quiet", &["/bin/sh", "-c", "printf 'ignored'"], None);
        let mut child = start_child(&job, 0, false).expect("child spawns");
        assert!(child.stdout.is_none());
        assert!(child.stderr.is_none());
        let status = tokio::time::timeout(CHILD_SHUTDOWN_BOUND, child.child.wait())
            .await
            .expect("child completes")
            .expect("exit observed");
        assert_eq!(status.code(), Some(0));
    }

    /// Unix only. How long the inheriting helper descendant stays alive.
    ///
    /// Far longer than the settle bound asserted against, so a regression is
    /// caught as a measured wait rather than a hang, and short enough to be
    /// harmless if a test cannot clean the descendant up itself.
    #[cfg(unix)]
    const INHERITING_DESCENDANT_SECS: u64 = 30;

    /// Unix only. A scheduled command that backgrounds a descendant which keeps
    /// the inherited stdout write end open, records that descendant's pid so the
    /// test can terminate it, and then exits successfully.
    ///
    /// The shell exists only to create the inherited-descriptor condition.
    /// Production execution stays direct argv with no shell.
    #[cfg(unix)]
    fn inheriting_job(name: &str, pid_file: &std::path::Path) -> ScheduledJobConfig {
        job_config_owned(
            name,
            vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!(
                    "sleep {INHERITING_DESCENDANT_SECS} & printf %s $! > {}; \
                     printf 'direct-child-done'",
                    pid_file.display()
                ),
            ],
            None,
        )
    }

    /// Unix only. Terminate the backgrounded helper descendant, best effort.
    ///
    /// The descendant is already bounded by
    /// [`INHERITING_DESCENDANT_SECS`], so a missing `kill`, an already-exited
    /// process, or an unread pid file is not a test failure — it only means the
    /// helper cleaned itself up.
    #[cfg(unix)]
    fn terminate_helper_descendant(pid_file: &std::path::Path) {
        let Ok(contents) = std::fs::read_to_string(pid_file) else {
            return;
        };
        let _ = std::fs::remove_file(pid_file);
        let Ok(pid) = contents.trim().parse::<i32>() else {
            return;
        };
        let _ = std::process::Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }

    /// Unix only. A unique pid file for one inheriting helper descendant.
    #[cfg(unix)]
    fn helper_pid_file(test: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "greggd_inherited_writer_{test}_{}.pid",
            std::process::id()
        ))
    }

    /// The plan's core regression: a scheduled command that backgrounds a
    /// descendant holding the inherited stdout must not keep greggd's one
    /// global child slot after the direct child itself has exited.
    ///
    /// A completion that waits for pipe EOF would block for the whole
    /// descendant lifetime here. The production boundary is the direct child's
    /// wait result plus the fixed post-exit settle bound, so the slot is freed
    /// after the descendant's write end has had that long to produce bytes.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_inherited_output_writer_cannot_retain_the_global_child_slot() {
        let pid_file = helper_pid_file("retain");
        let job = inheriting_job("inherits", &pid_file);
        let mut child = start_child(&job, 0, true).expect("child spawns");

        let started = Instant::now();
        let completion = tokio::time::timeout(
            Duration::from_secs(INHERITING_DESCENDANT_SECS / 2),
            await_child_completion(&mut child),
        )
        .await
        .expect("the direct child's completion must not wait for the descendant");
        let elapsed = started.elapsed();
        terminate_helper_descendant(&pid_file);

        assert_eq!(
            completion.status.expect("child exit observed").code(),
            Some(0)
        );
        assert_eq!(
            completion.stdout.into_wire().text,
            "direct-child-done",
            "output the direct child wrote before exiting is still captured"
        );
        assert!(
            elapsed >= POST_EXIT_OUTPUT_SETTLE,
            "the inherited writer must actually be waited out for the settle \
             bound, or this proves nothing ({elapsed:?})"
        );
        assert!(
            elapsed < Duration::from_secs(INHERITING_DESCENDANT_SECS / 2),
            "the inherited writer retained the slot for {elapsed:?}"
        );
        // The direct child's own timing is the execution event; the post-exit
        // settle must never be charged to its recorded duration.
        assert!(
            completion.duration_ms < duration_millis(POST_EXIT_OUTPUT_SETTLE),
            "duration {} ms must describe the direct child, not the settle",
            completion.duration_ms
        );
    }

    /// The one global child slot is the whole point: once the direct child's
    /// completion boundary is crossed, the next due job must be startable even
    /// while a descendant still holds the previous child's output descriptors.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_following_job_starts_after_the_direct_child_completion_boundary() {
        let pid_file = helper_pid_file("following");
        let first = inheriting_job("first", &pid_file);
        let second = job_config("second", &["/bin/sh", "-c", "printf 'second-job'"], None);
        let mut child = start_child(&first, 0, true).expect("child spawns");
        let completion = tokio::time::timeout(
            Duration::from_secs(INHERITING_DESCENDANT_SECS / 2),
            await_child_completion(&mut child),
        )
        .await
        .expect("completion must not wait for the inherited writer");
        terminate_helper_descendant(&pid_file);
        assert_eq!(
            completion.status.expect("child exit observed").code(),
            Some(0)
        );
        // `run` frees the global child slot by dropping the completed child and
        // republishing, which is exactly this boundary.
        drop(child);

        let mut next = start_child(&second, 1, true).expect("the next job starts");
        let next_completion =
            tokio::time::timeout(Duration::from_secs(10), await_child_completion(&mut next))
                .await
                .expect("the following job must complete promptly");
        assert_eq!(
            next_completion.status.expect("child exit observed").code(),
            Some(0)
        );
        assert_eq!(next_completion.stdout.into_wire().text, "second-job");
    }

    /// Two configured jobs share a name on purpose. A terminal record must be
    /// attributed through the index the child was launched with: a name search
    /// could not tell them apart, and there is no index-zero fallback.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_terminal_record_is_attributed_through_the_carried_index() {
        let jobs = vec![
            job_config("duplicate", &["/bin/sh", "-c", "printf first"], None),
            job_config("duplicate", &["/bin/sh", "-c", "printf second"], None),
        ];
        let mut engine = Engine::new_unchecked(&jobs, wall_time());
        let mut child = start_child(&jobs[1], 1, true).expect("child spawns");
        let completion = await_child_completion(&mut child).await;
        let finished = finish_child(&mut engine, &child, completion);
        assert_eq!(finished.outcome, SchedulerOutcomeV2::Success);
        assert_eq!(finished.exit_code, Some(0));
        assert!(
            engine.observer.last_summary(0).is_none(),
            "a same-named sibling must never receive another job's record"
        );
        let recorded = engine
            .observer
            .last_summary(1)
            .expect("the record belongs to the launched index");
        assert_eq!(recorded.exit_code, Some(0));
        assert_eq!(recorded.duration_ms, Some(finished.duration_ms));
        let wall_window = recorded
            .finished_unix_ms
            .saturating_sub(child.started_unix_ms);
        assert!(
            wall_window.abs_diff(finished.duration_ms) <= 1,
            "the recorded window must be the direct child's own ({wall_window} ms \
             vs {} ms)",
            finished.duration_ms
        );
        assert!(
            wall_window < duration_millis(POST_EXIT_OUTPUT_SETTLE),
            "the recorded window must exclude the post-exit settle ({wall_window} ms)"
        );
    }

    /// A reader with scripted readiness, so post-exit settle progress can be
    /// tested without depending on OS pipe scheduling.
    ///
    /// It yields exactly `blocks` full chunks, each filled with `byte`, and then
    /// reports EOF. Every `poll_read` hands over the whole script at once, so
    /// the number of completed `drain_step` calls is the number of iterations
    /// the settle helper managed — which is exactly what is under test.
    struct ScriptedReader {
        blocks: usize,
        byte: u8,
    }

    impl ScriptedReader {
        fn new(blocks: usize, byte: u8) -> Self {
            Self { blocks, byte }
        }
    }

    impl tokio::io::AsyncRead for ScriptedReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let this = self.get_mut();
            if this.blocks == 0 {
                // Empty result is EOF.
                return std::task::Poll::Ready(Ok(()));
            }
            let count = this.blocks.min(buf.remaining() / DRAIN_CHUNK);
            this.blocks -= count;
            let chunk = [this.byte; DRAIN_CHUNK];
            buf.put_slice(&chunk[..count * DRAIN_CHUNK]);
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// A pipe that stays open and permanently idle.
    ///
    /// This is the shape of a descriptor whose only writer is a descendant that
    /// inherited it and has nothing left to say: never readable, never EOF. It
    /// never registers a waker, so only the settle deadline can end it — which
    /// is precisely why it must not be able to withhold the other stream's
    /// progress.
    #[derive(Debug, Default)]
    struct IdleOpenReader;

    impl tokio::io::AsyncRead for IdleOpenReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Pending
        }
    }

    /// Poll the production settle helper the way a scheduler wake does: a fresh
    /// future, dropped one poll later.
    ///
    /// A zero timeout never sleeps, so each call runs the helper's select until
    /// it either returns or parks, and the loop below repeats it. Progress is
    /// therefore driven entirely by what the scripted readers return — no wall
    /// clock, no `test-util` timer, and no dependence on the settle deadline
    /// firing.
    #[allow(clippy::too_many_arguments)]
    async fn poll_settle_once<O, E>(
        stdout: &mut O,
        stdout_tail: &mut OutputTail,
        stdout_done: &mut bool,
        stderr: &mut E,
        stderr_tail: &mut OutputTail,
        stderr_done: &mut bool,
        settle_deadline: Instant,
    ) -> bool
    where
        O: tokio::io::AsyncRead + Unpin,
        E: tokio::io::AsyncRead + Unpin,
    {
        tokio::time::timeout(
            Duration::ZERO,
            settle_output(
                Some(&mut *stdout),
                stdout_tail,
                stdout_done,
                Some(&mut *stderr),
                stderr_tail,
                stderr_done,
                settle_deadline,
            ),
        )
        .await
        .is_ok()
    }

    /// The plan's core regression at the helper level: stdout has three chunks
    /// already buffered while stderr stays open and idle.
    ///
    /// Joining the two reads made stdout's second and third chunks unreachable:
    /// each joined iteration handed over one stdout chunk and then parked on
    /// stderr, so the settle ended with only the first chunk retained.
    #[tokio::test]
    async fn an_idle_stderr_does_not_withhold_ready_stdout_chunks() {
        let mut stdout = ScriptedReader::new(3, b'a');
        let mut stderr = IdleOpenReader;
        let mut stdout_tail = OutputTail::new();
        let mut stderr_tail = OutputTail::new();
        let mut stdout_done = false;
        let mut stderr_done = false;
        let settle_deadline = Instant::now() + POST_EXIT_OUTPUT_SETTLE;

        // One poll is enough to drain every chunk and observe EOF: the helper
        // iterated repeatedly without ever parking on stderr. Joining the two
        // reads instead parked after the first chunk, so this never completes
        // on one poll.
        assert!(
            !poll_settle_once(
                &mut stdout,
                &mut stdout_tail,
                &mut stdout_done,
                &mut stderr,
                &mut stderr_tail,
                &mut stderr_done,
                settle_deadline,
            )
            .await,
            "the settle must not finish while stderr's writer is still open"
        );
        assert!(
            stdout_done,
            "the ready stream must reach EOF while stderr stays pending"
        );
        assert!(
            !stderr_done,
            "a stream whose inherited writer stays open can never report EOF"
        );
        assert_eq!(
            stdout_tail.total_bytes(),
            3 * DRAIN_CHUNK as u64,
            "more than one chunk must be consumed while stderr stays pending"
        );
        assert_eq!(stderr_tail.total_bytes(), 0);

        // The settle is still genuinely waiting on stderr: another poll neither
        // completes nor drains anything, because the bound has not expired.
        assert!(
            !poll_settle_once(
                &mut stdout,
                &mut stdout_tail,
                &mut stdout_done,
                &mut stderr,
                &mut stderr_tail,
                &mut stderr_done,
                settle_deadline,
            )
            .await,
            "the settle must remain pending on the open idle stream"
        );
        assert_eq!(stderr_tail.total_bytes(), 0);

        // Retention is unaffected by how much the stream carried: the tail is
        // the final bounded bytes, whatever the total observed.
        let wire = stdout_tail.into_wire();
        assert_eq!(
            wire.text.len(),
            MAX_SCHEDULER_OUTPUT_TEXT_BYTES,
            "the retained tail is the final bounded bytes of that stream"
        );
        assert!(wire.truncated, "retention stays bounded");
    }

    /// The mirror case: stderr is the ready stream and stdout the idle one.
    ///
    /// This one cannot be reached by simply swapping arguments, because the
    /// alternating preference only tries the idle stream after the ready one.
    #[tokio::test]
    async fn an_idle_stdout_does_not_withhold_ready_stderr_chunks() {
        let mut stdout = IdleOpenReader;
        let mut stderr = ScriptedReader::new(3, b'b');
        let mut stdout_tail = OutputTail::new();
        let mut stderr_tail = OutputTail::new();
        let mut stdout_done = false;
        let mut stderr_done = false;
        let settle_deadline = Instant::now() + POST_EXIT_OUTPUT_SETTLE;

        // The mirror case cannot be reached by only preferring stdout: stderr's
        // own read has to win its turn.
        assert!(
            !poll_settle_once(
                &mut stdout,
                &mut stdout_tail,
                &mut stdout_done,
                &mut stderr,
                &mut stderr_tail,
                &mut stderr_done,
                settle_deadline,
            )
            .await,
            "the settle must not finish while stdout's writer is still open"
        );
        assert!(
            stderr_done,
            "stderr must reach EOF while stdout stays pending"
        );
        assert!(!stdout_done, "an idle open stdout cannot report EOF");
        assert_eq!(
            stderr_tail.total_bytes(),
            3 * DRAIN_CHUNK as u64,
            "stderr progress must not wait on an idle open stdout"
        );
        assert_eq!(stdout_tail.total_bytes(), 0);
    }

    /// Both streams ready at once must both progress: the preferred branch
    /// alternates, so neither stream can starve the other for the whole settle.
    #[tokio::test]
    async fn two_ready_streams_both_make_progress_within_the_settle() {
        let mut stdout = ScriptedReader::new(2, b'a');
        let mut stderr = ScriptedReader::new(2, b'b');
        let mut stdout_tail = OutputTail::new();
        let mut stderr_tail = OutputTail::new();
        let mut stdout_done = false;
        let mut stderr_done = false;
        let settle_deadline = Instant::now() + POST_EXIT_OUTPUT_SETTLE;

        // Both streams reach EOF without the deadline ever being consulted.
        assert!(tokio::time::timeout(
            Duration::ZERO,
            settle_output(
                Some(&mut stdout),
                &mut stdout_tail,
                &mut stdout_done,
                Some(&mut stderr),
                &mut stderr_tail,
                &mut stderr_done,
                settle_deadline,
            ),
        )
        .await
        .is_ok());

        assert!(stdout_done && stderr_done, "both streams reach EOF");
        assert_eq!(stdout_tail.total_bytes(), 2 * DRAIN_CHUNK as u64);
        assert_eq!(
            stderr_tail.total_bytes(),
            2 * DRAIN_CHUNK as u64,
            "an alternating preference must not starve either stream"
        );
    }

    /// The settle is bounded by the deadline frozen at the direct child's exit,
    /// and it returns *without* an EOF from the open writer. Two permanently
    /// idle streams are the strongest case: nothing can ever finish them, so
    /// only the deadline can end the wait.
    #[tokio::test]
    async fn a_silent_settle_ends_at_the_frozen_deadline_without_an_eof() {
        let mut stdout = IdleOpenReader;
        let mut stderr = IdleOpenReader;
        let mut stdout_tail = OutputTail::new();
        let mut stderr_tail = OutputTail::new();
        let mut stdout_done = false;
        let mut stderr_done = false;
        // An already-expired deadline: `sleep_until` is ready on the first poll,
        // so this proves the bound is what ends the settle without waiting.
        let settle_deadline = Instant::now() - Duration::from_millis(1);

        settle_output(
            Some(&mut stdout),
            &mut stdout_tail,
            &mut stdout_done,
            Some(&mut stderr),
            &mut stderr_tail,
            &mut stderr_done,
            settle_deadline,
        )
        .await;

        assert!(
            !stdout_done && !stderr_done,
            "no amount of settling produces EOF from an idle writer"
        );
        assert_eq!(stdout_tail.total_bytes(), 0);
        assert_eq!(stderr_tail.total_bytes(), 0);
    }

    /// Shutdown keeps its established bound: a killed direct child is reaped
    /// inside [`CHILD_SHUTDOWN_BOUND`] while a descendant still holds its
    /// output descriptors, so no inherited writer can extend daemon shutdown.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_reaps_a_killed_child_despite_an_inherited_writer() {
        let pid_file = helper_pid_file("shutdown");
        let job = job_config(
            "long-running",
            &[
                "/bin/sh",
                "-c",
                &format!("sleep {INHERITING_DESCENDANT_SECS} & wait"),
            ],
            None,
        );
        let mut child = start_child(&job, 0, true).expect("child spawns");
        // Let the helper background its descendant before the kill request, so
        // the pipes are genuinely inherited while the child is reaped.
        tokio::time::sleep(Duration::from_millis(150)).await;

        let started = Instant::now();
        child.child.start_kill().expect("kill request");
        let outcome = tokio::time::timeout(CHILD_SHUTDOWN_BOUND, child.child.wait())
            .await
            .expect("a killed direct child must be reaped inside the shutdown bound");
        let elapsed = started.elapsed();
        terminate_helper_descendant(&pid_file);
        assert!(outcome.is_ok());
        assert!(
            elapsed < CHILD_SHUTDOWN_BOUND,
            "shutdown waited {elapsed:?} on an inherited output writer"
        );
        // The read ends close with the child, so nothing on the descendant's
        // side is left open on the scheduler's behalf.
        drop(child);
    }

    /// The ordinary path still captures both streams and is not charged a
    /// settle on top of its own work: a child that closes its own descriptors
    /// reaches EOF, which ends the settle loop immediately. Spawning and
    /// reaping `/bin/sh` is single-digit milliseconds, so the 250 ms settle
    /// bound is a ~100x margin over the whole completion here.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_ordinary_exit_is_captured_without_spending_the_settle_bound() {
        let job = job_config(
            "ordinary",
            &["/bin/sh", "-c", "printf out; printf err 1>&2"],
            None,
        );
        let mut child = start_child(&job, 0, true).expect("child spawns");
        let started = Instant::now();
        let completion = await_child_completion(&mut child).await;
        let elapsed = started.elapsed();
        assert_eq!(
            completion.status.expect("child exit observed").code(),
            Some(0)
        );
        let stdout = completion.stdout.into_wire();
        let stderr = completion.stderr.into_wire();
        assert_eq!(stdout.text, "out");
        assert_eq!(stderr.text, "err");
        assert!(!stdout.truncated);
        assert!(!stderr.truncated);
        assert!(
            child.output_drained(),
            "a child that closed its own descriptors must reach pipe EOF"
        );
        assert!(
            elapsed < POST_EXIT_OUTPUT_SETTLE,
            "an ordinary exit must not wait out the settle bound ({elapsed:?})"
        );
    }

    /// A completion future cancelled inside the post-exit settle keeps the
    /// frozen exit and finishes its remaining settle on the next wake: the exit
    /// instant, exit status, and settle budget are all owned by the child.
    ///
    /// The cancellation is deterministic rather than a timing race. Polling the
    /// production future under a zero timeout is exactly what a deadline wake
    /// does — a fresh future, dropped one poll later — and it never sleeps, so
    /// the wake lands inside the settle window by construction once the exit is
    /// frozen.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_settle_cancelled_by_a_wake_keeps_the_frozen_exit_and_budget() {
        let pid_file = helper_pid_file("settle");
        let job = inheriting_job("inherits", &pid_file);
        let mut child = start_child(&job, 0, true).expect("child spawns");

        // Poll the production future until the direct child has exited.
        let give_up = Instant::now() + Duration::from_secs(20);
        while child.exit.is_none() {
            assert!(Instant::now() < give_up, "the direct child never exited");
            let _ = tokio::time::timeout(Duration::ZERO, await_child_completion(&mut child)).await;
            tokio::task::yield_now().await;
        }
        let frozen = child
            .exit
            .as_ref()
            .expect("the exit is frozen on the child");
        let (finished, finished_unix_ms) = (frozen.finished, frozen.finished_unix_ms);
        assert_eq!(
            child.settle_deadline,
            Some(finished + POST_EXIT_OUTPUT_SETTLE),
            "the settle budget is fixed at the exit instant"
        );

        // Another cancelled poll: the descendant still holds the write end, so
        // the settle is provably still in progress.
        let cancelled =
            tokio::time::timeout(Duration::ZERO, await_child_completion(&mut child)).await;
        assert!(
            cancelled.is_err(),
            "the settle must still be waiting on the inherited writer"
        );
        assert_eq!(
            child.exit.as_ref().map(|exit| exit.finished_unix_ms),
            Some(finished_unix_ms),
            "a cancelled future must not re-stamp or lose the frozen exit"
        );

        // The rebuilt future finishes the settle and reports the same exit.
        let completion = await_child_completion(&mut child).await;
        terminate_helper_descendant(&pid_file);
        assert_eq!(
            completion.status.expect("child exit observed").code(),
            Some(0)
        );
        assert_eq!(
            completion.finished_unix_ms, finished_unix_ms,
            "a rebuilt completion must not re-stamp the direct child's exit"
        );
        assert_eq!(completion.stdout.into_wire().text, "direct-child-done");
    }

    #[tokio::test]
    async fn spawn_failure_is_recorded_as_a_terminal_outcome() {
        let job = job_config("missing", &["/definitely/not/a/real/executable-xyz"], None);
        let publisher = SchedulerPublisher::empty();
        let mut observer = SchedulerObserver::new(
            std::slice::from_ref(&job.name),
            5,
            1_700_000_000_000,
            publisher,
        );
        let launch = Launch {
            index: 0,
            pending_age: Duration::from_millis(250),
            observed_load: None,
            load_window: None,
            scheduled: wall_time(),
            coalesced: false,
        };
        // Mirror the scheduler's own spawn-failure path.
        let Err(error) = start_child(&job, 0, true) else {
            panic!("a missing executable cannot spawn");
        };
        assert_ne!(error, "");
        observer.record_terminal(
            launch.index,
            TerminalRecord::without_child(
                launch.scheduled.timestamp_millis().max(0).unsigned_abs(),
                now_unix_ms(),
                duration_millis(launch.pending_age),
                launch.coalesced,
                SchedulerOutcomeV2::SpawnFailed,
            ),
            SchedulerOutcomeV2::SpawnFailed,
        );
        let document = published_history(&mut observer, 1).await;
        document.validate().expect("history validates");
        assert_eq!(
            document.jobs[0].records[0].outcome,
            SchedulerOutcomeV2::SpawnFailed
        );
    }

    #[tokio::test]
    async fn load_expiry_records_a_terminal_outcome_without_a_child() {
        let mut config = job_config("heavy", &["/bin/true"], Some(1.0));
        config.retry_interval_ms = Some(10_000);
        config.max_wait_ms = Some(120_000);
        let publisher = SchedulerPublisher::empty();
        let mut engine = Engine::new(
            std::slice::from_ref(&config),
            wall_time(),
            5,
            publisher.clone(),
        )
        .unwrap();
        let due = engine.states[0].next_due;
        let since = Instant::now();
        // Due, then deferred: never launched while the load gate is closed.
        assert!(engine
            .tick(due, since, LoadGateState::UNAVAILABLE, false)
            .unwrap()
            .is_none());
        // Past max_wait, the occurrence expires and becomes terminal.
        assert!(engine
            .tick(
                due,
                since + Duration::from_secs(180),
                LoadGateState::UNAVAILABLE,
                false
            )
            .unwrap()
            .is_none());
        let mut observer =
            SchedulerObserver::new(&[config.name.clone()], 5, 1_700_000_000_000, publisher);
        // The engine's own record first: the wire proof below is built from a
        // fresh observer, so without this the expiry could be one the engine
        // never actually recorded.
        let recorded = engine
            .observer
            .last_summary(0)
            .expect("an expired occurrence is a terminal record");
        assert_eq!(recorded.outcome, SchedulerOutcomeV2::LoadExpired);
        assert_eq!(recorded.exit_code, None);
        assert_eq!(recorded.duration_ms, None);
        // Re-derive the record the engine recorded, for the document proof.
        observer.record_terminal(
            0,
            TerminalRecord::without_child(
                due.timestamp_millis().max(0).unsigned_abs(),
                now_unix_ms(),
                120_000,
                false,
                SchedulerOutcomeV2::LoadExpired,
            ),
            SchedulerOutcomeV2::LoadExpired,
        );
        let expired = published_history(&mut observer, 1).await;
        expired.validate().expect("history validates");
        let stored = &expired.jobs[0].records[0];
        assert_eq!(stored.outcome, SchedulerOutcomeV2::LoadExpired);
        assert_eq!(stored.started_unix_ms, None);
        assert_eq!(stored.duration_ms, None);
        assert_eq!(stored.exit_code, None);
    }

    #[tokio::test]
    async fn per_job_ring_evicts_at_the_qualified_depth() {
        let job = job_config("repeat", &["/bin/true"], None);
        let publisher = SchedulerPublisher::empty();
        let mut observer = SchedulerObserver::new(
            std::slice::from_ref(&job.name),
            MAX_SCHEDULER_HISTORY_LIMIT,
            1_700_000_000_000,
            publisher,
        );
        for _ in 0..(MAX_SCHEDULER_HISTORY_LIMIT + 2) {
            observer.record_terminal(
                0,
                TerminalRecord {
                    scheduled_unix_ms: 1_700_000_000_000,
                    started_unix_ms: Some(1_700_000_001_000),
                    finished_unix_ms: 1_700_000_002_000,
                    delay_ms: 0,
                    coalesced: false,
                    exit_code: Some(0),
                    signal: None,
                    duration_ms: Some(1_000),
                    stdout: observation::OutputTail::new(),
                    stderr: observation::OutputTail::new(),
                },
                SchedulerOutcomeV2::Success,
            );
        }
        let document = published_history(&mut observer, 1).await;
        document.validate().expect("history validates");
        assert_eq!(document.jobs[0].records.len(), MAX_SCHEDULER_HISTORY_LIMIT);
        // Oldest evicted, sequence still strictly increasing.
        assert_eq!(document.jobs[0].records[0].sequence, 3);
    }

    #[tokio::test]
    async fn configured_job_is_visible_before_its_first_run() {
        let job = job_config("upcoming", &["/bin/true"], None);
        let publisher = SchedulerPublisher::empty();
        let mut engine = Engine::new(
            std::slice::from_ref(&job),
            wall_time(),
            5,
            publisher.clone(),
        )
        .unwrap();
        engine.publish(wall_time(), None).await;
        let publication = publisher.current().await;
        let summary: SchedulerSummaryV2 =
            serde_json::from_slice(&publication.summary_bytes).expect("summary JSON");
        summary.validate().expect("summary validates");
        assert_eq!(summary.jobs.len(), 1);
        assert_eq!(summary.jobs[0].name, "upcoming");
        assert_eq!(summary.jobs[0].state, SchedulerJobStateV2::Idle);
        assert_eq!(summary.jobs[0].last, None);
        // Nothing ran, so there is no history to show.
        let history: SchedulerHistoryV2 =
            serde_json::from_slice(&publication.history_bytes).expect("history JSON");
        assert_eq!(history.jobs[0].records.len(), 0);
    }

    #[tokio::test]
    async fn an_unchanged_reconciliation_wake_publishes_nothing() {
        let job = job_config("idle", &["/bin/true"], None);
        let publisher = SchedulerPublisher::empty();
        let mut engine = Engine::new(
            std::slice::from_ref(&job),
            wall_time(),
            5,
            publisher.clone(),
        )
        .unwrap();
        engine.publish(wall_time(), None).await;
        let first = publisher.current().await.summary_bytes.clone();
        // The Plan-160 one-minute wake: civil time is re-read, state is not.
        for _ in 0..3 {
            engine.publish(wall_time(), None).await;
        }
        let after = publisher.current().await.summary_bytes.clone();
        assert_eq!(first, after, "an unchanged wake must not republish");
    }

    /// Publish the observer's current history and parse it back.
    ///
    /// Reads the real serialized publication, so these proofs exercise the
    /// production serializer and the frozen validation rather than a
    /// hand-written fixture.
    async fn published_history(
        observer: &mut SchedulerObserver,
        job_count: usize,
    ) -> SchedulerHistoryV2 {
        let jobs: Vec<gregg_protocol::SchedulerJobV2> = (0..job_count)
            .map(|index| gregg_protocol::SchedulerJobV2 {
                name: format!("job-{index}"),
                schedule: "* * * * *".to_owned(),
                next_due_unix_ms: 1_700_000_060_000,
                state: SchedulerJobStateV2::Idle,
                load: None,
                pending_since_unix_ms: None,
                next_retry_unix_ms: None,
                running_since_unix_ms: None,
                last: observer.last_summary(index),
            })
            .collect();
        observer.publish(jobs, 1_700_000_002_000).await;
        let publication = observer.publication_handle().current().await;
        serde_json::from_slice(&publication.history_bytes).expect("history JSON")
    }
}
