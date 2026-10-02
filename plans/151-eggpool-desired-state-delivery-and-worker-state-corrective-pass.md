# Plan 151: EggPool desired-state delivery and worker-state contract corrective pass

Status: planned.

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

- [ ] EggPool control publication cannot block on worker queue capacity.
- [ ] No Activate, SetPeriod, Refresh, or Deactivate intent is lost because a queue is full.
- [ ] Rapid input converges to the latest active/period/generation state.
- [ ] Leaving EggPool always suppresses future passive requests after current work is aborted.
- [ ] Manual refresh remains observable even when period is unchanged.
- [ ] Passive refresh preserves reducer generation ownership.
- [ ] Only one request is in flight and superseded work is aborted.
- [ ] Cancellation terminates promptly without a queued Shutdown dependency.
- [ ] EggpoolStatus is renamed to an explicitly local worker-state type.
- [ ] Busy is removed from production state, rendering, docs, and tests.
- [ ] Summary API/auth/body-limit/period/metric behavior is unchanged.
- [ ] Systems polling and input responsiveness remain intact.
- [ ] Historical Plans 056/061/062 record both their original closure and the later d31d72f supersession truthfully.
- [ ] Focused tests, local checks, and ordinary CI pass.
- [ ] No new dependency, workflow, generalized actor/channel framework, retry system, or config option is added.

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

Not yet implemented.
