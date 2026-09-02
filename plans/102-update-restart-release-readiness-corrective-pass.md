# Plan 102: update, restart, and release-readiness corrective pass

Status: complete.

Depends on: Plans 098-101 as implemented through `2eb0577` and documentation closure `479ecbd`; existing CI run `33683771778` is green but does not close the defects below.

Release gate: complete this plan before cutting the first binary-bearing release after `v1.0.11`.

## Objective

Close the concrete lifecycle and staging defects found in post-implementation review of Plans 099-101 without reopening Gregg's release architecture.

The existing direction remains correct:

- `ci.yml` stays the ordinary source/platform correctness workflow;
- `.github/workflows/release-binaries.yml` remains release-only and builds the existing five prebuilt targets;
- crates.io publication and tag creation remain manual;
- the release workflow may assemble/update a draft GitHub Release but must not publish crates or auto-publish the release;
- `greggd run` remains independent of systemd, launchd, cron, and Windows SCM;
- deployment/service-manager behavior remains isolated to explicit startup/restart/update paths;
- prebuilt binaries remain preferred, with Cargo as a bounded fallback only where already intended;
- no package-manager repositories, signing infrastructure, updater daemon, background version checks, or new permanent CI matrix are added.

The post-closure defects are narrow but release-relevant:

1. on Windows, `greggd update` can stop a healthy SCM service before the replacement binary is fully staged and verified;
2. direct/cron `greggd restart` can blindly spawn after an uncertain or failed stop, creating a competing-daemon race and reporting success before readiness is known;
3. service-manager probes/restarts are not consistently bounded and systemd/launchd nonzero exits lose enough stderr context to classify authorization failures reliably;
4. Cargo fallback uses a receive timeout around a background thread but does not terminate the underlying `cargo install` process when the timeout expires;
5. Rust updater staging is described as private but uses predictable timestamp/PID paths under the system temp directory without exclusive/private directory creation; Cargo fallback additionally copies a privileged candidate to another predictable temp path;
6. both updater modules blanket-disable Clippy lint families even though the workspace otherwise treats Clippy warnings as part of the normal correctness gate;
7. user-facing docs currently recommend `releases/latest/download/install.sh`, and some pinned examples reference `v1.0.11/install.sh`, even though the currently published `v1.0.11` release has no assets. Those commands are known to fail until the first binary-bearing release is published.

This plan corrects those boundaries only. It does not redesign the updater, introduce a package manager, or expand supported platforms.

## Baseline findings

### 1. Windows service downtime begins too early

Current `greggd update` performs roughly:

```text
latest-version lookup
permission probe
capture StartupState
STOP WINDOWS SCM SERVICE if running
resolve target
fetch release binary or compile Cargo fallback
fetch checksum
verify checksum
run candidate version
replace executable
restart SCM service
```

This means an otherwise healthy Windows daemon can be stopped before any replacement candidate exists. A network failure, GitHub error, missing checksum, checksum mismatch, candidate mismatch, or Cargo failure can leave the old on-disk executable untouched but the service stopped.

The service can also remain down for the entire Cargo fallback build, which may be minutes on a slower host.

The correct transaction boundary is:

```text
prepare candidate completely
    -> only then quiesce the running service
    -> replace
    -> restart
```

A failed preparation phase must not alter daemon running state.

### 2. Direct/cron restart violates the safe `croncheck` start boundary

Current `restart_cron_direct()` sends the Unix control stop and then continues toward a detached spawn for:

- `Stopped`;
- `NotRunning`;
- `Uncertain`;
- most control errors other than explicit permission denial.

It also reports `greggd started (direct/cron)` immediately after `Command::spawn()` succeeds. Successful process creation does not prove the child acquired the configured port, reached a valid Gregg health state, or avoided racing an old process that had not actually stopped.

This is inconsistent with the existing `croncheck` safety contract, which deliberately starts only after the configured Gregg endpoint is definitely absent and refuses to blind-spawn against ambiguous occupancy.

Restart must reuse that same authoritative absence/readiness boundary rather than weakening it.

### 3. Service-manager execution is not uniformly bounded or diagnostically rich

`is_systemd_environment()` describes its `systemctl` check as bounded, but the implementation ultimately uses synchronous `Command::status()` without a process deadline.

`run_systemctl()` also collapses any nonzero exit into a generic `io::Error` containing only the status code. Consequently, callers cannot reliably recognize the exact class that originally motivated Gregg's earlier runtime correction, such as:

```text
Interactive authentication required
Access denied
Not authorized
```

The same principle applies to launchd operations: a nonzero manager exit should preserve stderr and be bounded enough that `greggd restart`, `startup`, or `update` cannot hang indefinitely on a manager command.

Do not add a generalized process-execution framework. A small reusable bounded command helper local to `greggd` is enough if it clearly reduces duplicate manager/Cargo timeout code.

### 4. Cargo timeout does not stop Cargo

The updater currently launches `cargo install` inside a Rust thread and waits on an `mpsc::recv_timeout`. When the timeout expires, only the waiting Rust path stops. The thread and child Cargo process continue running.

Required semantics are real child-process semantics:

```text
spawn cargo
poll/wait until deadline
if deadline expires:
    kill child
    wait/reap child
    return timeout error
```

A timed-out update must not leave a compiler/build process consuming CPU, memory, disk, or network in the background.

### 5. Privileged staging is not actually private

The updater constructs temp paths using process ID plus a timestamp/nanosecond value under `std::env::temp_dir()` and uses ordinary directory creation. Cargo fallback then copies the verified binary to another synthesized filename directly under the system temp directory.

This is weaker than the documented "private staging directory" claim, particularly for an updater that may be intentionally rerun as root to replace `/usr/local/bin/gregg` or `/usr/local/bin/greggd`.

The repository already depends on `tempfile` through `self-replace`. Prefer a tiny `tempfile::Builder` / `TempDir` based implementation or equivalent exclusive creation that provides:

- unique creation rather than create-if-absent reuse;
- owner-private directory permissions on Unix;
- automatic cleanup where practical;
- no verified candidate copied into a predictable shared-temp top-level pathname merely to outlive an inner guard.

Do not add a custom secure-temp abstraction or a new dependency when the existing dependency can provide the needed primitive.

### 6. Blanket Clippy suppression hides updater defects

Both updater modules currently begin with broad module-level allowances for all/pedantic/nursery Clippy lint groups.

These files implement network retrieval, candidate validation, process lifecycle, executable replacement, and daemon restart. They should not be exempted wholesale from the same `-D warnings` gate used by the rest of the workspace.

Remove blanket suppression. If a small number of lints are genuinely inappropriate, use narrow item-level or module-level lint names with a short reason.

This step is not a request for cosmetic refactoring. Do not churn updater code merely to satisfy style lints; fix only real warnings and narrowly allow justified cases.

### 7. Current prebuilt-install documentation points at nonexistent assets

The currently published `v1.0.11` GitHub Release is source-only and has no assets. Therefore:

```text
https://github.com/eggstack/gregg/releases/latest/download/install.sh
https://github.com/eggstack/gregg/releases/download/v1.0.11/install.sh
```

currently resolve to no installer asset.

Active README/package documentation must not present those commands as working current installation paths before the first binary-bearing release exists.

Until such a release is published, Cargo installation remains the truthful working public path. The release/binary installer documentation may remain documented as the upcoming/available-on-binary-bearing-releases path, but it must not claim that today's `latest` release already provides it.

Do not remove the installer itself or the future release flow.

## Authoritative behavior after Plan 102

### Update transaction ordering

For both `gregg update` and `greggd update`, candidate preparation must complete before executable replacement.

For `greggd` specifically, a running Windows service must remain running throughout all preparation work.

Required Windows sequence:

```text
1. query crates.io latest stable version
2. resolve target / choose binary or Cargo fallback
3. download or build candidate into private staging
4. verify checksum when binary path is used
5. verify candidate `greggd X.Y.Z`
6. capture/confirm pre-update service state
7. verify replacement permission
8. if SCM service is running, stop it and require a confirmed successful stop
9. replace executable
10. restart only if it was running before step 8
11. if replacement succeeded but restart failed, report partial success exactly as today
```

If steps 1-7 fail, the running SCM service must not be touched.

If SCM stop fails for any reason, do not continue to replacement. Return a hard pre-replacement error and leave the old executable in place.

A service that was installed but stopped before update remains stopped after update.

Do not introduce rollback of a successfully replaced executable merely because restart later fails; preserve `UpdatedButRestartFailed` semantics.

### Direct/cron restart safety

A direct/cron restart must not start a second daemon unless the configured endpoint is definitely absent after the stop attempt.

Required decision model:

```text
send local control stop
    |
    +-- Stopped -> wait/probe until endpoint is definitely absent -> spawn once
    +-- NotRunning -> verify endpoint is definitely absent -> spawn once
    +-- Uncertain -> do not spawn; return actionable error
    +-- control error -> classify; do not blind-spawn unless an independent health probe proves definite absence
```

After spawn, perform a bounded readiness/identity probe using the same Gregg health semantics already used by `croncheck`/update. Return success only when the child becomes a valid Gregg endpoint (`Ready`, `Warming`, or other already accepted running state according to the final Plan 091 contract).

If the child exits or the readiness deadline expires, return a nonzero restart error. Do not print `started` merely because process creation succeeded.

Do not use PID files, process-name scanning, or shell `ps` parsing.

### Bounded service-manager commands

Systemd/launchd commands used for detection, install, restart, and update activation must have finite deadlines appropriate to local service-manager calls. A small default such as 5-10 seconds is adequate unless existing native evidence justifies another bound.

Capture stderr for nonzero exits and classify at least:

```text
permission / authentication failure
manager unavailable / command missing
generic manager operation failure
timeout
```

The user-facing privilege path remains explicit and non-interactive:

```text
sudo systemctl restart greggd
sudo launchctl kickstart -k system/com.eggstack.greggd
```

No internal `sudo`, authorization UI, or silent cron fallback is permitted.

### Real Cargo termination

Both updater modules must use a real child-process timeout for Cargo fallback.

Acceptance requires proof that after the timeout path returns:

- the Cargo child has been killed;
- it has been waited/reaped;
- no build continues in the background;
- the current installed executable remains untouched;
- a running `greggd` service was never stopped merely because Cargo preparation later timed out.

Prefer one small local helper per crate or a tiny shared helper if duplication becomes obviously unnecessary. Do not create a new workspace crate just for process timeouts.

### Private staging

Use exclusive temporary files/directories for downloaded and Cargo-built candidates.

On Unix, private staging should be owner-only where supported (`0700` directory; ordinary candidate mode before validation is acceptable). Do not rely on predictable top-level `/tmp/gregg-candidate-<pid>-<timestamp>` names.

The candidate still must pass:

```text
SHA-256 verification (release-binary path)
exact program/version identity
successful bounded `version` execution
```

before executable replacement.

### Current installation documentation

Before a binary-bearing release exists, active public docs must clearly distinguish:

```text
Current working installation: cargo install ...
Prebuilt installer: available beginning with binary-bearing releases produced by release-binaries.yml
```

Remove any pinned `v1.0.11/install.sh` example because that asset does not exist.

Once the first binary-bearing release is actually published, the normal `releases/latest/download/install.sh` examples may become the recommended path without changing the underlying installer design.

The plan closure record should identify whether that live release has happened. Do not fabricate live-release evidence during implementation.

## Implementation sequence

### Step 1: separate preparation from activation in `greggd update`

Primary file:

```text
crates/greggd/src/update.rs
```

Refactor only enough to make the transaction boundary explicit.

A small internal representation is acceptable, for example conceptually:

```rust
struct PreparedUpdate {
    staged_path: PathBuf,
    source: UpdateSource,
    target_version: String,
    _temp_guard: ...,
}
```

The exact type is not mandated. The important invariant is that candidate lifetime and cleanup remain valid until replacement, while service state is unchanged during preparation.

Add deterministic seams/tests that prove:

1. preparation failure does not request SCM stop;
2. checksum failure does not request SCM stop;
3. candidate mismatch does not request SCM stop;
4. Cargo fallback failure/timeout does not request SCM stop;
5. only a fully prepared update reaches the stop/replace phase;
6. a stop failure prevents replacement;
7. a previously stopped service is never started by update;
8. a running service is restarted only after successful replacement.

Do not require live crates.io/GitHub access in unit tests.

### Step 2: make direct/cron restart use definitive absence + readiness

Primary files:

```text
crates/greggd/src/startup.rs
crates/greggd/src/cli.rs        # only if an existing probe/helper must be exposed
crates/greggd/src/update.rs     # reuse the corrected restart semantics
```

Prefer reusing/factoring the final Gregg health probe semantics rather than maintaining a third subtly different parser.

If sharing the existing `croncheck` probe requires moving a small helper to a crate-private location, do so narrowly. Do not move service-manager logic into `cli.rs` or `run.rs`.

Tests must cover:

- stopped -> definitely absent -> spawn authorized;
- not running -> definitely absent -> spawn authorized;
- uncertain control outcome + occupied/ambiguous endpoint -> no spawn;
- control error + valid Gregg endpoint -> no spawn;
- control error + ambiguous non-Gregg endpoint -> no spawn;
- after authorized spawn, valid Gregg readiness -> success;
- spawn succeeds but health never becomes Gregg-ready within bound -> failure;
- no branch can report successful restart from `Command::spawn()` alone.

Use loopback fixtures and injected decisions; no PID scanning or privileged service setup is needed for these tests.

### Step 3: add a small bounded local-command helper for manager operations

Primary file:

```text
crates/greggd/src/startup.rs
```

If useful, place a tiny crate-private helper elsewhere under `greggd`; do not add a workspace crate.

The helper should support:

```text
program + args
bounded deadline
captured stdout/stderr
exit status
kill + wait on timeout
```

Apply it to the manager calls for which hanging/error classification matters, including at minimum:

```text
systemctl is-system-running / is-active / restart / startup install operations
launchctl print / bootstrap / bootout / kickstart where used synchronously
```

Do not convert unrelated short-lived commands unless doing so is necessary for one consistent path.

Preserve stderr in the resulting error so authentication failures can map to `InstallError::Permission` and print the exact elevated command.

Add focused tests against a tiny helper process/script or platform-independent test executable behavior for:

- success;
- nonzero with captured stderr;
- timeout kills/reaps child.

Do not add sleep-heavy tests to ordinary CI; keep test deadlines short and deterministic.

### Step 4: correct Cargo timeout and staging in both updater modules

Primary files:

```text
crates/gregg/src/update.rs
crates/greggd/src/update.rs
crates/gregg/Cargo.toml          # only if direct tempfile dependency declaration is needed
crates/greggd/Cargo.toml         # same
Cargo.lock
```

Replace thread-only Cargo timeout with child lifecycle ownership.

Replace synthesized temp directory/path creation with private/exclusive temp primitives.

Eliminate the current pattern where a verified Cargo candidate is copied to a predictable second top-level temp pathname solely to outlive a guard. Keep the owning temp object alive through replacement instead.

Tests should prove:

- temp staging is exclusive;
- Unix directory mode is private where practical;
- cleanup occurs on success/error;
- timeout kills/reaps Cargo child in a deterministic fake-child test;
- candidate remains available until replacement;
- checksum mismatch/candidate mismatch still cannot touch the installed executable.

Do not weaken binary checksum or exact candidate-version checks.

### Step 5: remove blanket updater lint suppression

Primary files:

```text
crates/gregg/src/update.rs
crates/greggd/src/update.rs
```

Remove:

```rust
#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(clippy::nursery)]
```

Run the existing workspace Clippy command with warnings denied.

Fix correctness/readability warnings that are small and relevant. For consciously accepted lint cases, use the narrow lint name and document why it is appropriate.

Do not perform unrelated updater API redesign, documentation churn, or whole-file style rewrites.

### Step 6: correct public installation truth before the first binary release

Primary documentation surfaces:

```text
README.md
crates/gregg/README.md
crates/greggd/README.md
packaging/README.md
RELEASING.md
architecture/scripts-and-packaging.md
.opencode/skills/release-process/SKILL.md
packaging/install-linux.sh       # comments only if they currently claim latest works today
packaging/install-macos.sh       # comments only if needed
CHANGELOG.md                     # short correction note if project convention warrants it
```

Required changes before a binary-bearing release exists:

- do not label `releases/latest/download/install.sh` as a currently working/recommended install path while latest is source-only `v1.0.11`;
- remove pinned `v1.0.11/install.sh` examples;
- keep Cargo installation documented as working now;
- retain the prebuilt installer command as the intended path for releases that actually carry the asset;
- do not claim the first live binary release has been tested until it has actually been published.

After the first binary-bearing release is published, a small docs-only follow-up may promote `latest/download/install.sh` to the primary recommendation. That operational docs flip is not a reason to add release automation.

### Step 7: reconcile planning records without rewriting history

Primary files:

```text
plans/102-update-restart-release-readiness-corrective-pass.md
plans/README.md
plans/098-binary-distribution-install-update-roadmap.md   # append correction/closure note only if needed
plans/101-binary-first-self-update-and-release-integration.md # append correction note only if needed
```

Preserve the historical truth that Plans 099-101 were implemented and ordinary CI was green at the recorded SHAs/runs.

Also record the post-closure truth:

- source review found the Plan 102 defects;
- Plans 098-101 should not be treated as release-ready until Plan 102 closes;
- the first live binary-bearing release remains the final operational proof of installer/update consumption.

Do not alter old commit IDs, old CI run IDs, or claim those historical checks failed; they passed what they tested.

## Required tests

### Updater transaction tests

At minimum, deterministic tests must prove:

```text
prepare failure -> no daemon stop
checksum failure -> no daemon stop
candidate mismatch -> no daemon stop
Cargo timeout/failure -> no daemon stop
SCM stop failure -> no replacement
stopped service -> update leaves stopped
running service + successful replacement -> restart attempted
replacement success + restart failure -> UpdatedButRestartFailed/nonzero
```

Use dependency injection or a small decision helper rather than actually downloading a fake release during ordinary tests.

### Restart decision tests

At minimum:

```text
Stopped + definitely absent -> exactly one spawn
NotRunning + definitely absent -> exactly one spawn
Uncertain -> zero spawns
valid existing Gregg endpoint after control error -> zero spawns
ambiguous endpoint after control error -> zero spawns
spawn + readiness success -> restart success
spawn + readiness timeout/child failure -> restart error
```

### Process-timeout tests

Prove that both the general local-command timeout path and Cargo fallback timeout path kill and reap the child.

A tiny test child that sleeps past a short deadline is adequate. Keep total runtime low.

### Temp staging tests

Prove:

- no reuse of an attacker/preexisting path is possible through ordinary create semantics;
- Unix temp directory permissions are owner-private where the implementation/library guarantees that behavior;
- candidates are cleaned after the owning guard drops;
- candidate remains valid through replacement preparation.

### Existing invariant tests

Do not regress:

- exact crates.io stable-version authority;
- target-to-asset mapping;
- SHA-256 validation;
- exact `gregg X.Y.Z` / `greggd X.Y.Z` candidate identity;
- Cargo fallback only on existing intended cases (asset 404/source-only host), not checksum/transport failure;
- `UpdatedButRestartFailed` partial-success semantics;
- systemd/launchd/SCM/direct startup-state distinctions;
- `croncheck` ambiguity safety;
- Windows SCM lifecycle behavior;
- Rust 1.75 compilation.

## Verification

Use the existing light verification model. Do not add a Plan-102-specific permanent workflow.

Required local checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo check --workspace --all-targets --all-features
./scripts/check-local.sh
```

Because this is release/update-facing work, also run the existing release preflight before closure:

```bash
./scripts/check-local.sh --release
```

### Ubuntu/local lifecycle smoke

On the available Ubuntu environment, demonstrate with release binaries or the normal local release build:

1. a direct daemon is running;
2. `greggd restart` stops it and returns success only after the replacement daemon answers as Gregg;
3. an intentionally ambiguous/non-Gregg listener on the configured endpoint prevents blind spawn;
4. a failed/uncertain stop path does not create a second daemon;
5. `greggd update` on the current version returns `AlreadyCurrent` without altering daemon state.

If systemd is available and privilege is available, also verify one manager restart and authorization/error path. Do not require privileged CI.

### Windows verification

Use the existing Windows CI job; do not add a second Windows matrix.

The Windows job must continue to pass:

```text
workspace tests
release greggd build
existing SCM lifecycle smoke
```

Add deterministic updater ordering tests that compile/run on Windows so CI proves a preparation failure cannot reach the SCM-stop activation phase.

A live newer-version release is not required merely to test the ordering seam.

### macOS verification

Existing macOS Intel/ARM64 jobs should remain green. Manager command logic must at least compile natively on both jobs.

Do not add privileged launchd mutation to CI. If a macOS host is available manually, a read-only `startup instructions` / state probe is sufficient for Plan 102; actual LaunchDaemon installation remains operator-level behavior already covered by Plan 100's contract.

### Release workflow

Do not run/tag a fake production release merely to close Plan 102.

Inspect `.github/workflows/release-binaries.yml` after the changes and ensure its existing five-target/draft-only contract remains unchanged unless a source correction genuinely requires a tiny edit.

The first actual release after `v1.0.11` remains the live proof that:

```text
manual crates publish
-> annotated vX.Y.Z tag
-> release-binaries five-target build
-> draft release with installer/checksum assets
-> maintainer publish
-> fresh install from release asset
-> gregg/greggd update can consume the exact asset on a later release
```

Do not fabricate that evidence in the Plan 102 closure record.

## Acceptance criteria

Plan 102 is complete only when all of the following are true:

- [x] `greggd update` fully downloads/builds and validates its candidate before stopping a running Windows SCM service.
- [x] Any candidate-preparation failure leaves a running Windows daemon running and leaves the installed executable unchanged.
- [x] SCM stop failure is a hard pre-replacement failure; replacement does not continue after an unconfirmed stop.
- [x] A service installed but stopped before update remains stopped afterward.
- [x] `UpdatedButRestartFailed` remains the truthful result when replacement succeeded but activation failed.
- [x] Direct/cron restart never blind-spawns after `StopOutcome::Uncertain` or an ambiguous control/probe result.
- [x] Direct/cron restart returns success only after a bounded valid Gregg health/readiness confirmation, not merely `spawn()` success.
- [x] Direct/cron restart cannot create a second competing daemon against an occupied/ambiguous endpoint in deterministic regression coverage.
- [x] Systemd/launchd manager operations used by startup/restart/update are bounded and preserve stderr for error classification.
- [x] Interactive-auth/permission-style manager failures produce the documented explicit elevated command and do not silently fall back to cron.
- [x] Cargo fallback timeout kills and reaps the Cargo child rather than returning while it continues in the background.
- [x] Both updaters use private/exclusive staging rather than predictable shared-temp top-level candidate names.
- [x] Release checksum and exact candidate-version verification remain mandatory before replacement.
- [x] Both updater modules participate in normal Clippy `-D warnings` without blanket `clippy::all`/`pedantic`/`nursery` suppression.
- [x] Active docs no longer present nonexistent `v1.0.11/install.sh` or current source-only `latest/download/install.sh` assets as working current installation paths.
- [x] Cargo remains documented as the current working fallback/install path until a binary-bearing release is actually published.
- [x] No new permanent CI workflow/job/matrix, package-manager integration, updater daemon, PID scan, or service-manager coupling in `run.rs` is introduced.
- [x] `cargo fmt`, workspace Clippy with warnings denied, workspace tests, workspace check, default local check, and release preflight all pass.
- [x] Ubuntu direct restart safety smoke passes.
- [x] Existing Windows SCM CI smoke remains green together with deterministic updater-ordering tests.
- [x] Existing macOS Intel/ARM64 and MSRV jobs remain green.
- [x] Planning records truthfully mark Plans 098-101 as implemented historically but corrected/release-ready only through Plan 102.
- [x] No live binary-release success is claimed until a real release with assets has actually been published and consumed.

## Explicit non-goals

Do not add:

- apt, yum/dnf, pacman, Homebrew tap, Chocolatey, Scoop, Winget, MSI, pkg, deb, rpm, or other package-manager publishing;
- automated crates.io publication;
- automatic version bumps or tag creation;
- automatic final GitHub Release publication;
- artifact signing, SBOM, provenance/attestation, or key-management infrastructure;
- background update checks, update notifications in the TUI, or an updater service;
- Windows ARM64 or ARMv7 prebuilt support as part of this correction;
- a generic process supervisor abstraction;
- PID files or process-name discovery;
- a new workspace crate solely for updater/process helpers;
- a new permanent CI workflow or evidence bundle;
- broad refactoring of `startup.rs`, updater public APIs, daemon runtime, collectors, protocol, client rendering, scheduler, or configuration;
- changes to release asset naming or the existing five-target matrix unless required to fix a concrete correctness defect discovered while executing this plan.

## Handoff notes

Implementation order should be:

```text
A. candidate preparation/activation ordering
B. direct/cron restart safety
C. bounded manager/Cargo child execution
D. private staging
E. remove blanket Clippy suppression
F. documentation truth + plan reconciliation
G. local checks + Ubuntu smoke + existing CI
```

Do not start by refactoring the two updater modules into a shared crate. Correct the behavioral boundaries first. If, after the corrections, a very small helper can be shared without expanding API surface or dependency count, that is acceptable but not required.

The release workflow itself is not the problem found in review. Preserve its five-target, draft-only design unless execution reveals a directly related defect.

The first binary-bearing release after `v1.0.11` should be cut only after this plan is closed.

## Closure record

Completed at implementation `008092c0ab044f4b6cca4cb8cb5173a6c5b67a45`. The corrective implementation landed in `eb806805c351b380952294f5d7a44c2907202e2c`; `911ca1a34e94c1a0c51f49c3bf219dc4a1a8d677` then scoped the direct-restart decision seam to Unix after the first Windows CI compile check, and `008092c0ab044f4b6cca4cb8cb5173a6c5b67a45` added the Windows-only updater activation-ordering regression test.

Local evidence:

- `./scripts/check-local.sh` passed.
- `./scripts/check-local.sh --release` passed on the final implementation.
- The Ubuntu direct lifecycle smoke passed for restart readiness, ambiguous endpoint refusal, and current-version update behavior.
- Focused updater/startup tests, workspace format, Clippy with warnings denied, workspace tests, and workspace checks passed.

Remote evidence:

- Existing CI run [`33695133206`](https://github.com/eggstack/gregg/actions/runs/33695133206) passed all five jobs for the final implementation: Linux, macOS ARM64, macOS Intel, Windows including SCM lifecycle smoke, and Rust 1.75 MSRV.
- The earlier corrective run `33694063628` exposed only Unix-only direct-restart symbols being compiled as dead code on Windows; that was corrected by `911ca1a`. Run `33694381473` passed before the final Windows-only test addition.

Release truth:

- `v1.0.11` remains source-only. No live binary-release success is claimed; the first binary-bearing release remains the separate operational proof described above.
