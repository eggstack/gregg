# Plan 188: Gregg GUI daemon launch and isolated Slint build boundary

Status: **complete**. Implemented and closed; see the closure record at the end
of this file and the scope record in `plans/README.md`.

Depends on: Plan 187 and completed Gregg client-daemon Plans 164-172. Unblocks Plan 189. Independent of Plan 091.

## Objective

Make the existing client-daemon frontend interface safely usable by a **separately named** `gregg-gui` executable and create an optional Slint build target without changing the core Rust 1.89 workspace contract or introducing another poller.

## Root causes to address

1. `crates/gregg/src/clientd/launch.rs::ensure_running` calls `spawn_daemon`, which uses `std::env::current_exe()` to run `--config PATH daemon run`. This is correct for a TUI executable but wrong for a GUI executable invoking the same library.
2. `crates/gregg/Cargo.toml` unconditionally depends on Ratatui/Crossterm and exposes TUI modules through `lib.rs`; linking the GUI to this crate is operationally simple but the cost must be measured before refactoring.
3. Workspace MSRV is 1.89 while Slint 1.18 requires >=1.92. Adding the GUI as a normal root workspace member would make `cargo test --workspace --all-targets --all-features` under the existing MSRV job fail even if core code is untouched.
4. The daemon speaks a same-host, versioned IPC protocol, *not* a stable third-party public protocol; differently updated GUI and Gregg binaries must negotiate/refuse incompatibility rather than guess.

## Implementation path

### A. Explicit launcher without replacing existing lifecycle

- Introduce a narrow, documented daemon-binary selection boundary, such as `ensure_running_with_executable(store, daemon_exe: &Path)` or an equivalent injected launcher. Keep `ensure_running(store)` as a backward-compatible wrapper that uses `current_exe()` for the TUI. No duplicate copy of `probe`, `classify`, `rotate`, `wait_ready` or the lock implementation.
- Pass the **exact resolved executable path** to `spawn_daemon`, retaining `--config` identity and `daemon run` arguments and current stdin/stdout/stderr detachment. Avoid global PID files, process-name matching and unaudited PATH execution.
- GUI resolution priority: explicit path for local development/testing; an installer-owned adjacent or bundled Gregg executable; any user-selected external executable only after positive identity/compatibility checks. Missing/unusable executable must produce an actionable diagnostic, not secretly spawn `gregg-gui daemon run` or start direct remote polling. A bundled helper is allowed as packaging, not as an extra polling process.
- Preserve the preflight attach and second probe **under the config-specific launch lock**. Only absent endpoints authorize spawning. Foreign endpoints are never replaced; a newer owned protocol is never downgraded by an older frontend; retain the existing directional rotation guard when a proven compatible client executable is available. Do not stop a known working daemon merely because the GUI packaged the wrong helper.
- Introduce any needed executable-path selection tests with a fake launcher or fixture executable. Keep process invocation argv-only and path quoting safe on Unix and Windows. Do not silently change per-user startup, update/uninstall or config mutation semantics.

### B. Optional GUI toolchain boundary

- Create `apps/gregg-gui/` (or another clearly documented optional desktop app root) with an **independent Cargo workspace and lockfile**, rather than making Slint an ordinary member of the Rust 1.89 workspace. Document `cargo build --manifest-path apps/gregg-gui/Cargo.toml` using a newer compiler and the existing root commands for the five core packages. Add root workspace `exclude` only if needed to make Cargo ownership unambiguous.
- Depend on the existing `gregg` library via a path dependency for local IPC and normalized DTOs. Do not introduce `gregg-gui` into the main crate publish order; it is an optional app artifact.
- Introduce only the minimum Slint 1.18.x features needed (e.g. Winit + software renderer + system-tray + the matching `compat-1-18` feature), lock both `slint` and `slint-build` to compatible versions, and record resolved feature/dependency tree.
- Smoke-build a tray-only application and a simple window on Linux, macOS, and Windows when the native runners are available. Confirm `SystemTrayIcon` does not require a window and that tray/window are distinct top-level components. A Linux headless CI job must not be mistaken for a desktop tray runtime proof.
- Select and record the Slint licensing path and a visible, compliant attribution surface for Plan 189. Do not introduce a GPL dependency into the MIT release by accident.

### C. Preserve core dependency and API stability

- First prototype with `gregg` as a library; record the GUI's actual linker/runtime cost. Do **not** create a new shared `gregg-client` crate or make Ratatui/Crossterm optional as speculative optimization.
- If measurements show unacceptable TUI dependency drag, write a separate justified extraction plan that preserves the public `gregg` library API and TUI behavior. Extraction must be a review decision, not an implicit prerequisite to Plan 189.
- Keep local `clientd/protocol.rs` types out of `gregg-protocol`, which is the remote wire contract. No change to `greggd` or remote HTTP API.

## Regression matrix (deterministic where possible)

- [x] Legacy TUI `ensure_running` still launches the current `gregg` executable and attaches; existing tests remain green. — `ensure_running` is now literally `ensure_running_with_executable(store, &DaemonExecutable::Current)`, so there is one launcher rather than two; `clientd::launch` has 25 green tests.
- [x] Separate GUI executable selects and launches the real `gregg` daemon entry, not itself; daemon stays alive when the GUI closes. — `DaemonExecutable::Path` refuses a path that canonicalizes to `current_exe()`; the daemon is detached and outlives the GUI by construction. The GUI's own use of it is Plan 189's work and is still marked in `apps/gregg-gui/src/bridge.rs`.
- [x] Two simultaneous TUI/GUI starts against the same config converge to one daemon (lock/reprobe race). — unchanged: the preflight probe, the config-specific launch lock, and the re-probe *and re-classify* under the lock are shared, not duplicated. `two_concurrent_launches_produce_exactly_one_daemon` remains green.
- [x] Different config paths produce distinct daemons; symlink/normalized equivalent paths remain same identity. — unchanged; `a_daemon_started_for_one_config_is_invisible_to_another` remains green.
- [x] Missing helper, invalid path, inaccessible executable, foreign peer, same-config older/newer protocol, wrong config, and refused handshake are classified without destructive fallback. Recheck under the lock is exercised. — added `a_missing_executable_is_reported_with_its_path_instead_of_a_readiness_timeout`, `a_directory_is_not_a_usable_executable`, `a_file_without_the_execute_bit_is_refused`, `an_unusable_executable_is_reported_instead_of_a_readiness_timeout`; the foreign-peer, version-direction and refused-handshake classifications are unchanged and green.
- [ ] Windows path-with-spaces argv and native named-pipe behavior are tested, not only compiled on Linux. — **Not satisfied by this plan.** The launcher's argv construction is unchanged (`--config <path> daemon run`, argv-only, no shell), but a *native Windows* run is required and only the Windows CI job and a Windows host can provide it. Left unchecked deliberately: see the closure record.
- [x] Root workspace `cargo test --workspace --all-targets --all-features` on Rust 1.89 and existing Linux/Windows Clippy checks remain core-only and unchanged; the GUI build works with the documented newer compiler. — the root workspace lists `apps/gregg-gui` under `exclude` and `cargo metadata` shows only the five core members; the GUI is its own workspace with `rust-version = "1.92"`.
- [x] Slint minimum feature graph, attribution method, toolchain requirement and the selected integration API are recorded with evidence. — recorded in `apps/gregg-gui/README.md` and `apps/gregg-gui/dependency-tree.txt`; the attribution surface itself is Plan 189's deliverable.
- [x] No change in `greggd` compile graph, binary, networking or polling cadence; no second remote HTTP client is instantiated. — `greggd`'s dependency graph, its stripped release binary and its HTTP surface are untouched; the GUI opens no remote HTTP client at all.

## Stop conditions and handoff

If a separate root manifests / path dependency cannot isolate Rust 1.89 under real Cargo resolution, stop and document the smallest tested alternative before adding Slint to the root workspace. If launch extraction would weaken foreign-peer or version protections, stop rather than add a second launcher. No new GUI feature is allowed to require remote-polling access.

After the boundary compiles, Plan 189 may implement the GUI directly against `FrontendSnapshot`/`FrontendSender` while Plan 190 owns qualification and delivery.

## Closure record

Implemented and closed on 2026-10-21.

### What landed

**Part A — the launcher boundary.** `crates/gregg/src/clientd/launch.rs` gained
`DaemonExecutable` (`Current` | `Path`) and `ensure_running_with_executable`.
`ensure_running(store)` is now exactly
`ensure_running_with_executable(store, &DaemonExecutable::Current)`, so the TUI
keeps its own-image behavior and `probe`, `classify`, `rotate`, `wait_ready`
and the launch lock are **shared**, never copied.

The three properties that matter:

1. **The exact resolved path is spawned.** `spawn_daemon` takes a `&Path`, the
   path is made absolute first, and there is no `PATH` lookup, no PID file and
   no process-name matching.
2. **Resolution is proved before anything destructive, and the failure is
   raised only where a spawn is authorized.** A daemon that is already serving
   needs no helper, so an unusable helper can neither stop nor refuse it; a
   daemon that *is* replaced must be replaceable, so the failure is reported
   *before* the stop.
3. **A frontend may not launch itself.** `DaemonExecutable::Path` refuses a path
   that canonicalizes to `current_exe()`. `Current` is exempt, because for the
   `gregg` TUI its own image is the right binary.

`verify_gregg_client_executable` adds the bounded positive identity check
(`<exe> version` must answer `gregg <version>`, ten-second deadline, the child
is killed and reaped on the deadline) and `adjacent_daemon_executable` finds
the installer-owned client beside a frontend.

**A latent defect was corrected in the branch being rewritten.** The rotation
branch used to stop the stale daemon and then *attach* — to the daemon it had
just stopped, which can only fail, leaving the operator with "the client
daemon could not be reached" for a daemon that had been killed and never
replaced. Rotation is now one operation (`rotate_and_replace`): stop, spawn
the resolved executable, wait for readiness. `a_rotation_stops_the_stale_daemon_and_starts_a_replacement`
locks the new shape. The branch was unreachable in practice only because
`PROTOCOL_VERSION` has never been bumped.

**Part B — the isolated GUI build boundary.** `apps/gregg-gui/` is an
independent Cargo workspace with its own `Cargo.lock`, its own
`rust-toolchain.toml`, and `rust-version = "1.92"`. The root workspace lists
it under `exclude`, and `cargo metadata` confirms the root workspace still has
exactly its five core members, so the Rust 1.89 MSRV CI job never sees Slint.
Slint is pinned to `=1.18.1` with the minimal feature set `std`,
`backend-winit`, `renderer-software`, `software-renderer-systemfonts`,
`system-tray`, `compat-1-18`; `default-features = false` keeps the femtovg
OpenGL renderer, skia, wgpu, Qt and accessibility out. The Slint UI defines a
`Window` and a `SystemTrayIcon` as **separate** top-level components, the tray
containing exactly one `Menu`, and the tray does not require a window.

**Part C — the measured cost.** The GUI links the whole `gregg` library
(Ratatui and Crossterm come along transitively), exactly as the plan
prescribes, and the cost was measured rather than guessed:

| Binary | Stripped release size |
| --- | --- |
| Slint-only baseline (same features and `.slint`, no `gregg`) | 8,813,296 bytes |
| `gregg-gui` (Slint + the `gregg` library) | 9,561,232 bytes |
| **Delta attributable to `gregg` and its transitive dependencies** | **747,936 bytes (+7.8%)** |
| `gregg` (the TUI) | 5,224,096 bytes |
| `greggd` | 3,356,800 bytes |

The drag is measured and bounded, so no extraction plan is required. The
`gregg` library's public API and the TUI's behavior are unchanged, and no
`gregg-client` crate was created.

### The one thing that did not go as planned

**The dependency graph could not resolve at all.** `ratatui` 0.29 — pulled in
transitively by the `gregg` path dependency — pins `unicode-width =0.2.0`
*exactly*, while Slint's build-time compiler reaches `annotate-snippets` 0.12.9,
which requires `unicode-width ^0.2.2`. Cargo keeps at most one version of a
crate per semver-compatibility range, so the whole graph failed to resolve.

This is **structural**, not a mistake in the pin: `slint` 1.18.1 depends on
`slint-macros` unconditionally, and `slint-macros` depends on
`i-slint-compiler`, so the compiler — and therefore `annotate-snippets` — is in
the graph even for a runtime-only build. Slint 1.16.1, 1.17.1, 1.18.0 and
1.18.1 all conflict the same way. Nor is it fixed by moving `ratatui`: 0.29.0,
0.29.1-alpha.0 and every 0.30.0-alpha pin `unicode-width =0.2.0` as well.

The three candidate resolutions, and why one was chosen:

- **Make Ratatui/Crossterm optional in `crates/gregg`.** This is the cleanest
  end state, but the plan explicitly forbids making them optional as a
  speculative optimization, and extraction "must be a review decision, not an
  implicit prerequisite to Plan 189". It is also a change to the core crate's
  public surface, which this line must not make.
- **Drop the `gregg` library dependency from the GUI.** The plan requires the
  path dependency, precisely so the GUI renders the *real* reducer's output
  instead of a parallel implementation of it. A second protocol client would
  be protocol drift.
- **Vendor `annotate-snippets` 0.12.9 with one relaxed dependency bound.**
  Chosen.

The vendored copy is byte-identical to the crates.io release in every source
file (verified by diff against the upstream `.crate` tarball), keeps its
`LICENSE-MIT` and `LICENSE-APACHE` files, keeps version `0.12.9`, and changes
exactly one thing: `unicode-width` is relaxed from `^0.2.2` to `^0.2`, so the
graph unifies on `unicode-width 0.2.0`. The only `unicode-width` API the crate
uses is `UnicodeWidthChar::width(ch) -> Option<usize>`, which exists unchanged
in 0.2.0. The patch is **build-time only** and adds nothing to the shipped
binary, and it is scoped to the GUI workspace, so the root workspace's graph
and lockfile are untouched. The rationale, the rejected alternatives and the
re-vendoring procedure are in `apps/gregg-gui/vendor/VENDORED-PATCH.md`.

**When this must be revisited:** if the core crate ever stops depending on
Ratatui 0.29 — through the extraction plan this line deliberately did not
write — the vendored patch can be deleted and the GUI goes back to plain
crates.io. The trigger is stated here so the vendored copy does not become a
silent permanent.

### Honest gaps

- **The Windows native item is unchecked.** The launcher's argv construction is
  unchanged and argv-only, but "not only compiled on Linux" is a native-Windows
  requirement, and this plan was closed on a macOS host. It stays unchecked
  rather than being claimed.
- **The default local check does not pass on this host, and did not before this
  plan either.** `cargo test --workspace --all-targets --all-features` fails
  15-16 tests on a clean checkout of the same commit: ten `eggpool` worker
  tests and two `scheduler` fleet-scaling tests, plus
  `clientd::ipc::tests::a_config_inside_the_temp_directory_yields_one_location`.
  These were confirmed on an untouched worktree at the same commit, so they
  are **pre-existing and unrelated to this plan**. Two distinct causes:
  the `ipc` test hardcodes `/tmp` while the code uses `std::env::temp_dir()`,
  so it fails on any host where the temp directory is not `/tmp` (this host
  sets `TMPDIR`); and the `eggpool`/`scheduler` tests combine tokio's
  `start_paused` auto-advance with real loopback TCP I/O, so a loaded machine
  advances virtual time past the connect before the request is issued. The
  upstream CI run for this commit is green on all jobs. None of these were
  fixed here: they are outside this line's scope, and "fixing" them by
  changing the tests would have masked a real environment sensitivity.

### Evidence

- `cargo test -p gregg --all-features -- clientd::launch` — 25 passed, 0 failed.
- `cargo clippy -p gregg --all-targets --all-features -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `cargo test --manifest-path apps/gregg-gui/Cargo.toml` — 4 passed, 0 failed.
- `cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets --all-features -- -D warnings` — clean.
- `cargo build --manifest-path apps/gregg-gui/Cargo.toml` and `--release` — both succeed, zero warnings.
- `cargo metadata --no-deps` — the root workspace still has exactly its five core members.
- `cargo build --release -p gregg -p greggd` — 5,224,096 and 3,356,800 bytes stripped.

