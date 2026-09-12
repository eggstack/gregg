# Plan 115: Plan 113 restart ownership, Unix installer activation, and elevation diagnostics corrective pass

Status: planned; ready for implementation.

Depends on: Plan 113 and the settled startup/restart/update contracts from Plans 100-102. It is independent of Plan 114's TUI work and of the remaining Plan 091 soak record.

This is a narrow post-closure corrective pass. Plan 113 remains the historical record of the install/uninstall ownership work that landed in `ec6cb85` / `7295c6e`; this plan owns only the residual restart/activation/diagnostic findings discovered after that closure.

## Objective

Close three product defects and one planning-record inconsistency without redesigning Gregg's startup model:

1. make `greggd restart` choose systemd, launchd, or Windows SCM only when that manager registration belongs to the exact invoked `greggd` executable, rather than using host-global manager presence;
2. make a same-scope Unix user-local bootstrap replacement of a running direct/cron `greggd` actually transition the running daemon to the newly installed executable while preserving the stopped state of an inactive daemon;
3. make shared Windows update/uninstall permission diagnostics instruct the operator to rerun from an Administrator shell instead of emitting Unix `sudo ...` commands;
4. reconcile Plan 113's stale index status and append a short post-closure note pointing to this corrective pass.

The result must remain small: no new service manager, PID registry, package manager, workflow, or generalized process discovery.

## Confirmed post-closure findings

### 1. `greggd restart` still uses host-global manager state

Plan 113 made uninstall ownership exact-executable-aware, but `crates/greggd/src/startup/state.rs::startup_state()` still classifies restart state from canonical service presence/activity alone:

```text
systemd unit present/active -> Systemd*
launchd plist present/loaded -> Launchd*
SCM active/stopped          -> WindowsService*
```

`restart_daemon()` then dispatches directly to that manager.

A user-local, Cargo, or disposable `greggd` invoked on a host with a separate system installation can therefore attempt to restart the foreign system service. Unprivileged callers usually fail at permissions, but an elevated invocation can mutate the wrong installation. This is the same ownership boundary Plan 113 corrected for uninstall and should reuse the same exact-executable evidence.

### 2. Unix user-local installer replacement can leave the old daemon running

`packaging/install.sh` now correctly converges prebuilt and staged-Cargo acquisition into `finalize_greggd_install`, but that finalization delegates to `greggd startup install`.

For systemd/launchd, startup installation starts/restarts the managed service. For cron/direct operation, startup installation installs/refreshes the watchdog but does not replace a daemon that is already healthy. On Unix, replacing the executable file does not replace the already-running process image; the old daemon can therefore continue indefinitely while `croncheck` sees a healthy endpoint and has no reason to respawn it.

This affects same-scope user-local daemon updates regardless of whether the candidate came from a release asset or staged Cargo fallback.

### 3. Shared update/uninstall permission hints are Unix-specific on Windows

`crates/gregg-update/src/stage.rs` and `crates/gregg-update/src/uninstall.rs` construct elevation guidance as `sudo <exe> ...` in shared permission paths.

On Windows, failures against `%ProgramFiles%` or another protected location can therefore produce unusable guidance such as `sudo C:\...\greggd.exe uninstall`. The operation correctly fails, but the recovery instruction is platform-wrong.

### 4. Plan 113's index row is stale

The Plan 113 file and roadmap paragraph record it complete at `7295c6e` with CI `34711999742`, but its table row in `plans/README.md` still says `planned; ready for implementation`.

This pass should correct the active index while preserving Plan 113's original closure record and adding only a short post-closure note for the newly discovered defects.

## Scope decisions

### 1. Restart manager dispatch must be exact-executable-aware

Do not use host-global `startup_state()` as sufficient authority for `greggd restart`.

Reuse the ownership surfaces introduced by Plan 113:

- `systemd_artifact_ownership(current_exe)` and its parsed config identity;
- `launchd_artifact_ownership(current_exe)` and its parsed config identity;
- Windows `ServiceManager::query_registration()` with full `ServiceState` and registered executable path;
- `gregg_update::uninstall::paths_equivalent` for executable/config path identity.

A small restart-specific decision type/helper is acceptable. Do not overload uninstall planning or create a generalized installation registry.

Required dispatch semantics:

```text
owned systemd/launchd registration -> manager restart
foreign systemd/launchd registration -> never call that manager for this executable
unknown active manager ownership -> fail closed; do not guess
no manager owned by current executable -> Unix direct/cron restart path

owned Windows SCM registration -> SCM restart
foreign Windows SCM registration -> preserve it and return a precise error
unknown/query-failed SCM registration -> fail closed
SCM not installed -> no SCM restart; Windows has no direct restart fallback
```

For a foreign Unix manager, its known config identity must continue to protect the manager-owned daemon from the direct control path. If the foreign manager uses the same selected config, return an ownership error rather than sending the direct stop. If it uses a different config, the existing config-specific direct restart may operate on the selected daemon.

### 2. Keep the direct/cron restart primitive unchanged unless a correctness fix is required

`restart_cron_direct()` already uses the config-specific Unix control socket, waits for endpoint absence, and spawns only after the endpoint is definitely absent. Preserve those Plan 080-082 safety rules.

Do not add process-name scanning, PID files, `pkill`, a public shutdown endpoint, or a second direct-control protocol.

### 3. Unix bootstrap replacement must preserve running/stopped state

Apply the activation correction only to `greggd` **same-scope replacement**, not first install.

Before overwriting a user-local Unix `greggd` destination, record whether the selected default-config daemon currently returns a valid Gregg status. This is activation intent only; it is not manager-ownership evidence.

After the candidate has been verified, copied into place, and ordinary startup finalization has run:

- if the prior user-local daemon was not running, do not start it merely because the installer was rerun;
- if it was running, transition the selected direct/cron daemon to the new executable using existing safe CLI primitives;
- do not route this user-local activation through a foreign systemd/launchd manager.

The preferred bounded shell-level sequence for user-local activation is the existing Unix direct surface:

```text
<new-dest>/greggd stop
<new-dest>/greggd croncheck
```

`stop` is config-specific on Unix and does not call service managers; `croncheck` only spawns the current executable after the configured endpoint is definitely absent. If implementation instead uses `greggd restart`, it may do so only after the exact-executable restart-dispatch correction above is in place.

Do not infer success from endpoint health alone when a stop/restart step reports uncertainty. If binary replacement succeeded but activation fails, return a nonzero installer result with a truthful `binary updated; daemon activation/restart failed` diagnostic and an exact retry command. Do not roll back to an unverified old binary.

### 4. Do not change system-scope managed installer behavior unnecessarily

System/root bootstrap installs already flow through systemd/launchd startup installation, which starts or restarts the service. Do not add a blind second restart to those paths.

The new user-local activation logic must not stop, restart, disable, or rewrite a foreign system service. Preserve Plan 113's exact-executable ownership boundary.

### 5. Prebuilt and Cargo fallback must share the activation decision

The running-state capture and post-replacement activation belong around the common destination/finalization flow, not inside only the release-asset or Cargo branch.

Required invariants:

- same destination classification for prebuilt and Cargo candidates;
- same first-install/update reporting;
- same user-local active-daemon transition;
- same foreign-destination refusal;
- no persistent Cargo metadata from staged fallback.

### 6. Centralize platform-correct elevation guidance in `gregg-update`

Introduce one small shared helper for permission rerun guidance rather than keeping scattered `sudo` formatting.

Required output semantics:

- Unix: preserve the existing `sudo <exact-exe> <operation>` guidance;
- Windows: say to rerun the exact executable/operation from an **Administrator** terminal/PowerShell; never print `sudo`;
- no automatic UAC launch, `runas`, PowerShell elevation subprocess, or internal privilege escalation;
- preserve the requested operation and flags, including `uninstall --purge`.

Use the helper for at least:

- update destination writability preflight;
- update replacement permission failure;
- uninstall destination writability preflight;
- self-delete permission failure.

Keep application adapters thin; they should display the shared error rather than reconstructing platform guidance independently.

### 7. Reconcile Plan 113 without rewriting history

Append a short post-closure correction note to Plan 113 stating that later review found:

- restart dispatch still used host-global manager state;
- Unix user-local direct/cron bootstrap replacement could leave the old process running;
- shared Windows update/uninstall elevation hints could emit `sudo`.

Point to Plan 115 for correction. Do not edit Plan 113's implementation SHA, CI run, or historical evidence.

Update `plans/README.md` so:

- Plan 113's table row is `complete` with its implementation/CI truth;
- Plan 115 has a roadmap paragraph and table row;
- the dependency chain extends through 115;
- the dependency note states that 115 depends on 113/100-102, is independent of completed Plan 114, and does not depend on the remaining Plan 091 soak record.

## Implementation sequence

### Step 1: make restart dispatch ownership-aware

Refactor the restart decision first. Reuse Plan 113's systemd/launchd/SCM executable-target evidence and path-equivalence helper. Keep manager mutation behind a positive `Owned` decision.

Add deterministic tests before wiring the installer to any restart-capable surface.

### Step 2: add platform-correct shared elevation hints

Factor the shared rerun-hint formatter and replace the Unix-only strings in update/uninstall permission paths. Add pure/cfg tests that lock both Unix and Windows wording without requiring elevated execution.

### Step 3: preserve active user-local daemon state across bootstrap replacement

Extend `packaging/install.sh` common daemon flow to capture pre-replacement running intent for a user-local same-scope `greggd`, then reactivate only when it was previously running.

Use the existing safe direct Unix `stop` + `croncheck` path unless the now-owned-aware `restart` path is smaller and equally provable. Do not add shell parsing of systemd/launchd output or duplicate manager detection in Bash.

### Step 4: extend the existing installer harness

Teach `scripts/tests/test-install-rerun.sh` fake daemon candidates to model `status`, `stop`, `croncheck`, and startup calls. Prove prebuilt and Cargo replacement take the same activation path and that first install/stopped replacement do not start a daemon unexpectedly.

### Step 5: perform one narrow Unix lifecycle smoke

Use a disposable HOME/config/port and release `greggd` binary. Demonstrate that a running user-local daemon survives a same-scope installer replacement as a **new process** serving the same config, while no system service is touched.

### Step 6: reconcile planning/documentation

Append the Plan 113 correction note, register Plan 115, and update only active documentation/skills whose restart/elevation/install contracts change.

## Files likely touched

```text
crates/gregg-update/src/stage.rs
crates/gregg-update/src/uninstall.rs
crates/greggd/src/startup/state.rs
crates/greggd/src/startup/install.rs
crates/greggd/src/service/mod.rs
crates/greggd/src/service/windows.rs
packaging/install.sh
scripts/tests/test-install-rerun.sh
README.md
docs/installation.md
docs/daemon.md
architecture/gregg-update.md
architecture/greggd-daemon.md
architecture/scripts-and-packaging.md
AGENTS.md
.opencode/skills/greggd-daemon/SKILL.md
.opencode/skills/release-process/SKILL.md
plans/113-plan112-install-uninstall-ownership-corrective-pass.md
plans/README.md
```

Do not touch every file mechanically. Update only files whose active contract changes.

## Required deterministic tests

At minimum cover:

- systemd registration targeting the current executable dispatches restart through systemd;
- foreign systemd registration never receives a restart command from another executable;
- unknown active systemd ownership fails closed;
- launchd owned/foreign/unknown restart decisions mirror systemd;
- a foreign Unix manager using the same selected config blocks direct restart of that manager-owned daemon;
- a foreign Unix manager using a different config does not suppress safe config-specific direct restart;
- Windows owned SCM registration dispatches SCM restart;
- Windows foreign SCM registration is preserved and never restarted;
- Windows SCM query/parse failure fails closed;
- Windows `NotInstalled` does not become a stopped/owned SCM restart;
- Unix user-local `greggd` first install does not run the replacement-only activation sequence;
- Unix user-local stopped same-scope replacement remains stopped;
- Unix user-local running same-scope prebuilt replacement performs the direct activation sequence once;
- staged-Cargo user-local running replacement reaches the same activation sequence;
- activation failure after binary replacement returns a truthful nonzero result and does not claim full success;
- foreign/unidentifiable destination behavior remains unchanged;
- Unix elevation guidance still contains `sudo` and the exact operation;
- Windows update/uninstall elevation guidance contains `Administrator`, preserves the exact operation/flags, and contains no `sudo`.

Use the existing fake command/service adapters and shell installer harness. Do not add a generalized mocking framework.

## Operational verification

### Unix / Ubuntu

Run one disposable user-local lifecycle smoke on the implementation tree:

```text
build release greggd
-> use isolated HOME/default config on a non-conflicting port
-> seed/install a valid user-local greggd destination
-> start that destination and record its PID
-> verify /v2/healthz is valid
-> rerun the bootstrap installer at the same user-local scope with a verified candidate
-> verify the original PID exits
-> verify a new PID serves the same configured endpoint
-> verify health is valid after activation
-> verify no host systemd/launchd artifact was stopped or modified
```

A same-version candidate is acceptable for this smoke; the proof is process transition across executable replacement, not version transport. Keep this local; do not add a privileged CI job.

Also prove the stopped-state case with the deterministic installer harness rather than adding another operational smoke.

### Windows

Use the existing Windows CI job for native compilation/tests and existing SCM smoke. Add no workflow/job/matrix.

Native elevation is not required to test the diagnostic string; deterministic helper tests are sufficient. The existing Windows SCM smoke should remain green after restart ownership changes.

### macOS

Existing native compile/tests are sufficient for launchd ownership dispatch unless implementation introduces a macOS-only API that cannot be covered deterministically. Do not add privileged launchd CI.

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

The release preflight is required because `packaging/install.sh` changes.

Use one ordinary existing CI run after implementation for native Windows/macOS/MSRV truth. Record the exact implementation SHA and CI run ID in the Plan 115 closure record.

## Preserved exclusions

Do not add under Plan 115:

- global discovery or control of every Gregg installation;
- process-name scanning, PID files/registries, `pkill`, or a public shutdown endpoint;
- a new control protocol or network shutdown API;
- internal `sudo`, UAC prompting, `runas`, or automatic privilege escalation;
- a persistent install receipt/registry;
- Homebrew, apt/dpkg, rpm, winget, MSI, or other package-manager integration;
- changes to the metrics protocol, collectors, sampler, TUI, EggPool, or scheduler;
- first-install auto-start semantics beyond existing startup-manager behavior;
- automatic startup registration for a user-local install on a host where policy intentionally requires system-manager elevation;
- new dependencies when existing startup/service/update surfaces suffice;
- new workflows, jobs, matrices, privileged runners, evidence bundles, or release automation;
- rewriting Plan 113's historical implementation SHA, CI evidence, or completed acceptance record as though these residual defects were never present;
- unrelated cleanup or Plan 091 soak work.

## Acceptance criteria

Plan 115 is complete only when:

1. [ ] `greggd restart` no longer treats host-global systemd/launchd/SCM presence as ownership of the invoked executable.
2. [ ] Only a manager registration whose executable target matches the exact invoked `greggd` may receive restart mutation.
3. [ ] Foreign systemd/launchd registrations are preserved and cannot be restarted by a user-local/Cargo/disposable executable.
4. [ ] Unknown active Unix manager ownership fails closed rather than guessing or direct-stopping a potentially managed daemon.
5. [ ] Windows restart preserves `NotInstalled`, `Foreign`, `Owned`, and query/parse-failure distinctions through the native SCM registration query.
6. [ ] Foreign/unknown Windows SCM registration cannot be restarted by an unrelated executable.
7. [ ] The existing Unix config-specific direct restart safety rules remain intact.
8. [ ] Same-scope Unix user-local `greggd` replacement records prior running intent before overwriting the destination.
9. [ ] A previously running user-local direct/cron daemon is transitioned to the newly installed executable after successful replacement/finalization.
10. [ ] A previously stopped/unreachable user-local daemon is not started merely because the installer was rerun.
11. [ ] First install behavior is unchanged by the replacement-only activation logic.
12. [ ] Prebuilt and staged-Cargo candidates share the same active-daemon transition logic.
13. [ ] Activation uncertainty/failure after successful binary replacement is reported nonzero and never presented as a fully successful running update.
14. [ ] The activation correction never stops/restarts a foreign systemd/launchd service.
15. [ ] Unix update/uninstall permission hints retain the existing exact `sudo` rerun command.
16. [ ] Windows update/uninstall permission hints instruct rerun from an Administrator shell, preserve the exact operation/flags, and never contain `sudo`.
17. [ ] Existing destination classification, sibling-component preservation, Cargo staging cleanup, and uninstall ownership behavior remain unchanged.
18. [ ] The existing installer-rerun harness covers running/stopped/first-install activation and both prebuilt/Cargo acquisition paths.
19. [ ] The disposable Unix user-local lifecycle smoke proves old PID -> new PID transition with health restored and no system manager mutation.
20. [ ] Existing Windows SCM smoke and macOS/Windows/MSRV CI remain green with no new workflow/job/matrix.
21. [ ] Plan 113 receives a short post-closure correction note pointing to Plan 115 without rewriting its historical closure evidence.
22. [ ] `plans/README.md` marks Plan 113 complete, registers Plan 115, and records the correct dependency relationship.
23. [ ] Active restart/install/update/uninstall documentation and skills match the implemented ownership/elevation behavior.
24. [ ] `./scripts/check-local.sh`, release preflight, workspace fmt/clippy/tests, and Rust 1.75 check pass on the implementation tree.
25. [ ] The Plan 115 closure record names the implementation SHA, exact CI run used for native-platform truth, Unix lifecycle-smoke result, and any platform limitation without overstating evidence.
