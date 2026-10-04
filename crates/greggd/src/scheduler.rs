//! Bounded, local-only maintenance scheduler.

pub(crate) mod schedule;

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use chrono::{DateTime, Local};
use gregg_protocol::{LoadAverage, ReadinessState};
use tokio::process::Command;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;

use crate::config::{ScheduledJobConfig, MAX_JOBS};
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
    retry_at: Instant,
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
}

#[derive(Debug)]
struct Engine<'a> {
    configs: &'a [ScheduledJobConfig],
    states: Vec<JobState>,
}

/// A selected job, identified by its configuration index so the direct child
/// can be spawned from the borrowed configuration before the next await.
#[derive(Debug)]
struct Launch {
    index: usize,
    pending_age: Duration,
    observed_load: Option<f32>,
    load_window: Option<&'static str>,
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
    fn new(configs: &'a [ScheduledJobConfig], wall_now: DateTime<Local>) -> Result<Self, String> {
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
            });
        }
        Ok(Self { configs, states })
    }

    fn tick(
        &mut self,
        wall_now: DateTime<Local>,
        now: Instant,
        load: LoadGateState,
        slot_available: bool,
    ) -> Result<Option<Launch>, String> {
        let configs = self.configs;
        for (index, state) in self.states.iter_mut().enumerate() {
            let config = &configs[index];
            if state.next_due <= wall_now {
                if let Some(pending) = &mut state.pending {
                    pending.coalesced = true;
                } else {
                    state.pending = Some(PendingOccurrence {
                        since: now,
                        retry_at: now,
                        coalesced: false,
                        waiting_logged: false,
                    });
                }
                // Configuration validation already rejects calendar-impossible
                // expressions, so a failure here is an internal time-domain
                // error. It reaches the existing scheduler fatal boundary
                // instead of fabricating a replacement schedule.
                state.next_due = state
                    .schedule
                    .next_after(&wall_now)
                    .map_err(|error| format!("job {:?}: {error}", config.name))?;
            }
            if state.pending.as_ref().is_some_and(|pending| {
                now.saturating_duration_since(pending.since)
                    >= Duration::from_millis(config.effective_max_wait_ms())
                    && config.max_load.is_some()
            }) {
                let pending = state.pending.take().expect("pending checked above");
                tracing::info!(
                    job = %config.name,
                    pending_age_ms = duration_millis(now.saturating_duration_since(pending.since)),
                    coalesced = pending.coalesced,
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
        for (blocked, state) in self.states.iter_mut().enumerate() {
            let Some(pending) = state.pending.as_ref() else {
                continue;
            };
            if pending.retry_at > now || (pending.since, blocked) >= (since, index) {
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
        }

        let pending = self.states[index]
            .pending
            .take()
            .expect("selected candidate is pending");
        let gate = load_gate(&configs[index], load);
        Ok(Some(Launch {
            index,
            pending_age: now.saturating_duration_since(pending.since),
            observed_load: gate.and_then(|(_, _, observed, _)| observed),
            load_window: gate.map(|(_, window, _, _)| window),
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
                    deadline = Some(
                        deadline.map_or(pending.retry_at, |current| current.min(pending.retry_at)),
                    );
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

struct RunningChild<'a> {
    child: tokio::process::Child,
    job_name: &'a str,
    started: Instant,
}

enum SchedulerWake {
    Child(Option<std::io::Result<ExitStatus>>),
    Deadline,
    Shutdown,
}

fn start_child(job: &ScheduledJobConfig) -> Result<RunningChild<'_>, String> {
    let Some(executable) = job.command.first() else {
        return Err("validated command has no executable".to_owned());
    };
    let mut command = Command::new(executable);
    command
        .args(&job.command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(working_dir) = &job.working_dir {
        command.current_dir(working_dir);
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    Ok(RunningChild {
        child,
        job_name: &job.name,
        started: Instant::now(),
    })
}

/// Run the daemon's configured job set until its shutdown signal arrives.
pub(crate) async fn run(
    jobs: Vec<ScheduledJobConfig>,
    load_rx: watch::Receiver<LoadGateState>,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), String> {
    let mut engine = Engine::new(&jobs, Local::now())?;
    let mut active: Option<RunningChild<'_>> = None;
    loop {
        let now = Instant::now();
        let wall_now = Local::now();
        if let Some(launch) = engine.tick(wall_now, now, *load_rx.borrow(), active.is_none())? {
            let job = &jobs[launch.index];
            match start_child(job) {
                Ok(child) => {
                    tracing::info!(
                        job = %job.name,
                        pending_age_ms = duration_millis(launch.pending_age),
                        load_window = launch.load_window,
                        observed_load = launch.observed_load,
                        "scheduled job started"
                    );
                    active = Some(child);
                }
                Err(error) => tracing::info!(job = %job.name, %error, "scheduled job completed"),
            }
            continue;
        }

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
                    Some(child) => Some(child.child.wait().await),
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
            SchedulerWake::Child(Some(Ok(status))) => {
                let child = active.take().expect("completed child remains active");
                let elapsed_ms = duration_millis(child.started.elapsed());
                let exit_code = status.code();
                #[cfg(unix)]
                let signal = {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal()
                };
                #[cfg(not(unix))]
                let signal: Option<i32> = None;
                tracing::info!(
                    job = %child.job_name,
                    exit_code,
                    signal,
                    elapsed_ms,
                    "scheduled job completed"
                );
            }
            SchedulerWake::Child(Some(Err(error))) => {
                let child = active.take().expect("completed child remains active");
                tracing::info!(job = %child.job_name, %error, "scheduled job completed");
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
                }
            })
            .collect();
        Self { configs, states }
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
            let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut engine = Engine::new(&configs, wall).unwrap();
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
        let mut engine = Engine::new(&configs, wall).unwrap();
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
        let mut engine = Engine::new(&configs, wall).unwrap();
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
        let mut engine = Engine::new(&configs, wall).unwrap();
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
        let mut engine = Engine::new(&configs, wall).unwrap();
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
        let due = engine.states[0].next_due;
        let launch = engine
            .tick(due, Instant::now(), LoadGateState::UNAVAILABLE, true)
            .unwrap()
            .unwrap();
        assert_eq!(launch.index, 0);
        assert_eq!(engine.pending_count(), 0);
        config.command[0] = "/greggd-test-does-not-exist".to_owned();
        assert!(start_child(&config).is_err());
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
        let error = run(vec![impossible], load_rx, shutdown_rx.resubscribe())
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
        let engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let engine = Engine::new(configs, wall).unwrap();
        assert_eq!(engine.pending_count(), 0);
        assert!(engine.states.is_empty());
    }

    #[test]
    fn forward_wall_jump_coalesces_to_one_pending_and_advances_next_due() {
        let wall = wall_time();
        let config = daily_job("daily");
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut engine = Engine::new(&configs, wall).unwrap();
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
        let mut engine = Engine::new(std::slice::from_ref(&config), wall).unwrap();
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
        let mut child = start_child(&job).unwrap();
        child.child.start_kill().unwrap();
        let result = tokio::time::timeout(CHILD_SHUTDOWN_BOUND, child.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(result.code().is_none());
    }
}
