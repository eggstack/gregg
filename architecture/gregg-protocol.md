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
| `lib` | `src/lib.rs` | Root, re-exports, `SCHEMA_VERSION_V1 = 1` (`SCHEMA_VERSION_V2` lives in `v2.rs`), `MAX_IDENTITY_FIELD_BYTES = 512`, `MAX_HEALTH_MESSAGE_BYTES = 512`, `MAX_SAMPLE_INTERVAL_MS = 86_400_000`, `#![forbid(unsafe_code)]` |
| `snapshot` | `src/snapshot.rs` | V1 wire types: `StatusSnapshot`, `CpuMetrics`, `LoadAverage`, `MemoryMetrics`, `SwapMetrics`, `SystemIdentity`, `MetricCapabilities`; public entry is `StatusSnapshot::validate()` |
| `v2` | `src/v2.rs` | V2 wire types: `StatusSnapshotV2`, `StatusPayloadV2`, `CpuMetricsV2`, `SwapMetrics`, `CommitMetrics`, `MetricCapabilitiesV2`, `DriveMetrics`, `DiskIoMetrics`, `DiskIoPayload`, `NetworkInterfaceMetrics`, `NetworkPayload`, `HealthResponseV2`; constants `SCHEMA_VERSION_V2`, `MAX_DRIVE_ENTRIES`, `MAX_DRIVE_NAME_BYTES`, `MAX_DISK_IO_ENTRIES`, `MAX_NETWORK_INTERFACE_ENTRIES`, `MAX_LIVE_METRIC_ID_BYTES`, `MAX_LIVE_METRIC_NAME_BYTES`, `MAX_RATE_BYTES_PER_SEC`, `MAX_CAPACITY_BITS_PER_SEC`, `MAX_CPU_FREQUENCY_HZ` |
| `scheduler` | `src/scheduler.rs` | Scheduler-observability wire types served on the additive read-only `/v2/scheduler` and `/v2/scheduler/history` routes: `SchedulerSummaryV2`, `SchedulerHistoryV2`, `SchedulerJobV2`, `SchedulerJobHistoryV2`, `SchedulerRunSummaryV2`, `SchedulerRunRecordV2`, `SchedulerJobStateV2`, `SchedulerOutcomeV2`, `SchedulerLoadGateV2`, `SchedulerOutputV2`, `SchedulerEpochV2`; frozen constants `MAX_SCHEDULER_JOBS`, `DEFAULT_SCHEDULER_HISTORY_LIMIT`, `MAX_SCHEDULER_HISTORY_LIMIT`, `MAX_SCHEDULER_OUTPUT_BYTES`, `MAX_SCHEDULER_OUTPUT_TEXT_BYTES`, `MAX_SCHEDULER_HISTORY_BODY_BYTES`, `MAX_SCHEDULER_SUMMARY_BODY_BYTES`; helpers `json_escaped_len`, `truncate_to_escaped_budget`, `output_text_from_bytes` |
| `validate_scheduler` | `src/validate_scheduler.rs` | Scheduler document validation: violation kinds for schema version, job/record cardinality, field lengths, non-monotonic sequence, duplicate job names, implausible timestamps, unknown load windows, load-decision/state contradictions, and child status on non-child outcomes; `history_body_exceeds_budget` |
| `validate` | `src/validate.rs` | V1 validation: 9 violation kinds (`validate()` is `pub(crate)`; callers use `StatusSnapshot::validate()`) |
| `validate_v2` | `src/validate_v2.rs` | V2 validation: base and live-metrics violation kinds, capability/value consistency; re-exports `validate_v2()` and `validate_payload_v2()` |
| `health` | `src/health.rs` | `HealthResponse` (V1-only) plus shared `ReadinessState` / `HealthCategory` (`Warming`, `CollectorFailure`, `NotServing`) also used by V2 |
| `test_support` | `src/test_support.rs` | Feature-gated builder fixtures for tests |

## Wire format

All payloads are JSON with `snake_case` field names. The v1 status endpoint
returns `StatusSnapshot` directly. The v2 status endpoint returns
`StatusPayloadV2` which flattens the snapshot and adds optional drive capacity,
CPU-frequency, disk-I/O, and network telemetry fields. New telemetry is
additive: old v2 payloads omit it and old clients ignore it.

Scheduler observability is served on its own two routes rather than inside
`StatusPayloadV2`, so an ordinary metrics poll never carries command output.
The client polls `/v2/scheduler` at the metrics cadence and fetches
`/v2/scheduler/history` only when the summary's `history_revision` changes. The
full wire contract, resource constants, and security boundary are in
`architecture/protocol.md`.

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
| `ZeroNotAllowed` | Timestamps, `logical_cores`, or `memory`/`swap.total_bytes == 0` with nonzero used/usage |
| `SampleIntervalOutOfRange` | `sample_interval_ms` exceeds 24-hour protocol maximum |
| `PercentageNotFinite` | NaN or infinity in percentage fields |
| `PercentageOutOfRange` | Percentage outside `0.0..=100.0` |
| `LoadValueOutOfRange` | Load average non-finite or negative |
| `UsedExceedsTotal` | `used_bytes > total_bytes` |
| `IowaitCapabilityMismatch` | iowait presence disagrees with capability |
| `InvalidIdentityField` | Identity string empty, whitespace-only, NUL-padded, or over 512 UTF-8 bytes |

### V2 additional violation kinds

| Kind | What it catches |
|------|----------------|
| `AvailableExceedsTotal` | `available_bytes > total_bytes` |
| `LoadCapabilityMismatch` | load presence disagrees with `load_average` capability |
| `SwapCapabilityMismatch` | swap presence disagrees with `swap` capability |
| `CommitCapabilityMismatch` | commit presence disagrees with `memory_commit` capability |
| `EmptyDriveName` | Drive name is empty, whitespace-only, or NUL-padded |
| `DriveNameTooLong` | Drive name > 512 UTF-8 bytes |
| `TooManyDrives` | More than 32 drive entries |

Live telemetry adds bounded validation for positive and plausible CPU
frequency/capacities, disk-I/O and network collection sizes, non-empty
non-blank bounded IDs and names (disk/net IDs and names reject NUL and
whitespace-only values; drive names reject blank and NUL-padded labels),
unique IDs within each detail list, plausible aggregate throughput
(`MAX_RATE_BYTES_PER_SEC` = 1 TiB/s; above it is rejected as a buggy daemon,
never silently clamped), plausible capacity
(`MAX_CAPACITY_BITS_PER_SEC` = 2^48 bps) and CPU frequency
(`MAX_CPU_FREQUENCY_HZ` = 2^34 Hz), and the rule that loopback cannot
be an aggregate network-capacity member. `Some(0)` capacities are rejected;
missing capacities remain valid and mean that utilization cannot be derived.
The daemon-provided disk/network aggregates are intentionally not checked
against detail-record sums because their accounting sets may differ.

Duplicate detection uses hash sets rather than pairwise scans, so an
attacker-sized collection is validated in linear time; per-entry violations
are still reported after the `TooMany*` bound fires. A disk-I/O `drive_name`
association must match a payload drive whenever `drives` is `Some` (an empty
`Some([])` makes any association dangling); only `drives: None`
(unavailable/legacy) skips the check. A drive with `total_bytes == 0` is a
legitimate empty or placeholder volume and is rejected only when it also
claims non-zero `used_bytes`/`available_bytes`, matching memory/swap/commit.

The base v2 contract has 16 violation kinds (9 from v1 + 7 additional);
live-metrics validation adds 20 structured kinds (`CpuFrequencyZero`,
`CpuFrequencyExceedsMaximum`, `TooManyDiskIoDevices`, `DiskIoIdInvalid`/`TooLong`,
`DiskIoNameInvalid`/`TooLong`,
`DuplicateDiskIoId`, `DuplicateDriveName`, `UnknownDriveAssociation`,
`TooManyNetworkInterfaces`, `NetworkInterfaceIdInvalid`/`TooLong`,
`NetworkInterfaceNameInvalid`/`TooLong`, `DuplicateNetworkInterfaceId`,
`ZeroCapacity`, `CapacityExceedsMaximum`, `LoopbackAggregateMember`,
`RateExceedsMaximum` (aggregates
and per-device/per-interface rates), plus
identity/collection bounds), for 36 `ViolationKindV2` variants total.

## Health responses

Three states (non-ready responses carry their machine-readable category in
both v1 and v2; ready responses omit the category and include the snapshot;
`HealthCategory` is `Warming` / `CollectorFailure` / `NotServing`):
- **Ready** — daemon has a valid cached snapshot; includes the snapshot
- **Warming** — daemon alive but first counter delta not yet available
- **Failed** — collector error; carries category + message (no paths/chains)

Non-ready health responses must include their machine-readable category in both
v1 and v2 (`Warming` included); ready responses omit the category and include the snapshot.

The state/category pairing is a total allowlist, enforced on deserialize, so a
wire payload can never be self-contradictory:

| State | Allowed category |
|-------|------------------|
| `ready` | none (and no `message`) |
| `warming` | `warming` |
| `failed` | `collector_failure` or `not_serving` |

`message` is bounded by `MAX_HEALTH_MESSAGE_BYTES` (512 UTF-8 bytes) and
rejected when it contains NUL, matching the identity-field bounds.

Windows v2-only publication returns v1 `NotServing` health with HTTP 503;
v2 status and health remain independently ready after a valid sample.

Both v1 (`HealthResponse`) and v2 (`HealthResponseV2`) have constructors for
each state: `ready()`, `try_ready()`, `warming()`, `warming_with_message()`,
`try_warming_with_message()`, `failed()`, `try_failed()`.
`ready()` asserts the snapshot invariant in debug builds; `try_ready()`
validates first and returns the structured violation list, and the daemon
serves a `failed` envelope instead of a `200` when a cached snapshot fails
validation. `StatusPayloadV2` also has its own `validate()` method, which
validates the optional live telemetry in addition to the base snapshot and
drives.

## Test support

The `test_support` feature flag exposes builder fixtures and shared identity
defaults:

| Builder | Produces |
|---------|----------|
| `LinuxSnapshotBuilder` | V1 Linux snapshot with iowait |
| `MacosSnapshotBuilder` | V1 macOS snapshot without iowait |
| `LinuxSnapshotV2Builder` | V2 Linux snapshot with optional drives/live telemetry and `build_payload()` |
| `MacosSnapshotV2Builder` | V2 macOS snapshot test fixture (load average, no iowait; test-only swap-absent edge — native `gregg-host` macOS and `macos-v2.json` report `swap: true` from `vm.swapusage`) with optional drives/live telemetry and `build_payload()` |
| `WindowsSnapshotV2Builder` | V2 Windows snapshot with commit, optional live telemetry, and `build_payload()` |

`IdentityFixture` provides `linux()`, `macos()`, and `windows()` const
constructors for shared identity defaults across all builders.

All builders call `validate()` on `build()` (and `validate()` on
`build_payload()`) and panic if the fixture is invalid. This ensures tests
always start from valid baselines.

**Design note:** `DriveMetrics.available_bytes` is `Option<u64>` — callers
cannot assume it is always present. A zero `drive.total_bytes` is accepted for
an entirely empty or placeholder volume; validation rejects it only when the
record also claims non-zero `used_bytes`/`available_bytes`.

## Fixture files

Located in `tests/fixtures/`:
- `linux-v1.json`, `linux-v2.json`
- `live-metrics-v2.json`
- `macos-v1.json`, `macos-v2.json`
- `windows-v2.json`
- `health-ready-v1.json`, `health-warming-v1.json`, `health-collector-failure-v1.json`
- `health-ready-v2.json`

Fixtures deserialize, validate, and re-serialize value-stable (key-order-independent
JSON equality). Integration tests round-trip every fixture.

## Key constraints

- `#![forbid(unsafe_code)]` — no unsafe in this crate
- No runtime, HTTP, terminal, or platform dependencies
- Only `serde`, `serde_json`, and `thiserror` as dependencies
- Schema version is explicit; unknown versions are rejected, not ignored
