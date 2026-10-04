# Plan 163: greggd scheduler history and read-only API

Status: planned.

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

- [ ] Default remote history depth is five and configured bounds match Plan 162.
- [ ] Scheduler history is entirely memory-only.
- [ ] stdout/stderr capture is fixed-capacity and concurrently drained.
- [ ] Large child output cannot deadlock the scheduler or grow memory
      proportionally.
- [ ] Live state truthfully distinguishes the qualified pending/running states.
- [ ] Terminal history includes all qualified non-child failures.
- [ ] Scheduler summary/history routes are read-only and separate from status.
- [ ] Empty configured-job set returns valid empty scheduler documents.
- [ ] Existing metrics/health/v1/v2 behavior remains unchanged.
- [ ] Protocol fixtures/validation cover the new documents.
- [ ] Plan-160 clock semantics remain green.
- [ ] Existing scheduler/security/shutdown invariants remain green.
- [ ] Stripped greggd meets Plan 162's recorded footprint rule.
- [ ] Default local checks and the existing native CI jobs pass as appropriate.
- [ ] Active daemon/protocol docs are reconciled.

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
