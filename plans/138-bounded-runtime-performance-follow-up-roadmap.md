# Plan 138: bounded runtime performance follow-up roadmap

Status: complete.

Depends on: current main at `deb74658` after completed Plans 120-124 and 132-137. This campaign is independent of the remaining Plan 091 soak record.

Coordinates: Plans 139-143.

## Objective

Perform a second, regression-controlled runtime optimization pass against the post-`gregg-host` architecture without changing Gregg's public API surface, wire protocol, supported capabilities, sampling/polling semantics, failure behavior, or release model.

Plans 120-124 already removed the largest obvious ownership, reducer, TUI redraw, and status-serialization costs. This follow-up owns only residual costs that remain visible in the current source:

1. ready health responses still clone and serialize immutable snapshots on every request;
2. the client scheduler/poller still rebuilds endpoint URLs and clones endpoint-owned strings more than once per generation;
3. Linux native telemetry performs repeated topology/metadata reads around high-frequency counters, and macOS re-queries immutable page-size data;
4. the daemon sampler creates a new `spawn_blocking` job and mutex handoff for each sample;
5. the TUI/state path still has bounded no-op redraw and cross-render formatting opportunities.

The governing rule is structural work reduction first, timing second. Do not retain an optimization solely because a noisy benchmark appears faster.

## Research findings governing this campaign

### Tokio blocking work

Gregg runs `greggd` on a Tokio current-thread runtime and currently moves every native sample through `tokio::task::spawn_blocking`.

Current Tokio documentation states that started `spawn_blocking` tasks cannot be aborted and that persistent or long-lived blocking loops are better represented by a dedicated thread. That makes a persistent collector worker worth testing, but not safe to adopt blindly: Gregg's existing panic recovery, collector continuity, shutdown bounds, and blocked-native-call behavior are part of the compatibility contract.

Plan 142 therefore owns a reversible experiment rather than a mandatory redesign.

Reference: https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html

### Linux CPUFreq and network semantics

Linux exposes CPUFreq as policy objects under `/sys/devices/system/cpu/cpufreq/policyX`. Policy membership is structural while `cpuinfo_cur_freq` / `scaling_cur_freq` are current values. This permits caching only the proven structural portion while continuing to read current frequency every sample.

Reference: https://docs.kernel.org/admin-guide/pm/cpufreq.html

Linux `/proc/net/dev` already provides the interface set plus cumulative counters in one read. Link administrative/operational state and topology are dynamic; kernel documentation identifies rtnetlink `RTM_GETLINK` as the preferred richer link-information interface. A blanket TTL over `operstate`, master membership, link flags, or capacity would therefore be a freshness regression.

References:
- https://docs.kernel.org/networking/statistics.html
- https://docs.kernel.org/networking/operstates.html

Plan 141 may consolidate dynamic reads only if immediate change visibility is retained.

### Rust MSRV-safe one-time storage

`std::sync::OnceLock` is stable since Rust 1.70 and is therefore available under Gregg's Rust 1.89 MSRV. It is suitable for immutable process/platform values such as the macOS host page size and for private per-publication cache primitives where useful.

Reference: https://doc.rust-lang.org/std/sync/struct.OnceLock.html

## Phase ownership

### Plan 139: daemon health-response and dispatch optimization

Owns the lowest-risk daemon HTTP work:

- cache or memoize ready `/healthz` and `/v2/healthz` serialization per immutable publication without deep-cloning snapshots per request;
- preserve exact ready, stale, failed, Warming, and NotServing envelope semantics;
- avoid owned method/target strings on successful known routes;
- retain Plan-123 status-body caching unchanged.

### Plan 140: prepared client polling targets and endpoint ownership

Owns client-side per-generation allocation reduction:

- precompute normalized v1/v2 status targets when endpoint lists are installed/replaced;
- move a single generation-owned `Endpoint` into `PollResult` instead of cloning the full endpoint repeatedly;
- keep index/order metadata for panic recovery rather than a second full endpoint clone;
- preserve public `HttpClient::poll(&Endpoint)` behavior;
- move retained `SystemState` values during config reconciliation instead of deep-cloning them.

This is not a scheduler architecture rewrite.

### Plan 141: native telemetry acquisition work reduction

Owns `gregg-host` source-call and allocation reduction with freshness as a hard gate.

Required concrete candidates:

- Linux CPUFreq policy/member caching with immediate policy-set/core-count invalidation while current frequency remains sampled every cycle;
- macOS successful page-size caching;
- deterministic source-call accounting so improvements are demonstrated structurally.

Measured optional candidates:

- Linux network metadata consolidation, preferably through a bounded native link-info query if it removes per-interface sysfs reads without stale state;
- Linux block-counter/topology separation only if the current accounting-layer and hotplug/topology semantics remain exact;
- temporary counter-baseline retain-set allocation only if it remains material after I/O reduction.

A candidate that cannot preserve immediate semantics must be recorded as RETAIN CURRENT, not forced into the implementation.

### Plan 142: persistent native sampler worker experiment

Owns a reversible comparison of the current per-tick `spawn_blocking` model against one dedicated collector worker.

The candidate must preserve public `Sampler` behavior, panic recovery, sample ordering, readiness transitions, identity/capability refresh semantics, bounded memory, and shutdown behavior. It may close with RETAIN SPAWN_BLOCKING if the worker complicates lifecycle semantics or does not demonstrate structural/meaningful benefit.

Plan 142 follows Plan 141 so collector-source I/O does not hide or distort runtime-handoff measurements.

### Plan 143: TUI/state no-op and cross-render optimization

Owns remaining low-risk client presentation/state work:

- suppress redraw after provably rejected/stale batches and reducer actions that do not change render-visible state;
- retain condensed preformatted values across redraws when their render key is unchanged;
- avoid duplicate drive/network aggregate calculation within a normal render where practical;
- move unchanged system state during config reconciliation rather than cloning it.

Do not replace Ratatui, change geometry, or make fleet layout viewport-local.

## Measurement model

For every implementation plan:

1. record the implementation-start SHA and toolchain/target used for local measurements;
2. add deterministic structural evidence where possible: serialization counters, URL-build counters, source-call counters, draw counters, cache-hit/reuse assertions, pointer identity, or allocation-free ownership movement;
3. use release-mode ad hoc timing or the existing sustained-workload driver only as descriptive evidence;
4. do not add timing thresholds to ordinary CI;
5. measure stripped release binary sizes for `gregg` and/or `greggd` when the affected code is linked there;
6. investigate unexplained size growth above roughly 1%;
7. use one ordinary final CI run after the campaign is implemented.

No Criterion/iai framework or permanent performance workflow is required.

## Required implementation order

~~~text
Plan 138
  |
  +--> 139 daemon HTTP
  |
  +--> 140 client polling ownership
  |
  +--> 141 native acquisition
  |       |
  |       +--> 142 sampler worker experiment
  |
  +--> 143 TUI/state follow-up
~~~

Plans 139, 140, 141, and 143 may proceed independently. Plan 142 waits for Plan 141.

## Global compatibility invariants

All work must preserve:

1. public `gregg`, `greggd`, `gregg-host`, and `gregg-protocol` API surfaces unless an additive private/internal helper is sufficient;
2. exact v1/v2 JSON shapes, health/status codes, stale policies, and Plan-124 failure-message semantics;
3. Linux/macOS/Windows native telemetry meaning and FreeBSD `gregg-host` behavior;
4. current sample cadence and first-sample/reset semantics;
5. immediate visibility of supported hotplug/link/topology changes wherever current code observes them;
6. client endpoint order, generation, concurrency ceiling, cancellation, offline retry, and per-endpoint panic isolation;
7. v2-first/v1-fallback negotiation and current timeout/body/error classification;
8. normal/condensed TUI geometry, Unicode-width behavior, fleet-wide layout, selection, viewport, and expansion;
9. Rust 1.89 MSRV;
10. current dependency/release profile unless a plan explicitly measures and justifies a private std-only implementation change.

## Verification

At campaign closure:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Run `./scripts/check-local.sh --release` when an implementation materially changes linked runtime structure or when recording final binary-size evidence.

Use the existing native CI matrix, including the FreeBSD `gregg-host` qualification already established by Plans 136-137. Do not add a new workflow solely for performance.

## Acceptance criteria

- [x] Plan 139 removes repeated ready-health deep clone/serialization and known-route method/target allocation without changing HTTP semantics.
- [x] Plan 140 reduces per-generation endpoint/URL ownership work without changing scheduler behavior or public polling APIs.
- [x] Plan 141 demonstrates fewer native source calls/allocations only where metric freshness and hotplug/topology semantics remain exact.
- [x] Plan 142 records a fair worker-versus-`spawn_blocking` experiment and retains only the design that preserves lifecycle semantics and is justified by structural/measured evidence.
- [x] Plan 143 removes bounded no-op render/state work without any visible TUI change.
- [x] No supported capability, platform, protocol field, CLI operation, route, or failure category is removed.
- [x] No hidden TTL or freshness regression is introduced.
- [x] Rust 1.89 and all existing native qualification remain green.
- [x] Final closure records deterministic evidence and relevant stripped binary sizes without fabricating benchmark claims.

## Explicit non-goals

Do not include:

- protocol v3;
- new metrics;
- process/GPU/sensor telemetry;
- polling backoff or scheduler redesign;
- WebSockets, push telemetry, compression, ETags, HTTP/2, or history storage;
- release-profile tuning;
- unsafe representation tricks across `gregg-host` / `gregg-protocol`;
- arbitrary metric TTLs;
- a new async runtime or system-information framework;
- new performance CI infrastructure.

## Handoff note

Implement Plan 139 or 140 first for low-risk wins. Plan 141 should begin by adding deterministic source-call accounting before changing acquisition. Do not start Plan 142 until Plan 141 has settled the native source cost, and permit Plan 142 to close with RETAIN SPAWN_BLOCKING if the lifecycle contract is not cleanly reproducible.

## Closure record

Campaign implemented at `83df89e` with toolchain `rustc 1.98.1` on
`x86_64-unknown-linux-gnu`. Implementation order followed the plan:
139 and 140 (low-risk daemon/client wins), 141 (source-call
accounting then `CPUFreq`/page-size), 142 after 141 settled source
cost, and 143 independently. Local verification at closure:
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
--all-features -- -D warnings`, `cargo test --workspace
--all-targets --all-features`, and `./scripts/check-local.sh` green;
`./scripts/check-local.sh --release` green except the expected
clean-tree gate on the dirty tree (clean after the closure commit).
Final campaign CI run: recorded below after push (one ordinary
existing workflow run; no new performance workflow).

Per-plan outcomes:

- Plan 139: ready-health memoized per publication with borrowed
  wire-equivalent serialization; known-route dispatch borrow-only.
- Plan 140: prepared `Arc<str>` v1/v2 targets per installed list;
  owned scheduler poll path; index-based panic recovery; moved
  reconcile.
- Plan 141: `CPUFreq` structural cache + macOS page-size `OnceLock`
  memo with deterministic fixture accounting; RETAIN CURRENT network
  metadata, disk topology, and baseline scratch with recorded
  rationale; no TTL.
- Plan 142: reversible test-only worker experiment; RETAIN
  SPAWN_BLOCKING with zero production diff (lifecycle/complexity
  gate fails; native I/O dominates after Plan 141).
- Plan 143: changed-result batch/action seams driving the event-loop
  dirty gate; cross-render condensed `Rc` memo; single-aggregate
  normal misses; `TestBackend` output identical.

Measurement (descriptive, no CI timing gates): stripped release
`gregg` 3,740,592 bytes (delta 0 vs Plan-125 baseline), stripped
release `greggd` 2,694,560 bytes (campaign total; +2.5% vs the older
Plan-135 baseline on a prior toolchain, std-only with no new
dependencies; investigated per the 1% rule and retained as justified
memo/cache code). Structural evidence (serialization counters,
prepared-target reuse, source-call counts, worker handoff structure,
draw/format counters, pointer-identity reuse) is primary; release
timing was descriptive only.

Global invariants hold: no protocol/capability/platform/CLI/route/
failure-category removal; no hidden TTL/freshness regression; Rust
1.89 MSRV and existing native qualification green. Plans 138-143 are
independent of the remaining Plan 091 soak record. No future plan is
blocked: Plan 142's dependency on Plan 141 is satisfied (141
complete), and 139/140/141/143 were independent as planned.
