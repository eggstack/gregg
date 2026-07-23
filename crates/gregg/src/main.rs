mod action;
mod cli;
mod clock;
mod config;
mod endpoint;
mod event;
mod poller;
mod scheduler;
mod state;

use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = cli::Cli::parse();

    let config_path = cli::resolve_config_path(cli.config.as_ref());
    let store = config::ConfigStore::new(config_path);

    match &cli.command {
        None => {
            if let Err(e) = run_tui(store).await {
                eprintln!("error: {e}");
                std::process::exit(3);
            }
        }
        Some(command) => {
            if let Err(e) = cli::dispatch(command, &store) {
                eprintln!("error: {e}");
                let code = if let Some(ce) = e.downcast_ref::<config::ConfigError>() {
                    cli::ExitCode::from(ce)
                } else if let Some(ee) = e.downcast_ref::<endpoint::EndpointError>() {
                    cli::ExitCode::from(ee)
                } else {
                    cli::ExitCode::OperationError
                };
                std::process::exit(code as i32);
            }
        }
    }
}

async fn run_tui(store: config::ConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    let config = store.load_or_default()?;
    let mut app_state = state::AppState::from_config(&config);

    let timeout = Duration::from_millis(config.request_timeout_ms);
    let client = poller::HttpClient::new(timeout);
    let clock = clock::RealClock;
    let refresh = Duration::from_secs(config.refresh_seconds);
    let max_concurrent = config.max_concurrent_requests as usize;

    let endpoints: Vec<crate::endpoint::Endpoint> = config
        .systems
        .iter()
        .map(config::SystemEntry::to_endpoint)
        .collect();

    let cancel = CancellationToken::new();
    let ctrl_c_cancel = cancel.clone();

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        ctrl_c_cancel.cancel();
    });

    let scheduler = scheduler::PollScheduler::new(clock, client, refresh, max_concurrent);
    let mut rx = scheduler.run(endpoints, cancel.clone());

    while let Some(batch) = rx.recv().await {
        app_state.apply_batch(&batch);
        // Phase 8 will render here.
    }

    cancel.cancel();
    Ok(())
}
