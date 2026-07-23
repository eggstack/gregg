# Protocol schema

This document captures the version-1 wire contract implemented by
`gregg-protocol` and the compatibility rules that govern additive changes
within the version.

The authoritative description lives in the rustdoc on each public type and
constant in [`crates/gregg-protocol/src/lib.rs`](../crates/gregg-protocol/src/lib.rs).
This file is a higher-level summary intended for cross-platform contributors
and reviewers.

## Carried values

Every snapshot carries:

- `schema_version: u16` — schema major. Currently `1`. Any non-matching value
  is rejected by `StatusSnapshot::validate`.
- `observed_at_unix_ms: u64` — Unix epoch in milliseconds when the counters
  were sampled.
- `sample_interval_ms: u64` — sampling cadence used to derive percentages.
- `capabilities: MetricCapabilities` — per-metric support flags. A `false`
  flag means the metric is unsupported on this platform; servers must report
  `None` (null) for unsupported values rather than zero.
- `system: SystemIdentity` — name, hostname, OS name and version, kernel
  name and release, architecture. Fields are transported separately so the
  TUI can degrade by width priority.
- `cpu: CpuMetrics` — `logical_cores: u32`, `usage_pct: f32` (delta-derived,
  not instantaneous), `iowait_pct: Option<f32>`.
- `load: LoadAverage` — one-, five-, fifteen-minute averages as `f32`.
- `memory: MemoryMetrics` — `used_bytes: u64`, `total_bytes: u64`,
  `usage_pct: f32`.
- `swap: SwapMetrics` — same shape as memory; `usage_pct` is `0.0` when
  `total_bytes == 0`.

Percentages are reported in the closed interval `0.0..=100.0`. Values
outside that interval — and `NaN` / `±∞` — are rejected by
`StatusSnapshot::validate`. macOS has no Linux-equivalent aggregate CPU
I/O-wait accounting; it sets `capabilities.cpu_iowait = false` and
`cpu.iowait_pct = null`. The TUI renders this distinction rather than
treating it as zero.

## Health response

The daemon exposes a health endpoint that distinguishes three states:

- `Ready` — the daemon has a valid cached snapshot.
- `Warming` — the daemon is alive but the first counter delta is not yet
  available. No snapshot is included.
- `Failed` — the native collector reported an error. No snapshot is
  included. The response carries a coarse `HealthCategory` and a short
  human-readable message. Wire responses never embed filesystem paths,
  internal error chains, or platform-private structures.

## Validation

Validation is intentionally separate from serde deserialization. Adding
fields that serde does not know about must not change the strictness of
validation for fields that serde does know about, so callers can use
`serde_json::from_slice` and then call `validate()` explicitly.

`StatusSnapshot::validate()` returns `Ok(())` or `Err(Vec<ValidationViolation>)`.
Each violation carries a field path and a `ViolationKind`. The current kinds
are:

- `UnsupportedSchemaVersion { found: u16 }`
- `ZeroNotAllowed` (for `observed_at_unix_ms`, `sample_interval_ms`,
  `cpu.logical_cores`)
- `PercentageNotFinite`
- `PercentageOutOfRange`
- `UsedExceedsTotal` (memory or swap)
- `IowaitCapabilityMismatch`

## Compatibility policy

Within schema version 1:

- Unknown additive JSON fields are ignored by default.
- Required v1 fields remain required unless explicitly changed to optional
  under an additive compatibility decision.
- The client rejects unsupported schema majors per host rather than
  terminating the entire TUI.
- Capability flags control interpretation of optional metrics.

Breaking schema changes require a new schema major and explicit migration
handling.

## Compatibility fixtures

Canonical fixtures live at:

- `crates/gregg-protocol/tests/fixtures/linux-v1.json`
- `crates/gregg-protocol/tests/fixtures/macos-v1.json`
- `crates/gregg-protocol/tests/fixtures/health-ready-v1.json`
- `crates/gregg-protocol/tests/fixtures/health-warming-v1.json`
- `crates/gregg-protocol/tests/fixtures/health-collector-failure-v1.json`

These fixtures deserialise into `StatusSnapshot` / `HealthResponse`,
validate cleanly, and re-serialise byte-stable. The macOS fixture
demonstrates the `cpu_iowait: false` / `iowait_pct: null` distinction; the
Linux fixture demonstrates a measured non-zero I/O-wait.

## Collector contract

The shared `SystemCollector` trait lives in
`crates/greggd/src/collector/mod.rs`. It exposes three methods:
`identity()`, `sample()`, and `capabilities()`. `sample()` returns a
`CollectedMetrics` value.

`CollectedMetrics` is a daemon-internal normalised sample. It maps
losslessly to a `StatusSnapshot` once the daemon stamps
`observed_at_unix_ms` and `sample_interval_ms`. The collector never owns
a clock; the sampler (phase 4) does.

The Linux implementation lives behind `cfg(target_os = "linux")` and
reads procfs/sysfs only. No external commands are executed. The macOS
implementation arrives in phase 3.

For collector semantics and acceptance criteria, see
[`plans/002-linux-metrics-collector.md`](../plans/002-linux-metrics-collector.md).
