# gregg-protocol deep dive

The protocol crate defines the shared wire contract between daemon and client.
It is the foundation crate that both `greggd` and `gregg` depend on, and it
depends on nothing from either.

**Source:** `crates/gregg-protocol/`

## Purpose

- Define JSON serialization types for status snapshots, health responses, and
  validation errors
- Enforce schema versioning (v1 and v2)
- Provide structured validation separate from serde deserialization
- Support capability flags so platforms can truthfully report which metrics
  they support

## Module map

| Module | File | Purpose |
|--------|------|---------|
| `lib` | `src/lib.rs` | Root, re-exports, `SCHEMA_VERSION_V1 = 1`, `MAX_IDENTITY_FIELD_BYTES = 512`, `#![forbid(unsafe_code)]` |
| `snapshot` | `src/snapshot.rs` | V1 wire types: `StatusSnapshot`, `CpuMetrics`, `LoadAverage`, `MemoryMetrics`, `SwapMetrics`, `SystemIdentity`, `MetricCapabilities` |
| `v2` | `src/v2.rs` | V2 wire types: `StatusSnapshotV2`, `StatusPayloadV2`, `CpuMetricsV2`, `SwapMetrics`, `MetricCapabilitiesV2`, `DriveMetrics`, `CommitMetrics`, `HealthResponseV2`; constants `SCHEMA_VERSION_V2`, `MAX_DRIVE_ENTRIES`, `MAX_DRIVE_NAME_BYTES` |
| `validate` | `src/validate.rs` | V1 validation: 9 violation kinds; re-exports `validate()` |
| `validate_v2` | `src/validate_v2.rs` | V2 validation: 16 violation kinds, capability/value consistency; re-exports `validate_v2()` and `validate_payload_v2()` |
| `health` | `src/health.rs` | V1 health types: `HealthResponse`, `ReadinessState`, `HealthCategory` |
| `test_support` | `src/test_support.rs` | Feature-gated builder fixtures for tests |

## Wire format

All payloads are JSON with `snake_case` field names. The v1 status endpoint
returns `StatusSnapshot` directly. The v2 status endpoint returns
`StatusPayloadV2` which flattens the snapshot and adds an optional `drives` array.

### V1 snapshot shape

```json
{
  "schema_version": 1,
  "observed_at_unix_ms": 1234567890000,
  "sample_interval_ms": 1000,
  "capabilities": { "cpu_iowait": true },
  "system": { "name": "web-01", "hostname": "web-01.example.com", ... },
  "cpu": { "logical_cores": 8, "usage_pct": 25.2, "iowait_pct": 1.1 },
  "load": { "one": 1.5, "five": 1.2, "fifteen": 1.0 },
  "memory": { "used_bytes": 4294967296, "total_bytes": 17179869184, "usage_pct": 25.0 },
  "swap": { "used_bytes": 0, "total_bytes": 8589934592, "usage_pct": 0.0 }
}
```

### V2 snapshot shape

```json
{
  "schema_version": 2,
  "capabilities": { "cpu_iowait": false, "load_average": true, "swap": false, "memory_commit": true },
  "load": null,
  "swap": null,
  "commit": { "used_bytes": 1073741824, "limit_bytes": 4294967296, "usage_pct": 25.0 },
  "drives": [
    { "name": "/", "used_bytes": 10737418240, "total_bytes": 53687091200, "available_bytes": 42949672960 }
  ]
}
```

### Capability flags

| Flag | Linux | macOS | Windows |
|------|-------|-------|---------|
| `cpu_iowait` | `true` | `false` | `false` |
| `load_average` | `true` | `true` | `false` |
| `swap` | `true` | `true` | `false` |
| `memory_commit` | `false` | `false` | `true` |

A `false` capability means the corresponding field must be `None`/`null`.
All four v2 capability keys are required when decoding; validation also
rejects capability/value contradictions. Every system identity field is
limited to 512 UTF-8 bytes in addition to the empty/NUL checks.

## Validation

Validation is intentionally separate from serde. A payload can parse
successfully but fail validation (e.g., `schema_version = 99`). This keeps
additive JSON changes from silently loosening invariants.

### V1 violation kinds

| Kind | What it catches |
|------|----------------|
| `UnsupportedSchemaVersion` | `schema_version` != 1 |
| `ZeroNotAllowed` | Timestamps or logical_cores = 0 |
| `SampleIntervalOutOfRange` | `sample_interval_ms` exceeds 24-hour protocol maximum |
| `PercentageNotFinite` | NaN or infinity in percentage fields |
| `PercentageOutOfRange` | Percentage outside `0.0..=100.0` |
| `LoadValueOutOfRange` | Load average non-finite or negative |
| `UsedExceedsTotal` | `used_bytes > total_bytes` |
| `IowaitCapabilityMismatch` | iowait presence disagrees with capability |
| `InvalidIdentityField` | Identity string empty, NUL-padded, or over 512 UTF-8 bytes |

### V2 additional violation kinds

| Kind | What it catches |
|------|----------------|
| `AvailableExceedsTotal` | `available_bytes > total_bytes` |
| `LoadCapabilityMismatch` | load presence disagrees with `load_average` capability |
| `SwapCapabilityMismatch` | swap presence disagrees with `swap` capability |
| `CommitCapabilityMismatch` | commit presence disagrees with `memory_commit` capability |
| `EmptyDriveName` | Drive name is empty string |
| `DriveNameTooLong` | Drive name > 512 UTF-8 bytes |
| `TooManyDrives` | More than 32 drive entries |

V2 total: 16 violation kinds (9 from V1 + 7 additional).

## Health responses

Three states:
- **Ready** — daemon has a valid cached snapshot; includes the snapshot
- **Warming** — daemon alive but first counter delta not yet available
- **Failed** — collector error; carries category + message (no paths/chains)

Windows v2-only publication returns v1 `NotServing` health with HTTP 503;
v2 status and health remain independently ready after a valid sample.

Both v1 (`HealthResponse`) and v2 (`HealthResponseV2`) have constructors for
each state: `ready()`, `warming()`, `warming_with_message()`, `failed()`.
`StatusPayloadV2` also has its own `validate()` method.

## Test support

The `test_support` feature flag exposes builder fixtures and shared identity
defaults:

| Builder | Produces |
|---------|----------|
| `LinuxSnapshotBuilder` | V1 Linux snapshot with iowait |
| `MacosSnapshotBuilder` | V1 macOS snapshot without iowait |
| `LinuxSnapshotV2Builder` | V2 Linux snapshot with optional drives and `build_payload()` |
| `WindowsSnapshotV2Builder` | V2 Windows snapshot with commit and `build_payload()` |

`IdentityFixture` provides `linux()`, `macos()`, and `windows()` const
constructors for shared identity defaults across all builders.

All builders call `validate()` on `build()` (and `validate()` on
`build_payload()`) and panic if the fixture is invalid. This ensures tests
always start from valid baselines.

**Design note:** `DriveMetrics.available_bytes` is `Option<u64>` — callers
cannot assume it is always present. The `drive.total_bytes != 0` invariant
is enforced by validation.

## Fixture files

Located in `tests/fixtures/`:
- `linux-v1.json`, `linux-v2.json`
- `macos-v1.json`, `macos-v2.json`
- `windows-v2.json`
- `health-ready-v1.json`, `health-warming-v1.json`, `health-collector-failure-v1.json`
- `health-ready-v2.json`

Fixtures deserialize, validate, and re-serialize byte-stably. Integration
tests round-trip every fixture.

## Key constraints

- `#![forbid(unsafe_code)]` — no unsafe in this crate
- No runtime, HTTP, terminal, or platform dependencies
- Only `serde`, `serde_json`, and `thiserror` as dependencies
- Schema version is explicit; unknown versions are rejected, not ignored
