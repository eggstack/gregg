# Phase 073: native Windows SCM entry and readiness correction

Status: implementation complete; operational closure verified by Plan 074.

Depends on: Plan 072.

## Objective

Complete the Windows service implementation by adding the required Service Control Manager dispatcher and generated `ServiceMain` boundary, preserving the single-runtime/nonblocking shutdown work from Plan 072, and reporting `RUNNING` only after the daemon has successfully bound its listener.

This phase also makes the existing service configuration handoff and Windows lifecycle smoke truthful enough for the later CI-backed operational closure. It is a narrow Windows service closure pass, not a service-management redesign, packaging campaign, or CI expansion.

## Why this phase exists

Plan 072 correctly removed the nested Tokio runtime and blocking control receive, but the executable still launches service mode as:

```text
greggd.exe service --config <path>
```

and directly calls the worker that registers a control handler. A native Windows service process must first connect its main thread to the SCM dispatcher. The SCM then invokes a generated `ServiceMain` callback, and that callback registers the service control handler and runs the service worker.

The required control flow is therefore:

```text
Windows SCM
  -> greggd.exe service --config <path>
  -> service_dispatcher::start(SERVICE_NAME, ffi_service_main)
  -> generated ffi_service_main
  -> service_main(service_arguments)
  -> register control handler
  -> load selected config
  -> create one current-thread Tokio runtime
  -> run_with_shutdown(...)
  -> report STOPPED
```

The current implementation skips the dispatcher and `ServiceMain` layers. Compilation and unit tests cannot prove that the executable can start as a real service when those layers are absent.

Two directly related correctness issues must be addressed at the same time:

1. `main` resolves `--config`, but the Windows worker reloads `Config::default_path()` and ignores the resolved path.
2. The service reports `RUNNING` before Tokio runtime construction and before `run_with_shutdown()` binds the HTTP listener. A runtime or bind failure can therefore occur after an incorrect ready state has already been published to the SCM.

The existing `scripts/smoke-windows.ps1` is the correct bounded mechanism for operational proof, but its executable invocation and port/config expectations must be inspected and minimally corrected before it can serve as the CI-backed closure smoke.

## Authoritative implementation contract

Follow the `windows-service` crate's native service structure:

- use `define_windows_service!` to generate the low-level service entry callback;
- call `service_dispatcher::start` from the synchronous executable service branch;
- call `service_control_handler::register` from the `ServiceMain` worker, not directly from ordinary command dispatch;
- keep the main dispatcher thread under SCM control until the service callback exits.

This mirrors the Windows API contract in which `StartServiceCtrlDispatcher` connects the process to the SCM and the SCM invokes `ServiceMain`, which then registers its control handler.

Do not hand-write raw Win32 dispatcher FFI while the existing dependency provides the required API.

## Scope

### In scope

- Add the native `service_dispatcher::start` entry for `greggd service`.
- Add a generated `ServiceMain` callback using `define_windows_service!`.
- Move the current Windows service worker behind that callback.
- Preserve exactly one current-thread Tokio runtime inside the service worker.
- Preserve the Tokio one-shot Stop/Shutdown signal introduced by Plan 072.
- Pass the resolved CLI config path into the generated service callback through one small process-local launch context.
- Ensure custom `--config` paths used by service registration are honored.
- Delay the SCM `RUNNING` status until configuration, collector construction, runtime construction, and listener bind have succeeded.
- Report `STOPPED` with a nonzero exit code for service-worker failure.
- Extract a small pure control-mapping helper so Stop, Shutdown, Interrogate, unsupported controls, and duplicates are directly tested.
- Minimally correct `packaging/install-windows.ps1` so its selected config path is the path placed in the service image command.
- Minimally correct `scripts/smoke-windows.ps1` so it invokes the installed executable deterministically and asserts consistent config/port state.
- Run the existing local checks and one ordinary CI run with the existing
  Windows SCM lifecycle smoke.
- Reopen and then directly reconcile Plans 066, 072, 073, and `plans/README.md` with the demonstrated final state.

### Out of scope

- Replacing the `windows-service` crate.
- Supporting multiple services in one process.
- Dynamic service names, service hosting through `svchost`, or a service DLL.
- Pause/continue support, preshutdown controls, custom recovery policy, delayed auto-start, dependencies, or trigger-start configuration.
- Redesigning the existing `ServiceManager`/`ScmAdapter` lifecycle commands.
- Adding authentication, TLS, remote mutation, history, alerts, or monitoring features.
- Changing the protocol, TUI, collectors, scheduler, or EggPool behavior.
- Changing release profile, dependencies, MSRV, or binary-size work unless required for compilation, which is not expected.
- Adding another workflow, another Windows job, workflow artifacts, evidence bundles, or a privileged test framework.
- Running the full Windows lifecycle smoke on every ordinary local development loop.
- Creating Plan 074 merely to record closure.

## Product invariants

1. `greggd run` remains the ordinary foreground mode on Linux, macOS, and Windows.
2. `greggd service` remains hidden and Windows-only.
3. Foreground mode and Windows service mode each own exactly one current-thread Tokio runtime.
4. The shared daemon implementation remains `run_with_shutdown()` or one minimal internal wrapper around it.
5. Stop and Shutdown request graceful shutdown with distinct stable reasons.
6. Interrogate does not request shutdown.
7. Repeated Stop/Shutdown controls are harmless and nonblocking.
8. The service continues using the fixed SCM name `greggd`.
9. The configured service image path continues to carry the hidden `service` subcommand.
10. An explicitly selected service config path is the path the worker loads.
11. Errors are emitted once per failure path; service status and tracing must not duplicate console diagnostics.
12. CI remains one read-only, nonpublishing workflow.
13. Release remains manual.

## Expected files

Primary implementation files:

```text
crates/greggd/src/main.rs
crates/greggd/src/service/windows.rs
crates/greggd/src/run.rs
```

Focused tests may remain in those modules. Add a new test module only when the existing files become materially harder to read.

Directly related packaging and smoke files:

```text
packaging/install-windows.ps1
scripts/smoke-windows.ps1
```

Directly affected documentation and planning records:

```text
README.md
architecture/greggd-daemon.md
architecture/error-conventions.md       # only if service diagnostic wording changes
plans/066-bounded-correctness-and-maintainability-roadmap.md
plans/072-windows-service-runtime-and-record-correction.md
plans/073-native-windows-scm-entry-and-readiness-correction.md
plans/README.md
```

Do not edit unrelated architecture files, skills, protocol files, or release documentation.

## Implementation sequence for GPT-5.6 Luna

### Step 1: preserve synchronous command selection

Keep the Plan 072 synchronous executable boundary. Do not restore `#[tokio::main]`.

The Windows service branch in `crates/greggd/src/main.rs` should call a dispatcher entry function, not the service worker directly:

```rust
#[cfg(target_os = "windows")]
Command::Service => {
    service::windows::start_service_dispatcher(config_path)
}
```

The exact function name may differ, but the distinction must remain explicit:

```text
start_service_dispatcher  = SCM/process boundary
service_main              = SCM callback boundary
run_service_worker        = current service implementation
```

Do not hide all three responsibilities behind one ambiguous `run_service()` function.

### Step 2: add the generated ServiceMain entry

In the Windows service module, use the dependency-provided macro and dispatcher API. The intended structure is approximately:

```rust
use windows_service::{define_windows_service, service_dispatcher};

define_windows_service!(ffi_service_main, service_main);

pub fn start_service_dispatcher(config_path: PathBuf) -> Result<(), ServiceErrorType> {
    install_launch_context(config_path)?;
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn service_main(_service_arguments: Vec<OsString>) {
    let config_path = service_launch_config_path();
    if let Err(error) = run_service_worker(&config_path) {
        tracing::error!(error = %error, "Windows service exited with an error");
    }
}
```

Adapt names and concrete error mappings to the current crate. Do not copy the sketch blindly.

Requirements:

- `service_dispatcher::start` is called only after command selection and before handler registration;
- `service_control_handler::register` is called only inside the generated `ServiceMain` path;
- the worker does not call the dispatcher recursively;
- dispatcher connection failures return to `main` for existing exit-code classification;
- worker failures are reported once inside the service callback and through SCM status because the callback API cannot return them through ordinary `main` result propagation;
- no raw `StartServiceCtrlDispatcherW` FFI is added.

### Step 3: transfer the selected config path narrowly

The generated service callback is a static entry and cannot capture `main`'s resolved path. Use one small process-local launch context.

Preferred design:

```rust
static SERVICE_LAUNCH_CONFIG: OnceLock<PathBuf> = OnceLock::new();
```

Set it before `service_dispatcher::start`, then read it from `service_main`.

Rules:

1. The context stores only the resolved config path unless another existing value is demonstrably required.
2. Do not create a global application context, dependency container, or service registry.
3. Treat a second initialization attempt as an explicit error rather than silently replacing the value.
4. Do not parse the full CLI a second time inside `ServiceMain`.
5. The worker loads the stored path rather than calling `Config::default_path()` unconditionally.
6. Preserve the current requirement that an installed service config exists; do not silently manufacture a service config inside the worker.
7. Keep tests focused on launch-context construction/access without introducing shared-state test ordering problems. Prefer testing pure helpers and compile-checking the `OnceLock` wiring rather than repeatedly initializing the production static in one test process.

An equivalently small `Mutex<Option<PathBuf>>` is acceptable only if `OnceLock` prevents deterministic tests or correct callback use. Do not add a crate dependency.

### Step 4: split dispatcher, callback, and worker responsibilities

Refactor the current Windows `run_service()` implementation into a worker that assumes it is already running inside `ServiceMain`.

The worker owns:

- control-handler registration;
- `START_PENDING` publication;
- config loading from the selected path;
- collector construction;
- nonblocking shutdown channel construction;
- current-thread runtime construction;
- shared daemon execution;
- final `STOPPED` publication.

The dispatcher entry owns only:

- launch-context installation;
- `service_dispatcher::start`;
- dispatcher error return.

The generated callback owns only:

- launch-context retrieval;
- invoking the worker;
- one service-mode diagnostic if the worker fails before or during execution.

Do not add a service object hierarchy or generic dispatcher abstraction.

### Step 5: report RUNNING only after listener bind

The current worker reports `RUNNING` before the runtime is built and before `run_with_shutdown()` binds the HTTP listener. Correct this without duplicating daemon setup.

Preferred minimal design:

- retain the public/current `run_with_shutdown()` behavior for foreground callers;
- introduce one internal variant or callback seam such as `run_with_shutdown_on_ready(..., on_ready)`;
- perform validation and `TcpListener::bind` first;
- invoke `on_ready` immediately after successful bind and before normal server/sampler supervision;
- let foreground mode pass a no-op readiness callback;
- let Windows service mode report `RUNNING` from that callback.

Requirements:

1. A configuration, collector, runtime, or bind failure never publishes `RUNNING`.
2. A successful bind publishes `RUNNING` once.
3. The callback is not a generalized lifecycle/event framework.
4. The service does not bind a second listener or duplicate `ServerConfig` construction.
5. `run_with_shutdown()` remains the single daemon core in behavior and ownership.
6. If reporting `RUNNING` itself fails, return a service error and stop rather than serving while the SCM believes startup failed.

A tiny prepared-listener function is acceptable if it produces less code than a readiness callback. Do not create a large `PreparedDaemon` type or duplicate the foreground path.

### Step 6: preserve truthful STOPPED reporting

After handler registration, every worker exit should make a best effort to report `STOPPED`:

- normal Stop or Shutdown: exit code zero;
- configuration, collector, runtime, readiness-publication, bind, server, or sampler failure: nonzero exit code.

Keep the existing status model unless a minimal `STOP_PENDING` update is already trivial. Do not expand this phase into checkpoint progression or pause/continue state support.

Avoid duplicate diagnostics:

- dispatcher errors return to `main`, which prints once;
- worker errors are logged once by the service callback and represented in service status;
- cleanup diagnostics from the shared daemon core are not broadened.

Update architecture wording so it no longer claims every service-worker failure returns to ordinary `main` if the generated callback API makes that impossible.

### Step 7: extract and test actual control mapping

The current tests exercise `send_shutdown()` directly but do not prove the callback's Interrogate and unsupported-control behavior.

Extract one small helper under `cfg(any(test, target_os = "windows"))`, for example:

```rust
fn handle_service_control(
    control: ServiceControl,
    shutdown: &ShutdownSender,
) -> ServiceControlHandlerResult
```

The production registration closure should delegate to it.

Required tests:

1. Stop returns `NoError` and completes the receiver with `SCM_STOP`.
2. Shutdown returns `NoError` and completes the receiver with `SCM_SHUTDOWN`.
3. Interrogate returns `NoError` and leaves the receiver pending.
4. One representative unsupported control returns `NotImplemented` and leaves the receiver pending.
5. Stop followed by Shutdown preserves the first reason and does not panic.
6. Shutdown followed by Stop preserves the first reason and does not panic.
7. Sender loss maps to the stable `SCM_CHANNEL_CLOSED` reason.

Use `try_recv`, a zero-duration timeout, or `FutureExt::now_or_never` only if already available. Do not add a dependency solely to inspect a pending one-shot receiver.

### Step 8: test readiness ordering in the daemon core

Add focused tests around the readiness seam:

- a successful bind invokes readiness once before normal shutdown completes;
- a bind failure does not invoke readiness;
- a readiness callback failure causes the daemon startup to fail and does not spawn a second server path;
- foreground `run_with_shutdown()` remains behaviorally unchanged through its no-op wrapper.

Use port `0` or an already-bound ephemeral listener technique. Do not bind a fixed test port or sleep for production intervals.

Do not attempt to emulate the Windows SCM in cross-platform unit tests.

### Step 9: correct Windows installation config-path truth

Inspect `packaging/install-windows.ps1` and make the selected config path canonical before building the service image command.

Required behavior:

```text
-ConfigPath supplied  -> resolve and use that exact path
no ConfigPath         -> create/preserve default path, then use it
```

The final `$ImagePath` must reference the resolved `$ConfigPath`, not always `$DefaultConfigPath`.

Keep existing quoting around the executable and config path. Preserve LocalService, automatic start, recovery policy, and firewall behavior.

Do not redesign installation or add MSI/WiX packaging.

### Step 10: make the existing SCM smoke deterministic

Use `scripts/smoke-windows.ps1` as the operational closure mechanism. Correct only defects that prevent it from truthfully validating the service.

Required inspection and corrections:

1. Invoke the copied/installed executable explicitly, for example `& $InstalledExe stop ...`, rather than assuming `greggd` is available on `PATH`.
2. Ensure every lifecycle/config command uses the same intended config path.
3. Keep the configured service image path quoted correctly.
4. Make port mutations and later assertions internally consistent; do not restore one port and then assert that another remains configured.
5. Use `try/finally` or equivalent bounded cleanup so a failed assertion does not leave the `greggd` service installed.
6. Confirm the initial start reaches `/v2/healthz` and `/v2/status` over loopback.
7. Confirm Stop, Start, and Restart complete through the SCM.
8. Confirm a deliberate bind failure never reaches `Running` and yields a stopped/nonzero service outcome.
9. Confirm reinstall preserves the selected config.
10. Remove the service and temporary binary/config state according to the script's stated preservation/removal cases.

Do not turn the script into a general Windows test framework. Keep it bounded
and local to the existing CI runner.

### Step 11: run bounded verification

#### Cross-platform/local checks

```bash
cargo fmt --all -- --check
cargo test -p greggd --bin greggd
cargo test -p greggd service::windows
cargo test -p greggd run
cargo clippy -p greggd --all-targets --all-features -- -D warnings
./scripts/check-local.sh
```

Run the existing release preflight only if implementation changes packaging checks or release behavior. Runtime-only Rust changes do not automatically require another full release preflight.

#### Ordinary hosted CI

Push the implementation and require one ordinary existing CI run. The existing
Windows job must run workspace tests, a release `greggd` build, and the bounded
SCM smoke on `windows-2022`, compiling the real dispatcher/`ServiceMain`
production branch and exercising the lifecycle.

Do not add a workflow, job, artifact, or evidence upload.

#### Windows CI SCM smoke

Closure requires one successful run in the existing Administrator Windows CI
environment:

```powershell
cargo build --release -p greggd
.\scripts\smoke-windows.ps1 -ExePath .\target\release\greggd.exe
```

This is the lightest direct proof that:

- `StartService` reaches the dispatcher;
- the SCM invokes `ServiceMain`;
- handler registration succeeds;
- the worker binds and reports `RUNNING`;
- Stop/Restart controls reach the nonblocking shutdown path.

### Step 12: reconcile planning records directly

After focused checks, ordinary CI, and the Windows CI SCM smoke pass:

- mark Plan 073 complete;
- mark Roadmap 066 complete through 074;
- update `plans/README.md` to show no active corrective phase;
- preserve Plan 071 footprint records unchanged;
- record the implementation SHA, CI run ID, and smoke result in the relevant
  plans;
- do not create Plan 075, an evidence file, or a closure manifest.

## Acceptance criteria

### Native SCM entry

- [x] The Windows service command calls `service_dispatcher::start` from synchronous command dispatch.
- [x] `define_windows_service!` generates the low-level `ServiceMain` callback.
- [x] The SCM invokes the generated callback before control-handler registration.
- [x] `service_control_handler::register` occurs only inside the `ServiceMain` worker path.
- [x] No raw dispatcher FFI or second service implementation is introduced.
- [x] Dispatcher connection errors return to ordinary `main` for existing classification.

### Runtime and shutdown preservation

- [x] Foreground mode still owns one current-thread Tokio runtime.
- [x] Service mode still owns one current-thread Tokio runtime.
- [x] No production path calls `block_on` while entered in another runtime.
- [x] SCM callbacks perform no blocking receive, sleep, runtime entry, or daemon join.
- [x] Stop and Shutdown retain distinct one-shot reasons.
- [x] Interrogate and unsupported controls leave shutdown pending.
- [x] Duplicate controls are harmless and preserve the first shutdown reason.
- [x] `run_with_shutdown()` remains the single shared daemon core.

### Configuration and readiness

- [x] The config path resolved from the service CLI invocation reaches `ServiceMain` and the worker.
- [x] The worker no longer unconditionally loads `Config::default_path()` when a custom path was selected.
- [x] `install-windows.ps1 -ConfigPath` registers that exact resolved path.
- [x] `START_PENDING` is reported while startup work is incomplete.
- [x] `RUNNING` is reported only after successful listener bind.
- [x] Configuration, collector, runtime, readiness-publication, or bind failure never reports `RUNNING`.
- [x] Every post-registration worker exit makes a best effort to report `STOPPED` with the correct zero/nonzero outcome.

### Focused tests

- [x] Dispatcher and generated callback production code compile on Windows.
- [x] Stop, Shutdown, Interrogate, unsupported, duplicate, and channel-closed control cases are tested.
- [x] Readiness is invoked once after successful bind and never after bind failure.
- [x] Readiness callback failure is covered.
- [x] Existing foreground and service-manager tests remain green.
- [x] No test installs a service, requires Administrator privileges, waits for the 30-second transition timeout, or binds a fixed port.

### Operational verification

- [x] `cargo test -p greggd --bin greggd` passes.
- [x] `cargo test -p greggd service::windows` passes.
- [x] `cargo test -p greggd run` passes.
- [x] Focused Clippy and formatting pass with warnings denied.
- [x] `./scripts/check-local.sh` passes.
- [x] One ordinary existing CI run passes, including Windows production compilation/tests.
- [x] The corrected `scripts/smoke-windows.ps1` passes in the existing
  Administrator Windows CI environment.
- [x] The smoke proves start, health/status, stop, restart, bind-failure
  handling, reinstall, and cleanup.

### Scope and closure

- [x] No new monitoring feature, protocol field, service command, platform, or dependency is added.
- [x] No service framework, generic runtime abstraction, workflow, job, artifact, or evidence system is added.
- [x] Manual release and the one read-only CI workflow remain unchanged.
- [x] Plans 066, 072, 073, 074, and the index describe the demonstrated state.
- [x] Plan 071 footprint records remain untouched unless a direct factual error is found.
- [x] Plan 074 provides the operational closure; no Plan 075 or evidence-only
  closure document is created.

## Handoff format

Record only:

```text
Implementation SHA:
Dispatcher entry:
Generated ServiceMain:
Config-path handoff:
RUNNING readiness point:
Control-mapping tests:
Focused local checks:
Ordinary CI run:
Manual Windows SCM smoke:
Planning-record reconciliation:
```

Do not add generated evidence, logs, screenshots, or binary artifacts to the repository.

Implementation SHA: `92f13864845e79a2732fae2a3733dccd02c38498` (native SCM implementation)
Operational closure SHA: `e754f3f6b17c14bfc71234459a15237fe042736f`
Dispatcher entry: `greggd::service::windows::start_service_dispatcher`, called by synchronous `Command::Service` dispatch.
Generated ServiceMain: `define_windows_service!(ffi_service_main, service_main)`.
Config-path handoff: resolved `PathBuf` stored in a process-local `OnceLock` before dispatcher start and read by the callback worker.
RUNNING readiness point: `run_with_shutdown_on_ready` invokes the service callback immediately after successful listener bind and before daemon task spawning.
Control-mapping tests: Stop, Shutdown, Interrogate, unsupported control, duplicate ordering, and channel-closed behavior covered; Windows production branch compiled in CI.
Focused local checks: formatting, focused greggd tests, focused Clippy, `./scripts/check-local.sh`, Rust 1.75 check, docs, Windows target check, and `./scripts/check-local.sh --release` passed.
Ordinary CI run: GitHub Actions run `31040689848` passed with Windows, macOS, MSRV, and Linux jobs green.
Windows SCM smoke: GitHub Actions run `31040689848` passed the workspace tests, release build, and authoritative `scripts/smoke-windows.ps1` lifecycle smoke on `windows-2022`.
Planning-record reconciliation: Plans 066, 073, 074, and `plans/README.md` updated; Plan 074 is complete and no Plan 075 was created.
