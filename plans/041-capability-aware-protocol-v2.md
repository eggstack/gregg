# Phase 41: capability-aware protocol v2 for heterogeneous platforms

## Objective

Add a version-2 Gregg status protocol that can truthfully represent Linux, macOS, and Windows metric differences while preserving compatibility with existing version-1 daemons and clients.

The current version-1 status shape requires Unix-style load averages and swap values and only has a capability flag for CPU I/O wait. Windows cannot supply those fields with equivalent semantics. This phase must solve that protocol mismatch explicitly rather than fabricating load, labeling commit charge as swap, or emitting measured-looking zero values for unsupported metrics.

## Dependency and execution position

Depends on Phase 40 establishing a native Windows-capable client build and persistence/runtime baseline.

Must complete before:

- Phase 42 emits Windows daemon snapshots;
- Phase 43 installs a Windows service that serves those snapshots;
- Phase 44 declares mixed-platform closure.

Linux/macOS daemon v2 support may be implemented in this phase. Windows collector implementation must consume the finalized v2 types rather than invent a parallel Windows response.

## Governing invariants

1. Version 1 remains wire-compatible and semantically unchanged.
2. Version 2 represents unsupported metrics as absent plus explicit capability state.
3. Windows commit accounting is not named or serialized as swap.
4. Windows does not report synthetic Unix load averages.
5. A new client prefers v2 but can monitor existing v1 Linux/macOS daemons.
6. Existing v1 clients can continue monitoring upgraded Linux/macOS daemons through the v1 endpoint.
7. A Windows daemon is allowed to expose only v2 status if it cannot produce a truthful v1 snapshot.
8. Protocol validation rejects capability/value contradictions.
9. Rendering uses capability semantics, not platform-name string matching.
10. Protocol evolution remains narrow and does not become generalized telemetry negotiation.

## Scope

### In scope

- v2 protocol types and constants in `gregg-protocol`;
- v2 validation rules and fixtures;
- capability fields for load, swap, I/O wait, and commit;
- v2 status and health/readiness response types;
- daemon `/v2/status` support on Linux/macOS;
- client v2-first/v1-fallback polling;
- state normalization so the TUI can render v1 and v2 systems together;
- UI labels for unavailable metrics and Windows commit usage;
- compatibility tests;
- documentation of endpoint/version behavior.

### Out of scope

- Windows metric API implementation;
- Windows service control;
- arbitrary user-defined metrics;
- plugin/schema negotiation;
- protobuf, gRPC, WebSocket, streaming, or compression changes;
- changing v1 fields to nullable;
- removing `/v1/status` from Linux/macOS;
- automated migration of third-party consumers;
- release automation.

## Workstream A: freeze the v2 metric model

Introduce distinct v2 types rather than mutating the existing v1 `StatusSnapshot` in place.

Recommended conceptual shape:

```rust
pub struct StatusSnapshotV2 {
    pub schema_version: u16,
    pub observed_at_unix_ms: u64,
    pub sample_interval_ms: u64,
    pub capabilities: MetricCapabilitiesV2,
    pub system: SystemIdentity,
    pub cpu: CpuMetricsV2,
    pub load: Option<LoadAverage>,
    pub memory: MemoryMetrics,
    pub swap: Option<SwapMetrics>,
    pub commit: Option<CommitMetrics>,
}

pub struct MetricCapabilitiesV2 {
    pub cpu_iowait: bool,
    pub load_average: bool,
    pub swap: bool,
    pub memory_commit: bool,
}

pub struct CpuMetricsV2 {
    pub logical_cores: u32,
    pub usage_pct: f32,
    pub iowait_pct: Option<f32>,
}

pub struct CommitMetrics {
    pub used_bytes: u64,
    pub limit_bytes: u64,
    pub usage_pct: f32,
}
```

Naming may differ, but semantic separation is mandatory.

### Required platform mappings

Linux:

```text
cpu_iowait = true
load_average = true
swap = true when the collector exposes meaningful swap accounting
memory_commit = false
```

macOS:

```text
cpu_iowait = false
load_average = true
swap = true when meaningful native swap accounting is available
memory_commit = false
```

Windows:

```text
cpu_iowait = false
load_average = false
swap = false
memory_commit = true
```

If Linux/macOS swap is unavailable on a particular host, v2 may report `swap = false` and `swap: null` rather than constructing a zero-total value that ambiguously means either no configured swap or unsupported collection. Decide and document the distinction consistently.

### Workstream A acceptance criteria

- [ ] V2 types are separate from v1 types.
- [ ] Commit and swap are distinct types and JSON fields.
- [ ] Load is optional and capability-declared.
- [ ] I/O-wait optionality remains explicit.
- [ ] Platform mapping is documented.
- [ ] No Windows-specific platform check is needed to interpret the payload.

## Workstream B: define strict v2 validation

Validation must reject inconsistent payloads.

### General invariants

- `schema_version == SCHEMA_VERSION_V2`;
- timestamps and sample interval are nonzero and bounded by existing policy;
- logical cores are greater than zero;
- percentages are finite and within `0.0..=100.0`;
- used values do not exceed totals/limits;
- zero denominator implies zero usage percentage;
- identity strings satisfy existing length/content policy.

### Capability/value invariants

For each optional metric:

```text
capability true  => value Some and valid in Ready snapshot
capability false => value None
```

Specifically:

- `cpu_iowait == false` requires `iowait_pct == None`;
- `cpu_iowait == true` requires `iowait_pct == Some`;
- `load_average == false` requires `load == None`;
- `load_average == true` requires `load == Some`;
- `swap == false` requires `swap == None`;
- `swap == true` requires `swap == Some`;
- `memory_commit == false` requires `commit == None`;
- `memory_commit == true` requires `commit == Some`.

Do not permit both swap and commit to be required mutually exclusively unless there is a principled reason. A future platform could expose both. Windows's mapping simply reports commit only.

### Required negative fixtures

- capability false with present value;
- capability true with missing value;
- NaN/infinite percentage through constructed Rust values;
- percentage over 100;
- used greater than total/limit;
- nonzero percentage with zero total/limit;
- schema version mismatch;
- zero logical cores;
- empty required identity field;
- Windows-shaped fixture containing fabricated load;
- Windows-shaped fixture labeling commit data as swap;
- macOS-shaped fixture with I/O wait enabled;
- v1 fixture accidentally parsed as v2 without conversion.

### Workstream B acceptance criteria

- [ ] Capability/value contradictions fail validation.
- [ ] Invalid numeric states fail validation.
- [ ] Positive fixtures exist for Linux, macOS, and Windows.
- [ ] Negative fixtures cover every optional metric.
- [ ] Validation does not infer semantics from `os_name` alone.

## Workstream C: define v2 health/readiness behavior

The current `HealthResponse` is v1-specific and embeds a v1 snapshot. Add a v2 response type or a clearly versioned generic envelope.

Preferred simple approach:

```text
GET /v2/status
GET /v2/healthz
```

with `HealthResponseV2` embedding `StatusSnapshotV2` only when ready.

Alternative acceptable approach:

- retain `/healthz` as a non-snapshot readiness endpoint;
- add `/v2/status` for metrics;
- remove embedded snapshots from new health behavior.

Choose the simpler design that preserves compatibility and avoids ambiguous schema fields. Do not make one JSON structure dynamically contain either v1 or v2 snapshots without a tagged enum and strict tests.

### Required readiness semantics

- warming returns a non-ready response and no ready snapshot;
- collector failure returns a coarse category and no private internal error chain;
- ready returns a valid v2 snapshot;
- stale/freshness behavior remains aligned with existing sampler policy;
- endpoint status codes remain documented and stable.

### Workstream C acceptance criteria

- [ ] V2 readiness response cannot embed a v1 snapshot accidentally.
- [ ] Warming and failure states are unambiguous.
- [ ] Internal paths/API details are not leaked.
- [ ] Existing v1 health behavior remains available for v1 endpoints where currently supported.

## Workstream D: serve v2 from Linux and macOS daemons

Upgrade the daemon's internal collected metric representation so it can produce both:

- the unchanged v1 snapshot for Linux/macOS compatibility;
- a v2 snapshot with explicit capability fields.

Avoid sampling twice. One native sample should be normalized once and converted into the required wire representations.

### Conversion requirements

Linux v1 conversion:

- preserve existing values and semantics exactly.

Linux v2 conversion:

- load present;
- I/O wait present;
- swap capability/value consistent;
- commit absent.

macOS v1 conversion:

- preserve existing load/memory/swap behavior;
- I/O wait remains absent.

macOS v2 conversion:

- load present;
- I/O wait absent;
- swap capability/value consistent;
- commit absent.

Windows conversion is implemented in Phase 42 and should target v2 only unless a future truthful v1 mapping is defined.

### Endpoint behavior

- `/v1/status` remains unchanged on Linux/macOS;
- `/v2/status` returns v2 on Linux/macOS;
- root endpoint documentation advertises supported versions or remains a simple stable index;
- unknown version returns ordinary `404`;
- no content negotiation header system is needed.

### Workstream D acceptance criteria

- [ ] Linux/macOS use one native sample for v1 and v2 representations.
- [ ] Existing v1 endpoint tests remain unchanged/passing.
- [ ] New v2 endpoints return valid platform-specific snapshots.
- [ ] No duplicated collector cadence or request-triggered sampling is introduced.
- [ ] Unknown versions fail simply.

## Workstream E: implement client v2-first/v1-fallback polling

The client should monitor mixed fleets without per-endpoint manual protocol configuration.

### Required negotiation sequence

For each endpoint:

1. request `/v2/status`;
2. if v2 succeeds, validate and normalize it;
3. if the daemon clearly indicates v2 is unsupported, fall back to `/v1/status`;
4. if v2 exists but returns malformed/invalid data, report a protocol error and do not silently fall back to hide the defect;
5. if v2 returns warming/failure semantics, represent that state rather than falling back to v1;
6. cache known protocol support per endpoint for a bounded period or process lifetime if this materially avoids duplicate requests, while allowing recovery after daemon upgrades/restarts.

Define the exact fallback condition narrowly, preferably `404 Not Found` or a documented unsupported-version response. Do not fall back on arbitrary `500`, timeout, TLS, parse, or validation errors.

### Internal normalization

Introduce a client-internal normalized snapshot capable of representing optional load/swap/commit regardless of wire version.

V1 normalization:

- load present;
- swap represented according to v1 semantics;
- commit absent;
- I/O wait based on v1 capability.

V2 normalization:

- copy explicit optional fields/capabilities.

State reduction and UI code should consume normalized internal data rather than branch throughout on wire version.

### Required tests

- v2 success, no v1 request;
- v2 `404`, v1 success;
- v2 malformed JSON, no fallback;
- v2 validation failure, no fallback;
- v2 timeout, no fallback unless policy explicitly says otherwise;
- v2 warming/failure, no v1 fallback;
- endpoint upgrades from v1-only to v2 during process lifetime;
- mixed v1 Linux, v2 Linux, v2 macOS, and v2 Windows fixtures in one polling batch;
- ordering and offline-state semantics unchanged.

### Workstream E acceptance criteria

- [ ] Client prefers v2 automatically.
- [ ] Fallback occurs only for explicit unsupported-version behavior.
- [ ] Invalid v2 responses are surfaced rather than hidden.
- [ ] One normalized internal model serves state/UI code.
- [ ] Mixed v1/v2 fleet tests pass.

## Workstream F: update TUI rendering for optional metrics

The current four-row model should remain compact while adapting the fourth row label.

Recommended behavior:

Linux/macOS with swap:

```text
CPU  [...]
MEM  [...]
SWAP [...]
```

Windows with commit:

```text
CPU    [...]
MEM    [...]
COMMIT [...]
```

Platform with neither swap nor commit:

```text
CPU [...]
MEM [...]
SWAP --
```

or another compact unavailable representation that preserves four rows.

Header behavior:

- load unsupported renders `L --` or is omitted according to width priority;
- I/O wait unsupported renders `IO --`;
- logical core count remains shown;
- no renderer checks `os_name == "windows"` to decide metric labels when capabilities already express the state.

If both swap and commit are present in a future snapshot, define deterministic priority or a compact combined row. This does not need a generalized dynamic-row system.

### Required rendering tests

- v1 Linux;
- v2 Linux;
- v2 macOS;
- v2 Windows;
- no swap/commit;
- narrow widths;
- zero total values;
- offline row unchanged;
- mixed fleet scrolling with equal row accounting.

### Workstream F acceptance criteria

- [ ] Windows renders `COMMIT`, not `SWAP`.
- [ ] Unsupported load and I/O wait are visibly absent/unavailable.
- [ ] Four-row reachable layout remains stable.
- [ ] Rendering is capability-driven.
- [ ] Existing v1 UI tests remain valid or are intentionally normalized.

## Workstream G: compatibility and public API discipline

`gregg-protocol` is a published library crate. Keep v1 public types and exports available.

Recommended exports:

```rust
pub use v1::{StatusSnapshot, HealthResponse, ...};
pub use v2::{StatusSnapshotV2, HealthResponseV2, MetricCapabilitiesV2, CommitMetrics, ...};
```

or explicit module paths if that is clearer.

Do not alias v2 as the unversioned `StatusSnapshot` in the same release if it breaks downstream source compatibility.

Add documentation for:

- v1 stable behavior;
- v2 additions;
- endpoint/version support;
- conversion/normalization expectations;
- unsupported metric semantics.

### Compatibility test matrix

- old v1 fixture -> existing v1 type;
- old v1 fixture -> new client normalization;
- v2 Linux fixture -> v2 type;
- v2 macOS fixture -> v2 type;
- v2 Windows fixture -> v2 type;
- v2 fixture rejected by v1 parser where fields/shape conflict;
- round-trip serialization for each v2 platform fixture;
- unknown fields policy explicitly tested.

### Workstream G acceptance criteria

- [ ] V1 public API remains available.
- [ ] Existing v1 JSON fixtures remain byte/semantic compatible where promised.
- [ ] V2 public API is clearly versioned.
- [ ] Mixed-version behavior is documented.
- [ ] Downstream source compatibility impact is reviewed before release.

## Workstream H: protocol documentation and security review

Update:

- `architecture/protocol.md`;
- root README API section;
- `gregg-protocol` README;
- endpoint examples;
- security/limits documentation if response size changes.

Document that capability flags are authoritative and clients must not assume platform metrics.

Review response-size and numeric-validation limits. V2 adds only a small number of fields; do not add dynamic maps or unbounded metadata.

No authentication or TLS is introduced. The existing private-network threat model remains.

### Workstream H acceptance criteria

- [ ] Protocol docs include v1 and v2 examples.
- [ ] Optional metric semantics are explicit.
- [ ] Windows commit is distinguished from swap.
- [ ] Response remains bounded and simple.
- [ ] Security documentation remains accurate.

## Required validation commands

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p gregg-protocol --all-targets --all-features
cargo test -p greggd --all-targets --all-features
cargo test -p gregg --all-targets --all-features
cargo doc --workspace --no-deps
```

Run native Linux/macOS daemon endpoint smokes and Windows client mixed-fixture tests.

## Phase acceptance criteria

Phase 41 is complete only when:

- [ ] V1 types/endpoints remain compatible on Linux/macOS.
- [ ] V2 types model optional load, swap, I/O wait, and commit explicitly.
- [ ] Commit is never serialized or rendered as swap.
- [ ] Capability/value contradictions fail validation.
- [ ] Linux/macOS daemons serve valid v2 snapshots from the existing sample path.
- [ ] The client prefers v2 and falls back to v1 only on explicit unsupported-version behavior.
- [ ] Invalid v2 data is not hidden by fallback.
- [ ] A normalized internal model supports mixed v1/v2 fleets.
- [ ] TUI rendering is capability-driven and preserves compact layout.
- [ ] Positive and negative platform fixtures are complete.
- [ ] Protocol/API documentation is updated.
- [ ] No release or evidence automation is added.

## Evidence required for completion

Only:

- passing protocol/daemon/client tests;
- native Linux/macOS v2 endpoint smoke output;
- Windows client fixture/mixed-fleet test output;
- documentation diff.

Do not create protocol qualification artifacts or hosted evidence manifests.

## Handoff notes for a smaller implementation model

1. Add v2 types and validation before changing daemon/client behavior.
2. Keep v1 modules untouched except for shared helper extraction that preserves behavior.
3. Add platform fixtures early; they make semantic mistakes obvious.
4. Implement daemon v2 conversion from the existing normalized sample, not from a second collection pass.
5. Implement client normalization before UI changes.
6. Restrict fallback to explicit v2-not-supported responses.
7. Search UI code for platform-name branching and replace metric decisions with capabilities.
8. Do not attempt Windows collector work in this phase; use fixtures for the Windows shape.
9. End with docs and a full mixed-version test run.