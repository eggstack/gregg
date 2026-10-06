# Plan 177: greggd post-exit dual-stream drain progress corrective

Status: planned.

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

- [ ] During the post-exit settle, an idle/open stdout cannot block ready stderr
      progress and an idle/open stderr cannot block ready stdout progress.
- [ ] Multiple ready chunks on one stream can be consumed while the other stream
      remains pending.
- [ ] The original 250 ms settle deadline remains fixed at direct-child exit and
      is never restarted by a wake or per-stream progress.
- [ ] Direct-child finish time/duration and global-slot release semantics from
      Plan 173 remain unchanged.
- [ ] No spawned drain task, process-group ownership, descendant kill, whole
      output buffer, or `wait_with_output` is introduced.
- [ ] Existing flood, cancellation/rebuild, inherited-descriptor, shutdown, and
      carried-index regressions remain green.
- [ ] Focused scheduler tests, workspace tests, workspace Clippy, and
      `./scripts/check-local.sh` pass.
- [ ] Stripped paired `greggd` footprint is recorded and remains within the
      existing scheduler ceiling or opens a separate explicit decision.
- [ ] Active daemon architecture/skill documentation matches the corrected
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
