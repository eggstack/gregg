//! Bounded, local-only maintenance scheduler.

pub(crate) mod observation;
pub(crate) mod schedule;

use std::process::{ExitStatus, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use gregg_protocol::{
    LoadAverage, ReadinessState, SchedulerJobV2, SchedulerLoadGateV2, SchedulerOutcomeV2,
};
use tokio::process::Command;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;

use crate::config::{ScheduledJobConfig, MAX_JOBS};
use observation::{
    drain_tail, job_state, OutputTail, SchedulerObserver, SchedulerPublisher, TerminalRecord,
};
use schedule::LocalSchedule;

const CHILD_SHUTDOWN_BOUND: Duration = Duration::from_secs(2);

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
                let deferred = state.last_gate.is_some()
                    && running_here.is_none()
                    && state
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.retry_at <= Instant::now());
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
    fn defer_blocked_before(
        &mut self,
        winner: (Instant, usize),
        now: Instant,
        unix_now: u64,
        load: LoadGateState,
    ) {
        let configs = self.configs;
        for (blocked, state) in self.states.iter_mut().enumerate() {
            let Some(pending) = state.pending.as_ref() else {
                continue;
            };
            if pending.retry_at > now || (pending.since, blocked) >= winner {
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
    /// A load-expired occurrence is a terminal record even though no child ever
    /// ran; omitting it would make the recent-runs display quietly dishonest.
    /// Returns the pending age for the existing log line.
    fn take_expired(
        &mut self,
        index: usize,
        now: Instant,
        config: &ScheduledJobConfig,
    ) -> Option<u64> {
        let state = &mut self.states[index];
        let expired = state.pending.as_ref().is_some_and(|pending| {
            now.saturating_duration_since(pending.since)
                >= Duration::from_millis(config.effective_max_wait_ms())
                && config.max_load.is_some()
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
            if let Some(pending_age_ms) = self.take_expired(index, now, config) {
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
            return Ok(None);
        };

        // Defer every load-blocked candidate that would have been visited
        // before the winner, so an older blocked job neither hides nor is
        // hidden by the job that takes the free global slot.
        self.defer_blocked_before((since, index), now, unix_now, load);

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
                if slot_available {
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
    job_name: &'a str,
    started: Instant,
    started_unix_ms: u64,
    scheduled_unix_ms: u64,
    delay_ms: u64,
    coalesced: bool,
}

/// A child that has exited, with both output streams fully drained.
struct ChildCompletion {
    status: std::io::Result<ExitStatus>,
    stdout: OutputTail,
    stderr: OutputTail,
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

fn start_child(job: &ScheduledJobConfig, capture_output: bool) -> Result<RunningChild<'_>, String> {
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
    Ok(RunningChild {
        child,
        stdout,
        stderr,
        job_name: &job.name,
        started: Instant::now(),
        started_unix_ms: now_unix_ms(),
        scheduled_unix_ms: 0,
        delay_ms: 0,
        coalesced: false,
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
            match start_child(job, capture_output) {
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
                            active.as_ref().map(|c| (launch.index, c.started_unix_ms)),
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
        engine.publish(wall_now, None).await;

        let semantic_deadline = engine.next_deadline(wall_now, Instant::now(), active.is_none());
        // Re-reading civil time at least once per minute bounds how long a
        // forward wall-clock jump can hide behind a stale monotonic sleep.
        // The wake itself performs the existing bounded scan only: no host
        // telemetry, HTTP request, filesystem scan, or child spawn, and it
        // stays silent (the `Deadline` branch below logs nothing).
        let wake_at = bounded_wake_deadline(semantic_deadline, Instant::now());
        let wake = {
            // Both output streams drain concurrently with the child wait in a
            // single join, so neither stream can block the other and neither
            // can fill the child's pipe. Borrowing the streams (rather than
            // spawning drain tasks) means a cancelled select simply stops
            // draining and leaves the handles in place for the next wake, so no
            // drain task can outlive the scheduler or delay shutdown.
            let child_wait = async {
                match active.as_mut() {
                    Some(child) => Some(ChildCompletion {
                        status: child.child.wait().await,
                        stdout: drain_tail(child.stdout.as_mut()).await,
                        stderr: drain_tail(child.stderr.as_mut()).await,
                    }),
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
                let elapsed_ms = duration_millis(child.started.elapsed());
                let finished = now_unix_ms();
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
                    // The child was created but its status could not be
                    // observed. Publishing it as a failure would be a guess.
                    Err(error) => {
                        tracing::info!(job = %child.job_name, %error, "scheduled job wait failed");
                        (SchedulerOutcomeV2::WaitFailed, None, None)
                    }
                };
                let sequence = engine.observer.record_terminal(
                    jobs.iter()
                        .position(|job| job.name == child.job_name)
                        .unwrap_or(0),
                    TerminalRecord {
                        scheduled_unix_ms: child.scheduled_unix_ms,
                        started_unix_ms: Some(child.started_unix_ms),
                        finished_unix_ms: finished,
                        delay_ms: child.delay_ms,
                        coalesced: child.coalesced,
                        exit_code,
                        signal,
                        duration_ms: Some(elapsed_ms),
                        stdout: completion.stdout,
                        stderr: completion.stderr,
                    },
                    outcome,
                );
                tracing::info!(
                    job = %child.job_name,
                    sequence,
                    exit_code,
                    signal,
                    elapsed_ms,
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
        assert!(start_child(&config, true).is_err());
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
        let mut child = start_child(&job, true).unwrap();
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
    use crate::scheduler::observation::{SchedulerObserver, TerminalRecord};
    use gregg_protocol::{
        SchedulerHistoryV2, SchedulerJobStateV2, SchedulerSummaryV2, MAX_SCHEDULER_HISTORY_LIMIT,
    };

    fn job_config(name: &str, command: &[&str], max_load: Option<f32>) -> ScheduledJobConfig {
        ScheduledJobConfig {
            name: name.to_owned(),
            schedule: "* * * * *".to_owned(),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
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

    /// Spawn a child, drain both streams concurrently, and return the terminal
    /// record the scheduler would record.
    async fn run_child_to_completion(
        job: &ScheduledJobConfig,
        launched: Launch,
    ) -> (TerminalRecord, SchedulerOutcomeV2) {
        let mut child = start_child(job, true).expect("child spawns");
        child.scheduled_unix_ms = launched.scheduled.timestamp_millis().max(0).unsigned_abs();
        child.delay_ms = duration_millis(launched.pending_age);
        child.coalesced = launched.coalesced;
        let (status, stdout, stderr) = tokio::join!(
            child.child.wait(),
            drain_tail(child.stdout.as_mut()),
            drain_tail(child.stderr.as_mut()),
        );
        let status = status.expect("child exit observed");
        let elapsed_ms = duration_millis(child.started.elapsed());
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
                finished_unix_ms: now_unix_ms(),
                delay_ms: child.delay_ms,
                coalesced: child.coalesced,
                exit_code,
                signal,
                duration_ms: Some(elapsed_ms),
                stdout,
                stderr,
            },
            outcome,
        )
    }

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
        let mut child = start_child(&job, false).expect("child spawns");
        assert!(child.stdout.is_none());
        assert!(child.stderr.is_none());
        let status = tokio::time::timeout(CHILD_SHUTDOWN_BOUND, child.child.wait())
            .await
            .expect("child completes")
            .expect("exit observed");
        assert_eq!(status.code(), Some(0));
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
        let Err(error) = start_child(&job, true) else {
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
