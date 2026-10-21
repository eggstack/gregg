# Plan 190: GUI multi-frontend correctness, native qualification and packaging

Status: planned. Depends on Plans 188-189; closes Plan 187. Independent of Plan 091.

## Objective

Qualify the optional Slint frontend as a small and honest desktop product without regressing Gregg's one-poller daemon, Rust 1.89 core, existing TUI, Windows IPC correctness or manual release process. Make an explicit evidence-backed packaging decision; do not automatically publish new assets.

## Verification baseline and environment

Record the tested commit, Rust versions (root MSRV 1.89 and GUI build toolchain), exact Slint/slint-build locked versions, target OS/build identifiers, selected renderer/backend, optimized size and existing `gregg`/`greggd` baselines. Use deterministic fixtures for protocol logic; use actual native runners/desktops for system-tray and process lifecycle behavior. Do not infer macOS/Windows success from Linux or Windows GUI success from Linux cross-compilation.

## Multi-frontend proof (blocking)

Construct a local fleet fixture with counted routes and at least two monitored endpoints; include a scheduler-capable remote and an old remote with no scheduler routes. Run:

1. Client daemon alone; record a baseline for each remote `/v2/status`, scheduler summary and revision-gated history request over several relevant polling intervals.
2. Attach a TUI and a GUI to the **same config**, and optionally a second GUI/window; assert request counts/cadence do not multiply. The GUI's opened cron job may increase **local IPC document size**, but must not add remote history fetches.
3. Open/close the EggPool view across frontends; prove worker activation and newest-converged period match existing reduction behavior rather than starting one worker per frontend.
4. Close both frontends; prove the client daemon remains alive and polls with its configured cadence. Reattach; the newest coherent full snapshot should be available without rebuilding history from zero.
5. Exercise distinct config paths (isolation), same config path spellings (one identity), daemon restart, deliberate incompatibility, foreign IPC peers, explicit user restart and GUI quit. No cross-stop or unowned process termination.

Capture a concise request-count table and observable process identities; do not close the plan based solely on matching source comments.

## Native platform matrix

**macOS**: Apple Silicon and Intel builds (when runners available); actual `NSStatusItem` creation, icon/menu, Open Monitor, app activation, close-to-tray, retina scaling, no terminal, bundle executable/helper lookup, and user LaunchAgent interaction. Unsigned .app quarantine expectations must be documented; signing/notarization is a separate release decision.

**Windows x86-64**: native MSVC compile, installer/bundle path including spaces, named-pipe owner policy, two-frontends-at-once, Shell notification area menu, hide/reopen, app lifecycle, launched helper console visibility, and no privileged SCM coupling. Follow repository's existing native Windows authority requirement.

**Linux x86-64**: X11 and Wayland smoke where possible, D-Bus StatusNotifierItem host, tray menu and monitor window. Also test a plain GNOME session without SNI/AppIndicator extension (or an equivalent missing host): the application must provide a visible/failsafe window and clear guidance, not hide its only control surface. GUI runtime prerequisites must be documented independently of headless `greggd`. ARM64 GUI artifact is optional until a native build/runtime proof and delivery decision; remote greggd on ARM64 remains unchanged.

Do not add FreeBSD GUI support merely because `gregg-host` tests on FreeBSD.

## Package/install/update policy

- Document the exact daemon executable location the GUI launches on each platform. For development allow an explicit absolute CLI path; for end-user bundles prefer a fixed, installer-owned embedded/adjacent Gregg binary, with an explicit compatibility check. Never shell-execute an unverified PATH lookup or run `gregg-gui daemon run` as a fallback.
- Packaging may include the existing `gregg` binary but must **not** introduce a second daemon implementation. When `gregg` is separately upgraded, enforce local protocol compatibility; an incompatible/newer active daemon must receive actionable guidance, not a destructive silent downgrade.
- Name release/install assets unambiguously to avoid replacing the existing `gregg` or `greggd` executable accidentally. A GUI-only install must either bundle its required Gregg helper or clearly require an existing Gregg installation; choose and test exactly one default operator path.
- Keep manual releases and crates.io publication policy intact; no GUI publish/CI automation added until the required platform evidence exists. Preserve existing CLI self-update/uninstall ownership semantics.
- Ensure Slint royalty-free attribution is accessible by one documented compliant mechanism (e.g., visible AboutSlint through About menu) and third-party notice/asset attribution requirements are met. Verify license choice against the exact shipped version.
- Provide packaging and getting-started docs, GUI limitations, tray support, local-daemon lifecycle instructions, and a release-oriented smoke checklist. Record the decision if GUI remains source-build-only in this phase.

## Performance qualification

Use fat-LTO/stripped release builds consistent with the repository's measurement discipline, report raw unstripped/stripped binary size, package download size (if packaged), cold/warm startup latency, tray-only idle RSS, memory with a small window open, idle CPU and timer wakeups, and network request counts on a 3-host and a 25-host synthetic fleet. Attribute per-process cost separately: `gregg client daemon` vs `gregg-gui`, not one combined total.

Compare (a) daemon alone, (b) daemon + TUI, (c) daemon + GUI tray-only, and (d) daemon + GUI monitor visible. Report OS and method, baseline and delta rather than extrapolating across platforms. A resource-budget decision (accept/rework/defer) must be recorded before signing off: the explicit goal is a low-resource passive tray utility, not visual parity at any cost.

In particular, `FrameStream::next` has a 10 ms no-header polling sleep and daemon connections use a 20 ms input poll. Measure timer wakeups. If the cost is material in a tray-resident process, open a **narrow, independently tested** readiness/notification optimization; do not replace the whole IPC architecture, and do not count UI redraw loops as a substitute for event-driven snapshot delivery.

Do not introduce any SBC source-compilation performance gate; normal supported desktop toolchains are the targets for this GUI line.

## Closure gates

- [ ] Counted same-config multi-frontend integration proves identical remote request budgets and bounded cron/EggPool control behavior.
- [ ] GUI is offline-honest and retains optional/unsupported semantics, bounded strings/history, and terminal-output sanitization.
- [ ] Native macOS and Windows acceptance and Linux desktop/tray/fallback acceptance are recorded; missing real-run evidence stays unchecked and blocks shipment for that target.
- [ ] Builds/tests and workspace Clippy remain green for unchanged core packages, including Rust 1.89 MSRV tests; GUI newer-Rust build verified independently.
- [ ] No remote `greggd` schema/API or daemon-owned poller changes; no reintroduced TUI direct polling or duplication of the daemon's logic.
- [ ] No unauthorized launcher/foreign-peer behavior, user-local IPC exposure, hidden replacement, or incorrect daemon downgrade on mismatch.
- [ ] GUI-asset name, binary ownership, bundle/installer layout, startup modes and update compatibility are documented and tested.
- [ ] Slint licensing/attribution implementation is verified against distributed artifacts.
- [ ] Native footprint measurements and an accept/rework/defer decision are recorded. Original `greggd` and core Gregg release sizes must not be charged with optional GUI dependencies.
- [ ] Update `plans/README.md` roadmap and closure status, `architecture/`, README/user docs, relevant skill, `AGENTS.md` and `CHANGELOG.md` with concrete evidence; do not rewrite historical completed plans.

## Stop conditions

If Slint's native tray proves unreliable on a target or the GUI idle cost is excessive, stop that target's release and document measured evidence plus a narrow fallback (including a tray-only native-menu alternative if justified). Do not substitute a webview, new worker, or broad GUI rewrite without a new explicitly approved plan. This is a bounded optional frontend line and never overrides the headless monitor's correctness.
