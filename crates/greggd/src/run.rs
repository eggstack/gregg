//! Foreground daemon entry point.
//!
//! Wires the native collector, periodic sampler, HTTP server, signal handling,
//! and structured logging into a single foreground process. Uses the validated
//! [`crate::config::Config`] for all runtime parameters.
//!
//! ## Supervision model
//!
//! The daemon runs two critical tasks: the HTTP server and the periodic
//! sampler. A `tokio::select!` observes whichever completes first (or a
//! shutdown signal) and produces an outcome value. After the select, a
//! single bounded deadline governs cleanup of any still-running task. If the
//! deadline expires, the remaining task is aborted. The original outcome is
//! preserved as the daemon's exit result regardless of cleanup failures.

use std::sync::Arc;
use std::time::Duration;

use gregg_protocol::{ReadinessState, SCHEMA_VERSION_V1};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::info;

use crate::collector::SystemCollector;
use crate::config::Config;
use crate::sampler::{RealClock, Sampler};
use crate::server::error::ServerError;
use crate::server::{Config as ServerConfig, ServerState};

/// Single deadline for joining remaining tasks after the select fires.
/// Using one deadline (rather than per-task timeouts) prevents the total
/// cleanup window from multiplying when multiple tasks are still running.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

/// Outcome of the supervision `select!`.
///
/// Each variant captures the result of the branch that fired. After the
/// select, [`RunOutcome::into_result`] converts this into the daemon's
/// exit result, while [`join_remaining_tasks`] cleans up any surviving
/// task within [`SHUTDOWN_DEADLINE`].
#[derive(Debug)]
pub(crate) enum RunOutcome {
    /// A shutdown signal was received — normal exit.
    // The signal reason is retained for tracing/debugging even though normal
    // shutdown classification does not inspect it.
    #[allow(dead_code)]
    Signal(&'static str),
    /// The HTTP server task completed (or panicked).
    Server(Result<Result<(), ServerError>, tokio::task::JoinError>),
    /// The sampler task completed (or panicked).
    Sampler(Result<(), tokio::task::JoinError>),
}

impl RunOutcome {
    /// Convert the outcome into the daemon's exit result.
    ///
    /// A signal yields success. An unexpected clean exit (the task returned
    /// `Ok` without a shutdown signal) is treated as a failure — the server
    /// and sampler should only exit when the shutdown signal is received.
    /// Errors and panics are propagated as failures.
    #[allow(clippy::match_same_arms)]
    fn into_result(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Signal(_) => Ok(()),
            Self::Server(Ok(Ok(()))) => Err("HTTP server exited unexpectedly".into()),
            Self::Server(Ok(Err(e))) => Err(Box::new(e)),
            Self::Server(Err(e)) => Err(Box::new(e)),
            Self::Sampler(Ok(())) => Err("sampler exited unexpectedly".into()),
            Self::Sampler(Err(e)) => Err(Box::new(e)),
        }
    }
}

/// Run the daemon with the given collector and configuration.
///
/// This is the main entry point for `greggd run`. It uses the
/// platform-default shutdown signal (SIGTERM/SIGINT on Unix, Ctrl-C
/// otherwise).
///
/// # Errors
///
/// Returns an error if configuration is invalid or the server fails
/// to start.
pub async fn run<C: SystemCollector + 'static>(
    collector: C,
    config: Config,
) -> Result<(), Box<dyn std::error::Error>> {
    run_with_shutdown(collector, config, wait_for_shutdown_signal()).await
}

/// Run the daemon with an externally provided shutdown source.
///
/// This is the shared core used by both foreground and Windows service
/// modes. The `shutdown` future completes when the daemon should stop:
/// a signal on Unix, or an SCM control handler on Windows.
///
/// # Errors
///
/// Returns an error if configuration is invalid or the server fails
/// to start.
#[allow(clippy::too_many_lines)]
pub async fn run_with_shutdown<C, S>(
    collector: C,
    config: Config,
    shutdown: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    C: SystemCollector + 'static,
    S: std::future::Future<Output = &'static str>,
{
    info!(
        version = env!("CARGO_PKG_VERSION"),
        schema_version = SCHEMA_VERSION_V1,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "greggd starting"
    );

    let server_config = ServerConfig {
        host: config.host(),
        port: config.port(),
        sample_interval_ms: config.sample_interval_ms(),
        ..ServerConfig::default()
    };
    if let Err(e) = server_config.validate() {
        return Err(Box::new(e));
    }

    let interval_ms = match Sampler::<C, RealClock>::validate_interval(config.sample_interval_ms())
    {
        Ok(ms) => ms,
        Err(e) => {
            return Err(Box::new(e));
        }
    };

    info!(
        listen_addr = %server_config.socket_addr(),
        sample_interval_ms = interval_ms,
        stale_after_ms = config.stale_after_ms(),
        "effective configuration"
    );

    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Wire stale_after_ms from daemon config into the server state.
    let server_state =
        ServerState::with_stale_policy(0, Duration::from_millis(config.stale_after_ms()));

    // Bind the TCP listener before spawning tasks so bind failures
    // are surfaced immediately rather than silently lost.
    let addr = server_config.socket_addr();
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| Box::new(ServerError::Bind(e)) as Box<dyn std::error::Error>)?;

    // Spawn the sampler task.
    let sampler_handle = {
        let shutdown_rx = shutdown_tx.subscribe();
        let state = server_state.clone();
        let mut sampler = Sampler::with_interval(collector, RealClock, interval_ms)?;

        tokio::spawn(async move {
            sampler
                .run(shutdown_rx, |readiness, snap, snap_v2| {
                    let state = state.clone();
                    async move {
                        sync_sampler_state(&state, readiness, snap, snap_v2).await;
                    }
                })
                .await;
        })
    };

    // Spawn the HTTP server task with the pre-bound listener.
    let server_handle = {
        let shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(crate::server::serve(listener, server_state, shutdown_rx))
    };

    // Supervise: wait for shutdown signal, server failure, or sampler failure.
    // The select produces an outcome; common cleanup runs after the select
    // so no critical-task branch can bypass task joining.
    let mut server_handle = Some(server_handle);
    let mut sampler_handle = Some(sampler_handle);

    let outcome = supervise(&mut server_handle, &mut sampler_handle, shutdown).await;

    // Notify remaining tasks to shut down.
    let _ = shutdown_tx.send(());

    // Join surviving tasks within a single bounded deadline.
    join_remaining_tasks(server_handle, sampler_handle).await;

    info!("greggd stopped");
    outcome.into_result()
}

/// Supervise the server and sampler tasks.
///
/// Uses `tokio::select!` to wait for whichever of the three branches
/// completes first: a shutdown signal, the server task, or the sampler
/// task. The selected handle is taken out of its `Option` after the select
/// so it is not joined twice by [`join_remaining_tasks`].
///
/// The `shutdown` future allows tests to inject a controlled signal
/// (e.g. `std::future::pending()` to never fire, or an immediately-ready
/// future to simulate a signal).
pub(crate) async fn supervise<S>(
    server_handle: &mut Option<tokio::task::JoinHandle<Result<(), ServerError>>>,
    sampler_handle: &mut Option<tokio::task::JoinHandle<()>>,
    shutdown: S,
) -> RunOutcome
where
    S: std::future::Future<Output = &'static str>,
{
    // Borrow the handles without taking them so that a non-selected branch
    // does not consume its handle. The selected handle is taken after the
    // select completes.
    let outcome = {
        let mut server_fut = server_handle.as_mut().unwrap();
        let mut sampler_fut = sampler_handle.as_mut().unwrap();

        tokio::select! {
            signal_result = shutdown => {
                info!(reason = %signal_result, "shutdown signal received");
                RunOutcome::Signal(signal_result)
            }
            result = &mut server_fut => {
                match result {
                    Ok(Ok(())) => {
                        // Server exited cleanly without a shutdown signal — unexpected.
                        RunOutcome::Server(Ok(Ok(())))
                    }
                    Ok(Err(e)) => {
                        RunOutcome::Server(Ok(Err(e)))
                    }
                    Err(e) => {
                        RunOutcome::Server(Err(e))
                    }
                }
            }
            result = &mut sampler_fut => {
                match result {
                    Ok(()) => {
                        // Sampler exited cleanly without a shutdown signal — unexpected.
                        RunOutcome::Sampler(Ok(()))
                    }
                    Err(e) => {
                        RunOutcome::Sampler(Err(e))
                    }
                }
            }
        }
    };

    // Take the selected handle so it is not joined again by
    // join_remaining_tasks. Non-selected handles remain in their Options.
    match &outcome {
        RunOutcome::Server(_) => {
            let _ = server_handle.take();
        }
        RunOutcome::Sampler(_) => {
            let _ = sampler_handle.take();
        }
        RunOutcome::Signal(_) => {}
    }

    outcome
}

/// Join any still-running critical tasks within a single bounded deadline.
///
/// After the `select!` fires, one handle has already been consumed (taken
/// out of its `Option`). This function joins the remaining handle(s).
/// If a task does not complete within `deadline`, it is aborted
/// and its cancellation is awaited.
///
/// Cleanup failures are logged but do not override the original outcome.
pub(crate) async fn join_remaining_tasks(
    server_handle: Option<tokio::task::JoinHandle<Result<(), ServerError>>>,
    sampler_handle: Option<tokio::task::JoinHandle<()>>,
) {
    join_remaining_tasks_with_deadline(server_handle, sampler_handle, SHUTDOWN_DEADLINE).await;
}

/// Like [`join_remaining_tasks`] but with an explicit deadline, for testing.
pub(crate) async fn join_remaining_tasks_with_deadline(
    server_handle: Option<tokio::task::JoinHandle<Result<(), ServerError>>>,
    sampler_handle: Option<tokio::task::JoinHandle<()>>,
    deadline: Duration,
) {
    let deadline_instant = tokio::time::Instant::now() + deadline;

    if let Some(mut handle) = server_handle {
        tokio::select! {
            result = &mut handle => {
                match result {
                    Ok(Ok(())) => info!("HTTP server shut down cleanly"),
                    Ok(Err(e)) => eprintln!("HTTP server error during shutdown: {e}"),
                    Err(e) => eprintln!("HTTP server task panicked during shutdown: {e}"),
                }
            }
            () = tokio::time::sleep_until(deadline_instant) => {
                eprintln!("HTTP server did not shut down within deadline; aborting");
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    if let Some(mut handle) = sampler_handle {
        tokio::select! {
            result = &mut handle => {
                match result {
                    Ok(()) => info!("sampler shut down cleanly"),
                    Err(e) => eprintln!("sampler error during shutdown: {e}"),
                }
            }
            () = tokio::time::sleep_until(deadline_instant) => {
                eprintln!("sampler did not shut down within deadline; aborting");
                handle.abort();
                let _ = handle.await;
            }
        }
    }
}

/// Sync sampler state to the shared [`ServerState`].
async fn sync_sampler_state(
    server_state: &ServerState,
    readiness: ReadinessState,
    snap: Option<Arc<gregg_protocol::StatusSnapshot>>,
    snap_v2: Option<Arc<gregg_protocol::v2::StatusPayloadV2>>,
) {
    match readiness {
        ReadinessState::Ready => {
            match (snap, snap_v2) {
                (Some(snap), Some(snap_v2)) => {
                    // Standard path: both v1 and v2 available.
                    server_state
                        .update_snapshot((*snap).clone(), (*snap_v2).clone())
                        .await;
                }
                (None, Some(snap_v2)) => {
                    // Windows path: v2 only, no v1 snapshot.
                    server_state
                        .update_snapshot_v2_only((*snap_v2).clone())
                        .await;
                }
                (Some(snap), None) => {
                    // Fallback: v1 only (should not happen in normal operation).
                    server_state.update_snapshot_v1_only((*snap).clone()).await;
                }
                (None, None) => {
                    // Both absent — should not happen in Ready state.
                }
            }
        }
        ReadinessState::Warming => {
            server_state.set_warming().await;
        }
        ReadinessState::Failed => {
            let msg = "collector failure";
            server_state.set_failed(msg).await;
        }
    }
}

/// Wait for a platform-appropriate shutdown signal.
fn wait_for_shutdown_signal() -> impl std::future::Future<Output = &'static str> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        async move {
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => "SIGTERM",
                _ = sigint.recv() => "SIGINT",
            }
        }
    }
    #[cfg(not(unix))]
    {
        async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for Ctrl-C");
            "Ctrl-C"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Spawn a server-like task that returns the given result.
    fn spawn_server(
        result: Result<(), ServerError>,
    ) -> tokio::task::JoinHandle<Result<(), ServerError>> {
        tokio::spawn(async move { result })
    }

    /// Spawn a server-like task that sleeps briefly before returning.
    fn spawn_server_slow(
        result: Result<(), ServerError>,
    ) -> tokio::task::JoinHandle<Result<(), ServerError>> {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            result
        })
    }

    /// Spawn a sampler-like task that returns Ok.
    fn spawn_sampler_ok() -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }

    /// Spawn a sampler-like task that sleeps briefly before returning.
    fn spawn_sampler_slow() -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
    }

    /// Spawn a sampler-like task that panics.
    fn spawn_sampler_panic() -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {
            panic!("sampler panic");
        })
    }

    /// Spawn a server-like task that panics.
    fn spawn_server_panic() -> tokio::task::JoinHandle<Result<(), ServerError>> {
        tokio::spawn(async {
            panic!("server panic");
        })
    }

    /// Spawn a server-like task that never completes (non-cooperative).
    fn spawn_server_never() -> tokio::task::JoinHandle<Result<(), ServerError>> {
        tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        })
    }

    #[tokio::test]
    async fn server_error_shuts_down_and_joins_sampler() {
        let mut server = Some(spawn_server(Err(ServerError::Bind(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "test",
        )))));
        let mut sampler = Some(spawn_sampler_slow());

        let outcome = supervise(&mut server, &mut sampler, std::future::pending()).await;

        // Server should have been taken (consumed).
        assert!(server.is_none());
        // Sampler should still be present (not yet joined).
        assert!(sampler.is_some());

        // The outcome should be a server error.
        match &outcome {
            RunOutcome::Server(Ok(Err(_))) => {}
            other => panic!("expected Server error, got {other:?}"),
        }

        // Join remaining tasks.
        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn server_panic_shuts_down_and_joins_sampler() {
        let mut server = Some(spawn_server_panic());
        let mut sampler = Some(spawn_sampler_slow());

        let outcome = supervise(&mut server, &mut sampler, std::future::pending()).await;

        assert!(server.is_none());
        assert!(sampler.is_some());

        match &outcome {
            RunOutcome::Server(Err(_)) => {}
            other => panic!("expected Server panic, got {other:?}"),
        }

        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn unexpected_clean_server_exit_is_failure_and_joins_sampler() {
        let mut server = Some(spawn_server(Ok(())));
        let mut sampler = Some(spawn_sampler_slow());

        let outcome = supervise(&mut server, &mut sampler, std::future::pending()).await;

        assert!(server.is_none());
        assert!(sampler.is_some());

        match &outcome {
            RunOutcome::Server(Ok(Ok(()))) => {}
            other => panic!("expected Server Ok(Ok(())), got {other:?}"),
        }

        // The outcome should be a failure.
        assert!(outcome.into_result().is_err());

        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn sampler_panic_shuts_down_and_joins_server() {
        let mut server = Some(spawn_server_slow(Ok(())));
        let mut sampler = Some(spawn_sampler_panic());

        let outcome = supervise(&mut server, &mut sampler, std::future::pending()).await;

        assert!(sampler.is_none());
        assert!(server.is_some());

        match &outcome {
            RunOutcome::Sampler(Err(_)) => {}
            other => panic!("expected Sampler panic, got {other:?}"),
        }

        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn unexpected_clean_sampler_exit_is_failure_and_joins_server() {
        let mut server = Some(spawn_server_slow(Ok(())));
        let mut sampler = Some(spawn_sampler_ok());

        let outcome = supervise(&mut server, &mut sampler, std::future::pending()).await;

        assert!(sampler.is_none());
        assert!(server.is_some());

        match &outcome {
            RunOutcome::Sampler(Ok(())) => {}
            other => panic!("expected Sampler Ok(()), got {other:?}"),
        }

        // The outcome should be a failure.
        assert!(outcome.into_result().is_err());

        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn signal_shutdown_joins_both_tasks_and_returns_success() {
        let mut server = Some(spawn_server(Ok(())));
        let mut sampler = Some(spawn_sampler_ok());

        // Use an immediately-ready shutdown future.
        let shutdown = async { "test-signal" };

        let outcome = supervise(&mut server, &mut sampler, shutdown).await;

        // Both handles should still be present (neither was selected).
        assert!(server.is_some());
        assert!(sampler.is_some());

        match &outcome {
            RunOutcome::Signal(s) => assert_eq!(*s, "test-signal"),
            other => panic!("expected Signal, got {other:?}"),
        }

        // The outcome should be success.
        assert!(outcome.into_result().is_ok());

        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn non_cooperative_task_is_aborted_after_deadline() {
        // Create a server task that never completes.
        let mut server = Some(spawn_server_never());
        let mut sampler = Some(spawn_sampler_ok());

        // Use an immediately-ready shutdown future so the select fires
        // on the signal branch, leaving both tasks running.
        let shutdown = async { "test-signal" };

        let outcome = supervise(&mut server, &mut sampler, shutdown).await;

        // Both handles should still be present.
        assert!(server.is_some());
        assert!(sampler.is_some());

        // join_remaining_tasks should abort the non-cooperative server
        // after a short deadline.
        join_remaining_tasks_with_deadline(server, sampler, Duration::from_millis(100)).await;

        // Outcome should be success (signal).
        assert!(outcome.into_result().is_ok());
    }

    #[tokio::test]
    async fn original_error_preserved_after_cleanup() {
        let server_error = ServerError::Bind(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "original error",
        ));
        let mut server = Some(spawn_server(Err(server_error)));
        let mut sampler = Some(spawn_sampler_slow());

        let outcome = supervise(&mut server, &mut sampler, std::future::pending()).await;

        // The outcome should be the original server error.
        let result = outcome.into_result();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("AddrInUse") || err.contains("bind") || err.contains("Bind"),
            "expected original error in: {err}"
        );

        join_remaining_tasks(server, sampler).await;
    }

    #[tokio::test]
    async fn join_remaining_tasks_handles_none() {
        // Verify join_remaining_tasks handles None gracefully.
        let none: Option<tokio::task::JoinHandle<Result<(), ServerError>>> = None;
        let none2: Option<tokio::task::JoinHandle<()>> = None;
        join_remaining_tasks(none, none2).await;

        // Also verify it handles a completed task.
        let done = Some(spawn_server(Ok(())));
        let done2: Option<tokio::task::JoinHandle<()>> = None;
        join_remaining_tasks(done, done2).await;
    }

    #[test]
    fn run_outcome_signal_is_success() {
        let outcome = RunOutcome::Signal("SIGTERM");
        assert!(outcome.into_result().is_ok());
    }

    #[test]
    fn run_outcome_server_error_is_failure() {
        let outcome = RunOutcome::Server(Ok(Err(ServerError::Bind(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "test",
        )))));
        assert!(outcome.into_result().is_err());
    }

    #[test]
    fn run_outcome_server_clean_exit_is_failure() {
        let outcome = RunOutcome::Server(Ok(Ok(())));
        assert!(outcome.into_result().is_err());
    }

    #[tokio::test]
    async fn run_outcome_server_panic_is_failure() {
        let join_error = spawn_server_panic().await.unwrap_err();
        let outcome = RunOutcome::Server(Err(join_error));
        assert!(outcome.into_result().is_err());
    }

    #[test]
    fn run_outcome_sampler_clean_exit_is_failure() {
        let outcome = RunOutcome::Sampler(Ok(()));
        assert!(outcome.into_result().is_err());
    }

    #[tokio::test]
    async fn run_outcome_sampler_panic_is_failure() {
        let join_error = spawn_sampler_panic().await.unwrap_err();
        let outcome = RunOutcome::Sampler(Err(join_error));
        assert!(outcome.into_result().is_err());
    }

    #[test]
    fn shutdown_deadline_is_bounded() {
        assert!(SHUTDOWN_DEADLINE >= Duration::from_secs(5));
        assert!(SHUTDOWN_DEADLINE <= Duration::from_secs(30));
    }
}
