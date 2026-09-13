# Plan 116: update lifecycle ownership corrective pass

Status: in implementation; local fmt/clippy/tests/default check/MSRV green, release preflight green except clean-tree (uncommitted), awaiting remote CI.

Depends on: Plan 115 plus the settled updater/restart ownership from Plans 101-104 and 113. It is independent of Plan 114's TUI work and of the remaining Plan 091 soak record.

This is a narrow post-closure corrective pass. Plan 115 remains the historical record of the restart/bootstrap/elevation corrections that landed in `d7cba02` / `17079e5`; this plan owns only the residual `greggd update` lifecycle defect discovered after that closure.

## Objective

Make `greggd update` use exact-executable ownership for its **pre-replacement lifecycle decision**, not just for the post-replacement `restart` command.

The correction must ensure that:

1. Windows never stops, restarts, or otherwise mutates a foreign/unknown SCM registration while updating a different `greggd.exe`;
2. Unix foreign/inactive system-manager state does not suppress preservation of a running selected direct/cron daemon;
3. owned managed daemons preserve running versus stopped state across update;
4. candidate preparation still completes before any Windows service quiescence;
5. the existing exact-executable `restart_daemon()` boundary remains the final authority for post-replacement manager mutation.

Do not broaden this into process discovery, a new service abstraction, or another installer rewrite.

## Confirmed post-Plan-115 findings

### 1. Windows update can stop a foreign SCM service

`crates/greggd/src/update.rs::run_update()` still captures:

```text
pre_state = startup_state()
```

before replacement. On Windows, `startup_state()` calls `ServiceManager::is_active()` and reduces the host-level canonical SCM registration to `WindowsServiceRunning` / `WindowsServiceStopped` without comparing its registered image path to the exact invoked executable.

The flow then does:

```text
prepare verified candidate
-> if pre_state == WindowsServiceRunning: stop_windows_service_if_needed()
-> replace current executable
-> restart_after_update()
```

Plan 115 corrected `restart_after_update()` to call ownership-aware `restart_daemon()`, but the **pre-replacement stop** still happens from host-global state.

Concrete unsafe case:

```text
system SCM greggd -> C:\Program Files\Gregg\greggd.exe, running
user invokes      -> C:\Users\...\greggd.exe update
startup_state()   -> WindowsServiceRunning
candidate prepared
current code      -> stops canonical SCM service
current exe       -> user-local binary is replaced
restart_daemon()  -> correctly sees SCM as foreign and refuses restart
result            -> unrelated system service can be left stopped
```

The ownership check must occur before any service stop, not only at restart time.

### 2. Unix host-global stopped-manager state can hide a running direct daemon

On Linux/macOS, `startup_state()` similarly describes canonical systemd/launchd presence/activity without binding it to the invoked executable.

A foreign canonical manager that exists but is stopped can therefore produce `SystemdInstalledStopped` / `LaunchdInstalledUnloaded`. `restart_after_update()` treats those states as intentionally stopped and skips restart entirely.

If the selected user-local/custom-config daemon is actually running directly, the update replaces its on-disk executable but leaves the old process image alive indefinitely.

Plan 115 fixed this state-preservation problem for bootstrap installer reruns. The self-update path still needs the same ownership-aware lifecycle intent.

### 3. Plan 115's restart correction is sound but is reached too late

`restart_daemon()` now gates manager mutation on exact executable ownership and fails closed for foreign/unknown registrations. That is the correct **post-replacement** boundary and should be reused, not bypassed.

The remaining problem is the update coordinator's earlier lifecycle classification and Windows quiescence decision. Fix that coordinator rather than weakening restart ownership rules.

## Scope decisions

### 1. Stop using `startup_state()` as `greggd update` mutation authority

`startup_state()` may remain as a coarse read-only/status observation where appropriate. Do not globally redefine or remove it under this plan.

`crates/greggd/src/update.rs` must instead derive a small update-specific lifecycle disposition from:

- the exact invoked/current executable path returned by the existing writable preflight;
- the resolved selected config path;
- systemd/launchd exact-executable ownership helpers from Plan 113/115;
- Windows `ServiceManager::query_registration()` with full state plus registered image path;
- the existing bounded selected-endpoint health probe for direct/cron running intent.

A small enum such as `UpdateLifecycle` / `UpdateDisposition` is appropriate. Do not create a generalized installation registry.

### 2. Observe lifecycle intent after candidate preparation and immediately before mutation

Preserve Plan 102's prepare-before-quiesce transaction rule.

Preferred order:

```text
resolve available update
-> preflight destination writability
-> fully download/build/verify candidate
-> observe exact-executable update lifecycle immediately before mutation
-> quiesce only an owned Windows SCM service when required
-> replace exact current executable
-> restore only the lifecycle that was positively attributed to this installation
```

This avoids basing lifecycle intent on a stale observation made before a potentially long Cargo fallback build.

Read-only ownership/health queries may occur earlier for diagnostics, but the mutation decision must use the post-preparation observation.

### 3. Unix lifecycle classification must separate manager ownership from selected-daemon running intent

For Linux/macOS, derive update behavior from manager ownership, manager activity/config identity, and the selected config's health.

Required semantics:

```text
owned manager + active
  -> ManagedRunning
  -> replace atomically
  -> restart through ownership-aware restart_daemon()

owned manager + inactive
  -> ManagedStopped
  -> replace
  -> leave stopped

foreign manager + active + same selected config
  -> ForeignManagedSelectedConfig
  -> preserve foreign manager
  -> do not direct-stop/restart its endpoint
  -> replace only the invoked executable

foreign manager + active + different known config
  -> manager preserved
  -> selected config may independently be probed for DirectRunning

foreign manager + inactive
  -> manager is not currently controlling an endpoint
  -> selected config may independently be probed for DirectRunning

absent manager
  -> selected config may independently be probed for DirectRunning

unknown active manager ownership/config
  -> fail before executable replacement; do not guess

unknown inactive manager + selected endpoint running
  -> fail before replacement because safe post-update ownership cannot be established

unknown inactive manager + selected endpoint absent
  -> replacement may proceed and remain stopped
```

A valid selected Gregg health response means running intent only. It must never override a known active foreign manager using that same config.

### 4. Preserve direct/cron running state explicitly

When the ownership-aware disposition says the selected daemon is `DirectRunning`:

- atomically replace the exact invoked executable;
- call the existing ownership-aware `restart_daemon(exe, config, explicit)` after replacement;
- require the existing config-specific stop, endpoint-absence proof, spawn, and health-readiness checks;
- if restart fails after successful replacement, return `UpdatedButRestartFailed` exactly as today.

Do not add process-name scanning, PID files, `pkill`, port-owner discovery, or another direct-control path.

### 5. Windows SCM ownership must be established before any stop

Use `query_registration()` rather than `is_active()` for update mutation decisions.

Required classification:

```text
NotInstalled
  -> no SCM mutation; update exact invoked executable only

installed + executable path == exact invoked executable
  -> owned SCM

installed + executable path != exact invoked executable
  -> foreign SCM; preserve it; never stop/restart it

installed + missing/unparseable executable identity
query/access failure
  -> unknown; fail before replacement if safe ownership cannot be established
```

Do not infer ownership from service name alone.

### 6. Revalidate Windows ownership immediately before quiescence

A read-only observation taken before candidate preparation is not enough.

After the candidate is fully prepared and immediately before any stop/replacement:

1. query SCM registration again;
2. require the registered executable to still match the exact invoked executable before service mutation;
3. preserve full service state;
4. if registration became foreign/unknown, fail with zero service/binary mutation;
5. if it disappeared, proceed as unmanaged/no-SCM without attempting a stop;
6. if it is owned and running/start-pending, stop and wait using the existing bounded service abstraction;
7. if it is owned and stopped, replace and leave stopped;
8. if it is stop-pending, wait for stopped before replacement and do not restart merely because it was transitioning down.

The exact handling of pending states may reuse current bounded service helpers. Do not add polling infrastructure outside the existing service abstraction.

### 7. Foreign Windows SCM state must not suppress an unrelated update

A foreign SCM registration is not evidence that the exact invoked binary is managed or running.

For a user-local/disposable `greggd.exe update` while the canonical SCM points elsewhere:

- preserve the SCM registration and state byte-for-byte;
- do not call `stop()`, `restart()`, or `unregister()`;
- replace only the exact invoked executable if all ordinary update checks pass;
- report the update result without claiming the foreign service was updated/restarted.

Windows still has no direct foreground-daemon restart fallback under this plan. Do not add process discovery to compensate.

### 8. Keep post-replacement manager restart behind `restart_daemon()`

Do not reimplement systemd/launchd/SCM restart ownership inside `update.rs`.

For an owned managed-running disposition, call the Plan 115 `restart_daemon()` surface after replacement so it performs a fresh exact-executable ownership check before mutation.

If the registration changes between quiescence and restart, fail as `UpdatedButRestartFailed`; do not widen ownership from stale pre-update state.

### 9. Keep partial-success semantics truthful

Preserve existing update outcomes:

- successful replacement with successful required restart -> normal updated outcome;
- successful replacement with failed required restart -> `UpdatedButRestartFailed` and nonzero CLI behavior;
- stopped/foreign/unmanaged-not-running disposition -> successful replacement without fabricating a restart;
- ownership uncertainty discovered **before** replacement -> hard error with no binary/service mutation.

Do not roll back a verified replacement after a post-replacement restart failure.

### 10. Preserve installer/uninstaller work from Plans 113-115

This plan does not change:

- bootstrap installer destination classification or user-local `stop` + `croncheck` activation;
- uninstall ownership/discovery/Cargo sequencing;
- shared elevation guidance;
- `greggd restart` manager ownership rules;
- systemd/launchd/cron/SCM install semantics.

Only update-path lifecycle ownership is in scope.

## Implementation sequence

### Step 1: introduce a pure update lifecycle decision

In `crates/greggd/src/update.rs`, replace `StartupState`-only restart/quiesce policy with a small ownership-aware update disposition.

Factor the pure decision separately from platform I/O so deterministic tests can enumerate owned/foreign/unknown, active/stopped, config-equality, and endpoint-running cases without invoking managers.

### Step 2: wire Unix ownership-aware observation

Use `systemd_artifact_ownership()` / `launchd_artifact_ownership()` plus the existing selected-config health probe to derive:

- owned managed running;
- owned managed stopped;
- direct running;
- stopped/unmanaged;
- foreign manager preserved;
- unsafe/unknown.

Do not call `startup_state()` to decide update mutation/restart.

### Step 3: wire Windows registration-aware quiescence

Replace `should_quiesce_running_service(pre_state)` with an ownership-aware helper based on `ServiceRegistration` plus exact path equivalence.

Re-query after candidate preparation immediately before stop/replacement. Only an exact-owned running service may be stopped.

### Step 4: preserve post-replacement activation semantics

Route only `ManagedRunning` / `DirectRunning` through `restart_daemon()` after replacement. Leave positively stopped or unrelated foreign-manager cases untouched.

Keep `UpdatedButRestartFailed` unchanged for post-replacement restart failures.

### Step 5: add focused regressions

Add deterministic tests around the new lifecycle planner and fake/injected SCM registration behavior. Extend existing Windows-native coverage only where it adds platform truth; do not add a workflow/job/matrix.

### Step 6: reconcile active documentation and Plan 115

Append a short Plan 115 post-closure note pointing to Plan 116. Update the active updater/startup documentation only where it currently claims `greggd update` is fully exact-executable-aware before quiescence.

## Files likely touched

```text
crates/greggd/src/update.rs
crates/greggd/src/startup/install.rs          # only if a small shared ownership/restart helper is needed
crates/greggd/src/startup/systemd.rs          # only if visibility/test factoring is needed
crates/greggd/src/startup/launchd.rs          # only if visibility/test factoring is needed
crates/greggd/src/service/mod.rs              # only if pending-state/revalidation helper belongs here
crates/greggd/src/service/windows.rs          # only if existing bounded state transition support needs reuse/exposure
scripts/smoke-windows.ps1                     # only if a narrow native assertion is practical
README.md
docs/daemon.md
architecture/gregg-update.md
architecture/greggd-daemon.md
AGENTS.md
.opencode/skills/greggd-daemon/SKILL.md
.opencode/skills/release-process/SKILL.md
plans/115-plan113-restart-activation-and-elevation-corrective-pass.md
plans/README.md
```

Do not touch every file mechanically. The expected implementation center is `crates/greggd/src/update.rs`.

## Required deterministic tests

At minimum cover:

- owned active systemd registration -> managed-running update and post-replacement restart;
- owned inactive systemd registration -> replacement with no restart;
- foreign active systemd registration using the selected config -> preserved, never direct-stopped/restarted;
- foreign active systemd registration using a different config + selected direct daemon running -> direct-running update/restart;
- foreign inactive systemd registration + selected direct daemon running -> direct-running update/restart;
- unknown active systemd ownership -> pre-replacement refusal;
- unknown inactive systemd ownership + selected endpoint running -> pre-replacement refusal;
- unknown inactive systemd ownership + selected endpoint absent -> replacement may proceed stopped;
- equivalent launchd cases follow the same policy;
- Windows `NotInstalled` -> no SCM stop/restart;
- Windows owned `Running` / `StartPending` -> quiesce only after candidate preparation, then restart after replacement;
- Windows owned `Stopped` -> replacement with no restart;
- Windows owned `StopPending` -> bounded transition to stopped before replacement and no automatic restart;
- Windows foreign running SCM -> zero SCM mutation while exact invoked executable may still update;
- Windows foreign stopped SCM -> zero SCM mutation;
- Windows missing image path / query failure -> pre-replacement refusal when ownership is unsafe;
- SCM ownership is revalidated immediately before stop; a registration that changes from owned to foreign produces zero stop/replacement mutation;
- post-replacement registration change/ownership failure becomes `UpdatedButRestartFailed`, not a foreign restart;
- direct/cron restart still requires definitive endpoint absence and valid Gregg health readiness;
- successful replacement of a stopped/foreign installation does not print a false restart-success claim.

Use pure helpers and the existing fake service adapter surfaces. Do not add a generalized mocking framework.

## Operational verification

### Linux / Ubuntu

No new privileged CI is required.

Use the existing host only where safe and keep any operational proof disposable. If an existing foreign canonical systemd service is present, a useful narrow local smoke is:

```text
foreign canonical systemd registration remains unchanged
-> start disposable/user-local greggd on a separate custom config/port
-> exercise the ownership-aware update-lifecycle decision against that selected config
-> prove it resolves to DirectRunning rather than SystemdInstalledStopped/foreign-managed
-> prove the foreign systemd service remains unchanged
```

Do not require live crates.io/GitHub update transport merely to prove routing. A deterministic/injected transaction test is acceptable and preferred if it proves the same mutation ordering without network dependency.

### Windows

Existing Windows CI remains the native truth source.

At minimum, native tests must execute the production `ServiceRegistration`/path-equivalence code and the existing SCM lifecycle smoke must remain green. If practical within the existing smoke, add a bounded foreign-registration assertion; do not add another job or install external tooling.

### macOS

Existing native compile/tests are sufficient for launchd ownership routing unless implementation introduces a new macOS-only API. Do not add privileged launchd CI.

## Standard gates

Run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
./scripts/check-local.sh --release
rustup run 1.75 cargo check --workspace --all-features
```

The release preflight remains appropriate because self-update lifecycle behavior changes.

Use one ordinary existing CI run after implementation for native Windows/macOS/MSRV truth. Record the exact final implementation SHA and CI run ID in this plan's closure record.

## Preserved exclusions

Do not add under Plan 116:

- changes to bootstrap installer replacement/activation behavior;
- changes to uninstall ownership/Cargo lifecycle;
- changes to metrics protocol, collectors, sampler, TUI, EggPool, or scheduler;
- global discovery/control of every Gregg installation;
- process-name scanning, PID registries/files, WMI process search, `pkill`, or port-owner discovery;
- a Windows foreground-daemon stop/restart mechanism;
- a new shutdown protocol or public shutdown HTTP endpoint;
- internal `sudo`, UAC prompting, `runas`, or automatic privilege escalation;
- a persistent installation receipt/registry;
- a generalized systemd/launchd parser beyond the existing narrow ownership surfaces;
- new dependencies when existing startup/service/update helpers suffice;
- new workflows, jobs, matrices, privileged runners, evidence bundles, or release automation;
- rewriting Plan 115's historical implementation SHA, CI run, or valid closure evidence as though this later-discovered update-specific defect never existed;
- unrelated cleanup or Plan 091 soak work.

## Acceptance criteria

Plan 116 is complete only when:

1. [x] `greggd update` no longer uses host-global `startup_state()` as authority for pre-replacement service mutation or restart intent.
2. [x] Update lifecycle classification is explicitly relative to the exact invoked executable and selected config.
3. [x] Windows SCM stop is authorized only after `query_registration()` proves the registration still targets the exact invoked executable.
4. [x] A foreign Windows SCM service can never be stopped/restarted by updating an unrelated `greggd.exe`.
5. [x] Windows SCM query/identity uncertainty blocks unsafe mutation before executable replacement.
6. [x] Candidate preparation still completes before any Windows service quiescence.
7. [x] Windows ownership/state is revalidated immediately before quiescence/replacement rather than relying on a stale pre-download snapshot.
8. [x] Owned Windows running/start-pending service updates stop safely, replace, and restart; owned stopped service updates remain stopped.
9. [x] Owned Windows stop-pending state reaches stopped before replacement and is not spuriously restarted.
10. [x] Windows `NotInstalled` and foreign registration paths perform zero SCM mutation.
11. [x] Owned active systemd/launchd installations preserve managed-running state through replacement using `restart_daemon()`.
12. [x] Owned inactive systemd/launchd installations remain stopped after replacement.
13. [x] A foreign active Unix manager using the selected config is preserved and cannot be direct-stopped/restarted by the unrelated update.
14. [x] A foreign inactive manager no longer masks a running selected direct daemon.
15. [x] A foreign active manager using a different known config does not suppress a running selected direct daemon's update/restart.
16. [x] Unknown active Unix manager ownership fails before replacement; unknown inactive ownership is allowed only when the selected endpoint is not running.
17. [x] Direct/cron running intent is captured immediately before replacement and uses the existing config-specific safe restart path afterward.
18. [x] Post-replacement restart ownership failure remains a truthful `UpdatedButRestartFailed` partial success and never mutates a foreign manager.
19. [x] Foreign/stopped/no-running cases do not print or report a fabricated restart.
20. [x] Plan 115 receives a short post-closure note pointing to Plan 116 without rewriting its original closure evidence.
21. [ ] `plans/README.md` registers Plan 116, extends the dependency chain, and records Plan 115 as complete with this post-closure follow-up.
22. [x] Active update/restart documentation and skills no longer overstate exact-executable awareness before Windows quiescence.
23. [x] Focused deterministic tests cover the Windows foreign-running SCM regression and Unix foreign-stopped-manager/direct-running regression.
24. [ ] Existing Windows SCM smoke and macOS/Windows/MSRV CI remain green with no new workflow/job/matrix.
25. [ ] Workspace fmt/clippy/tests, default local check, release preflight, and Rust 1.75 check pass on the implementation tree.
26. [ ] Closure records the final implementation SHA, exact CI run used for native-platform truth, focused lifecycle regression results, and any platform limitation without overstating evidence.
