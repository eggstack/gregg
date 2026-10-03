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

#[derive(Debug)]
struct RuntimeJob {
    config: ScheduledJobConfig,
    schedule: LocalSchedule,
    next_due: DateTime<Local>,
    pending: Option<PendingOccurrence>,
}

#[derive(Debug)]
struct Engine {
    jobs: Vec<RuntimeJob>,
}

#[derive(Debug)]
struct Launch {
    job: ScheduledJobConfig,
    pending_age: Duration,
    observed_load: Option<f32>,
    load_window: Option<&'static str>,
}

impl Engine {
    fn new(configs: &[ScheduledJobConfig], wall_now: DateTime<Local>) -> Result<Self, String> {
        if configs.len() > MAX_JOBS {
            return Err(format!("scheduler has more than {MAX_JOBS} jobs"));
        }
        let mut jobs = Vec::with_capacity(configs.len());
        for config in configs {
            let schedule = LocalSchedule::parse(&config.schedule)?;
            let next_due = schedule.next_after(&wall_now)?;
            jobs.push(RuntimeJob {
                config: config.clone(),
                schedule,
                next_due,
                pending: None,
            });
        }
        Ok(Self { jobs })
    }

    fn tick(
        &mut self,
        wall_now: DateTime<Local>,
        now: Instant,
        load: LoadGateState,
        slot_available: bool,
    ) -> Option<Launch> {
        for runtime in &mut self.jobs {
            if runtime.next_due <= wall_now {
                if let Some(pending) = &mut runtime.pending {
                    pending.coalesced = true;
                } else {
                    runtime.pending = Some(PendingOccurrence {
                        since: now,
                        retry_at: now,
                        coalesced: false,
                        waiting_logged: false,
                    });
                }
                match runtime.schedule.next_after(&wall_now) {
                    Ok(next) => runtime.next_due = next,
                    Err(error) => {
                        tracing::error!(job = %runtime.config.name, %error, "unable to calculate next scheduled occurrence");
                        runtime.next_due = wall_now + chrono::Duration::days(366);
                    }
                }
            }
            if runtime.pending.as_ref().is_some_and(|pending| {
                now.saturating_duration_since(pending.since)
                    >= Duration::from_millis(runtime.config.effective_max_wait_ms())
                    && runtime.config.max_load.is_some()
            }) {
                let pending = runtime.pending.take().expect("pending checked above");
                tracing::info!(
                    job = %runtime.config.name,
                    pending_age_ms = duration_millis(now.saturating_duration_since(pending.since)),
                    coalesced = pending.coalesced,
                    "scheduled job expired waiting for load"
                );
            }
        }

        if !slot_available {
            return None;
        }

        let mut candidates = self
            .jobs
            .iter()
            .enumerate()
            .filter_map(|(index, runtime)| {
                let pending = runtime.pending.as_ref()?;
                (pending.retry_at <= now).then_some((index, pending.since))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(index, since)| (*since, *index));
        for (candidate, _) in candidates {
            let runtime = &mut self.jobs[candidate];
            let pending = runtime.pending.as_mut().expect("candidate is pending");
            let selected_load = runtime.config.max_load.map(|_| {
                let window = runtime.config.effective_load_window();
                let value = match (load.readiness, load.load) {
                    (ReadinessState::Ready, Some(values)) => Some(match window {
                        "1m" => values.one,
                        "5m" => values.five,
                        _ => values.fifteen,
                    }),
                    _ => None,
                };
                (window, value)
            });
            if let (Some(threshold), Some((window, observed))) =
                (runtime.config.max_load, selected_load)
            {
                let allowed = observed.is_some_and(|value| value.is_finite() && value <= threshold);
                if !allowed {
                    if !pending.waiting_logged {
                        tracing::info!(
                            job = %runtime.config.name,
                            load_window = window,
                            observed_load = observed,
                            max_load = threshold,
                            pending_age_ms = duration_millis(now.saturating_duration_since(pending.since)),
                            "scheduled job pending: load gate"
                        );
                        pending.waiting_logged = true;
                    }
                    pending.retry_at =
                        now + Duration::from_millis(runtime.config.effective_retry_interval_ms());
                    continue;
                }
            }

            let pending = runtime.pending.take().expect("candidate is pending");
            return Some(Launch {
                job: runtime.config.clone(),
                pending_age: now.saturating_duration_since(pending.since),
                observed_load: selected_load.and_then(|(_, value)| value),
                load_window: selected_load.map(|(window, _)| window),
            });
        }
        None
    }

    fn next_deadline(
        &self,
        wall_now: DateTime<Local>,
        now: Instant,
        slot_available: bool,
    ) -> Instant {
        let mut deadline = None;
        for runtime in &self.jobs {
            let until_due = (runtime.next_due - wall_now)
                .to_std()
                .unwrap_or(Duration::ZERO);
            let candidate = now + until_due;
            deadline = Some(deadline.map_or(candidate, |current: Instant| current.min(candidate)));
            if let Some(pending) = &runtime.pending {
                if slot_available {
                    deadline = Some(
                        deadline.map_or(pending.retry_at, |current| current.min(pending.retry_at)),
                    );
                }
                if runtime.config.max_load.is_some() {
                    let expiry = pending.since
                        + Duration::from_millis(runtime.config.effective_max_wait_ms());
                    deadline = Some(deadline.map_or(expiry, |current| current.min(expiry)));
                }
            }
        }
        deadline.expect("a scheduler is started only with at least one job")
    }

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.jobs.iter().filter(|job| job.pending.is_some()).count()
    }
}

struct RunningChild {
    child: tokio::process::Child,
    job_name: String,
    started: Instant,
}

enum SchedulerWake {
    Child(Option<std::io::Result<ExitStatus>>),
    Deadline,
    Shutdown,
}

fn start_child(launch: &Launch) -> Result<RunningChild, String> {
    let Some(executable) = launch.job.command.first() else {
        return Err("validated command has no executable".to_owned());
    };
    let mut command = Command::new(executable);
    command
        .args(&launch.job.command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(working_dir) = &launch.job.working_dir {
        command.current_dir(working_dir);
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    Ok(RunningChild {
        child,
        job_name: launch.job.name.clone(),
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
    let mut active: Option<RunningChild> = None;
    loop {
        let now = Instant::now();
        let wall_now = Local::now();
        if let Some(launch) = engine.tick(wall_now, now, *load_rx.borrow(), active.is_none()) {
            match start_child(&launch) {
                Ok(child) => {
                    tracing::info!(
                        job = %launch.job.name,
                        pending_age_ms = duration_millis(launch.pending_age),
                        load_window = launch.load_window,
                        observed_load = launch.observed_load,
                        "scheduled job started"
                    );
                    active = Some(child);
                }
                Err(error) => tracing::info!(
                    job = %launch.job.name,
                    %error,
                    "scheduled job completed"
                ),
            }
            continue;
        }

        let deadline = engine.next_deadline(wall_now, Instant::now(), active.is_none());
        let wake = {
            let child_wait = async {
                match active.as_mut() {
                    Some(child) => Some(child.child.wait().await),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                result = child_wait => SchedulerWake::Child(result),
                () = tokio::time::sleep_until(deadline) => SchedulerWake::Deadline,
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
        assert!(engine.jobs[0].next_due > wall);
        let due = engine.jobs[0].next_due;
        let launch = engine
            .tick(
                due,
                Instant::now() + Duration::from_secs(60),
                LoadGateState::UNAVAILABLE,
                true,
            )
            .unwrap();
        assert_eq!(launch.job.command, ["/bin/true", "--flag"]);
    }

    #[test]
    fn deferral_is_bounded_coalesced_and_expires_once() {
        let wall = wall_time();
        let mut config = job("heavy", Some(1.0));
        config.retry_interval_ms = Some(10_000);
        config.max_wait_ms = Some(120_000);
        let mut engine = Engine::new(&[config], wall).unwrap();
        let since = Instant::now();
        let due = engine.jobs[0].next_due;
        assert!(engine
            .tick(due, since, LoadGateState::UNAVAILABLE, false)
            .is_none());
        let retry_due = engine.jobs[0].next_due;
        let retry_wall = retry_due;
        assert!(engine
            .tick(
                retry_wall,
                since + Duration::from_secs(60),
                LoadGateState::UNAVAILABLE,
                false,
            )
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        assert_eq!(engine.jobs[0].pending.as_ref().unwrap().since, since);
        assert!(engine.jobs[0].pending.as_ref().unwrap().coalesced);
        assert!(engine
            .tick(
                retry_wall,
                since + Duration::from_secs(120),
                LoadGateState::UNAVAILABLE,
                false,
            )
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
            let mut engine = Engine::new(&[config], wall).unwrap();
            let due = engine.jobs[0].next_due;
            let launch = engine
                .tick(
                    due,
                    Instant::now(),
                    ready(values.0, values.1, values.2),
                    true,
                )
                .unwrap();
            assert_eq!(launch.observed_load, Some(expected));
        }

        let mut config = job("high", Some(8.0));
        config.retry_interval_ms = Some(10_000);
        let mut engine = Engine::new(&[config], wall).unwrap();
        let due = engine.jobs[0].next_due;
        let since = Instant::now();
        assert!(engine
            .tick(due, since, ready(8.01, 8.01, 8.01), true)
            .is_none());
        let retry_at = engine.jobs[0].pending.as_ref().unwrap().retry_at;
        let launch = engine
            .tick(due, retry_at, ready(8.0, 8.0, 8.0), true)
            .unwrap();
        assert_eq!(launch.observed_load, Some(8.0));
    }

    #[test]
    fn global_slot_and_oldest_stable_selection_prevent_herding() {
        let wall = wall_time();
        let configs = [job("first", None), job("second", None), job("third", None)];
        let mut engine = Engine::new(&configs, wall).unwrap();
        let due = engine.jobs[0].next_due;
        let mono = Instant::now();
        let first = engine
            .tick(due, mono, LoadGateState::UNAVAILABLE, true)
            .unwrap();
        assert_eq!(first.job.name, "first");
        assert!(engine
            .tick(due, mono, LoadGateState::UNAVAILABLE, false)
            .is_none());
        assert_eq!(engine.pending_count(), 2);
        assert!(engine.next_deadline(due, mono, false) > mono);
        let second = engine
            .tick(due, mono, LoadGateState::UNAVAILABLE, true)
            .unwrap();
        assert_eq!(second.job.name, "second");
    }

    #[test]
    fn high_load_candidate_does_not_block_time_only_job() {
        let wall = wall_time();
        let mut heavy = job("heavy", Some(0.0));
        heavy.retry_interval_ms = Some(10_000);
        let configs = [heavy, job("time-only", None)];
        let mut engine = Engine::new(&configs, wall).unwrap();
        let due = engine.jobs[0].next_due;
        let launch = engine
            .tick(due, Instant::now(), ready(1.0, 1.0, 1.0), true)
            .unwrap();
        assert_eq!(launch.job.name, "time-only");
    }

    #[test]
    fn load_is_rechecked_after_a_previous_child_releases_the_slot() {
        let wall = wall_time();
        let configs = [job("first", Some(8.0)), job("second", Some(8.0))];
        let mut engine = Engine::new(&configs, wall).unwrap();
        let due = engine.jobs[0].next_due;
        let mono = Instant::now();
        let first = engine.tick(due, mono, ready(7.9, 7.9, 7.9), true).unwrap();
        assert_eq!(first.job.name, "first");
        assert!(engine
            .tick(due, mono, ready(7.9, 7.9, 7.9), false)
            .is_none());
        assert_eq!(engine.pending_count(), 1);
        assert!(engine.tick(due, mono, ready(9.0, 9.0, 9.0), true).is_none());
        assert_eq!(engine.pending_count(), 1);
        let retry = engine.jobs[1].pending.as_ref().unwrap().retry_at;
        let second = engine.tick(due, retry, ready(8.0, 8.0, 8.0), true).unwrap();
        assert_eq!(second.job.name, "second");
    }

    #[tokio::test]
    async fn spawn_failure_is_terminal_for_the_occurrence() {
        let wall = wall_time();
        let mut engine = Engine::new(&[job("bad-exe", None)], wall).unwrap();
        let due = engine.jobs[0].next_due;
        let mut launch = engine
            .tick(due, Instant::now(), LoadGateState::UNAVAILABLE, true)
            .unwrap();
        launch.job.command[0] = "/greggd-test-does-not-exist".to_owned();
        assert!(start_child(&launch).is_err());
        assert_eq!(engine.pending_count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn active_direct_child_is_terminated_and_reaped_on_shutdown() {
        let launch = Launch {
            job: ScheduledJobConfig {
                name: "long-test-child".to_owned(),
                schedule: "* * * * *".to_owned(),
                command: vec!["/bin/sleep".to_owned(), "30".to_owned()],
                working_dir: None,
                max_load: None,
                load_window: None,
                retry_interval_ms: None,
                max_wait_ms: None,
            },
            pending_age: Duration::ZERO,
            observed_load: None,
            load_window: None,
        };
        let mut child = start_child(&launch).unwrap();
        child.child.start_kill().unwrap();
        let result = tokio::time::timeout(CHILD_SHUTDOWN_BOUND, child.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(result.code().is_none());
    }
}
