# Phase 068: coherent daemon state and health

Status: complete.

Depends on: Plan 066.

## Objective

Publish daemon snapshots, health, timestamps, and failure state as one coherent generation. Correct the Windows v2-only inconsistency where a v1 health route can return `200 OK` while serializing a `warming` body. Reduce synchronization code without changing the daemon's cached-snapshot, staleness, or route surface.

## Current defect

`ServerState` currently separates v1 snapshot, v2 snapshot, observation time, v1 health, v2 health, readiness, and failure count across multiple locks and atomics. A publication updates these pieces sequentially. On Windows, `update_snapshot_v2_only()` sets global readiness to true but intentionally leaves v1 health in `warming`. The v1 `/healthz` status is derived from the global readiness flag while its body comes from the stale v1 health object.

This phase makes status and body decisions from one state generation and derives readiness per protocol version.

## Required route semantics

### Linux and macOS after a successful sample

```text
/                 -> 200 v1 snapshot
/v1/status        -> 200 v1 snapshot
/healthz          -> 200 v1 ready health
/v2/status        -> 200 v2 payload
/v2/healthz       -> 200 v2 ready health
```

### Windows after a successful v2-only sample

```text
/                 -> 503 v1 failed/not-serving health
/v1/status        -> 503 v1 failed/not-serving health
/healthz          -> 503 v1 failed/not-serving health
/v2/status        -> 200 v2 payload
/v2/healthz       -> 200 v2 ready health
```

Use the existing v1 `HealthCategory::NotServing` and a short stable message such as `schema v1 status is unavailable on this platform`. Do not add a new readiness state or health category solely for Windows.

### Collector failure with a cached snapshot

Preserve existing stale-serving behavior:

- A status route may continue to serve its corresponding cached snapshot while the configured staleness policy permits it.
- Its health route remains non-ready and returns `503` with a failed health body.
- Once stale, the status route returns the matching failed health body with `503`.
- v1 and v2 make these decisions independently based on whether that version has a cached snapshot.

## Scope

### In scope

- Consolidate server publication data under one `RwLock` or an equally direct single-generation state mechanism.
- Derive route readiness from the corresponding version's state.
- Preserve failure count and age-based staleness.
- Preserve v1-only fallback support if the sampler supplies v1 without v2.
- Add focused tests for Windows v2-only health/status behavior and state coherence.
- Remove redundant locks, atomics, helper methods, and comments made obsolete by consolidation.

### Out of scope

- Protocol v3 or new health fields.
- Changing route names, response size limits, HTTP framework, or JSON encoding.
- Caching serialized JSON bytes.
- Changing sampling cadence or collector failure classification.
- A generalized state store, event bus, watch channel, actor, or lock-free structure.
- New telemetry or daemon self-monitoring.

## Expected files

```text
crates/greggd/src/server/mod.rs
crates/greggd/src/server/tests.rs
crates/greggd/src/run.rs
crates/greggd/src/sampler.rs        # only if a direct interface adjustment is required
architecture/greggd-daemon.md
architecture/protocol.md             # only for route semantics
README.md                             # only if active route documentation is inaccurate
```

## Target state shape

Prefer one plain structure similar to:

```rust
struct PublishedState {
    snapshot_v1: Option<Arc<StatusSnapshot>>,
    snapshot_v2: Option<Arc<StatusPayloadV2>>,
    health_v1: HealthResponse,
    health_v2: HealthResponseV2,
    last_observed_at_unix_ms: Option<u64>,
    consecutive_failures: u32,
}

pub struct ServerState {
    published: Arc<RwLock<PublishedState>>,
    max_consecutive_failures: u32,
    max_snapshot_age: Duration,
}
```

The exact names may differ. Do not add generics or a version-abstracted state hierarchy. Two explicit protocol fields are clearer for this small product.

## Implementation sequence for GPT-5.6 Luna

### Step 1: add failing route tests first

Add focused tests that demonstrate the defect and required behavior:

1. After `update_snapshot_v2_only`, `/healthz` returns `503`, not `200`.
2. That v1 health body has schema version 1, state `failed`, category `not_serving`, no snapshot, and the stable unavailable message.
3. `/v1/status` and `/` return the same v1 unavailable health response.
4. `/v2/status` and `/v2/healthz` remain ready and return `200`.
5. A later collector failure makes v2 health return `503` while v2 status continues serving the cached snapshot until stale.
6. Linux/macOS-style dual publication still makes both health versions ready.

Do not weaken existing tests to make the refactor pass.

### Step 2: define one coherent published state

Move all mutable response state into one lock. Keep immutable staleness policy outside the lock.

A successful dual publication must acquire one write lock and update:

- both snapshots;
- both ready health bodies;
- observation timestamp;
- failure count reset.

A successful v2-only publication must acquire one write lock and update:

- v1 snapshot to `None`;
- v1 health to failed/not-serving;
- v2 snapshot and ready health;
- observation timestamp;
- failure count reset.

A v1-only fallback publication, if retained, must symmetrically mark v2 as not serving rather than leaving unrelated warming state.

### Step 3: make warming and failure transitions coherent

`set_warming()` should clear both snapshots and set both health objects to warming in one write.

`set_failed()` should:

- increment failure count in the same write;
- preserve existing snapshots;
- set failed collector-health bodies for versions that are supported by the current collector publication mode;
- preserve not-serving health for a version that is structurally unavailable, such as v1 on Windows.

Avoid introducing a platform enum in the server. The presence/absence and established health state of each version are enough.

### Step 4: centralize stale evaluation over a read snapshot

Take one read lock per handler and make all decisions from that guard. A handler must not separately read a snapshot, timestamp, readiness flag, and health object.

A direct helper may accept:

```text
now_unix_ms
consecutive_failures
last_observed_at_unix_ms
configured stale policy
```

The helper should be pure and synchronously testable. Do not put an async lock acquisition inside the staleness predicate.

### Step 5: remove obsolete state and helpers

Delete:

- the global `ready` atomic;
- the failure-count atomic;
- individual snapshot/health/timestamp locks;
- helper methods that exist only to read those fields separately;
- comments describing cross-lock behavior that no longer exists.

Retain small test accessors only where they verify externally meaningful state. Prefer route-level tests over exposing internals.

### Step 6: preserve sampler/run integration

Keep the existing sampler callback and supervision model unless a minimal signature adjustment is required. This phase is not a supervision rewrite.

`sync_sampler_state` should remain an explicit match over readiness and snapshot availability. Simplify it only enough to call coherent publication methods. Do not create an adapter trait or generic version converter.

### Step 7: reconcile active documentation

State clearly that:

- Windows serves v2 metrics and returns a v1 not-serving response;
- status routes may serve a cached but not-yet-stale snapshot after collector failure;
- health routes reflect current readiness and return `503` during failure;
- each response is produced from one coherent state generation.

Do not rewrite historical plans.

## Focused verification

```bash
cargo test -p greggd server
cargo test -p greggd run
cargo test -p greggd sampler
./scripts/check-local.sh
```

Use existing concurrency tests to ensure the single lock remains responsive. Do not add a benchmark or lock-contention test suite; the expected request rate is small.

## Acceptance criteria

- [ ] All mutable server response state is published as one coherent generation.
- [ ] Handlers use one state read and cannot combine fields from different publications.
- [ ] Windows v2-only publication returns `503` with a v1 failed/not-serving body on `/`, `/v1/status`, and `/healthz`.
- [ ] Windows `/v2/status` and `/v2/healthz` return `200` after a valid sample.
- [ ] No health endpoint returns `200` with a non-ready body.
- [ ] No health endpoint returns `503` with a ready body.
- [ ] Cached status serving during temporary collector failure remains governed by the existing staleness policy.
- [ ] v1 and v2 cached-snapshot availability are evaluated independently.
- [ ] The global readiness atomic and fragmented response locks are removed.
- [ ] No actor, event bus, lock-free structure, protocol version, or new route is added.
- [ ] Focused tests and the default local check pass.

## Handoff format

Report the final state shape, route behavior matrix, focused test commands, and any platform behavior awaiting ordinary hosted CI. Do not create an evidence file.

## Completion

Server publication is one locked generation. Windows v2-only publication is
v1 503/not-serving and v2 200/ready after sampling; stale serving remains.
