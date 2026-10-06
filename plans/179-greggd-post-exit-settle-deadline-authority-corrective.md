# Plan 179: greggd post-exit settle deadline authority corrective

Status: planned.

Depends on: completed Plan 177 plus current main at
`fcbe2dfb129e0327dc0499d6666e54e1ac310fa8`.
Independent of Plan 091 and Plans 180-181.

Opened from the 2026-10-06 review of Plan 177's landed independent-stream
post-exit drain.

## Objective

Make Plan 173's fixed `POST_EXIT_OUTPUT_SETTLE = 250 ms` deadline authoritative
even when one or both inherited output streams remain continuously readable
after the scheduled direct child exits.

Plan 177 correctly removed stdout/stderr mutual starvation. This corrective
closes the remaining timer-starvation edge without changing the direct-child
execution boundary, output bounds, or descendant-ownership policy.

## Confirmed defect

The landed `settle_output` alternates which output stream is preferred, but each
iteration is a **biased** select ordered as:

~~~text
preferred drain_step
other drain_step
sleep_until(settle_deadline)
~~~

The alternation prevents one ready stream from starving the other. It does not
prevent a ready stream from starving the deadline.

If a descendant inherits stdout or stderr and continuously writes after the
scheduled direct child has already exited, at least one drain future can remain
ready on every poll. Once the 250 ms timer is also ready, the biased select still
chooses the earlier ready drain branch. The loop can therefore continue reading
after the frozen deadline for as long as output remains continuously available.

That violates the Plan-173/177 execution contract:

> the direct child's exit freezes one fixed post-exit budget; descendant output
> may be sampled during that budget but cannot extend ownership of the one
> global scheduler slot.

The existing idle-writer regressions do not cover a continuously-ready writer,
so CI can remain green while this bound is false.

## Required correction

Keep the current independent-stream state machine and alternating stream
preference, but make the deadline dominant once it becomes ready.

Preferred shape:

~~~text
loop:
    if both streams done:
        return

    biased select:
        sleep_until(frozen_deadline) -> return
        preferred unfinished stream -> drain one step
        other unfinished stream -> drain one step

    alternate preferred stream
~~~

The deadline must be the first branch of the biased select. An explicit
`Instant::now() >= settle_deadline` guard may also be used as defense in depth,
but it must not replace a deadline-first select in a way that allows a
continuously-ready drain to win indefinitely when the timer becomes ready
between the guard and the select.

Before the deadline, the timer is pending and the existing alternating
preference still controls two simultaneously-ready streams. At or after the
deadline, the timer wins immediately over either stream.

Do not restart or recompute the deadline after a read.

## Deterministic regressions

Add helper-level readers whose readiness is under the test's control. At
minimum cover:

1. **Expired deadline vs continuously-ready stdout.**
   - stdout can return many `DRAIN_CHUNK` blocks without ever becoming pending;
   - stderr is idle/open or absent;
   - the settle deadline is already reached when `settle_output` is first
     polled;
   - zero post-deadline stdout bytes are consumed.

2. **Expired deadline vs both continuously-ready streams.**
   - both drains are immediately ready;
   - the deadline still wins before either branch;
   - neither tail changes.

3. **Pre-deadline fairness remains intact.**
   - both streams have a finite set of immediately-ready chunks;
   - with a future deadline, both make progress and reach EOF;
   - alternating preference still prevents stream starvation.

4. **Idle inherited writer remains bounded.**
   - retain Plan 177's silent/open reader case and prove the helper returns when
     the frozen deadline fires without observing EOF.

5. **Rebuilt completion future keeps the same deadline.**
   - retain the Plan-173 cancellation/rebuild regression so a scheduler wake
     cannot grant another 250 ms.

The first test should fail against the current drain-first branch ordering. Do
not rely on an unbounded writer plus a wall-clock timeout merely to infer that
the loop might hang; use an already-ready deadline and deterministic reader so
the wrong branch ordering is observed immediately.

Keep the existing Unix process-level inherited-descriptor regression. If useful,
add one bounded process-level writer regression, but helper-level proof is the
authoritative way to test branch priority without creating runaway child
processes.

## Timing semantics

The hard semantic boundary remains:

- `finished_unix_ms` and `duration_ms` describe the direct child's exit;
- the scheduler may spend **up to** the fixed settle budget collecting residual
  pipe data;
- descendant EOF is never required;
- once the deadline is ready, no further output read may win merely because it
  is also ready;
- after settlement, pipe handles are dropped and the one global child slot is
  released.

Normal executor scheduling overhead after a timer wakes is not a new published
duration and does not change the direct-child timestamp. The requirement is
about state-machine ownership, not a nanosecond wall-clock SLA.

## Footprint and verification

No new dependency is expected.

Because this changes the scheduler completion loop, record a paired stripped
release `greggd` measurement against current pre-change main. Do not rewrite
Plan 159's baseline. Plan 162's explicit 3,400,000-byte scheduler ceiling remains
the bound unless implementation evidence forces a separate decision.

Run:

- focused scheduler/observation tests;
- `cargo fmt --all -- --check`;
- workspace Clippy with `-D warnings`;
- workspace tests;
- `./scripts/check-local.sh`;
- ordinary existing CI, with native Windows Clippy/Test retained as the
  cross-platform compile/lint authority.

No new workflow or job.

## Documentation

Reconcile active wording in:

- `architecture/greggd-daemon.md`;
- `crates/greggd/README.md`;
- `docs/daemon.md`;
- `.opencode/skills/greggd-daemon/SKILL.md`;
- `AGENTS.md`.

The docs should say that the two streams progress independently **only until**
the fixed settle deadline, and that the deadline outranks output readiness once
it fires.

## Acceptance criteria

- [ ] A continuously-ready inherited output stream cannot keep
      `settle_output` running after the frozen settle deadline is ready.
- [ ] When the deadline and any drain are simultaneously ready, the deadline
      wins.
- [ ] Before the deadline, two ready streams still make fair bounded progress.
- [ ] The 250 ms deadline is frozen once at direct-child exit and is not reset by
      reads, wakes, or future reconstruction.
- [ ] Direct-child timing, carried-index attribution, bounded output tails, and
      one-global-child semantics remain unchanged.
- [ ] No drain task is spawned and no descendant/process-group ownership is
      introduced.
- [ ] Existing inherited-writer, flood, rebuild, shutdown, and output-bound
      regressions remain green.
- [ ] Focused/local/workspace checks pass.
- [ ] Paired stripped `greggd` size is recorded and remains under the current
      scheduler ceiling or opens a separate explicit footprint decision.
- [ ] Existing six-job CI completes green.

## Stop conditions

Open a separate plan rather than broadening this one if enforcing the deadline
requires:

- killing or owning descendant processes;
- replacing bounded tails with whole-output buffering;
- changing the 250 ms product decision;
- changing scheduler protocol/history semantics; or
- crossing the existing scheduler footprint ceiling.

## Preserved exclusions

- cron syntax, wall-clock reconciliation, load gates, retry/max-wait/coalescing;
- scheduler protocol/routes/history retention;
- client-daemon/TUI behavior;
- persistent output;
- process groups/cgroups/Windows job objects;
- Plan 091 soak evidence;
- new CI infrastructure.
