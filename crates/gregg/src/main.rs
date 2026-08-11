use std::time::Duration;

mod action;
mod cli;
mod clock;
mod config;
mod eggpool;
mod eggpool_endpoint;
mod endpoint;
mod event;
mod input;
mod normalized;
mod poller;
mod scheduler;
mod state;
mod terminal;
mod ui;

fn spawn_eggpool_worker(
    config: &config::Config,
    timeout: Duration,
    cancel: tokio_util::sync::CancellationToken,
) -> Option<eggpool::EggpoolWorker> {
    config.eggpool.clone().map(|endpoint| {
        eggpool::spawn_worker(eggpool::EggpoolClient::new(timeout), endpoint, cancel)
    })
}

#[cfg(test)]
mod mixed_fleet_evidence;

#[cfg(test)]
mod sustained_workload;

use clap::Parser;

#[tokio::main(flavor = "current_thread")]
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
                } else if let Some(ee) = e.downcast_ref::<eggpool_endpoint::EggpoolEndpointError>()
                {
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

    let (scheduler_tx, scheduler_rx) = tokio::sync::mpsc::channel::<scheduler::SchedulerCommand>(4);

    let scheduler = scheduler::PollScheduler::new(clock, client, refresh, max_concurrent);
    let mut batch_rx = Some(scheduler.run(endpoints, cancel.clone(), scheduler_rx));

    let eggpool_worker = spawn_eggpool_worker(&config, timeout, cancel.clone());
    if app_state.active_pane == state::Pane::Eggpool {
        if let (Some((period, generation)), Some(worker)) =
            (app_state.begin_eggpool_request(), eggpool_worker.as_ref())
        {
            send_eggpool_command(
                &mut app_state,
                &worker.commands,
                eggpool::EggpoolCommand::Activate { period, generation },
            )
            .await;
        }
    }

    let eggpool_commands = eggpool_worker
        .as_ref()
        .map(|worker| worker.commands.clone());
    let mut eggpool_results = eggpool_worker.map(|worker| worker.results);

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
        &scheduler_tx,
        &store,
        eggpool_commands.as_ref(),
        &mut eggpool_results,
    )
    .await;

    event_stream.shutdown();
    terminal.restore();
    if let Some(commands) = eggpool_commands {
        let _ = commands.send(eggpool::EggpoolCommand::Shutdown).await;
    }
    cancel.cancel();

    result
}

#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    terminal: &mut terminal::Terminal,
    app_state: &mut state::AppState,
    batch_rx: &mut Option<tokio::sync::mpsc::Receiver<poller::PollBatch>>,
    event_rx: &mut tokio::sync::mpsc::Receiver<event::Event>,
    cancel: &tokio_util::sync::CancellationToken,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: &config::ConfigStore,
    eggpool_commands: Option<&tokio::sync::mpsc::Sender<eggpool::EggpoolCommand>>,
    eggpool_results: &mut Option<tokio::sync::mpsc::Receiver<eggpool::EggpoolResult>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initial render.
    terminal.draw(|f| ui::render(f, app_state))?;

    loop {
        tokio::select! {
            biased;

            () = cancel.cancelled() => {
                break;
            }

            maybe_batch = recv_poll_batch(batch_rx) => {
                match maybe_batch {
                    Some(batch) => {
                        app_state.apply_batch(&batch);
                    }
                    None => {
                        // An empty system list has no scheduler traffic. Keep
                        // the TUI alive for an EggPool-only or empty config.
                        *batch_rx = None;
                    }
                }
            }

            maybe_result = recv_eggpool_result(eggpool_results) => {
                if let Some(result) = maybe_result {
                    app_state.apply_eggpool_result(&result);
                } else {
                    // A worker channel closing is not a system-monitoring error.
                    // Mark only the optional pane unavailable and keep Systems responsive.
                    app_state.mark_eggpool_worker_unavailable();
                    *eggpool_results = None;
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
                            dispatch_action_with_store(
                                app_state,
                                action,
                                scheduler_tx,
                                Some(store),
                                eggpool_commands,
                            ).await;
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

async fn recv_poll_batch(
    receiver: &mut Option<tokio::sync::mpsc::Receiver<poller::PollBatch>>,
) -> Option<poller::PollBatch> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => futures_util::future::pending().await,
    }
}

async fn recv_eggpool_result(
    receiver: &mut Option<tokio::sync::mpsc::Receiver<eggpool::EggpoolResult>>,
) -> Option<eggpool::EggpoolResult> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => futures_util::future::pending().await,
    }
}

#[cfg(test)]
async fn dispatch_action(
    app_state: &mut state::AppState,
    action: action::Action,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    eggpool_commands: Option<&tokio::sync::mpsc::Sender<eggpool::EggpoolCommand>>,
) {
    dispatch_action_with_store(app_state, action, scheduler_tx, None, eggpool_commands).await;
}

async fn dispatch_action_with_store(
    app_state: &mut state::AppState,
    action: action::Action,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: Option<&config::ConfigStore>,
    eggpool_commands: Option<&tokio::sync::mpsc::Sender<eggpool::EggpoolCommand>>,
) {
    let is_refresh = matches!(action, action::Action::RefreshNow);
    let before_pane = app_state.active_pane;
    let before_period = app_state.eggpool.as_ref().map(|eggpool| eggpool.period);
    app_state.apply_action(action);

    let Some(commands) = eggpool_commands else {
        if is_refresh {
            refresh_systems(app_state, scheduler_tx, store);
        }
        return;
    };

    if is_refresh {
        if app_state.active_pane == state::Pane::Eggpool {
            if let Some((period, generation)) = app_state.begin_eggpool_request() {
                send_eggpool_command(
                    app_state,
                    commands,
                    eggpool::EggpoolCommand::Refresh { period, generation },
                )
                .await;
            }
        } else {
            refresh_systems(app_state, scheduler_tx, store);
        }
        return;
    }

    if before_pane != app_state.active_pane {
        match app_state.active_pane {
            state::Pane::Eggpool => {
                if let Some((period, generation)) = app_state.begin_eggpool_request() {
                    send_eggpool_command(
                        app_state,
                        commands,
                        eggpool::EggpoolCommand::Activate { period, generation },
                    )
                    .await;
                }
            }
            state::Pane::Systems => {
                send_eggpool_command(app_state, commands, eggpool::EggpoolCommand::Deactivate)
                    .await;
            }
        }
    } else if before_pane == state::Pane::Eggpool
        && before_period != app_state.eggpool.as_ref().map(|eggpool| eggpool.period)
    {
        if let Some((period, generation)) = app_state.eggpool_request() {
            send_eggpool_command(
                app_state,
                commands,
                eggpool::EggpoolCommand::SetPeriod { period, generation },
            )
            .await;
        }
    }
}

fn refresh_systems(
    app_state: &mut state::AppState,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: Option<&config::ConfigStore>,
) {
    let Some(store) = store else {
        let _ = scheduler_tx.try_send(scheduler::SchedulerCommand::Refresh);
        return;
    };

    match store.load_existing() {
        Ok(config) => {
            app_state.reconcile_systems(&config);
            let endpoints = config
                .systems
                .iter()
                .map(config::SystemEntry::to_endpoint)
                .collect();
            let _ = scheduler_tx.try_send(scheduler::SchedulerCommand::ReplaceEndpoints(endpoints));
        }
        Err(_) => {
            // Keep the last-known-good state when an external edit is
            // temporarily missing, malformed, or invalid.
            let _ = scheduler_tx.try_send(scheduler::SchedulerCommand::Refresh);
        }
    }
}

async fn send_eggpool_command(
    app_state: &mut state::AppState,
    commands: &tokio::sync::mpsc::Sender<eggpool::EggpoolCommand>,
    command: eggpool::EggpoolCommand,
) {
    if commands.send(command).await.is_err() {
        app_state.mark_eggpool_worker_unavailable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, EggpoolEntry, EggpoolScheme, SystemEntry};
    use std::fs;

    fn mixed_state() -> state::AppState {
        AppStateBuilder::mixed().build()
    }

    struct AppStateBuilder;

    impl AppStateBuilder {
        fn mixed() -> Config {
            let mut config = Config::default();
            config.systems.push(SystemEntry {
                id: "system".into(),
                host: "system.local".into(),
                port: 11310,
                name: None,
            });
            config.eggpool = Some(EggpoolEntry {
                id: "eggpool".into(),
                host: "pool.local".into(),
                port: 11300,
                scheme: EggpoolScheme::Http,
                name: None,
                api_key_env: None,
            });
            config
        }
    }

    trait BuildState {
        fn build(self) -> state::AppState;
    }

    impl BuildState for Config {
        fn build(self) -> state::AppState {
            state::AppState::from_config(&self)
        }
    }

    #[tokio::test]
    async fn pane_and_refresh_commands_are_scoped_to_active_pane() {
        let mut app = mixed_state();
        let (commands, mut received) = tokio::sync::mpsc::channel(4);
        let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel(4);

        dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert_eq!(app.active_pane, state::Pane::Eggpool);
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Activate {
                period: eggpool::EggpoolPeriod::Hour,
                generation: 1
            })
        ));

        dispatch_action(
            &mut app,
            action::Action::RefreshNow,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Refresh {
                period: eggpool::EggpoolPeriod::Hour,
                generation: 2
            })
        ));
        assert!(refresh_rx.try_recv().is_err());

        dispatch_action(
            &mut app,
            action::Action::PreviousPane,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert_eq!(app.active_pane, state::Pane::Systems);
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Deactivate)
        ));
        dispatch_action(
            &mut app,
            action::Action::RefreshNow,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert!(matches!(
            refresh_rx.try_recv(),
            Ok(scheduler::SchedulerCommand::Refresh)
        ));
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn systems_refresh_reloads_the_same_store_and_replaces_endpoints() {
        let dir = std::env::temp_dir().join(format!("gregg-main-reload-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gregg.toml");
        let store = config::ConfigStore::new(path.clone());

        let mut old_config = Config::default();
        old_config.systems.push(SystemEntry {
            id: "stable".into(),
            host: "192.168.182.143".into(),
            port: 11310,
            name: None,
        });
        old_config.write_atomic(&path).unwrap();
        let mut app = state::AppState::from_config(&old_config);

        let mut new_config = old_config.clone();
        new_config.systems[0].host = "192.168.183.143".into();
        new_config.write_atomic(&path).unwrap();

        let (commands, mut received) = tokio::sync::mpsc::channel(2);
        dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        )
        .await;

        assert_eq!(app.systems[0].endpoint.host, "192.168.183.143");
        assert_eq!(app.systems[0].reachability, state::Reachability::Pending);
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::ReplaceEndpoints(endpoints))
                if endpoints[0].host == "192.168.183.143"
        ));

        fs::write(&path, "not valid toml").unwrap();
        dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        )
        .await;
        assert_eq!(app.systems[0].endpoint.host, "192.168.183.143");
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::Refresh)
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_config_creates_no_eggpool_worker() {
        let config = Config::default();
        let cancel = tokio_util::sync::CancellationToken::new();
        assert!(spawn_eggpool_worker(&config, Duration::from_secs(1), cancel).is_none());
    }

    #[tokio::test]
    async fn bounded_command_pressure_delivers_final_state_change_in_order() {
        let mut app = mixed_state();
        let (commands, mut received) = tokio::sync::mpsc::channel(1);
        let (refresh_tx, _) = tokio::sync::mpsc::channel(1);
        commands
            .send(eggpool::EggpoolCommand::Deactivate)
            .await
            .unwrap();

        let mut dispatch = Box::pin(dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&commands),
        ));
        tokio::select! {
            () = tokio::task::yield_now() => {}
            () = &mut dispatch => panic!("dispatch should wait for bounded capacity"),
        }
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Deactivate)
        ));
        dispatch.await;
        assert_eq!(app.active_pane, state::Pane::Eggpool);
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Activate {
                period: eggpool::EggpoolPeriod::Hour,
                generation: 1
            })
        ));

        dispatch_action(
            &mut app,
            action::Action::PreviousPane,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert_eq!(app.active_pane, state::Pane::Systems);
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Deactivate)
        ));
    }

    #[tokio::test]
    async fn closed_eggpool_command_channel_marks_worker_unavailable() {
        let mut app = mixed_state();
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let (refresh_tx, _) = tokio::sync::mpsc::channel(1);
        dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert_eq!(
            app.eggpool.as_ref().unwrap().status,
            state::EggpoolStatus::WorkerUnavailable
        );
    }

    #[tokio::test]
    async fn clamped_eggpool_period_does_not_send_a_command() {
        let config = AppStateBuilder::mixed();
        let mut app = state::AppState::from_config(&config);
        app.apply_action(action::Action::NextPane);
        let (commands, mut received) = tokio::sync::mpsc::channel(4);
        let (refresh_tx, _) = tokio::sync::mpsc::channel(1);

        dispatch_action(
            &mut app,
            action::Action::MoveUp,
            &refresh_tx,
            Some(&commands),
        )
        .await;
        assert!(received.try_recv().is_err());
    }
}
