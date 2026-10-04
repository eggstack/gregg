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
- **V2** (`SCHEMA_VERSION_V2 = 2`): Extended with capability flags for load,
  swap, commit; optional drives, CPU frequency, disk-I/O, and network arrays

The client requests v2 first, accepts only the schema matching each endpoint, and falls back to v1 only on an HTTP 404 from /v2/status. `/v2/status` is the universal cross-platform endpoint.

## Key types

| Type | Location | Purpose |
|------|----------|---------|
| `StatusSnapshot` | `src/snapshot.rs` | V1 wire type |
| `StatusSnapshotV2` | `src/v2.rs` | V2 wire type |
| `StatusPayloadV2` | `src/v2.rs` | Flat wrapper with optional drives and live telemetry |
| `MetricCapabilities` | `src/snapshot.rs` | V1 capability flag (cpu_iowait) |
| `MetricCapabilitiesV2` | `src/v2.rs` | V2 capability flags (4 flags) |
| `DriveMetrics` | `src/v2.rs` | Per-drive used/total and optional caller-available bytes |
| `DiskIoPayload` / `DiskIoMetrics` | `src/v2.rs` | Daemon-selected aggregate and bounded per-device byte rates |
| `NetworkPayload` / `NetworkInterfaceMetrics` | `src/v2.rs` | Directional aggregate/interface byte rates and bit capacities |
| `CommitMetrics` | `src/v2.rs` | Windows commit charge |
| `HealthResponse` | `src/health.rs` | V1 health type |
| `HealthResponseV2` | `src/v2.rs` | V2 health type |

## Capability flags

| Flag | Linux | macOS | Windows |
|------|-------|-------|---------|
| `cpu_iowait` | `true` | `false` | `false` |
| `load_average` | `true` | `true` | `false` |
| `swap` | `true` | `true` | `false` |
| `memory_commit` | `false` | `false` | `true` |

A `false` capability means the corresponding field must be `None`/`null`. All
four v2 capability keys are required when decoding; omitted keys are not
treated as explicit `false`. Validation rejects capability/value
contradictions. System identity fields are limited to 512 UTF-8 bytes.

## Platform-specific rules

- macOS: `iowait_pct` is `null` (unsupported). Never fabricate `0.0`.
- Windows: load average, swap, iowait are all `null`/unsupported. Windows reports `commit` instead.
- Drives: `null` = unavailable/legacy, empty list = no eligible filesystems.
  `available_bytes` is optional for old-v2 compatibility; when present it is
  caller-available space and is independent of total filesystem free space.
- `cpu_frequency_hz` is an optional positive raw Hz value; never serialize
  formatted frequency text. It means current OS-reported frequency, not a
  base/max frequency claim; macOS may omit it.
- Disk/network rates are integer bytes per second. Network capacities are
  directional bits per second. Aggregate rates are daemon-selected and must
  not be reconstructed by summing detail records.
- Network loopback can appear in detail but cannot be an aggregate member;
  missing capacity leaves throughput available without a utilization value.

Compatibility matrix: v1-only daemons use the client's v2-404 fallback and
leave all live fields absent; pre-feature v2 daemons deserialize unchanged and
leave missing optional fields absent; current v2 families are normalized
independently. Older v2 clients ignore the additive JSON keys. Do not add
daemon-version transport to this contract.

## Validation

Validation is intentionally separate from serde deserialization. Adding fields that serde does not know about must not change the strictness of validation.

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
| `InvalidIdentityField` | Identity string empty/whitespace-only, NUL-padded, or over 512 UTF-8 bytes |

### V2 additional violation kinds

| Kind | What it catches |
|------|----------------|
| `AvailableExceedsTotal` | `available_bytes > total_bytes` |
| `LoadCapabilityMismatch` | load presence disagrees with capability |
| `SwapCapabilityMismatch` | swap presence disagrees with capability |
| `CommitCapabilityMismatch` | commit presence disagrees with capability |
| `EmptyDriveName` | Drive name is empty, whitespace-only, or NUL-padded |
| `DriveNameTooLong` | Drive name > 512 UTF-8 bytes |
| `TooManyDrives` | More than 32 drive entries |

V2 also validates live-metrics collection/string bounds, duplicate IDs,
positive CPU frequency/capacities, and the loopback aggregate-member rule.
`Some(0)` capacities and zero CPU frequency are rejected.

36 `ViolationKindV2` variants total: the 16 base kinds above plus
`CpuFrequencyZero`, `CpuFrequencyExceedsMaximum` (2^34 Hz),
`TooManyDiskIoDevices`, `DiskIoIdInvalid`/`TooLong`,
`DiskIoNameInvalid`/`TooLong`, `DuplicateDiskIoId`, `DuplicateDriveName`,
`UnknownDriveAssociation`, `TooManyNetworkInterfaces`,
`NetworkInterfaceIdInvalid`/`TooLong`, `NetworkInterfaceNameInvalid`/`TooLong`,
`DuplicateNetworkInterfaceId`, `ZeroCapacity`, `CapacityExceedsMaximum`
(2^48 bps), `LoopbackAggregateMember`, and `RateExceedsMaximum` (1 TiB/s).

Duplicate detection is hash-set based (linear over an attacker-sized
collection) and per-entry violations are still reported after the
`TooMany*` bound fires. A disk-I/O `drive_name` association must match a
payload drive whenever `drives` is `Some`; an empty `Some([])` makes any
association dangling, and only `drives: None` skips the check. A drive with
`total_bytes == 0` is a valid empty/placeholder volume unless it also claims
non-zero `used_bytes`/`available_bytes`.

## Scheduler observability routes

Additive and read-only, deliberately separate from `/v2/status` so metrics
polls never carry command output:

~~~text
GET/HEAD /v2/scheduler          -> SchedulerSummaryV2
GET/HEAD /v2/scheduler/history  -> SchedulerHistoryV2
~~~

`GET`/`HEAD` only (else 405); every other scheduler path is 404; no job
create/edit/start/cancel route exists. No configured jobs returns `200` with an
empty document; a pre-feature daemon returns `404`, which the client reads as
*observability unsupported*, not host offline.

- Live state is an enum: `idle` | `waiting_for_slot` | `load_high` |
  `load_unavailable` | `running`. `load_unavailable` must carry no `observed`
  reading, and `waiting_for_slot` must never render as load-delayed.
- Terminal outcome is an enum: `success` | `failed` | `spawn_failed` |
  `wait_failed` | `load_expired` | `cancelled` (reserved). No exit code,
  signal, start time, or duration on an outcome that never ran a child.
- Deduplication identity is `(SchedulerEpochV2, sequence)`. The epoch is a
  start timestamp plus an FNV-1a nonce; it is a dedup aid, never an auth token.
- `history_revision` changes only when retained history changes, so a live
  transition does not force a history refetch.
- Published output is bounded in **JSON-escaped** bytes
  (`MAX_SCHEDULER_OUTPUT_TEXT_BYTES = 512`), not raw bytes, because
  `serde_json` renders a C0 byte as six. That makes the maximum history body a
  closed calculation (832,022 bytes measured worst case vs a 1 MiB client cap).
- No `argv`/`working_dir` is published, but the listener is unauthenticated:
  document that job output is readable by anything that can reach it.

## Health envelopes

The state/category pairing is a total allowlist enforced on deserialize:

| State | Allowed category |
|-------|------------------|
| `ready` | none, and no `message` |
| `warming` | `warming` |
| `failed` | `collector_failure` or `not_serving` |

`message` is bounded by `MAX_HEALTH_MESSAGE_BYTES` (512 UTF-8 bytes) and
rejected when it contains NUL. `ready()` debug-asserts the snapshot
invariant; `try_ready()` validates first and returns the structured violation
list, and the daemon serves a `failed` envelope rather than a `200` when a
cached snapshot fails validation.

## Test support

The `test_support` feature flag exposes builder fixtures:

| Builder | Produces |
|---------|----------|
| `LinuxSnapshotBuilder` | V1 Linux snapshot with iowait |
| `MacosSnapshotBuilder` | V1 macOS snapshot without iowait |
| `LinuxSnapshotV2Builder` | V2 Linux snapshot with optional drives/live telemetry |
| `MacosSnapshotV2Builder` | V2 macOS snapshot (load average, no iowait, no swap) with optional drives/live telemetry |
| `WindowsSnapshotV2Builder` | V2 Windows snapshot with commit/live telemetry |

## Fixture files

Located in `crates/gregg-protocol/tests/fixtures/`:
- `linux-v1.json`, `linux-v2.json`
- `macos-v1.json`, `macos-v2.json`
- `windows-v2.json`
- `live-metrics-v2.json`
- `health-ready-v1.json`, `health-warming-v1.json`, `health-collector-failure-v1.json`
- `health-ready-v2.json`

Fixtures deserialize, validate, and re-serialize byte-stably. Integration tests round-trip every fixture.

## Key constraints

- `#![forbid(unsafe_code)]` — no unsafe in this crate
- No runtime, HTTP, terminal, or platform dependencies
- Only `serde`, `serde_json`, and `thiserror` as dependencies
- Schema version is explicit; unknown versions are rejected, not ignored
