# Plan 189: Slint tray and compact Gregg monitor MVP

Status: planned. Depends on completed Plan 188; unblocks Plan 190. Independent of Plan 091.

## Objective

Implement the optional `gregg-gui` frontend with a small native system tray/menu-bar entry and a compact Slint monitor window that mirrors the existing TUI's information, without starting a second polling plane or altering `greggd`.

## Product boundary

The tray is the normal always-visible entry. Use Slint's native `SystemTrayIcon` + menu, not a custom arbitrary-content popover. The native menu has a bounded system status summary, Open Monitor, Refresh, About/attribution and Quit. More detailed gauges, scrollable hosts and expansions live in a separate window opened by the menu. On macOS it uses `NSStatusItem`; on Linux `StatusNotifierItem`/AppIndicator; on Windows the Shell notification area.

The GUI window is initially approximately 420-520 logical pixels wide, resizable, compact and readable on high-DPI displays. It need not reproduce terminal cell geometry. It **must** preserve the semantic information and truthfulness of the TUI.

The MVP includes:

- Online / pending / offline fleet counts with meaningful disconnected/initializing state. Never classify unpolled as offline or unsupported cron as a host outage.
- One host row/card with identity, CPU%, physical memory%, applicable swap/Windows commit%, disk capacity%, and optional network throughput/utilization, plus load/current frequency/I/O wait when actually reported. Null is unavailable, never zero. Use normalized data and shared formatting/calculation helpers rather than duplicating percentage logic.
- Condensed fleet mode and an expanded selected-host detail section for drive mount/remaining/throughput and network interface rx/tx/capacity, including loopback only in interface detail as already defined.
- Optional cron summary per host: configured job, last terminal outcome, running/deferred/stale/unsupported markers and load-gate context. Selecting a job opens bounded recent stdout/stderr and the last-N records; the frontend sends `SetCronIntent` and releases it on close/switch/disconnect.
- Optional EggPool view when configured, with only the supported four periods, current readiness/health and exact four summary values already exposed by the DTO; sends active/period/refresh intents only while visible.
- A Refresh menu action equivalent to Systems config reload and immediate poll (`ReloadConfig`), respecting current acknowledgements/error diagnostics; do not add independent HTTP refresh.
- Accessible fallback window when a desktop exposes no tray service. Never become an invisible background process that has no way for the user to quit.

## Architectural implementation

### Local frontend bridge

- One daemon attachment from Plan 188 per GUI process/config; use `clientd::ensure_running_with_executable` (or the selected equivalent) and the existing `FrameStream` / `FrontendSender`. Do not own an HTTP client, poll scheduler, parsed remote config, or scheduler history store.
- Consume `FrontendFrame::Hello`, `Snapshot`, `ControlAck`, `VersionMismatch`, `ProtocolError` and `ShuttingDown` explicitly. Apply only increasing `generation` snapshots; an initial empty/pending document is legitimate. On disconnect, show disconnected/stale state rather than falsely retaining green/online freshness.
- GUI owns local selection, window visibility, details expansion, chosen EggPool period and active cron job. Key selection by stable system id, not a changing row index; repair selection when a host disappears/reorders so it cannot silently select a different host.
- Bridge Tokio tasks into Slint's UI event loop via a bounded latest-state channel or `slint::invoke_from_event_loop`. Slint properties, models and tray menu updates occur only on the UI thread. Coalesce snapshots and update changed properties rather than maintaining a 10/20 ms UI redraw timer. Do not block the UI loop on IPC, config, startup or process launch.
- Keep request generations and control acknowledgements explicit. For intent writes rejected by bounded channels, preserve the latest desired state for a safe retry on the next received frame rather than claiming a change was accepted.
- Cleanly release EggPool and cron intents when the corresponding view closes or an attachment is dropped; on reconnect publish the **new full** desired state, not stale deltas. Closing the window hides it and leaves tray + daemon observation active; explicit Quit ends the GUI only.
- The tray and Window are independent Slint top-level components, which **do not share global state automatically**. Drive both from the same Rust model. A Slint tray component must contain exactly one Menu child; dynamic item updates should be proven with real native behavior before relying on them.

### Presentation model

- Prefer a small pure `FrontendSnapshot -> GuiViewModel` projection, with unit-testable formatting, stable ordering and cap/truncation helpers. Do not convert DTOs to an invented second remote wire protocol.
- Reuse `crates/gregg/src/ui/text.rs` helpers when appropriate or extract only an actually reusable pure formatter without changing the existing TUI output.
- Preserve the remote scheduler's truthful UTC-vs-local labeling, load-gate relations, staleness and distinct remote-output truncation vs local view truncation. Treat arbitrary remote job output as hostile/untrusted display text: apply shared `sanitize` or an equivalent bounded control-visible conversion before rendering in a GUI or native menu; never interpret ANSI/OSC as styling or links.
- Avoid independently computing disk/network aggregates from per-device rows when the daemon already supplies authoritative aggregate fields.
- Bound tray menu entries and visible history row budgets; fleets can have many hosts and long display names. Full lists belong in a scrollable window, not an unbounded native menu.

### Native behavior

- Tray title/tooltip/icon changes with fleet health but does not encode false urgency for unsupported data. Avoid high-frequency menu rebuilding, continuous animations and fixed-interval repaints.
- macOS tray interaction must be via a native menu command to open the monitor; do not assume an OS-independent custom status-bar popover.
- On Linux without a StatusNotifierItem host, make the window visible on launch and explain tray limitations in user docs. Do not require GTK merely to draw the Slint window unless the measured selected backend needs it.
- Ensure window closing, reopening, app activation and final Quit work without orphan windows, double event loops, or premature termination of the client daemon.

## Deterministic test and manual acceptance

- [ ] With a synthetic `FrontendSnapshot`, each supported metric/optional/unsupported state projects correctly to a typed GUI model.
- [ ] Online -> offline, offline -> recovered, pending -> online, config repoint, changed order and disappearing selected id do not show stale wrong-host details.
- [ ] Cron capability unsupported, summary stale, out-of-order epoch/revision, selected job change and output truncation preserve all TUI meaning. Only a visible job asks for history; closing clears intent.
- [ ] EggPool inactive until its view opens, converges on the daemon-confirmed period, and deactivates when the last frontend closes the pane. Missing/unready health is not shown as ready.
- [ ] Two GUI/TUI windows use the same daemon without direct HTTP polling; the GUI has no HTTP dependency of its own.
- [ ] Local IPC disconnect, version mismatch, daemon shutdown, foreign peer, queue-full request, retry and reconnection lead to explicit UI state, never a hidden fallback poller.
- [ ] Slint event-loop integration updates tray and window correctly, with no UI-thread blocking and no redraw solely because a background timer woke.
- [ ] Native tray Open Monitor, Refresh, About/license attribution and Quit actions work; closing a monitor window leaves the tray running, and quitting leaves daemon/TUI running.
- [ ] The optional GUI is buildable independently without raising the root workspace MSRV, and the existing headless `gregg`/`greggd` release builds and tests remain unchanged.
- [ ] Add docs for tray availability, launch/install expectations, bundled Gregg helper resolution, error states, license attribution, and GUI keyboard/mouse actions in the same change as behavior; include CHANGELOG entry.

## Deliberate exclusions

No custom floating popover or compositor-positioning hacks, remote control or scheduled-job editing, alert/notification daemon, theme engine, webview, persistence of GUI viewport state, framework-agnostic widget library, new remote telemetry field, additional host polling, or general system service lifecycle redesign.

## Handoff to Plan 190

The MVP is complete only when there is a running tray and monitor consuming real daemon snapshots on at least one desktop and the frontend can exit without terminating the daemon. Plan 190 then supplies native cross-platform and footprint evidence before release.
