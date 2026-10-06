# Plan 181: EggPool desired-state preemption under result backpressure corrective

Status: planned.

Depends on: completed Plans 151 and 176 plus current main at
`fcbe2dfb129e0327dc0499d6666e54e1ac310fa8`.
Independent of Plan 091 and Plans 179-180.

Opened from the 2026-10-06 review of Plan 176's deterministic result-channel
cancellation primitive.

## Objective

Preserve Plan 151's latest-desired-state convergence even when an already
completed EggPool result is blocked on the full bounded result channel.

Plan 176 proved cancellation can interrupt that blocked send. A newer desired
state — period change, generation change, or deactivation — still cannot.

## Confirmed defect

The worker completion path calls:

~~~text
deliver_result_or_cancel(result_tx, cancel, completed_result)
~~~

and that helper selects only:

- cancellation; or
- `result_tx.send(result)`.

The result channel is bounded at `RESULT_CHANNEL_CAPACITY = 4`.

While the fifth completed result is waiting for receiver capacity, the worker is
inside that helper and no longer polls `control_rx.changed()`. Therefore a
newly published `EggpoolDesiredState` can remain unapplied until the daemon
drains a result slot.

That weakens the Plan-151 contract that the worker converges to the newest
desired active/period/generation state rather than replaying stale work. It is
especially visible for:

- deactivation: the worker can remain stuck delivering a result after the pane
  no longer wants EggPool work;
- period/generation change: a completed stale result can be delivered before the
  worker notices the newer request, and the newer fetch cannot start until
  capacity returns.

Cancellation is already correct and must remain first-priority.

## Required behavior

Make completed-result delivery interruptible by the control watch as well as
shutdown.

Use one production primitive with a typed result, conceptually:

~~~text
deliver_result_or_interrupt(
    sender,
    cancel,
    control_rx,
    result,
    result_generation,
    result_period
) -> DeliveryOutcome
~~~

The helper/state machine should select among:

1. cancellation;
2. `control_rx.changed()`;
3. the bounded result send.

A control notification must inspect the **newest retained desired state** with
`borrow_and_update()`.

If that desired state supersedes the completed result/worker state because it is:

- inactive; or
- active with a different period; or
- active with a different generation,

then abandon the completed result and return the new desired state to the worker
loop. The worker must immediately converge onto it:

- inactive -> abort/clear passive scheduling and remain inactive;
- active superseding state -> start exactly one request for the newest
  period/generation.

If a watch notification carries a state equivalent to the worker's current
desired state, consume the notification and continue waiting to deliver rather
than dropping a still-authoritative result.

If the watch sender is gone, terminate the worker as the outer
`control_rx.changed()` branch already does.

If receiver capacity becomes available with no cancellation/superseding state,
deliver normally and arm the passive refresh deadline exactly as today.

## Priority at simultaneous readiness

Use explicit biased ordering:

~~~text
cancel
control change
result send
~~~

Once shutdown or a superseding desired state is already ready, it must win over
a result-channel slot that becomes available in the same poll. The newest intent
is authoritative; an obsolete completed result need not be delivered first.

This is not permission to make ordinary result delivery lossy. Without a
superseding state or cancellation, full-channel backpressure remains.

## Worker-state integration

Keep one `EggpoolWorkerState` and one request task.

When delivery returns a superseding desired state:

1. do not arm the old result's passive refresh deadline;
2. call the existing convergence logic (or a narrowly refactored equivalent);
3. if it requests work, start one request for the newest desired state;
4. return to the worker select loop.

Do not create a second queue of desired states. The watch channel's latest-value
coalescing remains the product contract.

A completed result for the exact still-current desired state remains valid even
if an equivalent control publication was observed while blocked.

## Deterministic regressions

Exercise the production delivery/control primitive directly with the actual
bounded mpsc and watch channels, then retain worker-level lifecycle tests.

At minimum:

1. **Full result channel + deactivation.**
   - deterministically fill all four slots;
   - start fifth completed-result delivery and prove it is pending;
   - publish inactive desired state without freeing capacity;
   - delivery is abandoned;
   - inactive state is returned/adopted;
   - channel remains exactly four entries and contains no fifth result.

2. **Full result channel + period/generation supersession.**
   - same setup;
   - publish a newer active state;
   - old result is abandoned;
   - the worker converges to the newest period/generation without receiver
     capacity.

3. **Newest state wins over multiple publications.**
   - publish several active states while blocked;
   - only the latest retained state is adopted;
   - no intermediate request replay is required.

4. **Equivalent publication does not discard valid result.**
   - publish the same active/period/generation state;
   - helper consumes it and stays blocked;
   - after one receiver slot is freed, the result is delivered.

5. **Cancellation remains dominant.**
   - with cancellation and control change simultaneously ready, cancellation
     ends the worker and no result is delivered.

6. **Normal delivery unchanged.**
   - no superseding control state;
   - free one slot;
   - result is delivered and passive refresh is armed from completion as today.

7. **Worker-level convergence.**
   - at least one end-to-end worker test proves a newer desired state can start
     its request while the prior completed result would otherwise be blocked by
     full output backpressure.

Mutation-test the core property by removing the `control_rx.changed()` branch;
the deactivation/supersession regression must fail under its deterministic
bound.

No closed port or OS connection timing may be used to synthesize the full-channel
precondition; Plan 176 established the correct fixture pattern.

## Documentation

Reconcile the active EggPool worker description in:

- `architecture/gregg-client.md`;
- `crates/gregg/README.md`;
- `.opencode/skills/gregg-client/SKILL.md`;
- `AGENTS.md`.

State that latest-desired-state convergence remains live while completed-result
delivery is backpressured, not just while an HTTP request is in flight.

## Acceptance criteria

- [ ] A full EggPool result channel cannot delay deactivation.
- [ ] A full result channel cannot delay adoption of a newer period/generation.
- [ ] Superseded completed results are abandoned rather than delivered ahead of
      already-known newer intent.
- [ ] Equivalent desired-state notifications do not discard a still-current
      completed result.
- [ ] Cancellation remains higher priority than control change and result
      delivery.
- [ ] Without cancellation/supersession, the four-slot channel still applies
      ordinary bounded backpressure and results are not dropped.
- [ ] Latest-value watch coalescing and one-request-at-a-time semantics remain
      unchanged.
- [ ] Passive refresh cadence is armed only after a successfully delivered
      current result, as today.
- [ ] Existing EggPool wire/auth/body-cap/health semantics remain unchanged.
- [ ] Focused EggPool/client-daemon tests, formatting, workspace Clippy/tests,
      and `./scripts/check-local.sh` pass.
- [ ] Existing six-job CI completes green, with native Windows Test/Clippy
      providing the platform authority that motivated Plan 176.

## Stop conditions

Open a broader design plan instead if correctness requires:

- making the result channel unbounded;
- queueing every desired state rather than latest-value coalescing;
- more than one concurrent EggPool request;
- changing the EggPool API/wire/configuration contract; or
- changing frontend IPC.

## Preserved exclusions

- cron worker/scheduler behavior;
- greggd scheduler output;
- EggPool service schema changes;
- new channel-capacity/configuration knobs;
- TUI redesign;
- Plan 091 soak evidence;
- new CI workflows/jobs/matrices.
