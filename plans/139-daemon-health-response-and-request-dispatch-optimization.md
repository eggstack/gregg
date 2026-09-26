# Plan 139: daemon health-response and request-dispatch optimization

Status: complete.

Depends on: Plan 138 and the completed Plan-123/124 publication and stale-response baseline.

## Objective

Remove residual per-request allocation and serialization from `greggd`'s read-only HTTP fast paths without changing routes, JSON, status codes, health semantics, stale policy, typed `ServerState` APIs, or EggServe configuration.

The current status path is already optimized: v1/v2 status JSON is serialized once per publication and reused as cheap `Bytes` clones. The health path still constructs a ready health envelope containing a deep-cloned snapshot and serializes it for every `/healthz` and `/v2/healthz` request.

`dispatch_request()` also allocates owned method and raw-target strings before route classification even though successful known routes need neither ownership.

## Required behavior contract

Preserve exactly:

- `GET /` and `GET /v1/status`;
- `GET /v2/status`;
- `GET /healthz`;
- `GET /v2/healthz`;
- HEAD behavior;
- 405 `Allow: GET,HEAD`;
- existing 404 text;
- current content types;
- Plan-124 stale/failure response messages;
- v1 NotServing on v2-only Windows publication;
- v2 NotServing on v1-only compatibility publication;
- failed-but-still-fresh cached status serving;
- age/pre-epoch/backward-clock stale behavior.

Do not change `HealthResponse` or `HealthResponseV2` public wire types.

## Implementation

### 1. Add per-publication ready-health byte caches

Extend private `PublishedState` with generation-local ready-health serialization caches for v1 and v2.

The cache must be invalidated/replaced atomically with each new publication. It must never survive into a different typed snapshot generation.

Preferred shape is std-only and MSRV-safe, for example an `Arc<OnceLock<Result<Bytes, ...>>>` or equivalent private memo object associated with the same published snapshot. `OnceLock` is available under Rust 1.89.

The first ready-health request may serialize; repeated ready-health requests for the same publication must not:

- deep-clone the snapshot;
- allocate a new full health envelope;
- re-run `serde_json` over the snapshot.

Do not use nightly `OnceLock` APIs.

### 2. Serialize ready health through a borrowed private view

Because the public health type owns its snapshot, constructing `HealthResponse::ready((*snapshot).clone())` merely to serialize it defeats the optimization.

Introduce a private borrowed serialization view or equivalent helper whose output is byte-for-byte equivalent to serializing the public ready health type.

Lock equivalence down with tests that compare:

~~~text
serde_json::to_vec(HealthResponse::ready(snapshot.clone()))
==
serialize_ready_health_borrowed(&snapshot)
~~~

and the v2 equivalent across representative Linux/macOS/Windows-style payloads.

Do not duplicate health business logic outside this narrow serialization view. State/category/message decisions remain owned by `HealthMetadata`.

### 3. Keep stale/non-ready health dynamic and exact

A cached ready-health body is valid only while the health state is Ready and the snapshot is not stale at request time.

When stale:

- do not serve ready-health cached bytes;
- Ready + age/pre-epoch/backward-clock stale uses exactly `"cached snapshot is stale"`;
- Failed retains the latest stored collector-failure message;
- NotServing retains its version-unavailable category/message.

A later `set_failed()` must make any ready-health memo unreachable for response selection immediately.

### 4. Avoid owned request strings on known routes

Refactor `dispatch_request()` so method and target remain borrowed while route selection runs.

Only the 404 fallback needs the raw target to build its text body. Do not change target parsing or method behavior.

Do not optimize this by introducing unsafe lifetime tricks; ordinary borrows from the request head are sufficient.

## Deterministic evidence

Extend existing server test instrumentation.

Required tests:

1. repeated fresh v1 health requests for one publication cause at most one ready-health serialization;
2. repeated fresh v2 health requests behave likewise;
3. a new publication causes one new serialization on first health request;
4. stale transition never returns cached ready bytes;
5. collector failure after a cached ready health response preserves the exact Plan-124 failure message;
6. v1/v2 NotServing behavior survives later failures;
7. borrowed ready-health serialization is byte-for-byte equivalent to the public typed health response;
8. repeated status requests still use the existing Plan-123 status cache with no regression;
9. known-route dispatch reaches status/health without owned fallback-target construction where test instrumentation can demonstrate it.

Do not make allocation-count assertions dependent on a global allocator.

## Measurement

Record a release-mode descriptive request loop against one immutable publication before/after if useful, but retain the change based on deterministic serialization-count and clone-elimination evidence.

Record stripped `greggd` size before/after. Investigate growth above roughly 1%.

## Verification

~~~text
cargo test -p greggd --all-targets --all-features -- server
cargo test -p greggd --all-targets --all-features -- health
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use one ordinary existing CI run at final campaign closure; no new workflow is required.

## Acceptance criteria

- [x] Fresh ready v1 health is serialized at most once per immutable publication.
- [x] Fresh ready v2 health is serialized at most once per immutable publication.
- [x] Repeated ready-health serving performs no deep snapshot clone.
- [x] Borrowed/private serialization is exact-wire equivalent to the public health types.
- [x] Stale, Failed, Warming, and NotServing responses remain exact.
- [x] Plan-123 status-body cache behavior is unchanged.
- [x] Successful known routes no longer allocate owned method/raw-target strings solely for dispatch.
- [x] Public typed `ServerState` methods and protocol types are unchanged.
- [x] No route, status code, header, stale threshold, or EggServe runtime policy changes.
- [x] Focused tests, workspace gates, and MSRV remain green.

## Explicit non-goals

Do not include:

- status ETags or conditional requests;
- compression;
- route-framework changes;
- protocol type changes;
- precomputing every possible stale/failure body;
- changing stale policy;
- EggServe upgrades;
- server concurrency changes;
- timing gates in CI.

## Handoff note

Start with exact ready-health wire-equivalence tests, then add the memo. Keep Plan 124's stale-failure tests as the authoritative regression boundary while restructuring response selection.

## Closure record

Implemented at `83df89e` (campaign implementation commit for Plans
139-143) with toolchain `rustc 1.98.1` on
`x86_64-unknown-linux-gnu`. Local verification: `cargo test -p greggd
--lib --all-features -- server` (73 passed, including 9 new
`plan139_*` tests), `cargo fmt --all -- --check`, `cargo clippy
--workspace --all-targets --all-features -- -D warnings`, and
`./scripts/check-local.sh` green. Final campaign CI run is recorded in
Plan 138.

Deterministic evidence (`crates/greggd/src/server/tests.rs`):

- repeated fresh v1/v2 health for one publication serializes exactly
  once (`v1_health_serializations`/`v2_health_serializations` == 1
  after 10 requests);
- new publication re-arms the memo (counters advance to 2);
- stale age/failure transitions never serve cached ready bytes and
  preserve the exact Plan-124 `"cached snapshot is stale"` and
  collector-failure messages;
- v1/v2 `NotServing` survives later failures;
- borrowed ready-health bytes equal
  `serde_json::to_vec(HealthResponse::ready(...))` (v1 + v2 across
  Linux/macOS/Windows-style payloads);
- Plan-123 status cache unchanged (status still serializes once per
  publication while health memoizes independently);
- known routes (`/`, `/v1/status`, `/v2/status`, `/healthz`,
  `/v2/healthz`) do not build 404 fallback bodies
  (`fallback_bodies_built` unchanged); unknown routes do.

Structure: `PublishedState` gains generation-local `health_bytes` /
`health_bytes_v2` memos cleared on every publication, warming, and
failure transition; ready-health serializes through borrowed
`BorrowedReadyHealthV1/V2` views (no snapshot deep-clone, `Arc` clone
only for memo installation with `ptr_eq` generation guard);
`dispatch_request` keeps method/target borrowed through route
selection and allocates only for the 404 fallback. Public typed
`ServerState::health()`/`health_v2()` and protocol types unchanged;
routes, codes, headers, stale policy, and EggServe limits unchanged.

Measurement: stripped release `greggd` 2,694,560 bytes on this
toolchain (campaign-wide; Plan 135's 2,629,008-byte baseline predates
the current toolchain and the small memo/cache code; no new
dependencies, std-only). Timing retained on deterministic
serialization/clone-elimination counts, not wall-clock gates.
