# Plan 108: additive live-metrics protocol and client normalization

Status: implementation in progress.

Depends on: Plan 107.

Blocks: Plans 109-111.

## Objective

Extend the existing schema-v2 payload and client normalization layer so CPU frequency, disk-I/O throughput, and network telemetry can be added without breaking old daemons or forcing a protocol-v3 migration.

This plan is protocol/model work only. It does not implement native OS collection or TUI rendering.

## Governing compatibility contract

A new `gregg` client must continue to accept:

- v1-only daemons through the existing v2-first/v1-on-404 fallback;
- old v2 payloads that contain the current required snapshot plus optional `drives` but none of the new fields;
- new v2 payloads containing any supported subset of the new optional telemetry.

An old client should continue to ignore additional JSON fields emitted by a new daemon.

Do not add new required keys to `MetricCapabilitiesV2` for these features. That struct currently has required fields, so extending it naively would cause old-v2 payloads to fail deserialization in a new client. Presence/absence of the optional telemetry itself is sufficient capability signaling for this pass.

## Protocol shape

Extend `gregg_protocol::v2::StatusPayloadV2` additively. Exact names may be adjusted for repository conventions, but the intended shape is:

```rust
pub struct StatusPayloadV2 {
    #[serde(flatten)]
    pub snapshot: StatusSnapshotV2,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drives: Option<Vec<DriveMetrics>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_frequency_hz: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_io: Option<DiskIoPayload>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPayload>,
}
```

Do not add daemon binary version in this plan. Plan 107 explicitly defers fleet version transport.

## CPU frequency field

Semantics:

- unit is Hz;
- value is a host-level current-frequency summary produced by the collector;
- zero is invalid and should normalize to absence/rejection according to validation policy;
- missing/null means unavailable, unsupported, legacy daemon, or current collection unavailable;
- the wire never carries formatted `GHz` text.

Validation should reject values that cannot plausibly represent a positive frequency because of arithmetic corruption, but do not encode an overly narrow hardware ceiling that future CPUs could violate. A simple positive-value requirement plus checked conversion at the collector boundary is preferable to a speculative maximum.

## Disk-I/O wire model

Capacity and throughput are separate concepts.

Introduce a bounded payload conceptually like:

```rust
pub struct DiskIoPayload {
    pub aggregate_read_bytes_per_sec: u64,
    pub aggregate_write_bytes_per_sec: u64,
    pub devices: Vec<DiskIoMetrics>,
}

pub struct DiskIoMetrics {
    pub id: String,
    pub name: String,
    pub read_bytes_per_sec: u64,
    pub write_bytes_per_sec: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_name: Option<String>,
}
```

The exact association field may instead use an existing stable drive identity if Plan 109 establishes one cleanly. The important invariants are:

- aggregate throughput is supplied by the daemon from a de-duplicated accounting set; the client must not derive it by blindly summing display rows;
- per-device records have a stable identity for baseline/reset handling and deterministic rendering;
- optional association with existing drive/mount display records is explicit and may be absent;
- one device's inability to map to a mount does not make its I/O counters unusable;
- no field claims a physical-drive mapping when the source only identifies a logical block device/volume.

### Bounds

Add explicit protocol constants for:

- maximum disk-I/O entries;
- maximum id bytes;
- maximum display-name bytes.

Keep limits in the same scale as existing drive bounds unless native-platform enumeration demonstrates a need for more. The goal is bounded payloads, not exhaustive enterprise storage inventory.

## Network wire model

Introduce a bounded payload conceptually like:

```rust
pub struct NetworkPayload {
    pub aggregate_rx_bytes_per_sec: u64,
    pub aggregate_tx_bytes_per_sec: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_rx_capacity_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_tx_capacity_bps: Option<u64>,
    pub interfaces: Vec<NetworkInterfaceMetrics>,
}

pub struct NetworkInterfaceMetrics {
    pub id: String,
    pub name: String,
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_capacity_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_capacity_bps: Option<u64>,
    pub is_loopback: bool,
    pub aggregate_member: bool,
}
```

`aggregate_member` is recommended because topology/capacity eligibility is platform-specific and belongs in `greggd`, not in the client. If the implementation finds a smaller equivalent representation, keep the same semantic ownership: `gregg` must not rediscover Linux bond/bridge topology from names.

### Aggregate utilization derivation

The wire should carry raw throughput and capacity, not a preformatted percentage. The normalized/client layer can derive:

```text
rx_pct = 8 * aggregate_rx_bytes_per_sec / aggregate_rx_capacity_bps
tx_pct = 8 * aggregate_tx_bytes_per_sec / aggregate_tx_capacity_bps
usage_pct = max(rx_pct, tx_pct)
```

Clamp finite final display percentage to `0..=100`. If a directional capacity is absent or zero, that direction has no percentage. If neither direction has a valid capacity, aggregate network utilization is unavailable even though throughput remains available.

If only one direction has valid capacity, use that valid direction rather than fabricating the other.

## Rate representation

Use integer bytes per second (`u64`) for disk and network rates.

Do not use floating point on the wire for byte rates. The collector may compute with higher precision internally and round/truncate in a documented way after checked arithmetic.

Do not carry sampler cumulative counters on the public wire unless implementation proves they are needed by the client. Baselines belong to `greggd`; the client consumes rates.

## Client normalized model

Extend `crates/gregg/src/normalized.rs` with client-owned equivalents, conceptually:

```rust
pub struct NormalizedSnapshot {
    ...
    pub cpu_frequency_hz: Option<u64>,
    pub disk_io: Option<NormalizedDiskIo>,
    pub network: Option<NormalizedNetwork>,
}
```

V1 normalization must set all three to `None`.

Old-v2 payload normalization must set missing fields to `None`.

Do not branch in renderers on `wire_version` to determine support. Renderers consume normalized optional fields only.

### Derived helpers

Provide small pure helpers where useful:

- aggregate network utilization percentage from normalized directional throughput/capacity;
- checked byte-rate-to-bit-rate conversion;
- any disk/device lookup association needed by the detail renderer.

Keep formatting (`GHz`, `MiB/s`, percentage strings) out of normalization.

## Validation rules

Extend v2 payload validation without changing base snapshot invariants.

At minimum validate:

1. CPU frequency, if present, is nonzero.
2. Disk/network entry counts are bounded.
3. IDs/names are non-empty, NUL-free, and byte-length bounded.
4. IDs are unique inside each list.
5. Aggregate and per-record rates/capacities are ordinary finite integer quantities and do not require floating validation.
6. Capacity `Some(0)` is rejected or normalized to `None`; choose one invariant and enforce it consistently.
7. `aggregate_member` interfaces with capacity absent are allowed when they still contribute throughput, but the aggregate capacity fields must be computed only from valid eligible capacities by the daemon.
8. Loopback may be present in detail but must not be marked as an aggregate capacity member.
9. Payload deserialization of an old v2 JSON object with none of the new keys succeeds unchanged.

Do not attempt to validate that an aggregate rate equals the sum of every per-device record: platform topology intentionally allows the aggregate accounting set to differ from the display list.

## Test fixtures and compatibility evidence

Add fixtures/tests for at least:

### Protocol deserialization

- pre-feature v2 payload with `drives` omitted;
- pre-feature v2 payload with `drives` present;
- new payload with only CPU frequency;
- new payload with disk I/O only;
- new payload with network only;
- new payload with every feature;
- explicit `null` new fields;
- unknown future JSON fields remain ignored by current serde behavior.

### Validation

- zero CPU frequency rejected;
- duplicate disk IDs rejected;
- duplicate interface IDs rejected;
- oversized names/IDs rejected;
- loopback aggregate-member invariant rejected;
- zero/invalid capacities handled consistently;
- maximum-entry boundary accepted, one over rejected.

### Normalization

- v1 -> all new telemetry absent;
- old v2 -> all new telemetry absent;
- new v2 -> values preserved;
- valid network directional percentage math including simultaneous full-duplex traffic;
- 100 Mb/s RX + 100 Mb/s TX on a 100 Mb/s full-duplex interface reports 100%, not 200%;
- multiple capacities aggregate correctly;
- missing capacity yields no progress percentage while throughput remains accessible.

## Files likely touched

At minimum inspect/update:

```text
crates/gregg-protocol/src/v2.rs
crates/gregg-protocol/src/validate_v2.rs
crates/gregg-protocol/src/test_support.rs
crates/gregg/src/normalized.rs
crates/gregg/src/mixed_fleet_evidence.rs
architecture/protocol.md
architecture/gregg-client.md
```

Do not update user-facing TUI docs yet except where protocol architecture requires it; Plan 110/111 own final display documentation.

## Local verification

Mandatory:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p gregg-protocol --all-targets --all-features
cargo test -p gregg --all-targets --all-features normalized
cargo test -p gregg --all-targets --all-features mixed_fleet
cargo test --workspace --all-targets --all-features
```

No new CI job or protocol-major endpoint is required.

## Acceptance criteria

Plan 108 is complete only when:

1. New telemetry is additive and optional in `StatusPayloadV2`.
2. No existing required v2 capability key is changed in a way that breaks old payloads.
3. Pre-feature v2 JSON fixtures deserialize and normalize successfully.
4. V1 normalization sets all new metrics to absent.
5. CPU frequency uses raw Hz; disk/network throughput uses integer bytes/s; link capacity uses bits/s.
6. Disk I/O carries a daemon-computed aggregate separate from potentially overlapping display/device records.
7. Network payload preserves directional throughput and directional capacity and identifies daemon-selected aggregate membership.
8. Network utilization helper is full-duplex-safe and reports no utilization percentage when capacity is unavailable.
9. Validation bounds every new collection and string field.
10. Renderer code does not need to branch on protocol/wire version.
11. Daemon-version transport remains explicitly out of scope.
12. Full local verification passes.
