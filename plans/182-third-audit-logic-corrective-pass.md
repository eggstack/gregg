# Plan 182: Third-audit logic and robustness corrective pass

Status: complete. See the closure record at the end.

Depends on: current main at `adc948d` ("fix eight audited defects").
Independent of Plans 091, 179, 180, and 181 — none of them is reopened here.

Opened from a fresh read-only audit of the workspace (scheduler, server, client
daemon, cron, TUI state, UI, config, scripts, and CI), whose findings were
recorded in `bugs.md` at the repository root.

## Objective

Close all eight recorded findings and apply both recorded optimizations, without
adding product scope.

Every mechanical gate was already green before this pass — `cargo test`,
`cargo clippy -D warnings`, `cargo fmt`, and `shellcheck` all passed at
`adc948d`. That is why every finding below is logical: a bound that cannot be
reached, a state field cleared on the wrong path, a value a predicate ignores, a
view that drops the only statement of a failure, an instant read on the wrong
side of an await, a panic guarded only by a distant caller, a test budget
sized for a workload instead of the build that precedes it, and two script
surfaces no blocking job could see.

## Findings and required behavior

### 1. A coalescing occurrence erased its load gate reading (greggd)

`crates/greggd/src/scheduler.rs` cleared `state.last_gate = None` outside the
`if/else` that distinguishes a fresh occurrence from one coalesced into an
already-pending occurrence.

When a civil occurrence arrives while the load gate is still holding the
previous one, the occurrence coalesces — correctly — but then loses the reading
that is delaying it. `defer_blocked_before` skips a pending whose `retry_at` is
still in the future, so nothing downstream restores it before the retry fires.
The job is published through `publish` as `waiting_for_slot` with `load: null`
and `next_retry_unix_ms: null`, and `job_state` then makes that state permanent.

Required: only a **fresh** occurrence clears the gate. A coalesced occurrence is
the same occurrence, so the reading that delayed it still describes it.

### 2. The per-system retained-job ceiling could not be reached (gregg)

`crates/gregg/src/cron.rs::evict_stale_jobs` returned early whenever every cached
job was still live, placing the `MAX_CRON_JOBS_PER_SYSTEM` clamp behind that
return. That is the ordinary steady state, so the ceiling only applied when a job
had gone stale. A remote configured with 84 jobs retained 84 histories.

`MAX_CRON_JOBS_PER_SYSTEM` answers "how many live histories may one system
hold"; the stale sweep answers "has this job gone away". They are different
questions and the cap must not be conditional on the sweep's answer.

Required: enforce the cap unconditionally, on every history merge.

### 3. The EggPool footer froze on a stale age (gregg)

`crates/gregg/src/state.rs::eggpool_visibly_differ` excluded both
`last_success_at` and `last_attempt_at`, justified by analogy to system
timestamps that "are never rendered". But `ui/eggpool.rs` renders the success
age live as `Updated for Nm`, and states the footer's purpose as saying when a
summary went stale. A pane whose every other field was unchanged never repainted
and kept asserting an age the footer exists to correct.

Required: include the last-*success* age in the render-visible comparison. Keep
attempt ages excluded — no renderer reads them.

### 4. The condensed view dropped the offline reason (gregg)

`crates/gregg/src/ui/condensed.rs::status_line` never read
`system.offline_reason`. `status` is only the literal `offline` / `pending`, so
pressing `v` replaced `srv offline (http) HTTP 503` with `srv offline`.

Required: append the same stable failure category the normal view renders, inside
the existing width budget. A pending row still carries no reason.

### 5. The staleness clock was read before the await (greggd)

`crates/greggd/src/server/mod.rs` took `now_unix_ms()` as an argument to
`v1_status_data`, `v2_status_data`, `v1_health_cached`, `v2_health_cached`,
`v1_health_now`, and `v2_health_now`, so it was evaluated **before** the future
was polled and therefore before the read guard was acquired. `is_stale` treats a
negative age as stale, which it does for a backward clock jump. A read that
parked behind a writer therefore compared a pre-publication instant against a
post-publication snapshot and served a fresh sample as
`503 "cached snapshot is stale"` — rendered by a client as
`offline (http) HTTP 503`.

Required: read the clock **under** the published read guard. The health retry
attempt re-reads for the same reason.

### 6. `next_deadline` could panic on an empty job set (greggd)

`next_deadline` asserted `expect("a scheduler is started only with at least one
job")`, but `Engine::new` validated only the upper bound (`> MAX_JOBS`), which
zero passes. The sole guard was in `run.rs`, ~800 lines away.

Required: refuse an empty job set in `Engine::new`, next to the upper bound,
where the precondition is actually established. `run.rs` keeps its guard as
defense in depth.

### 7. A pytest budget wrapped a Cargo build (scripts)

`scripts/tests/test_sustained_runner.py` ran the sustained runner with
`timeout=90`. That runner locates its workload with `cargo test --no-run`, so
the budget covered a build the test does not control, and any cold or
invalidated cache reported a build problem as a workload failure.

Required: size the budget to the build, overridable from the environment.

### 8. Neither script surface ran in any blocking job (CI)

`shellcheck` existed only in `release-binaries.yml`, against
`packaging/install.sh`, with findings discarded by
`|| echo "shellcheck warnings (non-fatal for CI)"`. `pytest scripts/tests` ran
nowhere. The Rust gates cannot see either surface.

Required: a blocking CI job for both, and a fatal rather than echoed shellcheck
in the release workflow.

## Optimizations

- **O1** `crates/greggd/src/scheduler/observation.rs::publish` deep-cloned the
  whole job vector to build a comparison key even on the unchanged path. Compare
  the fields against the retained key directly; build the key only when
  publishing.
- **O2** `crates/gregg/src/cron.rs::evict_stale_jobs` allocated a `Vec<String>`
  of cloned names and ran two linear scans with string comparison inside a
  `retain`. Use a borrowed `HashSet`, matching the fast-path pattern in
  `gregg-host/src/rate.rs`. O2 also bounds finding 2's cost, since an
  unconditionally enforced ceiling makes the sweep run on every merge.

## Verification

Each behavioral fix carries a regression test that was executed against the
pre-fix logic and observed failing, then re-run against the fix and observed
passing:

| Test | Pre-fix failure observed |
|------|--------------------------|
| `scheduler::tests::coalescing_into_a_load_blocked_occurrence_keeps_its_gate_reading` | `left: None`, `right: Some(SchedulerLoadGateV2 { observed: Some(9.0) })` |
| `server::tests::a_snapshot_published_while_a_read_parks_is_not_reported_as_stale` | served as stale-from-the-future |
| `server::tests::ready_health_published_while_a_read_parks_is_not_reported_as_stale` | ready memo reported stale |
| `cron::tests::the_per_system_job_ceiling_holds_while_every_job_stays_live` | `left: 65`, `right: 64` |
| `cron::tests::the_ceiling_evicts_a_live_job_rather_than_leaving_it_bounded_by_nothing` | `left: 65`, `right: 64` |
| `ui::condensed::tests::offline_status_keeps_the_failure_category_the_normal_view_renders` | rendered `"srv  offline"` |
| `state::cron_intent_tests::an_advanced_eggpool_success_age_alone_forces_a_repaint` | no repaint |
| `scheduler::tests::empty_job_list_builds_no_engine_state` (corrected in place) | asserted a state-free engine was constructible, which is exactly the state `next_deadline` cannot serve |

The two server tests needed a wall-clock advance to be observable at all: the
defect is a *negative* age, so the publication's observation instant must land in
a strictly later millisecond than the instant a pre-fix reader captured. They
park the reader on the lock, advance 2 ms, then publish through the held guard —
`update_snapshot` cannot be used there because it takes the same lock.

### Gate results

| Gate | Command | Result |
|------|---------|--------|
| Format | `cargo fmt --all -- --check` | **pass** |
| Lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | **pass**, 0 warnings |
| Tests | `cargo test --workspace --all-targets --all-features` | **pass** — 951 (gregg) + 4 + 47 (gregg-host) + 119 (gregg-protocol) + 62 (integration) + 49 (gregg-update) + 517 (greggd) + 3, 0 failed |
| Shell | `shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh` | **pass**, zero findings |
| Python | `python3 -m pytest scripts/tests` | **61 passed**, including the sustained-runner smoke |

The pytest run took 30 minutes because the sustained runner's `cargo test
--no-run` competed with the concurrent full-workspace build for the target
directory. That is exactly the coupling finding 7 removed: the case now has a
budget sized to the build rather than to its 2-second workload.

## Out of scope

No feature additions, no new dependencies, no protocol change, no scope
broadening. The audit also recorded one low-severity candidate (a `u16` row
height addition needing a height above 65425 to overflow, which a terminal
cannot produce) and thirteen disproven candidates; all are recorded as
checked-and-correct in the audit's own notes and need no work.

The audit's own fourth pass — `gregg-protocol` validation caps, `gregg-host`
platform FFI, `gregg-update` shell-injection and 404-only Cargo fallback, and
the `greggd` control socket, startup ownership, and uninstall ownership logic —
did not complete and is recorded there as **unreviewed, not clean**. This plan
closes what the audit actually reviewed; it makes no claim about that surface.

## Closure record

**Complete.** All eight findings fixed, both optimizations applied, no scope
change, no new dependency, no protocol change, no wire-format change.

Every behavioral fix is mutation-verified: the pre-fix logic was reinstated in
place and the regression test observed failing with the exact symptom recorded
in the verification table, then the fix was restored and the test observed
passing. That distinguishes "the test exists" from "the test detects the
defect".

One existing test was corrected rather than duplicated:
`empty_job_list_builds_no_engine_state` asserted that an empty job set yields a
state-free engine, which is exactly the state `next_deadline` cannot serve a
deadline from. It now asserts the refusal and keeps its original point — a
jobless daemon still pays no reconciliation cost, because `run.rs` never spawns
the scheduler task at all.

The only test-visible behavioral change beyond the eight findings is the
`greggd` scheduler's refusal of an empty job set. `run.rs` already skipped the
scheduler task in that case, so no reachable configuration changes behavior; the
refusal converts a latent panic into the configuration error its callers
already handle.