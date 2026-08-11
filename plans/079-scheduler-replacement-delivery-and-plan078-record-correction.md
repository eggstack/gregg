# Phase 079: reliable scheduler endpoint replacement and Plan 078 record correction

Status: complete.

Depends on: Plan 078 implementation through `18aebb1edbbbfe39924709257ec127a59dee689a`.

## Objective

Close the two concrete issues discovered during post-implementation review of Plan 078 without reopening the broader Gregg client/runtime architecture:

1. make Systems-pane endpoint replacement delivery reliable under bounded scheduler-command pressure so `AppState` cannot advance to a newly reloaded endpoint while the scheduler silently remains on an older endpoint set;
2. correct the Plan 078 verification record so it accurately distinguishes the originally reported working endpoint (`192.168.183.143:11310`) from the later environment in which `192.168.182.143:11310` was observed reachable during closure smoke.

This is a narrow correctness pass. Do not redesign the scheduler, add config watching, change the endpoint model, revisit URL parsing or `configprint`, or add CI/release machinery.

## Baseline findings

### 1. Endpoint replacement can be silently dropped after state reconciliation

Plan 078 correctly introduced:

```text
SchedulerCommand::Refresh
SchedulerCommand::ReplaceEndpoints(Vec<Endpoint>)
```

and `Ctrl-R` now reloads the current `ConfigStore`, reconciles systems in `AppState`, then sends a replacement command to the poll scheduler.

The current production path in `refresh_systems()` is effectively:

```rust
app_state.reconcile_systems(&config);
let endpoints = ...;
let _ = scheduler_tx.try_send(SchedulerCommand::ReplaceEndpoints(endpoints));
```

The result of `try_send()` is discarded.

The channel is bounded. Therefore `TrySendError::Full` can occur while the scheduler is occupied with a generation or while several manual refreshes are queued. In that case:

1. `AppState` has already moved to the newest host/port;
2. the `ReplaceEndpoints` command is lost;
3. the scheduler retains an older endpoint vector;
4. periodic polling continues against the older target;
5. `AppState::apply_batch()` correctly rejects those results because host/port no longer match;
6. the newly displayed target can remain indefinitely `Pending` until another replacement happens to be delivered.

The stale-result host/port guard from Plan 078 prevents metric contamination, but it does not make replacement delivery reliable.

This is a real state/scheduler divergence risk and should be fixed before considering the runtime reload path fully closed.

### 2. Existing scheduler tests do not cover the production pressure path

The scheduler test `replacement_command_polls_only_the_replacement_endpoint` sends the replacement with:

```rust
commands.send(SchedulerCommand::ReplaceEndpoints(...)).await
```

which guarantees delivery by waiting for capacity.

The production path uses ignored `try_send()`. Therefore the current test does not exercise the failure mode above.

The repository already contains an EggPool bounded-command-pressure test that deliberately verifies that important final state changes are not lost. The Systems scheduler needs equivalent focused coverage, but not a new generalized command framework.

### 3. Plan 078's closure record rewrote the originating endpoint fact

The originating live report for this line of work stated that:

```text
192.168.183.143:11310
```

was the verified working `greggd` instance, while Gregg rendered/polled:

```text
192.168.182.143:11310
```

and showed it offline.

Plan 078 was initially written around that `.182 -> .183` correction.

The final closure commit later changed the plan to state that `.182` was the verified live daemon and rewrote the smoke as `.183 -> .182`, because `.182` was reachable in the later environment and `.183` was not reachable during that closure run.

The implementation is address-symmetric and the later `.182` live smoke is still useful evidence. However, the record should not replace the original observed fact. It should state both facts explicitly:

- **originating report:** `.183` was reported verified working and `.182` was the stale/wrong address displayed by Gregg;
- **later closure environment:** `.182` was the address actually reachable during the final smoke, so the operational reload smoke was performed in the reverse direction.

The planning record must distinguish those two environment observations rather than presenting the later one as though it were the original report.

## Authoritative behavior after Plan 079

### Systems `Ctrl-R`

The Plan 078 behavior remains authoritative:

```text
Ctrl-R on Systems
    -> reload the already-resolved ConfigStore
    -> reconcile valid system entries into AppState
    -> replace scheduler endpoints
    -> immediately poll the replacement set
```

The additional invariant after Plan 079 is:

> Once a valid config reload is committed into `AppState`, the scheduler must eventually receive the corresponding endpoint replacement. A bounded full command channel must not cause the replacement to be silently lost.

The implementation may wait briefly for bounded channel capacity because `Ctrl-R` is an explicit user action, not a high-frequency telemetry path. Correctness is more important than making a manual refresh fire-and-forget.

### Ordinary refresh commands

An ordinary `Refresh` is lower-value and idempotent relative to a replacement. It may remain coalescible or droppable under pressure if that keeps the implementation small, provided doing so cannot discard a newer `ReplaceEndpoints` command or prevent the scheduler from converging to the latest configured endpoint set.

Do not build a complex priority queue. One reliable replacement path is enough.

### Endpoint replacement ordering

If multiple valid config reloads occur quickly, the scheduler must converge to the newest accepted endpoint set in command order.

Acceptable simple semantics:

```text
reload A -> replacement A delivered
reload B -> waits for capacity -> replacement B delivered after A
```

The scheduler will therefore eventually poll B, the latest config state.

An explicit coalescing design that guarantees the latest replacement wins is also acceptable, but only if it is smaller and clearly tested. Do not introduce shared watch channels or a second scheduler-control abstraction solely for coalescing.

### Failed config reload

Plan 078 behavior remains unchanged:

- preserve last-known-good `AppState`;
- do not partially apply the invalid config;
- request an ordinary refresh of the existing scheduler endpoints when practical;
- do not crash or block indefinitely.

A failed reload does not require reliable replacement semantics because no replacement has been committed to state.

## Preferred implementation shape

Keep the correction local to the existing main/scheduler command boundary.

The simplest preferred change is:

1. make the Systems refresh helper async;
2. after successful `store.load_existing()` and `app_state.reconcile_systems(&config)`, use bounded async `send(...).await` for `SchedulerCommand::ReplaceEndpoints` rather than ignored `try_send()`;
3. return/handle a closed-channel failure deliberately instead of silently pretending replacement occurred;
4. leave ordinary `Refresh` behavior small and nonblocking unless sharing the reliable helper is simpler.

Example shape:

```rust
async fn refresh_systems(...) {
    match store.load_existing() {
        Ok(config) => {
            app_state.reconcile_systems(&config);
            let endpoints = ...;
            send_replacement(scheduler_tx, endpoints).await;
        }
        Err(_) => {
            let _ = scheduler_tx.try_send(SchedulerCommand::Refresh);
        }
    }
}
```

If a closed scheduler channel can happen while the TUI remains alive, do not leave state silently diverged. Choose the smallest truthful behavior after inspecting ownership:

- return an error that exits the TUI through the existing error boundary; or
- expose a minimal unavailable state if one already exists naturally.

Do not add a new persistent scheduler-status UI solely for this case.

### Important ordering note

Because `AppState` reconciliation currently happens before command delivery, an awaited send can briefly leave the TUI showing the new target while waiting for bounded scheduler capacity. That is acceptable as long as delivery is guaranteed once capacity opens.

If implementation inspection shows the send can fail because the scheduler receiver closed, avoid committing an unrecoverable divergent state. A small alternative is to derive the replacement first, await/send it, then reconcile state immediately after successful enqueue. Either ordering is acceptable if tests demonstrate that state and scheduler cannot remain permanently inconsistent.

Do not introduce rollback machinery unless actually needed.

## Required regression tests

### A. Production-path command pressure

Add a focused async test around the actual `refresh_systems` / `dispatch_action_with_store` path with a scheduler command channel of capacity 1.

Test sequence:

1. create a temporary config with stable system ID and endpoint A;
2. create `AppState` from A;
3. fill the scheduler command channel so it has no capacity;
4. atomically update the same config entry to endpoint B;
5. invoke Systems `Ctrl-R` through the production refresh helper;
6. verify the refresh future does **not** complete by silently dropping the replacement while the channel is full;
7. drain the preexisting command to release one slot;
8. verify the refresh completes;
9. verify `ReplaceEndpoints(B)` is delivered;
10. verify `AppState` ends on B and is `Pending` with old metrics cleared.

The test should not depend on wall-clock sleeps. Use channel capacity, `yield_now`, and bounded `tokio::time::timeout` only as guards.

### B. Rapid sequential replacements

Add one focused test proving command ordering/convergence:

```text
A -> B -> C
```

with the same stable system ID.

Under bounded capacity, ensure both accepted reloads are not silently lost and the final delivered replacement / final state is C.

This may be implemented at the main helper level or scheduler command level, whichever gives the smallest truthful coverage.

Do not add a large stress harness.

### C. Existing replacement behavior remains green

Retain the existing scheduler test proving `ReplaceEndpoints` immediately polls only the replacement endpoint.

Retain stale-result rejection coverage showing an old endpoint result cannot overwrite a new endpoint with the same stable ID.

### D. Invalid reload remains last-known-good

Retain or strengthen the existing invalid-TOML test so the failure path still preserves state and does not wait indefinitely for replacement delivery.

## Plan 078 record correction

Update `plans/078-client-endpoint-url-config-reload-and-daemon-configprint.md` without erasing either environment observation.

At minimum, revise the live-host statements to something equivalent to:

```text
Originating report: 192.168.183.143:11310 was reported as the verified
working daemon, while the running client displayed/polled 192.168.182.143.

Later closure smoke: the environment had changed; 192.168.183.143 was not
reachable, while 192.168.182.143 returned ready health. The implemented
reload path was therefore exercised in the reverse .183 -> .182 direction.
This demonstrates address replacement behavior but does not rewrite the
original observation.
```

Do not claim that the `.182` smoke independently proves `.183` was never working. It only proves `.182` was working during that later run.

Do not remove the completed implementation/verification details that remain accurate.

After Plan 079 passes, append a short note to Plan 078 stating that Plan 079 corrected bounded replacement-delivery semantics discovered during review.

## Scope

### In scope

- `crates/gregg/src/main.rs` Systems refresh command delivery;
- tiny scheduler-command helper changes only if directly needed;
- focused bounded-channel regression tests;
- preserving endpoint/state generation safety from Plan 078;
- correcting Plan 078's historical/live-host wording;
- updating `plans/README.md` to register and then close Plan 079;
- local verification through existing checks.

### Out of scope

- filesystem watchers or automatic config reload;
- new timers/background config tasks;
- scheduler rewrite, actor framework, watch channel architecture, or generic prioritized queue;
- changing the poll concurrency model;
- changing endpoint IDs, schema, URL parsing, or `gregg add` behavior;
- changing `greggd configprint`;
- changing `greggd croncheck`;
- TUI redesign or new scheduler-error pane;
- EggPool behavior except keeping existing tests green;
- new dependencies;
- new GitHub Actions workflow/job/matrix/artifact;
- release automation;
- another closure-only plan after 079.

## Expected files

Primary implementation/test surface:

```text
crates/gregg/src/main.rs
```

Potentially touched only if the smallest implementation genuinely requires it:

```text
crates/gregg/src/scheduler.rs
```

Planning records:

```text
plans/078-client-endpoint-url-config-reload-and-daemon-configprint.md
plans/079-scheduler-replacement-delivery-and-plan078-record-correction.md
plans/README.md
```

No documentation outside planning records should need modification unless implementation changes a user-visible behavior beyond the already documented reliable `Ctrl-R` semantics.

## Implementation sequence

### Step 1: add the failing bounded-capacity production-path test

Before changing delivery semantics, reproduce the exact defect with a channel capacity of 1.

The test must demonstrate that current ignored `try_send()` can reconcile state to B while failing to enqueue `ReplaceEndpoints(B)`.

Then change the implementation and make the same test prove reliable delivery.

### Step 2: make replacement delivery reliable

Change only the successful-config-reload branch.

Preferred behavior:

- derive the replacement endpoint vector;
- deliver `ReplaceEndpoints` through an awaited bounded send or equivalently reliable latest-value mechanism;
- ensure a full queue causes backpressure rather than loss;
- handle a closed scheduler channel explicitly;
- preserve immediate polling semantics once the scheduler receives the command.

Avoid unbounded channels. The existing bounded channel is desirable; the bug is silent dropping, not boundedness itself.

### Step 3: verify repeated replacement ordering

Add the A -> B -> C test and confirm the scheduler-control path eventually converges to C without silently losing the final replacement.

Do not test thousands of commands. Two sequential replacements under capacity pressure are sufficient.

### Step 4: run focused regression coverage

At minimum:

```bash
cargo fmt --all -- --check
cargo test -p gregg main
cargo test -p gregg scheduler
cargo test -p gregg state
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

If module filters do not map exactly, run the nearest focused package tests and record the actual commands.

No new CI work is required.

### Step 5: optional narrow local manual smoke

A second external-host smoke is not required to prove channel-pressure semantics; deterministic bounded-channel tests are stronger for this specific defect.

If the current Ubuntu environment still has a reachable daemon and a quick smoke is convenient, verify only:

1. run Gregg with a temporary config;
2. edit the stable-ID endpoint;
3. press `Ctrl-R`;
4. confirm the new endpoint is polled and becomes online when reachable.

Do not make Plan 079 completion depend on which of `.182` or `.183` happens to be reachable at that moment. The purpose of Plan 079 is command-delivery correctness, not another network-environment qualification.

### Step 6: correct the Plan 078 historical record

Revise the relevant paragraphs/checklist/verification record in Plan 078 to preserve both:

- the user's original `.183 working / .182 stale` report;
- the later `.182 reachable / .183 unavailable` closure environment.

Do not change source behavior based on this documentation correction.

### Step 7: close planning records directly

After code/tests pass:

1. set Plan 079 status to `complete`;
2. record the implementation SHA and concise local verification commands/results;
3. update Plan 078 with the bounded-delivery follow-up note and corrected environment wording;
4. update `plans/README.md` so 079 is complete and no active corrective work remains;
5. extend the dependency chain to `... -> 077 -> 078 -> 079`;
6. do not create Plan 080 solely for closure.

## Acceptance criteria

### Reliable replacement delivery

- [x] Successful Systems config reload cannot silently lose `ReplaceEndpoints` because the scheduler command channel is full.
- [x] Bounded channel capacity is retained; no unbounded command queue is introduced.
- [x] A full channel produces bounded backpressure or an equivalently reliable latest-replacement mechanism.
- [x] After a valid reload is committed, scheduler and `AppState` cannot remain permanently divergent because of a dropped replacement command.
- [x] A closed scheduler command receiver is handled explicitly rather than ignored.
- [x] Endpoint replacement still triggers an immediate poll once accepted by the scheduler.
- [x] Periodic polling after replacement uses the replacement endpoint set.
- [x] Existing host/port stale-result rejection remains intact.
- [x] Existing generation monotonicity remains intact.
- [x] Empty endpoint-list replacement remains supported.

### Bounded-pressure regression coverage

- [x] A production-path test fills the scheduler command channel and proves the replacement is not silently dropped.
- [x] The pressure test verifies final `AppState` and delivered scheduler endpoints agree.
- [x] A rapid A -> B -> C replacement test proves convergence to C under bounded capacity.
- [x] Existing normal replacement scheduler test remains green.
- [x] Existing invalid-config reload behavior remains green and last-known-good.
- [x] Tests use deterministic channel synchronization rather than arbitrary sleeps.

### Scope preservation

- [x] No watcher/config-monitor subsystem is added.
- [x] No new dependency is added.
- [x] No scheduler architecture rewrite or generic prioritized queue is added.
- [x] URL-form `gregg add` behavior remains unchanged and green.
- [x] `greggd configprint` behavior remains unchanged and green.
- [x] `greggd croncheck` and daemon runtime ownership remain unchanged.
- [x] EggPool command behavior remains unchanged.
- [x] No CI workflow/job/matrix/artifact changes are introduced.

### Plan 078 record correction

- [x] Plan 078 explicitly preserves the originating report that `.183` was the verified working endpoint and `.182` was the stale/wrong displayed endpoint.
- [x] Plan 078 separately records that `.182` was reachable and `.183` unavailable during the later closure smoke.
- [x] The record no longer rewrites the later environment as though it were the original observation.
- [x] Plan 078 retains the valid reverse-direction live smoke as evidence of address-replacement behavior.
- [x] Plan 078 notes that Plan 079 corrected the bounded scheduler-command delivery edge found during post-implementation review.

### Verification and closure

- [x] `cargo fmt --all -- --check` passes.
- [x] Focused `gregg` main/scheduler/state tests pass.
- [x] `cargo test -p gregg --bin gregg` passes.
- [x] `./scripts/check-local.sh` passes.
- [x] Plan 079 records final implementation SHA and verification results.
- [x] `plans/README.md` records Plan 079 complete with no remaining active corrective phase.
- [x] No Plan 080 is created solely to close this work.

## Implementation and verification record

Implementation commit `49c4c7d` makes successful Systems reloads await the
existing bounded scheduler sender before reconciling `AppState`, propagates a
closed scheduler channel through the TUI error boundary, and adds deterministic
capacity-pressure, A -> B -> C ordering, and closed-channel tests. It also
updates the user, architecture, agent, and skill guidance and corrects Plan
078's historical endpoint wording. Local verification passed with:

- `cargo fmt --all -- --check`;
- `cargo test -p gregg --bin gregg` (392 passed, 1 ignored);
- `cargo test -p gregg scheduler` (23 passed) and `cargo test -p gregg state` (38 passed);
- `./scripts/check-local.sh` (646 tests, 1 ignored);
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `RUSTFLAGS=-Dwarnings cargo test --workspace --all-targets --all-features`;
- `cargo +1.75 check --workspace --all-features`.

The `cargo test -p gregg main` filter matched no tests in this binary crate;
the full `--bin gregg` run is the applicable main/dispatch coverage. The clean
tree `./scripts/check-local.sh --release` preflight also passed, including
clippy, documentation, package lists, installed-daemon loopback smoke, and the
protocol publish dry-run. The resulting remote CI run is recorded below.

The push-triggered GitHub Actions run `31533109605` passed all Linux, macOS
(Apple Silicon and Intel), Windows, and Rust 1.75 MSRV jobs, including the
Windows SCM lifecycle smoke. It emitted only the repository's existing Node.js
20 deprecation annotation for GitHub actions and had no failed checks.

## Handoff notes

Do not solve this by making the scheduler channel unbounded. The current bounded command channel is appropriate for a small local monitor; only the endpoint-replacement command's loss semantics are wrong.

Do not weaken the Plan 078 stale-result host/port check. That check is still necessary even after replacement delivery becomes reliable because an old generation may complete after a host/port edit.

The target invariant is simple:

```text
state says endpoint B
    => scheduler replacement B is guaranteed to be queued/accepted
    => scheduler eventually polls B
```

The planning-record correction is historical accuracy, not a request to rerun network testing until one particular private-LAN address is reachable.
