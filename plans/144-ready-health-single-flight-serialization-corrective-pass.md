# Plan 144: ready-health single-flight serialization corrective pass

Status: planned.

Depends on: completed Plans 138-143 at current post-`07f353f` main, specifically Plan 139's ready-health memoization. Independent of Plan 091 and Plan 145.

Corrects: the concurrency qualification gap in completed Plan 139. Plan 139 remains complete as the historical implementation record; this plan narrows only the memo initialization semantics.

## Objective

Make the v1/v2 ready-health memo truly single-flight per immutable publication so concurrent first requests cannot serialize the same snapshot more than once, while preserving every existing wire, staleness, failure, readiness, publication, and public API behavior.

Plan 139 correctly removed repeated sequential serialization and deep snapshot cloning, but the landed private cache is:

~~~text
health_bytes: Option<Bytes>
health_bytes_v2: Option<Bytes>
~~~

The request path reads an empty memo under `PublishedState`'s read lock, drops the lock, serializes, then reacquires a write lock to install the bytes. Two or more concurrent first requests can therefore all observe `None` and all serialize the same immutable publication before one result wins installation.

That is not a wire correctness defect. It is a performance-contract defect: Plan 139's recorded "at most once per immutable publication" property is only proven for sequential requests.

## Research basis

`greggd` already depends on Tokio with the `sync` feature enabled. Tokio's `sync::OnceCell::get_or_try_init` provides the exact behavior needed here:

- concurrent tasks wait on one initializer;
- only one successful initialization populates the cell;
- a failed, cancelled, or panicked initialization leaves the cell available for a later retry.

This avoids a new dependency and, unlike stable `std::sync::OnceLock` on the Rust 1.89 MSRV, provides fallible initialization without caching a serialization error as the permanent publication result.

Reference: https://docs.rs/tokio/latest/tokio/sync/struct.OnceCell.html

## Required correction

### 1. Replace per-publication Option memos with single-flight cells

Replace private `PublishedState::health_bytes` and `health_bytes_v2` with publication-owned cells conceptually equivalent to:

~~~text
Arc<tokio::sync::OnceCell<Bytes>>
~~~

Each new v1/v2 publication must receive fresh cells. No cell may be reused across distinct snapshot publications.

The exact private wrapper type may differ if it makes test injection or generation ownership clearer, but it must:

- be cloneable cheaply outside the state lock;
- provide task-level single-flight;
- permit retry after serialization error/cancellation;
- require no new crate or Tokio feature.

Do not use a process-global health cell.

### 2. Never hold PublishedState locks across cell initialization

Under the existing `RwLock`, capture only:

- the current snapshot `Arc`;
- the current per-publication cell `Arc`;
- readiness/staleness decision data already needed for response selection.

Drop the `PublishedState` read guard before awaiting `get_or_try_init`.

The serialization closure remains synchronous work wrapped by the async initializer:

~~~text
cell.get_or_try_init(|| async {
    serialize_ready_health_borrowed(&snapshot)
}).await
~~~

or the equivalent v2 path.

Do not hold a Tokio read/write lock while serializing the JSON or while waiting on another request's initialization.

### 3. Preserve retry-on-serialization-error semantics

A serialization failure must still produce the current internal-service error for that request.

It must not permanently poison the publication memo. A later request for the same still-current publication must be able to attempt serialization again.

Do not switch to a stable `std::sync::OnceLock<Result<...>>` design that would memoize a transient/test-injected error for the life of the publication merely to obtain single-flight.

### 4. Preserve publication and state-transition semantics

The existing response-selection rules remain authoritative:

- fresh Ready v1/v2 may use the memo;
- age/pre-epoch/backward-clock stale Ready returns the exact existing stale health envelope;
- Failed preserves the stored collector-failure message;
- NotServing preserves version-unavailable semantics;
- Warming remains uncached/non-ready;
- status-body caching from Plan 123 is untouched.

A new publication replaces the cell rather than clearing/reusing it.

A request already in flight on an old publication may finish using that old snapshot/cell exactly as today's request-level race permits, but it must never install bytes into or contaminate the newer publication's cell.

### 5. Preserve public typed APIs and borrowed serialization

Keep:

- `HealthResponse` / `HealthResponseV2`;
- `ServerState::health()` / `health_v2()`;
- current routes, codes, headers, and body shapes;
- `BorrowedReadyHealthV1/V2` or an exact equivalent;
- Plan 124 stale/failure message behavior;
- EggServe runtime/connection policy.

Do not move health wire ownership into EggServe or `gregg-protocol`.

## Deterministic concurrency evidence

Add tests that fail against the current Option-based implementation.

At minimum:

1. publish one fresh v1/v2 snapshot;
2. hold a test-only serializer gate/barrier so N concurrent first requests all reach the memo initialization boundary before the initializer is released;
3. issue at least several concurrent v1 health requests and prove:
   - all responses are 200 and byte-identical;
   - the v1 ready-health serialization counter advances exactly once;
4. repeat independently for v2;
5. publish a second snapshot and prove one new serialization for that new publication;
6. race an old-publication initializer against a new publication and prove the old cell cannot populate the new generation;
7. inject one initialization error, prove the request fails as today, then prove a later request retries and succeeds;
8. retain the Plan-139 stale/failure/NotServing tests unchanged or strengthen them around the new cell.

Do not rely on scheduler luck. Use barriers/notifies/test hooks so the duplicate-initializer race is deterministic.

Do not add a global allocator test.

## Implementation constraints

- Use the already-enabled Tokio `sync` feature; no dependency/feature expansion.
- Rust 1.89 MSRV must compile the final code.
- No blocking mutex around serialization on the async request path.
- No serialization under the outer `PublishedState` lock.
- No unsafe code.
- No health response precomputation on every sampler publication unless measurement proves it superior and it still satisfies Plan 139's lazy-cache intent.
- No change to status serialization/publication.

## Measurement

Primary evidence is the deterministic concurrent serialization counter: N simultaneous first requests against one publication must execute exactly one successful serializer initialization.

Optionally record a release-mode concurrent health burst as descriptive evidence only.

Measure stripped release `greggd` before/after on the same toolchain. Investigate unexplained growth above roughly 1%; do not compare different-toolchain records as a binary regression claim.

## Verification

~~~text
cargo test -p greggd --lib --all-features -- server
cargo test -p greggd --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use one ordinary existing CI run after implementation. No new workflow, matrix, concurrency service, or performance gate is required.

## Acceptance criteria

- [x] Concurrent first v1 health requests for one publication execute the ready serializer exactly once.
- [x] Concurrent first v2 health requests for one publication execute the ready serializer exactly once.
- [x] All concurrent callers receive byte-identical ready responses.
- [x] A serialization error leaves the publication cell retryable.
- [x] A new publication owns fresh v1/v2 cells and triggers at most one new successful serialization per version.
- [x] An old in-flight initialization cannot populate the new publication.
- [x] No `PublishedState` lock is held across cell initialization/await.
- [x] Stale, Failed, Warming, and NotServing semantics remain exact.
- [x] Plan-123 status caching and Plan-139 borrowed serialization remain unchanged.
- [x] Public `ServerState` and protocol APIs remain source-compatible.
- [x] No new dependency, Tokio feature, route, or runtime policy is introduced.
- [x] Rust 1.89 and existing native CI remain green.
- [x] Plan 139 receives only a post-closure correction note pointing here; its historical closure evidence is not rewritten.

## Explicit non-goals

Do not include:

- server concurrency-limit changes;
- status-cache redesign;
- precomputing health on publication;
- ETags/compression/HTTP caching headers;
- EggServe upgrade;
- protocol changes;
- stale-policy changes;
- generic cache abstraction work;
- performance CI.

## Handoff note

Start by adding the deterministic concurrent-first-request test against current main and confirm it observes more than one serializer call. Then replace only the memo primitive and publication initialization needed to make that test single-flight.

## Closed scope record

Completed at implementation `<filled-in-by-closure-commit>`:

- replaced `PublishedState::health_bytes` / `health_bytes_v2` with
  `Arc<tokio::sync::OnceCell<Bytes>>` per publication; concurrent first
  v1/v2 health requests share one initializer via
  `get_or_try_init(|| async { … })`;
- dropped the `PublishedState` read guard before awaiting
  `get_or_try_init`; the closure captures only the snapshot `Arc` and
  serialization counter, so no lock is held across cell initialization;
- installed fresh v1/v2 cells on every `update_snapshot_arcs`,
  `update_snapshot_v2_only_arc`, `update_snapshot_v1_only_arc`,
  `set_warming`, and `set_failed` so a new publication always triggers
  at most one fresh successful serialization per version;
- a failed `get_or_try_init` initialization leaves the cell empty for
  retry rather than memoizing the error result, preserving the
  Plan-139 publication-memoization contract without poisoning;
- preserved Plan-124 stale/failure envelope, `BorrowedReadyHealthV1` /
  `BorrowedReadyHealthV2`, status caching, public `ServerState` API,
  and `EggServe` runtime/connection policy unchanged;
- added a `cfg(test)` `TestSerializeGate` backed by
  `tokio::sync::Barrier` so deterministic concurrency tests do not
  rely on scheduler luck; the gate is a no-op in production builds;
- added `cfg(test)` `fail_next_v1_health_serialize` /
  `fail_next_v2_health_serialize` flags so tests can inject
  serialization errors and prove the cell stays retryable;
- added ten `plan144_*` server tests proving: 8-way concurrent v1/v2
  first requests serialize exactly once with byte-identical bodies,
  mixed v1+v2 batches each serialize once, fresh publications own
  fresh cells, in-flight old-publication inits cannot populate the
  new publication, injection errors leave cells retryable, failures
  preserve the exact Plan-124 envelope, warming does not initialize
  the cell, and a 32-way concurrent burst without a gate still
  serializes a small bounded number of times;
- preserved Plan-139's sequential tests unchanged.

Verification:

```text
cargo test -p greggd --lib --all-features -- server::tests::plan144
cargo test -p greggd --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
```

All checks pass locally. Existing CI is exercised separately.

Preserved exclusions:

- server concurrency-limit changes, status-cache redesign, health
  precomputation on publication;
- ETags/compression/HTTP caching headers, EggServe upgrade, protocol
  changes, stale-policy changes, generic cache abstractions,
  performance CI;
- rewriting Plan 139's historical closure evidence;
- new dependencies, Tokio features, or runtime policy;
- reopening Plan 091, Plan 137, Plan 142, or Plan 145.

## Post-closure follow-ups

None. Plan 144 owns only the ready-health single-flight memoization
correction. The remaining Plan 141 corrective follow-up
(Plan 145) is independent and unblocked by Plan 144's closure.
