# Protocol schema

This document captures the versioned wire contract implemented by
`gregg-protocol` and the compatibility rules that govern additive changes
within each version.

The authoritative description lives in the rustdoc on each public type and
constant in [`crates/gregg-protocol/src/lib.rs`](../crates/gregg-protocol/src/lib.rs).
This file is a higher-level summary intended for cross-platform contributors
and reviewers.

## Schema versions

### Version 1

Version 1 is the original wire format for Linux and macOS daemons. It
carries required load averages and swap metrics, with a single capability
flag for CPU I/O wait.

### Version 2

Version 2 extends the protocol with explicit capability flags for load
average, swap, and memory commit. This allows the protocol to truthfully
represent Linux, macOS, and Windows metric differences without fabricating
unsupported values.

Key differences from v1:

- `load`, `swap`, and `commit` are `Option` fields with capability flags.
- `MetricCapabilitiesV2` has four flags: `cpu_iowait`, `load_average`,
  `swap`, and `memory_commit`.
- Windows can report `commit` without fabricating load or swap.
- `/v2/status` uses a flat `StatusPayloadV2` wrapper with optional `drives`.
  The wrapper preserves source compatibility for public Rust code that uses
  `StatusSnapshotV2` struct literals while keeping the JSON shape flat.
- Clients prefer v2 but fall back to v1 on 404 Not Found.

## Carried values

Every snapshot carries:

- `schema_version: u16` — schema major. Currently `1` or `2`. Any
  non-matching value is rejected by validation.
- `observed_at_unix_ms: u64` — Unix epoch in milliseconds when the counters
  were sampled.
- `sample_interval_ms: u64` — sampling cadence used to derive percentages;
  validation accepts values from 1 ms through 24 hours.
- `capabilities` — per-metric support flags. A `false` flag means the metric
  is unsupported on this platform; servers must report `None` (null) for
  unsupported values rather than zero.
- `system: SystemIdentity` — name, hostname, OS name and version, kernel
  name and release, architecture. Fields are transported separately so the
  TUI can degrade by width priority.
- `cpu` — `logical_cores: u32`, `usage_pct: f32` (delta-derived,
  not instantaneous), `iowait_pct: Option<f32>`.
- `memory` — `used_bytes: u64`, `total_bytes: u64`, `usage_pct: f32`.

V1 additional required fields:
- `load: LoadAverage` — one-, five-, fifteen-minute averages as `f32`.
- `swap: SwapMetrics` — same shape as memory; `usage_pct` is `0.0` when
  `total_bytes == 0`.

V2 additional optional fields:
- `load: Option<LoadAverage>` — `None` when `capabilities.load_average` is
  `false`.
- `swap: Option<SwapMetrics>` — `None` when `capabilities.swap` is `false`.
- `commit: Option<CommitMetrics>` — Windows commit charge; `None` when
  `capabilities.memory_commit` is `false`.

V2 status may also carry `drives: Option<Vec<DriveMetrics>>`. Each record has
an owned display `name`, `used_bytes`, `total_bytes`, and optional
`available_bytes`. `used_bytes` is derived from total filesystem free space;
`available_bytes`, when present, is the amount allocatable to the daemon or
service identity. Reservations or quotas can make used plus available less
than total. Older v2 daemons omit the field and clients use
`total_bytes - used_bytes` as a compatibility fallback. The list is
bounded to 32 entries and names to 512 UTF-8 bytes. A missing or null field
means unavailable/legacy; an empty list means successful enumeration with no
eligible filesystems. The daemon does not serialize aggregate totals or
percentages; the client derives them with checked arithmetic. A maximum-bound
valid payload is tested against the client’s existing 64 KiB response-body
cap, which remains unchanged.

Percentages are reported in the closed interval `0.0..=100.0`. Values
outside that interval — and `NaN` / `±∞` — are rejected by validation.
macOS has no Linux-equivalent aggregate CPU I/O-wait accounting; it sets
`capabilities.cpu_iowait = false` and `cpu.iowait_pct = null`. The TUI
renders this distinction rather than treating it as zero.

## Health response

The daemon exposes health endpoints that distinguish three states:

- `Ready` — the daemon has a valid cached snapshot.
- `Warming` — the daemon is alive but the first counter delta is not yet
  available. No snapshot is included.
- `Failed` — the native collector reported an error. No snapshot is
  included. The response carries a coarse `HealthCategory` and a short
  human-readable message. Wire responses never embed filesystem paths,
  internal error chains, or platform-private structures.

Version-specific health responses (`HealthResponse` for v1,
`HealthResponseV2` for v2) carry the corresponding schema version and
snapshot type. A v2 health response cannot accidentally embed a v1
snapshot.

## Validation

Validation is intentionally separate from serde deserialization. Adding
fields that serde does not know about must not change the strictness of
validation for fields that serde does know about, so callers can use
`serde_json::from_slice` and then call `validate()` explicitly.

### V1 validation

`StatusSnapshot::validate()` returns `Ok(())` or `Err(Vec<ValidationViolation>)`.
Each violation carries a field path and a `ViolationKind`. The current kinds
are:

- `UnsupportedSchemaVersion { found: u16 }`
- `ZeroNotAllowed` (for `observed_at_unix_ms`, `sample_interval_ms`,
  `cpu.logical_cores`)
- `SampleIntervalOutOfRange` — `sample_interval_ms` exceeds 24 hours
- `PercentageNotFinite`
- `PercentageOutOfRange`
- `LoadValueOutOfRange`
- `UsedExceedsTotal` (memory or swap)
- `IowaitCapabilityMismatch`

### V2 validation

`StatusSnapshotV2::validate()` returns `Ok(())` or
`Err(Vec<ValidationViolationV2>)`. In addition to v1 kinds, v2 adds:

- `AvailableExceedsTotal` — `available_bytes` exceeded `total_bytes`
- `LoadCapabilityMismatch` — load presence disagrees with capability
- `SwapCapabilityMismatch` — swap presence disagrees with capability
- `CommitCapabilityMismatch` — commit presence disagrees with capability
- `EmptyDriveName` — drive name is empty string
- `DriveNameTooLong` — drive name exceeds 512 UTF-8 bytes
- `TooManyDrives` — more than 32 drive entries

V2 total: 15 violation kinds (8 from v1 + 7 additional).

V2 validation rejects capability/value contradictions:
- `cpu_iowait == false` requires `iowait_pct == None`
- `load_average == false` requires `load == None`
- `swap == false` requires `swap == None`
- `memory_commit == false` requires `commit == None`

## Compatibility policy

Within each schema version:

- Unknown additive JSON fields are ignored by default.
- Required v1 fields remain required unless explicitly changed to optional
  under an additive compatibility decision.
- The client rejects unsupported schema majors per host rather than
  terminating the entire TUI.
- Capability flags control interpretation of optional metrics.
- V1 and v2 types coexist in the same release without breaking downstream
  source compatibility.

Breaking schema changes require a new schema major and explicit migration
handling.

## Compatibility fixtures

Canonical fixtures live at:

### V1
- `crates/gregg-protocol/tests/fixtures/linux-v1.json`
- `crates/gregg-protocol/tests/fixtures/macos-v1.json`
- `crates/gregg-protocol/tests/fixtures/health-ready-v1.json`
- `crates/gregg-protocol/tests/fixtures/health-warming-v1.json`
- `crates/gregg-protocol/tests/fixtures/health-collector-failure-v1.json`

### V2
- `crates/gregg-protocol/tests/fixtures/linux-v2.json`
- `crates/gregg-protocol/tests/fixtures/macos-v2.json`
- `crates/gregg-protocol/tests/fixtures/windows-v2.json`
- `crates/gregg-protocol/tests/fixtures/health-ready-v2.json`

These fixtures deserialise into the corresponding types, validate cleanly,
and re-serialise byte-stable. The v2 Windows fixture demonstrates the
`memory_commit: true` / `commit: Some(...)` pattern with no load or swap.
The v2 macOS fixture demonstrates `load_average: true` and `swap: false`,
with load populated and swap absent per the native collector capabilities.

## Collector contract

The shared `SystemCollector` trait lives in
`crates/greggd/src/collector/mod.rs`. It exposes five methods:
`identity()`, `sample()`, `capabilities()`, `capabilities_v2()`, and
`supports_v1_snapshot()`. `sample()` returns a `CollectedMetrics` value.

`CollectedMetrics` is a daemon-internal normalised sample. It maps
losslessly to both `StatusSnapshot` (v1) and `StatusSnapshotV2` (v2) once
the daemon stamps `observed_at_unix_ms` and `sample_interval_ms`. One
native sample produces both wire representations without duplicate
collection.

The Linux implementation lives behind `cfg(target_os = "linux")` and
reads procfs/sysfs only. No external commands are executed. The macOS
implementation lives behind `cfg(target_os = "macos")` and uses Mach
host statistics and sysctl APIs through a contained FFI module. The
Windows implementation lives behind `cfg(target_os = "windows")` and
uses native Windows APIs (`GetSystemTimes`, `GlobalMemoryStatusEx`,
`GetPerformanceInfo`, `GetComputerNameExW`, `RtlGetVersion`) through
raw `extern "system"` FFI. No external commands are executed.

For collector semantics and acceptance criteria, see the
collector phase plans under [`plans/`](../plans/).

## Sampler and HTTP server

The sampler owns cadence and clock. It periodically calls the collector,
computes deltas, and produces immutable v1 and v2 status values, including
the flat v2 `StatusPayloadV2` drive wrapper, that are cached by the HTTP server. The HTTP server serves these cached
snapshots and never triggers metric collection. This separation ensures
collection cadence is decoupled from request handling.

## Client polling

The client implements v2-first/v1-fallback negotiation:

1. Request `/v2/status`.
2. If v2 succeeds (200 OK), validate and use the v2 snapshot.
3. If v2 returns 404 Not Found, fall back to `/v1/status`; this is the only fallback condition.
4. If v2 returns malformed, invalid, or wrong-schema data, report the error without
   falling back.
5. If v2 returns warming/failure (503), represent that state without
   falling back.

On Windows, step 3 is not reachable because `/v2/status` is always
available. The daemon serves 503 for v1 when the collector does not
produce a v1 snapshot (the Windows path). Clients always use v2 on
Windows.

Both v1 and v2 snapshots are normalized into an internal
`NormalizedSnapshot` type that the state reducer and TUI consume. This
eliminates version-branching throughout the rendering code.
Normalized drive records preserve source order. `aggregate_drives` owns the
used/total/available/percentage calculation and returns no aggregate for an
empty, invalid, or overflowing list.

## Platform-specific endpoint behavior

The daemon always exposes all five routes (`/`, `/v1/status`,
`/v2/status`, `/healthz`, `/v2/healthz`). On platforms where a
truthful v1 snapshot cannot be produced, the v1 endpoint returns
`503 Service Unavailable` with a v1 health response:

- **Windows**: v1 is unsupported because load, swap, and CPU I/O-wait
  are absent and cannot be represented as non-optional zero values.
  `/v1/status`, `/`, and `/healthz` return 503 with a failed
  `HealthResponse` in the `NotServing` category and a stable unavailable
  message. Clients should prefer `/v2/status` on Windows.
- **Linux/macOS**: Both v1 and v2 return 200 OK when ready.

The client polling logic (`v2-first, v1-fallback on 404`) handles
mixed-version fleets correctly: a Linux daemon serves v2 (200), and
a Windows daemon serves v2 (200) while v1 returns 503.
