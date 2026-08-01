# Phase 61: EggPool refresh correctness and closure correction

Status: completed; implementation `1b77da1`; ordinary CI `30681153449` passed.

## Objective

Correct the remaining EggPool worker/event-loop defects discovered after the Phase 60 implementation and restore a truthful closure state for Roadmap 56.

The existing implementation is structurally sound and remains the baseline. This phase is a narrow corrective pass over:

- periodic result generation matching;
- the 60-second active-pane cadence;
- reliable worker-command delivery;
- deterministic timing and saturation tests;
- stale four-row client metadata;
- Roadmap 56 and Phase 60 closure wording.

This phase must not redesign the EggPool pane, add product scope, or replace the existing client architecture.

## Why this phase is required

Implementation commit `1406c2b` and ordinary CI run `30660744394` established that the code compiles and the existing tests pass on Linux, macOS, Windows, and Rust 1.75. Subsequent source review found three correctness gaps that those tests do not exercise.

### Defect 1: periodic results use an unknown generation

`AppState::apply_eggpool_result` accepts a result only when:

```text
result.generation == eggpool.request_generation
result.period == eggpool.period
```

User-triggered requests receive a generation from `AppState`, but the worker independently increments its local generation on a periodic timer tick. State is not informed of that new generation. The first timer-driven result is therefore rejected as stale, and later periodic results remain rejected.

Observed consequence:

```text
activation generation 1 -> accepted
periodic generation 2   -> rejected; state still expects 1
periodic generation 3   -> rejected
...
```

The worker continues issuing network requests while the visible summary no longer updates automatically.

### Defect 2: the timer is anchored at worker startup

The worker constructs a repeating interval when it is spawned. Activation, period change, and manual refresh do not reset that interval.

A user may therefore activate EggPool immediately before the pre-existing interval tick:

```text
worker starts at t=0 while Systems is active
pane activates at t=59 and performs an immediate request
old interval ticks at t=60 and performs another request
```

This violates the documented active cadence because two request starts may occur almost back-to-back. The same problem applies to period changes and `Ctrl-R`.

### Defect 3: `try_send` silently loses state-changing commands

The event loop applies a pane/period action to `AppState` and then uses `try_send` for EggPool `Activate`, `Deactivate`, `SetPeriod`, and `Refresh` commands. All send errors are ignored.

If the bounded command channel is full:

- state may advance its generation and enter `Refreshing` without a matching worker request;
- a period may change in the UI while the worker continues using the old period;
- a dropped `Deactivate` may leave passive EggPool traffic active on the Systems pane;
- a dropped `Activate` may leave the EggPool pane waiting indefinitely;
- results from queued older commands may be rejected after state has advanced.

The queue is bounded, but bounded silent loss is not correct coalescing.

### Verification and documentation gap

The current worker timing test only proves that an inactive worker does not emit a result during a short wait. It does not verify activation followed by timer refresh, generation acceptance, timer reset, rapid command delivery, or pane re-entry near a tick boundary.

The Roadmap 56 and Phase 60 closure records therefore overstate periodic-refresh coverage. Public crate/CLI metadata also retains the obsolete phrase that systems render in four rows.

## Dependencies and execution position

Depends on the implementation present in Phases 57 through 60.

This phase supersedes the Phase 60 closure claim only for the defects listed above. It does not reopen completed configuration, endpoint parsing, authentication, metric semantics, rendering, system polling, drive rendering, or cross-platform support unless a focused regression directly demonstrates a related failure.

Dependency chain:

```text
57 + 58 + 59 + 60 -> 61
61 -> truthful Roadmap 56 closure
```

## Governing invariants

1. EggPool integration remains entirely inside the `gregg` client crate.
2. One configured EggPool endpoint remains the maximum.
3. Only `/api/stats/summary` and the four fixed periods remain supported.
4. Only one EggPool HTTP request may be in flight.
5. User-triggered period or refresh changes supersede older requests.
6. Periodic refresh uses the current desired-state generation; it does not invent a state-unknown generation.
7. Request starts for one active period occur no faster than 60 seconds apart unless the user explicitly activates, changes period, or requests a manual refresh.
8. Activation, period change, and manual refresh reset the passive deadline so an old timer cannot cause an immediate duplicate.
9. Inactive panes generate no periodic EggPool requests.
10. State-changing EggPool commands are never silently discarded.
11. A closed worker channel becomes a bounded local unavailable state; it does not leave the UI permanently refreshing or terminate greggd monitoring.
12. Existing greggd scheduler behavior and `Ctrl-R` behavior on Systems remain unchanged.
13. Rendering remains I/O-free.
14. No new dependency, workflow, service container, evidence bundle, or release machinery is added.
15. Closure status is updated only after focused tests, the normal local check, and ordinary CI pass.

## Scope

### In scope

- `EggpoolCommand` delivery semantics;
- worker generation/epoch behavior;
- replacement of the startup-anchored repeating interval with an activation/request-relative deadline;
- event-loop send failure handling;
- focused reducer helpers required to clear a failed dispatch;
- deterministic paused-time worker tests;
- rapid-input/channel-pressure tests;
- active README/help/package-description corrections for the five-row Systems contract;
- Roadmap 56, Phase 60, Phase 61, and plan-index status reconciliation.

### Out of scope

- EggPool server or database changes;
- additional endpoints, metrics, periods, panes, or EggPool instances;
- configurable EggPool refresh intervals;
- retry, exponential backoff, circuit breakers, persistent caches, or history;
- provider/model/account drill-down, costs, request logs, charts, or alerts;
- a generic scheduler, command bus, actor framework, effect system, datasource plugin, or screen registry;
- live config file watching;
- changes to `greggd`, `gregg-protocol`, native collectors, service management, packaging, or release automation;
- broad state/UI refactors unrelated to the three defects.

## Design decision: keep generation as a desired-state epoch

The existing generation value should remain a state epoch, not become a count of every automatic request.

Required semantics:

```text
activation/manual refresh/period change
    -> AppState advances generation
    -> worker receives that generation
    -> older in-flight result is rejected

periodic refresh for unchanged active period
    -> worker reuses the current generation
    -> result remains acceptable to AppState
```

Do not add a second timer-generation counter to `AppState`. Do not weaken stale-result rejection to accept arbitrary newer worker generations. Reusing the current epoch is sufficient because the worker permits only one in-flight request and starts a periodic request only after the previous one has completed or been superseded.

Renaming `generation` to `epoch` is optional and should be done only if it makes the focused diff clearer without forcing broad churn. Behavioral correction is required; repository-wide renaming is not.

## Workstream A: correct periodic generation ownership

Update the worker so its passive refresh branch starts a request with the current command-supplied generation rather than incrementing a private generation.

Conceptual behavior:

```rust
// Current desired state supplied by Activate/SetPeriod/Refresh.
let generation = current_generation;

// Passive refresh does not change desired state.
start_request(period, generation);
```

Requirements:

- `Activate`, `SetPeriod`, and `Refresh` continue to install the state-supplied generation;
- passive refresh reuses it;
- a later user command with a higher generation aborts/supersedes the previous request;
- a late old-generation result remains rejected;
- pane deactivation does not need to invent a generation;
- no result acceptance rule is loosened to compare only period.

### Workstream A tests

Use a synthetic server and controlled time:

1. activate generation 1;
2. receive and apply success generation 1;
3. advance to the passive deadline;
4. receive a second result still carrying generation 1;
5. apply it successfully and verify the summary/timestamp changes.

Also verify:

- manual refresh advances to generation 2 and rejects a raced generation-1 result;
- period change advances the generation and rejects an old-period result;
- repeated passive results for the same epoch remain accepted in order.

### Workstream A acceptance criteria

- [x] Passive results are accepted by current state.
- [x] User supersession still rejects stale results.
- [x] Only one in-flight request remains possible.
- [x] No arbitrary-generation acceptance rule is added.

## Workstream B: make passive cadence relative to request triggers

Replace the worker-start-anchored repeating interval with one optional next-refresh deadline.

Preferred small model:

```text
inactive
    -> no passive deadline

start request because of Activate/SetPeriod/Refresh
    -> next_refresh_at = request_start + 60 seconds

start passive request
    -> next_refresh_at = request_start + 60 seconds

Deactivate
    -> clear passive deadline
```

A `tokio::time::Sleep` reset through a pinned future or a small optional-deadline helper is acceptable. Keep the implementation local to `eggpool.rs`.

The invariant is based on request start times:

```text
next request start >= previous request start + 60 seconds
```

If a request itself lasts more than 60 seconds, the next passive request may start after it completes because concurrency remains one; this still satisfies the no-faster-than-60-seconds rule.

Requirements:

- the worker has no active passive deadline while inactive;
- activation always fetches immediately and establishes a fresh deadline;
- period change fetches immediately and establishes a fresh deadline;
- manual refresh fetches immediately and establishes a fresh deadline;
- an old deadline cannot fire immediately after any of those actions;
- deactivation prevents later passive requests;
- reactivation establishes a new full 60-second window;
- the 60-second value remains a private constant, not configuration.

### Workstream B deterministic tests

Use Tokio paused time. If needed, enable Tokio's existing `test-util` feature for tests only; do not add a new crate.

Required scenarios:

1. worker exists inactive for 59 seconds, then activates:
   - one immediate request;
   - no second request one second later;
   - second request only after 60 seconds from activation.

2. active request succeeds, then at 59 seconds a manual refresh occurs:
   - immediate manual request;
   - old deadline does not fire one second later;
   - next passive request starts 60 seconds after manual refresh.

3. active request succeeds, then period changes at 59 seconds:
   - immediate new-period request;
   - no old-period tick at the original deadline;
   - next passive request uses the new period after a full interval.

4. deactivate before deadline:
   - no request at or after the old deadline;
   - reactivation fetches once and starts a new deadline.

5. a request lasting beyond the deadline:
   - no concurrent second request;
   - next request starts only after the first completes and never less than 60 seconds after its start.

### Workstream B acceptance criteria

- [x] No two passive/automatic request starts occur less than 60 seconds apart.
- [x] Explicit activation, period change, and manual refresh remain immediate.
- [x] Every explicit trigger resets the passive deadline.
- [x] Inactive time does not accumulate toward an immediate request burst.
- [x] Tests use controlled time rather than production-duration sleeps.

## Workstream C: remove silent command loss

Keep the existing bounded `mpsc` command channel. Do not add a watch-channel state mirror or generic command bus unless the simple correction is demonstrably impossible.

Preferred correction:

- make the narrow EggPool action-dispatch helper async;
- use `commands.send(command).await` instead of `try_send` for `Activate`, `Deactivate`, `SetPeriod`, and `Refresh`;
- preserve the existing bounded capacity;
- handle a closed receiver explicitly;
- keep greggd's existing refresh-channel behavior unchanged.

The worker processes commands while HTTP work runs in a separate join handle, so an awaited send should normally complete immediately. A bounded await is preferable to mutating state and silently discarding the corresponding command.

### Send-failure handling

If the worker receiver is closed before global shutdown:

- do not leave `EggpoolStatus::Refreshing` indefinitely;
- set the EggPool state to idle/unavailable through one focused reducer method;
- retain a prior same-period success if present;
- render one stable local-worker-unavailable message;
- keep Systems polling and input responsive;
- do not retain a raw channel error string.

A dedicated `WorkerUnavailable` outcome is acceptable if it is the smallest truthful representation. Reusing `NetworkError` for a local channel failure is discouraged because it misstates the failure domain.

### Workstream C tests

Required deterministic tests:

- fill or pressure the command channel, dispatch a final period change, and prove the worker eventually observes the final desired period;
- rapidly issue period movement followed by pane exit and prove the final state is inactive with no passive request;
- dispatch activation through the real helper and prove state does not remain refreshing without a corresponding command;
- close the worker receiver, dispatch a state-changing command, and prove the UI becomes bounded unavailable rather than hanging;
- preserve `Ctrl-R` separation: Systems refresh does not send EggPool work and EggPool refresh does not hit the greggd channel.

Do not build a full process-level input harness. Test the focused async dispatch helper and worker channels directly.

### Workstream C acceptance criteria

- [x] No EggPool state-changing command uses ignored `try_send` failure.
- [x] State and worker converge on the final requested pane/period.
- [x] Channel closure cannot leave permanent refreshing state.
- [x] Command capacity remains small and bounded.
- [x] No generic command/effect framework is introduced.

## Workstream D: strengthen the minimum worker integration coverage

Replace or correct the misleading test named `worker_fetches_activation_and_suppresses_inactive_ticks`. The current body sends only `Deactivate` and does not test activation.

Minimum focused test set:

1. public activation uses the exact 1-hour request and returns an applicable result;
2. passive refresh returns an applicable same-generation result;
3. inactive worker produces no passive traffic under advanced time;
4. activation near a former startup tick does not burst;
5. manual refresh resets cadence;
6. period change resets cadence and uses the correct API value;
7. stale user-superseded result is rejected;
8. rapid commands do not strand state or leave hidden periodic traffic;
9. shutdown/cancellation aborts an in-flight request promptly;
10. no EggPool configuration still creates no worker and no EggPool request.

Keep existing authentication, body-limit, status, decode, semantic, rendering, and system-regression tests. Do not duplicate them in a second harness.

### Workstream D acceptance criteria

- [x] Test names describe behavior actually exercised.
- [x] Generation and cadence defects each have a regression test that fails on `1406c2b`.
- [x] Tests are deterministic and fast.
- [x] No real EggPool service or credentials are required.

## Workstream E: correct active metadata and closure truth

Correct only active documentation affected by this pass.

### Client metadata

Update stale four-row wording in:

- the `gregg` crate package description;
- CLI `long_about` text;
- any active README sentence still claiming four rows.

Use the current five-row-base wording or a neutral compact-system-block description. Do not rewrite historical Phase 8 material solely for this correction.

### Planning status

At implementation start, Roadmap 56 must remain reopened through Phase 61. Phase 60 may remain recorded as the original implementation/CI pass, but it must not be represented as final closure while Phase 61 is open.

After implementation and verification:

- mark Phase 61 completed with the implementation commit and one ordinary CI run;
- mark Roadmap 56 completed through Phases 57–61;
- state that Phase 61 corrected periodic generation, cadence anchoring, and command delivery;
- do not erase the historical Phase 60 implementation or CI result;
- do not create a separate evidence file.

### Workstream E acceptance criteria

- [x] Active metadata no longer claims a four-row normal system block.
- [x] Plan index shows Phase 61 as the active correction until verified.
- [x] Roadmap closure is not restored before tests/local check/CI pass.
- [x] Historical implementation records remain concise and truthful.

## Workstream F: lightweight verification and final closure

Use the smallest relevant checks during implementation:

```text
cargo fmt --all -- --check
cargo test -p gregg eggpool --all-features
cargo test -p gregg state --all-features
cargo test -p gregg --all-targets --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

Final local check:

```text
./scripts/check-local.sh
```

Then use one ordinary existing cross-platform CI run. No workflow change is required because the correction is platform-neutral async/state logic and the existing jobs already compile/test `gregg` on Linux, macOS, Windows, and the Rust 1.75 MSRV.

A live EggPool smoke is optional and must not become a closure requirement.

### Workstream F acceptance criteria

- [x] Focused corrective tests pass.
- [x] Existing full `gregg` tests pass.
- [x] Existing local check passes.
- [x] One ordinary CI run passes without workflow changes.
- [x] No retained artifacts, screenshots, service containers, or manual platform evidence are added.

## Expected files

The correction should normally remain within:

```text
crates/gregg/src/eggpool.rs
crates/gregg/src/main.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/eggpool.rs          # only if WorkerUnavailable needs display text
crates/gregg/src/cli.rs                 # stale long_about wording only
crates/gregg/Cargo.toml                 # stale description; optional test-util feature only
README.md                               # only if stale four-row wording remains
AGENTS.md                               # only if cadence/generation wording needs correction
plans/056-eggpool-summary-pane-roadmap.md
plans/060-eggpool-pane-integration-and-lightweight-closure.md
plans/061-eggpool-refresh-correctness-and-closure.md
plans/README.md
```

Do not touch EggPool, `greggd`, `gregg-protocol`, workflows, release scripts, collector code, service-management code, or archived plans.

## Implementation sequence

1. Add regression tests that demonstrate the periodic-generation rejection and startup-anchored timer burst.
2. Define generation explicitly as the current desired-state epoch.
3. Make passive requests reuse the current epoch.
4. Replace the repeating startup interval with an optional request-relative deadline.
5. Add paused-time activation, refresh, period, deactivate, and long-request tests.
6. Convert EggPool command dispatch to reliable awaited sends.
7. Add bounded worker-channel-closure state handling.
8. Add rapid-command/final-state convergence tests.
9. Run existing authentication, rendering, state, and system regression tests.
10. Correct stale four-row active metadata.
11. Run focused checks and `./scripts/check-local.sh`.
12. Push the implementation and observe one ordinary CI run.
13. Update Phase 61/Roadmap 56 status from actual results only.
14. Inspect the final diff and remove any abstraction or unrelated cleanup not required by the defects.

## Phase acceptance criteria

Phase 61 is complete only when:

- [x] A periodic request reuses the current state generation and its result updates the displayed summary.
- [x] User activation, manual refresh, and period changes still supersede older generations.
- [x] No old-period or old-generation result can overwrite current state.
- [x] Passive request starts are never closer than 60 seconds apart.
- [x] Activation, period change, and manual refresh reset the passive deadline.
- [x] Inactive time cannot cause an immediate post-activation duplicate request.
- [x] Deactivation prevents later passive requests.
- [x] Only one EggPool request can be in flight.
- [x] EggPool state-changing commands are not silently dropped under channel pressure.
- [x] Worker channel closure cannot strand the UI in refreshing state or stop greggd monitoring.
- [x] Systems and EggPool `Ctrl-R` paths remain isolated.
- [x] Deterministic tests cover generation, cadence reset, deactivation, rapid commands, stale results, and shutdown.
- [x] Existing authentication, response-bound, metric-semantic, rendering, system navigation, drive, and greggd polling tests remain green.
- [x] Active client metadata no longer says the normal system block uses four rows.
- [x] The change remains inside the listed narrow client/planning surface.
- [x] Focused tests, the existing local check, and ordinary CI pass.
- [x] No EggPool server change, new metric/period/pane/instance, retry system, configurable cadence, generalized abstraction, new dependency, workflow, evidence system, or release machinery is added.

## Roadmap closure correction

Roadmap 56 may be considered closed again only after Phase 61 is complete.

The final closure record must distinguish:

```text
1406c2b / CI 30660744394
    original EggPool pane implementation and initial cross-platform verification

<phase-61 implementation SHA> / <ordinary CI run>
    periodic generation, cadence, reliable command delivery, and closure correction
```

Do not rewrite history to claim the original run covered the new regression tests. Do not require repeated CI runs or an evidence bundle.

## Handoff guidance for a smaller implementation model

- Keep the existing worker and channels; fix their semantics rather than replacing them.
- Treat `generation` as the current desired-state epoch. Passive refresh does not change desired state.
- Use one optional deadline that is reset whenever a request is intentionally started.
- Prefer awaited bounded sends over ignored `try_send` failures.
- Add a small explicit worker-unavailable state only if needed for closed-channel handling.
- Write the two failing regression tests first: rejected periodic result and activation-near-old-tick burst.
- Use paused Tokio time; never wait 60 real seconds.
- Do not modify the EggPool API, add retries, or generalize the integration.
- Stop if the implementation begins touching protocol, daemon, workflows, multiple endpoints, or a generic scheduler architecture.


## Closure record

- Implementation commit: `1b77da1` (`fix: close EggPool refresh correctness gaps`).
- Local verification: ./scripts/check-local.sh passed on Linux.
- Ordinary CI: run `30681153449` passed Linux, macOS arm64, macOS Intel, Windows, and Rust 1.75 MSRV jobs.
- Phase 61 corrected periodic generation ownership, request-relative cadence, reliable command delivery, worker-channel closure handling, active five-row metadata, and Roadmap 56 closure truth.
