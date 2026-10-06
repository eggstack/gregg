# Plan 180: Gregg cron observation delivery preemption corrective

Status: planned.

Depends on: completed Plan 178 plus current main at
`fcbe2dfb129e0327dc0499d6666e54e1ac310fa8`.
Independent of Plan 091 and Plans 179/181.

Opened from the 2026-10-06 review of Plan 178's landed active-round reload
preemption.

## Objective

Make reload and cancellation preempt the **entire** cron round, including a
finished observation that is blocked on the bounded worker-to-engine channel.

Plan 178 made HTTP fetches preemptible and made history-gate advancement
transactional with successful delivery. This corrective closes the remaining
backpressure gap at the channel hand-off.

## Confirmed defect

The active round now selects among cancellation, reload, and
`in_flight.next()` while HTTP work is outstanding. Once one fetch finishes,
however, it executes:

~~~rust
let (observation, commit) = settle(finished);
if updates.send(observation).await.is_err() {
    return RoundOutcome::EngineGone;
}
if let Some(commit) = commit {
    gate.commit(commit);
}
~~~

`updates` is a bounded mpsc channel with `CRON_CHANNEL_CAPACITY = 64`.

If the single-owner engine temporarily stops draining that channel and all 64
slots are occupied, the cron worker can park inside `send().await`. While
parked there it no longer polls either:

- `cancel.cancelled()`; or
- `reload.notified()`.

Thus Plan 178's statement that reload/cancellation are selected "inside an
active round" is incomplete under worker-to-engine backpressure. A config reload
can still wait for receiver capacity rather than immediately abandoning the
superseded observation and beginning the new endpoint snapshot.

The history gate does **not** advance while blocked today, which is correct and
must remain true.

## Required correction

Make observation delivery itself preemptible.

Prefer one small private helper or an inline select with a typed outcome:

~~~text
deliver observation:
    biased select:
        cancellation -> Cancelled
        reload -> Reloaded
        updates.send(observation) ->
            Ok -> Delivered
            Err -> EngineGone
~~~

Cancellation and reload must precede the send in the biased ordering so a slot
becoming free at the same instant as a stored shutdown/reload signal cannot make
the old observation win.

Only `Delivered` may apply `PendingGateCommit`.

On `Reloaded`:

- abandon the completed observation;
- do not commit its history gate;
- drop the remaining old-snapshot fetch futures;
- return `RoundOutcome::Reloaded`;
- let the outer loop immediately re-snapshot endpoints and start the next round.

On `Cancelled`, return promptly without committing.

On channel closure, return `EngineGone` as today.

This is bounded loss of superseded work, not lossy steady-state delivery. With
no reload/cancellation, the worker must still apply backpressure and wait for a
receiver slot rather than drop observations.

## Gate invariant

Retain and strengthen Plan 178's invariant:

> `HistoryGate` may say an epoch/revision was fetched only if the coherent
> observation carrying that history was successfully handed to the engine.

Therefore:

- blocked send: no commit;
- reload wins: no commit;
- cancellation wins: no commit;
- receiver closed: no commit;
- successful send: commit immediately after send, before the round advances.

Do not move the gate into the engine or add acknowledgement traffic. The
bounded mpsc acceptance remains the hand-off boundary.

## Deterministic regressions

Do not try to fill the channel indirectly through a large live fleet. Fill the
real bounded channel deterministically or expose the smallest production helper
needed to test the hand-off.

Cover:

1. **Full channel + reload.**
   - prefill all `CRON_CHANNEL_CAPACITY` slots;
   - start delivery of one completed observation with a pending gate commit;
   - prove delivery is pending while no signal exists;
   - notify reload without freeing receiver capacity;
   - delivery returns `Reloaded`;
   - the extra observation is absent and the gate still needs that history.

2. **Full channel + cancellation.**
   - same precondition;
   - cancel without freeing capacity;
   - return `Cancelled` promptly;
   - no commit and no extra observation.

3. **Full channel + receiver capacity later.**
   - with no reload/cancellation, free exactly one slot;
   - the observation is delivered rather than dropped;
   - the pending gate commit is applied exactly once.

4. **Simultaneous signal/capacity race.**
   - when reload or cancellation is already ready as capacity becomes available,
     the signal wins by explicit branch ordering.

5. **Active-round integration.**
   - retain Plan 178's slow-request reload tests;
   - add one round-level test proving a blocked observation hand-off cannot keep
     the old snapshot alive after reload.

6. **No steady-state budget regression.**
   - `CRON_MAX_IN_FLIGHT` remains 4;
   - channel capacity remains 64;
   - one startup round and 30-second delayed cadence remain unchanged;
   - history remains revision/epoch driven rather than every-cadence.

Mutation-test the key regression by temporarily replacing the preemptible send
with bare `updates.send(...).await`; the full-channel reload/cancellation test
must then fail or remain pending under its deterministic bound.

## Shutdown/lifecycle semantics

The cron worker should be able to honor its own cancellation token even when the
engine is not draining observations. The daemon may still abort the task during
outer teardown as a final safety net, but correctness must not depend on task
abort rescuing a blocked channel send.

Do not make the observation channel unbounded and do not increase its capacity
to hide the issue.

## Documentation

Reconcile:

- `architecture/gregg-client.md`;
- `crates/gregg/README.md`;
- `.opencode/skills/gregg-client/SKILL.md`;
- `AGENTS.md`.

"Reload preempts the active round" must explicitly include both remote fetch
waits and a blocked observation hand-off. Keep the Plan-178 gate-commit rule
next to that statement.

## Acceptance criteria

- [ ] Reload preempts a cron worker blocked on a full observation channel
      without waiting for receiver capacity.
- [ ] Cancellation does the same.
- [ ] Superseded/cancelled/closed-channel observations never advance the history
      gate.
- [ ] A successfully accepted observation advances its pending gate commit
      exactly once.
- [ ] With no reload/cancellation, a full channel still provides bounded
      backpressure rather than lossy delivery.
- [ ] `CRON_CHANNEL_CAPACITY = 64` and `CRON_MAX_IN_FLIGHT = 4` remain
      unchanged.
- [ ] Existing target-identity, epoch/revision coherence, startup/cadence,
      repoint, and active-fetch-preemption tests remain green.
- [ ] Focused cron/client-daemon tests, workspace Clippy/tests, formatting, and
      `./scripts/check-local.sh` pass.
- [ ] Existing six-job CI completes green.

## Stop conditions

Open a separate design plan rather than broadening this one if the fix requires:

- engine acknowledgements or a new IPC protocol;
- moving history-gate ownership into fleet state;
- an unbounded observation queue;
- overlapping cron rounds or increased request concurrency; or
- changes to the remote scheduler protocol.

## Preserved exclusions

- greggd scheduler execution/output;
- cron TUI rendering;
- persistent history;
- new polling/configuration knobs;
- general metrics scheduler redesign;
- EggPool worker semantics;
- Plan 091 soak evidence;
- new CI workflows/jobs/matrices.
