# Plan 158: Scheduler footprint and schedule-validation corrective pass

Status: complete at the Plan-159 decision. Implemented at `7a466f8`
(schedule validation, allocation-free selection, borrowed config, 366-day
fallback removal); the footprint gate it could not meet was superseded for
this line by Plan 159's re-baseline — see the closure note appended below.

Depends on: the current Plan-157 implementation on main (`308383cc5084683a326c481e89e83f4a2414b7db`) and completed Plan 156. Independent of the remaining Plan 091 soak record.

## Objective

Close the two remaining defects in the load-aware maintenance scheduler line without reopening its product architecture:

1. the integrated `greggd` release binary still exceeds Plan 156's hard footprint budget; and
2. configuration validates cron syntax but does not reject a syntactically valid schedule that can never occur, so an impossible expression can survive config loading and fail later when the scheduler initializes.

Once both defects are corrected, rerun the scheduler qualification, reconcile Plan 157's acceptance record, and close the Plan-155 roadmap only if the evidence is complete.

This is a corrective/closure pass. Do not broaden the scheduler into a more capable cron implementation, add concurrency, add persistence, or weaken any security/lifecycle contract to meet the size target.

## Current baseline

The merged scheduler tree is PR #1 / merge `308383cc5084683a326c481e89e83f4a2414b7db`, identical in content to tested PR head `4f820b97e4d57b7a9e7ced2c9290fe8705146caf`.

Existing CI run `37167983603` passed all six current jobs for that tested tree:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows including the SCM lifecycle smoke;
- MSRV Rust 1.89;
- FreeBSD 14.2 native `gregg-host`.

The functional architecture is therefore the baseline to preserve.

Plan 156's stripped Linux release baseline was:

~~~text
pre-scheduler greggd     3,097,200 bytes
~~~

Plan 157's current integrated result is:

~~~text
current greggd           3,272,456 bytes
delta                      175,256 bytes  (+5.6585%)
5% ceiling               3,252,060 bytes
128 KiB ceiling          3,228,272 bytes
required reduction          44,184 bytes  (to satisfy the stricter 128 KiB gate)
~~~

Both Plan-156 gates remain authoritative: no more than 5% growth **and** no more than 128 KiB absolute growth. The 128 KiB bound is the stricter current constraint.

The first integrated implementation already removed the production `cron-parser` dependency and replaced it with Gregg's bounded five-field bitmask parser. That reduced the binary but did not close the gate. Do not reintroduce `cron-parser`.

## Governing invariants

Preserve all settled Plans 155-157 behavior:

- direct argv execution only;
- optional `working_dir`;
- no implicit shell;
- no environment/secret store;
- same-principal execution;
- explicit Unix euid-0 opt-in;
- canonical systemd/launchd/SCM sandboxing unchanged;
- Windows time-only jobs allowed and load-gated jobs rejected;
- cached sampler load only; no new telemetry probe and no HTTP self-poll;
- 1m/5m/15m inclusive load thresholds;
- warming/failed/missing load fails closed;
- one pending occurrence per configured job;
- one global scheduled child;
- fixed retry plus bounded max wait;
- oldest-pending/config-order deterministic fairness;
- fresh load recheck between sequential load-gated jobs;
- no command retry after nonzero exit;
- no missed-job replay after daemon downtime;
- direct-child shutdown only;
- no remote command/control endpoint;
- no persistent scheduler queue;
- no protocol change.

The workspace-wide `unsafe_code = "deny"` policy is also invariant. Do not waive it to hand-write local-time FFI.

## Workstream A: reproduce and attribute the production footprint

Before changing runtime behavior, reproduce the current release size from a clean build using the same profile Plan 156 used:

~~~text
lto = "fat"
codegen-units = 1
strip = "symbols"
panic = "abort"
~~~

Record:

- exact current `target/release/greggd` bytes;
- exact delta from 3,097,200 bytes;
- `cargo tree -p greggd -e features`;
- direct scheduler-related dependency/features: `chrono` with `clock`, Tokio `process`, and their target-relevant transitive dependencies.

Use symbol/crate attribution if available (`cargo bloat`, `llvm-size`, `nm`, or equivalent), but do not add an analysis tool as a repository dependency or CI requirement.

The purpose is to identify where the remaining >44 KiB is linked, not to infer it from source line count.

### Controlled measurements

Where useful, use temporary local candidate patches to measure these components independently:

1. current scheduler code with the process path stubbed but cron/local-time code linked;
2. current scheduler code with a fixed/test wall-time seam but process path linked;
3. current production code with obvious scheduler allocation/cloning reductions;
4. alternative local-time backend candidates only if the low-risk reductions cannot close the gate.

Temporary measurement patches must not be left in main unless they are the selected correction.

## Workstream B: remove avoidable scheduler code/allocation surface first

Before replacing the time backend, simplify the existing state machine where the same semantics can be expressed with less code and less allocation.

At minimum evaluate these concrete opportunities.

### B1. Eliminate the per-tick candidate Vec + sort

Current `Engine::tick` builds a `Vec<(index, pending_since)>` and sorts it on every launch decision.

Replace it with a single bounded scan over at most 64 jobs that:

- updates due/coalesced/expired state;
- evaluates retry eligibility;
- applies load deferral and updates `retry_at`;
- tracks the oldest launchable candidate;
- uses config index as the stable tie breaker.

The resulting selection must preserve the current important behavior that an older load-blocked job does not prevent a younger time-only or load-eligible job from using the free global slot.

No heap allocation should be required merely to choose the next candidate.

### B2. Avoid cloning the complete job config on every launch

Current `Launch` owns a cloned `ScheduledJobConfig`, including command/name/path strings.

Prefer a launch decision that identifies the selected runtime job by index plus scalar/log metadata. Spawn the direct child synchronously from a borrowed job config before the next await, then retain only the child-lifecycle metadata needed while it runs (for example job name and start instant).

Preserve exact argv and working-directory behavior.

### B3. Remove fabricated schedule fallback

Current due advancement logs a `next_after` error and assigns:

~~~text
wall_now + 366 days
~~~

That fabricates schedule semantics and keeps a broken scheduler alive.

After Workstream D makes impossible schedules fail at config validation, a runtime `next_after` failure is an internal/time-domain error. Propagate it through the scheduler's existing fatal task boundary instead of inventing a one-year retry.

Prefer the smallest error path that remains testable. Do not panic directly inside schedule arithmetic if a normal `Result` propagation fits the existing `scheduler::run -> run.rs` supervision boundary.

### B4. Re-measure before dependency replacement

After B1-B3 and any similarly obvious no-semantics simplifications, rebuild the stripped release binary.

If the binary is <= 3,228,272 bytes, stop footprint redesign there. Do not replace a correct time backend merely to make the binary even smaller.

## Workstream C: evidence-gated local-time backend reduction

Only if Workstream B cannot recover the full footprint budget, measure alternative safe local-civil-time implementations.

The public cron semantics must remain:

- system/local civil time;
- five numeric fields plus the four existing aliases;
- Gregorian calendar;
- traditional DOM/DOW OR behavior;
- spring-forward nonexistent local minute skipped;
- both fall-back repeated-minute instants returned in chronological order;
- strictly-exclusive `next_after`;
- runtime timezone reflects the host rather than the machine that built the binary.

### Candidate C1: retain chrono, reduce its linked feature/path surface

First check whether the existing use of `chrono::Local`, `DateTime`, formatting/error paths, or enabled features can be narrowed while preserving the exact behavior above.

Do not claim a feature is removable unless an actual linked release build proves it and all schedule/DST tests remain green.

### Candidate C2: Jiff system-timezone experiment

A current Jiff release supports system timezone discovery and TZif-based DST handling. On Unix it can use the host's zoneinfo; on Windows its default support may require bundled timezone data. Jiff also documents a `perf-inline` feature that can be disabled when binary size is preferred.

Treat Jiff only as a measured candidate, not a presumed improvement.

Prototype with the smallest feature set that still supports:

- system timezone discovery;
- local civil-to-instant ambiguity/gap handling;
- current supported Linux/macOS/Windows targets.

Measure stripped `greggd` on the ordinary Linux qualification host and inspect Windows/macOS dependency implications. Reject it if it does not beat the current production backend enough to close the gate or materially complicates the supported-platform contract.

Do not bundle a full timezone database on Unix merely to replace `chrono`.

### Candidate C3: another safe library only with evidence

A different safe local-time crate may be tested only if it:

- supports Rust 1.89;
- preserves DST gap/overlap semantics on all supported daemon platforms;
- does not require Gregg-owned unsafe code;
- has a credible smaller linked footprint.

Do not write bespoke libc/Win32 timezone bindings in Gregg. The workspace forbids unsafe code, and changing that policy is outside this scheduler feature.

### Time-backend acceptance

Any replacement must pass the existing schedule/DST test matrix unchanged or with equivalent stronger tests. Record exact before/after stripped byte counts and dependency-feature deltas.

Do not accept a backend that satisfies Linux size by making Windows/macOS semantics materially weaker.

## Workstream D: reject calendar-impossible cron expressions during configuration

The current parser validates field syntax/ranges but a schedule such as:

~~~text
0 0 31 2 *
~~~

is syntactically valid even though no Gregorian date can satisfy it. `Config::validate` can therefore accept it and the scheduler later fails while computing its initial occurrence.

Move this error to configuration validation.

### Required contract

After parsing a schedule, prove that at least one calendar date can satisfy its month/day constraints over one Gregorian 400-year cycle.

The satisfiability check should be:

- deterministic;
- independent of the current wall clock;
- independent of the host timezone;
- allocation-free or trivially bounded;
- based on the same `date_matches` semantics used by runtime scheduling;
- cheap at the configured maximum of 64 jobs.

A pure calendar scan is preferred to calling `Local::now()` during config parsing.

Examples that must be accepted:

~~~text
0 0 29 2 *
0 0 31 1,3,5,7,8,10,12 *
0 0 31 2 1        # traditional DOM/DOW OR means Mondays make this satisfiable
0 0 * 2 1
~~~

Examples that must be rejected as unsatisfiable:

~~~text
0 0 31 2 *
0 0 30 2 *
~~~

Use the actual parser's wildcard/DOM-DOW semantics when deciding satisfiability; do not incorrectly reject a restricted DOW schedule just because its restricted DOM is impossible.

Config loading must surface the failure as `InvalidJobs` (or the existing scheduler-config classification) before the daemon binds its listener or launches runtime tasks.

### Runtime invariant after validation

Once a schedule has passed configuration validation, ordinary `Engine::new` must not discover an impossible-calendar expression.

Keep `next_after` fallible for genuine clock/range/internal failures, but propagate such a failure instead of silently changing the schedule.

## Workstream E: preserve and strengthen scheduler regressions

Retain all current Plan-157 coverage and add focused tests for the corrective behavior.

Required new tests:

- February 31 rejected by config validation;
- February 30 rejected;
- February 29 accepted and finds a leap-year occurrence;
- impossible DOM plus a satisfiable restricted DOW is accepted under OR semantics;
- config validation does not depend on local timezone/current clock;
- no fabricated `+366 days` fallback remains;
- a forced `next_after` runtime error reaches the scheduler fatal boundary;
- candidate selection allocates no temporary candidate vector if the implementation exposes an inspectable seam;
- oldest/config-order fairness remains unchanged;
- a blocked heavy job still does not block a younger eligible time-only job;
- five deferred jobs still produce one global launch;
- load is still rechecked after child completion.

Do not add production-duration sleeps.

## Workstream F: final scheduler qualification and planning reconciliation

After the corrective implementation:

1. run focused scheduler/config/run tests;
2. run the default local check;
3. run the release preflight;
4. rebuild and record the final stripped binary;
5. demonstrate the binary is <= 3,228,272 bytes and therefore also under the 5% ceiling;
6. run one harmless Unix scheduler smoke;
7. run the existing six-job CI workflow once.

The harmless smoke should cover, with temporary files only:

- one time-only job executes once;
- one low-threshold load-gated job defers;
- a short smoke max-wait expires without command execution;
- two due jobs do not run concurrently;
- direct-child shutdown remains bounded.

Do not run destructive maintenance such as cargo-cleanme as qualification.

### Planning records

At successful closure:

- append a Plan-158 closure record with exact implementation SHA, final stripped bytes/delta, dependency choice, local checks, smoke result, and CI run ID;
- reconcile Plan 157's acceptance checklist against actual evidence rather than checking every box mechanically;
- mark Plan 157 complete through Plan 158 and preserve its original over-budget implementation record;
- mark Plan 155 complete only when both 157 and 158 are truthfully closed;
- update `plans/README.md` rows and dependency order to `157 -> 158`;
- leave Plan 091 open unless its independent sustained-soak requirement has separately been satisfied.

Do not rewrite Plan 156's historical prototype measurements. Add a correction/closure note where needed so the record remains auditable.

## Expected implementation surface

Likely:

~~~text
crates/greggd/src/scheduler.rs
crates/greggd/src/scheduler/schedule.rs
crates/greggd/src/config.rs
crates/greggd/Cargo.toml        # only if the selected time backend/features change
Cargo.lock                      # only if dependency selection changes
docs/daemon.md                  # only if schedule-validation/user semantics wording changes
architecture/greggd-daemon.md   # only if implementation ownership changes materially
plans/155-load-aware-maintenance-scheduler-roadmap.md
plans/156-scheduler-execution-boundary-and-footprint-qualification.md
plans/157-load-aware-maintenance-scheduler-implementation.md
plans/158-scheduler-footprint-and-schedule-validation-corrective-pass.md
plans/README.md
~~~

Not expected:

~~~text
crates/gregg-protocol/**
crates/gregg-host/**
crates/gregg-update/**
crates/gregg/**
HTTP routes or schemas
systemd/launchd/SCM service policy
startup/update/uninstall/croncheck behavior
CI workflows or matrix expansion
release workflow changes
persistent scheduler storage
~~~

## Verification

Focused:

~~~text
cargo test -p greggd --all-targets --all-features -- scheduler
cargo test -p greggd --all-targets --all-features -- config
cargo test -p greggd --all-targets --all-features -- run
cargo +1.89 test -p greggd --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
./scripts/check-local.sh --release
cargo tree -p greggd -e features
cargo build -p greggd --release
~~~

Use the same size-measurement method as Plan 156; do not compare an unstripped/debug/different-profile artifact to the 3,097,200-byte baseline.

One ordinary existing CI run is sufficient hosted evidence. Do not add a scheduler workflow, job, artifact bundle, or repeated-green requirement.

## Acceptance criteria

- [x] The current 3,272,456-byte production result is reproduced or any variance is explained before optimization.
- [ ] Final stripped `greggd` is <= 3,228,272 bytes against the Plan-156 baseline. Not met: 3,261,664. See the measurement record.
- [ ] Final scheduler growth is therefore <=128 KiB and <=5%. Not met: +164,464 bytes, +5.309%. Also above the looser 5% ceiling.
- [x] The exact final dependency/feature graph is recorded.
- [x] No `cron-parser` production dependency is reintroduced.
- [x] Candidate selection no longer requires the current temporary Vec+sort, unless measurement proves retaining it is smaller and the plan records that evidence.
- [x] Per-launch full job-config cloning is removed or retained only with measured evidence that an alternative does not help the gate.
- [x] No fabricated `wall_now + 366 days` schedule fallback remains.
- [x] Calendar-impossible schedules fail config validation before listener/task startup.
- [x] DOM/DOW OR semantics are preserved in satisfiability validation.
- [x] Leap-day and DST gap/overlap semantics remain correct.
- [x] No Gregg-owned unsafe code is introduced and the workspace unsafe-code policy is unchanged.
- [x] Same-principal/security/service-sandbox behavior is unchanged.
- [x] Cached-load, coalescing, one-global-child, max-wait, and anti-herd behavior remain unchanged.
- [x] Windows continues to reject load-gated jobs without losing time-only scheduling.
- [x] HTTP/protocol/croncheck/control/startup/update/uninstall behavior is unchanged.
- [x] Focused, default, release, and Rust-1.89 checks pass.
- [x] Harmless scheduler smoke passes.
- [x] Existing six-job CI passes.
- [ ] Plan 157 is reconciled against real evidence and closed through this corrective. Partially done: every functional criterion is now demonstrated, but Plan 157 cannot close while the footprint item is open.
- [ ] Plan 155 is closed only if the complete scheduler line is actually finished. Not closed; the line is not finished.
- [x] Plan 091's independent soak status is not conflated with scheduler closure.

## Stop conditions

Stop and write a new qualification plan rather than forcing this correction if closing the 44,184-byte deficit would require any of the following:

- weakening DST/local-civil-time semantics;
- changing schedules from local time to UTC;
- adding unsafe timezone/process FFI to Gregg;
- weakening direct-child shutdown;
- removing Windows time-only support;
- relaxing the 128 KiB/5% gate without a separately approved roadmap decision;
- changing the single-child/coalescing architecture;
- adding remote control, persistent state, impersonation, secrets, or service-sandbox exceptions.

This stop condition is what fired. The work below records the implementation,
the measured evidence, and why each listed reduction was excluded, then hands
the residual budget decision to Plan 159.

## Handoff

Start with Workstreams A and B. The current implementation is functionally strong and already has all-platform CI evidence; the goal is to remove unnecessary linked/code surface, not to redesign the scheduler.

Only investigate a different local-time backend after the simple state-machine/allocation reductions are measured. Treat any time-backend swap as a reversible experiment and preserve exact DST behavior. In parallel, move impossible-calendar detection into pure configuration validation and delete the fabricated runtime fallback.

## Implementation and measurement record

Implementation commit: `7a466f86eb069a036158e105e13f17f3b9e23563`. CI run
`37172425056` is green across all six jobs at that SHA. Existing CI run
`37169031236` is green on the immediately preceding main.

### What landed

Workstream B, all three items, plus one further no-semantics reduction:

- **B1 (allocation-free selection).** `Engine::tick` no longer builds and sorts
  a `Vec<(index, pending_since)>` per launch decision. It makes two bounded
  passes over at most 64 jobs: the first finds the oldest launchable candidate
  with config order as the stable tie breaker, the second applies load
  deferral. The second pass reproduces the previous ordering exactly by
  touching only blocked candidates whose `(pending_since, index)` key precedes
  the winner, so an older blocked job is still deferred and still does not
  prevent a younger time-only or load-eligible job from taking the free slot.
  The `core::slice::sort::stable::{quicksort, drift::sort}` instantiations for
  `(usize, Instant)` are gone from the release binary; the remaining
  `core::slice::sort` uses in the binary are pre-existing `gregg-host`,
  `clap_builder`, `regex_syntax`, and `toml_edit` ones.
- **B2 (no per-launch job-config clone).** `Engine` borrows `&[ScheduledJobConfig]`
  and keeps only a parsed `JobState` per job, `Launch` identifies the selected
  job by index plus scalar/log metadata, and `start_child` takes a borrowed
  `&ScheduledJobConfig`. The active child retains only a borrowed job name and
  its start instant. `run.rs` hands the job list to the scheduler task with
  `mem::take` instead of a deep `Config` clone. `<ScheduledJobConfig as
  Clone>::clone` is no longer reachable.
- **B3 (no fabricated fallback).** `Engine::tick` returns
  `Result<Option<Launch>, String>` and propagates a `next_after` failure to
  `scheduler::run`, which already terminates through the existing
  `run.rs` fatal task boundary. The `wall_now + 366 days` assignment and the
  `chrono::Duration` use are deleted. `Engine::new` also reports the job name
  with an initial-occurrence failure.
- **Workstream D (calendar satisfiability).** `LocalSchedule::is_satisfiable`
  walks one Gregorian 400-year cycle from a fixed epoch (2000-01-01, no clock
  or timezone input) using the same `date_matches` day-matching semantics as
  runtime scheduling, stopping at the first match. `ScheduledJobConfig::validate`
  turns a `false` verdict into an `InvalidJobs` violation, so `0 0 31 2 *` and
  `0 0 30 2 *` fail config loading before the listener binds. Traditional
  DOM/DOW OR behavior is preserved, so `0 0 31 2 1` and `0 0 * 2 1` remain
  valid and `0 0 29 2 *` finds a real leap-year occurrence.

Workstream D is a genuine defect fix: before it, `0 0 31 2 *` passed
configuration loading and then failed inside the scheduler.

Workstream E added `schedule_validation.rs` (a single-test integration binary
so the `TZ` mutation cannot race another test) plus
`satisfiability_rejects_only_calendar_impossible_expressions`,
`leap_day_schedule_finds_a_real_leap_year_occurrence`,
`calendar_impossible_schedules_fail_validation`,
`only_candidates_older_than_the_winner_are_deferred`,
`five_deferred_jobs_still_produce_one_global_launch_each`,
`schedule_failure_propagates_instead_of_fabricating_a_fallback`, and
`impossible_schedule_reaches_the_run_error_boundary`. All pre-existing
scheduler, config, run, and DST tests are retained unchanged.

### Measurement method

Repository release profile (`lto = "fat"`, `codegen-units = 1`,
`strip = "symbols"`, `panic = "abort"`) and `stat -c %s target/release/greggd`
on `x86_64-unknown-linux-gnu`, the same method Plan 156 recorded. Attribution
used `cargo bloat` on an otherwise identical `CARGO_PROFILE_RELEASE_STRIP=none`
build; no analysis tool was added as a dependency or CI requirement.

### Results

| Variant | Bytes | Delta from baseline |
|---|---:|---:|
| Plan-156 baseline `e708e54`, rebuilt from a clean worktree | 3,097,200 | — (exactly reproduces Plan 156's reference) |
| Plan-157 integrated `308383c` | 3,272,456 | +175,256 (+5.6585%) |
| after B1+B2+B3 | 3,263,136 | +165,936 |
| **after the `mem::take` reduction (`7a466f8`)** | **3,261,664** | **+164,464 (+5.309%)** |
| 128 KiB ceiling | 3,228,272 | over by 33,392 |
| 5% ceiling | 3,252,060 | over by 9,604 |

Plan 158 recovered 10,792 bytes of the 44,184 required (24.4%).

### Attribution

| Component | Bytes | Method |
|---|---:|---|
| Whole scheduler module | 98,352 | scheduler spawn removed from `run.rs` (2 taps) |
| `jobs` config field total | 67,584 | the same ablation against the baseline |
| &nbsp;&nbsp;`(de)serialization` | 40,696 | `#[serde(default, skip)]` on the field |
| &nbsp;&nbsp;job validation + cron parse + satisfiability | 5,808 | validator body stubbed |
| `chrono::Local` local-civil-time support | 41,072 | `type Local = Utc` substitution |
| Tokio async child lifecycle | 20,448 | entire `tokio::process` path stubbed |
| scheduler/schedule/`run.rs` own code | 36,832 | residual |

### Workstream C result: rejected with measurements

- **C1 (narrow `chrono`): exhausted.** `greggd` already declares
  `chrono = { version = "0.4", default-features = false, features = ["clock"] }`.
  `chrono`'s `clock` feature is `[winapi, iana-time-zone, now]`; without it
  there is no `Local` at all, so the current feature set is already the
  minimum for local civil time on the three supported platforms.
- **C2 (Jiff 0.2): rejected.** A working prototype
  (`jiff 0.2.37`, `default-features = false`,
  `features = ["std", "tz-system", "tzdb-zoneinfo", "tzdb-bundle-platform", "alloc"]`,
  `tz::TimeZone::system()` replacing `chrono::Local`) built and measured
  **3,543,848 bytes**, i.e. 280,712 bytes *worse* than the retained chrono
  implementation. Its `.text` grows from 2.5 MiB to 2.7 MiB. It would also
  need bundled timezone data on Windows, which the plan forbids as a way to
  satisfy the Linux size target. The prototype was reverted; `Cargo.toml` and
  `Cargo.lock` carry no jiff entry.
- **C3 (other safe libraries / bespoke FFI): not taken.** The only remaining
  reducers are Gregg-owned platform timezone bindings (forbidden by the
  workspace `unsafe_code = "deny"` policy and by this plan's stop conditions)
  and a `std::process` polling replacement for the Tokio child lifecycle
  (forbidden because it weakens the event-driven direct-child shutdown
  contract Plan 156 qualified).

### Why the gate cannot be met inside this plan

The budget is 131,072 bytes of total scheduler growth. The irreducible
dependency and configuration-schema cost is 67,584 + 41,072 + 20,448 =
**129,104 bytes**, leaving **1,968 bytes** for the entire state machine, cron
parser, satisfiability walk, and `run.rs` wiring.

Plan 156 recorded 38,424 bytes of headroom for exactly that, and Plan 157's
implementation needed 82,608. The reason is visible in the tables above: Plan
156's candidate measurement (+92,648 for the combined cron/time + process
linked candidate) did not include the `jobs` configuration field that Plan 157
added, which costs 67,584 bytes on its own. Plan 156's recorded headroom was
therefore never available to a real implementation, and no amount of scheduler
code reduction changes that. Plan 156's numbers are not rewritten; this note
records the correction (see the note appended to that plan).

Plan 158's own stop conditions forbid the three remaining reducers: weakening
direct-child shutdown, adding unsafe timezone/process FFI, and relaxing the
128 KiB/5% gate without a separately approved roadmap decision. Plan 158 also
cannot close with an open acceptance criterion. The residual is therefore
handed to Plan 159, which exists solely to make that decision explicitly.

### Verification evidence

- `cargo test -p greggd --all-targets --all-features -- scheduler`,
  `-- config`, and `-- run` green.
- `cargo test --workspace --all-targets --all-features`: 1,318 passed,
  2 ignored, 13 suites.
- `cargo +1.89 test -p greggd --all-targets --all-features` green.
- `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- `./scripts/check-local.sh` green (default mode).
- `./scripts/check-local.sh --release` green at `7a466f8`: clippy, docs,
  clean tree, version/package checks, installed-binary loopback smoke, and the
  `gregg-protocol` publish dry run.
- Harmless scheduler smoke (temporary files only, no real maintenance):
  - `0 0 31 2 *` is rejected as `InvalidJobs` and the listener never binds,
    while `/v2/healthz` and `/v2/status` stay available on a valid config;
  - the time-only job executes on each minute boundary;
  - a `max_load = 0.0` job never executes under observed 1m load of ~4.1 and
    is logged as `scheduled job pending: load gate`;
  - a `max_load = 0.0`, `max_wait_ms = 10_000` job expires at ~10,001 ms with
    `scheduled job expired waiting for load` and never executes;
  - two 4-second jobs never overlap (2 serial launches, 2 starts, 2 ends), and
    only one launch happens per free slot;
  - `greggd stop` during an active `/bin/sleep 120` child logs
    `scheduler shutting down with active command`, the daemon exits 0, and no
    child survives.
- Existing six-job CI run `37172425056` green at `7a466f8`.

### Planning records

- Plan 156 keeps its original prototype measurements; a correction note was
  appended.
- Plan 157 stays open. Every functional criterion is now demonstrated by the
  tests, smoke, and CI above, but its footprint criterion and its
  "no binary-footprint gate waiver" stop condition keep it open, and its
  original over-budget implementation record is preserved.
- Plan 155 stays open; the line is not finished.
- Plan 159 owns the footprint budget decision and the eventual 155/157/158
  closure.
- Plan 091 is untouched.


## Closure note (Plan 159 outcome 1)

This plan is closed by Plan 159's decision. Its implemented scope is unchanged
from the record above; the footprint gate it was stopped by is superseded for
the scheduler line by the Plan-159 re-baseline (3,261,664-byte budget). The
stop condition fired correctly — the remaining bytes were not taken from any
forbidden lever — and the roadmap decision it escalated to is now recorded.
