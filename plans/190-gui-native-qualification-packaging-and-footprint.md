# Plan 190: GUI multi-frontend correctness, native qualification and packaging

Status: **complete, with a corrective pass on 2026-10-21** — the Windows
compilation break and the missing no-tray fallback are fixed, and the GUI
workspace now has its own CI; see the corrective-pass section and the closure
record at the end of this file, and the scope record in `plans/README.md`. The
**interactive** native acceptance for Windows and Linux is still **not
performed**, so those two targets remain **not shippable** on the strength of
this plan, and the GUI stays a source build.

Depends on Plans 188-189; closes Plan 187. Independent of Plan 091.

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

- [x] Counted same-config multi-frontend integration proves identical remote request budgets and bounded cron/EggPool control behavior. — `apps/gregg-gui/src/evidence.rs` runs the **real** client daemon against two counted in-process remotes (one scheduler-capable, one answering `404` for the scheduler routes — an old daemon), with a terminal-style frontend and the GUI's `Bridge` attached to the same config. The recorded table is in `apps/gregg-gui/EVIDENCE.md`: `/v2/status` is flat at 4 polls per 4-second window in every phase (daemon alone, with the TUI, with the TUI **and** the GUI, and after both frontends closed), the scheduler summary and history are fetched at their own cadence and never multiply, opening a cron job grew the **local IPC document** 1903 → 2276 bytes while the remote's history fetch stayed at 1, and the `EggPool` pane drove **one** daemon-owned worker on the converged window (`7d` asked for, `30d` never) that stopped asking the remote when the last pane closed.
- [x] GUI is offline-honest and retains optional/unsupported semantics, bounded strings/history, and terminal-output sanitization. — 54 deterministic GUI tests, including the projection's pending-is-not-offline and absent-is-`—` rules, the unsupported scheduler family, the bounded truncation on a character boundary, the remote-truncation-reported-separately rule, and the caret-notation escaping of remote control characters.
- [ ] Native macOS and Windows acceptance and Linux desktop/tray/fallback acceptance are recorded; missing real-run evidence stays unchecked and blocks shipment for that target. — **macOS is recorded** (see the closure record). **Windows and Linux are NOT performed**: this plan was closed on a macOS host, a Windows run needs a Windows host, and a Linux tray run needs a desktop session with a `StatusNotifierItem` host. Those two stay unchecked rather than being claimed, and they **block shipment for Windows and Linux** — which is consistent with the packaging decision below: the GUI is a source build and is not shipped for any target yet.
- [x] Builds/tests and workspace Clippy remain green for unchanged core packages, including Rust 1.89 MSRV tests; GUI newer-Rust build verified independently. — the root workspace still has exactly its five core members and its Rust 1.89 MSRV job never sees Slint; the GUI is verified separately on its own toolchain.
- [x] No remote `greggd` schema/API or daemon-owned poller changes; no reintroduced TUI direct polling or duplication of the daemon's logic. — `git diff` over `crates/greggd` and `crates/gregg-protocol` is empty; `greggd`'s stripped release binary is byte-for-byte the same size as before this line (3,356,800 bytes), and the GUI opens no network connection of its own.
- [x] No unauthorized launcher/foreign-peer behavior, user-local IPC exposure, hidden replacement, or incorrect daemon downgrade on mismatch. — the 25 launcher tests in `crates/gregg/src/clientd/launch.rs` are unchanged and green; the harness adds `the_gui_refuses_an_incompatible_daemon_without_stopping_it` and `distinct_config_paths_reach_distinct_daemons_and_one_spelling_reaches_one`.
- [x] GUI-asset name, binary ownership, bundle/installer layout, startup modes and update compatibility are documented and tested. — the decision is **source-build-only**: there is no GUI release asset, so there is no asset-naming collision with `gregg` or `greggd` and no bundle or installer layout to own. The binary is named `gregg-gui`, it requires an existing Gregg installation, and the exact resolution order (explicit `--gregg` with an identity probe, then an installer-owned adjacent client) is documented and tested in `apps/gregg-gui/README.md`.
- [x] Slint licensing/attribution implementation is verified against distributed artifacts. — the licence path is `LicenseRef-Slint-Royalty-free-2.0`, not GPL; the attribution text is **present in the shipped release binary** (verified with `strings` on `apps/gregg-gui/target/release/gregg-gui`), and it is reachable in the application through the `About` button and the tray's `About Gregg` item.
- [x] Native footprint measurements and an accept/rework/defer decision are recorded. Original `greggd` and core Gregg release sizes must not be charged with optional GUI dependencies. — the sizes, the per-process resource numbers and the **accept** decision are in the closure record. `gregg` (5,224,096 bytes) and `greggd` (3,356,800 bytes) are unchanged and carry **none** of the GUI's cost; the GUI's own footprint is measured against a purpose-built Slint-only baseline.
- [x] Update `plans/README.md` roadmap and closure status, `architecture/`, README/user docs, relevant skill, `AGENTS.md` and `CHANGELOG.md` with concrete evidence; do not rewrite historical completed plans. — done in the same pass; Plans 187-189's records were not rewritten.

## Stop conditions

If Slint's native tray proves unreliable on a target or the GUI idle cost is excessive, stop that target's release and document measured evidence plus a narrow fallback (including a tray-only native-menu alternative if justified). Do not substitute a webview, new worker, or broad GUI rewrite without a new explicitly approved plan. This is a bounded optional frontend line and never overrides the headless monitor's correctness.

## Closure record

Implemented and closed on 2026-10-21, on a macOS host (Apple M4 Pro, macOS
26.0.1, `rustc`/`cargo` 1.99.1 stable, `aarch64-apple-darwin`).

### The blocking gate: the counted multi-frontend proof

`apps/gregg-gui/src/evidence.rs` runs the **real** client daemon
(`gregg::clientd::daemon::run_daemon`, not a stub) against **two counted
in-process remotes**: one serving `/v2/status`, `/v2/scheduler` and
`/v2/scheduler/history`, and one answering `404` for the scheduler routes —
a healthy `greggd` from before the scheduler plane. A terminal-style frontend
(attached through the same public client-daemon API the TUI uses) and the
GUI's real `Bridge` attach to the same config. The recorded artifact is
`apps/gregg-gui/EVIDENCE.md`.

Observed over four-second windows (`refresh_seconds = 1`), on both remotes:

| remote | route | daemon alone | with TUI | with TUI + GUI | after both frontends closed |
| --- | --- | --- | --- | --- | --- |
| modern | `/v2/status` | 4 | 4 | 4 | 4 |
| modern | `/v2/scheduler` | 0 | 0 | 0 | 0 |
| modern | `/v2/scheduler/history` | 0 | 0 | 0 | 0 |
| old | `/v2/status` | 4 | 4 | 4 | 4 |
| old | `/v2/scheduler` | 0 | 0 | 0 | 0 |
| old | `/v2/scheduler/history` | 0 | 0 | 0 | 0 |

Three further observations from the same harness:

- **The cron intent grows the local document, not the remote budget.** Opening
  a cron job in the GUI grew the local IPC document 1903 → 2276 bytes while the
  remote's history-fetch counter stayed at exactly the one discovery fetch.
- **The `EggPool` pane drives one daemon-owned worker.** The GUI asked for the
  week window and the terminal-style frontend for the month window; the remote
  was asked for `7d` three times and for `30d` **never**, and the request
  stream stopped when the last pane closed.
- **The daemon outlives its frontends.** It survives their closing, keeps its
  cadence, and hands a reattaching frontend the newest document without
  refetching the scheduler history.

### The native 25-host measurement

The same claim, measured on this host against 25 synthetic endpoints with the
real daemon and the real GUI binary, with the measurement window **aligned to
the daemon's own start** so every configuration is sampled over the same
phase of the poll cadence:

| configuration | window | `/v2/status` | `/v2/scheduler` | total remote requests |
| --- | --- | --- | --- | --- |
| daemon alone | 61.0 s | 375 | 39 | 414 |
| daemon + GUI, tray only | 60.6 s | 375 | 39 | 414 |
| daemon + GUI, monitor visible | 60.6 s | 375 | 39 | 414 |

**Identical.** The GUI adds nothing to the remote request budget on a
25-host fleet, whether it is a tray icon or a visible window.

An earlier, unaligned run of the same measurement showed 325 status requests
for the daemon alone against 375 for the two GUI configurations. That was a
**phase artifact** — the window started at a different point of the five-second
cadence — and aligning the window to the daemon's start removed it completely.
The aligned numbers are the ones above; the unaligned ones are recorded here
because "the first measurement disagreed with the second" is exactly the kind
of thing a reader needs to know about a measurement.

### Footprint

Release builds with the repository's own profile (`lto = "fat"`,
`codegen-units = 1`, `strip = "symbols"`), so the numbers are comparable with
the TUI's.

| binary | stripped size |
| --- | --- |
| `gregg-gui` | 9,776,560 bytes (9.3 MiB) |
| Slint-only baseline (same features, same `main.slint`, same activation code, no `gregg`) | 8,929,216 bytes (8.5 MiB) |
| **difference** | **847,344 bytes (+9.5%)** |
| `gregg` (the TUI) | 5,224,096 bytes — **unchanged** |
| `greggd` | 3,356,800 bytes — **unchanged** |

The original `gregg` and `greggd` release sizes carry **none** of the GUI's
cost: `git diff` over `crates/greggd` and `crates/gregg-protocol` is empty, and
`greggd`'s stripped binary is the same size it was before this line.

There is no package download size to report, because there is no package: the
GUI is a source build.

### Resource cost, and the decision

Measured per process, on this host, with `getrusage` (microsecond CPU
resolution; macOS's `ps` has one-second resolution and no context-switch
counter). The window is aligned to the daemon's start, and the numbers below
are the **whole process lifetime** (about 73 seconds, startup included):

| configuration | remote requests | daemon RSS | daemon CPU | daemon wakeups | GUI RSS | GUI CPU | GUI wakeups |
| --- | --- | --- | --- | --- | --- | --- | --- |
| daemon alone | 414 | 7.2 MB | 0.111 s | 8,279 | — | — | — |
| daemon + GUI, tray only | **414** | — | 0.157 s | 11,471 | 73–83 MB | 0.491 s | 22,884 |
| daemon + GUI, monitor visible | **414** | — | 0.154 s | 11,333 | 72–82 MB | 0.476 s | 21,985 |

"CPU" is seconds of CPU time; "wakeups" is the count of voluntary context
switches, which is the observable proxy for a sleep/wake cycle. The GUI's CPU
is about **0.65% of one core** (0.49 s over ~73 s), and it is the same
whether the window is hidden or visible.

The resource budget decision is **accept**, and the reasoning is:

1. **The remote budget is unchanged.** That is the property the whole
   architecture exists to protect, and it is proven twice — deterministically in
   the harness and natively on 25 hosts.
2. **The CPU cost is a fraction of one core.** The GUI used 0.49 seconds of
   CPU over a 73-second lifetime — about 0.65% of one core — while the daemon
   used 0.11 to 0.16 seconds for the same period and the same fleet. Both are
   the same order of magnitude, and neither is a busy loop: the GUI is blocked
   on a socket read and on the window system.
3. **The wakeups are the ones the code already documents.** The daemon's
   voluntary context switches rise from 8,279 alone to about 11,400 with a
   frontend attached — a delta of about 3,200 over 73 seconds, which is
   44 per second and matches the "one timer wake per attached frontend every
   20 ms" that `clientd/daemon.rs` documents for `READ_POLL_INTERVAL`. The
   GUI's own wakeups are dominated by the frontend's shared 10 ms no-header
   poll in `FrameStream::next`, which the TUI already pays. **The GUI does not
   introduce a new kind of cost**; it adds one more frontend to a cost the
   architecture already accepted.
4. **The resident memory is Slint's, not the protocol's.** Hiding the monitor
   window saves under a megabyte (the two configurations differ by about
   0.5 MB); the rest is the software renderer and the system-font machinery. A
   tray-only GUI still pays it.

The measurement was noisy enough to matter, and the noise is reported rather
than smoothed away: RSS at the moment of sampling varied between 72 MB and
83 MB for the same configuration, and an A/B attempt to compare a
current-thread and a multi-thread bridge runtime was **inconclusive** — the
two builds measured closer together than two runs of the *same* build measured
apart. The runtime shape was therefore left unchanged. A speculative change
that the measurement cannot distinguish from noise is not a change.

**What was *not* done, deliberately.** The obvious lever for the wakeups is
the frontend's 10 ms no-header poll in `crates/gregg/src/clientd/frontend.rs`,
and it was **not** touched: it is core behaviour shared with the TUI, this line
must not reopen the headless client's correctness, and the effect could not be
distinguished from this host's measurement noise. Opening that work means a
new, explicitly approved plan with its own acceptance — not a quiet change
inside a qualification plan.

**When this must be revisited:** if the GUI is ever shipped as a long-lived
tray utility and its resident memory becomes a complaint, the lever is the
renderer — a GPU renderer, or a native tray-only front end — and **not** the
IPC architecture. The request budget and the wakeup structure are not the
problem, and changing them would trade a proven property for an unmeasured
one.

### The packaging decision

**The GUI is source-build-only in this phase. No installer, no bundle, no
release asset.**

Shipping an installer or a `.app` bundle would put an unsigned, un-notarized
binary in front of operators, and macOS Gatekeeper would quarantine it — so
the first thing a user would meet would be a warning dialog. The
cross-platform evidence a shipping decision needs (a native Windows run, a
Linux desktop session with a `StatusNotifierItem` host) does not exist yet
either. The decision is recorded in `apps/gregg-gui/README.md`, and it has
three consequences that are all testable:

- **One operator path: the GUI requires an existing Gregg installation.** It
  does not bundle a second `gregg` and it does not start a second daemon. The
  resolution order is an explicit `--gregg <path>` that must prove it is a
  Gregg client by answering `gregg <version>` to a bounded probe, then an
  installer-owned client beside the GUI, then an actionable diagnostic. There
  is no fourth case: it never launches itself, never does a `PATH` lookup and
  never polls the fleet directly.
- **No asset-name collision.** The binary is `gregg-gui`, distinct from both
  `gregg` and `greggd`, and there is no GUI release asset, so nothing can
  overwrite an existing installation.
- **No downgrade.** The GUI negotiates the local protocol version at attach
  time and reports an incompatible daemon as *incompatible*, with guidance.
  It is never downgraded and never silently replaced. The direction rules live
  in the core launcher and are unchanged.

### Honest gaps

- **The Windows-native and Linux-native acceptance are not performed.** The
  native matrix is the part of this plan that could not be completed on this
  host: a Windows run needs a Windows host (the repository's own rule is that
  only a native Windows run is authority), and a Linux tray run needs a
  desktop session with a `StatusNotifierItem` host — plus the "no SNI host"
  fallback case, which needs a plain GNOME session. Nothing about those two
  targets is claimed. **They block shipment for Windows and Linux**, which is
  consistent with the packaging decision: the GUI is not shipped for any
  target yet, and the source build is the only distribution.
  **Read this with the corrective pass above:** "not performed" described the
  *evidence*, not the *code*. The Windows build was broken and the no-tray
  fallback was missing; both are fixed now, and the GUI's own CI now covers
  the two targets it can cover without an interactive desktop. What is still
  outstanding is the **interactive** native run — a real Windows shell with a
  visible notification area, and a real Linux desktop session — which is still
  not claimed.
- **The macOS matrix is a partial one.** It was exercised on Apple Silicon
  only; the **Intel** build was not produced, because the runner is not
  available. Retina scaling was observed (the menu bar and the window render
  correctly at 2×), but there is no explicit DPI measurement recorded.
  Unsigned-`.app` quarantine expectations are therefore **not** documented —
  there is no `.app` to quarantine.
- **The default local check does not pass on this host, and did not before this
  plan either.** `cargo test --workspace --all-targets --all-features` fails
  14-16 tests on an untouched worktree at commit `eb45029`: the `eggpool`
  worker tests, the `scheduler` fleet-scaling tests, and
  `clientd::ipc::tests::a_config_inside_the_temp_directory_yields_one_location`.
  Re-verified during this plan on that clean worktree: the `ipc` test and the
  `eggpool` worker test fail there with none of this line's changes present.
  The `ipc` test hardcodes `/tmp` while the code uses `std::env::temp_dir()`,
  so it fails wherever the temp directory is not `/tmp` (this host sets
  `TMPDIR`); the `eggpool`/`scheduler` tests combine tokio's `start_paused`
  auto-advance with real loopback TCP I/O, so a loaded machine advances
  virtual time past the connect before the request is issued. The upstream CI
  run for the commit is green on every job. None of these were "fixed" here:
  they are outside this line's scope, and changing the tests to pass would
  have masked a real environment sensitivity.
- **The workspace Clippy gate does not pass on this host either, and did not
  before this plan either.** `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` fails in `gregg-host` (24 errors) and `greggd`
  (17 errors) with `clippy::borrow_as_ptr` and `clippy::too_many_lines`. Both
  are **pre-existing**: the same failures appear, with the same counts, on the
  untouched worktree at `eb45029`, and `git diff` over `crates/gregg-host` and
  `crates/greggd` is empty for this line. The cause is the **local toolchain**
  — clippy 1.99.1 has lints that the CI's clippy version does not flag — not
  the code, and not this line. `gregg`, `gregg-protocol` and `gregg-update` are
  clippy-clean here, and the GUI workspace is clippy-clean here too. These
  were not "fixed" here either: silencing a lint the CI does not even raise
  would have been a change to core crates made for no reason, in a line whose
  whole point is not to touch them.

### Corrective pass (2026-10-21)

This plan was closed as "complete, with the Windows and Linux native acceptance
recorded as not performed". That framing was **too kind to the reader**, and
the difference matters: it said those two targets were *unverified* when one of
them was **broken** and the other was **never implemented**. Both statements are
about this plan's own acceptance requirements, so both belong in the closure
record.

**1. Windows did not compile at all.** The native matrix requires a "native
MSVC compile" on Windows. The `#[cfg(windows)]` branch of
`is_executable_file` in `crates/gregg/src/clientd/launch.rs` (added by Plan
188) asked `std::fs::Metadata` for its `extensions()` — a method that does not
exist: `Path` has an extension, `Metadata` has none. The branch was never
compiled on the machine that wrote it, and it broke the **whole** `gregg` crate
on Windows, not just the GUI: the Windows CI job on this branch was **red for
every commit from `9029bc9` to `14a4b93`**, with
`error[E0599]: no method named 'extensions' found for reference '&std::fs::Metadata'`
(run `39628897811`). The GUI could not have been built for Windows either,
because it depends on that crate. "Not performed" was true; "not broken" was
not.

**2. The "no SNI host" fallback was never implemented.** The native matrix
requires, for a desktop without a `StatusNotifierItem` host, that "the
application must provide a visible/failsafe window and clear guidance, **not
hide its only control surface**". The GUI hid the monitor window on close and
had no fallback of any kind, so on such a desktop — a plain GNOME session, for
instance — closing the window left an invisible, unquittable GUI: the daemon
kept polling and the operator had nothing left to press.

Both defects were invisible for the same reason: **the GUI workspace had no CI
at all**. It is a separate Cargo workspace with its own lockfile and its own
toolchain, the root workspace excludes it, and no job built or tested it. So a
`#[cfg(windows)]` branch that nothing else compiles, and a UI requirement that
nothing asserts, could both ship unnoticed — which is the same trap the
repository's own rule warns about for Windows-only Rust.

The corrective pass therefore does four things:

- **fix the Windows branch** in `crates/gregg/src/clientd/launch.rs`: the
  executable test asks the **path** for its extension, not the metadata, and it
  applies the rule Windows itself applies when it is handed a path to run — a
  name with no extension is runnable (Windows appends `.exe`), and a name with
  an extension is runnable only when that extension is one Windows runs. The
  adjacent-client lookup now prefers `gregg.exe` over a bare `gregg` on
  Windows, because the bare name is only *found* by Windows through that same
  `.exe` appending. The behaviour is locked by **Windows-gated tests**
  (`#[cfg(windows)]`), which run on the native Windows CI job rather than on
  the machine that wrote them;
- **make the host list and the cron job list scroll**, with bounded heights, so
  a large fleet cannot push the metrics, cron and EggPool panes out of the
  fixed-size window — the previous layout grew without bound and a 25-host
  fleet pushed everything below the host list off screen;
- **implement the no-tray fallback** the matrix asked for: the GUI probes for a
  tray host (on Linux, the `org.kde.StatusNotifierWatcher` name on the session
  bus, because that is exactly what Slint's `ksni` backend requires), reports
  the result in one machine-readable line, shows the operator what is going on
  in the window, and **never hides the only control surface when there is no
  tray**: closing the window then quits the GUI instead of stranding it. The
  decision is a pure, unit-tested function, and the probe at close time is
  re-run rather than taken from a startup snapshot, because a tray host can
  appear after the GUI started;
- **give the GUI workspace its own CI**, so a Windows-only break in the code it
  depends on can never again reach a merged branch unnoticed: one job that
  builds, lints and tests the GUI workspace on macOS, Windows and Linux
  natively, plus native tray exercises on the Windows and Linux legs.

**Scope note.** The three scope records for Plans 187-190 each preserved an
exclusion of "new CI jobs, matrices, evidence bundles, uploaded artifacts,
signed releases, crates.io publication, or a release workflow". This pass
adds **one** CI job that is a **build check** — fmt, clippy, tests and a
release build of the GUI workspace, plus native tray exercises — and it adds
**no** artifact, no bundle, no release asset, no publication and no packaging
step. That is an explicitly approved change to the exclusion, recorded here
because the exclusion was a plan decision rather than a repository rule.

### Evidence

**Deterministic**

- `cargo test --manifest-path apps/gregg-gui/Cargo.toml` — **54 passed, 0
  failed**, three consecutive runs.
- `cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets
  --all-features -- -D warnings` — clean.
- `cargo fmt --manifest-path apps/gregg-gui/Cargo.toml -- --check` — clean.
- `cargo test -p gregg --all-features -- clientd::launch` — 25 passed, 0
  failed.
- `cargo clippy -p gregg --all-targets --all-features -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `cargo metadata --no-deps` — the root workspace still has exactly its five
  core members.
- `cargo +1.89 test --workspace --all-targets --all-features` — the whole
  workspace **compiles** on the root MSRV, and 967 tests pass. The 13 failures
  are the pre-existing, environment-induced ones recorded above (twelve
  `eggpool` worker tests and the `ipc` temp-directory test); the
  `scheduler` fleet-scaling tests passed on this run, which is the point: they
  fail only when the machine is loaded.

**Native (macOS, a real desktop session)**

- The tray entry is present in the menu bar, with a health-coloured icon; the
  tray menu contains exactly the fleet summary, Open Monitor, Reload, About
  Gregg and Quit Gregg.
- Open Monitor raises the window to the front, including from a background
  launch (this is what the Plan 189 activation fix made true).
- The close button hides the window; the tray and the daemon keep running.
- Reload produces an immediate poll and a scheduler summary read, then resumes
  the normal cadence.
- About Gregg renders the Slint royalty-free attribution inside the
  application; the same text is verified present in the shipped release
  binary with `strings`.
- Quit Gregg ends the GUI while the client daemon survives and keeps polling.
- The GUI process opens **no network connections of its own**.
