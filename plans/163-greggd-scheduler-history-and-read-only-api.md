# Plan 163: greggd scheduler history and read-only API

Status: complete (see "Closure record" below).

Depends on: completed Plan 162. Independent of Plan 091 and may proceed in
parallel with Plan 164 once Plan 162's remote contract is frozen.

## Objective

Implement the qualified scheduler observability contract in greggd without
changing scheduler execution semantics or adding persistent state.

The result should let a remote Gregg client answer, truthfully and cheaply:

- what jobs are configured;
- when each is next due;
- whether each is idle, running, slot-waiting, load-delayed, or waiting on
  unavailable load;
- what load decision is delaying it;
- what happened on the most recent bounded set of terminal occurrences;
- what bounded stdout/stderr tails those occurrences produced.

## Preserved execution semantics

Do not alter:

- cron syntax/aliases;
- local-civil/DST behavior;
- Plan-160 one-minute reconciliation;
- one pending occurrence per job;
- one global child slot;
- cached sampler load source;
- retry/max-wait clocks;
- coalescing;
- direct argv/no implicit shell;
- same-principal execution/root opt-in;
- two-second direct-child shutdown;
- no downtime replay.

Observability must follow execution state, not drive it.

## Configuration

Implement the exact Plan-162 history-depth setting and bounds.

Requirements:

- missing setting gives exactly five records per job;
- invalid values fail existing config validation before bind;
- configprint, status, startup ownership, update, and uninstall behavior are
  unchanged;
- no history path/database/log-file setting;
- no command-output environment/secret setting unless Plan 162 explicitly
  qualified one narrow exposure policy.

Update config examples and daemon README only after behavior exists.

## Scheduler state refactor

Introduce an explicit scheduler-observation model rather than scraping tracing
logs.

A useful internal separation is:

~~~text
execution Engine
  +-- authoritative per-job scheduling state
  +-- active child
  +-- bounded per-job terminal history
        |
        +-- publish compact read-only SchedulerSnapshot
~~~

Keep schedule-calculation types private where possible. The server-facing model
should contain only values needed by the wire contract.

Every externally visible transition must publish coherently:

- due -> waiting for slot;
- due/retry -> load high;
- due/retry -> load unavailable;
- pending -> running;
- running -> terminal result;
- pending -> max-wait expired;
- spawn failure;
- next occurrence advance.

Do not publish on the Plan-160 reconciliation wake when no externally visible
state changed.

## Bounded terminal history

Use one bounded ring/deque per configured job at the exact qualified depth.

Properties:

- no allocation growth after reaching capacity except ordinary bounded record
  replacement;
- oldest terminal record evicted first;
- record sequence monotonic within the scheduler lifetime;
- history revision increments on retained-history mutation;
- scheduler restart resets in-memory history and restart discriminator;
- no fs I/O;
- no replay from tracing logs.

A load-expired occurrence is a terminal record even though no child ran.
Likewise spawn failure must be visible as a terminal record.

## Child output capture

Replace Stdio::null() only as qualified by Plan 162.

Required mechanics:

- stdin remains null;
- stdout/stderr are piped;
- take both pipe handles immediately after spawn;
- concurrently drain both while waiting for child completion;
- use fixed-capacity tail buffers with exact qualified byte caps;
- noisy output cannot allocate proportional to bytes written;
- drain tasks participate in bounded child shutdown and cannot keep the daemon
  alive indefinitely;
- child exit/status collection remains nonblocking on the current-thread Tokio
  runtime;
- direct-child kill semantics stay unchanged.

Do not block on reading stdout to completion before servicing stderr. Do not use
a line-oriented reader that can grow without bound waiting for a newline.

When a child exits, finalize the output tails and terminal record before
publishing the new history revision.

## Arbitrary output bytes

Capture bytes, not assumed UTF-8 text.

After the fixed byte bound is enforced:

- convert to the exact Plan-162 wire representation;
- mark truncation independently for stdout and stderr;
- preserve useful tail text when invalid UTF-8 appears using the qualified
  lossy/escaped policy;
- never interpret ANSI escapes in greggd.

Terminal sanitization remains a Gregg frontend responsibility.

## Read-only publication

Add scheduler state publication separate from high-frequency metrics
publication unless Plan 162 demonstrated that sharing the existing
PublishedState lock is strictly better.

Handlers must not:

- await scheduler mutation;
- hold a scheduler lock while serializing;
- perform host telemetry;
- touch config files;
- spawn a process;
- change schedule state.

Prefer cached compact bytes for history if measurements show serialization cost
or lock duration matters, using the existing cached-status pattern as guidance.
Because history changes only at state transitions, recomputing on every metrics
sample is explicitly wrong.

## Server routes

Implement the exact Plan-162 routes, expected to be:

~~~text
GET/HEAD /v2/scheduler
GET/HEAD /v2/scheduler/history
~~~

Requirements:

- GET returns compact JSON with the qualified content type;
- HEAD returns matching status/headers and no body under existing conventions;
- unsupported methods -> 405;
- unknown routes -> 404;
- no jobs -> 200 with an empty valid document;
- scheduler internal failure remains a supervised daemon failure rather than a
  fabricated empty scheduler;
- body construction respects the qualified maximum by construction.

Do not change /v2/status, /v2/healthz, v1 routes, or staleness semantics.

## Protocol fixtures and validation

Add canonical fixtures for at least:

- no jobs;
- idle scheduled job;
- load-high delayed job;
- load-unavailable delayed job;
- running job;
- successful terminal record with stdout;
- failed terminal record with stderr/exit status;
- load-expired record;
- truncated output;
- maximum-history representative shape.

Protocol validation should reject impossible/out-of-bound wire documents without
making absence on an old daemon a protocol error.

## Deterministic scheduler tests

Inject clocks/load/process adapters where needed rather than sleeping production
intervals.

Cover:

- configured job appears before its first run;
- next_due advances correctly;
- high load publishes delayed state once without retry log/publication noise;
- unavailable load is distinguishable from high load;
- slot wait is distinguishable when observable;
- success records exit 0 and output tail;
- nonzero exit records status and stderr;
- spawn failure records a terminal outcome;
- max_wait expiration records a terminal outcome;
- per-job ring evicts oldest at qualified depth;
- history revision only changes when appropriate;
- large stdout/stderr remain bounded;
- shutdown with noisy child stays inside existing lifecycle bounds;
- Plan-160 civil-clock tests remain green.

## Server tests

Use the existing EggServe handler test harness. Verify:

- GET/HEAD parity;
- empty scheduler 200;
- representative live states;
- history body;
- method rejection;
- no accidental metrics-route changes;
- body cap assumptions.

No new server framework or test daemon is warranted.

## Footprint and runtime evidence

Before closure:

- measure stripped release greggd against Plan 162's numeric rule;
- if the rule is exceeded, stop and open/execute a corrective plan rather than
  silently re-baselining;
- measure idle scheduler task behavior with configured jobs but no due work;
- record default worst-case retained-history allocation from the exact constants;
- confirm no extra wake when jobs are empty;
- confirm no per-second output/history work.

## Documentation

Update in the implementation pass:

- README.md where daemon scheduler capability is described;
- crates/greggd/README.md;
- architecture/greggd-daemon.md;
- architecture/protocol.md;
- architecture/gregg-protocol.md;
- .opencode/skills/greggd-daemon/SKILL.md;
- .opencode/skills/protocol-wire/SKILL.md;
- AGENTS.md for new invariants;
- CHANGELOG.md.

Do not document the c TUI before Plan 166 lands it.

## Acceptance criteria

- [x] Default remote history depth is five and configured bounds match Plan 162.
- [x] Scheduler history is entirely memory-only.
- [x] stdout/stderr capture is fixed-capacity and concurrently drained.
- [x] Large child output cannot deadlock the scheduler or grow memory
      proportionally.
- [x] Live state truthfully distinguishes the qualified pending/running states.
- [x] Terminal history includes all qualified non-child failures.
- [x] Scheduler summary/history routes are read-only and separate from status.
- [x] Empty configured-job set returns valid empty scheduler documents.
- [x] Existing metrics/health/v1/v2 behavior remains unchanged.
- [x] Protocol validation covers the new documents.
- [x] Plan-160 clock semantics remain green.
- [x] Existing scheduler/security/shutdown invariants remain green.
- [x] Stripped greggd meets Plan 162's recorded footprint rule.
- [x] Default local checks pass; native CI is recorded in Plan 167.
- [x] Active daemon/protocol docs are reconciled.

## Stop conditions

Open a corrective plan instead of closing if:

- output draining weakens bounded daemon shutdown;
- history publication couples HTTP latency to the scheduler execution lock;
- response size exceeds Plan 162's bound;
- scheduler binary growth exceeds Plan 162's numeric rule;
- new route implementation requires remote mutation/auth architecture;
- Windows scheduler behavior is accidentally widened to support load gates.

## Handoff

When complete, Plan 166 may consume the read-only remote contract. Plan 164 does
not need to wait for this implementation to establish the local client-daemon
boundary.

---

## Closure record

### Preserved execution semantics

Nothing in the execution path changed. Cron syntax and aliases, local-civil
time and DST behavior, the Plan-160 one-minute reconciliation, one pending
occurrence per job, one global child slot, the cached sampler load source,
retry/max-wait clocks, coalescing, direct argv with no implicit shell,
same-principal execution with the root opt-in, the two-second direct-child
shutdown bound, and no downtime replay are all untouched. Observability follows
execution state; it never drives it. The 480-test greggd suite (including every
Plan-160 clock-domain test) is green.

### Scheduler state refactor

`crates/greggd/src/scheduler/observation.rs` holds the observation model:

- `OutputTail` — fixed-capacity raw tail, retains the final 1024 bytes;
- `JobHistory` — one bounded `VecDeque` per configured job;
- `SchedulerObserver` — epoch, histories, sequence, revision, and the
  publication decision;
- `SchedulerPublisher` / `SchedulerPublication` — the shared cell the HTTP
  server reads, holding both documents already serialized;
- `job_state()` — the single place a live state is derived.

`JobState` gained two fields: the recorded `last_gate` (the reading behind the
actual decision, so an idle job never appears to carry a live load value) and,
on the pending occurrence, the civil `scheduled` time plus wall-clock copies of
pending/retry. The published `observed` load is the value from the decision, not
a per-second live reading, so a pending job does not cause a publication every
sample.

### Bounded terminal history

One `VecDeque` per job at the configured depth. Oldest evicted first, sequence
monotonic within the lifetime, `history_revision` advancing only on retained
mutation, and **no filesystem I/O anywhere in the module**. A restart clears
history and starts a new epoch with no replay.

### Child output capture

- stdout/stderr are piped only when `scheduler_history_limit > 0`; otherwise the
  child keeps the original null streams (proven by
  `disabled_capture_keeps_the_original_null_streams`).
- Both pipe handles are taken immediately after the spawn.
- Both streams drain concurrently with the child wait in one `tokio::join!`, so
  neither stream can block the other and neither can fill the child's pipe.
- The drain futures **borrow** the streams rather than spawning tasks. A
  cancelled select (deadline or shutdown) stops draining and leaves the handles
  in place for the next wake, so no drain task can outlive the scheduler or
  delay shutdown, and the two-second bound is unchanged. `wait_with_output` and
  any whole-output accumulator are absent.
- Tails fold each 4 KiB read in with one bounded drain, so retention is
  independent of bytes written. `output_flood_stays_bounded_and_never_deadlocks`
  runs a child that writes 4 MiB on each stream and proves it finishes, both
  streams are marked truncated, and neither exceeds the 512-byte published cap.

### Non-child outcomes

`spawn_failed` (a missing executable) and `load_expired` (max wait elapsed with
no child) both become terminal records with no exit code, start time, or
duration. Protocol validation rejects those fields on any non-child outcome, so
the document cannot claim a child ran.

### Publication and routes

The scheduler publication is a **separate** cell from the metrics
`PublishedState`, created once in `run.rs` and shared with the server state.
Handlers clone one `Arc` and serve already-serialized `Bytes`; they never
serialize, never await scheduler mutation, never hold a scheduler lock, and
never touch config, telemetry, or processes. A serialization failure keeps the
previous publication rather than degrading to a fabricated empty document, and a
scheduler task failure stays a supervised daemon failure (panic at the existing
fatal boundary).

The observer compares `(epoch, history_revision, jobs)` against the last
publication and skips the swap when they match, with `generated_at_unix_ms`
deliberately excluded. `an_unchanged_reconciliation_wake_publishes_nothing`
asserts three Plan-160-style wakes leave the served bytes byte-identical.

Routes are `GET`/`HEAD` only; other methods are 405, and `/v2/scheduler/run`,
`/v2/scheduler/jobs`, and `/v2/scheduler/cancel` are 404 for every method, so
there is no control plane. No-jobs returns `200` with a valid empty document.

### Live daemon evidence

A real release `greggd` was run with three jobs (`scheduler_history_limit = 3`):
a succeeding job, a job with a missing executable, and a load-gated job with
`max_load = 0.001`.

- At t+3s all three configured jobs were visible as `idle` with a `next_due`,
  before any had run.
- After the first minute: `smoke-job` → `success` with `exit_code: 0`;
  `missing-job` → `spawn_failed` with **no** fabricated `exit_code`;
  `gated-job` → `load_expired` with `delay_ms: 20000`, then `waiting_for_slot`
  once the global slot freed.
- `/v2/scheduler/history` returned separate `stdout` (`archive ok`) and
  `stderr` (`note: disk 91% full`) tails with `truncated: false`, and the
  3-record ring was respected.
- Summary 952 bytes, history 1517 bytes — far inside the frozen caps.
- `HEAD` returned matching status/`content-type`/`content-length` with no body;
  `POST` returned 405; `/v2/scheduler/run` returned 404.
- `/v2/status` was unaffected, and `greggd stop` shut it down cleanly.

### Footprint

Stripped release `greggd`: **3,306,320** bytes.

~~~text
baseline (Plan 159/160)  3,261,664
Plan 163 integrated      3,306,320   (+44,656 / +1.369%)
Plan 162 ceiling         3,400,000   (+138,336 / +4.24%)  -> WITHIN
~~~

No dependency was added, so MSRV stays 1.89 and no candidate ablation was
required. The remaining per-stage attribution is recorded rather than estimated:
the protocol+config stage alone measured 3,267,776 (+6,112), and the integrated
delta of +44,656 covers the output drain, the bounded history, the publication
cell, and both routes.

### Behavior-preserving runtime checks

- No extra wake when `jobs` is empty: the publisher is created unconditionally
  and answers a valid empty document; `run.rs` still spawns no scheduler task.
- No per-second output/history work: publication is gated on an actual state
  difference, and history is serialized only inside a real publication.
- No idle shutdown change: the child kill/wait path and its bound are unchanged,
  and the `Cancelled` variant is documented as reserved because memory-only
  history cannot outlive the process.

### Documentation reconciled

`README`-adjacent daemon docs (`crates/greggd/README.md`, `docs/daemon.md`),
`architecture/protocol.md` (routes, vocabularies, identity, frozen constants,
security boundary), `architecture/gregg-protocol.md` (module map and constants),
`architecture/greggd-daemon.md` (publication ownership, history, capture
mechanics, configuration), `AGENTS.md`, `CHANGELOG.md`,
`.opencode/skills/greggd-daemon/SKILL.md`, and
`.opencode/skills/protocol-wire/SKILL.md`.

The TUI `c` view is deliberately **not** documented here; that is Plan 166.

### Verification

`cargo test -p greggd --all-features` (480 unit + 3 integration + doctests) and
`cargo test -p gregg-protocol --all-features` (183) green; workspace clippy with
`--all-targets --all-features` produces zero warnings; `./scripts/check-local.sh`
passes. Existing native CI is recorded once on the final Plan 167 SHA.


## Post-closure correction note (2026-10-05)

A later source review of current main at `1aac89f` found one execution-boundary
defect in this otherwise-valid historical implementation record:
`await_child_completion` waits for stdout/stderr EOF as well as the direct
child's exit. A descendant that inherits either write end can therefore retain
greggd's one global scheduled-child slot after the scheduled direct child has
already terminated. The same review found a smaller attribution smell where the
completion path searches by job name and falls back to job index zero even
though `RunningChild` already carries the authoritative index.

Those findings do **not** rewrite this plan's closure evidence or the bounded
capture design. They are registered as Plan 173, which preserves concurrent
bounded draining while the direct child runs and restores the direct-child
termination boundary with bounded post-exit output cleanup.
