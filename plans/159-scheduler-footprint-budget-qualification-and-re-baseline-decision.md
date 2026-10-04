# Plan 159: Scheduler footprint budget qualification and re-baseline decision

Status: planned.

Depends on: the Plan-158 implementation on main and its recorded measurement
package. Independent of the remaining Plan 091 soak record.

## Objective

Resolve the one open question the Plan-155 scheduler line cannot close: whether
`greggd`'s scheduler line may ship outside the Plan-156 footprint budget, or
whether a further measured reduction can bring it inside.

Plan 158 executed Plan 156's corrective brief faithfully and recovered 10,792
of the 44,184 required bytes. It also proved, by measurement, that the
remaining 33,392 bytes cannot come from the levers the plan authorized. This
plan exists to turn that measurement into an explicit, recorded decision
instead of leaving the scheduler line in permanent limbo.

This is a decision/qualification pass. It is not permission to redesign the
scheduler.

## Recorded measurement package (from Plan 158)

All numbers are stripped `x86_64-unknown-linux-gnu` release builds on one
host using the Plan-156 profile (`lto = "fat"`, `codegen-units = 1`,
`strip = "symbols"`, `panic = "abort"`).

| Variant | Bytes | Delta from baseline |
|---|---:|---:|
| Plan-156 pre-scheduler baseline `e708e54` (reproduced) | 3,097,200 | — |
| Plan-157 integrated (reproduced) | 3,272,456 | +175,256 |
| Plan-158 implementation `7a466f8` | 3,261,664 | +164,464 (+5.309%) |
| 128 KiB ceiling | 3,228,272 | over by 33,392 |
| 5% ceiling | 3,252,060 | over by 9,604 |

Attribution ablations on the Plan-158 tree:

| Component | Bytes | Method |
|---|---:|---|
| Whole scheduler module | 98,352 | scheduler spawn removed from `run.rs` |
| `jobs` config field (serde/toml + validation) | 67,584 | same ablation against the baseline |
| &nbsp;&nbsp;of which `(de)serialization` | 40,696 | `#[serde(skip)]` on the field |
| &nbsp;&nbsp;of which job validation, cron parse, satisfiability | 5,808 | validator body stubbed |
| `chrono::Local` local-civil-time support | 41,072 | `Local` substituted with `Utc` |
| Tokio async child lifecycle | 20,448 | entire `tokio::process` path stubbed |
| Scheduler/schedule/`run.rs` wiring (own code) | 36,832 | residual |
| Jiff 0.2 local-time candidate | 3,543,848 | measured prototype; rejected |

The decisive arithmetic: the budget is 131,072 bytes of total scheduler growth.
The irreducible dependency/schema cost is 67,584 + 41,072 + 20,448 =
129,104 bytes, which leaves 1,968 bytes for the entire state machine,
parser, wiring, and tests-in-production. Plan 156 recorded 38,424 bytes of
headroom for exactly that, but its candidate measurement did not include the
`jobs` configuration field that Plan 157 introduced (+67,584). The budget was
therefore never available to a real implementation, independent of how the
scheduler is written.

## Decision space

Exactly three honest outcomes. Pick one and record it:

1. **Re-baseline the scheduler line's budget** to the measured cost, with the
   attribution table above as the justification and an explicit statement that
   the general no-regression stance for other features is unchanged. This is a
   roadmap-level relaxation and must be recorded as such, not as a silent
   threshold change.
2. **Approve one named architectural reduction** and implement it here. The
   measured cost/benefit of each, all of which are currently excluded by a
   settled decision or an explicit Plan-158 stop condition:
   - Tokio async child lifecycle: 20,448 bytes, by replacing it with a polled
     `std::process` waiter. Rejected by Plan 156 (no smaller
     lifecycle-correct adapter identified) and it weakens the event-driven
     direct-child shutdown contract.
   - `chrono::Local`: 41,072 bytes, by a Gregg-owned platform timezone layer.
     Rejected: the workspace forbids unsafe code and Plan 158 forbids
     hand-written local-time FFI; Jiff 0.2 measured 280,712 bytes *worse* and
     would also need bundled timezone data on Windows.
   - `jobs` config field: 40,696 bytes of `serde`/`toml` array-of-tables
     codegen. Reducible only by changing the user-visible configuration schema
     or the public `greggd` Rust API, both outside this line.
3. **Revert the scheduler line** and return `greggd` to the 3,097,200-byte
   baseline, keeping only the parts that are independent of it: config-loaded
   schedule validation is scheduler-owned, so this is a full removal.

Outcome 1 is the one the evidence supports. Outcomes 2 and 3 remain legitimate
if the roadmap prefers them; they are listed so the choice is explicit.

## Governing invariants

Preserve everything Plans 155-158 settled:

- direct argv execution, no implicit shell, no environment/secret map;
- same-principal execution and the explicit Unix euid-0 opt-in;
- canonical systemd/launchd/SCM sandboxing;
- Windows time-only support with load gates rejected;
- cached sampler load only, no new probe and no HTTP self-poll;
- 1m/5m/15m inclusive thresholds, warming/failed/missing load fails closed;
- one pending occurrence per job, one global child, fixed retry plus bounded
  max wait, oldest/config-order fairness, fresh load recheck between
  sequential load-gated jobs, no retry after nonzero exit, no downtime replay;
- direct-child shutdown inside the two-second child bound;
- no remote control, no persistent scheduler state, no protocol change;
- the workspace-wide `unsafe_code = "deny"` policy;
- configuration validation rejecting calendar-impossible schedules before the
  listener binds, and no fabricated runtime schedule fallback.

## Scope

Decide, record, and implement only the chosen outcome. Do not:

- reopen the cron language, add seconds/year/name fields, or add a second
  schedule backend;
- add concurrency, persistence, replay, or remote control;
- add dependencies without an MSRV and pattern check;
- add a scheduler CI workflow, job, matrix, or artifact bundle;
- conflate this decision with the independent Plan 091 soak record.

## Expected implementation surface

If outcome 1:

```text
plans/159-scheduler-footprint-budget-qualification-and-re-baseline-decision.md
plans/155-load-aware-maintenance-scheduler-roadmap.md
plans/156-scheduler-execution-boundary-and-footprint-qualification.md
plans/157-load-aware-maintenance-scheduler-implementation.md
plans/158-scheduler-footprint-and-schedule-validation-corrective-pass.md
plans/README.md
```

If outcome 2 or 3, only the chosen mechanism's own source and the same plan
set change.

## Verification

- `./scripts/check-local.sh`
- `./scripts/check-local.sh --release` when product code changes
- one ordinary existing six-job CI run, recorded by exact run ID
- a final stripped `greggd` measurement recorded with the same method

One existing CI run is sufficient evidence. No new workflow, job, matrix, or
artifact requirement.

## Acceptance criteria

- [ ] One of the three outcomes is chosen explicitly and recorded with the
      measured attribution table.
- [ ] The decision states whether the general footprint stance changes or only
      the scheduler line's budget.
- [ ] Plans 155, 157, and 158 are reconciled against the decision and closed
      only if the scheduler line is actually finished.
- [ ] Plan 091's independent soak status is untouched.
- [ ] Focused, default, and release checks pass.
- [ ] One existing six-job CI run is green and recorded by exact run ID.
- [ ] The final stripped `greggd` byte count is recorded.
- [ ] No scheduler architecture, cron language, dependency, config schema, CI,
      or release behavior changes unless the chosen outcome requires it.

## Stop conditions

Stop and escalate to a fresh roadmap plan if the decision requires any of:

- unsafe code in Gregg;
- weakening direct-child shutdown;
- changing schedules from local civil time to UTC;
- a timezone database bundled on Unix;
- a second simultaneous child or persistence/replay;
- a remote execution surface, impersonation, secrets, or service-sandbox
  exceptions;
- a protocol, HTTP, croncheck, control, startup, update, or uninstall change.

## Handoff

Start by re-reading Plan 158's measurement record; do not re-run the
qualification from scratch. The only decision left is whether the roadmap
accepts a measured, attributed budget for this feature line.
