# Phase 62: EggPool worker regression coverage and closure polish

Status: planned.

## Objective

Complete Roadmap 56 truthfully by adding the narrow deterministic worker/event-loop regression coverage that Phase 61 required but did not implement, then correct the affected closure metadata.

Phase 61 implementation commit `1b77da1` corrected the known runtime defects:

- passive refresh now reuses the current state-owned generation;
- the fixed 60-second passive deadline is relative to activation, period changes, manual refresh, and passive request starts;
- deactivation clears the passive deadline;
- EggPool state-changing commands use bounded awaited delivery rather than ignored `try_send` failures;
- worker-channel closure becomes a bounded `WorkerUnavailable` state;
- stale four-row client metadata was corrected.

Ordinary CI run `30681153449` passed. The remaining problem is not a newly identified runtime defect. It is a verification and closure-integrity gap: the worker timing test was removed, but the deterministic cadence, generation, channel-pressure, deactivation, and shutdown tests promised by Phase 61 were not added. Phase 61 and Roadmap 56 were nevertheless marked complete.

This phase adds only the missing focused tests and closure corrections. It must not redesign the worker or broaden the EggPool feature.

## Dependencies and execution position

Depends on:

- Roadmap 56 and Phases 57 through 60;
- corrective implementation from Phase 61 at `1b77da1`;
- the repository's existing minimal local/ordinary-CI verification model.

Phase 62 is the only open item for the optional EggPool summary-pane line. Roadmap 56 remains functionally implemented but is not finally closed until this phase passes.

## Governing invariants

1. Product behavior from `1b77da1` is the baseline; alter it only if a deterministic regression test exposes a real defect.
2. Tests target the existing worker, state reducer, and focused dispatch helper rather than a process-level TUI harness.
3. Use synthetic loopback HTTP only; no live EggPool, database, API key, dashboard, provider, or internet dependency.
4. Use Tokio controlled time for 60-second behavior; never wait a production minute.
5. Keep the test set small by combining related assertions where doing so remains readable.
6. Do not add a generic clock, scheduler abstraction, datasource framework, command bus, retry system, or test-only product architecture.
7. Do not add a new CI job, workflow, service container, artifact, evidence bundle, screenshot, or release step.
8. Do not modify EggPool, `greggd`, `gregg-protocol`, collectors, service management, packaging, or release scripts.
9. One bounded worker, one endpoint, one in-flight request, four periods, four metrics, and the fixed cadence remain unchanged.
10. Closure checkboxes and status text may be restored only after the tests, existing local check, and ordinary CI pass.

## Current verification gap

The current `crates/gregg/src/eggpool.rs` tests cover:

- period mappings and clamping;
- public and protected request construction;
- secret non-retention;
- missing credentials;
- status/decode/body-limit behavior;
- summary semantic validation.

The current `main.rs` tests cover:

- basic pane and `Ctrl-R` command routing;
- clamped period movement;
- closed command-channel handling.

They do not execute the worker's passive deadline or prove:

- a second timer-driven request retains the state generation;
- activation after long inactive time starts a fresh 60-second deadline;
- manual refresh and period change reset that deadline;
- deactivation suppresses later passive traffic;
- awaited delivery converges under bounded-channel pressure;
- cancellation stops an in-flight worker promptly.

A green compile/test run therefore does not substantiate the corresponding checked Phase 61 criteria.

## Workstream A: add one reusable bounded synthetic server helper

Extend the existing loopback test helper only as much as needed to observe multiple requests.

Required properties:

- bind to `127.0.0.1:0`;
- accept a caller-specified bounded number of requests, or run until a cancellation token closes it;
- read only through the end of HTTP headers;
- record request path/query and request order through a bounded channel or returned vector;
- return a fixed small valid summary body selected by the request period;
- optionally hold one accepted request open for cancellation testing;
- terminate deterministically and leave no detached task.

Prefer a small test-only helper local to `eggpool.rs`. Do not add a reusable production HTTP server, external mock-server crate, or broad integration-test framework.

If Tokio controlled-time support is not currently enabled, add only the existing Tokio `test-util` feature in the appropriate development/test feature set. This is not a new dependency. Do not alter runtime behavior or MSRV.

### Workstream A acceptance criteria

- [ ] The helper handles the exact bounded request count needed by worker tests.
- [ ] Captured requests expose period/query and ordering without storing secrets or full bodies.
- [ ] The helper shuts down within the test and does not rely on long real-time sleeps.
- [ ] No new third-party test dependency or production abstraction is added.

## Workstream B: prove passive generation acceptance

Add one deterministic worker/state test covering the original generation defect end to end.

Required sequence:

```text
create configured AppState and worker
send Activate(period=1h, generation=1)
receive first result
apply result -> summary accepted
advance controlled time to 60 seconds
receive passive result
assert passive result period=1h and generation=1
apply passive result -> visible summary updates
```

Use distinct summary values for the first and passive responses so the final assertion proves the second result, not merely retained state, was applied.

Also prove a later user supersession remains protected:

```text
state advances to generation 2 for a user-triggered request
late generation-1 result arrives
state rejects it
```

Reuse the existing stale-result reducer test if it already proves this exact rule; do not duplicate it unnecessarily. The new worker test must specifically prove the passive result retains generation 1.

### Workstream B acceptance criteria

- [ ] The test fails against the pre-Phase-61 worker that increments generation on a timer tick.
- [ ] The first and passive results both carry the current state generation.
- [ ] Applying the passive result changes the displayed summary to the second response value.
- [ ] User-superseded stale results remain rejected.
- [ ] Only one request is in flight at a time.

## Workstream C: prove request-relative cadence and inactive gating

Cover cadence behavior with controlled time. Keep this to one or two tests, using clearly separated subcases if practical.

### Scenario 1: activation after inactive time

```text
spawn worker inactive
advance 59 seconds
send Activate -> immediate request
advance 1 second -> no second request
advance remaining 59 seconds -> one passive request
```

This proves the deadline is not anchored at worker startup.

### Scenario 2: explicit triggers reset the deadline

For manual refresh:

```text
activate -> immediate request
advance 59 seconds
send Refresh with next generation -> immediate request
advance 1 second -> no passive request
advance remaining 59 seconds -> one passive request using refreshed generation
```

For period change:

```text
activate 1h
advance 59 seconds
send SetPeriod(24h, next generation) -> immediate 24h request
advance 1 second -> no old-deadline request
advance remaining 59 seconds -> one passive 24h request
```

It is acceptable to combine manual refresh and period change in one table-driven/helper-backed test, provided failure output identifies which trigger violated the cadence.

### Scenario 3: deactivation suppresses passive work

```text
activate -> immediate request completes
send Deactivate
advance at least 120 controlled seconds
assert zero additional requests/results
```

Do not assert vague timing tolerances. Under paused time, assert exact request-count transitions at 59 and 60 seconds.

### Workstream C acceptance criteria

- [ ] Inactive elapsed time cannot cause an immediate post-activation duplicate request.
- [ ] Activation, manual refresh, and period change each establish a fresh 60-second passive deadline.
- [ ] Period change requests and later passive requests use the selected API value.
- [ ] Deactivation clears the deadline and prevents later passive traffic.
- [ ] No production-duration sleep is introduced.

## Workstream D: prove bounded command delivery and final convergence

The Phase 61 correction changed the focused action dispatcher to await bounded `mpsc::Sender::send`. Add one direct regression test that creates actual channel pressure rather than only testing a closed receiver.

Required behavior:

1. use the real command channel capacity or a smaller focused test channel;
2. fill the queue with valid state-changing commands while a receiver/worker is temporarily unable to drain it;
3. dispatch a final meaningful action through the real async helper;
4. resume draining;
5. prove the helper completes and the final command is observed in order;
6. prove application state and worker desired state converge on the final pane/period;
7. include a final `Deactivate` or equivalent assertion proving no hidden passive request remains after returning to Systems.

Do not simulate pressure by permanently abandoning the receiver, which only tests channel closure. Do not add an unbounded queue. Do not require every intermediate keypress to launch an HTTP request; correct ordered/coalesced convergence to the final requested state is the goal.

The test may use a small receiver task rather than the full network worker if that isolates the delivery contract more deterministically. If it uses the worker, keep the synthetic response count bounded.

### Workstream D acceptance criteria

- [ ] The test would expose the old ignored-`try_send` state/worker desynchronization.
- [ ] Bounded pressure delays rather than silently discards the final command.
- [ ] The dispatcher remains finite once the receiver resumes.
- [ ] Final pane/period/activation state matches the final requested action.
- [ ] Channel capacity remains bounded and unchanged in production.

## Workstream E: prove prompt cancellation of in-flight work

Add one bounded shutdown test:

```text
synthetic server accepts request and withholds response
worker receives Activate and starts request
cancel shared CancellationToken or send Shutdown
worker aborts request and closes result path promptly
server/helper is released
```

Use a short test timeout only as a deadlock guard, not as the behavior under test. Do not wait for the production HTTP timeout. Confirm there is no completed success/error result emitted after cancellation unless the existing contract explicitly allows a race that completed before cancellation.

If cancellation through the shared token and explicit `Shutdown` are mechanically identical after command receipt, one representative test is sufficient. Do not duplicate both solely for checkbox count.

### Workstream E acceptance criteria

- [ ] An in-flight request does not keep the worker alive until the HTTP timeout.
- [ ] Cancellation closes the worker path promptly and deterministically.
- [ ] The synthetic server task is joined or cancelled.
- [ ] No task/channel leak is left by the test.

## Workstream F: preserve the no-config and system-monitoring boundary

Confirm the existing optional construction seam still proves:

```text
config.eggpool == None
    -> no EggpoolClient
    -> no worker
    -> no command/result channel
    -> no EggPool request
```

Add a test only if current coverage does not directly prove this. Prefer testing a small existing runtime-construction helper over introducing a new application harness.

Also retain existing assertions that:

- Systems `Ctrl-R` does not send EggPool work;
- EggPool `Ctrl-R` does not trigger greggd refresh;
- EggPool channel failure does not stop system monitoring;
- system selection/layout/drive state is preserved across pane movement.

Do not duplicate broad system, drive, collector, protocol, or renderer suites.

### Workstream F acceptance criteria

- [ ] No-config startup cannot construct or contact EggPool.
- [ ] Active-pane refresh routing remains isolated.
- [ ] Existing greggd polling and TUI state tests remain green.
- [ ] No process-level terminal harness is added.

## Workstream G: correct closure metadata

After the focused tests pass locally, update planning truth.

Required corrections:

- remove the duplicated client-only sentence in `plans/README.md`;
- mark Roadmap 56 as reopened for Phase 62 until verification is complete;
- describe Phase 61 as runtime correction implemented at `1b77da1`, with its original closure superseded because required worker regression coverage was absent;
- register Phase 62 as the active verification-polish phase;
- update the dependency graph to `60 -> 61 -> 62`;
- update plan-range and scope wording from `056-061` to `056-062` where appropriate;
- correct the status line and phase map in `056-eggpool-summary-pane-roadmap.md`;
- add a concise note to Phase 61 that its implementation remains valid but final closure is owned by Phase 62;
- do not erase the successful `30681153449` CI history or claim it exercised tests that were not yet present.

Final closure may record:

- final implementation/test commit SHA;
- `./scripts/check-local.sh` result;
- one ordinary CI run at that SHA or a source-equivalent plan-only descendant;
- a concise statement that the missing worker regressions now exist.

Do not create a separate evidence file.

### Workstream G acceptance criteria

- [ ] Roadmap, Phase 61, Phase 62, and the plan index agree on status and ownership.
- [ ] Historical commits and CI runs remain accurate.
- [ ] No checkbox claims coverage absent from the repository.
- [ ] The duplicated index sentence is removed.
- [ ] Phase 62 is closed only after its own tests and ordinary verification pass.

## Expected files

Expected narrow implementation surface:

```text
crates/gregg/src/eggpool.rs
crates/gregg/src/main.rs                 # only if pressure/no-config test seam belongs here
crates/gregg/Cargo.toml                  # only if Tokio test-util feature is required
Cargo.lock                               # only if feature resolution changes it
plans/056-eggpool-summary-pane-roadmap.md
plans/061-eggpool-refresh-correctness-and-closure.md
plans/062-eggpool-worker-regression-coverage-and-closure-polish.md
plans/README.md
```

Potentially unnecessary and therefore discouraged:

```text
new integration-test crate
tests/process_tui.rs
new mock-server dependency
new scheduler/clock module
workflow changes
EggPool repository changes
crates/greggd/**
crates/gregg-protocol/**
release scripts
```

## Verification sequence

Use focused checks during implementation:

```text
cargo test -p gregg eggpool
cargo test -p gregg main::tests
cargo test -p gregg state::tests
```

Run the complete existing client suite:

```text
cargo test -p gregg
```

Then run the repository's existing local gate:

```text
./scripts/check-local.sh
```

Use the existing PowerShell equivalent only when validating locally on Windows:

```text
.\scripts\check-local.ps1
```

Final hosted proof is one ordinary existing CI run. Do not add or modify workflow coverage for this platform-neutral test correction.

## Explicit acceptance criteria

Phase 62 is complete only when:

- [ ] A deterministic worker test proves activation and passive refresh return the current state generation.
- [ ] Applying the passive result changes the visible summary to a distinct second response.
- [ ] Existing stale user-generation protection remains green.
- [ ] Activation after 59 seconds of inactive worker lifetime does not cause a duplicate request one second later.
- [ ] Manual refresh resets the passive deadline.
- [ ] Period change resets the passive deadline and uses the correct API period.
- [ ] Deactivation prevents passive traffic under at least 120 seconds of controlled time.
- [ ] A bounded-pressure test proves the final state-changing command is delivered rather than silently lost.
- [ ] Final application and worker state converge after rapid commands.
- [ ] Cancellation aborts an in-flight request promptly without waiting for the HTTP timeout.
- [ ] No-config behavior still constructs no EggPool runtime path.
- [ ] Existing authentication, body-limit, semantic, rendering, navigation, drive, greggd polling, and active-pane refresh tests remain green.
- [ ] Tests are deterministic, bounded, and use no live service, credentials, long sleeps, or screenshot artifacts.
- [ ] No new third-party dependency, generalized abstraction, retry system, configurable cadence, metric, period, pane, endpoint, workflow, evidence system, or release mechanism is added.
- [ ] Roadmap 56, Phase 61, Phase 62, and `plans/README.md` state closure truthfully.
- [ ] Focused tests, `cargo test -p gregg`, `./scripts/check-local.sh`, and one ordinary CI run pass.

## Stop conditions

Stop and separate newly discovered work if implementation would require:

- changing EggPool's API or database;
- changing `greggd` or `gregg-protocol`;
- supporting multiple EggPool endpoints;
- adding new metrics, periods, panes, history, charts, alerts, retries, or configurable cadence;
- introducing a generalized scheduler, datasource, command, clock, or plugin architecture;
- adding a new CI workflow, service container, retained artifact, release step, or evidence system;
- turning this polish phase into broad test-suite expansion.

A test exposing a concrete additional defect in the existing worker may justify the smallest local correction inside `eggpool.rs`, `main.rs`, or `state.rs`. Record the defect in the Phase 62 closure note. Do not expand scope speculatively.