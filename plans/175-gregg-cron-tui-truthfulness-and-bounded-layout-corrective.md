# Plan 175: Gregg cron TUI truthfulness and bounded-layout corrective

Status: planned.

Depends on: completed Plan 166 and Plan 174's corrected client-daemon state
publication/coherence behavior. Independent of Plan 091 and Plan 173.

Opened from the 2026-10-05 post-closure review of the new plain-`c` cron view.

## Objective

Make every cron line in Gregg semantically truthful and keep the selected job
usable on small terminals and large job sets.

This is a renderer/presentation corrective. It does not change scheduler
execution, remote protocol types, remote polling ownership, or history retention
policy.

## Finding 1: load-gate rows print the wrong relation

`load_gate_label` currently formats every gate as:

~~~text
load15 <observed> > <threshold>
~~~

greggd intentionally keeps the gate decision that admitted a job, so running or
idle rows can validly carry an observation **below** the threshold. Rendering
`1.20 > 8.00` is objectively false. `LoadUnavailable` currently becomes
`— > 8.00`, which is not a numeric comparison at all.

Render by state/meaning:

- `LoadHigh`: `load15 9.24 > 8.00` (or equivalent compact wording);
- `LoadUnavailable`: `load15 unavailable (max 8.00)`, with no fabricated
  comparison;
- `Running`: identify the admission/start gate, e.g.
  `start load15 1.20 <= 8.00`;
- `Idle` with retained gate context: identify it as the previous/last gate,
  e.g. `last gate load15 1.20 <= 8.00`, so an old observation is not presented
  as current live load;
- time-only jobs: no load token.

Keep `—`/unavailable semantics; never turn a missing observation into zero.

## Finding 2: elapsed-state wording says "ago"

`state_detail` reuses the generic `age()` helper for timestamps that describe
a state duration. It produces strings such as:

~~~text
running for 3m ago
pending 17m ago
queued 2m ago
~~~

Split countdown/age formatting from elapsed-state formatting:

- idle next-due: `next 11h` or `next in 11h`;
- running: `for 3m`;
- load delayed/unavailable: `pending 17m`;
- slot wait: `queued 2m`;
- retry: `retry 20s` / `retry in 20s`.

Clock skew must degrade conservatively (zero elapsed/countdown or an unavailable
marker), not underflow.

## Finding 3: history clock labels are UTC but look local

The renderer converts Unix milliseconds directly to day/hour/minute with no
timezone conversion. Those fields are UTC, while greggd cron schedules are
defined in the remote host's local civil time.

A New York job configured for `03:00` can therefore show a recent record at
`07:00` during DST with no indication that the two lines use different time
bases.

The current protocol does not carry the remote timezone history needed to
reconstruct scheduler-local civil timestamps correctly across DST. Do **not**
invent it in this corrective.

Label the record clock explicitly as UTC, for example:

~~~text
10-05 07:00Z
~~~

or another equally compact unambiguous form. Relative durations remain based on
Unix instants and need no timezone label.

A future remote-timezone feature, if desired, requires its own protocol plan.

## Finding 4: layout accounting omits the stale row and can truncate silently

`desired_rows` counts header/jobs/selected history but the renderer emits an
additional scheduler-stale warning. The outer layout can therefore reserve one
row too few exactly when the warning is present.

The renderer also returns immediately when its vertical budget is exhausted
while its module contract says viewport truncation is reported separately.

Make requested-height and emitted-height accounting derive from the same
logical rows. When content does not fit, reserve/show one explicit marker such
as:

~~~text
  … more cron rows not shown
~~~

Do not claim all five requested records are visible when the viewport only
contains a prefix.

## Finding 5: the fixed first-24 job slice can hide the selected job/history

The job table always takes the first `MAX_JOB_ROWS` jobs. With more than 24
configured jobs, keyboard selection can move to a job outside that slice. The
selected row then disappears.

On a short terminal, the large job list is also rendered before the selected
history and can consume the entire available block, so the operator presses
`c` to inspect a job and sees no recent-run section at all.

Keep one scroll model; do **not** add an inner persistent scrollbar. Compute a
bounded visible job window from the current selected job:

- the selected job row is always inside the visible window when it exists;
- clamp the window at the beginning/end of the job list;
- mark omitted jobs above/below when width/height permits;
- reserve vertical priority for the selected-job header and at least the newest
  terminal record headline when one exists;
- only then spend remaining rows on additional job rows/output/history.

The default requested history depth remains five. A tiny terminal may show less,
but must say it truncated rather than silently pretending otherwise.

## Rendering/tests

Add deterministic renderer/helper tests covering:

- load-high numeric relation;
- load-unavailable with no numeric comparison;
- running admitted-under-threshold relation;
- idle retained-gate wording that does not imply current load;
- running/pending/queued/retry elapsed grammar with no `ago` suffix;
- future and past clock-skew boundaries without underflow;
- explicit UTC/Z history timestamp;
- stale warning included in `desired_rows`;
- exact-fit and one-row-short vertical budgets;
- explicit local viewport truncation marker, distinct from remote output
  truncation `stdout+`/`stderr+`;
- 60+ jobs with a selected index before, inside, and after the old first-24
  boundary;
- selected job remains visible after `j`/`k` navigation;
- short terminal still prioritizes selected-job header/newest record over
  unrelated job rows;
- Unicode/control sanitization and terminal-cell clipping remain intact;
- drive/network/cron expansions remain independent.

Use Ratatui `TestBackend` only where final cells matter; keep pure formatting
tests for helper semantics.

## Documentation

Update `README.md`, `crates/gregg/README.md`,
`architecture/gregg-client.md`, AGENTS.md, and the gregg-client skill where
they describe cron display semantics.

Document:

- remote schedule strings are remote-local civil cron;
- record clock labels are explicitly UTC until the protocol carries remote
  timezone information;
- load-unavailable is never a numeric comparison;
- the selected cron job stays visible in a bounded job window;
- viewport truncation is explicit and separate from remote stream truncation.

## Acceptance criteria

- [ ] No cron row renders a mathematically false load comparison.
- [ ] Load unavailable is textual/unavailable, never zero and never
      `unavailable > threshold`.
- [ ] Running/pending/queued/retry durations use elapsed/countdown wording, not
      `ago` grammar.
- [ ] Absolute recent-run timestamps are visibly UTC/Z and cannot be mistaken
      for the remote scheduler's local-civil schedule.
- [ ] Stale-warning rows participate in height accounting.
- [ ] Local viewport truncation is explicitly marked and remains distinct from
      remote stdout/stderr truncation.
- [ ] A selected job remains visible with more than `MAX_JOB_ROWS` configured
      jobs.
- [ ] On constrained height, selected-job history has priority sufficient to
      show its header and newest record headline when one exists.
- [ ] No second persistent scroll state, mouse interaction, or unbounded card
      growth is introduced.
- [ ] Sanitization, width clipping, default-five history request, and independent
      `d`/`n`/`c` expansions remain green.
- [ ] Focused renderer/state tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [ ] Active user/architecture/skill documentation matches the corrected
      renderer semantics.

## Stop conditions

Open a separate plan rather than broadening if:

- rendering scheduler-local historical civil time requires a protocol timezone
  field;
- keeping the selected job visible requires a second persistent scroll model;
- a truthful load display requires changing greggd's gate publication contract;
- the correction would redesign the Systems card rather than the cron block.

## Preserved exclusions

- greggd scheduler execution/output behavior (Plan 173);
- client-daemon fetch/cache coherence and concurrency (Plan 174);
- protocol schema/version changes;
- persistent history or history-depth expansion;
- colors/themes/mouse support or general TUI redesign;
- alerting or scheduler mutation controls;
- new dependencies, workflows, jobs, matrices, or release automation.
