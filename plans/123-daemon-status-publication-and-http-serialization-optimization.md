# Plan 123: daemon status publication and HTTP serialization optimization

Status: complete; implementation `45582ce`.

Depends on: Plans 120 and 121. Plan 121 must first establish the shared Arc publication path so this phase does not optimize around avoidable full-payload copies.

## Objective

Make repeated greggd status reads cheap when the underlying immutable sample has not changed, while preserving the exact v1/v2 JSON API, health semantics, stale-snapshot policy, status codes, public typed ServerState accessors, and coherent reader/writer behavior.

The target steady-state path is:

~~~text
new sample
  -> validate/convert once
  -> publish typed Arc snapshot(s)
  -> serialize compact status JSON once per published version
  -> store shared immutable response bytes

repeated GET /v1/status or /v2/status
  -> read coherent publication state
  -> evaluate staleness
  -> clone shared bytes
  -> return 200
~~~

A failed/stale request must still construct/serve the current health response and must never return a cached 200 merely because old status bytes exist.

## Current costs

PublishedState currently stores:

- Arc<StatusSnapshot>;
- Arc<StatusPayloadV2>;
- HealthResponse containing another owned v1 snapshot on Ready;
- HealthResponseV2 containing another owned v2 base snapshot on Ready;
- failure/staleness metadata.

v1_status_data/v2_status_data clone the stored health response on every status request. On the normal fresh path the handler ignores that clone and serializes the Arc snapshot with serde_json::to_vec.

Thus a stable sample can be serialized once per polling client per refresh even though its JSON bytes are immutable.

## Design boundary

### Keep typed snapshots authoritative

Do not replace typed snapshots with JSON-only state.

The following existing behavior must remain available:

- ServerState::snapshot();
- ServerState::snapshot_v2();
- ServerState::health();
- ServerState::health_v2();
- consecutive failure/stale policy logic;
- tests and downstream library users that inspect typed values.

Cached bytes are an acceleration structure derived from the typed publication.

### Do not cache a status decision

Only cache serialized successful snapshot bodies.

Whether those bytes may be served with 200 remains a live decision based on:

- snapshot presence;
- current consecutive failure count;
- current max_consecutive_failures policy;
- current wall-clock snapshot age;
- backward-clock/future-snapshot handling;
- current v1/v2 serving availability.

A previously fresh cached body must be withheld immediately when current policy says the snapshot is stale.

### Avoid ready-health duplication in publication state

Refactor PublishedState so it does not need to own a second complete Ready snapshot solely through HealthResponse/HealthResponseV2.

Store the minimal health/readiness metadata required to reconstruct the existing public envelopes, for example:

- readiness state per version where v1/v2 differ;
- optional HealthCategory;
- bounded message;
- consecutive failures;
- typed snapshot Arcs.

Exact internal representation is flexible.

ServerState::health() and health_v2() must continue returning logically identical HealthResponse values, including a cloned snapshot when Ready because the public protocol type owns one.

The optimization is that ordinary status requests do not construct or clone that envelope.

## Workstream A: publication-time status serialization

For each newly published typed snapshot, serialize the successful status body once before or immediately adjacent to publication.

Store shared immutable bytes in PublishedState alongside the matching typed Arc.

Preferred body type is the Bytes type already available through the Axum body surface if it avoids adding a new direct dependency. Arc<[u8]> or another already-available cheap-clone representation is acceptable if it integrates cleanly with Axum without copying the body again.

Do not add a compression/cache dependency.

### Serialization failure compatibility

The existing public update_snapshot methods do not return serialization errors, so do not change those signatures.

If cache preparation unexpectedly fails:

- keep the typed snapshot publication semantics intact;
- record no cached bytes for that version;
- let the request path fall back to the existing on-demand serde_json::to_vec behavior;
- preserve the current 500 JSON serialization-failure response if that fallback also fails.

Do not unwrap or panic on serialization.

Protocol validation should continue to make this path practically unreachable for valid snapshots.

## Workstream B: coherent shared publication

A typed snapshot and its cached bytes must become visible atomically under the existing PublishedState lock.

Prepare expensive serialization outside the write lock where possible, then acquire one write guard and replace the matching typed/cached state coherently.

Dual v1/v2 publication must retain:

- one logical publication transition;
- observed_at_unix_ms = max(v1, v2);
- failure-count reset;
- ready state for both versions.

Windows v2-only publication must retain:

- no v1 snapshot/body;
- v1 HealthCategory::NotServing with the existing message;
- ready v2 state/body.

The existing fallback v1-only publication path remains supported.

set_warming must clear typed/cache state exactly consistently with current snapshot clearing.

set_failed must preserve last typed/cache snapshot so the current stale policy can continue serving still-fresh cached data with 200 until its failure-count/age threshold is crossed.

## Workstream C: status fast path

Replace v1_status_data/v2_status_data output shapes or add internal helpers so a fresh status request does not clone a HealthResponse.

A fresh request needs only:

- cached response bytes if available, otherwise typed Arc for fallback serialization;
- stale boolean / current serving decision.

Only stale/unavailable paths should construct the non-ready health envelope needed for 503.

A useful internal shape may be an enum similar to:

~~~text
FreshCached(Bytes)
FreshTyped(Arc<...>)     // fallback only
Unavailable(Health...)
Stale(Health...)
~~~

Do not expose a new public API solely for this.

Keep:

- content-type application/json;
- GET / aliasing /v1/status;
- status codes;
- compact JSON rather than pretty output;
- fallback 404 behavior outside registered routes.

## Workstream D: health endpoints and public health getters

/healthz and /v2/healthz must remain exactly compatible.

On Ready they currently serialize a HealthResponse containing the snapshot. It is acceptable to construct that envelope on demand because health requests are semantically asking for the envelope.

Optionally cache ready health bytes only if implementation demonstrates this is simpler than reconstructing the envelope; do not broaden scope to cache every possible failed/stale health variant.

ServerState::health() and health_v2() remain typed and source-compatible.

When stale logic changes a stored Ready state into a collector-failure 503 response, the message/category must remain exactly the current semantics: cached snapshot is stale.

## Workstream E: deterministic serialization-count evidence

Add small cfg(test)-only instrumentation around the status snapshot serialization helper.

Required proof:

1. publishing one dual-version sample serializes each available status body once for the cache;
2. ten repeated fresh /v1/status requests do not increment v1 status serialization count;
3. ten repeated fresh /v2/status requests do not increment v2 count;
4. publishing a new sample increments the matching count exactly once again;
5. v2-only Windows-style publication never creates v1 status bytes;
6. intentionally unavailable cache in a test exercises on-demand fallback without changing response semantics.

Do not use a global allocator or timing assertion.

If cfg(test) counters would materially complicate parallel tests, expose a private injected serializer/test hook instead. Keep production code free of synchronization added solely for measurements.

## Workstream F: concurrency/coherency tests

Add a bounded test with concurrent readers and repeated publication updates.

Every 200 response must deserialize into one complete valid snapshot corresponding to one published generation. No body may combine fields from two publications.

Also cover:

- failure below stale threshold continues returning the preserved snapshot/body;
- crossing max_consecutive_failures returns 503 even though cached bytes remain;
- age-based staleness returns 503;
- future observed_at after a backward clock jump remains stale according to current policy;
- recovery publishes new bytes and returns 200;
- warming clears snapshot/cache and returns 503;
- v1/v2 dual publication remains coherent;
- public snapshot()/health() values agree with HTTP behavior.

## Locking and runtime constraints

Keep the existing Tokio RwLock unless measurements show it is itself a bottleneck after serialization removal. Do not introduce ArcSwap, lock-free publication, watch channels, or another synchronization dependency in this phase.

Do not perform JSON serialization while holding a write lock if it can safely be prepared before acquisition.

Do not move HTTP serving or sampling to a multi-thread runtime as part of this work.

## Verification

Focused:

~~~text
cargo test -p greggd server
cargo test -p greggd sampler
cargo test -p greggd run
~~~

Then:

~~~text
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

For lightweight performance evidence, use a local release-mode loopback run with one fixed published sample and repeated /v1/status and /v2/status GETs before/after. Record requests/time or CPU time only as descriptive evidence; do not add a CI threshold or benchmark framework.

Record final stripped greggd size. Investigate material growth and keep the implementation compact.

## Acceptance criteria

- [x] Fresh /v1/status does not deep-clone a Ready HealthResponse merely to discard it.
- [x] Fresh /v2/status does not deep-clone a Ready HealthResponseV2 merely to discard it.
- [x] Each successfully published status snapshot is serialized once for cached serving, not once per request.
- [x] Repeated fresh status requests clone/share immutable bytes rather than rerunning serde serialization.
- [x] Typed Arc snapshots remain authoritative and existing public ServerState snapshot/health getters retain signatures and logical behavior.
- [x] Windows remains v2-only with unchanged v1 NotServing semantics.
- [x] Failure-below-threshold still serves the preserved fresh snapshot; stale threshold/age immediately force 503 despite cached bytes.
- [x] Warming/recovery/failure transitions keep typed and serialized state coherent.
- [x] Serialization failure falls back safely without panic or public signature change.
- [x] Concurrent publication/read tests cannot observe torn status bodies.
- [x] No protocol shape, content type, status code, route, runtime, synchronization dependency, or new HTTP feature is introduced.
- [x] Focused tests, workspace tests, strict clippy, and default local check pass.
- [x] Closure records deterministic serialization-count evidence, a descriptive loopback comparison without retained timing values, and final greggd release size.

## Closure record

Implementation `45582ce` (Rust 1.98.1, `x86_64-unknown-linux-gnu`, start SHA
`1c82884`) adds publication-time v1/v2 `Bytes` caches, on-demand typed health
envelopes, fallback serialization, and live stale/failure decisions. The
server suite passed 60 tests, including one-serialization-per-publication,
repeated fresh-request no-increment, v2-only, fallback-cache, stale/failure,
warming/recovery, and concurrent coherency evidence. Sampler and full
workspace tests also passed; strict clippy and `./scripts/check-local.sh`
passed.

The lightweight loopback evidence is deterministic in-process HTTP coverage on
the fixed published sample: repeated v1/v2 requests reuse cached immutable
bytes, while an intentionally cleared cache serializes once on demand and
returns identical JSON. The release loopback smoke is included in the local
release preflight. Final stripped `greggd` size is 2,432,408 bytes. Ordinary
CI run `35538999184` is green; its Windows job passed on rerun after one
hosted-runner readiness timeout.

Plan 124 later restored the exact stored collector-failure message for stale
retained snapshots while preserving this publication-time cache and its
serialization-count behavior.
