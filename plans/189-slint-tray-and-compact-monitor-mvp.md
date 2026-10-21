# Plan 189: Slint tray and compact Gregg monitor MVP

Status: **complete**. Implemented and closed; see the closure record at the end
of this file and the scope record in `plans/README.md`.

Depends on completed Plan 188; unblocks Plan 190. Independent of Plan 091.

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

- [x] With a synthetic `FrontendSnapshot`, each supported metric/optional/unsupported state projects correctly to a typed GUI model. — `apps/gregg-gui/src/model.rs` projects a document into a typed `GuiViewModel` with no threads, no channels and no Slint, so every rule is tested without a desktop session: the supported families produce rows, an unsupported family produces **no row**, and a missing value is `—` and never `0`.
- [x] Online -> offline, offline -> recovered, pending -> online, config repoint, changed order and disappearing selected id do not show stale wrong-host details. — `a_pending_host_is_pending_and_never_offline`, `an_offline_host_states_why_and_keeps_the_detail`, `a_returning_host_takes_the_selection_back`, `a_disappearing_host_releases_the_selection_instead_of_sliding_it`, `the_selection_follows_the_host_and_not_the_row`, plus the repoint and reorder cases. The selection is keyed by the stable system id, so a host that changes endpoint or position keeps its selection and its detail rows come from its *current* snapshot.
- [x] Cron capability unsupported, summary stale, out-of-order epoch/revision, selected job change and output truncation preserve all TUI meaning. Only a visible job asks for history; closing clears intent. — the words are the **shared** ones (`gregg::cron::state_label`, `outcome_label`, `load_gate_label`, `duration_label`), so the GUI cannot drift from the TUI. The epoch/revision gating itself is the daemon's reducer, and the GUI structurally cannot violate it: it reads the reduced document and never re-derives or merges an epoch of its own. Output is passed through `gregg::sanitize`, so remote control characters appear in caret notation instead of being interpreted, and remote truncation is reported separately from any local truncation. The cron intent is published only while a job is open and is released when the view closes or the host changes.
- [x] EggPool inactive until its view opens, converges on the daemon-confirmed period, and deactivates when the last frontend closes the pane. Missing/unready health is not shown as ready. — the GUI publishes the `EggPool` intent only while the pane is open and releases it on close; the pane shows the daemon's **converged** period, never a locally requested one; `ready` is true only for `EggpoolProxyHealth::Ready`; the pane shows exactly the four summary values the DTO exposes, with a missing optional value rendered as `—`.
- [x] Two GUI/TUI windows use the same daemon without direct HTTP polling; the GUI has no HTTP dependency of its own. — `bridge::evidence::a_gui_window_costs_the_fleet_exactly_one_polling_plane` runs a real client daemon against an in-process counted HTTP remote, attaches a TUI-like frontend and the GUI's `Bridge`, and asserts the request count does not grow with the window count. The GUI crate has no HTTP client dependency at all.
- [x] Local IPC disconnect, version mismatch, daemon shutdown, foreign peer, queue-full request, retry and reconnection lead to explicit UI state, never a hidden fallback poller. — each bridge message maps to a distinct `ConnectionState`: a version mismatch is reported as *incompatible*, not as a lost connection, because the two send the operator looking for different problems. A rejected control write stays dirty and is republished; a reconnect publishes the **new full** desired state, because the daemon answering a new attachment has never seen the old one. There is no fallback poller anywhere in the GUI: the bridge owns no HTTP client.
- [x] Slint event-loop integration updates tray and window correctly, with no UI-thread blocking and no redraw solely because a background timer woke. — the bridge's notification callback only *schedules* work with `slint::invoke_from_event_loop`, and the component mutation happens on the UI thread. **There is no timer**: `slint::Timer` does not appear anywhere in the GUI sources, so a redraw can only happen because the bridge published something. The native macOS run below shows the tray summary and the window tracking the daemon's documents.
- [x] Native tray Open Monitor, Refresh, About/license attribution and Quit actions work; closing a monitor window leaves the tray running, and quitting leaves daemon/TUI running. — verified natively on macOS; see the evidence below. The Windows and Linux native runs are Plan 190's matrix and are **not** claimed here.
- [x] The optional GUI is buildable independently without raising the root workspace MSRV, and the existing headless `gregg`/`greggd` release builds and tests remain unchanged. — `apps/gregg-gui` is its own workspace with `rust-version = "1.92"`; `cargo metadata --no-deps` shows the root workspace still has exactly its five core members. `greggd` is untouched.
- [x] Add docs for tray availability, launch/install expectations, bundled Gregg helper resolution, error states, license attribution, and GUI keyboard/mouse actions in the same change as behavior; include CHANGELOG entry. — `apps/gregg-gui/README.md` gained "Running it", "Finding the client daemon", "Error states", "The tray" and "Actions"; `README.md`, `crates/gregg/README.md`, `architecture/gregg-client.md`, `.opencode/skills/gregg-client/SKILL.md` and `CHANGELOG.md` were updated in the same pass.

## Deliberate exclusions

No custom floating popover or compositor-positioning hacks, remote control or scheduled-job editing, alert/notification daemon, theme engine, webview, persistence of GUI viewport state, framework-agnostic widget library, new remote telemetry field, additional host polling, or general system service lifecycle redesign.

## Handoff to Plan 190

The MVP is complete only when there is a running tray and monitor consuming real daemon snapshots on at least one desktop and the frontend can exit without terminating the daemon. Plan 190 then supplies native cross-platform and footprint evidence before release.

## Closure record

Implemented and closed on 2026-10-21.

### What landed

**A three-layer frontend.** `apps/gregg-gui` is split so that every rule the
GUI keeps is testable without a desktop session:

- `src/model.rs` is a **pure projection**: one `FrontendSnapshot` in, one
  `GuiViewModel` out. No threads, no channels, no Slint. Absent is absent,
  unpolled is not offline, a missing value is `—` and never `0`, and the
  selection is keyed by the stable system id — all of it lives here.
- `src/bridge.rs` owns the client-daemon attachment and the intents, and holds
  **no HTTP client, no poll scheduler, no parsed remote configuration and no
  history store**. The daemon owns all of that.
- `src/main.rs` owns the Slint components. Slint components are not `Send`, so
  every mutation happens on the UI thread, and the bridge's notification
  callback only *schedules* work with `slint::invoke_from_event_loop`.

**A native tray and a compact window.** The `SystemTrayIcon` and the `Window`
are separate top-level components — a tray-rooted component does not share
`Globals` with a window-rooted one — and both are driven from the same Rust
model. The tray holds exactly one `Menu`: a bounded fleet summary, Open
Monitor, Reload, About Gregg, Quit Gregg. The window is 560×520 logical
pixels and shows the fleet, the selected host's metrics, drives and
interfaces, its scheduler jobs and records, and the `EggPool` plane.

**No polling timer.** `slint::Timer` appears nowhere in the GUI sources. The
bridge's callback schedules the update; an idle GUI redraws for no reason at
all.

**Desired state, not a command queue.** The `EggPool` and cron intents ride a
`watch` channel: the newest value wins, a rejected write stays dirty and is
republished, and a reconnect publishes the **new full** desired state, because
the daemon answering a new attachment has never seen the old one. The cron
intent is published only while a job is open; the `EggPool` intent only while
that pane is open.

**Shared vocabulary.** The scheduler and `EggPool` words moved into
`gregg::cron` and `gregg::eggpool` and the TUI's `ui/cron.rs` and
`ui/eggpool.rs` now call them, so the two frontends cannot drift. **The TUI's
rendered output is unchanged**, and its existing tests still assert the same
words.

### The defect this found, and the fix

**"Open Monitor" did nothing when the GUI was launched the way a real one is
launched.** The native macOS run surfaced it: the tray was present and the
menu was correct, but clicking Open Monitor left the window behind — from a
**background** launch, the process had no on-screen window at all
(`CGWindowListCopyWindowInfo` reported zero windows for the pid), and the
window only appeared once the application was activated by hand.

The cause is macOS, not the GUI: an `NSStatusItem` menu does not activate the
application that owns it, and a background application cannot bring its own
window to the front with `orderFront:`. A tray application *is* a background
application, and a GUI launched from a login item, a launcher or an agent is
one from its first moment. The symptom a user would see is the worst kind:
click the tray item, nothing happens, and the operator concludes the GUI is
broken.

Slint 1.18 exposes no activation API, so `src/platform.rs` calls
`NSApplication activateIgnoringOtherApps:` through `objc2`/`objc2-app-kit`.
Those crates are **macOS-only** with only the `NSApplication` feature
enabled, so they never enter the Linux or Windows builds and the whole of
AppKit is not linked; on Windows and Linux the function is a no-op, because
there showing a window the process owns is enough. The fix was verified by
re-running the same background-launch scenario: the window is now **first in
the front-to-back window order** immediately after the tray click, with no
manual activation.

Plan 189's own wording required this — "Ensure window closing, reopening,
**app activation** and final Quit work" — so it was in scope, and the fact
that the first native run was needed to find it is the reason the native run
was done at all.

### Honest gaps

- **The Windows-native and Linux-native halves of the native tray matrix are
  not claimed.** The macOS run is real and is recorded below. A Windows run
  needs a Windows host; a Linux tray run needs a desktop session with a
  `StatusNotifierItem` host. Plan 190 owns that matrix.
- **There are no keyboard shortcuts.** The MVP is mouse-driven and every
  action, including quitting, is reachable by pointer. The documentation says
  so rather than implying one.
- **The default local check does not pass on this host, and did not before
  this plan either.** `cargo test --workspace --all-targets --all-features`
  fails 14-16 tests on an untouched worktree at commit `eb45029`: the
  `eggpool` worker tests, the `scheduler` fleet-scaling tests, and
  `clientd::ipc::tests::a_config_inside_the_temp_directory_yields_one_location`.
  Re-verified during this plan on that clean worktree: the `ipc` test and the
  `eggpool` worker test fail there with none of this plan's changes present.
  The `ipc` test hardcodes `/tmp` while the code uses `std::env::temp_dir()`,
  so it fails wherever the temp directory is not `/tmp` (this host sets
  `TMPDIR`); the `eggpool`/`scheduler` tests combine tokio's `start_paused`
  auto-advance with real loopback TCP I/O, so a loaded machine advances
  virtual time past the connect before the request is issued — the
  fleet-scaling tests pass again when the machine is idle and fail while
  another build is running. The upstream CI run for the commit is green on
  every job. None of these were "fixed" here: they are outside this line's
  scope, and changing the tests to pass would have masked a real environment
  sensitivity.

### Evidence

**Deterministic**

- `cargo test --manifest-path apps/gregg-gui/Cargo.toml` — **48 passed, 0
  failed**.
- `cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets
  --all-features -- -D warnings` — clean.
- `cargo fmt --manifest-path apps/gregg-gui/Cargo.toml -- --check` — clean.
- `cargo test -p gregg --all-features -- clientd::launch` — 25 passed, 0
  failed.
- `cargo clippy -p gregg --all-targets --all-features -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `cargo metadata --no-deps` — the root workspace still has exactly its five
  core members.

**Native (macOS, a real desktop session)**

- The tray entry is present in the menu bar: a health-coloured green dot
  beside the label, confirmed visually and through the accessibility API
  (`menu bar 2` of the `gregg-gui` process contains one item named `Gregg`).
- The tray menu contains exactly `1 online, 0 pending, 0 offline —
  counted-remote`, a separator, `Open Monitor`, `Reload`, `About Gregg`, a
  separator and `Quit Gregg`.
- **Open Monitor** raises the window to the front: it is first in the
  front-to-back window order immediately after the click, with no manual
  activation — including from a background launch.
- The **close button hides** the window and the tray and the client daemon
  keep running.
- **Reload** produces an immediate poll and a scheduler summary read (two
  extra requests at the same instant) and then resumes the normal five-second
  cadence.
- **About Gregg** renders the Slint royalty-free attribution inside the
  application window.
- **Quit Gregg** ends the GUI process while the client daemon **survives and
  keeps polling** — the fixture's request counter kept advancing after the
  GUI exited.
- The GUI process opens **no network connections of its own**: `lsof -p
  <gui> -a -i` lists none. Every remote request is the daemon's.
