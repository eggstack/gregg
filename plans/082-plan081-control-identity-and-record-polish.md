# Phase 082: Plan 081 control-identity and planning-record polish

Status: ready for implementation.

Depends on: Plan 081 implementation `59e17551c211df382c6f0219d0d465ef1c198a8a` and green follow-up record `6fb005b4a469cdd1ea4baf498fe4a18f5858f3be`.

## Objective

Perform one final narrow polish pass after Plan 081 without reopening the daemon/service architecture.

This phase has two purposes only:

1. make Unix control-socket identity stable when the same existing explicit `--config` file is spelled differently (for example relative path, absolute path, or symlink path), while preserving the config-path-scoped A/B isolation added by Plan 081;
2. reconcile the remaining Plan 080/081 planning-record inconsistencies so the repository describes the already-green implementation truthfully and with exact verification provenance.

This is not a new runtime feature phase. Do not change the `STOP\n`/`OK\n` protocol, Windows SCM behavior, Unix lifecycle commands, HTTP API, service-manager separation, CI topology, dependencies, or release process.

## Baseline findings

### 1. Plan 081 is functionally complete and current `main` is green

Plan 081 corrected the substantive defects from Plan 080:

- Windows foreground `greggd run` no longer references a Unix-only symbol;
- Unix control sockets are config-path-scoped instead of directory-scoped;
- two configs in the same directory cannot cross-stop one another;
- active control sockets require verified `0600` permissions;
- stale-socket removal is limited to safe connect-error classifications;
- the Ubuntu single-daemon lifecycle smoke passed;
- the Ubuntu two-daemon same-directory isolation smoke passed;
- existing Linux, macOS, Rust 1.75, and Windows CI jobs passed;
- the Windows job passed workspace tests, release `greggd` build, and SCM lifecycle smoke.

The authoritative implementation CI run is:

```text
run 31813136597
head 59e17551c211df382c6f0219d0d465ef1c198a8a
conclusion success
```

Current `main` subsequently also passed CI at `6fb005b4a469cdd1ea4baf498fe4a18f5858f3be` in run `31813615708`.

Plan 082 must preserve this behavior. It is not justified to add more platform infrastructure or repeat broad architecture work.

### 2. Current control identity hashes path spelling, not filesystem identity

`crates/greggd/src/control.rs` currently derives the control ID from `canonical_path_bytes(config_path)`, but that helper does not perform filesystem canonicalization. It returns the `OsStr` bytes exactly as supplied.

Therefore these two invocations can derive different control IDs even when they refer to the same existing config file:

```text
greggd --config ./greggd.toml run
greggd --config /absolute/path/greggd.toml stop
```

Likewise, a symlink path and its target path can produce different IDs.

The A/B fix from Plan 081 is still correct: two different config files in the same directory derive different IDs. The remaining issue is only that multiple spellings of the same existing file do not converge.

The correction must remain config-identity-based and deterministic. Do not return to directory-only naming, host/port naming, PID naming, random naming, or a registration database.

### 3. Missing implicit default config must continue to work

`greggd` intentionally allows the platform-default config path to be absent and then runs from `Config::default()` when the config was not explicitly supplied.

Therefore a blanket `std::fs::canonicalize(config_path)?` at CLI resolution would be wrong: canonicalization requires the path to exist and would turn the supported missing-default case into an error.

The identity normalization must distinguish these cases safely:

- existing config path: normalize to the actual filesystem path before hashing;
- absent implicit default path: retain a deterministic absolute/resolved path identity without requiring a file to exist;
- absent explicit config path: preserve the existing configuration error behavior before daemon/control startup.

Do not mutate the user's configured path or change config read/write semantics solely to derive the socket identity.

### 4. Planning records are slightly inconsistent after successful closure

Current `plans/README.md` correctly says Plan 081 closed the product defects, but its Plan 080 table row still says `corrective follow-up 081 active` even though Plan 081 is complete and native CI is green.

Plan 081 also records its CI success using prose that says to inspect `gh run list --limit 1` rather than recording the exact run ID. That instruction is already stale because a later green documentation commit produced another run.

Finally, Plan 081 is marked `Status: complete` while its acceptance checklist remains unchecked. The closure record contains the required evidence; the checklist should be reconciled with that evidence instead of leaving the document internally contradictory.

Do not rewrite valid Plan 080 historical evidence. Preserve its original Ubuntu lifecycle record and Plan 081 corrective note.

## Authoritative behavior after Plan 082

### Control identity

For an existing config file, all ordinary path spellings that resolve to the same filesystem file must derive the same control ID.

Examples that must converge when they refer to the same file:

```text
./greggd.toml
/home/user/greggd.toml
/home/user/config-link.toml -> /home/user/greggd.toml
```

Two different config files must still derive different IDs, including when they share a directory.

Changing TOML contents such as `host`, `port`, or `name` must not change the control ID.

### Missing implicit default config

When no `--config` was supplied and the default config file does not exist:

- `greggd run` must retain existing default-config behavior;
- control identity must still be deterministic;
- no filesystem canonicalization error may be introduced merely because the default TOML is absent.

### Planning records

After closure:

- Plan 080 is described as implemented and corrected by completed Plan 081, not as having an active follow-up;
- Plan 081 contains exact CI run `31813136597` for the implementation commit and may mention `31813615708` as the later green current-main confirmation;
- Plan 081 acceptance checkboxes are reconciled against the closure evidence rather than left all unchecked;
- Plan 082 records only the small path-identity polish and documentation cleanup;
- no Plan 083 is created solely to mark closure.

## Implementation sequence

### Step 1: add one narrow control-identity normalization helper

Keep the normalization local to `crates/greggd/src/control.rs` unless a smaller existing path utility is already appropriate.

Preferred behavior:

```text
if config_path exists:
    use std::fs::canonicalize(config_path) for control identity
else:
    derive a deterministic absolute path from the already-resolved path
    without requiring filesystem existence
```

The absent-path fallback should be lexical and bounded rather than introducing a new crate. If the resolved path is already absolute, using it directly is acceptable. If a relative path reaches this helper, anchor it against `std::env::current_dir()` and normalize only ordinary `.` / `..` components needed to make equivalent lexical spellings converge.

Do not use `DefaultHasher`. Keep the existing stable FNV-1a digest if it remains adequate after normalization.

Do not call `canonicalize()` in a way that changes config loading/writing semantics. This helper exists only to determine the local control-socket identity.

### Step 2: rename misleading helper/commentary

If the implementation continues to expose a helper named `canonical_path_bytes`, its name and documentation must match what it really does.

Preferred options:

- actually canonicalize before producing bytes and rename the byte helper to something precise such as `control_identity_path_bytes`; or
- split normalization and hashing into clearly named helpers.

Avoid terminology that claims filesystem canonicalization when only raw path bytes are used.

### Step 3: add focused identity tests

Add deterministic Unix tests proving at minimum:

1. same existing file via relative and absolute path -> same `config_id_for_path`;
2. same existing file via symlink and target path -> same ID where symlink creation is supported;
3. two different files in the same directory -> different IDs;
4. same config path before/after host/port content edits -> same ID;
5. absent implicit/default-style absolute path -> deterministic ID without requiring file creation;
6. resulting primary/fallback socket paths remain below `UNIX_PATH_MAX`.

Retain the existing Plan 081 A/B cross-config isolation tests.

Do not add sleeps or integration harness infrastructure.

### Step 4: run a small explicit-path lifecycle smoke on Ubuntu

Use one temporary existing config file and the real release binary.

Start with one spelling and stop with another spelling of the same file. For example:

```text
cd <temp-parent>
./target/release/greggd --config ./config/greggd.toml run

then from another working directory:
./target/release/greggd --config /absolute/.../config/greggd.toml stop
```

Required result:

- daemon becomes healthy through `croncheck`;
- relative and absolute spellings derive/reach the same control socket;
- `stop` succeeds;
- daemon exits cleanly;
- socket and TCP listener disappear;
- post-stop `croncheck` fails nonzero.

If practical on the host, repeat the stop using a symlink spelling; otherwise the deterministic symlink unit test is sufficient.

This is a narrow regression smoke, not a new evidence framework.

### Step 5: preserve the existing Plan 081 A/B isolation proof

Run the focused control tests containing the same-directory A/B isolation regression.

A full second two-daemon manual smoke is not required unless the identity-normalization code materially touches the A/B logic and the implementer judges the focused test insufficient. The previously recorded Plan 081 release-binary A/B smoke remains valid historical evidence.

Do not automatically repeat expensive verification that does not add new information.

### Step 6: run the existing local verification

Run at minimum:

```bash
cargo fmt --all -- --check
cargo test -p greggd control
cargo test -p greggd --bin greggd
cargo test -p greggd
./scripts/check-local.sh
```

If the repository's test filters differ, run the nearest equivalent and record the actual commands.

Because this change is Unix-only control-path normalization plus documentation, do not require a new CI workflow or additional matrix. The existing CI may run naturally on push; if it does, it must remain green, but Plan 082 does not add new CI obligations.

### Step 7: reconcile Plan 080/081 records

Update `plans/README.md`:

- change Plan 080 status wording from `corrective follow-up 081 active` to wording that clearly states it was corrected/closed by completed Plan 081;
- keep Plan 081 complete;
- record exact Plan 081 native CI run `31813136597` and implementation SHA `59e17551c211df382c6f0219d0d465ef1c198a8a`;
- optionally note current-main green run `31813615708` without making repeated green runs a future requirement;
- add Plan 082 as the active small polish phase until its acceptance criteria pass.

Update Plan 081:

- replace the ambiguous `gh run list --limit 1` provenance with exact run IDs;
- reconcile acceptance checkboxes only where the existing closure record and CI results directly demonstrate the criterion;
- do not invent evidence for anything that was not actually demonstrated;
- preserve the closure narrative and historical implementation SHA.

Plan 080:

- do not rewrite the original closure record;
- only adjust the post-closure correction/status wording if needed for consistency with completed Plan 081/082.

### Step 8: close Plan 082 without creating another closure-only phase

After implementation and verification:

1. mark Plan 082 complete;
2. record the implementation SHA;
3. record the focused tests and explicit relative/absolute lifecycle smoke;
4. record any naturally occurring existing CI run if one ran;
5. update `plans/README.md` so Plans 080-082 read consistently;
6. do not create Plan 083 solely for closure.

## Expected implementation surface

Primary:

```text
crates/greggd/src/control.rs
```

Planning/documentation:

```text
plans/080-greggd-runtime-croncheck-and-direct-stop-correction.md   # only if status wording needs reconciliation
plans/081-plan080-cross-platform-stop-corrective-pass.md
plans/082-plan081-control-identity-and-record-polish.md
plans/README.md
```

Potentially touched only if the implementation exposes the identity semantics there:

```text
AGENTS.md
architecture/greggd-daemon.md
README.md
```

No new crate or dependency is expected.

## Scope

### In scope

- normalize existing config paths for control identity;
- relative/absolute spelling convergence for the same existing file;
- symlink/target convergence where filesystem canonicalization supports it;
- deterministic absent-default-path identity without requiring file existence;
- precise helper naming/documentation;
- focused identity tests;
- one narrow Ubuntu explicit-path lifecycle smoke;
- preservation of Plan 081 A/B isolation;
- exact Plan 081 CI provenance;
- Plan 080/081/README status reconciliation;
- checkbox reconciliation based only on existing evidence.

### Out of scope

- changing daemon HTTP behavior;
- changing `croncheck` semantics;
- changing `STOP\n`/`OK\n` protocol;
- adding Unix `start`/`restart`;
- systemd/launchd invocation from `greggd`;
- PID files or process discovery;
- generic IPC/RPC;
- Windows named pipes;
- Windows SCM changes;
- host/port-derived control identity;
- random/PID/time-derived identity;
- new dependencies;
- new CI workflows, jobs, matrices, artifacts, or release gates;
- repeated evidence bundles;
- release automation/publication;
- unrelated refactoring.

## Acceptance criteria

### Control identity normalization

- [ ] Existing config file resolves to the same control ID through relative and absolute path spellings.
- [ ] Existing config file resolves to the same control ID through symlink and target spellings where supported.
- [ ] Two different config files in the same directory still produce different control IDs.
- [ ] Editing host/port/name inside the same config file does not change the control ID.
- [ ] No random, PID, time, or mutable TOML field participates in identity.
- [ ] Existing stable digest algorithm remains dependency-free unless a compelling implementation reason is documented.
- [ ] Primary and fallback socket paths remain below `UNIX_PATH_MAX`.
- [ ] The helper/documentation no longer calls raw path bytes "canonical" unless filesystem canonicalization actually occurs.

### Missing/default config behavior

- [ ] Missing implicit default config remains supported exactly as before.
- [ ] Control identity for an absent default-style path is deterministic without creating the file.
- [ ] Missing explicit config continues to fail through the existing configuration error path.
- [ ] No config read/write path semantics are changed solely for socket identity.

### Runtime verification

- [ ] Focused control identity tests pass.
- [ ] Existing Plan 081 A/B cross-config isolation tests remain green.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo test -p greggd --bin greggd` passes.
- [ ] `cargo test -p greggd` passes.
- [ ] `./scripts/check-local.sh` passes.
- [ ] Ubuntu release-binary smoke proves run with one path spelling can be stopped with another spelling of the same config.
- [ ] No new CI workflow/job/matrix is added.
- [ ] Any naturally triggered existing CI run remains green before closure.

### Planning-record polish

- [ ] `plans/README.md` no longer says Plan 081 is active after Plan 081 is complete.
- [ ] Plan 080 row is described as corrected/closed by Plan 081.
- [ ] Plan 081 records implementation SHA `59e17551c211df382c6f0219d0d465ef1c198a8a` explicitly.
- [ ] Plan 081 records exact native implementation CI run `31813136597` explicitly.
- [ ] Later green current-main run `31813615708` may be recorded as confirmation but is not turned into a recurring evidence requirement.
- [ ] Plan 081 acceptance checkboxes are reconciled only where evidence exists.
- [ ] Valid Plan 080 historical Ubuntu evidence is preserved.
- [ ] Plan 082 is registered as active until its criteria pass, then marked complete.
- [ ] No Plan 083 is created solely for closure.

## Closure standard

Plan 082 is complete when both statements are demonstrated:

```text
same existing config file, different ordinary path spellings -> same Unix control identity -> stop succeeds
```

and:

```text
Plans 080-082 and plans/README.md describe the already-green implementation with exact, non-ambiguous provenance
```

This phase should remain small. If implementation begins to require a new IPC mechanism, persistent registry, new dependency, service-manager integration, or CI expansion, stop and reduce the design rather than broadening scope.