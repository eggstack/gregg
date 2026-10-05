# Plan 174: Gregg cron client-daemon coherence and polling corrective

Status: complete. Code at `PLAN174SHA`; see the closure record at the end.

Depends on: completed Plans 166-167 plus current main at
`1aac89f1a82fcd347066379b55105e2ff6bfe770`. Independent of Plan 091 and
Plan 173. Blocks the TUI truthfulness closure in Plan 175.

Opened from the 2026-10-05 post-closure review of the client-daemon scheduler
plane.

## Objective

Make the Gregg client daemon publish every real scheduler-summary transition,
apply only coherent summary/history pairs to the correct configured target, and
keep the nominal 30-second cron cadence bounded across a fleet without an
immediate duplicate startup round.

No TUI may need to poll a remote greggd directly, and scheduler-route failures
remain independent from Systems reachability.

## Finding 1: live summary changes can be cached but never published

`CronObservation::apply` currently decides whether a supported observation is
"changed" from only:

- applied `history_revision`;
- scheduler `epoch`; and
- capability.

It then installs the new summary unconditionally.

That is insufficient because `history_revision` is intentionally a **history**
revision. It does not change for ordinary live state transitions such as:

- idle -> waiting for slot;
- waiting/load-delayed -> running;
- load-high <-> load-unavailable;
- retry deadline changes;
- pending/running timestamps;
- observed load-gate context;
- next-due advancement when no terminal record has yet been added.

The cache can therefore hold a newer valid summary while the daemon returns
`changed = false`, so its frontend `watch` document is not republished. The
TUI remains stale until an unrelated metrics/config/intent event causes another
publication.

The same predicate can suppress **error recovery**: a successful summary clears
`last_error`, but an unchanged epoch/history revision can make the daemon skip
the document that removes the stale warning.

### Required correction

Compare the old and new **operator-visible scheduler state** before overwrite.
At minimum, capability, current summary, and error state must participate.
Local bookkeeping timestamps alone must not force a full frontend publication
every 30 seconds.

A transition that changes any field the cron row can render must return dirty.
A successful read that clears an existing scheduler error must return dirty.

## Finding 2: summary/history requests can straddle a remote restart

The worker requests `/v2/scheduler` and then
`/v2/scheduler/history` independently. Each document is validated on its own,
but the pair is not checked for a matching scheduler epoch/revision before:

- recording the history gate; and
- merging the history into the local cache.

If greggd restarts or history changes between those requests, Gregg can combine
summary A with history B. Because the gate is advanced from B, the next summary
for B can then look already fetched. If the old summary's job set pruned B's
history, the new epoch's records can remain absent until a later history
revision changes.

### Required correction

A history response is applicable to a summary only when:

~~~text
history.epoch == summary.epoch
&& history.history_revision == summary.history_revision
~~~

A mismatch is a transient coherence failure, not valid history:

- do not merge it;
- do not advance the history gate;
- retain the valid summary;
- surface a scheduler-scoped stale/history diagnostic;
- retry on the next bounded cron cadence.

Do not require `generated_at_unix_ms` equality: a live-state-only publication
can legitimately rebuild the pair without changing retained history.

## Finding 3: a stale observation can cross a config repoint

Metrics results carry endpoint identity and are rejected when an ID has been
repointed to a different host/port. Cron observations currently carry only the
stable system ID.

A `Ctrl-R` can therefore replace target A with target B under the same ID
while an A scheduler request is in flight; the late A observation is then
accepted into B's cron state.

The history gate is also keyed only by system ID. If B happens to report the
same epoch/revision tuple as A, the worker can suppress B's initial history
fetch.

### Required correction

Bind cron observations and gate entries to the target identity that was
actually polled.

Prefer reusing the same normalized host/port equivalence rules the metrics
reducer already uses. The engine/fleet boundary must reject a cron observation
whose target no longer matches the current configured endpoint.

When a stable ID is deliberately repointed to a different endpoint:

- reset that ID's **current** cron capability/summary/error state;
- do not associate the old target's cached job history with the new target;
- invalidate the history gate for the old target;
- force first-support/history discovery against the new target even if its
  epoch/revision numerically collides with the old one.

A mere equivalent spelling change of the same normalized endpoint must not
discard state.

## Finding 4: startup polls twice immediately

The worker creates `tokio::time::interval(30s)`, performs a manual round, and
then awaits the interval's first tick. Tokio's first interval tick is immediately
ready, so startup performs two back-to-back summary rounds.

Use `interval_at`, consume the initial tick before the manual round, or an
equivalent deterministic schedule so there is exactly one startup round and the
next ordinary round begins one interval later.

Keep `MissedTickBehavior::Delay`: a slow round must not create catch-up bursts.

## Finding 5: one slow endpoint serializes the whole cron fleet

The scheduler-summary plane is intentionally slower and smaller than metrics,
but walking every endpoint strictly sequentially means one timeout delays every
system after it. With the supported request timeout reaching 60 seconds, the
effective fleet cadence can be far larger than the nominal 30 seconds.

Correct this without turning cron observation into a request burst.

Use a small fixed cron-specific in-flight bound (target: 4; do not exceed 4
without recorded evidence) and no new configuration field. A useful shape is:

1. snapshot the current normalized endpoint list and relevant gate state;
2. run at most the fixed number of endpoint observations concurrently;
3. return one typed observation per endpoint;
4. apply gate updates serially in the worker after a coherent history fetch;
5. send observations through the existing bounded channel.

Do not spawn one unbounded task per endpoint. Cancellation and reload must stay
bounded, and history must still be fetched only on discovery or revision/epoch
change.

The result order need not control frontend semantics because state is keyed by
stable ID, but tests must prove no observation can be applied to the wrong
target.

## Timestamp bookkeeping

Correct the two small provenance inconsistencies in the same pass:

- a successful scheduler summary is also the latest scheduler **attempt**, so
  update `last_attempt_at_unix_ms` along with `last_success_at_unix_ms`;
- `CronObservation.now_unix_ms` is documented as round completion time, so
  capture it after the request/history work completes rather than before the
  first request.

These timestamps must not themselves cause a frontend republish on every
unchanged successful poll.

## Deterministic regressions

Add focused tests for all of the following:

1. A live summary transition with the **same epoch and history revision** returns
   dirty and appears in the next frontend document.
2. Clearing a prior scheduler error with an otherwise unchanged summary returns
   dirty and removes the stale state.
3. An identical supported summary with no prior error does not cause needless
   frontend publication merely because the attempt timestamp advanced.
4. Summary epoch A + history epoch B is rejected, not merged, and does not
   advance the gate.
5. Same epoch but different history revisions is rejected the same way.
6. The next coherent poll after either mismatch refetches and applies history.
7. A late observation from old endpoint A is ignored after stable ID repoints to
   B.
8. A repointed B whose epoch/revision numerically equals A still performs first
   history discovery.
9. An equivalent normalized spelling of the same endpoint preserves cron state.
10. Startup issues exactly one summary round before the first 30-second period.
11. A deliberately slow endpoint does not prevent a later fast endpoint from
    completing within the fixed concurrency window.
12. Instrumented test servers prove the in-flight request count never exceeds
    the chosen cron bound.
13. Existing "history once on discovery/revision change", daemon-restart reseed,
    many-frontends-single-remote-request-plane, old-daemon unsupported, body-cap,
    and transient-failure-retains-data tests remain green.

Use paused/injected Tokio time or short test-only intervals; do not sleep 30
seconds in tests.

## Documentation

Update `architecture/gregg-client.md`, AGENTS.md, and the gregg-client skill
with the corrected invariants:

- frontend publication follows live summary changes, not only history revision;
- summary/history application is coherent on epoch+revision;
- cron observations are target-bound like metrics observations;
- one startup round, then delayed fixed cadence;
- cron endpoint polling is small-bounded concurrent rather than fleet-serial.

Do not expose the concurrency bound as new user configuration.

## Acceptance criteria

- [x] Every render-visible summary/error transition causes a new frontend
      document even when history revision is unchanged.
- [x] Unchanged successful polls do not republish only because local attempt
      timestamps move.
- [x] Summary/history epoch+revision mismatches are never merged and never
      advance the history-fetch gate.
- [x] A stable-ID endpoint repoint cannot accept an old target's cron
      observation or suppress the new target's initial history fetch.
- [x] Repointing clears target-specific current/history state; equivalent
      endpoint spellings preserve it.
- [x] Startup performs one cron round, not two back-to-back rounds.
- [x] Fleet cron polling uses a fixed small in-flight bound and removes
      single-endpoint head-of-line blocking without request bursts.
- [x] History remains revision/epoch-driven and is not downloaded every summary
      cadence.
- [x] Scheduler failures remain distinct from Systems reachability.
- [x] Scheduler attempt/success timestamps have truthful semantics.
- [x] Focused clientd/cron tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [x] Active client-daemon architecture/skill documentation is reconciled.

## Stop conditions

Open a separate plan rather than broadening if:

- endpoint coherence requires changing the local IPC/frame format rather than
  only the daemon's internal cache/observation model;
- bounded cron concurrency requires a new dependency or user configuration
  field;
- fixing live-summary publication would require putting command-output history
  into the high-frequency metrics document;
- a solution reintroduces per-TUI remote polling or changes host reachability on
  scheduler-route failure.

## Preserved exclusions

- greggd execution/output lifecycle (Plan 173);
- scheduler protocol schema/route mutation;
- persistent client history;
- alerting, notifications, job editing, or remote execution;
- unbounded request concurrency or a new polling configuration knob;
- broad local IPC redesign;
- unrelated EggPool worker/test behavior;
- new workflows/jobs/matrices or release automation.

## Closure record

Implemented in `crates/gregg/src/clientd/cron.rs` (worker, observation, gate),
`crates/gregg/src/cron.rs` (cache/retention), `crates/gregg/src/state.rs`
(reconciliation and the target check), and `crates/gregg/src/clientd/daemon.rs`
(the one place that drops a replaced target's answer).

### Finding 1 — publication follows operator-visible state

`CronSystemState::rendered_state()` returns the exact state a cron row can draw:
capability, the summary's job rows, the `(epoch, revision)` identity, and the
stale marker. `CronObservation::apply` snapshots it around the write and returns
`after != before`. That replaced a predicate built from `history_revision`,
`epoch`, and capability only, which could not see idle → waiting, a job starting,
a load gate appearing, a next-due advancing, or an error clearing — because
`history_revision` is a *history* revision and none of those move it.

The digest deliberately omits `generated_at_unix_ms` and the local
attempt/success timestamps. Nothing renders them, and `greggd` rebuilds its
document on any live transition, so including them would make every 30-second
cadence a full document rebuild for every frontend.

The digest clones the bounded job list instead of hashing it: one clone per
cadence per endpoint, next to an HTTP request that allocated far more.

### Finding 2 — summary and history are one pair

`settle` compares `history.epoch`/`history_revision` against the summary before
anything else happens. A mismatch produces the new
`CronFetchError::Incoherent`, is not merged, and leaves the gate where it was so
the next cadence retries. `generated_at_unix_ms` equality is deliberately not
required, because a live-state-only publication can rebuild the pair without
changing retained history.

### Finding 3 — observations and gates are target-bound

`CronObservation` carries the host and port it polled. The engine compares them
against the configured endpoint with the *same* `equivalent_endpoint_host` rule
the metrics reducer applies to poll results, and drops a mismatch — silently,
because a replaced endpoint is not an operator problem. `HistoryGate` is keyed by
`(system id, normalized host, port)` with a unit separator, so a repoint is a new
key and re-discovers its history even when the two report identical
epoch/revision numbers; `forget_absent` prunes by the same keys.
`FleetState::reconcile_systems` — the single boundary that can see a repoint —
calls `CronCache::reset_system`, clearing capability, summary, error, and
retained history for that id.

### Finding 4 — exactly one startup round

`tokio::time::interval`'s first tick is already due, so awaiting it right after
the manual startup round fired a second back-to-back round. `interval_at` with
`MissedTickBehavior::Delay` keeps one round at startup and one per period.

### Finding 5 — a fixed four-read window

A round is one `FuturesUnordered` window of at most `CRON_MAX_IN_FLIGHT` (4)
futures on the worker's own task; each completion refills the window from a
snapshot of the endpoint list. Nothing is spawned, so a reload or shutdown drops
at most those few reads and none outlives the worker. Gate *decisions* stay
serial: the concurrent phase reads an immutable round snapshot, and `settle`
mutates the single gate as results land, so history bookkeeping is still
single-owner. The bound is a constant, not configuration.

`futures-util`'s `async-await` feature is now declared explicitly. `StreamExt` in
`input.rs` already depended on it being switched on transitively by `hyper-util`,
which is not something this crate's correctness should rest on.

### Timestamp bookkeeping

`apply_summary` now records the successful read as *both* the latest attempt and
the latest success, since a success is the most recent attempt too.
`CronObservation.now_unix_ms` is captured in `settle`, after every request the
round made, which is what its documentation claims. Neither participates in the
publication decision.

### Two measurement traps the tests found

Both were mistakes in the *test instrument*, not in the product, and both are
recorded because the wrong instrument would have passed a regression:

1. Twelve endpoints pointed at one port measure a peak of one, because the HTTP
   client pools per origin. The window test now uses twelve distinct remotes.
2. A loopback answer fits in a single socket write, so a server handler runs to
   completion inside one poll and a genuinely concurrent client still measures a
   peak of one. The gated server therefore stalls *before* answering, and counts
   a request out the instant its response is on the wire — holding the connection
   afterwards instead double-counts a connection that already had its answer.

### Evidence

- `cargo test -p gregg --all-features -- clientd::cron` — **24 passed, 0 failed**
  (13 pre-existing, 11 new):
  - `a_live_transition_with_an_unchanged_history_revision_is_still_a_visible_change`
  - `clearing_a_scheduler_error_with_an_otherwise_identical_summary_is_a_visible_change`
  - `an_identical_successful_poll_does_not_publish_just_because_a_timestamp_moved`
  - `a_history_from_another_epoch_is_rejected_rather_than_merged`
  - `a_history_at_a_different_revision_is_rejected_the_same_way`
  - `the_next_coherent_poll_after_a_mismatch_refetches_and_applies_history` — an
    instrumented loopback server answers round 1 incoherently and round 2
    coherently, so recovery is proven rather than assumed
  - `a_target_key_distinguishes_a_repoint_from_an_equivalent_spelling`
  - `a_repointed_target_whose_numbers_collide_still_performs_first_discovery`
  - `startup_runs_exactly_one_round_before_the_first_period` — asserts one summary
    request at startup, no second round 150 ms later, and that the cadence still
    elapses
  - `one_slow_endpoint_does_not_stall_the_rest_of_the_fleet` — the slow endpoint
    is *first* in the list, so a sequential walk could not have delivered the
    seven fast systems inside the budget they are given
  - `the_in_flight_request_count_never_exceeds_the_cron_bound` — a fleet-wide
    meter over twelve remotes proves the peak is at most 4, greater than 1, and
    back to zero when the round ends
- `cargo test -p gregg --all-features -- state::` — includes
  `a_repoint_forgets_the_previous_targets_cron_state`,
  `a_late_observation_for_a_replaced_endpoint_is_ignored`, and
  `an_equivalent_endpoint_spelling_keeps_cron_state`.
- Unchanged regressions still green: `an_unchanged_revision_suppresses_the_history_body_entirely`,
  `a_revision_change_fetches_history_exactly_once`, `a_daemon_restart_reseeds_from_the_remote_ring`,
  `many_frontends_cannot_add_a_single_remote_request`,
  `an_old_daemon_is_unsupported_rather_than_a_failure`,
  `a_malformed_document_is_reported_as_invalid_not_as_a_network_error`,
  `a_failed_history_fetch_does_not_advance_the_gate`,
  `a_repeated_observation_does_not_grow_the_cache`,
  `a_scheduler_failure_keeps_the_last_known_data`,
  `a_summary_served_without_its_history_route_is_reported`.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean.
- `./scripts/check-local.sh` — `=== all checks passed (mode: default) ===`;
  **878 gregg tests, 0 failed** (was 866).

No stop condition was hit: no IPC/frame format changed, no dependency was added,
no command-output history entered the metrics document, no per-TUI polling was
reintroduced, and a scheduler-route failure still cannot reach reachability.

### Documentation

`architecture/gregg-client.md` gained "One publication per real change", "A
summary and its history are one pair", and "Scheduler state is target-bound",
plus the one-startup-round and four-read window in "Two unequal fetch planes".
The gregg-client skill's cron rules grew from three to seven, covering the same
invariants. `crates/gregg/README.md` and `AGENTS.md` state the pair rule, the
target binding, the publication predicate, and the bounded window.
