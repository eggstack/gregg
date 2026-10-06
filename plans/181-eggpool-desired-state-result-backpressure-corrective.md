# Plan 181: EggPool desired-state preemption under result backpressure corrective

Status: complete. See the closure record at the end.

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

- [x] A full EggPool result channel cannot delay deactivation.
- [x] A full result channel cannot delay adoption of a newer period/generation.
- [x] Superseded completed results are abandoned rather than delivered ahead of
      already-known newer intent.
- [x] Equivalent desired-state notifications do not discard a still-current
      completed result.
- [x] Cancellation remains higher priority than control change and result
      delivery.
- [x] Without cancellation/supersession, the four-slot channel still applies
      ordinary bounded backpressure and results are not dropped.
- [x] Latest-value watch coalescing and one-request-at-a-time semantics remain
      unchanged.
- [x] Passive refresh cadence is armed only after a successfully delivered
      current result, as today.
- [x] Existing EggPool wire/auth/body-cap/health semantics remain unchanged.
- [x] Focused EggPool/client-daemon tests, formatting, workspace Clippy/tests,
      and `./scripts/check-local.sh` pass.
- [x] Existing six-job CI completes green, with native Windows Test/Clippy
      providing the platform authority that motivated Plan 176. Recorded after
      closure: run `37414987171` on `7853743` is green across all six existing
      jobs, including the native Windows Clippy and Test steps that are the
      authority for the deterministic full-channel regression Plan 176 closed.
      See the CI evidence note at the end of the closure record.

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

## Closure record

Desired-state-under-backpressure corrective in `crates/gregg/src/eggpool.rs`. No
stop condition was hit: the result channel is still bounded at four, desired
states are still coalesced by the watch's latest value rather than queued, there
is still at most one concurrent request, the EggPool API/wire/config contract is
untouched, and frontend IPC is untouched. No new dependency, configuration knob,
workflow, job, or matrix.

### The blocked hand-off is now interruptible by newer intent

Plan 176 proved cancellation could interrupt a blocked completed-result send. It
selected only cancellation and the send, so while the fifth result waited for a
slot the worker was inside that helper and no longer polling
`control_rx.changed()`. A newly published `EggpoolDesiredState` — deactivation, a
period change, a refresh generation — stayed unapplied until the daemon drained a
result slot, which is exactly the pressure under which leaving the pane should be
fastest.

The primitive is now `deliver_result_or_interrupt`, with a typed
`ResultDelivery`:

```rust
loop {
    tokio::select! {
        biased;
        () = cancel.cancelled() => return ResultDelivery::Cancelled,
        changed = control_rx.changed() => {
            if changed.is_err() { return ResultDelivery::ControlGone; }
            let desired = *control_rx.borrow_and_update();
            if !supersedes_result(&result, desired) { continue; }
            return ResultDelivery::Superseded(desired);
        }
        reserved = sender.reserve() => match reserved {
            Ok(permit) => { permit.send(result); return ResultDelivery::Delivered; }
            Err(_) => return ResultDelivery::ResultReceiverGone,
        },
    }
}
```

Three implementation details are load-bearing:

**`borrow_and_update()`, not `borrow()`.** The retained value is the *newest*
desired state, so several publications while blocked collapse to the last one with
no intermediate replay — latest-value coalescing is the product contract, and a
helper that only read the first observed value would break it.

**`reserve()` rather than `send()`.** The helper loops: an equivalent publication
is consumed and the wait continues, so the same `result` must still be available.
`send()` would consume it on the first attempt and make the retry impossible.
`reserve()` takes the slot without moving the value, and dropping a losing future
returns the reservation, so the semantics are identical for the real delivery.

**An equivalent publication does not abandon.** `supersedes_result` is false for
an active state with the same period and generation. The notification is consumed
and the loop continues, because the completed result *is* the answer for that
state — discarding it would turn a redundant repaint into a silently lost
EggPool result. Inactive, a different period, or a different generation all
supersede.

### Worker-state integration

The completion branch now matches on the typed outcome:

```rust
ResultDelivery::Delivered => {
    if worker.desired.active {
        worker.next_refresh_at = Some(clock.tokio_now() + REFRESH_INTERVAL);
    }
}
ResultDelivery::Cancelled | ResultDelivery::ControlGone | ResultDelivery::ResultReceiverGone => {
    worker.abort_request();
    break;
}
ResultDelivery::Superseded(desired) => {
    if worker.converge(desired) {
        worker.start_request(&client, &endpoint, &clock);
    }
}
```

`Superseded` deliberately reuses the **existing** `EggpoolWorkerState::converge`
rather than a parallel path: it already clears the passive deadline, adopts the
newest state, aborts obsolete work, and reports whether exactly one request must
start. So the old result's passive refresh deadline is never armed, one request is
started for the newest state, and the worker returns to its select loop — one
worker state, one request, no second desired-state queue.

`ControlGone` and `ResultReceiverGone` are split rather than collapsed into one
"gone" value only so the reason survives for future diagnostics; both take the
same terminate path, which is Plan 176's unchanged behaviour.

Ordering is `biased` — cancellation, then control change, then send — so a slot
freeing in the same poll as a shutdown or a known-newer intent does not deliver an
obsolete result first. The newest intent is authoritative; an obsolete completed
result need not be delivered ahead of it.

This is **not** permission to make delivery lossy. With no shutdown and no
superseding state, `reserve()` still applies ordinary bounded backpressure and the
result is delivered when a slot frees.

### Regressions

Six new tests plus the retained Plan-176 test. EggPool tests went 82 → 87. All
drive the production primitive with the real bounded mpsc and watch channels, and
the full-channel precondition is built by filling the real channel — the Plan-176
fixture discipline, with no closed port or OS connection timing.

- `a_full_result_channel_never_delays_deactivation` — four slots filled, delivery
  pending, inactive published with no capacity freed, result abandoned, channel
  still exactly four entries.
- `a_full_result_channel_never_delays_a_newer_period_or_generation` — three
  publications while blocked; only the newest is returned, proving both that the
  newer intent is adopted without receiver capacity and that no intermediate
  replay is required.
- `an_equivalent_desired_state_does_not_discard_a_valid_result` — an identical
  `(active, period, generation)` publication is consumed, the helper stays blocked
  through a bounded yield storm, and the result is then delivered. The placeholders
  stay intact and in order, which also pins the bounded-mpsc FIFO behaviour the
  delivery relies on.
- `cancellation_outranks_a_superseding_control_change` — both ready, shutdown wins.
- `a_gone_channel_ends_delivery` — a dropped publisher is `ControlGone` and a
  dropped receiver is `ResultReceiverGone`; neither parks.
- `a_worker_converges_onto_newer_intent_while_a_result_is_backpressured` — worker
  level, `start_paused`, nothing reading `worker.results`: four passive refreshes
  fill the channel, the fifth result is undeliverable, and a newer period still
  starts its request. Deactivation is then honoured without a slot either.
- `a_full_result_channel_never_delays_cancellation` (retained, updated to the new
  signature) keeps Plan 176's cancellation proof and its closed-channel tail.

The worker-level test uses the file's existing real-clock OS-thread `watchdog`
rather than a Tokio timer, deliberately: arming a timer in a paused-time test
would let virtual clock auto-advance race the loopback round trip.

### Mutation-tested

Removing the `control_rx.changed()` branch fails the core regressions:

```text
test ...a_full_result_channel_never_delays_deactivation ... FAILED
  a deactivation must complete the blocked delivery
test ...a_full_result_channel_never_delays_a_newer_period_or_generation ... FAILED
  a newer period/generation must complete the blocked delivery
test result: FAILED. 1 passed; 2 failed; ... finished in 0.00s
```

and the worker-level test trips its watchdog:

```text
test ...a_worker_converges_onto_newer_intent_while_a_result_is_backpressured ... FAILED
test result: FAILED. 0 passed; 1 failed; ... finished in 15.01s
```

Plan 176's cancellation regression correctly still passes under this mutation,
which is the point: it proves the new branch was added rather than substituted.
The fix was then restored and the suite re-run green.

### Verification

```text
cargo test -p gregg --all-targets --all-features -- eggpool   # 87 passed
cargo test -p gregg --all-targets --all-features -- clientd::cron # 37 passed
cargo fmt --all -- --check                                     # clean
cargo clippy --workspace --all-targets --all-features -- -D warnings # clean
./scripts/check-local.sh                                       # all checks passed
```

The full workspace suite is green: 914 `gregg` tests (2 ignored) and 500 `greggd` tests, alongside `gregg-protocol` (118 + 44 integration), `gregg-host` (62), and `gregg-update` (45). Existing EggPool wire, auth, body-cap, health, and
retry semantics are untouched; `activation_then_deactivation_under_pressure_arms_no_passive_refresh`,
`rapid_publication_never_waits_for_worker_capacity`,
`worker_deactivation_aborts_an_in_flight_request`, and
`worker_panic_in_fetch_task_still_delivers_a_result` all remain green.

**Not claimed at the time of closure:** a green six-job CI run — see the
acceptance-criteria note.

**Superseded after closure:** CI run `37414987171` on `7853743` is now green
across all six existing jobs, including the native Windows Clippy and Test steps
that are the platform authority for the deterministic full-channel regression
Plan 176 closed. See "CI evidence" at the end of this record.

### Documentation reconciled

- `architecture/gregg-client.md` — the worker description now states that
  convergence is live *while a completed result is backpressured*, with the
  ordering, the equivalent-publication rule, the `reserve()` detail, and the
  passive-deadline rule.
- `crates/gregg/README.md` — the same in operator-facing terms.
- `.opencode/skills/gregg-client/SKILL.md` — an explicit paragraph after the
  Plan-164 convergence section, with "do not make delivery lossy to fix this" and
  "do not add a second desired-state queue".
- `AGENTS.md` — a new bullet in the client-polling constraint list.

### Not reopened

Plans 151, 152, 153, 176, 177, 178, 179, and 180 keep their closure records.
Plan 091 is untouched. No cron worker, greggd scheduler output, EggPool schema,
channel-capacity knob, or TUI change was made.

### CI evidence (recorded 2026-10-06, after closure)

CI run `37414987171` on `7853743` — <https://github.com/eggstack/gregg/actions/runs/37414987171> —
is green across all six existing jobs: Linux, macOS arm64, macOS Intel, Windows
(full-workspace Clippy and Test, both release builds, SCM lifecycle smoke), MSRV
Rust 1.89, and FreeBSD `gregg-host` native.

Plan 176 closed because a cross-checked local Windows Clippy run was not closure
evidence for a Windows-only test-determinism fix; this run supplies exactly that
authority. The new backpressure regressions and the retained cancellation
regression all execute in the Windows Test step on the commit carrying this plan.
The workflow is unmodified. The intermediate commits for Plans 179 and 180 were
superseded by later pushes and are covered by this same green tree, so no
separate run is claimed for them.
