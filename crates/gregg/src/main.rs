use std::time::Duration;

use gregg::action;
use gregg::cli;
use gregg::clientd::frontend::FrameStream;
use gregg::clientd::protocol::FrontendFrame;
use gregg::clientd::{ClientDaemonIdentity, ControlSink, EggpoolIntentRequest};
use gregg::event;
use gregg::input;
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

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();

    let config_path = cli::resolve_config_path(cli.config.as_ref());
    let store = gregg::config::ConfigStore::new(config_path);

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
            if let Err(e) = runtime.block_on(run_tui(&store)) {
                eprintln!("error: {e}");
                std::process::exit(3);
            }
        }
        Some(command) => {
            if let Err(e) = cli::dispatch(command, &store) {
                eprintln!("error: {e}");
                let code = if let Some(ce) = e.downcast_ref::<gregg::config::ConfigError>() {
                    cli::ExitCode::from(ce)
                } else if let Some(ee) = e.downcast_ref::<gregg::endpoint::EndpointError>() {
                    cli::ExitCode::from(ee)
                } else if let Some(ee) =
                    e.downcast_ref::<gregg::eggpool_endpoint::EggpoolEndpointError>()
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

/// Attach to the client daemon and run the TUI against it.
///
/// Plan 164: the TUI reads no configuration file and opens no network
/// connection. Everything it renders arrives as a complete document from the
/// per-config client daemon, and everything it controls — the reload boundary
/// and the `EggPool` pane's activation and window — is a request across the
/// same local channel. If the daemon cannot be reached, this fails with the
/// reason. It never resumes direct polling, because that would recreate the
/// duplicate ownership this architecture exists to remove and would hide the
/// daemon's failure from the operator.
async fn run_tui(store: &gregg::config::ConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    use tokio_util::sync::CancellationToken;

    let identity = ClientDaemonIdentity::for_path(store.path());
    let attachment = gregg::clientd::attach(&identity, &cli::version_string()).await?;

    let cancel = CancellationToken::new();
    let ctrl_c_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        ctrl_c_cancel.cancel();
    });

    let mut app_state = state::AppState::blank();
    let mut terminal = terminal::Terminal::init()?;
    let (event_stream, mut event_rx) = input::EventStream::new()?;

    if let Ok((w, h)) = terminal::Terminal::size() {
        app_state.apply_action(action::Action::Resize {
            width: w,
            height: h,
        });
    }

    let result = run_event_loop(
        &mut terminal,
        &mut app_state,
        attachment.frames,
        &mut event_rx,
        &cancel,
        &attachment.frontend,
    )
    .await;

    event_stream.shutdown();
    terminal.restore();
    cancel.cancel();
    result
}

/// Frontend-local reason to leave the TUI, so a daemon-side problem is
/// reported as itself rather than as a generic transport error.
#[derive(Debug, thiserror::Error)]
enum FrontendExit {
    #[error("{0}")]
    Daemon(String),
    #[error("the client daemon connection failed: {0}")]
    Transport(String),
    #[error("the client daemon sent an invalid frame: {0}")]
    Protocol(String),
}

#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    terminal: &mut terminal::Terminal,
    app_state: &mut state::AppState,
    mut frames: FrameStream,
    event_rx: &mut tokio::sync::mpsc::Receiver<event::Event>,
    cancel: &tokio_util::sync::CancellationToken,
    sink: &dyn ControlSink,
) -> Result<(), Box<dyn std::error::Error>> {
    // Plan 087: highlight deadline bookkeeping. The dormant sleep
    // sits far in the future so the highlight arm never fires while
    // no selection highlight is active.
    let mut highlight_deadline: Option<tokio::time::Instant> = None;
    let mut highlight_sleep: std::pin::Pin<Box<tokio::time::Sleep>> =
        Box::pin(tokio::time::sleep(HIGHLIGHT_DORMANT_DEADLINE));
    let mut request_generation = 0_u64;
    let mut last_intent: Option<EggpoolIntentRequest> = None;

    // Initial render.
    terminal.draw(|f| ui::render(f, app_state))?;

    loop {
        let mut dirty = false;
        tokio::select! {
            biased;

            () = cancel.cancelled() => {
                break;
            }

            // Input before daemon frames: a flood of state documents must not
            // starve `Quit` or other key handling.
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
                            request_generation = request_generation.saturating_add(1);
                            dirty = dispatch_action(
                                app_state,
                                action,
                                sink,
                                &mut request_generation,
                                &mut last_intent,
                            );
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

            next = frames.next() => {
                match next {
                    Ok(FrontendFrame::Snapshot(document)) => {
                        // A document that is not newer than the last applied
                        // one is skipped, not rendered: that is the whole point
                        // of a latest-state channel.
                        dirty |= app_state.adopt_snapshot(&document);
                    }
                    // The daemon's introduction carries no renderable state —
                    // the complete document that follows carries everything —
                    // and a control acknowledgement's outcome is visible only
                    // in the next document. Neither is a reason to draw.
                    Ok(FrontendFrame::Hello(_) | FrontendFrame::ControlAck { .. }) => {}
                    Ok(FrontendFrame::ShuttingDown) => {
                        return Err(Box::new(FrontendExit::Daemon(
                            "the client daemon stopped; run `gregg daemon run` to start it again"
                                .to_owned(),
                        )));
                    }
                    Ok(FrontendFrame::VersionMismatch(payload)) => {
                        return Err(Box::new(FrontendExit::Daemon(format!(
                            "the client daemon for this config speaks local protocol {}, but this gregg speaks {}; stop it with `gregg daemon stop` and start it again with the current binary",
                            payload.daemon, payload.frontend
                        ))));
                    }
                    Ok(FrontendFrame::ProtocolError(message)) => {
                        return Err(Box::new(FrontendExit::Protocol(message)));
                    }
                    Err(error) => {
                        return Err(Box::new(FrontendExit::Transport(error.to_string())));
                    }
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
        }

        if dirty {
            terminal.draw(|f| ui::render(f, app_state))?;
        }
    }

    Ok(())
}

/// Apply one presentation action and forward whatever it implies to the daemon.
///
/// The return value is the render-visible change the reducer reported. A
/// request that was sent but has not been answered yet changes nothing on
/// screen, so a key press that only crosses the process boundary does not
/// force a frame.
fn dispatch_action(
    app_state: &mut state::AppState,
    action: action::Action,
    sink: &dyn ControlSink,
    request_generation: &mut u64,
    last_intent: &mut Option<EggpoolIntentRequest>,
) -> bool {
    let before_pane = app_state.active_pane;
    let before_period = app_state.eggpool.as_ref().map(|eggpool| eggpool.period);
    let is_refresh = matches!(action, action::Action::RefreshNow);
    let changed = app_state.apply_action_changed(action);
    let has_eggpool = app_state.eggpool.is_some();
    if !has_eggpool {
        return changed;
    }

    let active = app_state.active_pane == state::Pane::Eggpool;
    let period = app_state
        .eggpool_period_request
        .or_else(|| app_state.eggpool.as_ref().map(|eggpool| eggpool.period))
        .expect("eggpool presence was just checked");

    if is_refresh && !active {
        // `Ctrl-R` on the Systems pane is the config-reload boundary. The
        // daemon re-reads, reconciles, and republishes; there is no watcher
        // and no local fallback, so an invalid file surfaces as the
        // diagnostic the daemon publishes rather than as a local guess.
        *request_generation = request_generation.saturating_add(1);
        let _ = sink.request_reload(*request_generation);
        return changed;
    }

    let period_moved = before_period != Some(period);
    let pane_moved = before_pane != app_state.active_pane;
    let intent = EggpoolIntentRequest {
        active,
        period,
        refresh: is_refresh,
    };
    // Re-sending an identical intent would mint no new worker generation, so
    // it is skipped rather than queued. The manual refresh is the one case
    // where an unchanged intent is still worth sending.
    if !is_refresh && !pane_moved && !period_moved && *last_intent == Some(intent) {
        return changed;
    }

    *request_generation = request_generation.saturating_add(1);
    let _ = sink.request_eggpool_intent(&intent, *request_generation);
    *last_intent = Some(intent);
    changed
}
