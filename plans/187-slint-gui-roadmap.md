# Plan 187: Optional Slint desktop frontend roadmap

Status: planned; planning/registration only. No GUI implementation is claimed.

Depends on: the completed Plan 161-172 Gregg client-daemon split and current main at planning time (2026-10-21). Independent of the Plan 091 soak record.

## Objective

Add a small, optional Rust desktop/menu-bar frontend that mirrors Gregg's fleet TUI without duplicating its polling, configuration, scheduler, cron history, or EggPool worker. The normal entry point is a native tray/menu-bar icon with a concise status menu. An **Open Monitor** action opens a compact, resizable monitor window. The GUI is not an agent, web dashboard, remote service, or privileged process.

## Verified baseline and reuse contract

- `greggd` supplies read-only remote metrics and scheduler endpoints. It never knows whether the consumer is a TUI or a GUI.
- The `gregg` **client daemon** is already one per normalized config, per user, and owns remote Systems/EggPool/cron polling, the normalized fleet reducer, bounded cron retention, and config-reload reconciliation.
- Frontends receive complete versioned `FrontendSnapshot` documents over owner-scoped local IPC via `clientd::frontend` and send replacement-style EggPool/cron intents plus explicit reload requests.
- The TUI starts/attaches through `clientd::ensure_running`, but this function currently launches `std::env::current_exe()` with `daemon run`. A separately named GUI executable cannot reuse that launch behavior unmodified.
- Core workspace MSRV is Rust 1.89, checked by `cargo test --workspace --all-targets --all-features`. Current Slint 1.18 requires Rust >=1.92. A GUI must not silently raise the core MSRV or put Slint into default core builds.
- Existing `crates/gregg/src/lib.rs` publicly exposes clientd, normalized, state, formatting and sanitization helpers. Reuse it before proposing extraction to yet another protocol/client crate.

Relevant code: `crates/gregg/src/clientd/{launch,frontend,daemon,protocol,snapshot}.rs`; `crates/gregg/src/main.rs`; `crates/gregg/src/normalized.rs`; `architecture/gregg-client.md`.

## Target topology

~~~text
greggd fleet -- HTTP --> one gregg client daemon per config
                               |
                               +-- local IPC --> existing gregg TUI(s)
                               |
                               +-- local IPC --> optional gregg-gui
                                                  |-- native tray/menu
                                                  +-- compact Slint window
~~~

The GUI owns its window/tray state, selected host, panels and view preferences. It **never** owns a remote HTTP client or polling scheduler, reads no remote config independently, and never silently falls back to direct polling. Different configs remain independent daemons; multiple GUI/TUI frontends on one config have one remote polling budget.

## Framework decision and constraints

Use **Slint 1.18.x** as the primary candidate, with its `SystemTrayIcon`, a Winit backend, and the software renderer; choose minimal Cargo features based on actual native builds, not assumed defaults. Slint's tray icon is its own top-level component and renders a native menu; it is not a general-purpose arbitrary-content tray popover. The first UI has a native tray menu and a separately opened Slint monitor window; do not promise identical popup positioning across macOS, Windows, X11 and Wayland.

Slint's 2026 royalty-free desktop license has a mandatory attribution alternative (top-level AboutSlint entry or public download-page badge). Choose and document the actual license path before shipping; keep Gregg's own source license unchanged unless legal review establishes a conflict. The GUI is a general-purpose desktop application only; this plan does not license embedded deployments. Verify the exact locked Slint/slint-build version and license on implementation.

Official references:
- https://docs.slint.dev/latest/docs/slint/reference/window/systemtrayicon/
- https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/
- https://docs.slint.dev/latest/docs/slint/guide/getting-started/
- https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md

## Planned sequence and ownership

**Plan 188 — frontend launch/toolchain boundary**. Establish a separate GUI build boundary and a safe explicit client-daemon executable selector (without altering the TUI's normal self-launch). Prove the Slint toolchain/tray setup compiles on a supported desktop. Keep the existing one-daemon lifecycle and local protocol.

**Plan 189 — menu bar and compact monitor**. Implement the separate GUI executable, native tray, narrow monitor window, snapshot-to-view projection, safe UI-thread bridge, system and optional details, and explicit cron/EggPool intent lifecycles. Never introduce direct polling.

**Plan 190 — cross-platform qualification and packaging**. Prove real Linux/macOS/Windows behavior, multi-frontend remote poll counts, daemon upgrade/identity safety, tray limitations and absence fallback, license attribution, packaging and measured footprint. No release before platform acceptance.

Dependency graph:

~~~text
161-172 (completed) --> 187 --> 188 --> 189 --> 190
091 (extended soak) -------------------------------- independent
~~~

## Invariants and boundaries

- Zero additional remote Systems or scheduler HTTP reads when opening another frontend; a GUI and a TUI share the same daemon and cached snapshots.
- Cron summary always available when supported; job history is published only for active `SetCronIntent`, released on pane close/disconnect. Intent never changes the remote fetch cadence.
- EggPool is activated by the converged intents of attached frontends, not always-on simply because a tray exists.
- Snapshot generation is monotonic; stale snapshots are discarded and missing values retain `None` semantics. Unsupported cron is not offline metrics.
- User-local daemon endpoint and handshake identity, launch lock, safe/version-directional rotation, bounded frame limits, IPC owner restrictions, and no PID-file/process-name discovery remain authoritative.
- A GUI exit never stops polling; a GUI explicit Quit stops only its own frontend, not the client daemon or other TUI windows.
- The existing Rust 1.89 core, TUI UX, headless CLI/daemon release path, manual publishing, and `greggd` footprint remain unchanged.

## Explicit exclusions

No webview/Tauri, network dashboard, remote control, new HTTP routes, remote cron mutation, alerts, tray-based privileged elevation, telemetry DB, secret storage, auto-update framework, mandatory daemon binary split, direct fleet polling, replacement of Ratatui, custom macOS/Wayland overlay window, or dedicated SBC compilation benchmark.

## Roadmap acceptance

- [ ] Plan 188 records a concrete dual-toolchain build path and proves launch/attach from two differently named frontend binaries.
- [ ] Plan 189 provides a working compact Slint tray + monitor and reuses the exact client-daemon snapshot/intents.
- [ ] Plan 190 demonstrates the three desktop platforms, a no-duplicate-poll integration proof, and the release/footprint decision.
- [ ] Gregg's existing Rust 1.89 CI/MSRV, tests, TUI, and headless `greggd` remain unaffected.
- [ ] Updates to `README.md`, `crates/gregg/README.md`, `architecture/gregg-client.md`, `AGENTS.md`, GUI docs, and `CHANGELOG.md` are made with the behavior they describe.

## Handoff

Start with Plan 188. Do not begin by cloning the TUI's network scheduler into `gregg-gui`. The launch path and toolchain isolation are the only architectural prerequisites. Close each child plan using observed native verification; do not check a box merely because a Linux cargo build passed.
