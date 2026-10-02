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
) -> Option<eggpool::EggpoolWorker> {
    config.eggpool.clone().map(|endpoint| {
        let client = eggpool::EggpoolClient::new(timeout);
        eggpool::spawn_worker(client, endpoint, cancel)
    })
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
                } else if let Some(ue) = e.downcast_ref::<gregg::uninstall::UninstallError>() {
                    // The client has no permission-specific exit; uninstall
                    // permission failures surface as operational errors with
                    // the exact elevated rerun already in the message.
                    let _ = ue;
                    cli::ExitCode::OperationError
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

    let eggpool_worker = spawn_eggpool_worker(&config, timeout, cancel.clone());
    if app_state.active_pane == state::Pane::Eggpool {
        app_state.begin_eggpool_request();
        if let (Some(desired), Some(worker)) =
            (app_state.eggpool_desired_state(), eggpool_worker.as_ref())
        {
            publish_eggpool_desired_state(&mut app_state, &worker.control, desired);
        }
    }

    let eggpool_control = eggpool_worker.as_ref().map(|worker| worker.control.clone());
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
        eggpool_control.as_ref(),
        &mut eggpool_results,
    )
    .await;

    event_stream.shutdown();
    terminal.restore();
    // Plan 151: the worker needs no queued shutdown command. Dropping the
    // control publisher and cancelling terminate it promptly.
    drop(eggpool_control);
    cancel.cancel();

    result
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn run_event_loop(
    terminal: &mut terminal::Terminal,
    app_state: &mut state::AppState,
    batch_rx: &mut Option<tokio::sync::mpsc::Receiver<poller::PollBatch>>,
    event_rx: &mut tokio::sync::mpsc::Receiver<event::Event>,
    cancel: &tokio_util::sync::CancellationToken,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: &config::ConfigStore,
    eggpool_control: Option<&eggpool::EggpoolControl>,
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
        let mut dirty = false;
        tokio::select! {
            biased;

            () = cancel.cancelled() => {
                break;
            }

            // Input before scheduler batches: a flooded batch channel
            // must not starve `Quit` or other key handling.
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
                                // `RefreshNow` itself never changes render-visible
                                // state; only the reload outcome does.
                                let _ = app_state.apply_action_changed(action);
                                dirty = begin_system_refresh(
                                    app_state,
                                    scheduler_tx,
                                    store,
                                    &mut pending_system_refresh,
                                )?;
                            } else {
                                dirty = dispatch_action_with_store(
                                    app_state,
                                    action,
                                    scheduler_tx,
                                    Some(store),
                                    eggpool_control,
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

            maybe_batch = recv_poll_batch(batch_rx) => {
                match maybe_batch {
                    Some(batch) => {
                        // Plan 143: rejected/stale batches and fully-ignored
                        // results do not force a frame.
                        dirty = app_state.apply_batch_owned_changed(batch);
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
                    dirty = app_state.apply_eggpool_result_changed(&result);
                } else {
                    // A worker channel closing is not a system-monitoring error.
                    // Mark only the optional pane unavailable and keep Systems responsive.
                    app_state.mark_eggpool_worker_unavailable();
                    *eggpool_results = None;
                    dirty = true;
                }
            }

            () = highlight_sleep.as_mut(), if highlight_deadline.is_some() => {
                highlight_deadline = None;
                highlight_sleep
                    .as_mut()
                    .reset(tokio::time::Instant::now() + HIGHLIGHT_DORMANT_DEADLINE);
                // Plan 143: clearing an already-clear highlight is a no-op.
                dirty = app_state
                    .apply_action_changed(action::Action::ClearSelectionHighlight);
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
                            dirty = true;
                        }
                    }
                }
            }
        }

        if dirty {
            terminal.draw(|f| ui::render(f, app_state))?;
        }
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
    eggpool_control: Option<&eggpool::EggpoolControl>,
) {
    dispatch_action_with_store(app_state, action, scheduler_tx, None, eggpool_control)
        .await
        .expect("scheduler refresh without a config store cannot fail");
}

async fn dispatch_action_with_store(
    app_state: &mut state::AppState,
    action: action::Action,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: Option<&config::ConfigStore>,
    eggpool_control: Option<&eggpool::EggpoolControl>,
) -> Result<bool, SchedulerUnavailable> {
    // Plan 143: drive the dirty gate from the reducer result plus explicit
    // extras the reducer never touches (EggPool request identity/local
    // worker state and the `config_reload_error` diagnostic owned by
    // refresh paths).
    let before_eggpool = app_state.eggpool_request();
    let before_eggpool_worker = app_state
        .eggpool
        .as_ref()
        .map(|eggpool| eggpool.worker_state);
    let before_error = app_state.config_reload_error.clone();
    let is_refresh = matches!(action, action::Action::RefreshNow);
    // Plan 151: activation and manual refresh are the only transitions that
    // mint a new refresh generation; period moves mint one inside the
    // reducer and deactivation never fabricates one.
    let before_desired = app_state.eggpool_desired_state();
    let reducer_changed = app_state.apply_action_changed(action);

    let dirty = |app_state: &state::AppState| {
        reducer_changed
            || before_eggpool != app_state.eggpool_request()
            || before_eggpool_worker
                != app_state
                    .eggpool
                    .as_ref()
                    .map(|eggpool| eggpool.worker_state)
            || before_error != app_state.config_reload_error
    };

    let Some(control) = eggpool_control else {
        if is_refresh {
            return refresh_systems(app_state, scheduler_tx, store).await;
        }
        return Ok(dirty(app_state));
    };

    if is_refresh {
        if app_state.active_pane == state::Pane::Eggpool {
            app_state.begin_eggpool_request();
        } else {
            return refresh_systems(app_state, scheduler_tx, store).await;
        }
    } else if app_state.active_pane == state::Pane::Eggpool
        && before_desired.is_some_and(|desired| !desired.active)
    {
        // Entering the pane activates the worker with a fresh generation.
        app_state.begin_eggpool_request();
    }

    // Plan 151: one nonblocking latest-desired-state publication replaces
    // per-transition commands. Nothing here awaits worker capacity, and no
    // activation, period change, manual refresh, or deactivation can be
    // dropped while the worker is busy.
    if let Some(desired) = app_state.eggpool_desired_state() {
        if before_desired != Some(desired) {
            publish_eggpool_desired_state(app_state, control, desired);
        }
    }

    Ok(dirty(app_state))
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
) -> Result<bool, SchedulerUnavailable> {
    // Plan 143: a successful reload reconciles or reports diagnostics
    // (both render-visible); backpressure defers the visible change to the
    // pending branch. `RefreshNow` itself never changes state.
    let error_before = app_state.config_reload_error.clone();
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
    let error_changed = app_state.config_reload_error != error_before;

    match scheduler_tx.try_send(command) {
        Ok(()) => {
            if let Some(config) = replacement {
                app_state.reconcile_systems(&config);
                app_state.clear_config_reload_error();
                // Reconciliation installs a new endpoint list (visible via
                // pending/offline rows even before the next batch); clearing
                // a previous diagnostic is also visible.
                return Ok(true);
            }
            Ok(error_changed)
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(command)) => {
            let sender = scheduler_tx.clone();
            *pending = Some(PendingSystemRefresh {
                send: Box::pin(async move {
                    sender.send(command).await.map_err(|_| SchedulerUnavailable)
                }),
                replacement,
            });
            // Visible change (if any) happens when the pending send completes.
            Ok(error_changed)
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(SchedulerUnavailable),
    }
}

async fn refresh_systems(
    app_state: &mut state::AppState,
    scheduler_tx: &tokio::sync::mpsc::Sender<scheduler::SchedulerCommand>,
    store: Option<&config::ConfigStore>,
) -> Result<bool, SchedulerUnavailable> {
    let Some(store) = store else {
        scheduler_tx
            .send(scheduler::SchedulerCommand::Refresh)
            .await
            .map_err(|_| SchedulerUnavailable)?;
        // Bare refresh without a store never changes render-visible state
        // by itself; the next batch drives redraws.
        return Ok(false);
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
            Ok(true)
        }
        Err(error) => {
            // Keep the last-known-good state when an external edit is
            // temporarily missing, malformed, or invalid.
            app_state.set_config_reload_error(format!("config reload failed: {error}"));
            scheduler_tx
                .send(scheduler::SchedulerCommand::Refresh)
                .await
                .map_err(|_| SchedulerUnavailable)?;
            Ok(true)
        }
    }
}

/// Publish the latest desired `EggPool` worker state.
///
/// Plan 151: publication is synchronous and capacity-free, so the input
/// path never waits on a slow worker and a full queue can never discard a
/// state-changing transition. A closed worker is the only failure and is
/// surfaced as `WorkerUnavailable`; there is no busy substitute for
/// failed convergence.
fn publish_eggpool_desired_state(
    app_state: &mut state::AppState,
    control: &eggpool::EggpoolControl,
    desired: eggpool::EggpoolDesiredState,
) {
    if control.publish(desired).is_err() {
        app_state.mark_eggpool_worker_unavailable();
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

    /// A live `EggPool` worker control handle for one unreachable
    /// loopback endpoint. Requests fail fast, so the control contract can
    /// be observed without any external service.
    fn eggpool_control() -> (eggpool::EggpoolControl, tokio_util::sync::CancellationToken) {
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker = eggpool::spawn_worker(
            eggpool::EggpoolClient::new(Duration::from_secs(1)),
            EggpoolEntry {
                id: "eggpool".into(),
                host: "127.0.0.1".into(),
                port: 1,
                scheme: EggpoolScheme::Http,
                name: None,
                api_key_env: None,
            },
            cancel.clone(),
        );
        (worker.control.clone(), cancel)
    }

    fn desired(
        active: bool,
        period: eggpool::EggpoolPeriod,
        generation: u64,
    ) -> eggpool::EggpoolDesiredState {
        eggpool::EggpoolDesiredState {
            active,
            period,
            generation,
        }
    }

    #[tokio::test]
    async fn pane_and_refresh_desired_state_are_scoped_to_active_pane() {
        let mut app = mixed_state();
        let (control, cancel) = eggpool_control();
        let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel(4);

        dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert_eq!(app.active_pane, state::Pane::Eggpool);
        assert_eq!(
            control.published(),
            desired(true, eggpool::EggpoolPeriod::Hour, 1)
        );

        dispatch_action(
            &mut app,
            action::Action::RefreshNow,
            &refresh_tx,
            Some(&control),
        )
        .await;
        // A manual refresh at an unchanged period is still observable.
        assert_eq!(
            control.published(),
            desired(true, eggpool::EggpoolPeriod::Hour, 2)
        );
        assert!(refresh_rx.try_recv().is_err());

        dispatch_action(
            &mut app,
            action::Action::MoveDown,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert_eq!(
            control.published(),
            desired(true, eggpool::EggpoolPeriod::Day, 3)
        );

        dispatch_action(
            &mut app,
            action::Action::PreviousPane,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert_eq!(app.active_pane, state::Pane::Systems);
        // Leaving the pane never fabricates a new request generation.
        assert_eq!(
            control.published(),
            desired(false, eggpool::EggpoolPeriod::Day, 3)
        );
        dispatch_action(
            &mut app,
            action::Action::RefreshNow,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert!(matches!(
            refresh_rx.try_recv(),
            Ok(scheduler::SchedulerCommand::Refresh)
        ));
        assert_eq!(
            control.published(),
            desired(false, eggpool::EggpoolPeriod::Day, 3)
        );
        cancel.cancel();
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
        assert!(app.systems[0].offline_reason.is_none());

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
        assert!(spawn_eggpool_worker(&config, Duration::from_secs(1), cancel).is_none());
    }

    #[tokio::test]
    async fn rapid_eggpool_state_changes_never_block_systems_dispatch() {
        let mut app = mixed_state();
        let (control, cancel) = eggpool_control();
        let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel(4);

        // Rapid EggPool transitions (period moves, deactivation, and
        // reactivation) must never stall input handling or Systems
        // poll-result processing behind worker capacity. Only EggPool
        // control is under observation here, so the burst stays
        // synchronous: any pending await would fail this test.
        let burst = async {
            for _ in 0..2_000 {
                dispatch_action(
                    &mut app,
                    action::Action::MoveDown,
                    &refresh_tx,
                    Some(&control),
                )
                .await;
                dispatch_action(
                    &mut app,
                    action::Action::PreviousPane,
                    &refresh_tx,
                    Some(&control),
                )
                .await;
                dispatch_action(
                    &mut app,
                    action::Action::MoveUp,
                    &refresh_tx,
                    Some(&control),
                )
                .await;
                dispatch_action(
                    &mut app,
                    action::Action::NextPane,
                    &refresh_tx,
                    Some(&control),
                )
                .await;
            }
        };
        tokio::select! {
            biased;
            () = tokio::task::yield_now() => {
                panic!("dispatch blocked on EggPool worker capacity");
            }
            () = burst => {}
        }
        // The burst ends with the pane deactivated, and the retained
        // latest state matches the reducer exactly.
        assert_eq!(app.active_pane, state::Pane::Systems);
        let published = control.published();
        assert!(!published.active);
        assert_eq!(
            published.generation,
            app.eggpool.as_ref().unwrap().request_generation
        );
        assert_eq!(published.period, app.eggpool.as_ref().unwrap().period);

        // Systems polling stays responsive after the burst.
        dispatch_action(
            &mut app,
            action::Action::RefreshNow,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert!(matches!(
            refresh_rx.try_recv(),
            Ok(scheduler::SchedulerCommand::Refresh)
        ));
        // Reactivating still publishes a fresh generation.
        dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert_eq!(app.active_pane, state::Pane::Eggpool);
        assert!(control.published().active);
        assert!(control.published().generation > published.generation);
        cancel.cancel();
    }

    #[tokio::test]
    async fn closed_eggpool_control_channel_marks_worker_unavailable() {
        let mut app = mixed_state();
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut worker = eggpool::spawn_worker(
            eggpool::EggpoolClient::new(Duration::from_secs(1)),
            EggpoolEntry {
                id: "eggpool".into(),
                host: "127.0.0.1".into(),
                port: 1,
                scheme: EggpoolScheme::Http,
                name: None,
                api_key_env: None,
            },
            cancel.clone(),
        );
        let control = worker.control.clone();
        let (refresh_tx, _) = tokio::sync::mpsc::channel(1);
        cancel.cancel();
        // The worker exits on cancellation, so publication now has no
        // live worker to reach.
        assert!(worker.results.recv().await.is_none());
        dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert_eq!(
            app.eggpool.as_ref().unwrap().worker_state,
            state::EggpoolWorkerState::WorkerUnavailable
        );
    }

    #[tokio::test]
    async fn clamped_eggpool_period_does_not_change_desired_state() {
        let config = AppStateBuilder::mixed();
        let mut app = state::AppState::from_config(&config);
        let (control, cancel) = eggpool_control();
        let (refresh_tx, _) = tokio::sync::mpsc::channel(1);

        dispatch_action(
            &mut app,
            action::Action::NextPane,
            &refresh_tx,
            Some(&control),
        )
        .await;
        let before = control.published();
        assert_eq!(before.generation, 1);

        // The shortest period cannot move, so neither the desired state nor
        // the refresh generation changes.
        dispatch_action(
            &mut app,
            action::Action::MoveUp,
            &refresh_tx,
            Some(&control),
        )
        .await;
        assert_eq!(control.published(), before);
        assert_eq!(app.eggpool.as_ref().unwrap().request_generation, 1);
        cancel.cancel();
    }
}
