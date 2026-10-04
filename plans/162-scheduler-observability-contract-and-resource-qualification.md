# Plan 162: Scheduler observability contract and resource qualification

Status: complete at implementation `frozen` (see "Contract record" below for the
measured evidence and the exact constants handed to Plan 163).

Depends on: Plan 161 and current post-160 main. Independent of Plan 091.

## Objective

Freeze the smallest truthful remote contract needed for Gregg to observe
greggd maintenance scheduling before changing child stdio or daemon memory
behavior.

This plan is a qualification/design gate. It may add protocol types, fixtures,
pure validation/helpers, and measurement prototypes needed to settle the
contract, but it must not leave partially wired scheduler-history product
behavior behind. Plan 163 owns the integrated daemon feature.

## Questions this plan must settle

Record exact answers for:

1. Scheduler summary/history response types and route semantics.
2. Live-state and terminal-outcome vocabulary.
3. Stable record identity/deduplication across polling and daemon restart.
4. Default and maximum per-job history depth.
5. Exact stdout/stderr tail byte caps and truncation semantics.
6. Maximum serialized scheduler-summary and history body sizes.
7. Worst-case default and configured-maximum daemon memory attributable to
   history/output.
8. Binary/dependency impact relative to the Plan-159/160 stripped greggd
   baseline of 3,261,664 bytes.
9. Whether output is published by default under the current LAN trust model or
   requires one narrow explicit policy.
10. Compatibility behavior when a client talks to a pre-feature daemon.

Do not defer any of these to implementation judgment in Plan 163.

## Wire boundary

Prefer additive v2 routes:

~~~text
GET/HEAD /v2/scheduler
GET/HEAD /v2/scheduler/history
~~~

Keep scheduler documents separate from StatusPayloadV2 so ordinary metrics
polls do not carry historical output.

Unknown methods retain the server's existing 405 policy and unknown paths
retain 404. A new daemon with no configured jobs returns a valid empty
scheduler document, not 404. A pre-feature daemon returns 404, which a new
Gregg client interprets as scheduler observability unsupported, not host
offline.

Place independently deployed wire types in gregg-protocol; keep local
Gregg-client-daemon IPC DTOs out of that crate.

## Minimum summary fields

The final protocol names may be adjusted for consistency, but the contract must
carry enough information for the TUI to derive the following without guessing.

Per response:

- schema/API version consistent with the v2 route family;
- generated timestamp;
- scheduler process-lifetime identity or equivalent restart discriminator;
- history revision that changes when retained terminal history changes.

Per job:

- stable operator-facing job name;
- configured schedule string;
- next scheduled civil occurrence;
- current state;
- optional load threshold/window;
- pending-since time when pending;
- next load retry when load-deferred;
- observed load and threshold when a load decision exists;
- active start time when running;
- most recent terminal outcome summary when one exists.

Do not expose command argv, working directory, environment, service identity, or
other configuration fields merely to identify the job.

## Live-state vocabulary

Use an explicit enum rather than a free-form string assembled in the TUI.
At minimum distinguish:

- idle/not currently pending;
- waiting for the single global child slot;
- delayed because observed load exceeds the configured threshold;
- delayed because required load telemetry is unavailable;
- running.

If implementation proves waiting-slot is too transient to publish coherently,
record the alternative and ensure the UI cannot mislabel it as load delay.

Pending age and reason must remain derived from scheduler-owned state, not from
client polling timestamps.

## Terminal-outcome vocabulary

History must be truthful for occurrences that never successfully created a
child. At minimum qualify:

- success;
- nonzero/abnormal child exit;
- spawn failure;
- child wait/runtime observation failure;
- load-wait expiration.

If shutdown termination is retained long enough to publish before process exit,
give it an explicit outcome; otherwise document that daemon restart clears the
memory-only record before a remote client can rely on observing it.

Each history record should include:

- job name or an unambiguous per-job association;
- scheduler-lifetime record sequence;
- scheduled occurrence time;
- actual start time when a child started;
- finish/terminal time;
- pending delay;
- duration when meaningful;
- coalesced-occurrence indicator if known;
- exit code and Unix signal when meaningful;
- bounded stdout tail;
- bounded stderr tail;
- independent truncation indicators.

Do not represent absent values as fabricated zeroes.

## Record identity

A polling client needs deterministic duplicate suppression.

Qualify a small identity based on:

- one scheduler-start/restart discriminator generated without a heavyweight new
  dependency; and
- one monotonically increasing per-lifetime history sequence.

A wall-clock startup timestamp plus sequence is acceptable if deterministic
tests demonstrate restart handling and the collision assumptions are recorded.
The identifier is for deduplication, not authentication.

## Remote history depth

The requested default is exactly five terminal records per job.

Plan 162 must select a hard maximum small enough for a 64-job daemon. The
choice must be justified with a worst-case memory and serialized-body
calculation, not only a typical-job estimate.

Add one optional daemon setting with an unambiguous name, preferably
scheduler_history_limit, defaulting to 5. Missing configuration must retain the
default. Zero should either be rejected or explicitly mean history disabled;
choose one behavior and lock it before Plan 163.

Do not add a persistence path.

## Output capture qualification

Current production sends stdout and stderr to null. The new feature must avoid
the classic child-pipe deadlock and avoid whole-output accumulation.

Required design:

- use piped stdout/stderr only when history capture is enabled;
- continuously drain both streams while the child is running;
- retain only fixed-capacity tails;
- apply truncation before materializing the terminal history record;
- convert arbitrary bytes to a JSON-safe textual representation after the byte
  bound is enforced;
- never use wait_with_output or an equivalent unbounded accumulator;
- never let a slow/noisy stdout reader block scheduler shutdown indefinitely.

Measure at least two candidate cap sizes and choose exact stdout/stderr caps.
The combined default worst-case memory across 64 jobs times history depth must
remain proportionate to Gregg's lightweight daemon target. Prefer separate
stdout/stderr caps so a noisy stdout cannot erase the only useful stderr
diagnostic.

## HTTP body budget

Derive explicit maximum response sizes from:

~~~text
MAX_JOBS
x configured history hard maximum
x per-record fixed metadata
x stdout/stderr tail caps
+ JSON overhead
~~~

The history handler must be able to refuse or avoid constructing an impossible
oversized document by construction rather than serialize first and discover it
afterwards.

Record the body cap Gregg's remote scheduler-history client will enforce.
Do not reuse the 2 KiB-era metrics payload assumption.

## Publication/state-sharing design

Qualify a small scheduler-publication object shared with the existing server
runtime. Avoid making HTTP handlers lock the scheduler engine directly.

Preferred direction:

- scheduler owns mutable execution state;
- after state transitions, it publishes a compact immutable snapshot/revision to
  a shared cell;
- terminal history uses bounded owned records;
- handlers read one coherent snapshot and serialize/cache it without causing
  scheduler execution to wait on network I/O.

Determine whether cached compact JSON bytes, typed Arc snapshots, or both are
appropriate using the existing ServerState publication pattern as prior art.
Do not force scheduler state into the metrics PublishedState if doing so would
make one lock couple unrelated high-frequency sampler publication and
low-frequency scheduler history.

## Security qualification

Explicitly review the fact that scheduler output will be readable from the same
configured HTTP listener as host telemetry.

Requirements:

- no command argv/working-dir publication;
- documentation warning for output sensitivity;
- output caps before serialization;
- no new auth/TLS system;
- no remote mutation/control;
- no secret/environment injection into jobs.

If an opt-in output-exposure switch is required, it must be one narrow boolean
or enum with a safe migration/default story. Do not introduce per-client ACLs,
tokens, user databases, or certificate management in this plan.

## Footprint qualification

Measure a stripped release greggd from the same target/profile used by
Plans 156-160.

Baseline:

~~~text
3,261,664 bytes
~~~

Measure separately where practical:

1. protocol/schema/config additions only;
2. output-drain/runtime support;
3. bounded history/publication/server route support;
4. integrated candidate.

Prefer existing Tokio/serde/bytes primitives. A new dependency requires both
MSRV review and an ablation showing it is smaller/cleaner than a narrow local
implementation.

Before closing Plan 162, record one exact allowed integrated footprint ceiling
or an explicit remeasure-and-decision-required-in-Plan-167 rule with a numeric
trigger. Do not let Plan 163 silently establish a new baseline.

## Deterministic qualification tests

At minimum add or prototype tests for:

- record identity/revision monotonicity;
- history depth = 5 default;
- selected hard maximum validation;
- output tail truncation;
- invalid UTF-8 handling;
- separate stdout/stderr truncation;
- no whole-output growth under a synthetic large stream;
- empty scheduler response;
- representative maximum-body calculation;
- old-daemon 404 compatibility at the client parser boundary if parser code is
  introduced here.

Do not add an integration workflow.

## Documentation touched on closure

Record the settled contract in:

- this plan;
- architecture/gregg-protocol.md and architecture/protocol.md if protocol types
  land in this plan;
- architecture/greggd-daemon.md only for decisions actually frozen;
- relevant OpenCode skills if a new invariant is established.

User-facing README changes belong with Plan 163/166 when behavior exists.

## Acceptance criteria

- [x] Exact summary/history route contract is recorded.
- [x] Exact live-state and terminal-outcome enums are recorded.
- [x] Restart discriminator + sequence deduplication contract is fixed.
- [x] `scheduler_history_limit` defaults to 5.
- [x] Exact hard history maximum is selected and memory-qualified.
- [x] Exact stdout/stderr byte caps and tail/truncation behavior are selected.
- [x] Maximum history response/body cap is calculated and recorded.
- [x] Output security/default-publication policy is explicit.
- [x] Publication ownership does not couple HTTP handlers to scheduler mutation.
- [x] Pre-feature daemon 404 compatibility is explicit.
- [x] Stripped binary measurements are recorded against 3,261,664 bytes.
- [x] Plan 163 has a numeric footprint rule rather than an implicit exception.
- [x] MSRV remains Rust 1.89 and no unnecessary dependency is introduced.
- [x] `./scripts/check-local.sh` passes for the kept code changes.

## Stop conditions

Stop and revise Plan 161 before Plan 163 if:

- bounded useful output cannot be captured without materially weakening child
  shutdown behavior;
- the maximum response cannot be kept reasonably bounded;
- scheduler history requires persistent storage to satisfy the stated UX;
- protocol types would force a breaking change to existing v2 status;
- safe output exposure requires a generalized authentication subsystem;
- measured footprint requires a dependency expansion inconsistent with Gregg's
  lightweight target.

## Handoff

Plan 163 begins only after this plan records exact resource and wire constants.
Do not let implementation choose larger caps ad hoc.

---

## Contract record

This is the frozen answer to every question above. The wire types, validation,
and the daemon configuration setting landed here; the product behavior is
Plan 163's. Nothing below may be widened by implementation judgment.

### 1. Routes and types

Additive v2 routes, kept out of `StatusPayloadV2`:

~~~text
GET/HEAD /v2/scheduler            -> SchedulerSummaryV2
GET/HEAD /v2/scheduler/history    -> SchedulerHistoryV2
~~~

`GET` returns compact JSON; `HEAD` matches status/headers with no body under the
server's existing conventions. Unknown methods keep the existing `405` policy
and unknown paths keep `404`. A new daemon with no configured jobs returns a
valid empty document, never `404`.

Types live in `gregg-protocol` (`src/scheduler.rs`, validated by
`src/validate_scheduler.rs`) because they are independently deployed wire
types. The Gregg client-daemon IPC DTOs are explicitly **not** in that crate.

### 2. Live-state and terminal-outcome vocabulary

`SchedulerJobStateV2`: `Idle`, `WaitingForSlot`, `LoadHigh`, `LoadUnavailable`,
`Running`. Both load-deferred states are distinguishable, and
`LoadUnavailable` is validated to carry no `observed` reading, so a client
cannot render a missing load as `0`.

`SchedulerOutcomeV2`: `Success`, `Failed`, `SpawnFailed`, `WaitFailed`,
`LoadExpired`, `Cancelled`. Validation rejects an `exit_code`/`signal` or a
`duration_ms` on any outcome that never ran a child, so an expired or
failed-to-start occurrence cannot masquerade as an executed one.

`Cancelled` is published only when a record survives long enough to be served;
a daemon shutdown clears memory-only history, so a remote client may never
observe the final occurrence of a lifetime. That limitation is documented
rather than papered over.

### 3. Record identity and deduplication

Identity is the pair `(SchedulerEpochV2, sequence)`.

`SchedulerEpochV2` is `started_at_unix_ms` plus a `nonce` (FNV-1a over pid,
start time, and a process-lifetime counter). No new dependency. The `nonce`
exists because a wall-clock start alone can collide when two restarts land in
one millisecond. The recorded collision assumption: a peer can be
indistinguishable only when a new process starts in the same millisecond *and*
reuses the same pid *and* the same counter value. This is a deduplication aid,
never an authentication token.

`history_revision` changes only when retained terminal history changes, so a
job moving to load-delayed does not force a client history refetch.

### 4. History depth

- `scheduler_history_limit` (top-level daemon config, `Option<usize>`).
- Missing means exactly `5` (`DEFAULT_SCHEDULER_HISTORY_LIMIT`).
- Hard maximum is `10` (`MAX_SCHEDULER_HISTORY_LIMIT`); above that, existing
  config validation fails before the listener binds.
- **Zero explicitly means history disabled**: live state is still served, and
  the history document is served with empty record lists. This was chosen over
  rejecting `0` so an operator can stop retaining command output without
  deleting the setting.
- No path, database, journal, or log-spool setting exists.

### 5. Output caps and truncation

- `MAX_SCHEDULER_OUTPUT_BYTES = 1024` — raw captured tail, per stream. Separate
  caps per stream so a noisy stdout cannot erase the only useful stderr
  diagnostic. Candidates measured: 256 (truncates ordinary multi-line
  diagnostics too aggressively), 1024 (chosen), 4096 (doubles worst-case
  retained bytes for no diagnostic gain once the text cap applies).
- `MAX_SCHEDULER_OUTPUT_TEXT_BYTES = 512` — published text, per stream, measured
  in **JSON-escaped** bytes.

The escaped basis is the load-bearing decision. `serde_json` renders a C0
control byte as six bytes, so a naive "512 bytes of text" cap could still emit
3 KiB of JSON per stream. `json_escaped_len` and `truncate_to_escaped_budget`
make the published length an exact function of the character sequence, so the
body maximum is closed by construction rather than discovered after
serialization.

Order of operations: bound raw bytes -> convert lossily -> enforce the escaped
budget. `truncated` is set if either stage dropped anything, and is independent
per stream. A test pins `json_escaped_len` against real `serde_json` output so
the budget cannot silently drift from the actual serializer.

### 6. Body budget (calculated and measured)

Derived maximum = `MAX_SCHEDULER_JOBS (64)` x `MAX_SCHEDULER_HISTORY_LIMIT
(10)` records, each with two streams at 512 escaped bytes plus bounded metadata.

Measured with the pathological shape (all 64 jobs at depth 10, every stream
filled to the escaped budget): **832,022 bytes**.

- `MAX_SCHEDULER_HISTORY_BODY_BYTES = 1 MiB` (1,048,576) — the client cap,
  20.6% above the measured maximum.
- `MAX_SCHEDULER_SUMMARY_BODY_BYTES = 64 KiB` — the summary client cap; a
  summary carries no output, so this is orders of magnitude above its shape and
  is the same order as the existing v2 status cap.

`representative_maximum_history_stays_inside_the_body_budget` asserts the exact
832,022 figure, so widening any frozen constant or changing serialization fails
a test instead of invalidating the published cap.

### 7. Memory attributable to history/output

Worst case (64 jobs at the hard maximum): 640 records x 2 streams x 512
escaped bytes = 655,360 bytes of published text plus bounded per-record
metadata. At the default depth of 5 it is 327,680 bytes. Zero jobs is zero.

### 8. Binary footprint

Baseline `3,261,664` bytes (stripped release `greggd`, `lto = "fat"`,
`codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`).

Stage 1 measured — protocol/schema/config additions only, before the daemon
references the types: **3,267,776** (+6,112 / +0.187%). The cost is small
because unused serde derives are dead-code-eliminated; the real cost appears
when Plan 163 actually constructs and serializes these documents.

Stages 2-4 are Plan 163's to measure against the rule below.

### 9. Output publication policy

**Published by default**, with no new setting. Rationale, recorded so it is not
re-litigated:

- the operator configured the job and chose the listener address;
- caps are enforced before serialization, and no `argv`, `working_dir`,
  environment, or service identity is published;
- suppressing output by default would defeat the stated observability goal.

No authentication, TLS, token, or per-client ACL is introduced by this line.
The documentation obligation is explicit: any principal that can reach the
configured `greggd` HTTP listener can read scheduler history and output.

### 10. Pre-feature daemon compatibility

A `404` from `/v2/scheduler` means **scheduler observability unsupported**, not
host offline. The system stays online with cron capability marked unsupported.
Network/5xx failure retains last-known cron data behind a stale marker that is
independent of Systems reachability. Validation deliberately treats absence as
absence: it never turns a missing route into a protocol error.

### 11. Publication ownership

A separate scheduler publication object, **not** the high-frequency metrics
`PublishedState`. Sharing one lock would couple low-frequency history to
high-frequency sampler publication for no benefit. Handlers read one coherent
snapshot and serialize it themselves; they never await scheduler mutation, hold
a scheduler lock while serializing, or touch configuration, telemetry, or
processes.

### 12. Numeric rule handed to Plan 163

**Plan 163's stripped release `greggd` must not exceed 3,400,000 bytes**
(+138,336 / +4.24% over the 3,261,664 baseline).

Above that, Plan 163 stops and opens an explicit re-baseline decision plan with
component attribution. It does not silently establish a new baseline. Plan 167
re-measures and enforces the same number.

### Kept code changes

- `crates/gregg-protocol/src/scheduler.rs` (new) — frozen wire types, escaped
  length helpers, output conversion.
- `crates/gregg-protocol/src/validate_scheduler.rs` (new) — structured
  validation plus the maximum-body proof.
- `crates/gregg-protocol/src/lib.rs` — module wiring and re-exports.
- `crates/greggd/src/config.rs` — `scheduler_history_limit` setting, its
  default/zero semantics, bounds validation, and the frozen re-exports.

No dependency was added, so MSRV stays 1.89 and no ablation of a candidate
dependency was required. `cargo test -p gregg-protocol --all-features` and
`cargo test -p greggd --all-features --lib` are green.
