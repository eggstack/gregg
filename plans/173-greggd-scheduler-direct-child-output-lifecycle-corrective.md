# Plan 173: greggd scheduler direct-child output lifecycle corrective

Status: complete. Code at `56d43d5`; see the closure record at the end.

Depends on: completed Plans 163 and 167 plus current main at
`1aac89f1a82fcd347066379b55105e2ff6bfe770`. Independent of Plan 091 and of
the Gregg client/TUI corrections in Plans 174-175.

Opened from the 2026-10-05 post-closure review of the cron observability line.
This plan does not reopen the scheduler architecture or the Plan-159 footprint
decision; it corrects one direct-child lifecycle defect exposed by adding output
capture.

## Objective

Keep scheduler history output capture truthful without letting a descendant
that merely inherited stdout/stderr keep greggd's single scheduled-child slot
occupied after the scheduled direct child has already terminated.

The execution boundary remains the direct child. Output observation must follow
that boundary rather than silently extending the job lifetime to "all inherited
pipe writers have closed".

## Defect

`await_child_completion` currently joins three futures:

~~~rust
tokio::join!(
    child.child.wait(),
    drain_into(child.stdout.as_mut(), &mut child.stdout_tail),
    drain_into(child.stderr.as_mut(), &mut child.stderr_tail),
)
~~~

That prevents the ordinary full-pipe deadlock because stdout and stderr drain
while the direct child runs. It also means completion waits for **EOF on both
pipes**, not merely for the scheduled direct child to exit.

A scheduled command can spawn a descendant that inherits fd 1 or 2 and then
exit. The direct child is terminal, but the descendant still owns a write end,
so the drain waits for EOF and `active` remains occupied. Because greggd has one
global scheduled-child slot, every later maintenance job is then blocked by a
process that greggd did not schedule as its active child.

This is a regression in scheduler semantics introduced by observability:
before Plan 163, stdout/stderr were null and an inherited output descriptor
could not extend the direct child's lifetime.

A second, smaller correctness smell is in terminal attribution: the running
child already carries its authoritative configuration index, but the completion
path searches by job name and falls back to index zero if the search fails.
That fallback must disappear.

## Required execution semantics

Preserve all settled scheduler semantics from Plans 155-163:

- one pending occurrence per job and one global direct-child slot;
- direct argv execution, no implicit shell, same-principal execution;
- monotonic retry/max-wait/child duration bookkeeping;
- bounded memory-only per-job history;
- concurrent stdout/stderr draining while the direct child is alive;
- no whole-output accumulation and no `wait_with_output`;
- the two-second scheduler shutdown bound;
- no process-group/session ownership expansion and no attempt to kill arbitrary
  descendants merely because they inherited an output descriptor.

The **direct child's wait result is the terminal execution event**. Record
`finished_unix_ms` and child duration from that event, not from a later pipe
EOF.

After the direct child reaches a terminal wait result, greggd may spend one
small fixed, non-configurable grace period draining bytes already in flight.
Use a dedicated constant no greater than 250 ms unless implementation evidence
shows a smaller bound is not reliable. At the bound:

- stop waiting for pipe EOF;
- preserve every byte already folded into the fixed-capacity tails;
- close/drop the read handles;
- finalize the terminal record and free the global slot.

A descendant that writes after that boundary is outside greggd's direct-child
execution/history contract. Do not block the scheduler to capture it.

The normal running path must still continuously drain both streams. This
corrective is **not** permission to wait for the child first and then read the
pipes, which would reintroduce the classic pipe-buffer deadlock.

## Implementation shape

Prefer an explicit child/output state machine over detached drain tasks:

1. While the child is running, poll the direct-child wait and both bounded pipe
   drains concurrently, retaining the existing `RunningChild`-owned tails
   across civil-clock reconciliation wakes.
2. When the direct-child wait resolves, freeze the execution finish instant and
   exit status.
3. Continue bounded nonblocking/async drain progress for the short post-exit
   settle interval, ending earlier if both streams reach EOF.
4. At EOF or the settle deadline, finalize the existing tails and publish the
   terminal record.
5. Use `RunningChild.index` directly for terminal attribution. Remove the
   job-name search and `unwrap_or(0)` fallback.

Do not spawn drain tasks that can outlive the scheduler. If the implementation
uses helper futures, cancellation at the minute reconciliation wake must retain
the already-accumulated tails exactly as Plan 163 requires.

## Deterministic regressions

Add focused tests that prove:

- a direct child can spawn/background a descendant that inherits stdout or
  stderr, exit successfully, and release the scheduler slot within the
  post-exit bound even though the inherited write end remains open;
- a second due job can start after that direct-child exit instead of waiting for
  the descendant;
- child duration and `finished_unix_ms` describe direct-child termination, not
  the post-exit drain grace;
- ordinary stdout and stderr are still captured when the child exits normally;
- output floods remain bounded and retain the qualified tail/truncation policy;
- the existing cancelled/rebuilt drain-future regression remains green;
- shutdown with a running child still respects the established two-second bound;
- terminal records are attributed through the carried index, with no
  name-search fallback.

On Unix, a shell helper is acceptable only inside the test to create the
inherited-descriptor condition. Reap/terminate the helper descendant so the test
does not leak processes. Production execution remains direct argv/no shell.

## Documentation and footprint

If the implementation changes the precise output-lifetime wording, update
`architecture/greggd-daemon.md`, the greggd skill, and AGENTS.md in the same
pass. Document that output belongs to the scheduled direct child and is bounded
after its exit; do not claim greggd captures arbitrary descendant output.

Re-run the stripped greggd measurement used by Plans 159/162 if this corrective
adds linked code. The Plan-159 3,261,664-byte scheduler baseline remains the
comparison point for scheduler-line growth; a material increase requires an
explicit record rather than silent re-baselining. No new dependency is expected.

## Acceptance criteria

- [x] An inherited stdout/stderr writer cannot retain greggd's global scheduler
      slot after the scheduled direct child has exited beyond the fixed
      post-exit settle bound.
- [x] Output still drains concurrently while the child is alive; output-flood
      and truncation tests remain green.
- [x] Execution finish time and duration are measured at the direct-child wait
      result and are not inflated by post-exit output cleanup.
- [x] A following due job can launch after the direct-child completion boundary.
- [x] Terminal attribution uses the carried job index and has no index-zero
      fallback.
- [x] Scheduler shutdown remains bounded at two seconds and no drain task/thread
      can outlive the scheduler.
- [x] No process-group killing, shell execution, persistent history, new API
      mutation, or scheduler concurrency expansion is introduced.
- [x] Focused scheduler tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [x] Relevant active architecture/skill documentation is reconciled with the
      corrected direct-child/output boundary.

## Stop conditions

Open a separate follow-up rather than broadening this plan if:

- reliably ending an inherited-pipe wait requires process-group ownership or
  descendant termination semantics;
- the only viable design requires detached output tasks that can outlive the
  scheduler;
- the correction materially changes the scheduler's one-child execution model;
- the stripped greggd growth is large enough to reopen the Plan-159 footprint
  decision.

## Preserved exclusions

- cron syntax, aliases, local-civil/DST calculation, load gating, retry/max-wait,
  or coalescing changes;
- protocol schema or scheduler HTTP route changes;
- persistent output/history storage;
- remote scheduler mutation;
- process trees, process groups, cgroups/job objects, or descendant management;
- changes to Gregg client-daemon/TUI behavior (Plans 174-175);
- new workflows, jobs, matrices, or release automation;
- the unrelated current Windows EggPool test failure.

## Closure record

Implemented as an explicit two-phase child/output state machine in
`crates/greggd/src/scheduler.rs`, with the bounded read primitive in
`crates/greggd/src/scheduler/observation.rs`.

### Phase 1 — the direct child is running

`await_child_completion` polls the direct-child wait and **both** drains
concurrently on the same task. The join lives inside the non-waiting branch of
the select, not as a sequence:

```rust
tokio::select! {
    status = &mut wait => Some(status),
    () = async {
        let _ = tokio::join!(&mut stdout, &mut stderr);
    } => None,
}
```

Draining stdout to EOF *before* touching stderr is the same deadlock as awaiting
`wait()` first — just wearing a different hat: a child blocked writing to a full
second pipe can never reach the EOF that ends the first. The existing
`output_flood_stays_bounded_and_never_deadlocks` regression caught that mistake
during this pass, which is why the suite is worth keeping.

### Phase 2 — the direct child has exited

`RunningChild::freeze_exit` records the terminal execution event and its settle
deadline at the instant the wait result resolves, and the existing civil-clock
reconciliation wake rebuilds the completion future against them, so a rebuilt
future cannot re-stamp the finish time or restart the settle budget. The settle
loop then spends at most `POST_EXIT_OUTPUT_SETTLE` (250 ms) making one
non-blocking read per open stream per iteration, ending the moment both streams
reach EOF. At the bound, greggd drops the read handles, takes the tails, and
returns.

Nothing new is spawned and nothing beyond the direct child is killed: a
descendant that still holds an inherited write end gets `EPIPE` after the
boundary, and greggd never reports output it did not wait for. No stop condition
was hit — no process-group ownership, no descendant termination, no detached
output task, and no change to the one-child execution model.

`drain_into` is now a loop over a new `drain_step` primitive, so the read,
tail-fold, EOF bookkeeping, and read-error handling exist once instead of twice.
Per-stream EOF is now state (`stdout_done`/`stderr_done` on the child) rather
than a property of a future a wake can drop, which is what lets a rebuilt drain
skip a pipe it has already seen.

### Attribution

`finish_child` takes `&RunningChild` and records through `child.index`. The
`jobs.iter().position(|job| job.name == child.job_name).unwrap_or(0)` search and
its index-zero fallback are gone; a duplicate name cannot move a record onto a
sibling.

### Evidence

- `cargo test -p greggd --lib -- scheduler` — **76 passed, 0 failed** in 0.52s.
  New cases:
  - `an_inherited_output_writer_cannot_retain_the_global_child_slot` — a `/bin/sh`
    helper backgrounds a 30-second descendant that inherits stdout, writes the
    descendant's pid to a file, prints its own output, and exits 0. The
    completion returns after the settle bound, far inside the 15s guard, while
    the descendant's write end is still open; the recorded duration is asserted
    to be *under* the settle bound so the grace cannot be charged to the child.
    The pid file is then terminated via `/bin/kill` so the helper leaves no
    process behind, and the descendant's own 30s bound is a backstop if that
    fails.
  - `a_following_job_starts_after_the_direct_child_completion_boundary` — the
    next job's child starts and completes after the first completion, with the
    first descendant still holding the previous pipes.
  - `a_terminal_record_is_attributed_through_the_carried_index` — two configured
    jobs share a name; the record lands on index 1 with a window matching the
    direct child's own duration (±1 ms of wall/monotonic rounding), and index 0
    stays empty.
  - `a_settle_cancelled_by_a_wake_keeps_the_frozen_exit_and_budget` — polling the
    production future under a zero timeout is exactly what a deadline wake does,
    so the cancellation lands inside the settle by construction instead of by
    timing. The settle deadline stays pinned to the frozen exit, the frozen
    `finished_unix_ms` survives, and the rebuilt future reports the same exit.
  - `shutdown_reaps_a_killed_child_despite_an_inherited_writer` — the killed
    direct child is reaped inside `CHILD_SHUTDOWN_BOUND` with a descendant
    holding its output descriptors.
  - `an_ordinary_exit_is_captured_without_spending_the_settle_bound` — both
    streams captured, untruncated, pipe EOF reached, and no settle charged on the
    common path.
- Unchanged regressions still green: `output_flood_stays_bounded_and_never_deadlocks`
  (30s bound, completes in well under), `output_written_before_a_deadline_wake_still_reaches_the_record`,
  `a_cancelled_drain_keeps_what_it_already_folded_in`, `disabled_capture_keeps_the_original_null_streams`.
- `cargo test -p greggd --lib -- scheduler::observation` — drain EOF/done-flag
  semantics including `a_drain_step_on_a_finished_stream_makes_no_progress`.
- `cargo clippy -p greggd --all-targets --all-features -- -D warnings` — clean.
- `./scripts/check-local.sh` — `=== all checks passed (mode: default) ===`
  (fmt check, workspace Clippy, workspace tests across all crates).

### Footprint

Stripped release `greggd`, measured the same way Plans 159/162 measured, by
stashing only this corrective's source and rebuilding both sides:

- before this corrective: **3,307,360 bytes**
- after: **3,312,464 bytes**
- delta: **+5,104 bytes (+0.154%)**

Against the Plan-159 scheduler baseline of 3,261,664 bytes, the scheduler line
now sits at 3,312,464 (+50,800 / +1.556%), which is under Plan 162's 3,400,000
ceiling. This is recorded rather than re-baselined.

Note the local absolute figure differs from the 3,316,568 recorded in Plans
169/171: this toolchain (`rustc 1.99.0`) produces a ~9 KB smaller binary for the
*identical* pre-change source, which is why the delta above is measured against a
freshly built local baseline instead of the recorded number. The recorded
history is left untouched.

### Documentation

`architecture/greggd-daemon.md`, `crates/greggd/README.md`, `docs/daemon.md`,
`.opencode/skills/greggd-daemon/SKILL.md`, and `AGENTS.md` now state that output
belongs to the scheduled direct child, that the wait result is the terminal
execution event, that capture is bounded after the exit by a fixed 250 ms settle,
and that greggd does not track or kill descendants. The daemon skill also records
the "never drain one stream to EOF before the other" half of the concurrency
rule, which the earlier wording left implicit.

### CI

Existing workflow run `37371100771` on this line's final SHA: macOS arm64
**success**, macOS Intel **success**, MSRV (Rust 1.89) **success**, FreeBSD
(`gregg-host` native) **success**. Linux was **cancelled** after 15 minutes by the
run's own fail-fast once Windows finished; it was not a test failure, and the
same job was **success** on this line's earlier commits in runs `37367547505`
(Plan 173) and `37370062234` (Plan 174), whose only difference from this SHA is
the cron renderer and documentation.

The Windows job is red for exactly one test,
`eggpool::tests::worker_cancellation_wins_over_a_full_result_channel`
("the channel must be full for this to test anything",
`crates\gregg\src\eggpool.rs:1887`), which is the unrelated pre-existing
failure these plans list under preserved exclusions: it is present in run
`37350010528` on the base commit before this line, and `eggpool.rs` was last
touched by commit `1aac89f`, which predates all three plans. Everything else on
Windows is green — 846 passed, 1 failed, up from 816 passed on the base run.
No plan here changed EggPool behaviour, and no plan was allowed to widen into it.


## Follow-up correction note (2026-10-05, Plan 177)

A later review of the landed two-phase drain found one bounded
output-completeness edge that does not invalidate this plan's direct-child
liveness correction. During the fixed post-exit settle, stdout and stderr
currently advance one `drain_step` each through a joined pair. If one stream is
held open but idle by a descendant while the other has multiple chunks ready,
the ready stream can consume one chunk and then wait behind the idle read until
the settle deadline. The scheduler slot still releases at the correct fixed
deadline, but bytes already available on the active stream can be omitted.

Plan 177 owns the correction: keep the same frozen 250 ms deadline and
single-task/borrowed-stream model, but let ready post-exit streams make
independent progress. This plan's closure evidence, direct-child timing
semantics, no-descendant-ownership boundary, and footprint record remain
historical facts.
