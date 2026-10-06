# Plan 178: Gregg cron worker reload preemption corrective

Status: complete. See the closure record at the end.

Depends on: completed Plan 174 plus current main at
`892a77195b0a91c7d4e9fb7ee02d560e7ae64c83`.
Independent of Plan 091 and Plans 176-177.

Opened from the 2026-10-05 post-Plan-174 review.

## Objective

Bound cron-observation reaction time to an accepted config reload independently
of fleet size and old-endpoint request timeouts.

A reload that adds/removes/repoints systems should stop spending the rest of a
fleet round on the superseded endpoint snapshot and begin observing the current
snapshot promptly, while preserving Plan 174's target-bound observations and
epoch/revision history coherence.

## Current behavior

The worker loop snapshots the endpoint list, completes the entire bounded
concurrency round, and only then waits on:

- cancellation;
- the 30-second interval tick; or
- `cron_reload.notified()`.

`Notify` retains a reload permit, so the reload is not *lost*, and the engine
already rejects late observations whose host/port no longer matches the current
configuration. State correctness is therefore preserved.

But the reload cannot preempt the active round.

The concurrency window is four and the configured request timeout may be as
high as 60 seconds. With a large fleet of slow/unreachable endpoints, an
accepted reload can wait through many four-endpoint timeout waves before the
worker notices the stored reload permit. That makes the documentation's
"reload wakes the worker immediately" claim materially false and can leave a
new/repointed target unobserved for minutes.

This is a responsiveness/liveness corrective, not a cache-coherence redesign.

## Required behavior

Make reload and cancellation observable **while a cron round is active**.

When reload wins:

1. stop enqueueing work from the old endpoint snapshot;
2. cancel/drop any in-flight HTTP futures from that snapshot;
3. return to the outer loop;
4. snapshot the current endpoint list;
5. prune history-gate keys absent from that new normalized target set;
6. begin a new bounded round immediately, without waiting for the 30-second
   cadence.

Cancellation must likewise end the worker promptly rather than waiting for a
remote request timeout. The daemon currently aborts the worker task during
teardown, so this change is primarily about making the worker's own contract
truthful and testable, not replacing the outer abort safety net.

## Gate-commit invariant

Preemption must not create a new history-loss race.

Today `settle` both:

- constructs the observation; and
- advances `HistoryGate` for a coherent history response.

A reload-aware round must never advance the gate for a history document that is
then dropped before it reaches the engine. Otherwise an equivalent/unchanged
target could suppress the next history fetch even though its cache never
received the prior document.

Refactor the boundary so coherent fetch settlement can produce a **pending gate
commit** separately from the observation. Commit that target's
`(epoch, history_revision)` only after the observation has been accepted by the
bounded worker-to-engine channel.

Equivalent designs are acceptable if they prove the same invariant:
"gate says fetched" implies that coherent observation was handed to the engine.

For a true endpoint repoint, an already-delivered old-target observation may
still race with reload; Plan 174's fleet-side host/port check remains the final
authority and the next round prunes the old target key.

## Round structure

Prefer one task with a bounded `FuturesUnordered` window, as today.

A suitable shape is:

- snapshot endpoints and a read-only gate view;
- seed at most `CRON_MAX_IN_FLIGHT` fetches;
- inside the active round, select among:
  - cancellation;
  - reload notification;
  - next completed endpoint fetch;
- settle/send one completed fetch, commit its gate update only after successful
  channel delivery, then refill one slot;
- on reload/cancel, drop the remaining fetch futures and return a typed round
  outcome to the outer loop.

Do not spawn one task per endpoint and do not increase the concurrency bound.

A cadence tick that becomes due during a long round does not need to start a
second overlapping round; `MissedTickBehavior::Delay` and one worker-owned
round remain the intended request budget.

## Deterministic regressions

Use controlled loopback servers and paused/injected time where appropriate.

Cover:

1. Four slow first-wave endpoints begin requests; a reload to a new target is
   issued; the old requests are dropped and the new target is requested without
   waiting for their request timeout.
2. A large old fleet does not make reload latency proportional to
   `ceil(fleet/4) * request_timeout`.
3. A removed/repointed target's late result is still rejected by the fleet
   reducer.
4. An equivalent normalized endpoint spelling does not cause unnecessary cache
   reset or history refetch.
5. A coherent history fetch completed just before reload but **not delivered**
   cannot advance the history gate.
6. A coherent observation successfully sent to the engine may advance the gate,
   and the next unchanged round suppresses history normally.
7. A repointed target with colliding epoch/revision still performs first history
   discovery.
8. Cancellation during a slow active round ends the worker without waiting for
   the configured request timeout.
9. Ordinary no-reload operation remains one startup round plus 30-second
   delayed cadence with at most four reads in flight.
10. History remains first-discovery/revision/epoch driven and is never fetched
    every summary cadence.

No test should sleep for a 30- or 60-second wall-clock duration.

## Documentation

Reconcile `architecture/gregg-client.md`, `crates/gregg/README.md`,
AGENTS.md, and the gregg-client skill so "reload wakes the worker immediately"
means active-round preemption, not merely a stored notification consumed after
the old fleet finishes.

Document the gate-commit rule together with the existing summary/history
coherence rule.

## Acceptance criteria

- [x] Accepted config reload can interrupt an active cron round and does not wait
      for the remaining old-fleet timeout waves.
- [x] In-flight requests from the superseded endpoint snapshot are dropped on
      reload; no unbounded task survives the round.
- [x] Cancellation interrupts an active round without waiting for request
      timeout.
- [x] `CRON_MAX_IN_FLIGHT` remains four and no new concurrency/config knob is
      added.
- [x] A history gate entry is committed only after its coherent observation has
      been handed to the engine.
- [x] Plan 174's endpoint identity rejection and epoch+revision coherence remain
      authoritative and green.
- [x] Startup/cadence/history-fetch request budgets remain unchanged in the
      no-reload case.
- [x] Focused cron/client-daemon tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [x] Active client-daemon documentation states the bounded reload/preemption
      semantics accurately.

## Stop conditions

Open a separate plan rather than broadening this one if:

- prompt reload requires changing the local IPC/frame protocol;
- the only design needs unbounded per-endpoint tasks;
- preserving gate correctness requires moving the cron cache out of the
  single-owner engine;
- reload preemption would create overlapping cron rounds or increase the remote
  request budget beyond the existing fixed window.

## Preserved exclusions

- greggd scheduler execution/output (Plan 177);
- TUI rendering;
- scheduler remote protocol/schema;
- persistent history;
- new polling configuration fields;
- general metrics scheduler redesign;
- EggPool behavior;
- Plan 091 soak evidence;
- new workflows/jobs/matrices.

## Closure record

Responsiveness and gate-transactionality corrective in
`crates/gregg/src/clientd/cron.rs`. No stop condition was hit: the local IPC frame
protocol is unchanged, no per-endpoint task is spawned, the cron cache stayed in
the single-owner engine, and rounds never overlap or exceed the existing window.
No new configuration field, dependency, workflow, or job.

### Reload now preempts the round it lands in

`CronWorker::round` takes the reload notification and the cancellation token, and
selects among them *inside* the active round:

```rust
let finished = tokio::select! {
    biased;
    () = cancel.cancelled() => return RoundOutcome::Cancelled,
    () = reload.notified() => return RoundOutcome::Reloaded,
    Some(finished) = in_flight.next() => finished,
};
```

The round returns a typed `RoundOutcome`. `Reloaded` is handled by `continue` in
the outer loop, which re-reads the shared endpoint list, prunes history-gate keys
absent from the new normalized target set via the existing `forget_absent`, and
starts a fresh bounded round immediately — it does **not** fall through to the
cadence wait. That distinction was the one real implementation trap: a first
version that treated `Reloaded` and `Completed` identically passed the round but
then parked on the next `select!`, so the reload test failed until the arms were
split.

Dropping the round drops its `FuturesUnordered`, and those futures *are* the
requests, so in-flight HTTP is cancelled with no task to leak and nothing to
abort. The round stays one task with one `FuturesUnordered`, `CRON_MAX_IN_FLIGHT`
is still 4, and `MissedTickBehavior::Delay` still prevents an overlapping round, so
the no-reload request budget is byte-for-byte unchanged.

`biased` is deliberate: a stored reload permit must win over an endpoint result
that happens to be ready in the same poll.

### The history gate commits only after delivery

`settle` no longer mutates the gate. It returns `(CronObservation,
Option<PendingGateCommit>)`, where the commit carries the target's `(epoch,
history_revision)` for the coherent document it produced. The round applies it
only after `updates.send(observation).await` succeeds:

```rust
let (observation, commit) = settle(finished);
if updates.send(observation).await.is_err() {
    return RoundOutcome::EngineGone;
}
if let Some(commit) = commit {
    gate.commit(commit);
}
```

That closes the history-loss race: previously a coherent document that was then
dropped — by a preempted round or a closed engine channel — still advanced the
gate, so an equivalent or unchanged target could suppress every later history
fetch while no cache ever received a byte. The invariant is now simply **"the gate
says fetched" implies the coherent document reached the engine.**

`HistoryGate::record` was replaced by `commit`, which takes a
`PendingGateCommit` rather than a document, so the only way to advance the gate is
to have produced one. `observe()` commits inline: returning the observation to its
caller *is* the delivery, and that path has no channel hand-off.

Coherence itself is unchanged — a straddling `(epoch, revision)` pair still is
never merged, never produces a commit, and is still reported as a scheduler-scoped
`Incoherent` diagnostic with the summary kept.

### Regressions

Five new deterministic tests, none of which sleeps for a 30- or 60-second
duration:

- `a_reload_interrupts_an_active_round_before_its_requests_time_out` — exactly
  one full window of stalled endpoints (60 s, far beyond any bound the test waits
  on), then a fleet replacement: the new target is observed within 10 s and read
  exactly once.
- `reload_latency_does_not_scale_with_the_superseded_fleet_size` — twelve stalled
  endpoints, three timeout waves at a bound of four; the replacement target still
  lands inside 5 s, and the meter's peak stays at or below `CRON_MAX_IN_FLIGHT`.
- `cancellation_interrupts_an_active_round_without_a_request_timeout` — the worker
  task ends while every request is still stalled.
- `an_undelivered_history_document_never_advances_the_gate` — the engine channel
  is dropped, so the round returns `EngineGone` after a history fetch that really
  happened (`history_hits() == 1`); the gate still asks for history.
- `a_delivered_history_document_advances_the_gate_and_suppresses_the_next_fetch` —
  the delivered observation advances the gate, and the next unchanged round
  suppresses the history body (`history_hits() == 1`, `summary_hits() == 2`).

Both new invariants were mutation-tested. Restoring the old
`in_flight.next().await` loop fails the reload and cancellation tests; moving the
gate commit back ahead of the send fails the undelivered-history test.

Plan 174's behaviors remain authoritative and green, covering the plan's
regressions 3, 4, 7, and 10: `a_repointed_target_whose_numbers_collide_still_performs_first_discovery`,
`a_target_key_distinguishes_a_repoint_from_an_equivalent_spelling`,
`a_history_from_another_epoch_is_rejected_rather_than_merged`,
`a_history_at_a_different_revision_is_rejected_the_same_way`,
`an_unchanged_revision_suppresses_the_history_body_entirely`, and
`a_revision_change_fetches_history_exactly_once`. Regression 9 is
`startup_runs_exactly_one_round_before_the_first_period` plus
`the_in_flight_request_count_never_exceeds_the_cron_bound`.

### Verification

29 `clientd::cron` tests pass (24 existing, 5 new),
`cargo fmt --all -- --check`, workspace Clippy with `-D warnings`, workspace
tests, and `./scripts/check-local.sh` are green.

Native Windows CI run `37409557600` on `3b2a94a` is green across all six existing
jobs, including the Windows Test step that exercises these tests on the platform
where the previous cron rounds depended most on request-timeout behaviour. The
failure recorded in Plan 177's record on the previous SHA was a `-D dead-code`
gate on a test-only predicate and not a cron or reload defect.

Documentation reconciled: `architecture/gregg-client.md` gains a dedicated
"A reload preempts the round it lands in" section and states the gate-commit rule
beside the existing coherence rule; `crates/gregg/README.md`,
`.opencode/skills/gregg-client/SKILL.md` (rules 8 and 9), and `AGENTS.md` now say
that "reload wakes the worker immediately" means active-round preemption rather
than a stored notification consumed after the old fleet finishes.


## Follow-up correction note (2026-10-06, Plan 180)

A later review found one remaining backpressure gap after this plan's active HTTP
round preemption. Once an endpoint fetch settles, the worker still awaits the
bounded `updates.send(observation)` directly. If the 64-slot worker-to-engine
channel is full, that await stops polling both reload and cancellation until the
engine frees capacity. The pending history-gate commit is correctly withheld
while blocked, so the transactional gate fix remains valid.

Plan 180 owns the narrow correction: make the observation hand-off itself
selectable by reload/cancellation, abandon superseded work without committing
its gate entry, and keep ordinary full-channel backpressure when no signal is
present. This plan's request preemption, target binding, coherence, four-read
bound, and steady cadence remain the settled baseline.

### Resolution (2026-10-06, after Plan 180 closed)

Plan 180 landed the correction this note anticipated. The bare
`updates.send(observation).await` is replaced by `deliver_observation`, which
selects cancellation, then reload, then the send; only its `Ok(())` reaches
`gate.commit(commit)`, so every non-delivery path still fails closed. This plan's
claim that reload and cancellation are selected inside an active round is now
true for the whole round rather than only while HTTP work is outstanding, and its
transactional gate invariant is unchanged. See
`180-gregg-cron-observation-delivery-preemption-corrective.md`.
