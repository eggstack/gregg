# Plan 113: Plan 112 install/uninstall ownership corrective pass

Status: planned; ready for implementation.

Depends on: Plan 112 and the settled installer/update/startup ownership from Plans 099-105.

This is a post-closure corrective pass. Plan 112 remains the historical record of what landed in `ce7ea5e` plus `56980f8` / `a62c7a2`; this plan owns only the defects found in review after that closure.

## Objective

Close four lifecycle defects without turning Gregg into a package manager or redesigning startup management:

1. make `greggd uninstall` classify systemd, launchd, cron, and Windows SCM integration relative to the exact invoked daemon executable instead of treating every Gregg artifact on the host as owned by the current invocation;
2. preserve the Windows SCM distinction between not-installed, stopped, running, and query failure, including the registered executable path, so an unmanaged foreground daemon cannot be mistaken for a stopped service;
3. make Cargo-owned Unix uninstall preserve Cargo bookkeeping **and** complete the rest of the selected component lifecycle instead of returning before startup teardown or `--purge`;
4. make source-build/Cargo bootstrap fallback flow through the same daemon post-install/startup finalization as the prebuilt path on Unix and Windows.

The intended result is narrow: an uninstall mutates only resources bound to the binary being removed, and every bootstrap acquisition path receives the same verified post-install behavior.

## Confirmed post-closure findings

### 1. Daemon startup discovery is host-global

`crates/greggd/src/uninstall.rs` currently derives:

```text
systemd_teardown = canonical unit exists OR greggd is active
launchd_teardown = canonical plist exists OR Gregg label is loaded
cron_teardown    = current crontab contains the Gregg marker
scm_teardown     = SCM looks running/stopped
```

Those facts describe the host, not ownership by the exact `current_exe()` being uninstalled.

A user-local or disposable `greggd` can therefore discover a separate system installation and plan teardown of that system installation. The Plan 112 closure record observed this directly on the Ubuntu host: the disposable daemon uninstall could not complete because host-global systemd discovery always planned teardown of the pre-existing system daemon.

The correction is not to ignore native managers. It is to classify native artifacts as **owned**, **foreign**, **absent**, or **unknown** relative to the executing binary.

### 2. Windows SCM discovery collapses `NotInstalled` into `Stopped`

The native SCM adapter now returns `ServiceState::NotInstalled` correctly, but uninstall discovery calls `ServiceManager::is_active()`, which maps both `NotInstalled` and `Stopped` to `false`. That `false` is then interpreted as `ScmDiscovery::Stopped`.

Consequences:

- no SCM registration can be mistaken for an installed-but-stopped service;
- a foreground Windows daemon can appear manager-owned when it is not;
- the `blocked_running` safety path can be skipped;
- SCM query failure can degrade into a state that permits executable deletion instead of blocking when ownership is unknown.

Uninstall needs the full service state and registration identity, not a boolean active predicate.

### 3. Cargo-owned uninstall returns before the rest of the lifecycle

Both client and daemon uninstall paths special-case positive Cargo ownership before ordinary execution.

On Unix the code delegates to:

```text
cargo uninstall --root <root> <package>
```

and returns immediately.

For `gregg`, this means `--purge` is not applied. For `greggd`, it additionally means owned cron/startup integration and direct-daemon shutdown can be skipped.

Cargo must remain authoritative for deleting a Cargo-managed executable, but package-manager ownership of the executable does not imply ownership of Gregg configuration or Gregg-created startup integration.

### 4. Bootstrap Cargo fallback bypasses daemon post-install work

`packaging/install.sh` and `packaging/install.ps1` now stage Cargo builds privately, which correctly avoids stale Cargo metadata. However their source-only/fallback branches return after copying the binary.

That bypasses the ordinary daemon finalization path:

- Unix fallback can skip `greggd startup install`, so an updated daemon may not be restarted/re-registered;
- Windows fallback can skip the shared default-config/SCM stop-replace-register-restart path and may try to overwrite a running image directly.

Acquisition method must not decide whether daemon post-install lifecycle runs.

## Scope decisions

### 1. Model startup ownership explicitly

Introduce a small uninstall-specific ownership model, conceptually:

```rust
enum ArtifactOwnership {
    Absent,
    Owned,
    Foreign,
    Unknown,
}
```

Exact type names are implementation detail. The required semantics are not.

The uninstall plan must distinguish:

- artifact exists and targets the exact executing `greggd` binary -> **owned**;
- artifact exists but targets another Gregg executable -> **foreign**;
- artifact does not exist -> **absent**;
- ownership cannot be established safely -> **unknown**.

Only **owned** artifacts are eligible for mutation.

`--dry-run` should render foreign/unknown state truthfully enough that an operator can see why a host-level Gregg service or watchdog is being preserved.

Do not treat a path containing `greggd`, a canonical service name, or a managed marker alone as proof that the artifact belongs to the current executable.

### 2. Reuse one executable-path equivalence policy

Use one existing/small path-identity helper for startup ownership comparisons.

Requirements:

- exact existing files should converge across ordinary equivalent path spellings where the platform permits canonicalization;
- a user-local `~/.local/bin/greggd`, Cargo binary, temporary test binary, and `/usr/local/bin/greggd` remain distinct installations;
- do not compare by basename alone;
- do not add a persistent install receipt merely to solve this correction.

If an artifact's target cannot be interpreted safely, classify it `Unknown` rather than guessing ownership.

### 3. systemd ownership is bound to its executable target

The Gregg systemd integration is the canonical `greggd.service` identity, but an uninstall invocation owns it only when the service target is the exact binary being removed.

For the normal Gregg-generated unit this means the standard `/usr/local/bin/greggd` installation owns the unit; a user-local/Cargo/disposable binary does not.

Implementation may use the settled canonical unit contract or a narrow read-only `ExecStart` target parser if that materially improves correctness. Do not build a general systemd parser.

Required behavior:

- exact system installation -> existing stop/disable/remove/reload sequence;
- user-local/Cargo/disposable invocation while the system unit exists -> preserve systemd integration;
- unknown/unreadable ownership -> do not remove the unit;
- no internal `sudo`.

A foreign active system service must not be stopped merely because a different binary is being uninstalled.

### 4. launchd ownership follows the same rule

The canonical Gregg launchd label/plist belongs to the exact binary referenced by its launch configuration.

A non-system `greggd` invocation must not boot out or remove the system launchd job solely because `com.eggstack.greggd` exists.

Keep the existing bounded `launchctl` path and canonical label. Do not add a general launchd discovery framework or XML dependency.

### 5. Cron ownership must include the executable path

The managed marker is necessary evidence of Gregg ownership, but it is not sufficient evidence that the block belongs to the current binary.

Extend the existing cron parsing boundary narrowly enough to classify the managed block's command target.

Required behavior:

- marker + command targeting exact current executable -> owned and removable;
- marker + command targeting a different Gregg executable -> foreign and preserved;
- malformed/ambiguous managed block -> unknown and preserved with an actionable diagnostic;
- unrelated crontab entries remain byte-for-byte preserved under the existing helper contract;
- no enumeration/mutation of other users' crontabs.

Do not infer ownership solely from the marker or from the config path.

### 6. Windows SCM discovery must expose full registration state

Do not use `is_active()` as uninstall discovery authority.

Extend the existing service abstraction narrowly so uninstall can obtain, in one bounded native query surface:

- `ServiceState` including `NotInstalled`;
- registered executable/image path when the service exists;
- the configured daemon config path if it is readily available from the registered command line, or enough command-line text to compare the executable target safely;
- typed access-denied/query errors.

Prefer `windows-service` native query/config APIs already in the dependency graph. Do not introduce a permanent `sc.exe query` parser.

The uninstall ownership decision must be:

```text
NotInstalled                         -> absent
registered target == current_exe    -> owned
registered target != current_exe    -> foreign
query/parse cannot establish target -> unknown
```

`Unknown` is safety-significant: it must block SCM mutation and must not be silently treated as no service.

### 7. Correct unmanaged Windows foreground behavior

With the full SCM state preserved:

- no SCM registration + configured Gregg endpoint running -> refuse self-delete because Windows has no direct control-socket stop path;
- owned running SCM service -> stop/wait/delete through the native service abstraction;
- owned stopped SCM service -> delete registration;
- foreign SCM service -> preserve it;
- unknown SCM ownership/query failure -> fail before executable/config mutation with a precise diagnostic.

If a foreign running SCM service clearly owns the endpoint/config being probed, it is not evidence that the current unrelated executable is running. Preserve the foreign service and allow removal of the unrelated executable after ordinary preflight.

Do not add process-name scanning or PID discovery to disambiguate foreground instances.

### 8. Direct Unix stop must not target a foreign managed daemon

The Unix direct control path remains the stop mechanism only for the selected config identity when no **owned** native manager controls the executing installation.

When a foreign active manager exists, use its known config identity where available to avoid confusing its endpoint with an unmanaged daemon from the current executable.

Required cases:

- foreign system manager using the same standard config/endpoint -> preserve it; do not send the current invocation's direct stop at that managed daemon;
- foreign manager using a different config while the selected custom config endpoint is running -> the existing direct stop path may stop the custom daemon;
- uncertain direct stop continues to block deletion.

Do not weaken the Plan 080-082 control-socket identity rules.

### 9. Do not let teardown helpers rediscover ownership differently

The pure uninstall plan is the authority for **whether** an artifact is owned.

Execution helpers may re-check presence/state for races and idempotence, but they must not widen a plan from `Foreign`/`Unknown` to removable merely because a canonical service name exists.

Keep discovery/planning and mutation visibly separate so `--dry-run` and execution share the same ownership decisions.

### 10. Cargo-owned Unix client uninstall completes purge semantics

On Unix, positive Cargo ownership changes who deletes the executable; it does not end the command early.

For `gregg uninstall`:

1. resolve the plan and preflight requested config mutation;
2. delegate executable/package removal to Cargo;
3. only after successful Cargo removal, apply `--purge` to the resolved client config if requested;
4. preserve config by default.

This ordering avoids deleting configuration if Cargo refuses to uninstall the package.

On Windows retain the Plan 112 pre-mutation handoff behavior for Cargo-owned running images unless implementation can prove a smaller safe native solution. The required correction does not justify a new helper executable, shell script, or package-manager subsystem.

### 11. Cargo-owned Unix daemon uninstall completes daemon lifecycle

For Unix `greggd` with positive Cargo ownership:

1. fully resolve ownership and preflight executable/startup/config mutations;
2. tear down only startup integration owned by this executable;
3. stop a direct/unmanaged daemon when the safe control identity proves it is the selected instance;
4. delegate package/executable removal to `cargo uninstall --root ...`;
5. apply `--purge` only after Cargo succeeds;
6. preserve config when `--purge` is absent.

If Cargo removal fails after manager teardown, return a precise retryable error. The binary remains installed and config remains preserved; do not pretend the uninstall completed.

Windows Cargo-owned daemon behavior may continue to fail before mutation with the exact Cargo handoff command as allowed by Plan 112. Document that this is a package-manager handoff, not a completed Gregg lifecycle.

### 12. Dry-run must not hide Cargo/startup work

Unix Cargo-owned dry-run output must show the full intended lifecycle, including owned startup resources, config preserve/purge intent, and Cargo package removal.

Do not return from `render()` immediately after printing the Cargo handoff if additional Gregg-owned resources would be changed on a platform where real execution would change them.

Windows pre-mutation handoff should explicitly say that no startup/config mutation will occur before the operator completes the Cargo-owned removal path.

### 13. Refactor installer acquisition from finalization

Both bootstrap scripts should have one conceptual flow:

```text
classify destination
-> acquire/stage verified candidate (prebuilt OR Cargo)
-> component-safe replacement
-> shared component post-install finalization
-> report first install/update
```

Do not let `Cargo fallback` be a terminal branch that bypasses finalization.

Exact shell/PowerShell function names are implementation detail.

### 14. Unix Cargo fallback must run the same daemon finalization

After a staged Cargo fallback installs `greggd` to the canonical selected destination, run the same existing startup delegation used by the prebuilt path:

```text
<dest>/greggd startup install
```

Preserve current privilege behavior:

- root/system install can update/restart systemd/launchd integration;
- user-local install does not internally elevate and receives the existing exact guidance on system-manager hosts;
- cron-only environments may configure the current user's watchdog.

A fallback update of an installed daemon must not leave the old running process indefinitely just because acquisition came from Cargo.

### 15. Windows Cargo fallback must use the same SCM-safe replacement path

For Administrator daemon installs/updates, source-only fallback must share the prebuilt finalization sequence:

1. stage and verify candidate before service mutation;
2. preserve/create default config as today;
3. stop and wait for an existing `greggd` SCM service when needed;
4. replace `greggd.exe`;
5. create/update the SCM registration;
6. restart and wait for Running;
7. preserve sibling `gregg.exe`.

Do not `Copy-Item -Force` a staged fallback daemon over a running Windows image before the service has been stopped.

For non-Administrator/user-local fallback, retain user-local installation and no implicit service registration.

### 16. Keep installer classification behavior identical across acquisition methods

The Plan 112 destination contract remains:

- absent -> first install;
- valid same-component destination -> update/replacement;
- foreign/unidentifiable destination -> refuse before overwrite;
- same privilege/scope only;
- no global install search;
- no internal elevation.

Cargo fallback must use the same classification and first-install/update reporting as the prebuilt path.

## Implementation sequence

### Step 1: lock ownership decisions into pure data

Refactor daemon uninstall discovery/plan types first so owned/foreign/absent/unknown state is explicit for systemd, launchd, cron, and SCM.

Add path-equivalence and artifact-target helpers beside the existing startup/service owners rather than embedding platform parsing in CLI dispatch.

### Step 2: correct Windows SCM query fidelity

Expose full native state plus registration target through the current `ServiceManager`/`ScmAdapter` boundary, retain fake-adapter determinism, and make access-denied/query failure explicit.

Then update uninstall discovery to stop using `is_active()`.

### Step 3: correct manager/direct-stop selection

Make the pure plan remove only owned artifacts, preserve foreign artifacts, block unknown unsafe states, and choose Unix direct-stop only when the selected configured daemon is not explained by a manager belonging to another installation.

### Step 4: sequence Cargo-owned uninstall through the ordinary Unix lifecycle

Remove the early-return behavior from client and daemon Unix execution while keeping Cargo authoritative for package deletion and keeping Windows pre-mutation handoff bounded.

### Step 5: unify bootstrap post-install finalization

Refactor `install.sh` and `install.ps1` so prebuilt and staged-Cargo acquisition converge before replacement/finalization. Do not duplicate service/startup logic into a second fallback branch.

### Step 6: reconcile Plan 112 documentation claims

Do not rewrite Plan 112's closure record. Append the short post-closure correction note naming this plan.

Update active install/uninstall documentation where behavior or edge-case guidance changes.

## Files likely touched

```text
crates/gregg-update/src/uninstall.rs
crates/gregg/src/uninstall.rs
crates/greggd/src/uninstall.rs
crates/greggd/src/service/mod.rs
crates/greggd/src/service/windows.rs
crates/greggd/src/startup/systemd.rs
crates/greggd/src/startup/launchd.rs
crates/greggd/src/startup/cron.rs
packaging/install.sh
packaging/install.ps1
scripts/tests/test-install-rerun.sh
scripts/smoke-windows.ps1
README.md
docs/installation.md
docs/client.md
docs/daemon.md
packaging/README.md
architecture/gregg-update.md
architecture/scripts-and-packaging.md
architecture/greggd-daemon.md
AGENTS.md
.opencode/skills/gregg-client/SKILL.md
.opencode/skills/greggd-daemon/SKILL.md
.opencode/skills/release-process/SKILL.md
plans/112-installer-upgrade-and-cross-platform-uninstall.md
plans/README.md
```

Do not touch every file mechanically; update only documentation whose active contract changes.

## Required deterministic tests

At minimum cover:

- system `/usr/local/bin/greggd` owns the canonical systemd/launchd artifact;
- user-local/Cargo/disposable executable sees canonical system manager integration as foreign and never plans its removal;
- foreign active system manager using the same standard config does not trigger Unix direct-stop against that daemon;
- foreign manager on a different config does not suppress the safe direct-stop path for a selected custom-config daemon;
- cron marker targeting the exact current executable is removable;
- cron marker targeting a different Gregg executable is preserved;
- malformed/ambiguous managed cron ownership is not guessed;
- Windows `NotInstalled` remains distinct from `Stopped` through production-plan logic;
- Windows no-SCM + running endpoint blocks binary deletion;
- Windows registered target different from `current_exe` is foreign and never unregistered;
- Windows SCM query/access denied blocks before binary/config mutation;
- owned running SCM registration preserves stop -> wait -> delete ordering;
- Unix Cargo-owned `gregg --purge` removes Cargo package then selected config;
- Cargo failure leaves client config intact;
- Unix Cargo-owned `greggd` executes owned manager/direct-stop lifecycle before Cargo package removal and purges config only after Cargo succeeds;
- Windows Cargo handoff remains zero-mutation before the operator command;
- Unix staged-Cargo daemon fallback reaches the same startup-install finalization as prebuilt acquisition;
- Windows staged-Cargo daemon fallback uses the same stop/replace/register/restart finalization as prebuilt acquisition;
- first-install/update/foreign destination classification is unchanged for both acquisition methods.

Use injected command/service adapters and existing shell-script harnesses. Do not add a new general mocking framework.

## Operational verification

### Ubuntu

Use disposable executable/config paths and the existing host only where safe.

The correction should demonstrate both cases that Plan 112 could not:

1. **foreign system-service preservation**

```text
pre-existing system greggd remains active
-> run disposable/user-local greggd uninstall --dry-run
-> systemd integration is reported foreign/preserved
-> real uninstall removes only disposable binary
-> system service remains active and unchanged
```

2. **direct custom-config lifecycle**

```text
start disposable greggd run --config <temp> on a non-conflicting port
-> health ready
-> dry-run reports direct stop + exact binary removal, not foreign systemd teardown
-> real uninstall
-> endpoint absent
-> disposable binary absent
-> temp config preserved unless --purge
-> pre-existing system service remains active
```

Do not mutate the operator's real unit/config to manufacture the proof.

### Windows

Extend the existing Windows job/smoke only as needed; do not add another job or matrix.

Required native proofs:

```text
SCM absent + foreground daemon evidence -> uninstall refuses binary deletion
foreign SCM registration target -> current unrelated binary survives service mutation because no mutation occurs
owned SCM registration -> existing component-safe uninstall still removes greggd and leaves gregg.exe runnable
```

If the smoke cannot safely launch the unmanaged foreground case, retain deterministic fake-adapter coverage for that branch and use the existing owned-SCM smoke for native API truth.

### macOS

Existing native compile/tests are sufficient for launchd path/ownership logic unless implementation introduces a macOS-only API that cannot be covered deterministically. Do not add privileged launchd CI.

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

The release preflight is required because bootstrap installer behavior changes.

Use one ordinary existing CI run after implementation for native Windows/macOS/MSRV truth. Record its exact implementation SHA and run ID in this plan's closure record.

## Preserved exclusions

Do not add under Plan 113:

- global discovery/removal of every Gregg installation;
- a persistent install receipt/registry;
- Homebrew, apt/dpkg, rpm, winget, MSI, or other package-manager integration;
- an `uninstall --all` command;
- automatic deletion of the `greggd` system user/group;
- internal `sudo`, UAC prompting, or privilege escalation;
- process-name scanning, `pkill`, PID files, or a public shutdown endpoint;
- mutation of other users' crontabs;
- recursive deletion of install/config directories;
- a generic systemd/launchd parser framework;
- a new self-delete/package-manager helper executable solely for Windows Cargo handoff;
- a new dependency when the existing path/service/startup surfaces suffice;
- new workflows, jobs, matrices, privileged runners, evidence bundles, or release automation;
- protocol, metrics, collector, TUI, EggPool, or scheduler changes;
- rewriting Plan 112's historical closure evidence as though these defects were never present.

## Acceptance criteria

Plan 113 is complete only when:

1. [ ] Daemon uninstall startup ownership is explicitly classified relative to the exact invoked executable; host-global presence alone never authorizes teardown.
2. [ ] A user-local/Cargo/disposable `greggd uninstall` cannot stop/disable/remove a foreign systemd or launchd installation.
3. [ ] Managed cron removal requires the managed block to target the current executable; foreign/ambiguous Gregg blocks are preserved.
4. [ ] Windows uninstall discovery preserves `ServiceState::NotInstalled` versus `Stopped` and obtains the registered executable target through the native SCM abstraction.
5. [ ] SCM query/access-denied/unknown ownership cannot silently degrade to self-deletion.
6. [ ] Windows no-SCM plus a running Gregg endpoint follows the unmanaged-running safety path and refuses deletion.
7. [ ] A foreign SCM registration is preserved and never deleted by an unrelated `greggd` executable.
8. [ ] Unix direct-stop selection distinguishes a selected custom-config daemon from a foreign active manager and never knowingly stops the foreign managed daemon.
9. [ ] Unix Cargo-owned client uninstall delegates package removal to Cargo while still honoring default config preservation and post-success `--purge`.
10. [ ] Unix Cargo-owned daemon uninstall performs owned startup/direct-stop lifecycle, delegates executable removal to Cargo, and applies `--purge` only after Cargo succeeds.
11. [ ] Windows Cargo-owned uninstall retains a truthful zero-mutation pre-exit handoff unless a smaller proven-safe native solution is implemented.
12. [ ] Cargo-owned dry-run output no longer hides Gregg-owned lifecycle work on platforms where execution would perform it.
13. [ ] Unix staged-Cargo bootstrap fallback converges with the prebuilt path before daemon startup finalization.
14. [ ] Windows staged-Cargo bootstrap fallback converges with the prebuilt path before config/SCM stop-replace-register-restart finalization.
15. [ ] Same-scope install/update/foreign-destination classification remains unchanged across prebuilt and Cargo acquisition.
16. [ ] Ubuntu disposable foreign-system-service preservation and direct custom-config lifecycle smokes both pass without altering the operator's system installation.
17. [ ] Existing Windows component-safety smoke remains green and new SCM ownership/NotInstalled regressions are covered by deterministic tests/native smoke as appropriate.
18. [ ] Existing macOS/Windows/MSRV CI remains green with no new workflow/job/matrix.
19. [ ] Active install/uninstall documentation and skills describe exact-executable startup ownership, Cargo handoff/delegation, and fallback finalization accurately.
20. [ ] Plan 112 receives a short post-closure correction note pointing to Plan 113 without rewriting its original closure record.
21. [ ] `./scripts/check-local.sh`, release preflight, workspace fmt/clippy/tests, and Rust 1.75 check pass on the implementation tree.
22. [ ] The closure record names the implementation SHA, exact CI run used for native-platform truth, Ubuntu lifecycle smoke results, and any platform limitation without overstating evidence.
