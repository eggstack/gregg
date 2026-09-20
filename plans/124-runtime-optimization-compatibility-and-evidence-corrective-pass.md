# Plan 124: runtime optimization compatibility and evidence corrective pass

Status: ready for implementation.

Depends on: completed Plans 120-123 and their implementation at `45582ce`.

## Objective

Close two narrow post-implementation findings from the Plans 120-123 runtime
optimization campaign without reopening its architecture:

1. restore the pre-`45582ce` HTTP stale/failure response-envelope semantics
   for retained snapshots that become stale after collector failure; and
2. make the Plans 120/122/123 closure evidence wording match the evidence that
   was actually recorded, without inventing timing numbers or adding a
   performance benchmark/CI system after the fact.

The performance implementation itself remains valid: one-pass daemon sample
conversion, Arc-preserving publication, owned client normalization, O(N)
ordinary reducer matching, stable counter keys, TUI render/cache reductions,
and publication-time status JSON caching all remain in place.

## Triggering findings

### 1. Stale retained snapshots now replace the collector failure message

Before Plan 123, `ServerState::v1_status_data` and
`ServerState::v2_status_data` cloned the stored health envelope and replaced it
with:

~~~text
category = collector_failure
message = "cached snapshot is stale"
~~~

only when the stored health state was still `Ready`.

That distinction mattered. After `set_failed("failure 3")`, the stored health
state was already `Failed`, so once the retained snapshot crossed the failure
threshold, `/v1/status` and `/v2/status` returned 503 while preserving the
latest collector failure message (for example, `"failure 3"`).

Plan 123 correctly removed the unused ready-health clone from the fresh 200
path, but the new stale branch currently manufactures
`"cached snapshot is stale"` unconditionally whenever a retained snapshot is
stale:

~~~text
if snapshot_is_stale {
    return Unavailable(HealthResponse::failed(
        CollectorFailure,
        "cached snapshot is stale",
    ));
}
~~~

The status code and category are still correct, and existing tests therefore
pass, but the JSON response body changed observably. Plan 123 explicitly
required preservation of stale/failure semantics and no protocol-surface
change, so this is a compatibility defect.

### 2. Closure wording overstates the recorded timing evidence

Plans 120, 122, and 123 require or claim lightweight before/after performance
measurements. Their closure records document deterministic structural evidence,
serialization-count evidence, release sizes, and that descriptive release-mode
loops were run, but they do not record numerical before/after timing results.

Do not invent missing timing values.

The planning record should distinguish:

- deterministic structural work-reduction evidence;
- exact release binary sizes;
- test/CI evidence;
- descriptive ad hoc timing runs that were not retained as numerical gates.

This is a record-correction task, not a reason to create a benchmark framework
or rerun historical measurements solely to manufacture numbers.

## Required behavior contract

### Collector-failure stale path

When a retained snapshot is stale and the corresponding stored health metadata
is already `Failed` with `CollectorFailure`:

- `/v1/status` and `/v2/status` return 503;
- the response state remains `Failed`;
- the category remains `CollectorFailure`;
- the response message remains the latest stored collector failure message;
- the response carries no snapshot in the health envelope;
- cached successful status bytes are not served.

This must match the pre-Plan-123 behavior.

### Ready-but-age-stale path

When the stored health state is still `Ready` but the snapshot is stale due
to age, a pre-epoch clock, or a future `observed_at` after a backward clock
jump:

- return 503;
- synthesize the existing stale health envelope;
- retain the message `"cached snapshot is stale"`.

This was the pre-Plan-123 behavior and must remain.

### NotServing path

Version-unavailable semantics remain unchanged:

- Windows/v2-only publication keeps v1 `NotServing`;
- v1-only compatibility publication keeps v2 `NotServing`;
- a collector failure must not overwrite the unavailable-version category or
  message.

### Failure below threshold

When failure count/age policy still permits serving the retained snapshot:

- `/v1/status` or `/v2/status` remains 200;
- the publication-time cached status body remains reusable;
- `/healthz` or `/v2/healthz` remains failed as before;
- no additional status serialization is introduced.

## Implementation approach

### A. Reuse stored health metadata on stale failure

Refactor the private v1/v2 status-data decision so stale handling distinguishes
the current health state.

A small helper on `HealthMetadata`, or equivalent local logic, should express:

~~~text
if stale {
    if health state is Ready:
        synthesize "cached snapshot is stale"
    else:
        reconstruct the stored non-ready health response without a snapshot
}
~~~

Do not restore the old behavior of cloning a complete ready
`HealthResponse`/snapshot on every fresh status request.

The fresh fast path must remain:

~~~text
FreshCached(Bytes)
or
FreshTyped(Arc<...>) only as serialization fallback
~~~

The correction must therefore preserve Plan 123's allocation/serialization
win.

### B. Keep status and health paths consistent without merging them again

The current dedicated `v1_health_data` / `v2_health_data` split is useful
because health endpoints intentionally construct typed health envelopes.

Keep that separation.

After the correction:

- status and health endpoints must agree on state/category/message whenever
  both return 503 for the same failure;
- health endpoints may continue to construct their response on demand;
- fresh status endpoints must not construct an unused ready health envelope.

### C. Strengthen exact-wire regression tests

Extend the existing server tests rather than creating a new harness.

Required v1 tests:

1. Publish a fresh dual-version snapshot.
2. Call `set_failed("failure 1")`, `set_failed("failure 2")`,
   `set_failed("failure 3")` with threshold 3.
3. Assert `/v1/status` is 503 and deserialize the complete
   `HealthResponse`.
4. Assert:
   - `state == Failed`;
   - `category == CollectorFailure`;
   - `message == Some("failure 3")`;
   - `snapshot.is_none()`.
5. Assert `/healthz` returns the same state/category/message.

Required v2 equivalent:

- cover `/v2/status` and `/v2/healthz`;
- include the v2-only publication path so Windows-style serving semantics are
  locked down.

Required age-stale tests:

- with health still `Ready`, force an age-stale snapshot and assert the
  message remains exactly `"cached snapshot is stale"`;
- retain the existing pre-epoch/backward-clock tests and add an exact message
  assertion where the response body is available.

Required below-threshold tests:

- one collector failure under a threshold of three still returns the cached
  snapshot with 200;
- serialization-count instrumentation from Plan 123 does not increment on
  repeated 200 requests.

Required unavailable-version tests:

- v2-only publication still returns v1 `NotServing` with
  `V1_UNAVAILABLE_MESSAGE`;
- a subsequent collector failure does not convert that response into generic
  `CollectorFailure`.

### D. Reconcile the planning evidence record truthfully

Update Plans 120, 122, and 123 only where their current wording implies
numerical timing evidence that is not actually recorded.

Preserve historical implementation facts:

- implementation SHA `45582ce`;
- Rust/toolchain/target information already recorded;
- focused/full test counts;
- structural evidence;
- serialization-count evidence;
- final stripped sizes;
- CI run provenance.

Replace claims such as "before/after measurements are recorded" with wording
that matches the actual record, for example:

- "deterministic structural work-reduction evidence and release sizes are
  recorded";
- "descriptive release-mode timing loops were run but no timing value is used
  or retained as a gate."

Do not fabricate elapsed times, throughput values, medians, percentage speedups,
or baseline numbers.

Do not rewrite accurate historical records from Plans 121-123 beyond the
minimum wording needed for truthful closure.

### E. Reconcile Plan 120-124 status/index after implementation

Once the code/tests pass:

- mark Plan 124 complete with implementation SHA and one ordinary CI run;
- amend Plan 123 status/closure with a short note that Plan 124 restores the
  stale failure-message compatibility edge;
- amend Plan 120 roadmap closure to state that the campaign is closed through
  Plan 124;
- update `plans/README.md` accordingly.

Do not rewrite `45582ce` as a failed implementation. Its performance work
remains the implementation baseline; Plan 124 is a bounded post-closure
compatibility correction.

## Explicit non-goals

Do not include:

- scheduler redesign or concurrency changes;
- collection cadence or collector freshness changes;
- changes to publication-time `Bytes` caching;
- removal of the Plan-123 serialization cache;
- additional HTTP caching semantics, ETags, compression, or protocol fields;
- changes to status codes, routes, content type, v1/v2 schema, or serde shape;
- TUI cache/layout redesign;
- further dirty-redraw precision for rejected old batches or action boundary
  no-ops;
- persistent benchmark infrastructure, Criterion/iai, CI performance gates, or
  generated evidence artifacts;
- release-profile, dependency, MSRV, installer, or distribution changes;
- a new closure-only plan after this pass.

The remaining dirty-redraw micro-optimization is intentionally deferred: it is
not a correctness regression and does not justify reopening this campaign.

## Verification

Focused server verification:

~~~text
cargo test -p greggd server
cargo test -p greggd stale_snapshot
cargo test -p greggd status_serialization
~~~

Use the actual test filters available after implementation; do not add duplicate
tests merely to satisfy these example command names.

Then run:

~~~text
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

No release preflight is required unless implementation changes release-facing
files beyond documentation. No new workflow/job/matrix is required.

Run one ordinary existing CI workflow at the final source state.

## Acceptance criteria

- [ ] Failure-threshold stale v1 status preserves the latest stored collector
      failure message exactly.
- [ ] Failure-threshold stale v2 status preserves the latest stored collector
      failure message exactly.
- [ ] Matching health endpoints return the same failed state/category/message.
- [ ] Ready-but-age-stale responses retain exactly
      `"cached snapshot is stale"`.
- [ ] Version-unavailable `NotServing` semantics remain unchanged through
      later collector failures.
- [ ] Failure-below-threshold status responses still serve cached 200 snapshot
      bytes without additional serialization.
- [ ] Fresh status requests still avoid constructing/cloning a ready health
      envelope.
- [ ] Publication-time v1/v2 `Bytes` caching and serialization-count behavior
      from Plan 123 remain intact.
- [ ] No public protocol, route, status-code, content-type, typed
      `ServerState` API, scheduler, collector, or TUI behavior changes.
- [ ] Plans 120/122/123 no longer claim unrecorded numerical before/after
      timing evidence.
- [ ] No timing or throughput numbers are fabricated during record
      reconciliation.
- [ ] Focused tests, workspace tests, strict clippy, and the default local
      check pass.
- [ ] One ordinary existing CI run is green at the final implementation SHA.
- [ ] Plan 120 and Plan 123 closure wording points to Plan 124 as the bounded
      corrective closeout.

## Closure record

When implemented, append:

- implementation SHA;
- exact stale-failure v1/v2 regression tests added;
- exact age-stale and NotServing compatibility tests retained/strengthened;
- Plan-123 serialization-count regression result;
- workspace/clippy/local-check result;
- the planning-record wording corrected;
- one ordinary CI run.

Do not create a Plan 125 solely to restate closure.
