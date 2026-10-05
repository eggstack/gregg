# Plan 175: Gregg cron TUI truthfulness and bounded-layout corrective

Status: complete. Code at `PLAN175SHA`; see the closure record at the end.

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

- [x] No cron row renders a mathematically false load comparison.
- [x] Load unavailable is textual/unavailable, never zero and never
      `unavailable > threshold`.
- [x] Running/pending/queued/retry durations use elapsed/countdown wording, not
      `ago` grammar.
- [x] Absolute recent-run timestamps are visibly UTC/Z and cannot be mistaken
      for the remote scheduler's local-civil schedule.
- [x] Stale-warning rows participate in height accounting.
- [x] Local viewport truncation is explicitly marked and remains distinct from
      remote stdout/stderr truncation.
- [x] A selected job remains visible with more than `MAX_JOB_ROWS` configured
      jobs.
- [x] On constrained height, selected-job history has priority sufficient to
      show its header and newest record headline when one exists.
- [x] No second persistent scroll state, mouse interaction, or unbounded card
      growth is introduced.
- [x] Sanitization, width clipping, default-five history request, and independent
      `d`/`n`/`c` expansions remain green.
- [x] Focused renderer/state tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [x] Active user/architecture/skill documentation matches the corrected
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

## Closure record

Renderer-only, in `crates/gregg/src/ui/cron.rs`. No scheduler execution, protocol
type, polling ownership, or retention policy changed, and no stop condition was
hit: no protocol timezone field was added, no second scroll model was introduced,
`greggd`'s gate publication contract is untouched, and the Systems card was not
redesigned.

### Finding 1 — the load relation, by meaning

`load_gate_label` matches on `(job.state, gate.observed)` instead of formatting
every gate as `observed > threshold`:

- `LoadHigh` with a reading — the only state where it really is above — prints
  `load15m 9.24 > 8.00`;
- `LoadUnavailable` prints `load15m unavailable (max 8.00)`;
- `Running` prints `start load15m 1.20 <= 8.00`, naming the gate it was admitted
  under, which is why the retained reading can legitimately be *below* the
  threshold;
- `Idle` and `WaitingForSlot` print `last gate load15m 1.20 <= 8.00`, so an old
  observation is never presented as current live load;
- a missing reading in *any* state prints `unavailable` and no comparison. The
  old `— > 8.00` was not a comparison at all;
- a time-only job still has no load token.

The retry context stays a separate fact about the job and is still the first
thing dropped on a narrow terminal.

### Finding 2 — elapsed grammar, split from countdown grammar

`age()` is gone. `elapsed_since` and `countdown_to` both saturate at `0ms`, so a
remote clock ahead — or a transition captured before its own start — degrades
conservatively instead of underflowing a `u64`. Rows now read `next 11h00m`,
`for 3m00s`, `pending 17m00s`, `queued 2m00s`, `retry 20.0s`, and never
`running for 3m ago`.

### Finding 3 — the clock is labelled UTC

`clock_label` emits `10-05 07:00Z`. These fields are UTC instants and the
schedule column beside them is the remote's local civil cron, so an unlabelled
`07:00` next to a `03:00` schedule read as a contradiction with nothing to
explain it. The protocol carries no remote timezone history, so the fix is the
label; reconstructing scheduler-local civil time stays a separate protocol
change, as the plan requires.

### Finding 4 and 5 — one row builder, one reserved section, one window

`block_rows` builds the block once as ordered groups — `head` (CRON header plus
the stale notice), `reserved` (the selected job's header and its **newest**
record), `table` (the job window), `rest` (older records) — and both
`desired_rows` and `render` read it. That closes the accounting defect
structurally: the stale-scheduler row is counted because it is *in* the list, not
because a separate estimate remembered it.

`render` reserves one row for `… more cron rows not shown` whenever the rows do
not fit, and spends what is left on the job window *around the selection*:
`job_window` centres on the selected job and clamps at both ends, so `Shift-J`
past the old 24-row prefix can no longer make the highlighted row vanish, and
hidden jobs are counted above and below. When the block is truncated the table
budget also reserves the window's two marker rows — without that, a selection at
the window's edge landed on the last visible line and was cut by the very
truncation it should have survived. That was caught by the 60-job boundary test.

On a short terminal the selected job's header and newest record are drawn before
any job row, which is what makes pressing `c` useful at 20 rows.

### What the reorder cost, and what it did not

Reserving the selected section moves the job table below it, so one existing
test's "the job row is the second line" assumption had to become "the job row is
the first table line". Its actual claim — that the name and state survive a
narrow cut while the gate detail is dropped — is unchanged and still asserted.
No second scroll state, scrollbar, mouse handling, or unbounded growth was added.

### Evidence

- `cargo test -p gregg --all-features -- ui::cron` — **47 passed, 0 failed**
  (31 before; 16 new), including:
  - load: `a_load_high_row_is_the_only_one_that_compares_above_the_threshold`,
    `a_running_row_reports_the_gate_that_admitted_it_below_the_threshold`,
    `an_idle_row_says_last_gate_so_an_old_reading_is_not_current_load`,
    `load_unavailable_is_never_a_numeric_comparison_in_any_state` (asserted
    across all five states), `a_time_only_job_has_no_load_token_at_all`
  - grammar/skew/clock: `elapsed_states_use_elapsed_wording_and_never_ago`,
    `clock_skew_degrades_to_zero_rather_than_wrapping`,
    `a_record_clock_is_labelled_utc_and_cannot_be_read_as_local_civil`
  - layout: `the_stale_notice_is_part_of_the_requested_height`,
    `a_short_budget_ends_with_an_explicit_truncation_marker`,
    `an_exact_fit_draws_everything_and_no_marker`,
    `the_truncation_marker_is_not_confused_with_remote_stream_truncation`,
    `the_selected_job_stays_visible_across_the_old_first_slice_boundary` (60 jobs,
    selection before/inside/after the old 24 boundary),
    `omitted_jobs_are_counted_above_and_below`,
    `navigation_moves_the_window_with_the_selection`,
    `a_short_terminal_prioritises_the_selected_job_over_unrelated_job_rows`
- Unchanged regressions still green, including
  `a_narrow_terminal_drops_the_tail_rather_than_wrapping_or_overflowing`,
  `remote_command_output_cannot_reach_the_terminal_as_control_sequences`,
  `remote_truncation_is_reported_and_is_not_the_same_as_a_viewport_cut`,
  `wide_glyphs_are_measured_in_cells_not_bytes`,
  `the_job_table_is_capped_so_the_selected_history_still_has_rows`,
  `a_zero_row_budget_draws_nothing_rather_than_indexing_outside_the_buffer`,
  `the_condensed_view_draws_the_cron_block_rather_than_reserving_a_gap`, and the
  `d`/`n`/`c` independence tests in `state.rs`.
- `cargo test -p gregg --all-features` — **894 passed, 0 failed** (was 878).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean.
- `./scripts/check-local.sh` — `=== all checks passed (mode: default) ===`.

### Documentation

`architecture/gregg-client.md` states the relation-by-meaning rule, the
elapsed/countdown split with the skew boundary, why the clock is labelled `Z`
and why that is not a timezone feature, the three distinct truncation markers,
and the group order with the reserved section and selection-centred window.
`README.md`, `crates/gregg/README.md`, AGENTS.md, and the gregg-client skill
carry the same invariants; the skill's cron rules grew from three to ten.
