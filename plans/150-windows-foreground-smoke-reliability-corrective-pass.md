# Plan 150: Windows foreground smoke reliability corrective pass

Status: planned.

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

- [ ] `windows_smoke.rs` uses Cargo's `CARGO_BIN_EXE_greggd` integration-test binary path or an MSRV-compatible equivalent.
- [ ] The smoke no longer executes nested `cargo build -p greggd` subprocesses.
- [ ] The foreground smoke selects a loopback port from an OS `127.0.0.1:0` allocation rather than a deterministic test-name hash.
- [ ] No new random/port-allocation dependency is introduced.
- [ ] Child stdout/stderr are not left as unread pipes.
- [ ] Failure diagnostics include bounded daemon stdout/stderr, selected port/config context, and the last readiness probe outcome.
- [ ] The readiness loop checks `child.try_wait()` and fails immediately with exit status when the daemon exits.
- [ ] A child that remains alive but fails to become ready still fails at the thirty-second outer readiness deadline.
- [ ] Every panic/error/success path guarantees child termination and reaping.
- [ ] A failed smoke cannot leave a live `greggd.exe` for later tests in the Windows job.
- [ ] The existing v2 status, Windows capability, identity, and metric assertions remain at least as strong.
- [ ] No `greggd` production runtime, collector, service-manager, protocol, API, or configuration behavior changes unless the corrected diagnostics expose a separately demonstrated product defect.
- [ ] No CI retry wrapper, `continue-on-error`, test skip, or Windows-job weakening is introduced.
- [ ] Focused native Windows smoke passes with normal test-harness concurrency.
- [ ] The full existing Windows workspace test/build/SCM path passes.
- [ ] One ordinary six-job CI run is green.
- [ ] Closure records the exact implementation SHA and exact CI run ID.

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
