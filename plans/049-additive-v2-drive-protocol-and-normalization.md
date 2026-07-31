# Phase 49: additive v2 drive protocol and client normalization

## Objective

Define the smallest backward-compatible wire and client model needed to carry per-drive capacity data through `/v2/status` while leaving v1 unchanged.

This phase ends when:

- drive entries have a bounded, validated v2 representation;
- old v2 JSON without drive data remains accepted;
- the published Rust API compatibility impact has been deliberately resolved;
- `greggd` can carry drive data from `CollectedMetrics` into its cached v2 status representation without collecting it yet;
- `gregg` normalizes drive data and derives aggregate used, total, available, and percentage values through one tested helper.

Native OS enumeration is Phase 50. TUI behavior is Phases 51 and 52.

## Dependencies and execution position

Depends only on the completed v2 protocol baseline from Phase 41.

Must complete before:

- Phase 50 serializes native drive results;
- Phase 51 renders aggregate disk information;
- Phase 52 renders condensed disk percentages or expanded rows.

## Governing invariants

1. `StatusSnapshot` v1 and all v1 endpoints remain structurally and semantically unchanged.
2. Drive data is added only to the universal v2 status response.
3. Old v2 payloads without a drive field remain valid input.
4. A missing drive field means unavailable/legacy, not measured empty storage.
5. Drive values use numeric bytes and an owned display name; no human-formatted strings appear on the wire.
6. The daemon does not serialize aggregate totals; the client derives them from the exact list it receives.
7. Validation is structural and numeric. It does not infer filesystem semantics from OS names.
8. Lists and strings are bounded so the existing client response-size guard remains meaningful.
9. No v3 endpoint, new HTTP route, or separate drive request is introduced.
10. No TUI state or rendering work is performed in this phase beyond normalized helper tests.

## Scope

### In scope

- public drive wire type;
- optional v2 drive collection field;
- wrapper-versus-direct-field source-compatibility decision;
- v2 validation violations and field paths;
- positive and negative protocol fixtures/tests;
- test-support builders;
- daemon-internal `CollectedMetrics` carriage and v2 conversion plumbing;
- client `NormalizedSnapshot` drive representation;
- overflow-safe aggregate helper;
- focused protocol/normalization documentation.

### Out of scope

- Linux, macOS, or Windows drive enumeration;
- filtering or deduplication policy implementation;
- TUI disk rows, view modes, key bindings, or viewport changes;
- filesystem type, UUID, label, mount source, flags, inodes, I/O rate, SMART, or physical-disk data;
- a generalized metric-extension map;
- changes to HTTP body-size limits unless bounded worst-case drive data demonstrably exceeds the current cap;
- release/version changes.

## Workstream A: freeze the wire semantics

Add one bounded drive record with this semantic shape:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DriveMetrics {
    pub name: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}
```

Do not add `available_bytes` or `usage_pct`; both are derivable and duplicated values could become inconsistent.

Define v2 status semantics as:

```text
field absent or null  = drive collection unavailable or daemon predates the field
Some(empty list)      = enumeration succeeded and found no eligible mounted local filesystems
Some(nonempty list)   = successfully collected eligible filesystems
```

Use `#[serde(default, skip_serializing_if = "Option::is_none")]` on the drive collection.

### Boundedness decision

Define small protocol constants or validation limits:

```text
maximum drive entries: 128
maximum drive-name bytes: 1024
```

The exact constants may be smaller if repository conventions justify it, but they must comfortably cover ordinary local machines while preventing unbounded status growth. Names are UTF-8 strings after native conversion.

Do not silently truncate names or lists in the protocol layer. Reject invalid constructed values; platform collectors should filter/normalize before publication.

### Workstream A acceptance criteria

- [ ] `DriveMetrics` contains only name, used bytes, and total bytes.
- [ ] Drive collection distinguishes unavailable from successfully empty.
- [ ] No duplicated aggregate/percentage wire values exist.
- [ ] Entry count and name size have explicit bounds.
- [ ] Wire types do not carry platform-only handles or paths beyond the public display name.

## Workstream B: resolve published Rust source compatibility

`StatusSnapshotV2` is public and is constructed through struct literals in repository tests and potentially downstream crates. Adding a public field directly is an additive JSON change but a Rust source-breaking change.

Review the actual construction sites and choose one of these deliberately:

### Preferred option: flat status payload wrapper

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusPayloadV2 {
    #[serde(flatten)]
    pub snapshot: StatusSnapshotV2,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drives: Option<Vec<DriveMetrics>>,
}
```

Requirements:

- serialized JSON remains flat;
- existing `StatusSnapshotV2` remains source-compatible;
- `/v2/status` serves `StatusPayloadV2`;
- polling parses the payload and still validates the base snapshot plus drive list;
- v2 health behavior remains intentionally unchanged unless the current endpoint contract requires the same payload type.

### Acceptable option: direct optional field

A direct field may be used only if maintainers intentionally accept source breakage for the next release and update every public API note/test accordingly. Do not choose this accidentally because it produces less code.

### Decision record

Record the selected design in `architecture/protocol.md` and rustdoc. Keep the note short: JSON compatibility, Rust source compatibility, and why no schema major was added.

### Workstream B acceptance criteria

- [ ] The source-compatibility impact is explicitly reviewed.
- [ ] Old `StatusSnapshotV2` literals either continue compiling or the break is intentionally documented.
- [ ] Serialized `/v2/status` JSON remains the existing flat shape plus optional `drives`.
- [ ] No v3 endpoint or generic envelope hierarchy is introduced.

## Workstream C: validate drive records

Extend v2 validation with focused violations. Reuse existing violation conventions and field paths rather than adding a parallel validator.

Required checks:

```text
drives.len() <= MAX_DRIVE_ENTRIES
name is nonempty after validation policy
name byte length <= MAX_DRIVE_NAME_BYTES
total_bytes > 0
used_bytes <= total_bytes
```

Do not reject platform-valid characters such as `/`, `\`, spaces, Unicode, colons, or mount-point punctuation. Do not canonicalize names in validation.

If the wrapper approach is selected, provide one public validation method on the wrapper that:

1. validates the base `StatusSnapshotV2`;
2. validates each drive in order;
3. returns the same structured v2 violation family with paths such as:

```text
drives
drives[0].name
drives[0].total_bytes
drives[0].used_bytes
```

Required tests:

- one valid drive;
- several valid drives;
- omitted drives;
- explicit null drives if serde produces the same `None` state;
- empty successful list;
- empty name;
- overlong name;
- zero total;
- used greater than total;
- too many entries;
- Unicode and Windows root names accepted;
- unknown additive JSON fields still ignored according to existing policy.

### Workstream C acceptance criteria

- [ ] Every drive invariant has one focused positive/negative test.
- [ ] Violations identify the indexed drive field.
- [ ] Validation does not inspect `os_name` or filesystem type.
- [ ] Existing v2 snapshot validation remains unchanged for non-drive fields.

## Workstream D: update compatibility fixtures and builders

Update `gregg-protocol` test support without making every existing builder call supply drives.

Requirements:

- default v2 builders produce the old/no-drive shape or an explicit sensible fixture according to the selected wrapper API;
- add builder methods for unavailable, empty, and populated drive data;
- add at least one Linux, macOS, and Windows representative populated status fixture;
- retain an old v2 fixture with no drive field and prove it parses/validates;
- ensure fixture round trips remain deterministic;
- avoid host-specific exact capacities in fixtures beyond clear synthetic numbers.

Do not modify v1 fixtures.

### Workstream D acceptance criteria

- [ ] Old v2 fixture without drives passes.
- [ ] Populated cross-platform fixtures pass.
- [ ] V1 fixture bytes and semantics remain untouched.
- [ ] Test builders keep common test setup concise.

## Workstream E: carry drive data through `greggd`

Extend the daemon-internal sample:

```rust
pub struct CollectedMetrics {
    // existing fields
    pub drives: Option<Vec<DriveMetrics>>,
}
```

At this phase, platform collectors may return `None` until Phase 50 supplies real implementations.

Conversion requirements:

- v1 conversion ignores drive data completely;
- v2 status conversion preserves `None`, empty, or populated values;
- one collected sample still produces all cached representations;
- health/readiness behavior remains driven by existing core collector readiness, not by drive availability;
- no server route performs collection.

Update sampler/server tests with synthetic drive data to prove `/v2/status` serialization. Do not change the root or v1 endpoint response.

If the wrapper approach is selected, change the cached v2 status type and server state narrowly. Avoid renaming unrelated snapshot types or rewriting the server module.

### Workstream E acceptance criteria

- [ ] `CollectedMetrics` carries optional drive data.
- [ ] V1 conversion is byte/semantic compatible.
- [ ] V2 status includes synthetic drives when present.
- [ ] Drive unavailability does not change readiness.
- [ ] No duplicate sampling or request-triggered collection exists.

## Workstream F: normalize and aggregate in `gregg`

Add a client-owned normalized drive record or reuse the dependency-light protocol record if doing so does not leak wire-version assumptions. Prefer a small normalized type if aggregate helpers benefit from a stable internal API.

Conceptual internal shape:

```rust
pub struct NormalizedDrive {
    pub name: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

pub struct NormalizedSnapshot {
    // existing fields
    pub drives: Option<Vec<NormalizedDrive>>,
}
```

V1 normalization sets `drives = None`. V2 normalization copies the optional drive collection.

Add one aggregate helper:

```rust
pub struct DriveAggregate {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub usage_pct: f32,
}

pub fn aggregate_drives(drives: &[NormalizedDrive]) -> Option<DriveAggregate>
```

Required semantics:

- empty list returns `None` so the renderer can show unavailable/no eligible storage rather than a fake zero-capacity bar;
- sums use checked or saturating arithmetic with an explicit policy;
- because protocol values are individually valid, normal sums should satisfy used <= total;
- `available_bytes = total_bytes - used_bytes`;
- zero total never produces NaN or infinity;
- percentage is finite and clamped to `0.0..=100.0` defensively;
- source drive order is preserved for detail rendering.

Preferred overflow policy: checked sums returning `None` or a dedicated aggregate error internally. Do not silently wrap. Saturation is acceptable only if documented and tested, but checked failure is clearer because impossible aggregate sizes indicate invalid/unrepresentable input.

Required tests:

- v1 -> unavailable drives;
- old v2 -> unavailable drives;
- populated v2 preserves names/order;
- empty list;
- one drive;
- several drives;
- exact used/available arithmetic;
- zero/invalid defensive cases through directly constructed internal values;
- sum overflow behavior.

### Workstream F acceptance criteria

- [ ] State/UI can consume one normalized drive representation independent of wire version.
- [ ] One helper owns aggregate arithmetic.
- [ ] Empty/unavailable behavior is explicit.
- [ ] Aggregation cannot wrap integer sums.
- [ ] Detail order remains stable.

## Workstream G: response-size and security review

The current client caps response bodies at 64 KiB. Calculate the bounded worst-case drive payload using the selected name and entry limits.

Preferred resolution:

- choose limits that remain below the existing cap with substantial room for the rest of the snapshot;
- keep `MAX_RESPONSE_BYTES` unchanged.

Only increase the cap if the bounded calculation proves it necessary. Any increase must remain small and documented. Do not remove the cap or make it configurable.

No filesystem paths, native error chains, mount options, credentials, or privileged metadata are exposed beyond the operator-visible drive name.

### Workstream G acceptance criteria

- [ ] Worst-case serialized payload fits the documented response bound.
- [ ] Existing network/body-size defenses remain enabled.
- [ ] Drive errors do not leak private native error chains through the wire.

## Expected files

Primary files likely include:

```text
crates/gregg-protocol/src/v2.rs
crates/gregg-protocol/src/validate_v2.rs
crates/gregg-protocol/src/test_support.rs
crates/gregg-protocol/tests/fixtures/*-v2.json
crates/greggd/src/collector/mod.rs
crates/greggd/src/sampler.rs
crates/greggd/src/server/mod.rs
crates/greggd/src/server/tests.rs
crates/gregg/src/poller.rs
crates/gregg/src/normalized.rs
architecture/protocol.md
```

Do not touch TUI layout/rendering files in this phase.

## Implementation sequence

1. Inventory all public `StatusSnapshotV2` construction and server/poller parse sites.
2. Choose and document wrapper versus direct optional field.
3. Add `DriveMetrics`, bounds, and serialization semantics.
4. Add indexed validation and fixtures.
5. Extend `CollectedMetrics` and cached v2 status plumbing with `None` defaults.
6. Update server synthetic tests.
7. Extend v2 polling/parsing and normalized snapshots.
8. Add aggregate helper and exhaustive arithmetic tests.
9. Update focused protocol documentation.
10. Run crate-level checks and inspect the diff for accidental v1/TUI changes.

## Required verification

Use focused commands during implementation:

```text
cargo fmt --all -- --check
cargo test -p gregg-protocol --all-targets --all-features
cargo test -p greggd server sampler collector --all-features
cargo test -p gregg normalized poller --all-features
cargo clippy -p gregg-protocol -p greggd -p gregg --all-targets --all-features -- -D warnings
```

If Cargo test-name filtering does not match repository organization, run the corresponding full crate tests. Do not add a new verification script.

## Phase acceptance criteria

Phase 49 is complete only when:

- [x] V1 types, fixtures, conversion, and endpoints are unchanged.
- [x] V2 status can represent unavailable, empty, and populated drive data.
- [x] Every drive entry carries only name, used bytes, and total bytes.
- [x] Drive list and name size are bounded.
- [x] Old v2 JSON without drives parses and validates.
- [x] The public Rust source-compatibility decision is documented and tested.
- [x] Invalid drive entries return structured indexed violations.
- [x] `CollectedMetrics` carries drives without affecting readiness or v1.
- [x] `/v2/status` serializes synthetic drive data from cached state.
- [x] The client normalizes drive data across v1/v2.
- [x] Aggregate used/total/available/percentage arithmetic is centralized and overflow-safe.
- [x] The existing response-size cap remains effective.
- [x] No native collection, TUI behavior, new route, or new workflow was added.

## Handoff guidance for a smaller implementation model

- Start by writing compatibility tests before changing the public type.
- Do not mutate `StatusSnapshotV2` directly until the struct-literal impact has been inspected.
- Keep v1 code paths mechanically unchanged; add tests that prove this rather than refactoring them.
- Use synthetic drive values in daemon/server tests. Phase 50 supplies real values later.
- Keep aggregation in `normalized.rs`; do not place arithmetic in renderers.
- Stop if implementation starts requiring a protocol registry, trait-object metric system, or second endpoint. That is outside scope.
