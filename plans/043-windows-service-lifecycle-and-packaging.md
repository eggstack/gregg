# Phase 43: Windows service lifecycle and packaging

## Objective

Make `greggd` operable as a native Windows service while preserving the foreground `greggd run` mode for development and diagnostics.

This phase adds:

- a Windows Service Control Manager runtime entry point;
- a native Windows `ServiceManager` implementation for start/stop/restart/status;
- machine-scoped daemon configuration;
- simple PowerShell installation/removal packaging;
- graceful stop/shutdown integration;
- minimal service diagnostics;
- deterministic unit tests plus an elevated native lifecycle smoke.

The design must remain small. It must not expand into a general installer framework, package-manager integration, update service, GUI tray application, or remote administration system.

## Dependency and execution position

Depends on:

- Phase 41 protocol v2;
- Phase 42 native Windows collector and foreground daemon support.

Must complete before Phase 44 final Windows integration/release readiness.

## Governing invariants

1. `greggd run` remains a normal foreground process on every platform.
2. Windows service mode integrates with the Service Control Manager rather than simulating a daemon with detached processes or PID files.
3. Start, stop, restart, and status operations use native service APIs through a narrow adapter.
4. Service commands never return success when no action occurred.
5. Stop/shutdown controls feed the existing daemon cancellation path and allow bounded cleanup.
6. Service state transitions are reported truthfully.
7. Default daemon config is machine-scoped under `%ProgramData%`.
8. Installation is explicit and requires elevation.
9. The installer does not download code, contact crates.io, or mutate user profiles.
10. Uninstallation does not silently delete operator configuration unless explicitly requested.
11. Runtime arguments and paths with spaces are quoted/encoded correctly.
12. No CI publishing or release workflow is added.

## Scope

### In scope

- Windows service runtime dispatch;
- service control handler;
- service status reporting;
- Windows `ServiceManager` implementation;
- Windows default daemon config path;
- service install/uninstall PowerShell scripts;
- installed binary/config directory conventions;
- start/stop/restart/croncheck semantics;
- fatal/startup/shutdown service diagnostics;
- service-account and filesystem permission choices;
- elevated native lifecycle smoke instructions;
- README/packaging documentation.

### Out of scope

- MSI/MSIX/WiX installers;
- winget, Chocolatey, Scoop, or Microsoft Store distribution;
- automatic updates;
- GUI controls;
- service discovery;
- remote service administration;
- domain Group Policy deployment;
- TLS/authentication;
- full Windows Event Log tracing pipeline if a smaller bounded diagnostic path is sufficient;
- automatic GitHub/crates.io release integration;
- Windows ARM64 support claims.

## Workstream A: define Windows runtime modes

The binary must have two clear execution modes:

```text
greggd run       # foreground, console-owned
greggd service   # internal/native SCM service entry point
```

The exact internal command name may differ, but it must be stable for the registered service image path and documented as not intended for interactive use.

### Foreground mode

- initializes normal console logging;
- listens for Ctrl-C;
- runs the Windows collector and HTTP server;
- returns an ordinary process exit code;
- requires no service registration.

### Service mode

- calls the Windows service dispatcher;
- registers the service control handler;
- reports pending/running/stopping/stopped states;
- receives stop and shutdown controls;
- enters the same core async daemon runner with a service-provided shutdown future/token;
- does not initialize interactive terminal behavior;
- reports fatal startup failure to SCM and minimal diagnostics.

Refactor the existing daemon runner so signal-source selection is injected rather than hardcoded. The core server/sampler supervision and bounded cleanup must remain shared.

Recommended internal split:

```rust
run_with_shutdown(collector, config, shutdown_future, logging_mode)
run_foreground(...)
run_windows_service(...)
```

Avoid duplicating the sampler/server startup sequence in service code.

### Workstream A acceptance criteria

- [ ] Foreground and service entry points are distinct.
- [ ] Both use one core daemon supervision implementation.
- [ ] Service mode does not require a console.
- [ ] Foreground Ctrl-C behavior remains unchanged.
- [ ] Unsupported platforms cannot invoke Windows service mode successfully.

## Workstream B: implement the Windows SCM service runtime

Use a target-specific Windows service crate or narrow Windows API wrapper with verified workspace MSRV.

### Required service states

Report at minimum:

```text
START_PENDING
RUNNING
STOP_PENDING
STOPPED
```

State checkpoints/wait hints should be reasonable during startup and shutdown. Do not remain indefinitely in a pending state.

### Required controls

Handle:

- service stop;
- system shutdown where delivered;
- interrogate/status query through SCM mechanisms;
- unsupported controls by returning the appropriate result without panic.

Pause/continue is not required.

### Startup sequence

1. SCM enters service main.
2. Register control handler.
3. Report `START_PENDING`.
4. Resolve/load machine config.
5. Construct Windows collector.
6. Bind HTTP listener and initialize sampler.
7. Report `RUNNING` only after startup has reached a defined operational point.
8. Continue normal supervision.

Define whether `RUNNING` is reported immediately after listener bind or only after collector readiness. Preferred: report `RUNNING` after the service process and listener are operational; expose collector warm-up through health/readiness rather than holding SCM startup pending for sample intervals.

### Shutdown sequence

1. Receive stop/shutdown control.
2. Report `STOP_PENDING`.
3. signal core daemon shutdown;
4. wait for bounded server/sampler cleanup;
5. report `STOPPED` with success or service-specific failure code;
6. return from service main.

Do not terminate the process abruptly unless bounded cleanup exceeds policy and the existing daemon abort behavior takes over.

### Required tests

Use an injectable service-status sink/control source for deterministic tests:

- successful state sequence;
- config load failure before running;
- bind failure before running;
- collector construction failure;
- stop while warming;
- stop while ready;
- duplicate stop controls;
- unsupported control;
- cleanup timeout;
- panic/fatal task outcome maps to stopped/failure;
- no `RUNNING` after failed startup;
- no state report after terminal `STOPPED`.

### Workstream B acceptance criteria

- [ ] SCM runtime registers and reports truthful state.
- [ ] Stop/shutdown reaches the shared daemon shutdown path.
- [ ] Startup failures never report running.
- [ ] Cleanup is bounded.
- [ ] State-machine tests cover success and failure.

## Workstream C: implement `WindowsServiceManager`

Add a Windows implementation of the existing service-manager trait.

Required operations:

```rust
start()
stop()
restart()
is_active()
```

Use native service-manager APIs through the selected dependency. Do not execute `sc.exe`, PowerShell, or `net start` at runtime for these CLI commands.

### Service identity

Use stable constants:

```text
service name: greggd
display name: Gregg Metrics Daemon
```

Keep service name distinct from display name.

### Operation semantics

`start`:

- open SCM/service with minimal required access;
- if already running, return success idempotently;
- if start-pending, wait boundedly for running;
- if stopped, start and wait boundedly;
- if stop-pending, wait then start or return a clear transient error according to documented policy.

`stop`:

- if already stopped, return success idempotently;
- request stop;
- wait boundedly for stopped;
- surface access denied, nonexistent service, marked-for-delete, and timeout distinctly where possible.

`restart`:

- stop using the above semantics;
- start only after confirmed stopped;
- preserve the first meaningful error;
- do not create the service if absent.

`is_active`:

- return true only for running or a deliberately documented active-pending state;
- define how start-pending/stop-pending map;
- absent service must not silently become false if callers need to distinguish not-installed from stopped. If the existing trait cannot express this safely, extend it narrowly with a service-state enum rather than discarding information.

### Trait review

The current boolean `is_active` may be too weak for Windows pending/not-installed states. Preferred revision:

```rust
enum ServiceState {
    NotInstalled,
    Stopped,
    StartPending,
    Running,
    StopPending,
}
```

with compatibility helpers for existing Linux/macOS behavior.

Do not add a generic service abstraction larger than the three platform adapters require.

### Required tests

Use an injectable SCM runner/state source:

- not installed;
- stopped;
- running;
- start pending -> running;
- stop pending -> stopped;
- start timeout;
- stop timeout;
- access denied;
- service deleted during operation;
- restart state sequence;
- idempotent start/stop;
- fixed service identity and no shell interpolation.

### Workstream C acceptance criteria

- [ ] Windows CLI service operations use native APIs.
- [ ] Not-installed state is truthful.
- [ ] Pending states are bounded and tested.
- [ ] Restart waits for confirmed stop.
- [ ] Existing Linux/macOS service behavior remains correct after any trait refinement.

## Workstream D: define machine-scoped Windows configuration

Target default path:

```text
%ProgramData%\gregg\greggd.toml
```

Resolve through a testable machine-data path helper. Do not fall back to the current directory for supported Windows service execution.

### Config ownership and access

The installer should:

- create `%ProgramData%\gregg`;
- create a default config only if none exists;
- preserve an existing config on reinstall;
- grant read access to the selected service account;
- grant write access only if `greggd host`/`port` mutation commands are intended to work under the invoking administrator and service account policy;
- avoid world-writable ACLs.

### Service account decision

Preferred initial account:

```text
NT AUTHORITY\LocalService
```

provided it can:

- read the config;
- bind the configured unprivileged port;
- query required system metrics APIs;
- write only the minimal diagnostic destination if one is used.

Do not run as `LocalSystem` without a demonstrated requirement. If `LocalService` cannot access a required API or path, document the exact blocker before selecting a more privileged account.

### Config mutation commands

`greggd host` and `greggd port`:

- require an elevated operator when writing ProgramData/service configuration;
- atomically write config;
- restart the service through `WindowsServiceManager`;
- preserve the old config if write fails;
- report service-not-installed separately from config-write failure.

### Workstream D acceptance criteria

- [ ] Default config is under ProgramData.
- [ ] Installer preserves existing config.
- [ ] ACLs are least-privilege and not world-writable.
- [ ] Service runs under LocalService unless a documented blocker requires otherwise.
- [ ] Host/port mutation and restart behavior is transactional and truthful.

## Workstream E: add simple Windows installation/removal scripts

Add:

```text
packaging/install-windows.ps1
packaging/uninstall-windows.ps1
```

These are explicit packaging helpers, not release automation.

### Install script responsibilities

- require/check administrator privileges;
- accept an explicit source `greggd.exe` path or use a documented default;
- create `%ProgramFiles%\Gregg`;
- copy `greggd.exe` to a stable installed path;
- create `%ProgramData%\gregg`;
- install a default config only when absent;
- register the Windows service with correctly quoted executable/config arguments;
- configure automatic start or a documented default start mode;
- configure the selected least-privilege account;
- optionally configure a small failure-restart policy if simple and safe;
- start the service only after registration succeeds;
- print installed paths and next diagnostic commands;
- fail nonzero on any incomplete step.

### Uninstall script responsibilities

- require/check administrator privileges;
- stop service if present;
- delete service registration;
- remove installed binary directory;
- preserve ProgramData config by default;
- support an explicit `-RemoveConfig` flag;
- tolerate already-absent service/binary idempotently;
- report files that could not be removed because they are in use.

### Script implementation guidance

Using PowerShell service cmdlets or `sc.exe` inside the explicit installer/uninstaller is acceptable because these scripts are administrative packaging surfaces, not runtime control paths. Prefer structured PowerShell APIs/cmdlets where they correctly support the needed service-account/image-path settings.

Quote paths carefully. Test installation from a path containing spaces.

Do not download the executable, query GitHub Releases, or select versions automatically.

### Required script tests

Static/unit where practical:

- PowerShell parse/syntax;
- command construction with spaces;
- default config preservation;
- `-RemoveConfig` behavior;
- admin check;
- idempotent absent-service handling.

Native elevated rehearsal:

- fresh install;
- reinstall preserving config;
- uninstall preserving config;
- uninstall removing config explicitly;
- install from path with spaces;
- failed copy/registration cleanup behavior.

### Workstream E acceptance criteria

- [ ] Installation is explicit and local-file based.
- [ ] No network/download/release lookup occurs.
- [ ] Config is preserved by default.
- [ ] Paths with spaces work.
- [ ] Scripts fail clearly and are idempotent where appropriate.
- [ ] No MSI/package-manager scope is introduced.

## Workstream F: minimal service diagnostics

Foreground mode retains structured console logging.

Service mode needs enough diagnostics to explain startup and fatal failure without introducing a logging subsystem.

Choose one bounded approach:

1. emit startup, running, stop, and fatal events to Windows Event Log through a small adapter and installer registration;
2. write a small bounded/rotated service log under `%ProgramData%\gregg` using an existing lightweight tracing writer;
3. if neither is proportionate, rely on SCM exit codes plus a documented foreground diagnostic command, but only if startup failures remain actionable.

Preferred: minimal Event Log integration for lifecycle/fatal events, not every sample/request.

Do not create an unbounded append-only log.

Diagnostics must not include:

- full internal error chains with private paths in network responses;
- secrets;
- repeated per-sample noise;
- metrics payload history.

### Required diagnostic events

- service starting/version;
- service running/listen address;
- service stopping;
- config load failure;
- bind failure;
- collector fatal failure;
- unexpected critical task exit;
- cleanup timeout.

### Workstream F acceptance criteria

- [ ] An administrator can determine why the service failed to start.
- [ ] Logging is bounded/minimal.
- [ ] Foreground mode remains the detailed diagnostic path.
- [ ] No telemetry history or general logging platform is added.

## Workstream G: CLI behavior and unsupported-platform correction

Update CLI descriptions from `Linux and macOS` to supported platform-neutral wording where appropriate.

Before this phase, unsupported platforms must no longer receive a no-op manager that returns success. After this phase:

- Windows returns `WindowsServiceManager`;
- Linux returns systemd manager;
- macOS returns launchd manager;
- truly unsupported platforms return an explicit unavailable manager/error.

Review `croncheck` naming. It may remain for compatibility, but its behavior on Windows must be documented as an idempotent service-health/start check rather than cron integration.

Consider adding a clearer alias such as `ensure-running` without removing `croncheck`, but do not expand CLI scope unless useful across all platforms.

### Required CLI tests

- Windows service command parsing;
- service internal command hidden/help behavior;
- start/stop/restart error mapping;
- permission denied exit code;
- service not installed exit code/diagnostic;
- host/port config mutation followed by restart;
- foreground run unaffected;
- unsupported target manager returns error, not success.

### Workstream G acceptance criteria

- [ ] Windows service manager is selected natively.
- [ ] Unsupported platform operations fail.
- [ ] CLI help accurately describes Windows support.
- [ ] Exit codes remain meaningful.
- [ ] Foreground mode remains directly accessible.

## Workstream H: native elevated lifecycle smoke

Create a documented manual/elevated test procedure and, where feasible, a repository PowerShell smoke helper that never installs in ordinary CI.

Test sequence:

1. build release `greggd.exe`;
2. install from local path;
3. verify service registration and account;
4. verify ProgramData config path;
5. wait for running state;
6. query `/v2/healthz` and `/v2/status` over loopback;
7. run `greggd stop` and verify stopped;
8. run `greggd start` and verify running;
9. run `greggd restart` and verify new process/running state;
10. change port through CLI and verify atomic config plus restart;
11. simulate bind failure and verify service reports failure/not running;
12. reinstall and verify config preservation;
13. uninstall and verify service/binary removal with config preserved;
14. uninstall/remove config explicitly in a separate disposable test.

The smoke helper must use only local files and loopback. It must not contact crates.io or GitHub.

Ordinary CI should not require elevation or service installation. Phase 44 may keep this as a manual maintainer test.

### Workstream H acceptance criteria

- [ ] Full install/start/query/stop/restart/uninstall lifecycle passes natively.
- [ ] Config mutation/restart passes.
- [ ] Failure state is observable.
- [ ] Reinstall preserves config.
- [ ] Uninstall preserves config by default.
- [ ] No external network is required.

## Workstream I: documentation

Update:

- root README;
- `packaging/README.md`;
- daemon crate README;
- Windows platform notes;
- service commands;
- config paths;
- service account;
- firewall/private-network warning;
- foreground troubleshooting instructions;
- known limitations.

Document installation as:

```powershell
cargo install greggd --version "=X.Y.Z"
# Then run the repository-provided installer against the local installed exe,
# or copy the script from the matching source tag.
```

Because crates.io packages may include packaging files according to the manifest, verify whether the PowerShell scripts are included and document the actual retrieval path. Keep instructions practical and avoid assuming a repository checkout if packaged scripts are available.

### Firewall note

The service binds according to config, defaulting to the existing address policy. Do not automatically create a public firewall rule. Document that LAN exposure is operator-controlled and the daemon has no TLS/authentication.

### Workstream I acceptance criteria

- [ ] Windows service installation/removal is documented end-to-end.
- [ ] ProgramData/ProgramFiles paths and service account are documented.
- [ ] Foreground diagnostic mode is documented.
- [ ] No automatic firewall exposure is created.
- [ ] Security limitations remain prominent.

## Required validation commands

Non-elevated Windows:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
cargo build -p greggd --release
```

PowerShell syntax:

```powershell
$null = [System.Management.Automation.Language.Parser]::ParseFile(
  "packaging/install-windows.ps1", [ref]$null, [ref]$null
)
$null = [System.Management.Automation.Language.Parser]::ParseFile(
  "packaging/uninstall-windows.ps1", [ref]$null, [ref]$null
)
```

Elevated native lifecycle smoke as described above.

Run full local validation on Linux/macOS after service trait changes.

## Phase acceptance criteria

Phase 43 is complete only when:

- [ ] Windows service mode integrates with SCM.
- [ ] Foreground and service modes share the core daemon runner.
- [ ] Start/stop/restart/status use native service APIs.
- [ ] Service states and failures are truthful and bounded.
- [ ] Default daemon config is `%ProgramData%\gregg\greggd.toml`.
- [ ] Service runs under LocalService or a more privileged account only with documented necessity.
- [ ] Installer/uninstaller use local files, preserve config by default, and handle spaces/idempotency.
- [ ] Minimal lifecycle/fatal diagnostics are available without unbounded logging.
- [ ] Unsupported platforms no longer return no-op success.
- [ ] Elevated install/start/query/stop/restart/uninstall smoke passes.
- [ ] Linux systemd and macOS launchd behavior remain green.
- [ ] No package-manager, updater, release workflow, or artifact-evidence system is added.

## Evidence required for completion

Only:

- passing unit/native tests;
- concise elevated lifecycle-smoke transcript in the handoff note;
- PowerShell syntax validation;
- passing Linux/macOS local checks;
- code/documentation diff.

Do not retain service-install CI artifacts or create a hosted release qualification run.

## Handoff notes for a smaller implementation model

1. Refactor the core daemon runner to accept a shutdown source before writing SCM code.
2. Implement and unit-test the service state machine with fake status/control adapters.
3. Implement the runtime SCM adapter next.
4. Implement `WindowsServiceManager` separately from service runtime.
5. Add ProgramData path and permissions before installer scripts.
6. Keep install/uninstall scripts local-file-only.
7. Add minimal diagnostics, not a new logging subsystem.
8. Run Linux/macOS service tests after any trait change.
9. Perform the elevated lifecycle smoke last on a disposable Windows host.
10. Do not make ordinary CI require administrator privileges.