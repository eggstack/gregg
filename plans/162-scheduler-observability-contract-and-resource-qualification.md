# Plan 162: Scheduler observability contract and resource qualification

Status: planned.

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

- [ ] Exact summary/history route contract is recorded.
- [ ] Exact live-state and terminal-outcome enums are recorded.
- [ ] Restart discriminator + sequence deduplication contract is fixed.
- [ ] scheduler_history_limit, or recorded replacement name, defaults to 5.
- [ ] Exact hard history maximum is selected and memory-qualified.
- [ ] Exact stdout/stderr byte caps and tail/truncation behavior are selected.
- [ ] Maximum history response/body cap is calculated and recorded.
- [ ] Output security/default-publication policy is explicit.
- [ ] Publication ownership does not couple HTTP handlers to scheduler mutation.
- [ ] Pre-feature daemon 404 compatibility is explicit.
- [ ] Stripped binary measurements are recorded against 3,261,664 bytes.
- [ ] Plan 163 has a numeric footprint rule rather than an implicit exception.
- [ ] MSRV remains Rust 1.89 and no unnecessary dependency is introduced.
- [ ] ./scripts/check-local.sh passes for any kept code changes.

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
