# Plan 187: Optional Slint desktop frontend roadmap

> **The interaction model below is superseded by Plan 192.** This roadmap's premise — *"Slint's
> tray icon … renders a native menu; it is not a general-purpose arbitrary-content tray
> popover"*, so *"the first UI has a native tray menu and a separately opened Slint monitor
> window"* — was half true, and the conclusion drawn from it was wrong. The **native menu**
> cannot host arbitrary Slint content, true; but Slint's `SystemTrayIcon` also has a
> **`clicked`** callback, and a native menu with no entries detaches itself so the platform
> routes the tray's click to that callback. The tray's click can therefore open a Slint popup
> that **carries the content**, placed under the tray item: the popup *is* the pulldown, and
> the native menu need not be the dropdown at all.
>
> So the corrected direction, which **Plan 192** implements, is: **the tray's click opens the
> pulldown, and the pulldown carries the panel's content** — not a launcher that opens a
> separate window. The objective below, the target topology, the framework constraints and
> everything else in this roadmap stand; the two sentences that describe the *menu* and the
> *separately opened window* are amended in place.

Status: **complete for everything a macOS host can verify, plus a corrective
pass on 2026-10-21** that fixed three real defects — the broken Windows build,
the missing no-tray fallback, and the launch lock that made it impossible to
start a client daemon on Windows — and added native CI on Windows and Linux.
Children 188 and 189 are closed; child **Plan 190 is implemented, pending
release qualification** — its own native-platform acceptance gate is still
unchecked, and the outstanding work is listed in that plan's *Release
qualification* section. Child **Plan 191 is implemented, pending native
interactive acceptance**: the separate monitor window is gone, the GUI is one
tray pulldown, and it can add, remove, rename and re-point hosts. **Its
interaction model is superseded by Plan 192**, which makes the tray's click open
the pulldown and the pulldown carry the content, rather than the native menu
launching a separate window. The **interactive** three-desktop demonstration is
still **outstanding and not claimed** — see the closure record at the end of this
file and the scope record in `plans/README.md`. The GUI is therefore **not
shippable for Windows or Linux**, and it stays a source build.

Depends on: the completed Plan 161-172 Gregg client-daemon split and current main at planning time (2026-10-21). Independent of the Plan 091 soak record.

## Objective

Add a small, optional Rust desktop/menu-bar frontend that mirrors Gregg's fleet TUI without duplicating its polling, configuration, scheduler, cron history, or EggPool worker. The normal entry point is a native tray/menu-bar icon, and **clicking it opens the pulldown, which carries the content itself**. ~~The normal entry point is a native tray/menu-bar icon with a concise status menu. An **Open Monitor** action opens a compact, resizable monitor window.~~ *(Amended by Plan 192: the status menu was a launcher, and the monitor window was a separate surface. The tray's click now opens the pulldown, and the pulldown carries the fleet, the metrics, the drives, the interfaces, the cron, the records, the `EggPool` plane and the host forms.)* The GUI is not an agent, web dashboard, remote service, or privileged process.

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
                                                  |-- native tray icon
                                                  +-- one Slint pulldown,
                                                      opened by the tray's
                                                      click, carrying the
                                                      content itself
~~~

*(Amended by Plan 192: the pulldown replaces the "compact Slint window" that the tray's
menu used to launch. It is still one Slint window — a native menu cannot host Slint widgets —
but it is opened by the tray's click rather than by a menu entry, and it carries the content
rather than pointing at it.)*

The GUI owns its window/tray state, selected host, panels and view preferences. It **never** owns a remote HTTP client or polling scheduler, reads no remote config independently, and never silently falls back to direct polling. Different configs remain independent daemons; multiple GUI/TUI frontends on one config have one remote polling budget.

## Framework decision and constraints

Use **Slint 1.18.x** as the primary candidate, with its `SystemTrayIcon`, a Winit backend, and the software renderer; choose minimal Cargo features based on actual native builds, not assumed defaults. Slint's tray icon is its own top-level component and renders a native menu; it is not a general-purpose arbitrary-content tray popover. ~~The first UI has a native tray menu and a separately opened Slint monitor window;~~ do not promise identical popup positioning across macOS, Windows, X11 and Wayland.

*(Amended by Plan 192.)* The constraint above is still true of the **native menu**: it cannot
host arbitrary Slint content. What it did not say — and what the first UI wrongly assumed — is
that the content must therefore live in a **separately opened** window that the menu
*launches*. Slint's `SystemTrayIcon` also has a **`clicked`** callback, and a native menu with
no entries detaches itself so the platform routes the tray's click to that callback instead of
popping the menu. So the tray's click can open a Slint popup carrying the content, placed
under the tray item. The corrected constraint is: **the pulldown is a Slint window, opened by
the tray's click, and the native menu is not the dropdown.** The positioning caveat stands and
is unchanged — Slint 1.18.1 does not expose the tray item's screen position, so the pulldown
is anchored to the platform's trailing corner and clamped to the display's bounds.

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

- [x] Plan 188 records a concrete dual-toolchain build path and proves launch/attach from two differently named frontend binaries. — `apps/gregg-gui` is its own workspace with its own lockfile and its own Rust 1.92 floor; the root workspace excludes it and `cargo metadata` still shows exactly five core members, so the Rust 1.89 MSRV job never sees Slint. `clientd::DaemonExecutable` + `ensure_running_with_executable` is the single launcher, with `ensure_running` reduced to its `Current` wrapper, and the **GUI — a differently named binary — launches and attaches to the `gregg` client daemon** for real, natively.
- [x] Plan 189 provides a working compact Slint tray + monitor and reuses the exact client-daemon snapshot/intents. — a native `SystemTrayIcon` (exactly one `Menu`) beside an independent 560×520 monitor window, both driven from the same Rust model, fed by the daemon's own `FrontendSnapshot` documents over the existing local IPC; the `EggPool` and cron intents are the daemon's existing replacement-style intents.
- [ ] Plan 190 demonstrates the three desktop platforms, a no-duplicate-poll integration proof, and the release/footprint decision. — the **no-duplicate-poll proof and the release/footprint decision are done**: the counted harness keeps `/v2/status` flat in every phase and the native 25-host measurement is identical in all three configurations, and the packaging decision is source-build-only. The **three desktop platforms are not demonstrated**: only macOS was exercised, because a Windows run needs a Windows host and a Linux tray run needs a desktop session with a `StatusNotifierItem` host. Left unchecked rather than claimed, and it blocks shipment for Windows and Linux. Plan 190's own status is therefore **implemented, pending release qualification**, not closed.
- [x] Gregg's existing Rust 1.89 CI/MSRV, tests, TUI, and headless `greggd` remain unaffected. — the root workspace still has exactly its five core members; `cargo +1.89 test --workspace --all-targets --all-features` compiles the whole workspace and passes 967 tests; `git diff` over `crates/greggd` and `crates/gregg-protocol` is empty and `greggd`'s stripped release binary is unchanged at 3,356,800 bytes; the TUI's rendering and its key bindings are unchanged, and the shared scheduler/`EggPool` vocabulary it now uses renders exactly the same words.
- [x] Updates to `README.md`, `crates/gregg/README.md`, `architecture/gregg-client.md`, `AGENTS.md`, GUI docs, and `CHANGELOG.md` are made with the behavior they describe. — done in the same pass as each behavior, across Plans 188, 189 and 190.

## Invariants, as verified

Every invariant this roadmap set out is now checked against evidence rather
than against intent:

| invariant | how it is verified |
| --- | --- |
| Zero additional remote Systems or scheduler HTTP reads when opening another frontend | the counted harness: `/v2/status` is flat at four polls per window in every phase; natively, 375 status + 39 scheduler requests in **every** configuration on a 25-host fleet |
| Cron summary always available when supported; job history only for an active intent, released on close/disconnect; the intent never changes the remote fetch cadence | the cron-intent test: the local document grew 1903 → 2276 bytes and the remote's history counter stayed at the one discovery fetch |
| `EggPool` is activated by the converged intents of attached frontends, not always-on because a tray exists | the `EggPool` test: `7d` was asked for three times and `30d` never, and the request stream stopped when the last pane closed |
| Snapshot generation is monotonic; stale snapshots are discarded; missing values keep `None` semantics; unsupported cron is not offline metrics | the projection tests: only a strictly newer generation is applied, an absent metric is `—` and never `0`, and an unsupported scheduler family is not a host outage |
| User-local endpoint and handshake identity, launch lock, safe/version-directional rotation, bounded frames, IPC owner restrictions, no PID-file or process-name discovery | the 25 launcher tests in `clientd/launch.rs`, unchanged and green, plus the isolation test: distinct config paths reach distinct daemons, one path spelled two ways reaches one |
| A GUI exit never stops polling; an explicit Quit stops only its own frontend | natively: quitting the GUI left the client daemon alive and still polling |
| The Rust 1.89 core, the TUI UX, the headless CLI/daemon release path, manual publishing and `greggd`'s footprint remain unchanged | `cargo metadata` shows five core members; the MSRV job compiles and passes; `git diff` over `crates/greggd` is empty and its binary size is identical; the GUI's own dependency cost is measured separately and charged to the GUI |

## Handoff

Start with Plan 188. Do not begin by cloning the TUI's network scheduler into `gregg-gui`. The launch path and toolchain isolation are the only architectural prerequisites. Close each child plan using observed native verification; do not check a box merely because a Linux cargo build passed.

## Closure record

Closed on 2026-10-21 on a macOS host (Apple M4 Pro, macOS 26.0.1, `rustc`/`cargo`
1.99.1 stable, `aarch64-apple-darwin`). Children 188, 189 and 190 are all
closed; this roadmap's invariants are all verified.

### What the line produced

- **`apps/gregg-gui`** — an optional Slint desktop frontend that is a
  **frontend, never a second poller**. It attaches to the same per-config
  client daemon the TUI attaches to, over the same local IPC, and renders the
  documents that daemon already publishes.
- **A native tray and a compact monitor window**, with the truthfulness rules
  in a *pure* `FrontendSnapshot → GuiViewModel` projection so they are
  testable without a desktop session, and the scheduler and `EggPool` words
  **shared** with the TUI rather than reimplemented.
- **One launcher.** `clientd::DaemonExecutable` and
  `ensure_running_with_executable` let a frontend that is *not* the `gregg`
  binary name the client it should launch, with `ensure_running` reduced to
  its `Current` wrapper. A latent defect in the rotation branch — stop the
  stale daemon, then *attach* to the daemon it had just stopped — was
  corrected.
- **An isolated toolchain.** The GUI is its own Cargo workspace with its own
  lockfile and its own Rust 1.92 floor, excluded from the root workspace, so
  the Rust 1.89 MSRV job never sees Slint. One structural dependency conflict
  (`ratatui` 0.29's exact `unicode-width =0.2.0` against Slint's
  `annotate-snippets` needing `^0.2.2`) was resolved with a byte-identical
  vendored copy of `annotate-snippets` 0.12.9 and one relaxed bound, scoped to
  the GUI workspace only.
- **A counted qualification** proving the one-polling-plane property twice —
  deterministically in the harness and natively on 25 hosts — and an explicit
  packaging decision: **source-build-only**, one operator path, no asset-name
  collision, no downgrade, and the Slint royalty-free attribution verified
  present in the shipped binary.

### What this line did not do, and why

- **It did not demonstrate the three desktop platforms.** Only macOS was
  exercised. A Windows run needs a Windows host — the repository's own rule is
  that only a native Windows run is authority, and a cross-compiled build is
  not it — and a Linux tray run needs a desktop session with a
  `StatusNotifierItem` host, plus the "plain GNOME, no SNI host" fallback
  case. **Those two targets are not shippable on the strength of this line.**
  Read this with the corrective pass below: this bullet is about the
  **interactive** demonstration, and it was also, for a while, hiding two real
  defects that a build-only CI job would have caught.
- **It did not ship a GUI artifact.** There is no installer, no bundle and no
  release asset, and the GUI requires an existing Gregg installation.
- **It did not change the core.** `crates/greggd` and `crates/gregg-protocol`
  are untouched; `greggd`'s stripped binary is the same size it was; the TUI's
  rendered output is unchanged; the only core change is the *addition* of
  shared display vocabulary that the TUI now calls instead of its own copies.
- **It did not quiet the wakeups.** The obvious lever is the frontend's shared
  10 ms no-header poll, which is core behaviour the TUI also pays; changing it
  here would have reopened the headless client's correctness for a gain this
  host could not measure above its own noise. That is a future, separately
  approved piece of work.

### The corrective pass (2026-10-21)

Re-opening this roadmap for one focused pass found that the closure language
was describing the *evidence* accurately and the *code* inaccurately, and that
the reason was structural rather than accidental.

- **The GUI workspace had no CI.** It is a separate Cargo workspace with its own
  lockfile and its own toolchain, the root workspace excludes it, and no job
  built or tested it. A `#[cfg(windows)]` branch that nothing else compiles was
  therefore never compiled, and a UI requirement that nothing asserts was never
  asserted. The `#[cfg(windows)]` branch in
  `crates/gregg/src/clientd/launch.rs` asked `std::fs::Metadata` for its
  `extensions()` — a method that does not exist — and broke the **whole**
  `gregg` crate on Windows; the Windows CI job was red for every commit of this
  line from `9029bc9` to `14a4b93` (run `39628897811`). The GUI's
  "no-StatusNotifierItem-host" fallback, which this roadmap required by name,
  was never implemented at all: the GUI hid its only window and left the
  operator with nothing to press.
- **What the pass changed:** the Windows branch is fixed and locked by
  Windows-gated tests; the host list and the cron job list scroll with bounded
  heights; the no-tray fallback is implemented, reported and unit-tested; and
  the GUI workspace gets its own CI — fmt, clippy, tests and a release build
  on macOS, Windows and Linux natively, plus native tray exercises on the
  Windows and Linux legs.
- **What the new CI then found on Windows.** The first native Windows run of
  the GUI's own job found a defect no amount of reading had: the GUI started
  and created a native window, but **the client daemon it launched never came
  up**. The cause was in the core launcher, not in the GUI: the launch lock's
  path was derived from the endpoint, and on Windows the endpoint is a **named
  pipe**, so the lock path was inside the pipe *device* namespace, where no
  file can ever exist. The lock could therefore never be taken, and the
  launcher failed **before it spawned anything** — which also meant the
  **terminal client** could not launch a daemon on Windows either. The fix is
  a real file beside the config, named after the same identity digest; the
  Unix path is unchanged. The launcher's Windows rules now live in their own
  `#[cfg(all(test, windows))]` module, because the old module was
  `#[cfg(all(test, unix))]` — so the Windows-gated tests written inside it
  never compiled on Windows and never ran, which is how the first defect
  above got through in the first place.
- **What the pass does not change:** the GUI is still a **source build**, still
  ships for no target, and the **interactive** native demonstration on Windows
  and Linux is still **not performed** and still **not claimed**. A CI runner
  is a native Windows host and a native Linux host, so the *code* is now
  compiled, linted, tested and started there; it is not an operator's desktop,
  so a visible notification-area icon and a real GNOME session are still
  unproven. The wording "the three-desktop demonstration" is therefore
  narrowed, not widened: the **builds** now cover three desktops, the
  **demonstration** does not.

### Evidence

- `cargo test --manifest-path apps/gregg-gui/Cargo.toml` — **63 passed, 0
  failed** (48 of them from Plan 189 and 6 from Plan 190's counted harness,
  plus 9 added by the corrective pass), across three consecutive runs.
- `cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets
  --all-features -- -D warnings` — clean.
- `cargo fmt --manifest-path apps/gregg-gui/Cargo.toml -- --check` — clean.
- `cargo +1.89 test --workspace --all-targets --all-features` — the workspace
  compiles on the root MSRV; 967 pass. The 13 failures are the pre-existing,
  environment-induced ones, identical on an untouched worktree at `eb45029`.
- `cargo metadata --no-deps` — the root workspace still has exactly its five
  core members.
- `cargo build --release -p gregg -p greggd` — 5,224,096 and 3,356,800 bytes
  stripped, unchanged.
- Native macOS: the tray is present and its menu is correct; Open Monitor
  raises the window to the front; Reload polls immediately; About renders the
  Slint attribution; closing the window hides it and leaves the tray running;
  Quit ends the GUI while the client daemon survives and keeps polling; and
  the GUI process opens no network connections of its own.
- The measured artifacts are `apps/gregg-gui/EVIDENCE.md` (the counted
  request-count table and the process identities) and
  `apps/gregg-gui/README.md` (the footprint, the runtime cost and the
  packaging decision).
