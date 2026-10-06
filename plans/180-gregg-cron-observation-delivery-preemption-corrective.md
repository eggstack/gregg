# Plan 180: Gregg cron observation delivery preemption corrective

Status: complete. See the closure record at the end.

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

- [x] Reload preempts a cron worker blocked on a full observation channel
      without waiting for receiver capacity.
- [x] Cancellation does the same.
- [x] Superseded/cancelled/closed-channel observations never advance the history
      gate.
- [x] A successfully accepted observation advances its pending gate commit
      exactly once.
- [x] With no reload/cancellation, a full channel still provides bounded
      backpressure rather than lossy delivery.
- [x] `CRON_CHANNEL_CAPACITY = 64` and `CRON_MAX_IN_FLIGHT = 4` remain
      unchanged.
- [x] Existing target-identity, epoch/revision coherence, startup/cadence,
      repoint, and active-fetch-preemption tests remain green.
- [x] Focused cron/client-daemon tests, workspace Clippy/tests, formatting, and
      `./scripts/check-local.sh` pass.
- [x] Existing six-job CI completes green. Recorded after closure: run
      `37414987171` on `7853743` is green across all six existing jobs, including
      the native Windows Test step that runs these cron tests. No new workflow,
      job, or matrix was introduced. See the CI evidence note at the end of the
      closure record.

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

## Closure record

Hand-off preemption corrective in `crates/gregg/src/clientd/cron.rs`. No stop
condition was hit: no engine acknowledgement and no new IPC frame was added, the
history gate stays worker-private, the observation channel is still bounded at
`CRON_CHANNEL_CAPACITY = 64`, rounds never overlap, request concurrency is still
`CRON_MAX_IN_FLIGHT = 4`, and the remote scheduler protocol is untouched. No new
configuration field, dependency, workflow, job, or matrix.

### The blocked hand-off is now preemptible

Plan 178 made the HTTP fetches preemptible and made the gate transactional with
delivery, but the delivery itself still ran as a bare `updates.send(observation).await`
in the round body. `updates` is the bounded 64-slot worker-to-engine channel whose
only receiver is the single-owner engine, so that await is the one place a round
could park for an unbounded time — and while parked the round polled neither
`reload.notified()` nor `cancel.cancelled()`. Plan 178's claim that reload and
cancellation are selected "inside an active round" was therefore incomplete under
exactly the backpressure it exists for.

The fix is one small private helper beside `RoundOutcome`:

```rust
async fn deliver_observation(
    updates: &mpsc::Sender<CronObservation>,
    reload: &Notify,
    cancel: &CancellationToken,
    observation: CronObservation,
) -> Result<(), RoundOutcome> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(RoundOutcome::Cancelled),
        () = reload.notified() => Err(RoundOutcome::Reloaded),
        delivered = updates.send(observation) => match delivered {
            Ok(()) => Ok(()),
            Err(_) => Err(RoundOutcome::EngineGone),
        },
    }
}
```

and the round now reads:

```rust
let (observation, commit) = settle(finished);
if let Err(outcome) = deliver_observation(updates, reload, cancel, observation).await {
    return outcome;
}
if let Some(commit) = commit {
    gate.commit(commit);
}
```

Returning `Result<(), RoundOutcome>` rather than a new enum keeps the round's
existing typed-outcome vocabulary: `Ok(())` is the only path that reaches the gate
commit, and every other outcome is already a `RoundOutcome` the outer loop already
understands (`Reloaded` → `continue` and re-snapshot, `Cancelled`/`EngineGone` →
return). So `Reload` abandons the completed observation, drops the remaining
old-snapshot fetch futures as the round returns, and lets the outer loop
immediately re-read endpoints and start a fresh round.

**Ordering is the load-bearing detail.** Cancellation and reload precede the send
so a slot freeing at the same instant as a stored signal cannot let the old
observation — and its pending gate commit — win. `Cancelled` and `Reloaded` sit
above each other for the same reason a shutdown must outrank a wake.

`biased` also means the *engine* is never starved by a stored signal: the select
only takes a signal branch when that signal is genuinely ready, so ordinary
delivery is unchanged.

### Gate invariant preserved and strengthened

The Plan-178 transaction is now closed on **all five** paths rather than two:

| path | observation | gate |
| --- | --- | --- |
| blocked send, no signal | waiting | not advanced |
| reload wins the hand-off | abandoned | not advanced |
| cancellation wins the hand-off | abandoned | not advanced |
| receiver closed | undeliverable | not advanced |
| channel accepts it | delivered | committed, exactly once, immediately |

Every non-delivery path returns from `round` *before* the `gate.commit(commit)`
line, so an abandoned document can never claim "already fetched". The invariant is
unchanged and now unreachable-by-construction: **"the gate says fetched" implies
the coherent document reached the engine.** The gate was not moved into fleet
state and no acknowledgement traffic was added — the bounded mpsc acceptance
remains the hand-off boundary, which is also what makes the helper testable at
the primitive.

### Regressions

Six new tests, all driving the production primitive with the **real** bounded
channel — `CRON_CHANNEL_CAPACITY` was widened from a private daemon constant to
`pub(crate)` precisely so the precondition could be filled directly rather than
synthesized through a large live fleet. Cron tests went 29 → 37.

- `a_full_observation_channel_never_delays_a_reload` — 64 slots filled, delivery
  proven pending, reload notified with no capacity freed, delivery returns
  `Err(Reloaded)`, channel length unchanged.
- `a_full_observation_channel_never_delays_cancellation` — the mirror, returning
  `Err(Cancelled)`.
- `a_full_observation_channel_still_delivers_once_a_slot_frees` — the
  steady-state half. With no signal, freeing one slot delivers the observation and
  the placeholders stay intact and in order. This is what stops the preemption
  from quietly becoming lossy delivery.
- `a_stored_reload_outranks_a_ready_send` — both ready on the first poll, the
  reload wins; with the reload consumed and capacity free the identical delivery
  then succeeds, which proves the first result was ordering rather than a channel
  that had no room.
- `cancellation_outranks_a_stored_reload` — the top of the ordering.
- `a_closed_observation_channel_ends_the_round` — `EngineGone` is still the only
  closed-receiver outcome.
- `a_reload_preempts_a_round_blocked_on_the_observation_channel` — round level:
  one real endpoint, the real channel already at capacity, the round downloads
  history, parks in the hand-off, then returns `Reloaded` on reload with the gate
  still asking for that history.
- `the_cron_budgets_are_unchanged` — the two constants, named and locked.

Every blocking precondition uses a `yield_now` bound rather than a timer, so a
regression fails fast instead of hanging the suite. Cadence, startup-round, and
revision-driven-history behaviour stay locked by the pre-existing
`startup_runs_exactly_one_round_before_the_first_period`,
`the_in_flight_request_count_never_exceeds_the_cron_bound`, and
`an_unchanged_revision_suppresses_the_history_body_entirely`.

### Mutation-tested

Replacing the preemptible send with a bare `updates.send(...).await` fails the
three key regressions immediately and deterministically:

```text
test ...a_full_observation_channel_never_delays_cancellation ... FAILED
test ...a_full_observation_channel_never_delays_a_reload ... FAILED
test ...a_reload_preempts_a_round_blocked_on_the_observation_channel ... FAILED
  a reload must complete the blocked delivery
  a reload must end the round that is blocked on delivery
test result: FAILED. 2 passed; 3 failed; ... finished in 0.03s
```

The 0.03 s runtime is the point: the mutation is caught by assertion, not by a
timeout expiring. The fix was then restored and the cron suite re-run green.

### Verification

```text
cargo test -p gregg --all-targets --all-features -- clientd::cron   # 37 passed
cargo fmt --all -- --check                                          # clean
cargo clippy --workspace --all-targets --all-features -- -D warnings # clean
./scripts/check-local.sh                                            # all checks passed
```

The full workspace suite is green: 914 `gregg` tests (2 ignored) and 500 `greggd` tests, alongside `gregg-protocol` (118 + 44 integration), `gregg-host` (62), and `gregg-update` (45). Plan 174's target binding, epoch+revision coherence,
and startup/cadence budgets remain authoritative and untouched.

**Not claimed at the time of closure:** a green six-job CI run — see the
acceptance-criteria note. No new configuration field, dependency, or job was
introduced.

**Superseded after closure:** CI run `37414987171` on `7853743` is now green
across all six existing jobs, including the native Windows Clippy and Test steps.
No new workflow, job, or matrix was introduced. See "CI evidence" at the end of
this record.

### Documentation reconciled

- `architecture/gregg-client.md` — "preemptible inside the round" now explicitly
  covers both awaits a round can park on, with the ordering and the
  bounded-loss-of-superseded-work distinction.
- `crates/gregg/README.md` — the same statement in operator-facing terms.
- `.opencode/skills/gregg-client/SKILL.md` — rule 8 rewritten, rule 9 strengthened
  to name the abandoned hand-off, plus "never grow `CRON_CHANNEL_CAPACITY` to hide
  the gap".
- `AGENTS.md` — the same rule with its reasoning.

### Not reopened

Plans 173, 174, 175, 177, 178, and 179 keep their closure records. Plan 091 is
untouched. No cron TUI rendering, persistent history, polling knob, or
EggPool worker semantics were changed.

### CI evidence (recorded 2026-10-06, after closure)

CI run `37414987171` on `7853743` — <https://github.com/eggstack/gregg/actions/runs/37414987171> —
is green across all six existing jobs: Linux, macOS arm64, macOS Intel, Windows
(full-workspace Clippy and Test, both release builds, SCM lifecycle smoke), MSRV
Rust 1.89, and FreeBSD `gregg-host` native.

The Windows Test step is the platform where the reload/cancel round-selection
regressions actually execute, so it is the authority this plan was withholding.
The workflow is unmodified. The intermediate commits for Plans 179 and 181 were
superseded by later pushes and are covered by this same green tree, so no
separate run is claimed for them. Later run `37485114828` on the
documentation-only record commit `0670165` confirms this evidence note itself
also left `main` green; repeated green runs are not a standing requirement.
