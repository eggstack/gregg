# Plan 160: Scheduler civil-clock reconciliation hardening

Status: complete.

Depends on: completed scheduler line through Plan 159 and current main at
`e08538373fd9343efce77ec6a25c072ad719a196`. Independent of the remaining
Plan 091 sustained-soak record.

## Objective

Harden the completed greggd maintenance scheduler against discontinuous
wall-clock changes while preserving its settled cron, load-gating, execution,
security, and footprint architecture.

The current scheduler converts the next civil-time occurrence into a Tokio
monotonic `Instant` and may sleep directly to that deadline. That is correct
while wall time advances normally, but a large forward system-clock adjustment
after the sleep is armed does not advance Tokio's monotonic clock. A job whose
civil-time occurrence is now due can therefore remain asleep until the old
monotonic deadline.

Example:

~~~text
01:00 local     daily 03:00 job -> scheduler arms a ~2 hour monotonic sleep
01:05 local     operator/NTP advances wall clock to 04:05
04:05 local     job is already civil-time due, but monotonic sleep still has
                ~1h55m remaining
~~~

The scheduler should instead reconcile civil time periodically at a cadence
appropriate to its minute-granularity cron language, while continuing to use
monotonic time for retry/max-wait/child lifecycle semantics.

This is a post-closure correctness pass. It does not reopen Plans 155-159 or
their product/budget decisions.

## Current behavior to preserve

The completed scheduler line already guarantees:

- strict five-field minute-granularity local cron schedules;
- the existing `@hourly`, `@daily`, `@weekly`, and `@monthly` aliases;
- traditional DOM/DOW OR behavior;
- correct spring-forward gap and fall-back overlap handling;
- calendar-impossible schedules rejected before listener bind;
- strictly-exclusive next-occurrence calculation;
- no replay of daemon downtime;
- direct argv execution with no implicit shell;
- same-principal execution and Unix root opt-in;
- one pending occurrence per job;
- one global scheduled child;
- fixed monotonic load-retry and max-wait bounds;
- fresh cached-load recheck before every gated launch;
- no HTTP self-poll or duplicate host sampling;
- direct-child shutdown under the existing bounded lifecycle;
- no persistent queue, remote execution surface, user impersonation, or secret
  store;
- Windows time-only jobs and rejection of Windows load gates;
- scheduler-line stripped-binary baseline of 3,261,664 bytes from Plan 159.

All of those are invariants for this plan.

## Clock-domain rule

Make the scheduler's use of time explicit:

### Civil wall time

Use `chrono::Local::now()` only to decide:

- whether a cron occurrence is due;
- what the next cron occurrence is under local civil-time/DST semantics.

Wall time is allowed to jump forward or backward.

### Monotonic time

Continue using `tokio::time::Instant` for:

- load-gate retry intervals;
- pending `max_wait` expiration;
- child elapsed duration;
- child shutdown bounds;
- the bounded civil-clock reconciliation wake itself.

A wall-clock adjustment must not shorten or extend a configured monotonic
`retry_interval_ms` or `max_wait_ms`.

## Core correction: cap long scheduler sleeps

Add one small scheduler-internal civil-clock reconciliation bound.

Recommended first-release value:

~~~text
MAX_CIVIL_RECHECK = 60 seconds
~~~

The scheduler's actual sleep deadline becomes logically:

~~~text
wake_at = min(
    next semantic monotonic deadline,
    monotonic_now + MAX_CIVIL_RECHECK
)
~~~

where the semantic deadline is still the earliest of:

- next cron occurrence mapped from the current civil wall time;
- next pending load retry;
- pending max-wait expiry.

Child completion and daemon shutdown remain separate `select!` branches as
today.

A one-minute cap matches the public cron language's finest granularity and
keeps the correction lean. Do not add a sub-minute ticker.

### Idle-cost expectation

With configured jobs and no imminent occurrence, the scheduler may now wake at
most once per minute to re-read civil time and recompute its sleep. A daemon
with no configured jobs still spawns no scheduler task and pays zero cost for
this feature.

The reconciliation wake must:

- read `Local::now()`;
- read `Instant::now()`;
- perform the existing bounded O(number-of-jobs) scheduler scan;
- perform no host telemetry collection;
- perform no HTTP request;
- perform no filesystem scan;
- spawn no process merely because the cap expired.

At the maximum 64 configured jobs, this should remain negligible relative to
greggd's existing one-second sampling cadence.

## Forward wall-clock jumps

When wall time advances beyond one or more cron occurrences:

- the scheduler must observe the new civil time no later than one
  `MAX_CIVIL_RECHECK` interval after the jump, unless an active child or
  shutdown path already causes an earlier wake;
- each affected job becomes at most one pending occurrence;
- existing coalescing semantics apply;
- `next_due` advances strictly after the newly observed wall time;
- no catch-up queue is created for every civil occurrence skipped by the jump;
- if the global child slot is free and any load gate permits launch, at most
  one job starts;
- remaining jobs retain the existing deterministic oldest/config-order
  selection and load recheck behavior.

A forward jump therefore means "notice promptly and coalesce", not "replay
every skipped cron firing."

## Backward wall-clock jumps

When wall time moves backward:

- do not treat the monotonic wake corresponding to the old wall estimate as
  proof that the civil occurrence is due;
- re-read wall time and, if `next_due > wall_now`, go back to sleep under the
  same one-minute reconciliation cap;
- do not recreate an occurrence that the scheduler has already consumed merely
  because the clock moved backward;
- preserve the cron layer's existing DST fall-back behavior, where the two real
  instants corresponding to an ambiguous local minute are intentionally
  distinct occurrences.

Configured load retry/max-wait deadlines remain monotonic and must continue to
expire normally even while wall time moves backward.

## Timezone-change boundary

Do not add platform-specific timezone watchers, `/etc/localtime` polling,
registry watches, unsafe FFI, or a bundled timezone database.

The primary contract of this pass is wall-clock discontinuity correction.

During each reconciliation wake, continue obtaining a fresh
`chrono::Local::now()`. If the existing Chrono/OS local-time path reflects a
changed system timezone, subsequent due checks and newly calculated
occurrences should naturally use that current local-time view.

Do not promise immediate detection of a timezone-rule change that the existing
`chrono::Local` backend itself does not expose to the running process. A
dedicated dynamic-timezone feature would require separate research and a
separate plan.

## Implementation shape

Keep the production change minimal.

Preferred seam:

~~~text
const MAX_CIVIL_RECHECK: Duration = Duration::from_secs(60);

fn bounded_wake_deadline(
    semantic_deadline: Instant,
    now: Instant,
) -> Instant {
    semantic_deadline.min(now + MAX_CIVIL_RECHECK)
}
~~~

The exact helper/name may differ, but preserve these properties:

- pure monotonic calculation;
- no allocation;
- no async work;
- independently unit-testable;
- one call in the scheduler event loop immediately before `sleep_until`.

Do not push the cap into `Engine::next_deadline` if doing so obscures the
distinction between semantic deadlines and the reconciliation wake. Keeping the
semantic deadline truthful makes clock-jump tests easier to reason about.

## Deterministic test matrix

Do not test this by changing the CI runner's real system clock.

Use the existing pure `Engine::tick(wall_now, now, ...)` seams plus a pure
bounded-deadline helper, or introduce the smallest clock seam needed to avoid
real sleeps.

Required coverage:

### Wake bound

- a cron occurrence ten hours away yields a scheduler wake no more than
  60 seconds of monotonic time away;
- a cron occurrence 15 seconds away retains the 15-second semantic deadline;
- a load retry 10 seconds away is not delayed by the civil recheck;
- a max-wait expiry 20 seconds away is not delayed by the civil recheck;
- no-job daemon path still constructs no scheduler task.

### Forward jump

Starting from a daily job whose stored next occurrence is two hours away:

1. compute the capped wake;
2. advance monotonic time only by the cap;
3. supply a wall time now beyond the stored occurrence;
4. prove exactly one pending/launch decision appears;
5. prove `next_due` moves strictly after the jumped wall time.

Add a multi-occurrence case, such as an every-minute job jumped forward by
several hours, and prove it still creates one pending occurrence rather than a
backlog.

### Backward jump

Starting shortly before a due occurrence:

1. arm the bounded wake;
2. move wall time backward before the wake;
3. prove the old monotonic estimate does not launch the job;
4. prove subsequent capped wakes remain bounded;
5. move wall time forward to the actual civil occurrence and prove exactly one
   launch.

### Monotonic load deadlines

With a pending load-gated job:

- move wall time forward several hours while monotonic time advances less than
  `retry_interval_ms`; retry remains not due;
- move wall time backward while monotonic time crosses `retry_interval_ms`;
  retry becomes due;
- repeat for `max_wait_ms` and prove wall changes cannot extend or shorten the
  pending expiration.

### DST regression

Retain the existing spring-gap and fall-overlap tests unchanged. Add only a
scheduler-level regression if the new wake cap alters behavior observable
above `LocalSchedule`; otherwise do not duplicate the schedule module's
existing DST suite.

## Logging

Do not log every civil reconciliation wake.

Normal once-per-minute rechecks are an internal timing mechanism and should be
silent.

Do not attempt to infer or log "system clock changed" merely by comparing wall
and monotonic deltas in this pass. Clock-step detection thresholds become
another policy surface and are unnecessary for correctness.

Existing job pending/start/completion/expiry logs remain unchanged.

## Footprint review

Plan 159 states that future scheduler-line growth beyond 3,261,664 stripped
bytes reopens review. Plan 160 is that review.

The change should be tiny and target a byte-neutral or near-byte-neutral result.
Measure using the same release profile and method as Plans 156/158/159.

Required record:

~~~text
Plan-159 scheduler baseline     3,261,664 bytes
Plan-160 final                  <measured>
delta                           <measured>
~~~

Rules:

- do not add a dependency;
- do not change `chrono` or Tokio feature selection;
- do not relax LTO/strip/panic settings;
- do not hide positive growth by selecting a different baseline/toolchain
  method;
- prefer reducing equivalent local code if the new cap causes measurable
  growth.

A small positive delta may be accepted by Plan 160 only if it is directly
attributable to this correctness fix and recorded explicitly. Material growth
that suggests a new abstraction/dependency must stop for a separate footprint
decision rather than silently moving the Plan-159 baseline again.

## Documentation

Update only where the runtime contract benefits from it:

- `architecture/greggd-daemon.md`: scheduler uses local civil time for cron
  semantics, monotonic time for deferral/lifecycle, and rechecks civil time at
  a bounded minute cadence;
- `docs/daemon.md`: note that large wall-clock adjustments are reconciled
  within approximately one minute and skipped occurrences coalesce rather than
  replay;
- matching daemon/config skill documentation if that contract is repeated
  there;
- CHANGELOG if repository convention requires behavior fixes to be recorded.

Do not turn this into end-user clock/NTP configuration guidance.

## Expected implementation surface

Likely:

~~~text
crates/greggd/src/scheduler.rs
architecture/greggd-daemon.md
docs/daemon.md
CHANGELOG.md                         # if required by repository convention
matching skills                     # only where scheduler timing is documented
plans/160-scheduler-civil-clock-reconciliation-hardening.md
plans/README.md
~~~

Not expected:

~~~text
crates/greggd/src/scheduler/schedule.rs   # cron/DST semantics already correct
crates/greggd/src/config.rs
Cargo.toml
Cargo.lock
crates/gregg-protocol/**
crates/gregg-host/**
crates/gregg-update/**
crates/gregg/**
systemd/launchd/SCM definitions
HTTP routes
croncheck/control/update/uninstall
CI workflow/matrix files
~~~

## Verification

Focused:

~~~text
cargo test -p greggd --all-targets --all-features -- scheduler
cargo test -p greggd --all-targets --all-features -- schedule
cargo test -p greggd --all-targets --all-features -- run
cargo +1.89 test -p greggd --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
./scripts/check-local.sh --release
cargo build -p greggd --release
~~~

Record the final stripped `greggd` size using the Plan-159 measurement method.

One ordinary existing six-job CI run is sufficient hosted qualification. Do
not add a clock-jump workflow, privileged runner, time-changing integration
test, scheduler-specific CI job, or artifact bundle.

A real system-clock-changing smoke is not required and should not be performed
on shared CI/production hosts. The pure deterministic clock-domain tests are
the authoritative evidence.

## Acceptance criteria

- [ ] Long civil-time sleeps are capped by a scheduler-internal
      `MAX_CIVIL_RECHECK` of approximately one minute.
- [ ] Semantic cron/retry/max-wait deadlines remain distinct from the
      reconciliation wake.
- [ ] A forward wall-clock jump is observed within the reconciliation bound.
- [ ] Forward jumps coalesce skipped cron occurrences rather than replaying a
      backlog.
- [ ] A backward wall-clock jump does not launch a job before its stored civil
      occurrence is actually due.
- [ ] Already-consumed occurrences are not recreated by a backward jump.
- [ ] Retry and max-wait remain monotonic and are not shortened/extended by
      wall-clock jumps.
- [ ] Existing DST spring-gap and fall-overlap behavior remains green.
- [ ] No scheduler wake is added when `jobs` is empty.
- [ ] Reconciliation wakes do not sample host telemetry, poll HTTP, touch the
      filesystem, or spawn processes.
- [ ] Normal reconciliation wakes produce no log noise.
- [ ] No dependency or Cargo feature change is introduced.
- [ ] Same-principal execution, load gating, coalescing, one-child policy,
      shutdown, Windows behavior, and remote/security boundaries are unchanged.
- [ ] Final stripped `greggd` size and delta from 3,261,664 are recorded.
- [ ] Any positive footprint delta is explicitly justified under this plan; no
      silent scheduler budget increase occurs.
- [ ] Focused/default/release/MSRV checks pass.
- [ ] Existing six-job CI passes.
- [ ] Plans 155-159 remain historically closed and are not rewritten as open.
- [ ] Plan 091 remains independent and unchanged unless its own soak evidence
      separately lands.

## Stop conditions

Stop and write a separate plan rather than broadening this one if the
implementation requires:

- a new dependency;
- unsafe clock/timezone FFI;
- platform-specific timezone watchers;
- polling more frequently than once per minute merely for clock correction;
- changing local cron semantics to UTC;
- changing DST gap/overlap semantics;
- persistent scheduler state or missed-job replay;
- a second simultaneous child;
- changes to load-gate retry/max-wait semantics;
- material stripped-binary growth that cannot be offset or narrowly justified;
- HTTP/protocol/startup/control/croncheck/update/uninstall changes.

## Handoff

Implement the bounded reconciliation wake as a timing-layer correction around
the existing scheduler engine. Do not rewrite the cron parser or the scheduler
state machine.

The key proof is separation of clock domains: civil wall time decides cron
eligibility; monotonic time decides retry/expiry/lifecycle; the scheduler never
trusts a long monotonic sleep for more than roughly one cron-resolution minute
without re-reading civil time.

## Closure record

Implemented at `6bb5dcf` with a Windows-only test-attribute correction at
`d650c59`. Existing CI run `37177061388` is green across all six jobs
(Linux, macOS arm64, macOS Intel, Windows incl. SCM smoke, MSRV 1.89,
FreeBSD `gregg-host` native) at `d650c59`. An earlier run `37176534146`
at `6bb5dcf` failed only its Windows job with `E0425: cannot find function
daily_job`: the test insertion had landed after the shutdown test's
`#[cfg(unix)]` attribute, cfg-ing out the helper on Windows. The correction
moves the attribute back onto
`active_direct_child_is_terminated_and_reaped_on_shutdown` with no
production change, verified locally by
`cargo check -p greggd --all-targets --all-features --target
x86_64-pc-windows-msvc`, and the rerun is fully green. The failed run is
preserved as history, not rewritten.

Production change (`crates/greggd/src/scheduler.rs`, +19 lines):

- `MAX_CIVIL_RECHECK = Duration::from_secs(60)`;
- pure monotonic `bounded_wake_deadline(semantic_deadline, now)` helper
  (no allocation, no async, independently unit-tested);
- one call in `run()` immediately before `sleep_until`;
  `Engine::next_deadline` stays truthful and uncapped.

Deterministic coverage (nine new tests, no real clock changes, no sleeps):

- `civil_recheck_cap_is_one_minute_and_pure`: 10 h wakes within 60 s;
  15 s cron, 10 s retry, and 20 s max-wait deadlines are retained exactly;
- `semantic_deadline_stays_truthful_while_the_wake_is_capped`: the engine
  still reports the true civil deadline while only the sleep is capped;
- `empty_job_list_builds_no_engine_state`: the `run.rs` `jobs.is_empty`
  branch (unchanged) still spawns no scheduler task;
- `forward_wall_jump_coalesces_to_one_pending_and_advances_next_due`
  (daily job) and `forward_jump_of_every_minute_job_does_not_build_a_backlog`
  (every-minute job jumped three hours): exactly one launch, `next_due`
  strictly after the jumped wall time;
- `backward_wall_jump_does_not_launch_before_the_stored_occurrence`:
  the old monotonic estimate launches nothing, later wakes stay capped,
  the real occurrence launches exactly once, and moving back over the
  consumed occurrence recreates nothing;
- `load_retry_stays_monotonic_across_wall_jumps` and
  `max_wait_expiry_stays_monotonic_across_wall_jumps`: forward wall jumps
  cannot shorten retry/max-wait, backward jumps cannot extend them.

Footprint review (same release profile and `stat -c %s
target/release/greggd` method as Plans 156/158/159, `x86_64-unknown-linux-gnu`):

```text
Plan-159 scheduler baseline     3,261,664 bytes
Plan-160 final                  3,261,664 bytes
delta                                 0 bytes (byte-neutral)
```

No dependency, feature, profile, or toolchain change (`git status` shows
only `scheduler.rs` plus docs). The existing DST spring-gap and
fall-overlap tests in `schedule.rs` are unchanged and green, so no
scheduler-level DST duplication was added. Reconciliation wakes log
nothing (the `Deadline` branch is untouched) and perform no telemetry,
HTTP, filesystem, or process work by construction. Same-principal
execution, load gating, coalescing, one-child policy, shutdown, Windows
time-only behavior, and the remote/security boundaries are unchanged.

Verification at `d650c59`:

- `cargo test -p greggd --all-targets --all-features -- scheduler`:
  26 passed;
- `cargo test -p greggd --all-targets --all-features -- schedule`:
  28 passed (includes the unchanged DST tests);
- `cargo test -p greggd --all-targets --all-features -- run`: 48 passed;
- `cargo test --workspace --all-targets --all-features`: 1326 passed,
  2 ignored;
- `cargo +1.89 test -p greggd --all-targets --all-features`: 448 passed,
  0 failed;
- `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  clean;
- `./scripts/check-local.sh` (default): all checks passed;
- `./scripts/check-local.sh --release`: all product gates passed,
  including the clean-tree check after the implementation commits. One
  transient failure of the network-dependent
  `gregg-update::exec::download_classifies_code_in_a_single_request`
  (live curl to `example.invalid`) appeared mid-pass and passed on
  immediate retry in isolation and in the full rerun; `gregg-update` is
  untouched by this plan (`git status` confirms), so it is recorded as
  environmental flake, not plan evidence.

All acceptance criteria are met. Plans 155-159 remain historically closed
and are not rewritten; their records are only referenced. Plan 091 keeps
its existing status, gated solely on its own extended soak record, and is
unaffected by this closure. No future plan depends on Plan 160, so this
closure unblocks nothing and changes no other plan's status. No real
system-clock smoke was performed, per the plan's own prohibition.
