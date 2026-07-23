//! Foreground daemon entry point.
//!
//! Wires the native collector, periodic sampler, HTTP server, signal handling,
//! and structured logging into a single foreground process. Uses the validated
//! [`crate::config::Config`] for all runtime parameters.

use std::sync::Arc;

use gregg_protocol::{ReadinessState, SCHEMA_VERSION_V1};
use tokio::sync::broadcast;
use tracing::info;

use crate::collector::SystemCollector;
use crate::config::Config;
use crate::sampler::{RealClock, Sampler};
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
        "effective configuration"
    );

    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let server_state = ServerState::new();

    // Spawn the sampler task.
    let sampler_handle = {
        let shutdown_rx = shutdown_tx.subscribe();
        let state = server_state.clone();
        let mut sampler = Sampler::with_interval(collector, RealClock, interval_ms)?;

        tokio::spawn(async move {
            sampler
                .run(shutdown_rx, |readiness, snap| {
                    let state = state.clone();
                    tokio::spawn(async move {
                        sync_sampler_state(&state, readiness, snap).await;
                    });
                })
                .await;
        })
    };

    // Spawn the HTTP server task.
    let server_handle = {
        let shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(crate::server::serve(
            server_config,
            server_state,
            shutdown_rx,
        ))
    };

    // Wait for Ctrl-C or SIGTERM.
    let signal_result = wait_for_shutdown_signal().await;
    info!(reason = %signal_result, "shutdown signal received");

    // Notify all tasks to shut down.
    let _ = shutdown_tx.send(());

    // Wait for both tasks to finish.
    let _ = tokio::join!(sampler_handle, server_handle);

    info!("greggd stopped");
    Ok(())
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
