# Plan 178: Gregg cron worker reload preemption corrective

Status: planned.

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

- [ ] Accepted config reload can interrupt an active cron round and does not wait
      for the remaining old-fleet timeout waves.
- [ ] In-flight requests from the superseded endpoint snapshot are dropped on
      reload; no unbounded task survives the round.
- [ ] Cancellation interrupts an active round without waiting for request
      timeout.
- [ ] `CRON_MAX_IN_FLIGHT` remains four and no new concurrency/config knob is
      added.
- [ ] A history gate entry is committed only after its coherent observation has
      been handed to the engine.
- [ ] Plan 174's endpoint identity rejection and epoch+revision coherence remain
      authoritative and green.
- [ ] Startup/cadence/history-fetch request budgets remain unchanged in the
      no-reload case.
- [ ] Focused cron/client-daemon tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [ ] Active client-daemon documentation states the bounded reload/preemption
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
