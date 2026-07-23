# Phase 2: Linux metrics collector

## Objective

Implement a Linux-native collector for `greggd` that produces stable system identity and normalized metric samples without spawning external commands. The collector must support x86-64 and ARM64 Linux, including Ubuntu-class servers and Raspberry Pi-class devices, while remaining cheap enough for continuous one-second sampling.

This phase stops at native collection and normalization. It does not expose HTTP, manage services, or own the long-running sampling loop.

## Module structure

Create a collector boundary similar to:

```text
crates/greggd/src/collector/
├── mod.rs
└── linux/
    ├── mod.rs
    ├── cpu.rs
    ├── memory.rs
    ├── identity.rs
    ├── source.rs
    └── fixtures.rs
```

The common module should define a trait usable by both Linux and macOS collectors:

```rust
pub trait SystemCollector: Send {
    fn identity(&self) -> Result<SystemIdentity, CollectError>;
    fn sample(&mut self) -> Result<CollectedMetrics, CollectError>;
    fn capabilities(&self) -> MetricCapabilities;
}
```

`CollectedMetrics` may be daemon-internal if it maps losslessly to the protocol snapshot. Do not make the collector responsible for timestamps owned by the scheduler unless the clock boundary is explicit and testable.

## Data sources

Use native kernel/user-space interfaces:

- `/proc/stat` for aggregate CPU counters and I/O-wait counters.
- `/proc/loadavg` for one-, five-, and fifteen-minute load averages.
- `/proc/meminfo` for physical memory and swap totals/availability.
- `/etc/os-release` for distribution name/version.
- `uname` and hostname interfaces for kernel name/release and architecture.
- `std::thread::available_parallelism()` or a documented native source for logical core count, with sensible fallback behavior.

A focused crate such as `procfs` is acceptable if its dependency and parsing behavior are suitable. Keep an abstraction over file reads so tests can inject fixture directories and malformed content. Do not make tests overwrite or depend on the host `/proc` filesystem.

## CPU semantics

Read the aggregate `cpu` line from `/proc/stat` and retain the previous cumulative counters. Calculate percentages only from nonnegative deltas between samples.

Recommended normalized definitions:

```text
busy = user + nice + system + irq + softirq + steal
iowait = iowait
total = user + nice + system + idle + iowait + irq + softirq + steal
usage_pct = delta(busy) / delta(total) * 100
iowait_pct = delta(iowait) / delta(total) * 100
```

Do not add guest and guest-nice again because Linux accounts them within user/nice counters. Document this explicitly in code comments and tests.

The collector must handle:

- First sample: return a warming-up/no-delta state rather than reporting zero CPU.
- Zero total delta: retain a nonready sample or return a typed transient error; never divide by zero.
- Counter decrease/reset: treat the interval as invalid, reset the baseline, and wait for a subsequent sample.
- Extremely large counters: use sufficiently wide integer arithmetic and checked/saturating operations where appropriate.
- Floating-point conversion: ensure finite results and clamp only small numerical overshoot, not arbitrary malformed values.

Linux I/O wait is an accounting estimate, not exact blocked-I/O wall time. Keep the public label as I/O wait but document the kernel accounting limitation. `MetricCapabilities.cpu_iowait` is true for valid Linux collector snapshots.

## Load semantics

Parse the first three values from `/proc/loadavg` as `f32` or `f64` internally, then normalize to protocol precision. Retain raw load values; do not divide by core count in the API. The TUI may display the logical core count alongside load to aid interpretation.

Reject NaN, infinity, missing fields, and malformed values. A temporary load read failure should produce a collector error rather than reusing a stale load value inside an otherwise fresh sample unless the sampler later adopts an explicit partial-sample policy.

## Memory semantics

Use:

```text
memory_total = MemTotal
memory_available = MemAvailable
memory_used = memory_total - memory_available
```

If `MemAvailable` is absent on an old or unusual kernel, implement and document a conservative fallback based on available fields, such as free plus reclaimable cache components, while avoiding double counting. Record which path was used in internal diagnostics if useful, but do not expand the version-1 API merely for this detail.

Use:

```text
swap_total = SwapTotal
swap_used = SwapTotal - SwapFree
```

Normalize all `/proc/meminfo` kilobyte units to bytes with checked multiplication. Handle zero swap as a normal state with zero used bytes and zero percentage. Clamp available/free values that transiently exceed totals only if kernel counter races can explain the discrepancy; otherwise report a normalization error and test the policy.

## Identity semantics

Collect stable identity once during collector construction or first use:

- Configured display name, if supplied by daemon configuration.
- Hostname.
- Distribution pretty name or name/version from `os-release`.
- Kernel name and release.
- Architecture.
- Logical core count if treated as identity rather than dynamic metrics.

The configured daemon name should override the display name only; it must not replace the actual hostname field. Parse `os-release` without shelling out and support quoted/escaped values sufficiently for common distributions.

If `os-release` is missing, fall back to a truthful generic Linux identity rather than failing all metric collection. Kernel identity failure should be surfaced because it indicates a deeper platform problem.

## Error taxonomy

Define errors sufficiently specific for logs and readiness reporting:

- Source unavailable or permission denied.
- Parse failure with source category, not unbounded raw content.
- Counter baseline unavailable/warming up.
- Counter reset or invalid delta.
- Numeric overflow or invalid normalized value.
- Identity fallback warning versus fatal identity failure.

Avoid leaking full `/proc` contents in errors. Include enough context to identify the failing source.

## Fixture strategy

Create captured, reviewed fixtures under a dedicated test directory. Include at least:

1. Typical Ubuntu x86-64 sample pair.
2. Typical ARM64/Raspberry Pi-style sample pair.
3. Zero-swap host.
4. High-memory host with large counters.
5. Missing `MemAvailable` fallback case.
6. CPU counter reset/decrease.
7. Zero total CPU delta.
8. Malformed `/proc/stat`.
9. Malformed `/proc/loadavg`.
10. Malformed or overflowing `/proc/meminfo`.
11. Missing and minimally populated `os-release`.

Store paired CPU samples so expected percentages are calculated by hand in test comments. Avoid fixtures that encode sensitive real hostnames or environment details.

## Tests

Add pure unit tests for counter-delta math and normalization. Add source-level tests that read from fixture directories. Native smoke tests may read the running Linux host but must assert only broad invariants, not expected load percentages.

Property tests are useful for:

- Nondecreasing counters produce finite percentages.
- Busy plus idle accounting never produces a usage percentage outside the allowed range, absent input errors.
- Used memory and swap never exceed totals after accepted normalization.

Keep property-test dependency cost proportionate; a deterministic table-driven suite is acceptable if complete.

## Performance considerations

Read only the files required for each sample. Avoid per-sample heap churn where straightforward reuse is possible, but do not obscure correctness with premature micro-optimization. Identity and static capacities should be cached.

Collect a lightweight benchmark or measurement harness that can run repeated fixture samples and native samples. Its purpose is regression detection, not a crates.io benchmark promise.

## Acceptance criteria

Phase 2 is complete when:

1. `LinuxCollector` implements the shared collector contract behind `cfg(target_os = "linux")`.
2. No runtime metric path invokes external commands.
3. First-sample warming, valid delta, zero delta, and counter-reset behavior are deterministic and tested.
4. CPU busy and I/O-wait percentages match hand-calculated fixture expectations within documented tolerance.
5. Guest counters are not double counted.
6. Load averages are parsed and validated without core normalization.
7. Memory uses `MemAvailable` where present and has a tested, documented fallback.
8. Swap-zero behavior cannot produce NaN, infinity, or division errors.
9. Identity works with normal, missing, and minimal `os-release` inputs.
10. x86-64 and ARM64 fixture suites pass; native Linux CI passes smoke tests.
11. Repeated sampling shows no unbounded allocation/state growth.
12. The collector maps into a protocol-valid Linux snapshot with `cpu_iowait: true`.

## Handoff to phase 4

Expose a constructor that accepts a configured display name and either production data sources or an injectable source abstraction. Phase 4 must be able to own the cadence and clock while calling `sample()` cheaply and publishing only complete valid snapshots.
