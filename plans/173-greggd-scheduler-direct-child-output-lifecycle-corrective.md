# Plan 173: greggd scheduler direct-child output lifecycle corrective

Status: planned.

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

- [ ] An inherited stdout/stderr writer cannot retain greggd's global scheduler
      slot after the scheduled direct child has exited beyond the fixed
      post-exit settle bound.
- [ ] Output still drains concurrently while the child is alive; output-flood
      and truncation tests remain green.
- [ ] Execution finish time and duration are measured at the direct-child wait
      result and are not inflated by post-exit output cleanup.
- [ ] A following due job can launch after the direct-child completion boundary.
- [ ] Terminal attribution uses the carried job index and has no index-zero
      fallback.
- [ ] Scheduler shutdown remains bounded at two seconds and no drain task/thread
      can outlive the scheduler.
- [ ] No process-group killing, shell execution, persistent history, new API
      mutation, or scheduler concurrency expansion is introduced.
- [ ] Focused scheduler tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [ ] Relevant active architecture/skill documentation is reconciled with the
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
