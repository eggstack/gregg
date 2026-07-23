//! Foreground daemon entry point.
//!
//! Wires the native collector, periodic sampler, HTTP server, signal handling,
//! and structured logging into a single foreground process. Phase 5 will add
//! config discovery and service-manager commands around this entry point.

use std::net::IpAddr;

use gregg_protocol::SCHEMA_VERSION_V1;
use tokio::sync::broadcast;
use tracing::info;

use crate::collector::SystemCollector;
use crate::sampler::{Clock, RealClock, Sampler};
use crate::server::{Config as ServerConfig, ServerState};

/// Daemon run configuration.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Address to bind the HTTP server to.
    pub host: IpAddr,
    /// TCP port to listen on.
    pub port: u16,
    /// Native sampling interval in milliseconds.
    pub sample_interval_ms: u64,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 11310,
            sample_interval_ms: 1000,
        }
    }
}

/// Run the daemon with the given collector.
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
/// Returns an error if configuration is invalid.
pub async fn run<C: SystemCollector + 'static>(
    collector: C,
    config: RunConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        schema_version = SCHEMA_VERSION_V1,
        "greggd starting"
    );

    let server_config = ServerConfig {
        host: config.host,
        port: config.port,
        sample_interval_ms: config.sample_interval_ms,
    };
    if let Err(e) = server_config.validate() {
        eprintln!("configuration error: {e}");
        std::process::exit(1);
    }

    let interval_ms = match Sampler::<C, RealClock>::validate_interval(config.sample_interval_ms) {
        Ok(ms) => ms,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(1);
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
            run_sampler_loop(&mut sampler, &state, interval_ms, shutdown_rx).await;
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

/// Run the sampling loop, syncing state to the shared [`ServerState`].
///
/// This runs the sampler's collection cycle on the configured interval and
/// publishes snapshots to the HTTP server state after each successful sample.
async fn run_sampler_loop<C: SystemCollector, Clk: Clock>(
    sampler: &mut Sampler<C, Clk>,
    server_state: &ServerState,
    interval_ms: u64,
    mut shutdown: broadcast::Receiver<()>,
) {
    use gregg_protocol::ReadinessState;

    loop {
        sampler.sample_once();

        // Sync sampler state to the shared server state.
        match sampler.readiness() {
            ReadinessState::Ready => {
                if let Some(snap) = sampler.snapshot() {
                    server_state.update_snapshot((*snap).clone()).await;
                }
            }
            ReadinessState::Warming => {
                server_state.set_warming().await;
            }
            ReadinessState::Failed => {
                let health = sampler.health_response();
                let msg = health.message.unwrap_or_else(|| "collector failure".into());
                server_state.set_failed(&msg).await;
            }
        }

        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_millis(interval_ms)) => {}
            _ = shutdown.recv() => {
                tracing::info!("sampler shutting down");
                break;
            }
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
