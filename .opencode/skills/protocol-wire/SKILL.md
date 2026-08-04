---
name: protocol-wire
description: Work with gregg-protocol wire types, schema versions, and validation
---

## What I do

Guide agents through the protocol crate's wire types, schema versions, validation rules, and compatibility constraints.

## When to use me

Use this when modifying wire types, adding new schema versions, changing validation rules, or working with the protocol crate.

## Schema versions

- **V1** (`SCHEMA_VERSION_V1 = 1`): Original Linux/macOS format with required load/swap
- **V2** (`SCHEMA_VERSION_V2 = 2`): Extended with capability flags for load, swap, commit; drives array

The client requests v2 first, accepts only the schema matching each endpoint, and falls back to v1 only on an HTTP 404 from /v2/status. `/v2/status` is the universal cross-platform endpoint.

## Key types

| Type | Location | Purpose |
|------|----------|---------|
| `StatusSnapshot` | `src/snapshot.rs` | V1 wire type |
| `StatusSnapshotV2` | `src/v2.rs` | V2 wire type |
| `StatusPayloadV2` | `src/v2.rs` | Flat wrapper with optional `drives` |
| `MetricCapabilities` | `src/snapshot.rs` | V1 capability flag (cpu_iowait) |
| `MetricCapabilitiesV2` | `src/v2.rs` | V2 capability flags (4 flags) |
| `DriveMetrics` | `src/v2.rs` | Per-drive used/total bytes |
| `CommitMetrics` | `src/v2.rs` | Windows commit charge |
| `HealthResponse` | `src/health.rs` | V1 health type |
| `HealthResponseV2` | `src/v2.rs` | V2 health type |

## Capability flags

| Flag | Linux | macOS | Windows |
|------|-------|-------|---------|
| `cpu_iowait` | `true` | `false` | `false` |
| `load_average` | `true` | `true` | `false` |
| `swap` | `true` | `false` | `false` |
| `memory_commit` | `false` | `false` | `true` |

A `false` capability means the corresponding field must be `None`/`null`. Validation rejects capability/value contradictions.

## Platform-specific rules

- macOS: `iowait_pct` is `null` (unsupported). Never fabricate `0.0`.
- Windows: load average, swap, iowait are all `null`/unsupported. Windows reports `commit` instead.
- Drives: `null` = unavailable/legacy, empty list = no eligible filesystems.

## Validation

Validation is intentionally separate from serde deserialization. Adding fields that serde does not know about must not change the strictness of validation.

### V1 violation kinds

| Kind | What it catches |
|------|----------------|
| `UnsupportedSchemaVersion` | `schema_version` != 1 |
| `ZeroNotAllowed` | Timestamps or logical_cores = 0 |
| `PercentageNotFinite` | NaN or infinity in percentage fields |
| `PercentageOutOfRange` | Percentage outside `0.0..=100.0` |
| `UsedExceedsTotal` | `used_bytes > total_bytes` |
| `IowaitCapabilityMismatch` | iowait presence disagrees with capability |

### V2 additional violation kinds

| Kind | What it catches |
|------|----------------|
| `LoadCapabilityMismatch` | load presence disagrees with capability |
| `SwapCapabilityMismatch` | swap presence disagrees with capability |
| `CommitCapabilityMismatch` | commit presence disagrees with capability |
| `EmptyDriveName` | Drive name is empty string |
| `DriveNameTooLong` | Drive name > 512 UTF-8 bytes |
| `TooManyDrives` | More than 32 drive entries |

V2 total: 12 violation kinds (6 from V1 + 6 additional).

## Test support

The `test_support` feature flag exposes builder fixtures:

| Builder | Produces |
|---------|----------|
| `LinuxSnapshotBuilder` | V1 Linux snapshot with iowait |
| `MacosSnapshotBuilder` | V1 macOS snapshot without iowait |
| `LinuxSnapshotV2Builder` | V2 Linux snapshot with optional drives |
| `WindowsSnapshotV2Builder` | V2 Windows snapshot with commit |

## Fixture files

Located in `crates/gregg-protocol/tests/fixtures/`:
- `linux-v1.json`, `linux-v2.json`
- `macos-v1.json`, `macos-v2.json`
- `windows-v2.json`
- `health-ready-v1.json`, `health-warming-v1.json`, `health-collector-failure-v1.json`
- `health-ready-v2.json`

Fixtures deserialize, validate, and re-serialize byte-stably. Integration tests round-trip every fixture.

## Key constraints

- `#![forbid(unsafe_code)]` — no unsafe in this crate
- No runtime, HTTP, terminal, or platform dependencies
- Only `serde`, `serde_json`, and `thiserror` as dependencies
- Schema version is explicit; unknown versions are rejected, not ignored
