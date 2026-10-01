# Plan 150: Windows foreground smoke reliability corrective pass

Status: complete at implementation `2ceafcd3d4a7223b29c3f29051ab91c61e82553e`; CI run `36919813734` green across all six jobs.

Depends on: current post-Plan-149 main state. Independent of the remaining Plan 091 soak record and Plans 147-149 semantics.

## Objective

Correct the Windows-only foreground daemon smoke harness so a transient CI failure is both less likely and immediately diagnosable, without changing `greggd` runtime behavior, Windows telemetry semantics, service lifecycle behavior, HTTP protocol behavior, or the existing thirty-second readiness contract.

The triggering evidence is CI run `36915516145` on documentation-only commit `5e4d83908ab7c9d3f18cfb8110105817306c79a6`. The Windows workspace test failed only in:

~~~text
crates/greggd/tests/windows_smoke.rs::foreground_daemon_serves_v2_status
daemon did not become ready within 30s
~~~

The immediately preceding implementation run `36908815046` was green across all six jobs using identical product code. The diff between implementation SHA `8de5c123193c149112cbe94530ffd609bfe092b1` and failed closure SHA `5e4d83908ab7c9d3f18cfb8110105817306c79a6` changes only Plan 149 planning documentation. This is therefore a test reliability/diagnostic defect unless stronger evidence demonstrates an actual Windows daemon regression.

## Current harness defects

The current `crates/greggd/tests/windows_smoke.rs` has several concrete weaknesses.

### 1. The port is deterministic, not availability-selected

`unique_port("foreground_daemon_serves_v2_status")` hashes a constant test name into the same dynamic-range port on every Windows run.

That reduces collision probability relative to a conventional fixed low port but does not prove the selected port is free on a particular hosted runner. A transient occupant can cause the daemon bind to fail while the test continues polling for readiness.

### 2. Child stdout/stderr are piped but never consumed

The spawned daemon uses:

~~~rust
.stdout(Stdio::piped())
.stderr(Stdio::piped())
~~~

but the test does not read either stream.

This has two problems:

- startup/bind/collector errors are unavailable in the failure message;
- a sufficiently chatty child can eventually block on a full pipe buffer.

The latter is not claimed as the cause of run `36915516145`, but the harness should not create that backpressure risk.

### 3. Readiness polling ignores child exit

The readiness loop probes `/v2/healthz` for thirty seconds but never calls `child.try_wait()`.

If `greggd.exe` exits immediately because of bind failure, config failure, startup failure, or another runtime error, CI still waits the full timeout and reports only:

~~~text
daemon did not become ready within 30s
~~~

The actual exit status and daemon diagnostics are discarded.

### 4. Failure paths do not guarantee child cleanup

The test kills and waits for the child only after every readiness/status assertion has succeeded.

If readiness, HTTP parsing, schema validation, or any later assertion panics first, dropping `std::process::Child` does not terminate the Windows process. A failed smoke can therefore leave a live `greggd.exe` until the outer runner cleanup intervenes.

That can contaminate later work in the same job and makes port-related failures harder to reason about.

### 5. Integration tests launch nested Cargo builds

`ensure_binary()` invokes `cargo build -p greggd` from inside each integration test before locating `../../target/debug/greggd.exe`.

Cargo already builds the package binary for integration tests and exposes its exact path through `CARGO_BIN_EXE_greggd`. The nested build:

- duplicates work;
- creates target-directory lock/contention opportunities when the two smoke tests execute concurrently;
- hardcodes the default target-directory layout;
- adds another process/build boundary unrelated to the behavior being tested.

This is test-harness debt, not a product requirement.

## Scope decisions

This plan owns only `crates/greggd/tests/windows_smoke.rs` and the minimum planning record needed to close the observed native-Windows CI defect.

Preserve:

- the existing two Windows smoke intents: `greggd --help` works, and foreground `greggd run` reaches valid v2 status;
- the exact configured identity/capability assertions already present;
- native Windows execution in the existing Windows CI job;
- the existing thirty-second overall readiness deadline unless implementation evidence proves that a healthy daemon legitimately needs longer;
- the current 200-ms polling cadence unless a smaller mechanical refactor makes the exact sleep spelling unnecessary;
- the existing raw HTTP helper behavior unless a correctness defect is exposed while making diagnostics deterministic;
- current product code unchanged unless improved diagnostics reveal a reproducible product defect.

Do not convert this into a general integration-test framework or Windows daemon redesign.

## Implementation

### A. Use Cargo's integration-test binary path

Replace the hand-built target path and nested `cargo build` helper with Cargo's integration-test binary environment:

~~~rust
fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_greggd"))
}
~~~

or an equivalent compile-time retrieval that is valid on the MSRV.

Remove `ensure_binary()` and its nested `cargo build -p greggd` subprocess.

The `windows_daemon_binary_compiles_and_runs` test should continue to invoke the already-built binary with `--help`; its behavioral assertion remains useful even though compilation itself is guaranteed by Cargo before the integration test starts.

Do not add a dependency or external script to locate the binary.

### B. Select a currently free loopback port through the OS

Replace deterministic hash-based `unique_port()` with a standard-library helper that binds:

~~~text
127.0.0.1:0
~~~

reads the OS-selected local port, then releases the temporary listener immediately before spawning `greggd`.

Use `std::net::TcpListener`; add no random-number or port-allocation dependency.

Document the unavoidable small bind-after-release race. Do not attempt to solve that race by adding a complex broker, privileged reservation mechanism, or production socket-inheritance feature.

If implementation discovers an address-in-use child exit despite OS allocation, the new child-exit diagnostics should expose that directly. A bounded retry may be considered only for a demonstrated bind race, not added speculatively.

### C. Capture daemon output without pipe backpressure

Create stdout/stderr capture files inside the smoke test's temporary directory and pass cloned file handles to the child process through `Stdio::from`.

This gives the child ordinary file-backed output instead of unread pipes.

On failure, after the child is stopped/reaped as appropriate, read a bounded amount of both files and include it in the panic/error text together with:

- child exit status if known;
- selected port;
- config path;
- last readiness probe result;
- elapsed time.

Bound diagnostic output to a reasonable size so a pathological child cannot flood CI logs. Keep the most useful tail when truncation is necessary.

On success, no verbose daemon log dump is required.

Do not introduce asynchronous log-reader threads unless file-backed capture proves insufficient.

### D. Detect child exit during readiness polling

During every readiness iteration:

1. call `child.try_wait()`;
2. if the child has exited, stop polling immediately;
3. collect its exit status and captured stdout/stderr;
4. fail with an explicit startup-exit diagnostic rather than waiting thirty seconds.

If the child is still running, probe `/v2/healthz` as today.

Track the last probe outcome in bounded form:

- connection error; or
- HTTP status plus a bounded body excerpt.

On timeout, report that last outcome so CI distinguishes:

- never-listening daemon;
- live daemon returning warming/failure;
- malformed/unexpected HTTP response;
- ordinary ready success.

Do not weaken the requirement that v2 health reach HTTP 200 and ready state.

### E. Guarantee child/process cleanup on every exit path

Introduce a small test-local RAII owner or equivalent structured helper whose `Drop` path:

- checks whether the child is still running;
- attempts `kill()`;
- waits/reaps the process;
- never panics during cleanup.

Ensure the success path also performs explicit orderly test teardown before assertions finish.

Temporary test files/directories should be removed best-effort after process teardown. If preserving the directory on failure materially improves diagnostics, either print its path or read the diagnostic files before removal; do not rely on runner persistence.

The key invariant is that a failed assertion cannot leave `greggd.exe` running.

No production process-management helper should be added solely for this test.

### F. Keep the timeout evidence-driven

Retain:

~~~rust
Duration::from_secs(30)
~~~

for the readiness budget in the first implementation.

The preceding green Windows run proves that the current daemon can satisfy this budget, while the failed run provides no evidence that it remained healthy for more than thirty seconds because child state/output were not inspected.

Only raise the deadline if the corrected harness demonstrates:

- the child remains alive;
- no startup error is reported;
- readiness genuinely arrives after thirty seconds on ordinary hosted Windows.

A longer timeout must not be used to hide immediate child exit, bind failure, or collector failure.

### G. Keep assertions semantic and bounded

Continue validating:

- v2 health becomes ready;
- `/v2/status` returns 200;
- schema version is 2;
- Windows capability flags remain correct;
- configured name/hostname remain valid;
- CPU/memory/commit metrics are present;
- unsupported Windows load/swap remain null.

If touching the health check, prefer parsing the existing JSON state exactly rather than making substring matching broader. Do not relax current semantics merely to reduce flakes.

## Verification

Run formatting and non-Windows repository checks normally:

~~~text
cargo fmt --all -- --check
./scripts/check-local.sh
~~~

Native Windows is the authoritative environment for this plan.

On Windows, run at minimum:

~~~text
cargo test -p greggd --test windows_smoke
cargo test --workspace --all-targets --all-features
~~~

The focused smoke must run under the default Rust test-harness concurrency; do not serialize it merely to avoid nested-build or port contention that this plan is intended to remove.

One ordinary existing CI workflow must then be green across:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows workspace tests;
- Windows release builds;
- Windows SCM lifecycle smoke;
- MSRV Rust 1.89;
- FreeBSD 14.2 native `gregg-host`.

One green implementation run is sufficient closure evidence. Do not add repeated-run CI, a new workflow, a Windows matrix, or a flaky-test retry wrapper.

If the corrected smoke fails again, its diagnostics must identify whether the child exited, remained alive but not ready, could not bind, or returned non-ready health. A reproducible product defect discovered through those diagnostics should be corrected under a separately scoped implementation/corrective plan rather than hidden in this test-harness pass.

## Acceptance criteria

- [x] `windows_smoke.rs` uses Cargo's `CARGO_BIN_EXE_greggd` integration-test binary path or an MSRV-compatible equivalent.
- [x] The smoke no longer executes nested `cargo build -p greggd` subprocesses.
- [x] The foreground smoke selects a loopback port from an OS `127.0.0.1:0` allocation rather than a deterministic test-name hash.
- [x] No new random/port-allocation dependency is introduced.
- [x] Child stdout/stderr are not left as unread pipes.
- [x] Failure diagnostics include bounded daemon stdout/stderr, selected port/config context, and the last readiness probe outcome.
- [x] The readiness loop checks `child.try_wait()` and fails immediately with exit status when the daemon exits.
- [x] A child that remains alive but fails to become ready still fails at the thirty-second outer readiness deadline.
- [x] Every panic/error/success path guarantees child termination and reaping.
- [x] A failed smoke cannot leave a live `greggd.exe` for later tests in the Windows job.
- [x] The existing v2 status, Windows capability, identity, and metric assertions remain at least as strong.
- [x] No `greggd` production runtime, collector, service-manager, protocol, API, or configuration behavior changes unless the corrected diagnostics expose a separately demonstrated product defect.
- [x] No CI retry wrapper, `continue-on-error`, test skip, or Windows-job weakening is introduced.
- [x] Focused native Windows smoke passes with normal test-harness concurrency.
- [x] The full existing Windows workspace test/build/SCM path passes.
- [x] One ordinary six-job CI run is green.
- [x] Closure records the exact implementation SHA and exact CI run ID.

## Explicit non-goals

Do not include:

- increasing the readiness timeout without evidence;
- weakening or skipping `foreground_daemon_serves_v2_status`;
- adding automatic test retries;
- changing Windows collector formulas or capability semantics;
- changing daemon readiness publication;
- changing EggServe runtime limits or HTTP behavior;
- changing Windows SCM service behavior;
- adding named pipes, IPC redesign, or service-dispatch changes;
- changing production port-validation semantics;
- adding a general-purpose test-process framework;
- adding tempfile/random/portpicker dependencies solely for this smoke;
- adding a new Windows workflow/job/matrix;
- changing FreeBSD CI or reopening Plan 149;
- modifying Plans 074/075/149 historical evidence;
- Plan 091 soak work;
- Plan 147 EggFetch dependency adoption;
- unrelated lint, dependency, installer, updater, TUI, or release work.

## Handoff note

Start in `crates/greggd/tests/windows_smoke.rs`.

The first goal is not to guess why run `36915516145` timed out. Make the smoke incapable of hiding the reason: remove nested Cargo builds and deterministic-port selection, make child lifetime explicit, and surface startup output/exit state immediately.

If the corrected harness then exposes a real daemon readiness defect, preserve that evidence and open the smallest product corrective rather than masking it with a longer timeout or retry.

## Closure record

Implemented in `2ceafcd3d4a7223b29c3f29051ab91c61e82553e` (single commit;
`git show --stat` shows `crates/greggd/tests/windows_smoke.rs` only, +357/-120).
No production source, workflow, script, manifest, or documentation file changed.

What landed, mapped to the implementation sections above:

- **A.** `binary_path()` is now `env!("CARGO_BIN_EXE_greggd")`. `ensure_binary()`
  and its nested `cargo build -p greggd` subprocess are gone; the Windows job's
  test step now contains no nested `Compiling greggd` activity. The
  `--help` behavioral assertion is retained unchanged.
- **B.** `unique_port()` (and its `cast_possible_truncation` allowance) is
  replaced by `free_loopback_port()`, which binds `127.0.0.1:0`, reads the
  OS-selected port, and drops the listener immediately before spawning. The
  residual bind-after-release race is documented in the function's doc comment
  and is deliberately not engineered away. No dependency was added.
- **C.** The child gets `Stdio::from(File)` handles writing to
  `daemon-stdout.log` / `daemon-stderr.log` in the test's temp directory, so
  there are no unread pipes. `capture_tail()` bounds each stream to the last
  8 KiB (tail-kept, truncation marked), and reports `<empty>` /
  `<capture file unavailable>` rather than guessing. No reader threads exist.
- **D.** `await_ready()` calls `poll_exit()` (`Child::try_wait`) on every
  iteration and returns immediately when the daemon has exited. `last_probe`
  records either the connection error or `HTTP <status> state=<state> body=…`,
  bounded to 200 characters, so a timeout distinguishes never-listening,
  live-but-warming/failed, and non-v2-body responses. Readiness now requires
  HTTP 200 plus an exactly parsed `"state": "ready"` instead of a
  `body.contains("\"ready\"")` substring.
- **E.** `DaemonProcess` owns the child; `stop()` kills and reaps it and is
  called from both `Drop` and the explicit success path, so a failed assertion
  cannot leave a live `greggd.exe`. The temp directory is removed best-effort on
  success and retained on failure, where its path is part of the report. The
  report itself carries the reason, port, config path, elapsed time, child
  state, last probe, and both bounded output tails.
- **F.** `READY_TIMEOUT` is still `Duration::from_secs(30)` and
  `READY_POLL_INTERVAL` is still 200 ms. Nothing was raised to mask a failure.
- **G.** The `/v2/status` 200, `schema_version == 2`, Windows capability,
  identity, metric-present, and null-`load`/`swap` assertions are preserved and
  expressed as `Result` checks so a failure reports through the same
  diagnostic path. Capability checks now use `as_bool() == Some(false/true)`,
  which additionally rejects a missing or null flag.

Verification:

- `cargo fmt --all -- --check` passed.
- `./scripts/check-local.sh` passed (`all checks passed (mode: default)`).
- Because the file is `#![cfg(target_os = "windows")]` and cannot compile on
  Linux, a temporary cfg-stripped copy was compiled and linted locally with
  `cargo clippy -p greggd --all-features --tests` (pedantic warn level): clean,
  zero warnings. The copy was deleted before the commit.
- The same temporary copy was used to exercise the harness mechanics against a
  real foreground `greggd` on Linux with a platform-appropriate status body
  (also deleted before the commit). Observed behavior: the happy path passed;
  a daemon that exited immediately failed in 200 ms with
  `exited with code 1` and the daemon's own captured stderr
  (`configuration validation failed: - port 0 is outside valid range 1..=65535`);
  a live child that never listened still failed at the full 30 s deadline with
  `still running at failure, terminated by this test` and
  `connect to 127.0.0.1:<port>: Connection refused`; dropping the owner without
  an explicit stop reaped the child (the `/proc/<pid>` entry was gone); and the
  capture tail truncated, bounded, and reported empty/absent files correctly.
- Native Windows truth: CI run `36919813734` (commit `2ceafcd3`) green across
  all six jobs — Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.89, and
  FreeBSD 14.2 native `gregg-host`. The Windows job's Test step passed with
  both smoke tests running under default harness concurrency
  (`windows_daemon_binary_compiles_and_runs ... ok` at 20:14:56.69Z,
  `foreground_daemon_serves_v2_status ... ok` at 20:14:57.19Z), followed by the
  release `greggd` and `gregg` builds and the Windows SCM lifecycle smoke. The
  smoke test binary went from 49.63 s in the failing run `36915516145` (18.5 s of
  it the nested `cargo build`) to about 0.53 s.
- No timeout, retry, `continue-on-error`, skip, or Windows-job weakening was
  added, and the workflow file is byte-identical to its pre-plan state.

Scope reconciliation:

- Only `crates/greggd/tests/windows_smoke.rs` changed. `greggd` runtime,
  collector, sampler, server, SCM service, protocol, API, and configuration
  behavior are untouched, so no corrected product defect was exposed and no
  separately scoped product corrective was needed.
- Inspected `architecture/greggd-daemon.md` and
  `.opencode/skills/greggd-daemon/SKILL.md`, which describe this test as
  "binary help + foreground daemon + v2 health polling". That remains accurate
  and no stale statement names the nested build or the hash-derived port, so no
  live-doc edit was required. Nothing user-visible changed, so no
  `README.md`, crate README, or `CHANGELOG.md` entry applies (same treatment as
  completed Plan 149). Plans 074/075/149 history was not rewritten.
- The root cause of run `36915516145` remains unproven — the old harness
  discarded exactly the evidence that would have shown it, and the corrected
  harness did not reproduce it. The commit that added this plan
  (`fd9433675b31afcf92cc6c5dc31811abfe52403a`, run `36917837384`) was green
  with the old harness, which is consistent with a transient hosted-runner
  condition rather than a product regression.

Future-plan impact: Plan 150 is terminal in the dependency order
(`149 -> 150`); it unblocks nothing further. The two remaining open plans were
already recorded as independent of it and keep their statuses unchanged: Plan
091 stays in implementation progress pending its extended soak record, and Plan
147 stays planned. No index status beyond Plan 150 itself required a change.
