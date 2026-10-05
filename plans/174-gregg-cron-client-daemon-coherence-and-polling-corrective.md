# Plan 174: Gregg cron client-daemon coherence and polling corrective

Status: planned.

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

- [ ] Every render-visible summary/error transition causes a new frontend
      document even when history revision is unchanged.
- [ ] Unchanged successful polls do not republish only because local attempt
      timestamps move.
- [ ] Summary/history epoch+revision mismatches are never merged and never
      advance the history-fetch gate.
- [ ] A stable-ID endpoint repoint cannot accept an old target's cron
      observation or suppress the new target's initial history fetch.
- [ ] Repointing clears target-specific current/history state; equivalent
      endpoint spellings preserve it.
- [ ] Startup performs one cron round, not two back-to-back rounds.
- [ ] Fleet cron polling uses a fixed small in-flight bound and removes
      single-endpoint head-of-line blocking without request bursts.
- [ ] History remains revision/epoch-driven and is not downloaded every summary
      cadence.
- [ ] Scheduler failures remain distinct from Systems reachability.
- [ ] Scheduler attempt/success timestamps have truthful semantics.
- [ ] Focused clientd/cron tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [ ] Active client-daemon architecture/skill documentation is reconciled.

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
