# Plan 179: greggd post-exit settle deadline authority corrective

Status: complete. See the closure record at the end.

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

- [x] A continuously-ready inherited output stream cannot keep
      `settle_output` running after the frozen settle deadline is ready.
- [x] When the deadline and any drain are simultaneously ready, the deadline
      wins.
- [x] Before the deadline, two ready streams still make fair bounded progress.
- [x] The 250 ms deadline is frozen once at direct-child exit and is not reset by
      reads, wakes, or future reconstruction.
- [x] Direct-child timing, carried-index attribution, bounded output tails, and
      one-global-child semantics remain unchanged.
- [x] No drain task is spawned and no descendant/process-group ownership is
      introduced.
- [x] Existing inherited-writer, flood, rebuild, shutdown, and output-bound
      regressions remain green.
- [x] Focused/local/workspace checks pass.
- [x] Paired stripped `greggd` size is recorded and remains under the current
      scheduler ceiling or opens a separate explicit footprint decision.
- [x] Existing six-job CI completes green. Recorded after closure: run
      `37414987171` on `7853743` is green across all six existing jobs — Linux,
      macOS arm64, macOS Intel, Windows (full-workspace Clippy and Test plus the
      release builds and SCM smoke), MSRV Rust 1.89, and FreeBSD `gregg-host`
      native. The workflow is unmodified. See the CI evidence note at the end of
      the closure record.

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

## Closure record

Deadline-authority corrective in `crates/greggd/src/scheduler.rs`. No stop
condition was hit: the direct-child execution boundary, the 250 ms product
decision, output bounds, scheduler protocol/history semantics, and descendant
ownership policy are all unchanged. Nothing is spawned, no drain task exists, and
no process group or descendant kill was introduced. No new dependency, workflow,
or job.

### The deadline is now the first biased branch

Plan 177's landed select was ordered `preferred drain`, `other drain`,
`sleep_until(deadline)`. Alternating the two drain branches removed mutual
starvation, but the timer was still last: a descendant that inherits a descriptor
and keeps writing leaves one `drain_step` ready on *every* poll, so once the timer
became ready the earlier ready drain branch kept winning and the settle continued
for as long as output stayed available. The frozen 250 ms bound was therefore not
authoritative, which is the exact liveness property the bound exists to provide.

The fix is the ordering, and nothing else:

```rust
if prefer_stdout {
    tokio::select! {
        biased;
        () = tokio::time::sleep_until(settle_deadline) => return,
        _ = drain_step(stdout.as_deref_mut(), stdout_tail, stdout_done), if !*stdout_done => {}
        _ = drain_step(stderr.as_deref_mut(), stderr_tail, stderr_done), if !*stderr_done => {}
    }
} else {
    tokio::select! {
        biased;
        () = tokio::time::sleep_until(settle_deadline) => return,
        _ = drain_step(stderr.as_deref_mut(), stderr_tail, stderr_done), if !*stderr_done => {}
        _ = drain_step(stdout.as_deref_mut(), stdout_tail, stdout_done), if !*stdout_done => {}
    }
}
```

Before the deadline the timer is pending, so the alternating preference alone
arbitrates between two simultaneously-ready streams and Plan 177's fairness is
untouched. At or after it the timer wins over either stream, and no further output
read happens however ready that read is. The state machine, the
`drain_step` primitive, the borrowed tails, and the passed-in `settle_deadline`
are all unchanged — the deadline is still never recomputed, so a wake, a read, or
a rebuilt completion future cannot extend it.

**The `Instant::now()` guard was deliberately not added.** The plan allowed one as
defense in depth, but a comparison alone cannot carry the property: the guard can
pass, a drain can then complete, and the next poll can still hand the iteration
to a ready drain. Priority has to come from the select ordering, which is tested
against a timer that is ready in the same poll as the drain. The reasoning is
recorded in the function's doc comment and in `AGENTS.md` so a future agent does
not "simplify" the ordering into a wall-clock check.

### Regressions

Two new helper-level tests plus the two retained Plan-177/Plan-173 cases:

- `a_ready_stream_never_outranks_an_expired_settle_deadline` — a continuously
  ready stdout against an idle open stderr, with the deadline already reached
  before the first poll. Asserts zero post-deadline bytes consumed.
- `no_ready_stream_outranks_an_expired_settle_deadline` — the same with *both*
  streams continuously ready, asserting neither tail moves.
- `two_ready_streams_both_make_progress_within_the_settle` (retained, doc
  extended) is now explicitly the pre-deadline half of the authority claim.
- `a_silent_settle_ends_at_the_frozen_deadline_without_an_eof` (retained) still
  proves an idle inherited writer is bounded without an EOF.
- `a_settle_cancelled_by_a_wake_keeps_the_frozen_exit_and_budget` (retained) still
  proves a rebuilt completion future cannot restart the 250 ms budget.

The deterministic-reader fixture is `ScriptedReader` with `POST_DEADLINE_CHUNKS`
(64) chunks. That budget is finite *on purpose*: a reader that is ready on every
poll and never reports EOF is the descendant shape under test, but with the drains
ordered first it also spins the settle loop forever, because a `Ready` drain never
yields the task back to the timer. A large finite budget keeps the mutation
observable immediately — the wrong ordering drains all 64 chunks instead of
returning at zero — with no wall-clock timeout and no `test-util` and no risk of a
hanging test. This is the deviation from the plan's "many `DRAIN_CHUNK` blocks
without ever becoming pending" phrasing, and it is a strictly stronger
observation than a timeout-based hang inference.

### Mutation-tested

Moving the drain branches back ahead of the timer fails both new tests
immediately and deterministically:

```text
test scheduler::observation_tests::a_ready_stream_never_outranks_an_expired_settle_deadline ... FAILED
test scheduler::observation_tests::no_ready_stream_outranks_an_expired_settle_deadline ... FAILED
  assertion `left == right` failed: no byte may be read after the frozen settle deadline is ready
    left: 262144
   right: 0
```

262,144 bytes is 64 × `DRAIN_CHUNK` — the entire post-deadline budget read past
its bound. The fix was then restored and the observation suite re-run green.

### Footprint

Paired stripped release `greggd` on this host, measured against pre-change main
`fcbe2df`:

```text
before  3,312,688
after   3,312,704
delta   +16 bytes (+0.0005%)
```

Plan 159's 3,261,664-byte baseline is **not** re-baselined and Plan 177's recorded
3,312,688 figure stands unchanged. Plan 162's explicit 3,400,000-byte scheduler
ceiling still holds with 87,296 bytes of headroom, so no separate footprint
decision is opened. No new dependency.

### Verification

```text
cargo test -p greggd --all-features -- settle                    # 6 passed
cargo test -p greggd --all-features -- observation_tests         # 22 passed
cargo fmt --all -- --check                                      # clean
cargo clippy --workspace --all-targets --all-features -- -D warnings   # clean
./scripts/check-local.sh                                        # all checks passed (default mode)
```

The Unix process-level inherited-descriptor regression
(`an_inherited_output_writer_cannot_retain_the_global_child_slot`), the flood
regression, the carried-index attribution regression, the shutdown regression, and
the ordinary-exit regression all remain green, as do all 500 `greggd` tests and
the rest of the workspace (914 `gregg`, 118 + 44 `gregg-protocol`, 62
`gregg-host`, 45 `gregg-update`).

**Not claimed at the time of closure:** a green six-job CI run. This change is
Linux/macOS/Windows source in `greggd`'s scheduler with no platform-specific
code and no protocol, route, or history change, so the default local check plus
the workspace Clippy/test gate was the lightest appropriate mechanism under this
repository's completion rule. The native Windows job remains the platform
authority for Windows lint/test status, and a cross-checked local Windows Clippy
run is not closure evidence.

**Superseded after closure:** CI run `37414987171` on `7853743` is now green
across all six existing jobs — Linux, macOS arm64, macOS Intel, Windows
(full-workspace Clippy and Test, plus the two release builds and the SCM
lifecycle smoke), MSRV Rust 1.89, and FreeBSD `gregg-host` native. The Windows
steps are the native authority this record above was withholding, and they pass
on the commit that carries this plan. The workflow is unmodified. See "CI
evidence" at the end of this record.

### Documentation reconciled

- `architecture/greggd-daemon.md` — the settle section now states that stream
  independence holds *only until* the deadline fires, that the deadline is the
  first biased branch, and why a wall-clock guard is not a substitute.
- `crates/greggd/README.md` — same claim in operator-facing terms.
- `docs/daemon.md` — the user-facing statement that a descendant that keeps
  writing cannot extend the 250 ms.
- `.opencode/skills/greggd-daemon/SKILL.md` — explicit "never reorder the drains
  ahead of the timer" and "never substitute `Instant::now()`" instructions.
- `AGENTS.md` — the deadline-first rule with its reasoning.

### Not reopened

Plans 173, 174, 175, 177, and 178 keep their closure records unchanged. Plan 091
is untouched and remains gated only on its own extended soak record.

### CI evidence (recorded 2026-10-06, after closure)

CI run `37414987171` on `7853743` — <https://github.com/eggstack/gregg/actions/runs/37414987171> —
is green across all six existing jobs:

- Linux (fmt, workspace Clippy `-D warnings`, workspace tests)
- macOS arm64 (`macos-15`)
- macOS Intel (`macos-15-intel`)
- Windows — full-workspace Clippy and Test, both release builds, and the SCM
  lifecycle smoke
- MSRV (Rust 1.89) workspace tests
- FreeBSD `gregg-host` native

The workflow is unmodified; no job, matrix entry, or step was added. The Windows
job is the native authority this plan could not substitute locally, and it is
green on the commit that carries the deadline-first ordering — including the
`-D warnings` gate that Plan 177 once failed on a test-only predicate. The
intermediate commits for Plans 180 and 181 were superseded by later pushes and
are covered by this same green tree, so no separate run is claimed for them.
Later run `37485114828` on the documentation-only record commit `0670165`
confirms this evidence note itself also left `main` green; repeated green runs
are not a standing requirement.
