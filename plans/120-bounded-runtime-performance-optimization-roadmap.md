# Plan 120: bounded runtime performance optimization roadmap

Status: ready for implementation.

Depends on: the current Rust 1.89 / eggfetch-core 0.1.7 baseline after completed Plans 117-119. The source-review baseline for this roadmap is main at b8964549bed524692423a7ea10a78ed1d24006ba.

Coordinates: Plans 121-123.

## Objective

Reduce steady-state CPU work, allocation volume, copying, and repeated serialization in Gregg's daemon/client hot paths without changing any public protocol, CLI surface, supported platform, polling semantics, metric capability, presentation semantics, or failure behavior.

The work is intentionally bounded around costs visible in the current architecture:

1. repeated deep copies while one daemon sample is converted, published, polled, normalized, and applied;
2. O(N squared) client reducer lookup in the ordinary one-result-per-system batch path;
3. avoidable stable-ID allocation in disk/network counter baselines;
4. repeated fleet-wide formatting and cache lookup work in the TUI;
5. unconditional redraw work after event-loop wakeups that do not change render-visible state;
6. repeated JSON serialization and ready-health cloning for immutable daemon snapshots.

The campaign must optimize the existing ownership boundaries rather than replace them.

## Baseline findings

### Release and dependency baseline

The workspace already has an intentionally size-oriented release profile:

~~~text
lto = fat
codegen-units = 1
strip = symbols
panic = abort
~~~

Plan 119 also moved the client to eggfetch-core 0.1.7 with the lean standard-http1 + tls-rustls feature profile and measured a stripped gregg release binary of 3,740,592 bytes, 12.3% below the Plan-118 0.1.5 record.

Accordingly, this roadmap does not reopen the release profile or HTTP dependency decision.

### Daemon sample/publication path

The current sampler creates both v1 and v2 products from one CollectedMetrics value. On v1-capable platforms it clones CollectedMetrics before producing v1, while the v2 conversion clones optional drive, disk-I/O, and network collections before consuming the remaining structure.

The sampler then stores Arc snapshots, but sync_sampler_state dereferences and clones those snapshots before ServerState wraps them in new Arcs. This defeats part of the sharing already established by the sampler and is especially expensive for v2 payloads containing bounded vectors of drives, disk devices, and interfaces.

### Client state reduction

AppState::apply_batch iterates every PollResult and performs a linear search through systems by stable ID. A normal scheduler generation preserves endpoint order, so a fleet of N systems performs up to O(N squared) string comparisons even though the matching index is ordinarily already known.

The event loop owns each received PollBatch, but the public borrowed reducer path forces normalization to clone strings and collections immediately before the batch is dropped.

### Counter baselines

CounterBaselines::observe currently inserts id.to_owned() on every observation. Existing interfaces and devices therefore allocate a new String key every sample even though their map keys are stable for the daemon lifetime.

### TUI rendering

Normal view has a per-system metric-row memo, but the memo is a Vec searched linearly for every online system and stores a full NormalizedSnapshot clone only to decide whether four/five base metric rows need rebuilding.

Fleet layout repeatedly allocates formatted suffix Strings to measure width and later formats them again for rendering.

Condensed mode preformats all online systems into seven Strings per system to compute widths, discards those values, then preformats visible rows again.

The event loop draws after every select iteration, even when a terminal event maps to no action.

### Daemon HTTP serving

ServerState stores typed snapshots behind Arc, but also stores ready HealthResponse values containing cloned snapshots. A normal fresh /v1/status or /v2/status read clones the health envelope even though the handler discards it on the 200 path.

Each status request serializes the same immutable snapshot again with serde_json::to_vec until a new sample arrives.

## Scope and ownership

### Plan 121: allocation, ownership, and reducer optimization

Owns the mechanical low-risk work:

- one-pass daemon sample conversion with moved v2-only collections;
- Arc-preserving sampler-to-server publication through additive internal methods;
- an owned production batch-application path while preserving the public borrowed reducer;
- ordinary O(N) batch matching with a safe fallback for reordered/stale results;
- stable counter-baseline keys without per-sample String allocation.

Plan 121 must land first because Plan 123 can then build its publication cache on the settled shared-snapshot ownership.

### Plan 122: TUI render-path optimization

Owns:

- event-loop dirty redraw gating;
- O(1)-average per-system render memo lookup;
- compact render keys instead of full snapshot copies;
- removal of visible-entry linear searches through fleet row caches;
- reuse of condensed preformatted values within one render;
- cached or borrowed normal-view suffix data where it removes repeat formatting without changing width behavior.

It must preserve byte-for-byte/cell-for-cell presentation semantics at the existing tested widths and mixed-fleet cases.

Plan 122 can be implemented after Plan 121 or in parallel once the AppState reducer interface is settled.

### Plan 123: daemon publication and HTTP response optimization

Owns:

- separating fresh-status fast-path data from health-envelope construction;
- preserving typed public ServerState getters while avoiding unused ready-health deep clones;
- publication-time compact JSON caching for immutable v1/v2 status responses;
- cheap shared response-body clones for repeated status GETs;
- exact stale/failure/health semantics and coherent publication under concurrent readers.

Plan 123 depends on Plan 121's Arc-preserving publication path.

## Explicit non-goals

This roadmap must not:

- redesign PollScheduler, change result ordering, remove per-endpoint panic isolation, or replace bounded scheduler channels;
- change request concurrency, refresh cadence, timeout semantics, v2-first/v1-fallback negotiation, or offline retry behavior;
- change daemon sample cadence or cache CPU-frequency/link metadata in a way that reduces metric freshness;
- remove spawn_blocking from native collection on the current-thread daemon runtime;
- add compression, ETags, HTTP/2, WebSockets, streaming telemetry, persistent history, exporters, or push updates;
- alter v1/v2 JSON shape, schema validation, health response shape, status codes, content type, or stale-snapshot policy;
- add a custom allocator, unsafe code, SIMD, no_std work, or a new HTTP/TUI/runtime framework;
- add Criterion, iai, a benchmark CI job, performance thresholds in ordinary CI, or generated evidence artifacts;
- add a dependency solely for performance when std or an already-resolved dependency surface is sufficient;
- change public function/type signatures merely to make internal ownership easier.

## Measurement model

Performance work must be demonstrated without creating a permanent benchmark program.

For each implementation plan:

1. record the implementation-start SHA and rustc/target used for local measurements;
2. use deterministic tests to prove the structural optimization where possible, such as Arc pointer identity, one-result-per-system mapping, no-repeat serialization counters, or cache reuse;
3. use release-mode ad hoc loops or the existing sustained workload driver for before/after timing where useful, but do not make timing assertions in CI;
4. record final stripped release binary sizes for the affected binary/binaries;
5. investigate any unexplained binary growth above roughly 1%; runtime improvement may still justify it, but the closure record must state why;
6. run the existing default local check and the ordinary native CI workflow once after the campaign is complete.

No result should be retained solely because a noisy wall-clock measurement appears faster. The source-level work reduction must be identifiable and compatibility tests must pass.

## Required implementation order

~~~text
Plan 120 roadmap
    |
    +--> Plan 121 allocation / ownership / reducer
              |
              +--> Plan 123 daemon publication / HTTP cache
    |
    +--> Plan 122 TUI render path
~~~

Plans 122 and 123 are independent of each other after Plan 121.

## Global compatibility invariants

All work must preserve:

1. current public gregg-protocol v1/v2 Rust types and serialized shapes;
2. current public gregg and greggd library APIs unless an additive internal or public helper is introduced;
3. new-client/old-daemon v1 fallback and mixed-version fleet behavior;
4. Windows v2-only daemon behavior and v1 NotServing response semantics;
5. existing stale snapshot thresholds and failed-but-still-fresh status behavior;
6. endpoint order, cancellation, concurrency, generation, and panic behavior in PollScheduler;
7. Linux/macOS/Windows native metric capability truth;
8. normal/condensed TUI geometry, Unicode cell-width handling, selection, expansion, and viewport behavior;
9. EggPool behavior and optional isolation;
10. Rust 1.89 MSRV and the existing release/distribution model.

## Global verification

At minimum after all three implementation plans are complete:

~~~text
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

Run ./scripts/check-local.sh --release only if the implementation changes release-facing manifests/profile behavior or if the existing repository convention requires it for final closure. The optimization campaign itself must not modify the release profile.

One ordinary hosted CI run at the final implementation SHA is sufficient. Do not add jobs or rerun merely to collect performance numbers.

## Global acceptance criteria

The roadmap is complete only when:

- [ ] Plan 121 removes the identified avoidable deep-copy/key-allocation/reducer costs while preserving public borrowed APIs and semantics.
- [ ] Plan 122 reduces steady-state render preparation/redraw work while existing renderer behavior tests remain unchanged.
- [ ] Plan 123 serves fresh immutable status snapshots without per-request snapshot serialization and without unused ready-health cloning.
- [ ] Reordered or stale client batches still resolve safely through a fallback path.
- [ ] Public typed ServerState snapshot/health getters still return the same logical data.
- [ ] Stale/failure transitions cannot serve a cached 200 body when current policy requires 503.
- [ ] No supported metric, platform, CLI operation, endpoint, protocol field, or TUI capability is removed.
- [ ] No scheduler architecture, collection cadence, dependency framework, or release-profile rewrite is introduced.
- [ ] Deterministic tests and the default local check pass.
- [ ] Final release binary sizes and lightweight before/after measurements are recorded in the implementation plans.
- [ ] One ordinary existing CI run is green at the final source state.

## Closure record

Append the final implementation SHAs for Plans 121-123, local measurement environment, key structural/performance deltas, final release sizes, default local-check result, and the one ordinary CI run. Do not create a separate closure-only plan unless implementation uncovers a concrete product defect outside these scopes.
