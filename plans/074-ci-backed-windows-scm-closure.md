# Phase 074: CI-backed Windows SCM closure

Status: complete.

Depends on: Plan 073.

## Objective

Close the remaining Windows service verification gap without requiring a separately maintained Windows host. Correct the existing lifecycle smoke and run it inside the repository's existing GitHub Actions Windows job.

This is a narrow verification implementation. It must not add a workflow, matrix tier, artifact, self-hosted runner, or generalized service-test framework.

## Scope

### In scope

- Correct deterministic defects in `scripts/smoke-windows.ps1`.
- Run the corrected smoke in the existing Windows CI job after a release build of `greggd`.
- Pin that existing job to `windows-2022` for a stable Windows Server compatibility baseline.
- Treat the CI smoke as the authoritative operational proof for the SCM dispatcher and service lifecycle.
- Reconcile Plans 066, 073, 074, and `plans/README.md` after one green run.

### Out of scope

- A second workflow or Windows job.
- Self-hosted or privileged runner infrastructure.
- Uploaded binaries, logs, screenshots, or evidence bundles.
- Installer redesign, MSI/WiX packaging, or broader SCM behavior.
- Product, protocol, collector, TUI, scheduler, or EggPool changes.
- A Plan 075 created only to record closure.

## Implementation

### 1. Make the smoke deterministic

Update `scripts/smoke-windows.ps1` only where required:

- replace port `1` as the bind-failure mechanism with an occupied ephemeral loopback port held open for the duration of the failure check;
- recreate `$InstallDir` before every copy into `$InstalledExe`;
- apply `NT AUTHORITY\LocalService` after every service recreation;
- wrap `sc.exe create`, `config`, `delete`, and related native commands so nonzero `$LASTEXITCODE` fails immediately;
- retain explicit invocation of `$InstalledExe` for Gregg lifecycle/config commands;
- retain bounded waits and unconditional `finally` cleanup;
- keep all service names, ports, files, and URLs local to the runner.

The smoke must demonstrate:

1. service creation and native SCM start;
2. `/v2/healthz` and `/v2/status` availability;
3. graceful stop, start, and restart;
4. config mutation and custom config-path persistence;
5. deterministic bind failure without publishing a stable Running service;
6. recovery after restoring a valid port;
7. reinstall with config preservation;
8. final service, binary, and temporary-config cleanup.

Do not turn the script into a reusable PowerShell framework.

### 2. Extend the existing Windows CI job

Modify only `.github/workflows/ci.yml`:

- change the existing Windows runner from `windows-latest` to `windows-2022`;
- retain the existing workspace test step;
- add `cargo build --release -p greggd`;
- invoke the existing smoke with PowerShell and a bounded timeout, for example:

```yaml
- name: Build greggd release
  run: cargo build --release -p greggd

- name: Windows SCM lifecycle smoke
  shell: pwsh
  timeout-minutes: 10
  run: ./scripts/smoke-windows.ps1 -ExePath ./target/release/greggd.exe
```

Do not add artifacts, service-account secrets, retries, a second runner, or a separate qualification workflow.

### 3. Preserve failure visibility

- Any failed `sc.exe`, Gregg command, service transition, health request, or cleanup assertion must fail the CI step.
- The smoke's `finally` path must still attempt cleanup after failure.
- Do not use `continue-on-error` for the SCM smoke.
- Do not suppress a failed bind-failure assertion merely because the service eventually stops.

### 4. Verification and record closure

Run locally where supported:

```bash
cargo fmt --all -- --check
./scripts/check-local.sh
```

Then require one ordinary workflow run at the implementation SHA or a source-equivalent documentation-only descendant. The Windows job must show all three stages green:

```text
workspace tests
release build
authoritative SCM lifecycle smoke
```

After that run passes:

- mark Plan 073 complete through CI-backed operational verification;
- mark Plan 074 complete and record the implementation SHA and workflow run ID;
- mark Roadmap 066 fully closed;
- update `plans/README.md` so Plans 066-074 move out of active work;
- do not create a separate evidence file or Plan 075.

## Acceptance criteria

### Smoke correctness

- [x] Bind failure uses an actually occupied ephemeral port.
- [x] Every service recreation reapplies `LocalService`.
- [x] Every required destination directory exists before copying the binary.
- [x] Native `sc.exe` failures are checked immediately.
- [x] The script always attempts bounded cleanup.
- [x] The script exits nonzero on any failed lifecycle assertion.

### CI integration

- [x] The existing Windows job is pinned to `windows-2022`.
- [x] No new workflow or job is added.
- [x] The Windows job runs workspace tests, a release build, and the SCM smoke.
- [x] The SCM smoke is not allowed to fail silently.
- [x] No artifact, evidence bundle, or self-hosted runner is introduced.

### Operational proof

- [x] CI proves dispatcher connection and generated `ServiceMain` execution.
- [x] CI proves custom config-path handling and post-bind `RUNNING` publication.
- [x] CI proves start, health/status, stop, restart, deterministic bind failure, recovery, reinstall, and cleanup.
- [x] One ordinary cross-platform workflow run is green at the final implementation state.

### Closure

- [x] Plans 066, 073, 074, and the index describe the demonstrated state.
- [x] Manual release and the single read-only workflow remain unchanged in scope.
- [x] No Plan 075 or closure-only evidence document is created.

## Handoff

Report only:

```text
Implementation SHA:
Smoke corrections:
Windows job change:
Workflow run:
SCM lifecycle result:
Planning-record closure:
```

Implementation SHA: `e754f3f6b17c14bfc71234459a15237fe042736f`
Smoke corrections: occupied ephemeral loopback bind-failure port, per-copy install-directory creation, `LocalService` reapplication, checked `sc.exe` commands, and failure-visible bounded cleanup.
Windows job change: existing job pinned to `windows-2022`; workspace tests, release build, then `smoke-windows.ps1` with a 10-minute timeout.
Workflow run: [31040689848](https://github.com/eggstack/gregg/actions/runs/31040689848)
SCM lifecycle result: green; workspace tests, release build, and authoritative lifecycle smoke all passed.
Planning-record closure: Plans 066, 073, 074, and `plans/README.md` reconciled; no Plan 075 or evidence document.
