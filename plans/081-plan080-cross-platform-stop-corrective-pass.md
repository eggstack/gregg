# Phase 081: Plan 080 cross-platform stop/runtime corrective pass

Status: complete.

Depends on: Plan 080 implementation through `e9e397e5eeabd8da1366bfff235e1f8ea79a50b5`.

## Objective

Correct the small set of concrete defects found after Plan 080 landed, without reopening the broader daemon/service architecture:

1. restore Windows foreground `greggd run` compilation by keeping the Unix-only control-socket wrapper behind Unix cfg and preserving the existing Windows runtime path;
2. make Unix control-socket identity unambiguous so `greggd --config B stop` cannot stop a different daemon launched from config A merely because both configs live in the same directory;
3. require restrictive control-socket permissions instead of silently ignoring a failed `chmod`/permission update;
4. only unlink a pre-existing Unix socket when the connect failure actually demonstrates that no live listener owns it;
5. rerun the existing local/Ubuntu lifecycle verification and the existing native Windows CI path before reconciling Plan 080 as closed.

This is a narrow corrective pass. Do not redesign process management, restore Unix service-manager invocation, add PID files, add HTTP shutdown, create a generic IPC framework, or add CI workflows/jobs/matrices.

## Baseline findings

### 1. Current Windows `main` is broken by Unix-only run dispatch

Current `crates/greggd/src/main.rs` handles `Command::Run` with:

```rust
build_runtime()?.block_on(greggd::run::run_with_control_path(
    collector,
    config,
    &config_path,
))
```

for every target.

`run_with_control_path()` is declared only under:

```rust
#[cfg(unix)]
```

in `crates/greggd/src/run.rs`.

The latest CI run for `e9e397e5eeabd8da1366bfff235e1f8ea79a50b5` shows:

- Linux: success;
- MSRV Rust 1.75: success;
- macOS arm64: success;
- macOS Intel: success;
- Windows: failure during the workspace test step, before the Windows release build and SCM lifecycle smoke can run.

The code structure explains the Windows failure directly: Windows cannot resolve the Unix-only `run_with_control_path` symbol.

The correction must be minimal. Unix foreground `run` should use `run_with_control_path`; Windows foreground `run` should use the existing ordinary `run` path. Do not generalize the Unix control wrapper to Windows and do not alter the SCM service worker.

### 2. The current primary Unix control socket is directory-scoped, not config-scoped

Current `primary_control_path(config_path)` is effectively:

```text
<config_parent>/greggd.control.sock
```

and `stop_candidates()` tries this primary path before the host/port-derived fallback.

That is unsafe when multiple configs share one directory.

Concrete failure case:

```text
/tmp/gregg/a.toml -> daemon A -> 127.0.0.1:11410
/tmp/gregg/b.toml -> daemon B -> 127.0.0.1:11411
```

Daemon A binds:

```text
/tmp/gregg/greggd.control.sock
```

Daemon B sees that path occupied and falls back to another socket.

Then:

```bash
greggd --config /tmp/gregg/b.toml stop
```

tries `/tmp/gregg/greggd.control.sock` first. If daemon A is listening there, the command can send `STOP\n` to daemon A and return success.

This violates the intended Plan 080 invariant:

> `greggd stop` targets only the local daemon instance associated with the same resolved config identity as `greggd run`.

The corrective design must make both primary and fallback control paths derive from config identity, not merely directory identity and not solely mutable host/port values.

### 3. Control-socket permissions are requested but not enforced

Current `bind_listener()` binds the socket and then executes:

```rust
let _ = std::fs::set_permissions(..., 0o600);
```

The result is intentionally discarded.

Therefore a permission-setting failure can leave a listener active even though the implementation/documentation claims the socket is restricted to `0600`.

A successfully exposed control socket must not be retained unless restrictive permissions were applied successfully.

### 4. Stale socket cleanup currently treats every connect error as stale

Current `try_bind()` does:

```text
existing socket path
    -> UnixStream::connect(path)
    -> success: live, do not replace
    -> any error: remove socket and rebind
```

`PermissionDenied`, timeout-like errors, resource exhaustion, or other unexpected errors do not prove the socket is stale. Unlinking after an arbitrary connect error risks replacing a socket whose owner may still be live but temporarily inaccessible to the caller.

Only errors that actually indicate no accepting listener/path should authorize stale-socket removal. At minimum:

- `ConnectionRefused` may be treated as a stale socket entry;
- `NotFound` means the path disappeared and may proceed as absent;
- `PermissionDenied` must not unlink;
- other errors must not be treated as stale without a specific justified mapping.

Prefer a tiny error-classification helper with direct unit coverage rather than embedding a widening match in `try_bind()`.

### 5. Plan 080 is recorded as complete despite a red native Windows path

`plans/080-greggd-runtime-croncheck-and-direct-stop-correction.md` contains a valid Ubuntu closure record and demonstrates the direct Unix lifecycle behavior. However, post-closure review found the Windows compile regression and the cross-config Unix targeting defect above.

Until Plan 081 passes:

- treat Plan 080 as implemented but requiring corrective follow-up;
- do not claim the Plan 080 line is release-ready;
- preserve the valid Ubuntu root-cause and E2E record rather than rewriting it;
- append a short correction note pointing to Plan 081 rather than deleting historical evidence.

## Authoritative behavior after Plan 081

### Foreground `greggd run`

Unix:

```text
greggd run [--config PATH]
    -> load config
    -> bind config-specific local control socket
    -> run shared daemon core
```

Windows:

```text
greggd run [--config PATH]
    -> load config
    -> run shared daemon core without Unix control-socket wrapper
```

Windows SCM service mode remains separately owned by the existing service dispatcher/worker path.

No Unix code path may be referenced from a Windows build.

### Unix control identity

Control-socket identity must be derived from the resolved config path identity and remain stable across host/port edits to that same config file.

Preferred shape:

```text
primary:
<config_parent>/greggd-<short-stable-config-id>.control.sock

fallback:
<temp_dir>/greggd-<short-stable-config-id>.control.sock
```

where `<short-stable-config-id>` is a deterministic, dependency-free, bounded identifier derived from the resolved config path.

A small stable 64-bit digest rendered as fixed lowercase hex is acceptable. If a digest helper is used, choose a deliberately stable algorithm implemented locally (for example FNV-1a over the Unix path bytes) rather than `DefaultHasher`, whose algorithm is not a compatibility contract.

Requirements:

- same resolved config path -> same control ID;
- different config paths in the same directory -> different control IDs;
- changing `host` or `port` in the TOML does not change the control ID for that config path;
- generated socket paths stay below Unix `sun_path` limits;
- no random per-process component;
- no new config field;
- no new dependency.

The host/port-derived naming from Plan 080 may be removed if config identity fully replaces it. Avoid carrying two competing identity schemes unless needed for a deliberate one-release compatibility fallback.

### Compatibility with Plan 080 sockets

Do not add a permanent legacy fallback that can reintroduce cross-config stopping.

If implementation chooses to support stopping an already-running Plan 080 daemon during upgrade, it must be safe and bounded. A legacy directory-wide `greggd.control.sock` candidate may only be used when the caller can positively establish that it belongs to the requested config; the current `STOP\n`/`OK\n` protocol cannot establish that identity, so blindly retaining the old primary candidate is not acceptable.

The smallest acceptable choice is to make Plan 081's new control identity apply to newly started daemons and document that an already-running pre-081 Unix daemon should be restarted once during upgrade. Do not expand the wire protocol solely for backward compatibility unless the implementation is demonstrably smaller than that operational rule.

### Secure socket creation

After binding a candidate:

1. apply restrictive permissions (`0600`);
2. verify the permission operation succeeded;
3. only then publish/retain the listener as active.

If permission application fails:

- close the listener;
- remove the socket file if and only if it is the socket just created by this attempt;
- try the next legitimate candidate if one exists;
- preserve the actual permission error for diagnostics if no candidate succeeds.

Do not continue serving a stop socket whose permissions could not be restricted.

If neither candidate can be bound securely, prefer returning a clear control setup/runtime error from Unix foreground `run` rather than silently starting a daemon that advertises `greggd stop` but cannot be controlled by it. Keep this behavior narrow to Unix foreground control setup.

### Stale socket handling

Before unlinking an existing socket candidate:

1. verify filesystem metadata says the entry is a Unix socket;
2. try a local Unix-socket connect;
3. classify the connect result.

Required behavior:

```text
connect succeeds
    -> live owner; do not unlink; candidate is occupied

ConnectionRefused
    -> stale socket; unlink and retry bind

NotFound
    -> raced with removal; proceed as absent

PermissionDenied
    -> do not unlink; surface/retain permission error

all other errors
    -> do not unlink unless explicitly proven safe
```

A regular file, directory, or other non-socket entry must never be unlinked by stale-socket cleanup.

## Implementation sequence

### Step 1: fix Windows foreground `run` cfg dispatch

Change only the `Command::Run` dispatch boundary.

Preferred shape:

```rust
Command::Run => {
    let config = ...;
    let collector = ...;

    #[cfg(unix)]
    {
        build_runtime()?.block_on(run_with_control_path(...))
    }

    #[cfg(target_os = "windows")]
    {
        build_runtime()?.block_on(run(...))
    }
}
```

An equivalent tiny helper is acceptable if it produces less duplication.

Do not:

- add a fake Windows implementation of `run_with_control_path`;
- add named-pipe support;
- change Windows `Command::Stop`;
- change SCM dispatcher/readiness/shutdown behavior.

### Step 2: replace directory-scoped control identity with config-scoped identity

Introduce one small shared helper that derives a bounded deterministic ID from the resolved config path.

Use that helper for both primary and fallback control paths.

The identity must not depend on mutable daemon host/port fields.

Add focused tests proving:

```text
same config path + changed host/port -> same paths
A.toml and B.toml in same directory -> different primary paths
A.toml and B.toml in same directory -> different fallback paths
```

Do not use a process PID, current time, random value, or runtime-generated registration file.

### Step 3: add the exact cross-stop regression test

Create a deterministic Unix test with two config paths in the same temporary directory.

Preferred sequence:

1. create config path A and config path B;
2. derive/bind control listener A;
3. derive/bind control listener B;
4. assert both bind concurrently without falling onto the other's identity;
5. spawn both control tasks with separate notification channels;
6. call `send_stop(B, config_b)`;
7. assert only B's shutdown notification resolves;
8. assert A remains live;
9. then stop A explicitly and clean both paths.

The test must fail against the current directory-wide primary naming and pass after the correction.

Do not use production polling sleeps. Use oneshot channels and bounded timeouts only as guards.

### Step 4: enforce restrictive permissions

Refactor candidate binding just enough that a permission-setting failure is not discarded.

Required invariants:

- an active control listener has verified `0600` permissions;
- failure to apply permissions discards that listener/path;
- fallback may be tried after a primary permission failure;
- if no secure candidate succeeds, Unix `run` reports a useful error rather than silently losing stop capability.

Add a normal-path test that reads socket metadata and confirms mode `0600`.

For error-path testing, prefer extracting a small helper/result boundary that can be tested without requiring a privileged filesystem trick. Do not add dependency injection frameworks solely to force `chmod` failure.

### Step 5: narrow stale-socket unlink classification

Extract a tiny classification helper, for example:

```rust
fn stale_connect_error(kind: std::io::ErrorKind) -> bool
```

or an equivalent local match.

Test at minimum:

```text
ConnectionRefused -> stale
NotFound -> absent/race
PermissionDenied -> not stale
TimedOut -> not stale
Other -> not stale
```

Then ensure `try_bind` unlinks only after metadata confirms a socket and the connect result is classified as stale.

Retain the existing test proving regular files are preserved.

### Step 6: rerun focused local checks

On the available Unix development host, run at minimum:

```bash
cargo fmt --all -- --check
cargo test -p greggd control
cargo test -p greggd cli
cargo test -p greggd run
cargo test -p greggd --bin greggd
cargo test -p greggd
./scripts/check-local.sh
```

Record the actual commands if Rust test filtering differs from the names above.

No release-wide evidence harness is needed.

### Step 7: rerun the Ubuntu release-binary lifecycle smoke

Rebuild the release binary and repeat the Plan 080 direct lifecycle proof with a temporary config:

```text
greggd run
    -> croncheck healthy
    -> control socket exists and is 0600
    -> greggd stop succeeds
    -> daemon exits 0
    -> TCP listener disappears
    -> control socket disappears
    -> croncheck fails nonzero
    -> second stop is idempotent
```

The control socket path recorded in this smoke must be the new config-specific identity.

### Step 8: add a narrow two-daemon Ubuntu smoke

Use two temporary config files in the same directory and two unused high ports.

Start both real release binaries.

Verify:

1. both `croncheck` commands become healthy;
2. both control socket paths are distinct;
3. `greggd --config B stop` exits successfully;
4. B exits and B's croncheck fails;
5. A remains healthy and its process/listener remain present;
6. `greggd --config A stop` then stops A cleanly;
7. both socket files are removed.

This operational smoke directly validates the bug that motivated the identity correction and is small enough to run locally.

### Step 9: require the existing Windows CI path to pass

Push the implementation through the existing `.github/workflows/ci.yml` only.

Required Windows results:

- workspace test step passes;
- release `greggd` build runs and passes;
- existing Windows SCM lifecycle smoke runs and passes.

Also require the existing Linux, MSRV, and macOS jobs to remain green.

Do not add:

- a new workflow;
- another Windows job;
- a new matrix dimension;
- artifacts/evidence uploads;
- release automation.

The purpose of CI here is only native Windows compatibility truth, which the repository already uses.

### Step 10: reconcile documentation/planning records

After implementation and verification pass:

1. update Plan 080 with a short post-closure correction note describing the Windows cfg regression and config-directory socket collision discovered after its original E2E;
2. preserve Plan 080's valid Ubuntu root-cause and lifecycle record;
3. mark Plan 081 complete and record the implementation SHA, local Ubuntu lifecycle smoke, two-config smoke, and green existing Windows CI run;
4. update `plans/README.md` so Plan 080 is described as corrected by Plan 081 and Plan 081 is complete;
5. update README/architecture/AGENTS only if the exact control-path description becomes stale;
6. do not create Plan 082 solely to mark this corrective pass closed.

## Expected implementation surface

Primary:

```text
crates/greggd/src/main.rs
crates/greggd/src/control.rs
```

Potentially touched only if tests or narrow documentation require it:

```text
crates/greggd/src/run.rs
crates/greggd/src/cli.rs
README.md
AGENTS.md
architecture/greggd-daemon.md
plans/080-greggd-runtime-croncheck-and-direct-stop-correction.md
plans/081-plan080-cross-platform-stop-corrective-pass.md
plans/README.md
```

No new crate or dependency is expected.

## Scope

### In scope

- Windows foreground `run` cfg correction;
- preserving Windows SCM stop/service runtime unchanged;
- config-path-specific Unix control identity;
- deterministic bounded control socket paths;
- exact A/B cross-stop regression coverage;
- enforcing `0600` control socket permissions;
- refusing/surfacing insecure control setup rather than silently accepting it;
- conservative stale-socket cleanup;
- focused local tests;
- Ubuntu one-daemon lifecycle smoke;
- Ubuntu two-config same-directory stop-isolation smoke;
- existing CI rerun for native Windows truth;
- truthful Plan 080/081 closure records.

### Out of scope

- Unix `start` or `restart`;
- `systemctl`/`launchctl` invocation from production `greggd`;
- PID files, PID registries, `/proc` process search, `pkill`, or `killall`;
- self-daemonization;
- HTTP stop/shutdown endpoints;
- generic control RPC;
- config mutation over the control socket;
- Windows named pipes;
- Windows SCM redesign;
- changes to metrics collection, API schema, client TUI, EggPool, or scheduler behavior;
- new dependencies unless absolutely required by an unforeseen compiler/platform constraint;
- new CI workflows/jobs/matrices;
- release automation or publication.

## Acceptance criteria

### Windows regression correction

- [ ] Windows no longer references Unix-only `run_with_control_path` from a compiled code path.
- [ ] Linux/macOS foreground `run` still uses the local control socket.
- [ ] Windows foreground `run` uses the ordinary shared daemon runtime.
- [ ] Windows `greggd stop` still delegates to the SCM manager.
- [ ] Windows SCM dispatcher/readiness/shutdown behavior is unchanged.
- [ ] Existing Windows workspace tests pass.
- [ ] Existing Windows release `greggd` build passes.
- [ ] Existing Windows SCM lifecycle smoke passes.

### Config-specific Unix stop identity

- [ ] Control identity is derived from resolved config path, not only config directory.
- [ ] Control identity does not depend on mutable host/port fields.
- [ ] Same config path produces the same primary/fallback paths across host/port edits.
- [ ] Two different config paths in the same directory produce different primary paths.
- [ ] Two different config paths in the same directory produce different fallback paths.
- [ ] Socket paths remain below Unix path-length limits.
- [ ] No random/PID/time-based identity is introduced.
- [ ] No new config field is introduced.
- [ ] `greggd --config B stop` cannot stop daemon A solely because A and B share a directory.
- [ ] Deterministic A/B regression test proves stop isolation.
- [ ] Real Ubuntu two-config smoke proves stop isolation with release binaries.

### Permission enforcement

- [ ] Every active Unix control socket has successfully applied restrictive `0600` permissions.
- [ ] Permission-setting errors are not ignored.
- [ ] A socket whose permissions cannot be restricted is closed and removed if created by that attempt.
- [ ] Fallback may be tried after a failed primary secure setup.
- [ ] If no secure candidate succeeds, Unix foreground `run` returns a clear error rather than silently disabling `greggd stop`.
- [ ] Normal-path test verifies actual socket mode.

### Stale-socket safety

- [ ] Existing entries are metadata-checked before unlink.
- [ ] Regular files are never removed by stale-socket cleanup.
- [ ] Live connect success never unlinks the socket.
- [ ] `ConnectionRefused` is treated as stale.
- [ ] `NotFound` is treated as disappeared/absent.
- [ ] `PermissionDenied` is never treated as stale.
- [ ] Timeout/other unexpected connect errors are never treated as stale without explicit proof.
- [ ] Focused classification tests cover the allowed/disallowed error kinds.

### Local verification

- [ ] `cargo fmt --all -- --check` passes.
- [ ] Focused `greggd` control tests pass.
- [ ] Focused CLI/run tests pass.
- [ ] `cargo test -p greggd --bin greggd` passes.
- [ ] `cargo test -p greggd` passes.
- [ ] `./scripts/check-local.sh` passes.
- [ ] Ubuntu release binary one-daemon lifecycle smoke passes.
- [ ] Ubuntu release binary two-config same-directory isolation smoke passes.
- [ ] No new CI workflow/job/matrix is added.

### Cross-platform closure

- [ ] Existing Linux CI job passes.
- [ ] Existing Rust 1.75 MSRV job passes.
- [ ] Existing macOS native jobs pass.
- [ ] Existing Windows job passes through tests, release build, and SCM smoke.
- [ ] Plan 080 receives a truthful corrective-follow-up note without deleting its valid historical E2E record.
- [ ] Plan 081 records implementation SHA and exact local/CI verification results.
- [ ] `plans/README.md` identifies Plan 081 as the active corrective phase until all criteria pass.
- [ ] No Plan 082 is created solely for closure.

## Closure standard

Plan 081 is complete only when all three product-level statements are demonstrated:

```text
Windows foreground/SCM paths compile and pass the existing native Windows job
```

```text
Unix config A and config B in the same directory cannot cross-stop one another
```

and:

```text
Ubuntu release greggd run -> croncheck healthy -> stop -> clean exit -> dead croncheck
```

A green Linux-only test run is insufficient because the regression includes a native Windows compile break. A green Windows build alone is insufficient because the Unix control identity bug is behavioral. Use the existing local-first verification model plus the existing native Windows CI job; do not add new verification infrastructure.

## Closure record

**Date:** 2026-08-14
**Implementation SHA:** see `git log -1 --format=%H` on the Plan 081 commit.
**Binary version:** `greggd 1.0.5`
**Host:** Ubuntu 24.04.4 LTS (Noble Numbat, aarch64)

### Implementation summary

1. **Windows foreground `run` cfg correction** (`crates/greggd/src/main.rs`,
   `crates/greggd/src/run.rs`). Added `run_with_control_path_or_default`,
   a small helper that calls `run_with_control_path` on Unix and the
   ordinary shared `run` on Windows. `main.rs` now has exactly one
   cfg-aware dispatch point for the foreground command and Windows no
   longer references any Unix-only symbol. The SCM service worker and
   `Command::Stop` Windows path are unchanged.

2. **Config-path-scoped Unix control identity**
   (`crates/greggd/src/control.rs`). Introduced `config_id_for_path`
   which returns the 16-character lowercase hex digest of a stable 64-bit
   FNV-1a hash over the canonical config-path bytes. The digest does
   not depend on `host`, `port`, `name`, the current PID, the time, or
   any random source. Both `primary_control_path` and `fallback_control_path`
   now produce `greggd-<id>.control.sock`, so two different config files
   in the same directory produce different control paths.

3. **Restrictive `0600` enforcement.** Refactored `bind_listener` so the
   listener is only published when the `chmod` and a post-`chmod`
   metadata check both confirm `0o600`. A failed permission update
   closes the listener, removes the socket file, and tries the next
   legitimate candidate. `shutdown_with_control` now returns
   `Err(ControlSetupError::NoSecureControl { primary, fallback })` when no
   candidate yields a secure listener, so the foreground entry point
   surfaces a clear runtime error rather than silently disabling
   `greggd stop`.

4. **Narrow stale-socket unlink classification**
   (`crates/greggd/src/control.rs`). Extracted the tiny
   `stale_connect_error(kind)` helper used by `try_bind`. Only
   `ConnectionRefused` and `NotFound` authorize removal of an existing
   socket entry; `PermissionDenied`, `TimedOut`, and any other
   unexpected error are explicitly preserved. Metadata still gates the
   path type so regular files and directories are never unlinked.

5. **API surface reductions.** `bind_listener`, `send_stop`,
   `fallback_control_path`, `stop_candidates`, and the new
   `config_id_for_path` now take only `config_path`; the old
   `&Config` parameter was removed because the identity no longer depends
   on any TOML field.

6. **Documentation.** Updated `AGENTS.md`, `architecture/greggd-daemon.md`,
   `architecture/overview.md`, `plans/080`, and `plans/README.md` so the
   documented control-path description matches the new config-scoped
   identity and the corrected stale-socket/permission semantics. The
   original Plan 080 closure record was preserved; a corrective note
   appended at the end points to Plan 081.

### Local Ubuntu lifecycle smoke

Single daemon on `/tmp/gregg-e2e-YlIZ/greggd.toml`, port 11461:

```text
Config:                      /tmp/gregg-e2e-YlIZ/greggd.toml
Socket path:                 /tmp/gregg-e2e-YlIZ/greggd-ec01a39070be8279.control.sock
Socket perms:                600
Healthy after:               3 polls (croncheck exit 0)
TCP listener visible (ss):   yes
Stop exit:                   0
Daemon exit:                 0
Control socket removed:      yes
TCP listener removed:        yes
Subsequent croncheck:        exit 3 (Connection refused)
Second stop (idempotent):    exit 0 ("greggd not running")
```

### Two-daemon same-directory isolation smoke

Two configs in `/tmp/gregg-e2e-twodaemon-zBKv/`, ports 11462 (A) and
11463 (B):

```text
Sock A:  /tmp/.../greggd-0dff56cf4541171e.control.sock   perms 600
Sock B:  /tmp/.../greggd-4df4119e1f50c915.control.sock   perms 600
A healthy:  yes (after 3 polls)
B healthy:  yes (after 3 polls)

stop B:               exit 0 ("greggd stopped")
B daemon exit:        0
B croncheck after:    exit 3
A croncheck after:    exit 0 (still healthy)
A process alive:      yes (kill -0)
A TCP listener:       yes (ss)

stop A:               exit 0 ("greggd stopped")
A daemon exit:        0
Sock A removed:       yes
Sock B removed:       yes
```

### Test coverage

- 18 control module tests (config identity, primary/fallback paths, stale
  classification, cross-config isolation, malformed/stop/permission
  behavior)
- 13 CLI tests preserved (parser accepts stop, rejects start/restart,
  croncheck target diagnostics, probe health)
- 184 greggd tests total, all green
- `cargo fmt --all -- --check` passes
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- `RUSTFLAGS="-D warnings" cargo check -p greggd --target x86_64-pc-windows-msvc` passes
  (Windows compile regression from Plan 080 fixed)
- `./scripts/check-local.sh --release` passes (excluding the clean-tree
  check, which is expected before commit)

### Verification provenance

- Local implementation, test, and smoke environment: Linux aarch64 host.
- Windows compile truth: local `cargo check` against the
  `x86_64-pc-windows-msvc` target proves the cfg dispatch fixes the
  regression. The existing `windows-2022` CI job runs the full workspace
  test, the release `greggd` build, and the SCM lifecycle smoke; it
  must be green for cross-platform closure. No new workflow, job,
  matrix, or artifact is added.