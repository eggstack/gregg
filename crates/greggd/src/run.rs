//! Foreground daemon entry point.
//!
//! Wires the native collector, periodic sampler, HTTP server, signal handling,
//! and structured logging into a single foreground process. Uses the validated
//! [`crate::config::Config`] for all runtime parameters.

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

/// Run the daemon with the given collector and configuration.
///
/// This is the main entry point for `greggd run`. It:
///
/// 1. Initializes structured logging.
/// 2. Validates configuration.
/// 3. Starts the periodic sampler and HTTP server.
/// 4. Handles Ctrl-C / SIGTERM for graceful shutdown.
/// 5. Logs the shutdown reason and exits cleanly.
///
/// # Errors
///
/// Returns an error if configuration is invalid or the server fails
/// to start.
#[allow(clippy::too_many_lines)]
pub async fn run<C: SystemCollector + 'static>(
    collector: C,
    config: Config,
) -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

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
        eprintln!("configuration error: {e}");
        std::process::exit(crate::cli::ExitCode::RuntimeError as i32);
    }

    let interval_ms = match Sampler::<C, RealClock>::validate_interval(config.sample_interval_ms())
    {
        Ok(ms) => ms,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(crate::cli::ExitCode::RuntimeError as i32);
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
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        eprintln!("failed to bind {addr}: {e}");
        Box::new(ServerError::Bind(e)) as Box<dyn std::error::Error>
    })?;

    // Spawn the sampler task.
    let sampler_handle = {
        let shutdown_rx = shutdown_tx.subscribe();
        let state = server_state.clone();
        let mut sampler = Sampler::with_interval(collector, RealClock, interval_ms)?;

        tokio::spawn(async move {
            sampler
                .run(shutdown_rx, |readiness, snap| {
                    let state = state.clone();
                    async move {
                        // Await state updates inline — no detached spawns.
                        // This ensures ordered state updates and that no
                        // update races with shutdown.
                        sync_sampler_state(&state, readiness, snap).await;
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
    // An unexpected clean exit (Ok) from either task without a shutdown signal
    // is treated as a failure — the server and sampler should only exit when
    // the shutdown signal is received.
    let mut server_handle = Some(server_handle);
    let mut sampler_handle = Some(sampler_handle);

    tokio::select! {
        signal_result = wait_for_shutdown_signal() => {
            info!(reason = %signal_result, "shutdown signal received");
        }
        result = async { server_handle.take().unwrap().await } => {
            match result {
                Ok(Ok(())) => {
                    // Server exited cleanly without a shutdown signal — unexpected.
                    eprintln!("HTTP server exited unexpectedly");
                    let _ = shutdown_tx.send(());
                    return Err("HTTP server exited unexpectedly".into());
                }
                Ok(Err(e)) => {
                    eprintln!("HTTP server error: {e}");
                    let _ = shutdown_tx.send(());
                    return Err(Box::new(e));
                }
                Err(e) => {
                    eprintln!("HTTP server task panicked: {e}");
                    let _ = shutdown_tx.send(());
                    return Err(Box::new(e));
                }
            }
        }
        result = async { sampler_handle.take().unwrap().await } => {
            match result {
                Ok(()) => {
                    // Sampler exited cleanly without a shutdown signal — unexpected.
                    eprintln!("sampler exited unexpectedly");
                    let _ = shutdown_tx.send(());
                    return Err("sampler exited unexpectedly".into());
                }
                Err(e) => {
                    eprintln!("sampler task panicked: {e}");
                    let _ = shutdown_tx.send(());
                    return Err(Box::new(e));
                }
            }
        }
    }

    // Notify remaining tasks to shut down.
    let _ = shutdown_tx.send(());

    // Joined shutdown: wait for both tasks to complete, with a timeout.
    join_tasks(server_handle, sampler_handle).await;

    info!("greggd stopped");
    Ok(())
}

/// Wait for both the server and sampler tasks to complete, with a timeout.
/// Logs the outcome of each task.
async fn join_tasks(
    server_handle: Option<tokio::task::JoinHandle<Result<(), ServerError>>>,
    sampler_handle: Option<tokio::task::JoinHandle<()>>,
) {
    let join_timeout = Duration::from_secs(10);

    if let Some(handle) = server_handle {
        match tokio::time::timeout(join_timeout, handle).await {
            Ok(Ok(Ok(()))) => info!("HTTP server shut down cleanly"),
            Ok(Ok(Err(e))) => eprintln!("HTTP server error during shutdown: {e}"),
            Ok(Err(e)) => eprintln!("HTTP server task panicked during shutdown: {e}"),
            Err(_) => eprintln!("HTTP server did not shut down within timeout"),
        }
    }

    if let Some(handle) = sampler_handle {
        match tokio::time::timeout(join_timeout, handle).await {
            Ok(Ok(())) => info!("sampler shut down cleanly"),
            Ok(Err(e)) => eprintln!("sampler error during shutdown: {e}"),
            Err(_) => eprintln!("sampler did not shut down within timeout"),
        }
    }
}

/// Sync sampler state to the shared [`ServerState`].
async fn sync_sampler_state(
    server_state: &ServerState,
    readiness: ReadinessState,
    snap: Option<Arc<gregg_protocol::StatusSnapshot>>,
) {
    match readiness {
        ReadinessState::Ready => {
            if let Some(snap) = snap {
                server_state.update_snapshot((*snap).clone()).await;
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
async fn wait_for_shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
        let mut sigint =
            signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => "SIGTERM",
            _ = sigint.recv() => "SIGINT",
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl-C");
        "Ctrl-C"
    }
}

/// Initialize structured logging from the `RUST_LOG` environment variable.
fn init_logging() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
