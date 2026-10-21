# Plan 188: Gregg GUI daemon launch and isolated Slint build boundary

Status: planned. Implementation not started.

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

- [ ] Legacy TUI `ensure_running` still launches the current `gregg` executable and attaches; existing tests remain green.
- [ ] Separate GUI executable selects and launches the real `gregg` daemon entry, not itself; daemon stays alive when the GUI closes.
- [ ] Two simultaneous TUI/GUI starts against the same config converge to one daemon (lock/reprobe race).
- [ ] Different config paths produce distinct daemons; symlink/normalized equivalent paths remain same identity.
- [ ] Missing helper, invalid path, inaccessible executable, foreign peer, same-config older/newer protocol, wrong config, and refused handshake are classified without destructive fallback. Recheck under the lock is exercised.
- [ ] Windows path-with-spaces argv and native named-pipe behavior are tested, not only compiled on Linux.
- [ ] Root workspace `cargo test --workspace --all-targets --all-features` on Rust 1.89 and existing Linux/Windows Clippy checks remain core-only and unchanged; the GUI build works with the documented newer compiler.
- [ ] Slint minimum feature graph, attribution method, toolchain requirement and the selected integration API are recorded with evidence.
- [ ] No change in `greggd` compile graph, binary, networking or polling cadence; no second remote HTTP client is instantiated.

## Stop conditions and handoff

If a separate root manifests / path dependency cannot isolate Rust 1.89 under real Cargo resolution, stop and document the smallest tested alternative before adding Slint to the root workspace. If launch extraction would weaken foreign-peer or version protections, stop rather than add a second launcher. No new GUI feature is allowed to require remote-polling access.

After the boundary compiles, Plan 189 may implement the GUI directly against `FrontendSnapshot`/`FrontendSender` while Plan 190 owns qualification and delivery.
