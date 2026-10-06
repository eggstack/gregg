# Plan 176: Windows EggPool full-result-channel test determinism corrective

Status: planned.

Depends on: current main at `892a77195b0a91c7d4e9fb7ee02d560e7ae64c83`.
Independent of Plan 091 and Plans 177-178.

Opened from the 2026-10-05 post-Plan-175 repository review. This is a
test/evidence corrective unless deterministic reproduction proves the production
cancellation path itself is wrong.

## Objective

Restore a truthful green Windows CI signal for the existing EggPool
backpressure/cancellation contract without weakening the contract or changing
EggPool polling behavior merely to satisfy a platform-sensitive fixture.

The contract being tested is narrow and valid:

> once a completed EggPool result is blocked on a full bounded result channel,
> worker cancellation must still win; the worker must not park forever inside
> `Sender::send`.

## Confirmed CI defect

Current `main` is red in workflow run `37373241900` only because:

~~~text
eggpool::tests::worker_cancellation_wins_over_a_full_result_channel
assertion failed: the channel must be full for this to test anything
left: 0
right: 4
~~~

The same failure predates Plans 173-175 and is present on their base line. The
production branch already performs delivery inside a cancellation-biased
`tokio::select!`:

~~~rust
tokio::select! {
    biased;
    () = cancel.cancelled() => false,
    sent = result_tx.send(result) => sent.is_ok(),
}
~~~

The failing test does not deterministically reach that state. It points the
worker at closed loopback port 1, publishes four generations, advances paused
Tokio time, yields once per generation, and assumes each network failure has
already become a delivered result. That assumption is scheduler/OS dependent.
On the Windows runner none of the four requests has completed into the result
channel when the assertion runs.

A test named for **full-channel cancellation** must first establish the full
channel itself; a closed-port timing race is not evidence for that property.

## Preferred correction

Factor the existing production delivery expression into one small private async
primitive, for example conceptually:

~~~text
deliver_result_or_cancel(sender, cancel, result) -> delivered: bool
~~~

The worker completion branch must call that exact primitive. Do not create a
test-only reimplementation of the select.

Test the full-channel property directly:

1. create the real bounded mpsc channel at `RESULT_CHANNEL_CAPACITY`;
2. fill all four slots synchronously/deterministically with valid
   `EggpoolResult` values;
3. start/poll a fifth call to the production delivery primitive and prove it
   cannot complete while the channel is full;
4. cancel the token without draining a slot;
5. require the delivery primitive to complete promptly with "not delivered";
6. verify no fifth result entered the channel;
7. drop the sender and drain the four preloaded results so channel closure is
   also explicit.

Keep the existing worker-level cancellation tests for lifecycle coverage:

- cancellation aborts an in-flight request;
- deactivation aborts an in-flight request;
- worker shutdown closes its result stream.

The direct full-channel test complements those tests rather than asking a real
HTTP failure to synthesize backpressure.

If extracting the delivery primitive would materially complicate production
code, a test-only injected completed-fetch seam is acceptable, but it must still
exercise the worker's actual send/cancel branch. A local TCP server or closed
port is not the preferred fixture because neither makes completion scheduling
the property under test.

## Production invariants

This corrective must preserve:

- result channel capacity = 4;
- latest-desired-state/watch semantics;
- cancellation-biased result delivery;
- completed-result backpressure rather than unbounded buffering;
- request abortion on cancellation/deactivation;
- passive refresh cadence and period/generation semantics;
- EggPool summary/status wire behavior, authentication, body limits, and
  endpoint handling;
- daemon-owned EggPool worker lifecycle.

Do not increase the channel capacity, add sleeps/retries to production, or make
the send lossy to turn CI green.

## Deterministic regressions

The corrected test must prove the precondition and behavior separately:

- exactly `RESULT_CHANNEL_CAPACITY` entries are buffered before the blocked
  delivery starts;
- the fifth delivery remains pending while the token is live and no receiver
  slot is freed;
- cancellation completes that delivery without consuming receiver capacity;
- the buffered four results remain intact and in order;
- dropping all senders eventually closes the receiver;
- existing worker cancellation/deactivation/convergence tests remain green.

Do not use a fixed real-time sleep as proof that a future is pending. Prefer
paused Tokio time, `poll!`/a bounded select, or another deterministic
synchronization primitive.

## CI evidence

The closure record must include a new ordinary CI run on the implementation SHA.

Because the current Windows failure triggers workflow fail-fast and causes
later Linux/MSRV work to be cancelled in some runs, closure requires the final
run to finish all existing jobs rather than merely showing the focused Windows
test green:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows, including full-workspace Clippy and Test;
- MSRV Rust 1.89;
- FreeBSD gregg-host native.

No new workflow, job, matrix, or retry wrapper is required.

## Acceptance criteria

- [ ] The full-result-channel test no longer depends on closed-port completion,
      OS connection timing, or a single `yield_now`.
- [ ] The test fills the real bounded result channel deterministically before
      asserting cancellation behavior.
- [ ] The production send/cancel select remains cancellation-biased and bounded.
- [ ] A fifth completed result blocked by a full channel is abandoned on
      cancellation without requiring the receiver to free capacity.
- [ ] Existing EggPool worker lifecycle, convergence, period, health, and
      backpressure tests remain green.
- [ ] No EggPool wire/config/API behavior changes.
- [ ] `cargo fmt --all -- --check`, workspace Clippy with `-D warnings`,
      workspace tests, and `./scripts/check-local.sh` pass.
- [ ] Native Windows CI executes the corrected test and the complete existing
      six-job workflow finishes green.

## Stop conditions

Open a separate production corrective rather than broadening this plan if a
deterministic full-channel fixture shows that:

- cancellation does not actually win over the blocked production send;
- a completed result can be duplicated/reordered by the delivery branch; or
- worker shutdown still parks after the delivery primitive returns.

## Preserved exclusions

- EggPool protocol/schema or service-health changes;
- channel-capacity changes;
- lossy `try_send` result delivery;
- new retry/backoff behavior;
- client-daemon IPC or cron work;
- scheduler execution/output work;
- Plan 091 soak evidence;
- new CI jobs or workflows.
