# Phase 070: bounded client asynchronous simplification

Status: complete; no change retained.

Depends on: Plans 066 and 069. Execute after correctness phases have stable tests.

## Objective

Reduce client polling or EggPool worker machinery only where a small, behavior-preserving implementation is demonstrably simpler. This phase is not required to produce code changes. A documented no-change conclusion is the correct outcome when the alternatives increase code, weaken isolation, obscure ordering, or alter UI timing.

## Why this phase is conditional

The current client is functional and bounded. The review identified complexity, not a confirmed production failure:

- one Tokio task per system poll plus a semaphore and ordered handle joins;
- bounded EggPool command/result queues with awaited sends;
- explicit generation, cancellation, and cadence state.

These mechanisms may be more elaborate than necessary for a small local fleet, but simplification must preserve the exact operational contract. Do not trade tested behavior for fewer visible lines without measuring the full diff and test burden.

## Scope

### In scope

- Inspect and document the existing scheduler and EggPool state machines.
- Establish focused behavioral tests before changing either subsystem.
- Independently evaluate one smaller implementation for each subsystem.
- Retain a rewrite only if production code and concepts are reduced without increasing test scaffolding or changing behavior.
- Delete obsolete helpers and comments when a rewrite is retained.

### Out of scope

- New refresh settings, retry/backoff, per-endpoint cadence, or incremental UI streaming.
- Multiple EggPool endpoints, new periods, new metrics, or passive background polling while its pane is inactive.
- Changing the fleet-wide batch contract.
- Changing v2-first/404-only-v1 fallback.
- A generic scheduler, actor framework, task supervisor, command bus, or reusable worker crate.
- New dependencies.
- Performance benchmarking infrastructure.

## Decision rule for GPT-5.6 Luna

Evaluate scheduler and EggPool separately. For each, produce a small before/after table in the implementation handoff:

```text
production lines changed
number of tasks spawned
number and type of channels
ordering semantics
cancellation semantics
new helper types
focused tests changed
```

Retain the candidate only when all are true:

1. Production code is materially smaller or removes a synchronization primitive.
2. No new abstraction layer or dependency is added.
3. Existing public/config/UI behavior is unchanged.
4. Focused deterministic tests remain at least as strong.
5. Cancellation and shutdown remain bounded.
6. The candidate does not increase release binary size in Phase 071 measurement.

If any condition fails, revert that candidate completely and record `no change retained`.

## Workstream A: poll scheduler

### Current contract to preserve

- First generation starts immediately.
- Periodic generations use the configured fixed cadence and skip missed ticks.
- Manual refresh starts one generation without permanently resetting periodic cadence.
- Generations do not overlap.
- Poll concurrency never exceeds `max_concurrent_requests`.
- Every configured endpoint produces exactly one result in each batch.
- Results retain stable endpoint identity and deterministic state application.
- Receiver drop or cancellation terminates the scheduler.
- v2-first polling and 404-only v1 fallback remain inside `HttpClient`.

### Required tests before editing

Ensure active tests cover:

```text
immediate generation 1
increasing generation numbers
no generation overlap
bounded poll concurrency
manual refresh behavior
periodic cadence after manual refresh
closed refresh channel does not busy-loop
cancellation closes output
receiver drop stops the scheduler
one result per endpoint
```

Fix missing annotations under Plan 069 before using the suite as evidence.

### Candidate evaluation

The preferred candidate is a direct bounded future stream rather than explicit per-endpoint tasks plus a semaphore, for example `stream::iter(...).buffered(max_concurrent)` or an equivalently small standard Tokio/Futures pattern already available in the dependency graph.

Constraints:

- Preserve deterministic endpoint-order output, or explicitly reorder by configured endpoint ID before constructing the batch.
- Do not add panic-catching machinery merely to reproduce synthetic `Cancelled` results unless a production poll path can realistically panic. If removing task isolation would allow a credible endpoint-specific panic to terminate the scheduler, reject the candidate.
- Do not use `JoinSet` if it leaves the same task/semaphore/channel complexity with different names.
- Do not split the batch into per-result UI updates.

### Scheduler acceptance criteria

- [ ] Existing scheduler behavior tests pass unchanged or become stronger.
- [ ] No endpoint is omitted from a completed batch.
- [ ] Concurrency remains bounded.
- [ ] Batch and cadence semantics remain unchanged.
- [ ] The retained implementation removes meaningful task/semaphore machinery and is smaller; otherwise the current scheduler is retained.

## Workstream B: EggPool worker

### Current contract to preserve

- No worker exists when EggPool is not configured.
- Entering the EggPool pane triggers an immediate request.
- Leaving the pane disables periodic requests.
- Period changes supersede obsolete requests.
- Manual refresh affects only the active pane.
- Results carry generations and stale results are rejected.
- Automatic refresh remains request-relative at the existing fixed 60-second interval.
- API key values remain environment-referenced and sensitive.
- Shutdown/cancellation aborts in-flight work promptly.
- Systems polling remains responsive if EggPool is unavailable.

### Required tests before editing

Retain the Phase 062 tests for:

```text
generation ownership
request-relative cadence
bounded command pressure
deactivation
cancellation
worker channel closure
no-config behavior
stale-result rejection
```

Add a test only if needed to demonstrate a concrete UI stall or lost-final-state defect. Do not manufacture a failure by using unrealistic channel manipulation as the sole reason for a rewrite.

### Candidate evaluation

A `watch` channel or one-slot latest-state mechanism may be evaluated because EggPool commands describe current desired state rather than a durable event log. A candidate state may contain:

```text
active
period
generation
refresh nonce
shutdown/cancellation remains external
```

Constraints:

- Manual refreshes must remain distinguishable even when period and active state do not change; a nonce or generation may be used directly.
- Do not remove generation checks.
- Do not poll while inactive.
- Do not create a generalized worker-state abstraction.
- Do not add retries, backoff, configurable intervals, or queues.
- If a watch-based implementation requires more state transitions or tests than the current bounded channel, reject it.

### EggPool acceptance criteria

- [ ] Pane activation, deactivation, period changes, refresh, cancellation, and stale-result behavior remain identical.
- [ ] Systems-pane input and polling cannot be blocked by EggPool command/result backpressure.
- [ ] The retained implementation uses fewer queue/backpressure concepts and is smaller; otherwise the current worker is retained.
- [ ] No EggPool feature, endpoint, metric, retry, or configuration is added.

## Verification

Run focused tests after each independent candidate:

```bash
cargo test -p gregg scheduler
cargo test -p gregg eggpool
cargo test -p gregg main
./scripts/check-local.sh
```

Do not run release-size comparisons in this phase; Phase 071 measures the final retained code. Do not add stress-test workflows or permanent benchmarks.

## Phase acceptance criteria

- [ ] Scheduler and EggPool were evaluated independently against explicit contracts.
- [ ] Any retained change materially reduces production machinery and introduces no dependency or framework.
- [ ] Any rejected candidate is fully reverted rather than left partially implemented.
- [ ] Fleet batch, cadence, fallback, pane, generation, and cancellation behavior are unchanged.
- [ ] Focused tests and the default local check pass.
- [ ] A no-change outcome is recorded truthfully when simplification is not clearly beneficial.

## Handoff format

For each subsystem state either:

```text
retained: concise before/after complexity summary and test results
```

or:

```text
no change retained: candidate and exact rejection reason
```

Do not create a benchmark report or evidence file.

## Completion

No scheduler or EggPool rewrite was retained: candidates did not reduce
production machinery while preserving isolation, ordering, cadence, and
cancellation semantics.
