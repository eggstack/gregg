# Plan 151: EggPool desired-state delivery and worker-state contract corrective pass

Status: complete. See the closure record at the end of this file.

Depends on: completed EggPool summary baseline Plans 056-062, the Plan-070 async-state-machine review, and current main after the intentional August 25 command-delivery change in d31d72f. Independent of the remaining Plan 091 soak record and planned Plan 147.

## Objective

Correct the current Gregg EggPool worker-control contract without blocking terminal input.

Current main uses a bounded mpsc command channel and try_send. When the queue is full, the requested Activate, SetPeriod, Refresh, or Deactivate command is dropped and AppState is changed to EggpoolStatus::Busy. That behavior was intentionally introduced in d31d72f to avoid awaiting command capacity inside the input path, but it superseded the earlier Phase-61/62 convergence contract. A dropped state-changing command can leave reducer intent and worker intent different: the visible pane/period/generation may advance while the worker never sees the transition, and a dropped Deactivate can leave passive EggPool polling armed after the user returned to Systems.

Replace that lossy queue-pressure behavior with one nonblocking latest-desired-state handoff. Keep the worker bounded, single-endpoint, generation-aware, and request-relative. Do not reintroduce an await on channel capacity inside the input event arm.

This phase is transport/state correctness only. It does not add EggPool service-health data; Plan 152 owns the newer /api/status integration after this worker contract is stable.

## Historical contract reconciliation

The implementation history is intentional and must remain truthful:

- Plan 061 at 1b77da1 replaced ignored try_send failures with awaited bounded delivery.
- Plan 062 at 38d89bf added deterministic pressure/convergence tests and closed that contract.
- d31d72f later intentionally changed the design to try_send + Busy to guarantee the input path never waits on a slow worker.
- Current main and architecture guidance describe the d31d72f behavior accurately.
- The 2026-10-02 review found that the new nonblocking behavior trades away desired-state convergence and can violate the inactive-pane no-poll invariant.

Do not rewrite the older closure records as if they were false when written. Append a supersession note and make this plan the authority for current behavior.

## Governing invariants

1. EggPool control must never block terminal input or Systems poll-result handling on worker command capacity.
2. The worker must eventually converge to the latest reducer-owned desired state while its control receiver is alive.
3. Leaving the EggPool pane must disable passive EggPool work even under rapid input or worker pressure.
4. Entering the pane, period changes, and manual refreshes must still trigger the corresponding immediate request.
5. Generation ownership and stale-result rejection remain authoritative; passive refresh reuses the current generation.
6. At most one EggPool request is in flight. A superseding desired state aborts obsolete work.
7. Passive refresh remains request-relative and fixed at 60 seconds while active.
8. No EggPool worker, control channel, timer, or request exists when EggPool is not configured.
9. Cancellation must abort in-flight work promptly. CancellationToken remains the shutdown authority.
10. Summary transport, authentication, periods, body cap, metric semantics, and rendering remain unchanged in this phase.
11. No new dependency, generic actor framework, command bus, retry system, or configurable cadence is introduced.

## Workstream A: replace lossy commands with latest desired state

Preferred design: replace the command mpsc with Tokio watch carrying one small desired-state value:

~~~text
EggpoolDesiredState
    active: bool
    period: EggpoolPeriod
    generation: u64
~~~

The existing reducer generation is already a refresh nonce:

- pane activation calls begin_eggpool_request and increments generation;
- manual refresh calls begin_eggpool_request and increments generation;
- a period change increments generation;
- passive refresh does not increment generation;
- deactivation changes active to false without fabricating a new request generation.

A watch sender is synchronous and capacity-free. Publishing the latest state therefore cannot stall on queue capacity and cannot silently discard a final desired state because an mpsc slot is occupied.

Use the smallest Tokio watch API that still reports a closed receiver. If a send cannot reach a live worker, mark the local worker unavailable. Do not keep Busy as a substitute for failed convergence.

If implementation proves that a one-slot latest-state primitive using existing types is materially smaller than watch, it is acceptable only if it has the same properties: nonblocking publication, retained latest value, closed-worker detection, and no unbounded queue. Do not return to direct send().await from the input arm and do not retain drop-on-full semantics.

### Workstream A acceptance criteria

- No command publication waits for bounded mpsc capacity.
- No state-changing EggPool transition is discarded because a queue is full.
- The latest desired active/period/generation state remains observable by the worker until superseded.
- Closed worker control is surfaced as WorkerUnavailable.
- Production control state remains one endpoint-specific primitive, not a generalized channel abstraction.

## Workstream B: make the worker consume desired state deterministically

Refactor spawn_worker / spawn_worker_with_clock around the desired-state receiver.

Required transition rules:

Inactive -> active:
- start an immediate request for desired period/generation;
- establish a fresh request-relative passive deadline only after completion.

Active -> inactive:
- abort any in-flight request;
- clear the passive deadline;
- emit no synthetic success/failure result solely for deactivation.

Active state changes period or generation:
- abort obsolete in-flight work;
- start exactly one immediate request for the newest period/generation;
- discard/coalesce intermediate desired states that the worker did not observe individually, because only the newest state is authoritative.

Passive deadline:
- start one request only when still active;
- use the current desired period and generation;
- never increment generation internally.

Cancellation:
- abort the request and terminate promptly;
- do not depend on a queued Shutdown command.

The worker may observe several watch changes as one latest value. That is correct: the contract is convergence to current desired state, not durable replay of every keypress.

### Workstream B acceptance criteria

- Rapid Activate -> period changes -> Deactivate converges inactive with no passive request left armed.
- Rapid period changes converge on the final period and generation.
- A manual refresh with unchanged period is still observable because generation changes.
- Passive refresh uses the current generation and selected period.
- Obsolete in-flight requests are aborted before their results can mutate visible state.
- Cancellation remains prompt and bounded.

## Workstream C: separate local worker state naming from EggPool service health

Rename the reducer/UI enum from EggpoolStatus to a name that cannot be confused with EggPool's service-health status contract, preferably EggpoolWorkerState.

The local state should describe only Gregg machinery, for example:

~~~text
Idle
Refreshing
WorkerUnavailable
~~~

Remove Busy once lossy queue pressure no longer exists.

Do not add ready/degraded/unready to this enum. Those are EggPool service-health facts and belong to the separate Plan-152 model sourced from /api/status.

Update code comments/tests so:

- Refreshing means Gregg has published a current desired request and is awaiting its result;
- WorkerUnavailable means Gregg's local worker/control path is gone;
- neither variant claims that the EggPool proxy or its providers are healthy.

### Workstream C acceptance criteria

- No type named EggpoolStatus ambiguously mixes local worker lifecycle with remote service health.
- Busy is absent from production state and rendering.
- Existing summary errors remain EggpoolFetchOutcome values and are not folded into worker lifecycle.
- The rename is internal/client-only; no config or external wire contract changes.

## Workstream D: deterministic pressure and convergence tests

Replace the current test full_command_channel_does_not_block_dispatch_and_marks_busy with tests for the corrected contract.

Required tests:

1. Rapid desired-state publication never waits for worker capacity.
2. A worker deliberately held in an in-flight request sees the final desired period/generation after release or supersession.
3. Activate -> Deactivate under pressure produces no later passive request after at least 120 seconds of paused Tokio time.
4. Multiple period changes converge to the final API period and generation without requiring every intermediate request to execute.
5. Manual refresh at the same period triggers a new generation/request.
6. Closed desired-state receiver marks WorkerUnavailable.
7. Existing passive-generation, request-relative cadence, stale-result, cancellation, panic-recovery, and no-config tests remain green.
8. Systems polling remains responsive while EggPool desired state changes rapidly.

Use loopback synthetic HTTP and paused Tokio time. Do not add production-duration sleeps or a process-level terminal harness.

## Workstream E: planning and active guidance reconciliation

During implementation/closure:

- append, do not erase, supersession notes in Plans 056, 061, and 062;
- make the Plan-056 phase map and Plan-062 status wording internally consistent;
- update plans/README.md so the original 056-062 roadmap remains a completed historical baseline and Plan 151 is its post-closure command-convergence correction;
- update architecture/gregg-client.md and .opencode/skills/eggpool/SKILL.md from try_send/Busy to the retained latest-state contract;
- update AGENTS.md only if its compact invariants need a new EggPool control rule;
- add a CHANGELOG.md entry because the visible worker-busy state is removed and pressure behavior changes;
- do not falsify the historical d31d72f rationale.

## Expected implementation surface

Likely files:

~~~text
crates/gregg/src/eggpool.rs
crates/gregg/src/state.rs
crates/gregg/src/main.rs
crates/gregg/src/ui/eggpool.rs
architecture/gregg-client.md
.opencode/skills/eggpool/SKILL.md
CHANGELOG.md
plans/056-eggpool-summary-pane-roadmap.md
plans/061-eggpool-refresh-correctness-and-closure.md
plans/062-eggpool-worker-regression-coverage-and-closure-polish.md
plans/151-eggpool-desired-state-delivery-and-worker-state-corrective-pass.md
plans/README.md
~~~

No change is expected in greggd, gregg-protocol, gregg-host, EggPool, config schema, packaging, workflows, or release scripts.

## Verification

Focused:

~~~text
cargo test -p gregg --all-targets --all-features -- eggpool
cargo test -p gregg --all-targets --all-features -- main::tests
cargo test -p gregg --all-targets --all-features -- state::tests
~~~

Then:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use the existing ordinary CI workflow for hosted cross-platform closure. Add no new job.

## Explicit acceptance criteria

- [x] EggPool control publication cannot block on worker queue capacity.
- [x] No activation, period change, manual refresh, or deactivation intent is lost because a queue is full.
- [x] Rapid input converges to the latest active/period/generation state.
- [x] Leaving EggPool always suppresses future passive requests after current work is aborted.
- [x] Manual refresh remains observable even when period is unchanged.
- [x] Passive refresh preserves reducer generation ownership.
- [x] Only one request is in flight and superseded work is aborted.
- [x] Cancellation terminates promptly without a queued Shutdown dependency.
- [x] `EggpoolStatus` is renamed to the explicitly local `EggpoolWorkerState` type.
- [x] `Busy` is removed from production state, rendering, docs, and tests.
- [x] Summary API/auth/body-limit/period/metric behavior is unchanged.
- [x] Systems polling and input responsiveness remain intact.
- [x] Historical Plans 056/061/062 record both their original closure and the later `d31d72f` supersession truthfully, and Plan 151's contract.
- [x] Focused tests and local checks pass (ordinary CI is recorded in the closure record).
- [x] No new dependency, workflow, generalized actor/channel framework, retry system, or config option is added.

## Stop conditions

Stop and split follow-up work if correction requires:

- changing EggPool's API or database;
- adding service-health fields or /api/status consumption (Plan 152 owns that);
- changing summary metric semantics or periods;
- multiple EggPool endpoints;
- generic datasource/worker infrastructure;
- unbounded queues;
- blocking terminal input on worker capacity;
- new CI/release infrastructure.

## Closure record

Implementation: `4f9debe` (`fix: converge EggPool worker on one nonblocking
desired state`).

### Workstream A: one nonblocking latest desired state

- Added `EggpoolDesiredState { active, period, generation }` and
  `EggpoolControl`, a `tokio::sync::watch` sender wrapper whose `publish`
  is synchronous and capacity-free. It reports a closed worker through
  `EggpoolWorkerClosed` and exposes the retained value through `published`
  for observation, so there is no second control path.
- `EggpoolCommand`, its bounded `mpsc` channel, `try_send`, and the `Shutdown`
  command are gone. `AppState::eggpool_desired_state()` derives `active` from
  the visible pane, so the pane itself is the deactivation authority.
- The event loop now publishes whenever the reducer-owned desired state
  changes, replacing the per-transition command branches.

### Workstream B: deterministic convergence in the worker

- `spawn_worker`/`spawn_worker_with_clock` hold one `EggpoolWorkerState`
  (newest desired state, at most one in-flight request, passive deadline).
  `converge` returns whether an immediate request is required: inactive aborts
  in-flight work and clears the deadline without emitting a synthetic result,
  while a new active state, changed period, or changed generation aborts
  obsolete work and starts exactly one request.
- The passive-deadline arm adopts the newest published state before deciding,
  so a pending deactivation/period/manual change always wins over a passive
  deadline and a passive refresh can never fetch superseded intent. It uses the
  current desired period and generation and never increments the generation.
- Cancellation aborts the request and terminates the worker; dropping the
  control publisher also terminates it, so no queued shutdown command is
  required.

### Workstream C: local worker-state naming

- `EggpoolStatus` became `EggpoolWorkerState` with `Idle`, `Refreshing`, and
  `WorkerUnavailable`; the `EggpoolState::status` field became `worker_state`.
  `Busy` and `mark_eggpool_busy` were deleted along with the "worker busy"
  pane text. `EggpoolFetchOutcome` remains the summary-transport classification
  and is unchanged. No config or wire surface changed.

### Workstream D: deterministic tests

- New worker coverage: `rapid_publication_never_waits_for_worker_capacity`
  (a 9,998-publication burst completes without yielding and converges on the
  newest generation), `worker_held_in_flight_converges_on_the_final_desired_state`,
  `rapid_period_changes_converge_on_the_final_period_and_generation`,
  `activation_then_deactivation_under_pressure_arms_no_passive_refresh` (no
  request or result for at least 120 seconds of paused time),
  `closed_control_channel_reports_a_missing_worker`, and
  `unconfigured_state_has_no_worker_control_or_request`.
- Existing cadence/generation/cancellation/panic-recovery tests were rewritten
  onto the new control surface; the manual-refresh case now asserts that an
  unchanged period still produces a new generation and request.
- The removed `full_command_channel_does_not_block_dispatch_and_marks_busy` was
  replaced by main-event-loop tests: `rapid_eggpool_state_changes_never_block_systems_dispatch`
  (2,000 synchronous pane/period transitions, then a Systems refresh that is
  still delivered), `closed_eggpool_control_channel_marks_worker_unavailable`,
  `clamped_eggpool_period_does_not_change_desired_state`, and
  `pane_and_refresh_desired_state_are_scoped_to_active_pane`.
- The loopback summary server now serves keep-alive connections concurrently and
  can hold responses open on demand, which is what makes abort-then-refetch
  convergence observable. Positive waits are bounded by a real-clock watchdog
  thread rather than a Tokio timer: in a paused-time test an armed timer lets
  virtual clock auto-advance race the loopback round trip and produce a
  spurious failure that does not exist in real time. No production-duration
  sleep and no process-level terminal harness was added.

### Workstream E: reconciliation

- `plans/056-...md`: phase-map rows 61/62 and the dependency note now describe
  Phase 61/62 as the original awaited-delivery baseline, `d31d72f` as the
  intentional later change, and Plan 151 as the current correction.
- `plans/061-...md`: unchanged; its existing supersession note already records
  the 2026-10-02 review and Plan 151 ownership.
- `plans/062-...md`: status line now marks the phase a historical record whose
  pressure contract was superseded, consistent with its supersession note.
- `architecture/gregg-client.md`, `.opencode/skills/eggpool/SKILL.md`, and one
  `AGENTS.md` invariant now describe the retained latest-state contract; the
  Plan-070 rejection note is reconciled rather than deleted.
- `CHANGELOG.md` records the delivery change and the removal of the visible
  `Busy` state.

### Verification

- `cargo test -p gregg --all-targets --all-features -- eggpool` (47 tests) and
  the client bin suite (9 tests) pass.
- `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
  `cargo test --workspace --all-targets --all-features` all pass.
- `./scripts/check-local.sh` passed.
- Ordinary CI is recorded in `plans/README.md` once the workflow run for
  `4f9debe` completes; no new workflow or job was added.

### Scope reconciliation

Only `crates/gregg` plus active documentation and planning records changed.
`greggd`, `gregg-protocol`, `gregg-host`, `gregg-update`, EggPool, the config
schema, packaging, workflows, and release scripts are untouched, and no
dependency, generalized channel/actor framework, retry system, or config
option was added.

Future-plan impact: Plan 151 unblocks Plan 152, which is the only plan that
depended on this worker contract. Plans 091 and 147 are independent of it and
keep their statuses.
