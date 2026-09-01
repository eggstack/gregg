use std::time::Duration;

use gregg::action;
use gregg::cli;
use gregg::clock;
use gregg::config;
use gregg::eggpool;
use gregg::eggpool_endpoint;
use gregg::endpoint;
use gregg::event;
use gregg::input;
use gregg::poller;
use gregg::scheduler;
use gregg::state;
use gregg::terminal;
use gregg::ui;

/// Plan 087: how long the visual selection highlight remains active
/// after the most recent selection-changing Systems action.
pub(crate) const SELECTION_HIGHLIGHT_DURATION: Duration = Duration::from_secs(10);

/// Plan 087: a far-future sleep deadline used to keep the highlight
/// timer dormant when no selection highlight is active. The value is
/// chosen to be large enough that no realistic test or operator
/// session ever crosses it.
const HIGHLIGHT_DORMANT_DEADLINE: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Plan 087: does this action activate or reset the Systems selection
/// highlight when the operator is currently on the Systems pane? Used
/// by the event loop to decide when to reset the highlight deadline.
fn selection_changing_systems_action(action: action::Action, pane: state::Pane) -> bool {
    if pane != state::Pane::Systems {
        return false;
    }
    matches!(
        action,
        action::Action::MoveDown
            | action::Action::MoveUp
            | action::Action::PageDown
            | action::Action::PageUp
            | action::Action::SelectFirst
            | action::Action::SelectLast
    )
}

fn spawn_eggpool_worker(
    config: &config::Config,
    timeout: Duration,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Option<eggpool::EggpoolWorker>, reqwest::Error> {
    config
        .eggpool
        .clone()
        .map(|endpoint| {
            eggpool::EggpoolClient::new(timeout)
                .map(|client| eggpool::spawn_worker(client, endpoint, cancel))
        })
        .transpose()
}

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();

    let config_path = cli::resolve_config_path(cli.config.as_ref());
    let store = config::ConfigStore::new(config_path);

    match &cli.command {
        None => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("error: failed to start runtime: {e}");
                    std::process::exit(3);
                }
            };
            if let Err(e) = runtime.block_on(run_tui(store)) {
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
    let client = poller::HttpClient::new(timeout)?;
    let clock = clock::RealClock;
    let refresh = Duration::from_secs(config.refresh_seconds);
    let max_concurrent = config.max_concurrent_requests as usize;

    let endpoints: Vec<gregg::endpoint::Endpoint> = config
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

    let eggpool_worker = spawn_eggpool_worker(&config, timeout, cancel.clone())?;
    if app_state.active_pane == state::Pane::Eggpool {
        if let (Some((period, generation)), Some(worker)) =
            (app_state.begin_eggpool_request(), eggpool_worker.as_ref())
        {
            send_eggpool_command(
                &mut app_state,
                &worker.commands,
                eggpool::EggpoolCommand::Activate { period, generation },
            );
        }
    }

    let eggpool_commands = eggpool_worker
        .as_ref()
        .map(|worker| worker.commands.clone());
    let mut eggpool_results = eggpool_worker.map(|worker| worker.results);

    let mut terminal = terminal::Terminal::init()?;
    let (event_stream, mut event_rx) = input::EventStream::new()?;

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
    let mut pending_system_refresh: Option<PendingSystemRefresh> = None;
    // Plan 087: highlight deadline bookkeeping. The dormant sleep
    // sits far in the future so the highlight arm never fires while
    // no selection highlight is active.
    let mut highlight_deadline: Option<tokio::time::Instant> = None;
    let mut highlight_sleep: std::pin::Pin<Box<tokio::time::Sleep>> =
        Box::pin(tokio::time::sleep(HIGHLIGHT_DORMANT_DEADLINE));

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
                            let before_pane = app_state.active_pane;
                            let before_highlight = app_state.selection_highlight_active;
                            let resets_highlight =
                                selection_changing_systems_action(action, before_pane);
                            if matches!(action, action::Action::RefreshNow)
                                && before_pane == state::Pane::Systems
                            {
                                app_state.apply_action(action);
                                begin_system_refresh(
                                    app_state,
                                    scheduler_tx,
                                    store,
                                    &mut pending_system_refresh,
                                )?;
                            } else {
                                dispatch_action_with_store(
                                    app_state,
                                    action,
                                    scheduler_tx,
                                    Some(store),
                                    eggpool_commands,
                                ).await?;
                            }
                            // Plan 087: a successful Systems selection-
                            // changing action always arms/reset the
                            // highlight timer; leaving Systems or
                            // clearing the highlight explicitly disarms
                            // it so a stale reversed row cannot
                            // reappear later.
                            if resets_highlight {
                                let new_deadline = tokio::time::Instant::now()
                                    + SELECTION_HIGHLIGHT_DURATION;
                                highlight_sleep.as_mut().reset(new_deadline);
                                highlight_deadline = Some(new_deadline);
                            } else if !app_state.selection_highlight_active
                                && before_highlight
                            {
                                highlight_sleep
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + HIGHLIGHT_DORMANT_DEADLINE);
                                highlight_deadline = None;
                            }
                        }
                    }
                    None => break,
                }
            }

            () = highlight_sleep.as_mut(), if highlight_deadline.is_some() => {
                highlight_deadline = None;
                highlight_sleep
                    .as_mut()
                    .reset(tokio::time::Instant::now() + HIGHLIGHT_DORMANT_DEADLINE);
                app_state.apply_action(action::Action::ClearSelectionHighlight);
            }

            result = async {
                let pending = pending_system_refresh.as_mut()?;
                Some(pending.send.as_mut().await)
            }, if pending_system_refresh.is_some() => {
                if let Some(pending) = pending_system_refresh.take() {
                    if let Some(result) = result {
                        result?;
                        if let Some(config) = pending.replacement {
                            app_state.reconcile_systems(&config);
                            app_state.clear_config_reload_error();
                        }
                    }
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
    dispatch_action_with_store(app_state, action, scheduler_tx, None, eggpool_commands)
        .await
        .expect("scheduler refresh without a config store cannot fail");
}

async fn dispatch_action_with_store(
    app_state: &mut state::AppState,
    action: action::Action,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: Option<&config::ConfigStore>,
    eggpool_commands: Option<&tokio::sync::mpsc::Sender<eggpool::EggpoolCommand>>,
) -> Result<(), SchedulerUnavailable> {
    let is_refresh = matches!(action, action::Action::RefreshNow);
    let before_pane = app_state.active_pane;
    let before_period = app_state.eggpool.as_ref().map(|eggpool| eggpool.period);
    app_state.apply_action(action);

    let Some(commands) = eggpool_commands else {
        if is_refresh {
            refresh_systems(app_state, scheduler_tx, store).await?;
        }
        return Ok(());
    };

    if is_refresh {
        if app_state.active_pane == state::Pane::Eggpool {
            if let Some((period, generation)) = app_state.begin_eggpool_request() {
                send_eggpool_command(
                    app_state,
                    commands,
                    eggpool::EggpoolCommand::Refresh { period, generation },
                );
            }
        } else {
            refresh_systems(app_state, scheduler_tx, store).await?;
        }
        return Ok(());
    }

    if before_pane != app_state.active_pane {
        match app_state.active_pane {
            state::Pane::Eggpool => {
                if let Some((period, generation)) = app_state.begin_eggpool_request() {
                    send_eggpool_command(
                        app_state,
                        commands,
                        eggpool::EggpoolCommand::Activate { period, generation },
                    );
                }
            }
            state::Pane::Systems => {
                send_eggpool_command(app_state, commands, eggpool::EggpoolCommand::Deactivate);
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
            );
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("poll scheduler command channel closed")]
struct SchedulerUnavailable;

struct PendingSystemRefresh {
    send: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), SchedulerUnavailable>> + Send>,
    >,
    replacement: Option<config::Config>,
}

fn begin_system_refresh(
    app_state: &mut state::AppState,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: &config::ConfigStore,
    pending: &mut Option<PendingSystemRefresh>,
) -> Result<(), SchedulerUnavailable> {
    let (command, replacement) = match store.load_existing() {
        Ok(config) => {
            let endpoints = config
                .systems
                .iter()
                .map(config::SystemEntry::to_endpoint)
                .collect();
            (
                scheduler::SchedulerCommand::ReplaceEndpoints(endpoints),
                Some(config),
            )
        }
        Err(error) => {
            // Keep the last-known-good state when an external edit is
            // temporarily missing, malformed, or invalid.
            app_state.set_config_reload_error(format!("config reload failed: {error}"));
            (scheduler::SchedulerCommand::Refresh, None)
        }
    };

    match scheduler_tx.try_send(command) {
        Ok(()) => {
            if let Some(config) = replacement {
                app_state.reconcile_systems(&config);
                app_state.clear_config_reload_error();
            }
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(command)) => {
            let sender = scheduler_tx.clone();
            *pending = Some(PendingSystemRefresh {
                send: Box::pin(async move {
                    sender.send(command).await.map_err(|_| SchedulerUnavailable)
                }),
                replacement,
            });
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            return Err(SchedulerUnavailable);
        }
    }
    Ok(())
}

async fn refresh_systems(
    app_state: &mut state::AppState,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: Option<&config::ConfigStore>,
) -> Result<(), SchedulerUnavailable> {
    let Some(store) = store else {
        scheduler_tx
            .send(scheduler::SchedulerCommand::Refresh)
            .await
            .map_err(|_| SchedulerUnavailable)?;
        return Ok(());
    };

    match store.load_existing() {
        Ok(config) => {
            let endpoints = config
                .systems
                .iter()
                .map(config::SystemEntry::to_endpoint)
                .collect();
            scheduler_tx
                .send(scheduler::SchedulerCommand::ReplaceEndpoints(endpoints))
                .await
                .map_err(|_| SchedulerUnavailable)?;
            app_state.reconcile_systems(&config);
            app_state.clear_config_reload_error();
        }
        Err(error) => {
            // Keep the last-known-good state when an external edit is
            // temporarily missing, malformed, or invalid.
            app_state.set_config_reload_error(format!("config reload failed: {error}"));
            scheduler_tx
                .send(scheduler::SchedulerCommand::Refresh)
                .await
                .map_err(|_| SchedulerUnavailable)?;
        }
    }

    Ok(())
}

/// Queue one `EggPool` command without ever blocking the event loop.
///
/// The command channel is bounded by design; when it is momentarily full
/// the command is dropped and the pane is surfaced as busy instead of
/// stalling key handling and poll-batch processing behind a slow fetch.
fn send_eggpool_command(
    app_state: &mut state::AppState,
    commands: &tokio::sync::mpsc::Sender<eggpool::EggpoolCommand>,
    command: eggpool::EggpoolCommand,
) {
    match commands.try_send(command) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => app_state.mark_eggpool_busy(),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            app_state.mark_eggpool_worker_unavailable();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg::config::{Config, EggpoolEntry, EggpoolScheme, SystemEntry};
    use gregg_protocol::test_support::LinuxSnapshotBuilder;
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
        .await
        .unwrap();

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
        .await
        .unwrap();
        assert_eq!(app.systems[0].endpoint.host, "192.168.183.143");
        assert!(app.config_reload_error.is_some());
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::Refresh)
        ));

        new_config.write_atomic(&path).unwrap();
        dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        )
        .await
        .unwrap();
        assert!(app.config_reload_error.is_none());
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::ReplaceEndpoints(_))
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn systems_refresh_waits_for_replacement_capacity_and_reconciles_after_delivery() {
        let dir =
            std::env::temp_dir().join(format!("gregg-main-reload-pressure-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gregg.toml");
        let store = config::ConfigStore::new(path.clone());

        let mut old_config = Config::default();
        old_config.systems.push(SystemEntry {
            id: "stable".into(),
            host: "endpoint-a.local".into(),
            port: 11310,
            name: None,
        });
        old_config.write_atomic(&path).unwrap();
        let mut app = state::AppState::from_config(&old_config);
        app.apply_batch(&poller::PollBatch {
            generation: 1,
            started_at: std::time::Instant::now(),
            completed_at: std::time::Instant::now(),
            results: vec![poller::PollResult {
                system_id: "stable".into(),
                endpoint: old_config.systems[0].to_endpoint(),
                outcome: poller::PollOutcome::Online(Box::new(
                    LinuxSnapshotBuilder::default().build(),
                )),
                latency: Duration::from_millis(1),
            }],
        });
        assert_eq!(app.systems[0].reachability, state::Reachability::Online);
        assert!(app.systems[0].latest.is_some());

        let (commands, mut received) = tokio::sync::mpsc::channel(1);
        commands
            .send(scheduler::SchedulerCommand::Refresh)
            .await
            .unwrap();

        let mut new_config = old_config.clone();
        new_config.systems[0].host = "endpoint-b.local".into();
        new_config.write_atomic(&path).unwrap();

        let mut dispatch = Box::pin(dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        ));
        tokio::select! {
            biased;

            result = &mut dispatch => panic!("replacement dispatch completed while the channel was full: {result:?}"),
            () = tokio::task::yield_now() => {}
        }

        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::Refresh)
        ));
        tokio::time::timeout(Duration::from_secs(1), dispatch)
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::ReplaceEndpoints(endpoints))
                if endpoints.len() == 1 && endpoints[0].host == "endpoint-b.local"
        ));
        assert_eq!(app.systems[0].endpoint.host, "endpoint-b.local");
        assert_eq!(app.systems[0].reachability, state::Reachability::Pending);
        assert!(app.systems[0].latest.is_none());
        assert!(app.systems[0].last_success_at.is_none());
        assert!(app.systems[0].last_attempt_at.is_none());
        assert!(app.systems[0].latency.is_none());
        assert!(app.systems[0].last_error.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sequential_system_replacements_converge_in_command_order() {
        let dir =
            std::env::temp_dir().join(format!("gregg-main-reload-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gregg.toml");
        let store = config::ConfigStore::new(path.clone());

        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "stable".into(),
            host: "endpoint-a.local".into(),
            port: 11310,
            name: None,
        });
        config.write_atomic(&path).unwrap();
        let mut app = state::AppState::from_config(&config);
        let (commands, mut received) = tokio::sync::mpsc::channel(1);
        commands
            .send(scheduler::SchedulerCommand::Refresh)
            .await
            .unwrap();

        config.systems[0].host = "endpoint-b.local".into();
        config.write_atomic(&path).unwrap();
        let mut dispatch_b = Box::pin(dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        ));
        tokio::select! {
            biased;

            result = &mut dispatch_b => panic!("replacement B completed while the channel was full: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::Refresh)
        ));
        tokio::time::timeout(Duration::from_secs(1), &mut dispatch_b)
            .await
            .unwrap()
            .unwrap();
        drop(dispatch_b);

        config.systems[0].host = "endpoint-c.local".into();
        config.write_atomic(&path).unwrap();
        let mut dispatch_c = Box::pin(dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        ));
        tokio::select! {
            biased;

            result = &mut dispatch_c => panic!("replacement C completed while replacement B was queued: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::ReplaceEndpoints(endpoints))
                if endpoints[0].host == "endpoint-b.local"
        ));
        tokio::time::timeout(Duration::from_secs(1), dispatch_c)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            received.recv().await,
            Some(scheduler::SchedulerCommand::ReplaceEndpoints(endpoints))
                if endpoints[0].host == "endpoint-c.local"
        ));
        assert_eq!(app.systems[0].endpoint.host, "endpoint-c.local");
        assert_eq!(app.systems[0].reachability, state::Reachability::Pending);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn closed_scheduler_channel_does_not_commit_reloaded_state() {
        let dir =
            std::env::temp_dir().join(format!("gregg-main-reload-closed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gregg.toml");
        let store = config::ConfigStore::new(path.clone());

        let mut old_config = Config::default();
        old_config.systems.push(SystemEntry {
            id: "stable".into(),
            host: "endpoint-a.local".into(),
            port: 11310,
            name: None,
        });
        old_config.write_atomic(&path).unwrap();
        let mut app = state::AppState::from_config(&old_config);
        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);

        let mut new_config = old_config.clone();
        new_config.systems[0].host = "endpoint-b.local".into();
        new_config.write_atomic(&path).unwrap();

        let result = dispatch_action_with_store(
            &mut app,
            action::Action::RefreshNow,
            &commands,
            Some(&store),
            None,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(app.systems[0].endpoint.host, "endpoint-a.local");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_config_creates_no_eggpool_worker() {
        let config = Config::default();
        let cancel = tokio_util::sync::CancellationToken::new();
        assert!(
            spawn_eggpool_worker(&config, Duration::from_secs(1), cancel)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn full_command_channel_does_not_block_dispatch_and_marks_busy() {
        let mut app = mixed_state();
        let (commands, mut received) = tokio::sync::mpsc::channel(1);
        let (refresh_tx, _) = tokio::sync::mpsc::channel(1);
        commands
            .send(eggpool::EggpoolCommand::Deactivate)
            .await
            .unwrap();

        // The single slot is occupied; dispatch must return immediately
        // instead of stalling the event loop behind a slow fetch.
        let dispatch = dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&commands),
        );
        tokio::select! {
            () = tokio::task::yield_now() => {
                panic!("dispatch blocked on a full eggpool command channel");
            }
            () = dispatch => {}
        }
        assert_eq!(app.active_pane, state::Pane::Eggpool);
        assert_eq!(
            app.eggpool.as_ref().unwrap().status,
            state::EggpoolStatus::Busy
        );

        // Only the pre-filled command was ever queued; the dropped
        // Activate must not surface later.
        assert!(matches!(
            received.recv().await,
            Some(eggpool::EggpoolCommand::Deactivate)
        ));
        assert!(received.try_recv().is_err());

        // Once capacity frees up, the next command is delivered normally.
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
