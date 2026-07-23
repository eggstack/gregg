# Phase 3: macOS metrics collector

## Objective

Implement a native macOS collector for `greggd` supporting Intel and Apple Silicon without spawning `top`, `vm_stat`, `sysctl`, `sw_vers`, or other external commands at runtime. The collector must normalize Darwin/Mach metrics into the same protocol used by Linux while preserving semantic differences through capability metadata.

This phase includes the necessary safe FFI boundary and native validation. It does not implement the long-running sampler, HTTP server, or launchd lifecycle commands.

## Supported targets

Version-1 targets:

```text
x86_64-apple-darwin
aarch64-apple-darwin
```

Select and document a minimum supported macOS version compatible with the chosen Rust toolchain and APIs. macOS 12 or newer is a reasonable initial floor unless project requirements demand older systems.

## Module structure

Use a platform-isolated module:

```text
crates/greggd/src/collector/macos/
├── mod.rs
├── ffi.rs
├── cpu.rs
├── memory.rs
├── swap.rs
├── identity.rs
└── normalize.rs
```

All unsafe calls and C/Mach structure handling belong in `ffi.rs`. The rest of the collector consumes owned safe Rust records.

Required safety properties:

- Validate every Mach return status.
- Validate structure/count values returned by APIs.
- Initialize buffers correctly before foreign calls.
- Do not retain pointers into temporary foreign buffers.
- Convert C strings with explicit invalid-UTF-8 behavior.
- Check integer conversions and page-size multiplication.
- Document the safety invariant immediately above each unsafe block.

## CPU collection

Use Mach host CPU load statistics, typically `host_statistics` with `HOST_CPU_LOAD_INFO`, to obtain cumulative user, system, idle, and nice ticks. Preserve a previous sample and calculate interval utilization:

```text
busy = user + system + nice
total = user + system + nice + idle
usage_pct = delta(busy) / delta(total) * 100
```

Handle first sample, zero delta, counter decrease/reset, overflow, and nonfinite conversion consistently with the Linux collector.

macOS does not expose an aggregate CPU state equivalent to Linux `/proc/stat` `iowait`. The normalized output must therefore set:

```text
capabilities.cpu_iowait = false
cpu.iowait_pct = null
```

Never infer I/O wait from disk activity, blocked tasks, idle time, or Activity Monitor categories. Never report zero for an unsupported measurement.

Determine logical core count through a native API such as `sysctlbyname("hw.logicalcpu")`, with a documented fallback where appropriate. Validate nonzero output.

## Load averages

Use `getloadavg()` or a stable native sysctl mechanism to obtain one-, five-, and fifteen-minute load averages. Treat a partial return as an error. Reject nonfinite or negative values.

As on Linux, do not normalize load by core count in the API. Core count is transported separately and rendered beside load.

## Memory collection

Use Mach VM statistics, preferably `host_statistics64` with `HOST_VM_INFO64`, plus host page size and physical memory total from a native source such as `hw.memsize`.

Define and document version-1 normalization. A practical availability-oriented policy is:

```text
available_pages = free_pages + inactive_pages
available_bytes = available_pages * page_size
used_bytes = total_bytes - min(available_bytes, total_bytes)
```

Before finalizing this policy, compare with current Activity Monitor and `vm_stat` behavior on Intel and Apple Silicon and record why the chosen definition is useful for a compact cross-platform utilization bar. Do not claim exact equality with Activity Monitor’s “Memory Used” or memory-pressure model.

Consider whether speculative pages are already represented within free/inactive accounting for the chosen API version; avoid double counting. Treat compressed, wired, active, purgeable, and file-backed classifications as diagnostic validation inputs rather than expanding the version-1 protocol.

The implementation must handle:

- Very large page counts and page sizes with checked arithmetic.
- Available bytes transiently exceeding physical total.
- Failure to read physical memory total.
- Mach structure version/count differences.
- Simulator or unusual host behavior if encountered in CI.

## Swap collection

Use a native sysctl query for `vm.swapusage` or another stable Darwin interface yielding total and used swap. Wrap the concrete C structure in `ffi.rs` and return owned integer byte counts.

Normalize:

```text
swap_used = min(reported_used, reported_total)
swap_pct = 0 when total is zero, otherwise used / total * 100
```

A host with no configured swap must remain valid. Do not treat compressed memory as swap unless the native swap interface reports it as such.

## Identity collection

Collect:

- Configured display name.
- Actual hostname.
- Product name (`macOS`).
- Product version.
- Darwin kernel name and release.
- Architecture (`x86_64` or `arm64`).
- Logical core count.

Use native APIs. Product version may require a stable system property or plist-backed native source; do not shell out to `sw_vers`. If the exact marketing version is unavailable, return a truthful fallback rather than inventing one, and document the limitation.

Keep Darwin kernel release distinct from macOS product version. Do not serialize a single opaque combined label.

## FFI testability

Split foreign acquisition from normalization:

```rust
struct RawCpuTicks { ... }
struct RawVmStats { ... }
struct RawSwapUsage { ... }
struct RawIdentity { ... }
```

Pure Rust functions should convert these records into normalized values. This enables exhaustive tests on Linux CI for arithmetic modules if they are platform-independent, while native macOS CI verifies the actual calls.

Provide a trait or function indirection for native queries so tests can inject failures, short returned lengths, invalid C strings, oversized counters, and counter resets without relying on the host state.

## Native validation strategy

Use diagnostic commands only in developer/integration validation scripts, never in production code. Compare sampled values with:

- Activity Monitor for broad CPU/memory plausibility.
- `top` for CPU/load plausibility.
- `vm_stat` for page-count interpretation.
- `sysctl` for logical CPUs, physical memory, kernel identity, and swap.

Comparisons should allow sampling and semantic tolerance. Record expected differences, especially memory categorization.

Test on both Intel and Apple Silicon runners or physical systems. Cross-compilation alone is insufficient evidence for FFI correctness.

## Error taxonomy

Map native failures into collector errors with categories such as:

- Mach call failure, retaining numeric status for logs.
- Unexpected returned structure length/count.
- Sysctl lookup/query failure.
- Invalid C-string or unsupported encoding.
- Counter baseline unavailable/warming up.
- Counter reset/invalid delta.
- Numeric overflow or normalization violation.

Public readiness output should remain concise and must not expose unsafe memory contents or arbitrary native buffers.

## Performance constraints

Cache static identity and physical memory total. Per-sample calls should be limited to the APIs necessary for CPU, load, VM statistics, and swap. Avoid repeatedly allocating large buffers for fixed-size sysctl values.

Measure repeated native sample cost on Intel and Apple Silicon. Correctness takes priority over micro-optimization, but the implementation should not invoke expensive process enumeration or system-profiler APIs.

## Acceptance criteria

Phase 3 is complete when:

1. `MacOsCollector` implements the shared collector contract behind `cfg(target_os = "macos")`.
2. Runtime collection invokes no external commands.
3. Unsafe code is confined to a small FFI module with documented invariants and no escaping pointers.
4. CPU utilization is derived from successive Mach tick samples and covers warming, valid delta, zero delta, and reset cases.
5. I/O wait is always represented as unsupported/null with a matching capability flag.
6. Load averages are obtained natively and validated.
7. Memory normalization is documented, deterministic, overflow-safe, and tested against synthetic raw statistics.
8. Swap handles zero total and used-greater-than-total anomalies safely.
9. Product version, Darwin release, architecture, hostname, and logical cores remain distinct identity fields.
10. Native CI runs on both Intel and Apple Silicon and passes collector smoke tests.
11. Diagnostic comparison notes document expected differences from Activity Monitor and command-line tools.
12. The collector maps into a protocol-valid macOS snapshot and repeated sampling shows no unbounded state growth.

## Handoff to phase 4

Expose the same construction and sampling shape as the Linux collector. Platform selection should be compile-time and transparent to the sampler. Phase 4 should not know Mach or sysctl details and should interpret unsupported I/O wait only through normalized capabilities.
