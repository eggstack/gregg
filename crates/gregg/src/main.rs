mod action;
mod cli;
mod clock;
mod config;
mod endpoint;
mod event;
mod input;
mod poller;
mod scheduler;
mod state;
mod terminal;
mod ui;

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
    let mut batch_rx = scheduler.run(endpoints, cancel.clone());

    let mut terminal = terminal::Terminal::init()?;
    let (event_stream, mut event_rx) = input::EventStream::new();

    // Set initial terminal size in state.
    if let Ok((w, h)) = terminal::Terminal::size() {
        app_state.apply_action(action::Action::Resize {
            width: w,
            height: h,
        });
    }

    let result = run_event_loop(
        &mut terminal,
        &mut app_state,
        &mut batch_rx,
        &mut event_rx,
        &cancel,
    )
    .await;

    event_stream.shutdown();
    terminal.restore();
    cancel.cancel();

    result
}

async fn run_event_loop(
    terminal: &mut terminal::Terminal,
    app_state: &mut state::AppState,
    batch_rx: &mut tokio::sync::mpsc::Receiver<poller::PollBatch>,
    event_rx: &mut tokio::sync::mpsc::Receiver<event::Event>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initial render.
    terminal.draw(|f| ui::render(f, app_state))?;

    loop {
        tokio::select! {
            biased;

            () = cancel.cancelled() => {
                break;
            }

            maybe_batch = batch_rx.recv() => {
                match maybe_batch {
                    Some(batch) => {
                        app_state.apply_batch(&batch);
                    }
                    None => break,
                }
            }

            maybe_event = event_rx.recv() => {
                match maybe_event {
                    Some(evt) => {
                        if let Some(action) = event::translate_event(&evt) {
                            if matches!(action, action::Action::Quit) {
                                app_state.apply_action(action);
                                break;
                            }
                            app_state.apply_action(action);
                        }
                    }
                    None => break,
                }
            }
        }

        terminal.draw(|f| ui::render(f, app_state))?;
    }

    Ok(())
}
