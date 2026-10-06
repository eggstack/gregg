# Plan 177: greggd post-exit dual-stream drain progress corrective

Status: complete. See the closure record at the end.

Depends on: completed Plan 173 plus current main at
`892a77195b0a91c7d4e9fb7ee02d560e7ae64c83`.
Independent of Plan 091 and Plans 176/178.

Opened from the 2026-10-05 review of Plan 173's landed two-phase child/output
state machine.

## Objective

Preserve Plan 173's direct-child completion boundary while ensuring the fixed
250 ms post-exit settle can continue draining whichever output stream is ready,
even when a descendant keeps the other stream open but idle.

The scheduler slot must remain bounded by the same settle deadline. This is an
output-completeness corrective, not permission to wait longer for descendants.

## Confirmed edge

Plan 173 correctly changed direct-child completion from "child wait **and both
pipe EOFs**" to:

1. drain stdout/stderr concurrently while the direct child runs;
2. freeze the direct child's exit event;
3. spend at most `POST_EXIT_OUTPUT_SETTLE = 250 ms` on residual output;
4. close the read handles and free the one global scheduler slot.

The post-exit loop currently advances one stdout step and one stderr step with a
`tokio::join!` inside each iteration.

That creates an asymmetric-progress edge. If, after direct-child exit:

- stdout has multiple chunks already buffered; and
- a descendant inherited stderr and keeps that descriptor open without writing,

then the first joined iteration can consume one stdout chunk and park waiting
for the idle stderr read. Stdout cannot issue its next read until stderr either
produces data/EOF or the 250 ms settle deadline wins. The slot still frees on
time, so Plan 173's liveness correction remains valid, but bytes already
available on the active stream can be omitted from the retained tail solely
because the *other* stream stayed open.

That conflicts with the intended settle contract: during the fixed budget,
ready bytes on either stream should make independent progress.

## Required semantics

Keep the two phases distinct.

### While the direct child is running

Do not weaken the existing rule. Both streams must remain concurrently drained
with the child wait so neither pipe can fill and deadlock the child.

The existing full-output/flood regression remains authoritative here.

### After the direct child exits

Advance stdout and stderr **independently** until either:

- both streams reach EOF/read-error/not-piped; or
- the original frozen `settle_deadline` expires.

A pending read on one stream must not prevent another ready stream from
performing additional reads within the remaining settle budget.

Use one task and borrowed streams. Do not spawn drain tasks.

A suitable shape is a small post-exit helper that selects among:

- one `drain_step` for stdout when stdout is not done;
- one `drain_step` for stderr when stderr is not done;
- `sleep_until(settle_deadline)`.

When both streams are continuously ready, make progress fair/deterministic
enough that one stream cannot starve the other for the whole settle. An
alternating preferred branch with `biased` selection is acceptable; an
equivalent explicitly fair state machine is also acceptable.

Cancellation of a read must remain lossless: only bytes returned by a completed
read are folded into the `RunningChild`-owned tail. A rebuilt completion future
must retain:

- the frozen direct-child exit;
- the original settle deadline;
- each stream's done flag;
- both output tails.

## Deterministic test seam

Add a helper-level regression with controlled async readers rather than relying
only on OS pipe scheduling.

The test should establish:

1. stream A has more than one `DRAIN_CHUNK` of immediately readable data;
2. stream B has no readable data and no EOF because its writer remains open;
3. the post-exit helper is given a fixed settle deadline;
4. stream A performs multiple completed reads before the deadline despite B
   remaining pending;
5. the retained tail equals the final bounded bytes from A and its internal
   observed-byte count proves more than one chunk was consumed;
6. the helper still returns at the same settle deadline while B remains open.

Add the mirror case with stdout/stderr reversed.

Retain at least one process-level regression where a descendant holds an
inherited descriptor open so the direct-child/global-slot behavior remains
covered end to end.

## Footprint and scope

No new dependency is expected. Re-measure stripped release `greggd` against a
fresh paired build of the pre-change SHA because this touches the scheduler
state machine tracked by Plans 159/162/173.

Record the delta; do not silently re-baseline Plan 159. Plan 162's 3,400,000-byte
scheduler ceiling remains the current explicit ceiling unless implementation
evidence forces a separate footprint decision.

## Documentation

If helper/state-machine wording changes, reconcile:

- `architecture/greggd-daemon.md`;
- `crates/greggd/README.md`;
- `docs/daemon.md`;
- `.opencode/skills/greggd-daemon/SKILL.md`;
- AGENTS.md.

State precisely that post-exit stdout and stderr progress independently within
one fixed settle budget. Do not claim descendant output is guaranteed or that
greggd waits for descendant EOF.

## Acceptance criteria

- [x] During the post-exit settle, an idle/open stdout cannot block ready stderr
      progress and an idle/open stderr cannot block ready stdout progress.
- [x] Multiple ready chunks on one stream can be consumed while the other stream
      remains pending.
- [x] The original 250 ms settle deadline remains fixed at direct-child exit and
      is never restarted by a wake or per-stream progress.
- [x] Direct-child finish time/duration and global-slot release semantics from
      Plan 173 remain unchanged.
- [x] No spawned drain task, process-group ownership, descendant kill, whole
      output buffer, or `wait_with_output` is introduced.
- [x] Existing flood, cancellation/rebuild, inherited-descriptor, shutdown, and
      carried-index regressions remain green.
- [x] Focused scheduler tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [x] Stripped paired `greggd` footprint is recorded and remains within the
      existing scheduler ceiling or opens a separate explicit decision.
- [x] Active daemon architecture/skill documentation matches the corrected
      independent-progress semantics.

## Stop conditions

Open a separate plan rather than broadening this one if:

- independent progress requires detached tasks that can outlive the scheduler;
- the only workable design extends the 250 ms settle or waits for descendant
  EOF;
- a correct implementation requires process-group/descendant ownership;
- the footprint crosses the existing scheduler ceiling.

## Preserved exclusions

- cron syntax, DST/civil-clock, load gating, retries, max-wait, or coalescing;
- scheduler protocol/schema/routes;
- history retention sizes or persistent output;
- client-daemon/TUI behavior;
- process groups/cgroups/job objects;
- Plan 091 soak evidence;
- new workflows/jobs/matrices.

## Closure record

Output-completeness corrective in `crates/greggd/src/scheduler.rs` plus a test
seam in `scheduler/observation.rs`. No stop condition was hit: progress needed no
detached task, the 250 ms settle is unchanged, greggd still never waits for
descendant EOF, and no process-group or descendant ownership was introduced.

### The edge, restated

Plan 173's post-exit loop advanced one stdout step and one stderr step through a
`tokio::join!` per iteration. With stdout holding several ready chunks and stderr
inherited by a descendant that keeps the descriptor open without writing, the
first iteration consumed one stdout chunk and then parked on the idle stderr read.
Stdout could not issue its next read until stderr produced data, hit EOF, or the
settle deadline won. The slot still freed on time — Plan 173's liveness correction
is untouched — but bytes already available on the active stream were dropped from
the retained tail purely because the *other* stream stayed open.

### One helper, three independent futures

The post-exit loop is now `settle_output`, which selects per iteration among:

- one `drain_step` for stdout, while stdout is not finished;
- one `drain_step` for stderr, while stderr is not finished;
- `sleep_until(settle_deadline)`.

Whichever stream is ready first wins that iteration, so a pending read on one can
never withhold progress from the other. The `biased` preferred branch alternates
each iteration, so when both streams are continuously ready neither starves the
other for the whole settle.

Two details make the guard meaningful rather than decorative:

- **Finished streams are excluded by precondition, not by their return value.**
  `drain_step` returns `false` immediately for a finished stream, so under `biased`
  a completed stream would otherwise be permanently ready and win every iteration,
  starving the live one. Each branch carries `if !*<stream>_done`.
- **Each iteration strictly reduces what remains.** A `drain_step` either folds
  bytes or marks its own stream finished, so the loop cannot spin.

The deadline is passed in rather than recomputed, so neither a wake nor a
per-stream read can extend the bound frozen at the child's exit. Nothing is
spawned; the streams are still borrowed, and only bytes returned by a completed
read are folded into the `RunningChild`-owned tails, so cancelling a losing read
loses nothing. The frozen exit, its status, the settle deadline, both done flags,
and both tails stay on `RunningChild`, so a future rebuilt by a wake retains
them.

### Deterministic helper-level tests

`ScriptedReader` yields a fixed number of `DRAIN_CHUNK` blocks and then EOF;
`IdleOpenReader` is permanently `Pending` and never registers a waker — the shape of
a pipe whose only writer is an idle descendant. `poll_settle_once` drives the
production helper under a `tokio::time::timeout(Duration::ZERO, …)`, the pattern
already used by `a_settle_cancelled_by_a_wake_keeps_the_frozen_exit_and_budget`:
a fresh future dropped one poll later, exactly what a deadline wake does. No
`test-util` feature, no paused clock, and no wall-clock wait.

- `an_idle_stderr_does_not_withhold_ready_stdout_chunks` — stdout's three chunks
  and EOF are consumed in a single poll while stderr never becomes readable, and
  the settle is still pending afterwards. `total_bytes()` proves three chunks, not
  one; the retained tail is the final bounded 512 escaped bytes and is truncated.
- `an_idle_stdout_does_not_withhold_ready_stderr_chunks` — the mirror case.
- `two_ready_streams_both_make_progress_within_the_settle` — both streams reach
  EOF without the deadline being consulted, so alternating preference starves
  neither.
- `a_silent_settle_ends_at_the_frozen_deadline_without_an_eof` — two permanently
  idle streams end the settle at an already-expired deadline and neither reports
  EOF, which is precisely the bound.

Both mirror tests were mutation-tested: restoring the old `join!` implementation
makes them fail (the ready stream cannot get past its first chunk), and they pass
against the new helper. The process-level
`an_inherited_output_writer_cannot_retain_the_global_child_slot`,
`a_settle_cancelled_by_a_wake_keeps_the_frozen_exit_and_budget`,
`shutdown_reaps_a_killed_child_despite_an_inherited_writer`,
`output_flood_stays_bounded_and_never_deadlocks`, and the carried-index
attribution regressions all remain green, so the end-to-end descendant behavior is
still covered.

`RunningChild::output_drained` is now used only by the Unix
inherited-descriptor regression and is marked `#[cfg(all(test, unix))]`; the
settle no longer needs the predicate because it owns the loop that would have
called it. `DRAIN_CHUNK` and `OutputTail::total_bytes` became
`pub(crate)`/`pub(crate)`-visible for the same reason.

That gate was not cosmetic. A plain `#[cfg(test)]` compiled on Linux — where the
Unix regression consumes it — but left the method dead on Windows, and the
existing native Windows Clippy gate failed the build with `-D dead-code` on the
first CI run for this plan (`37408868235`). The narrower gate was confirmed with a
local `--target x86_64-pc-windows-gnu` Clippy cross-run, which is only a fast
development aid: native Windows CI remains the authority for Windows lint status.

### Footprint

Paired stripped release `greggd`, both built from this workspace with
`cargo build --release -p greggd` and `strip`:

| Build | Stripped bytes |
|-------|----------------|
| pre-change (`892a771`) | 3,312,456 |
| post-change | 3,312,688 |
| **delta** | **+232 (+0.007%)** |

Plan 159's baseline is not re-baselined and no separate decision is opened. The
scheduler line stands at 51,024 bytes over the 3,261,664 baseline, and Plan 162's
explicit 3,400,000-byte ceiling is retained with 87,312 bytes of headroom. No new
dependency.

### Verification

20 `greggd` scheduler/observation tests pass, `cargo fmt --all -- --check`,
workspace Clippy with `-D warnings`, workspace tests, and
`./scripts/check-local.sh` are green.

Documentation reconciled: `architecture/greggd-daemon.md`,
`crates/greggd/README.md`, `docs/daemon.md`, `.opencode/skills/greggd-daemon/SKILL.md`,
and `AGENTS.md` now state that stdout and stderr progress independently within one
fixed settle budget, and none of them claims descendant output is guaranteed or
that greggd waits for descendant EOF.
