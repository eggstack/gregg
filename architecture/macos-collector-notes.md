# macOS collector diagnostic comparison notes

This document records expected differences between `greggd`'s macOS collector
output and the values shown by Activity Monitor, `top`, and `vm_stat`. These
differences are intentional design choices, not bugs.

## CPU

**Source:** Mach `host_statistics` with `HOST_CPU_LOAD_INFO` (cumulative ticks).

| Tool | What it shows | Relationship to greggd |
|------|--------------|----------------------|
| `top` | Per-process and aggregate CPU % from same Mach tick source | Very close; differs only by sampling interval alignment |
| Activity Monitor | "CPU Usage" bar, "User" and "System" columns | Aggregate should be close; per-process breakdown is not exposed |

greggd computes `delta(busy) / delta(total) * 100` across the interval between
two samples. The formula is identical to what `top` uses internally. Minor
differences (typically < 1%) arise from different sample timing and the fact
that `top` may include additional kernel accounting in `system` ticks.

**I/O wait:** macOS has no aggregate I/O-wait equivalent to Linux
`/proc/stat` `iowait`. greggd reports `cpu_iowait = false` and
`cpu.iowait_pct = None`. Activity Monitor shows "I/O Wait" in its CPU tab, but
this is a per-thread sampling heuristic, not an aggregate system counter. The
values are not comparable across tools and are intentionally omitted.

## Memory

**Source:** Mach `host_statistics64` with `HOST_VM_INFO64` (page counts) and
`hw.memsize` (physical total).

| Tool | Metric | greggd equivalent | Expected difference |
|------|--------|-------------------|-------------------|
| Activity Monitor | "Memory Used" | `memory.used_bytes` | greggd reports **less** used memory (see below) |
| Activity Monitor | "Memory Pressure" (color) | Not exposed | No equivalent; greggd does not model page-in/compression pressure |
| Activity Monitor | "App Memory", "Wired", "Compressed" | Not exposed | Diagnostic-only categories; not in version-1 protocol |
| `vm_stat` | Raw page counts | `memory.used_bytes`, `memory.total_bytes` | Same source data; greggd normalizes to bytes |

### Why greggd's memory differs from Activity Monitor

Activity Monitor's "Memory Used" sums **active + inactive + wired + compressed**
pages, which represents all memory that the kernel considers "in use" even if
some of it could be reclaimed.

greggd's version-1 normalization uses an **availability-oriented** definition:

```text
available_pages = free_count + inactive_count
available_bytes = available_pages × page_size
used_bytes = total_bytes − min(available_bytes, total_bytes)
```

This means:

- **Wired memory** is counted as "used" (it cannot be paged out).
- **Active memory** is counted as "used" (it is actively referenced).
- **Inactive memory** is counted as "available" (it can be reclaimed under
  memory pressure).
- **Free memory** is counted as "available" (it is already unused).

The result is a more conservative utilization figure that reflects how much
memory is genuinely available, not how much the kernel has touched. This is
suitable for a compact utilization bar and aligns with the "memory available"
semantics used by `free` on Linux.

Compressed memory is **not** treated as available because the compressor is
backing store; the pages are not free in the traditional sense. However, the
native swap interface (`vm.swapusage`) reports compressed pages as swap on
macOS 10.6+ when compression is active, so swap metrics may include
compressed-memory accounting.

### Speculative pages

`host_statistics64` exposes speculative page counts in some kernel versions.
greggd's version-1 normalization does **not** include speculative pages in
`available` or `used` because their accounting is kernel-internal and may
double-count pages already reflected in `free_count`. This is a deliberate
conservative choice; a future version may include them if validation shows they
are distinct.

## Swap

**Source:** sysctl `vm.swapusage` (`xswusage` struct).

| Tool | What it shows | Relationship to greggd |
|------|--------------|----------------------|
| `sysctl vm.swapusage` | Total, used, pagesize | Same source; greggd clamps used ≤ total |
| `top` | Swap: X/Y | Same sysctl source; values should match |
| Activity Monitor | Not directly shown | No comparison |

A host with no swap configured reports `total = 0, used = 0`.
greggd normalizes this to `usage_pct = 0.0`. Compressed memory is reported as
swap by the native interface on macOS 10.6+; greggd does not separately
decompose this.

## Load averages

**Source:** `getloadavg()` (same as `top`).

| Tool | What it shows | Relationship to greggd |
|------|--------------|----------------------|
| `top` | Load Avg: X, Y, Z | Same source; values should match exactly |
| Activity Monitor | Not directly shown | No comparison |

Load averages are not normalized by core count. A load of 8.0 on an 8-core
machine means all cores are fully utilized; on a 2-core machine it means 4×
overcommit. Core count is reported separately in `cpu.logical_cores`.

## Identity

**Source:** sysctl and `SystemVersion.plist`.

| Field | Source | Notes |
|-------|--------|-------|
| hostname | `kern.hostname` sysctl | Same as `sysctl kern.hostname` and `hostname` command |
| os_name | Hardcoded `"macos"` | Matches protocol constant |
| os_version | `SystemVersion.plist` ProductVersion | Same as `sw_vers -productVersion` |
| kernel_name | Hardcoded `"Darwin"` | Matches `uname -s` output |
| kernel_release | `kern.osrelease` sysctl | Same as `uname -r` output |
| architecture | `hw.machine` sysctl | Same as `uname -m` output; `x86_64` or `arm64` |

The product version from `SystemVersion.plist` is the marketing version (e.g.,
`14.5`). If the plist is unavailable (e.g., restricted container), the version
falls back to `"unknown"` rather than fabricating a value.

## CI validation

CI runs on both Intel (`macos-13`) and Apple Silicon (`macos-latest`). All
collector tests use `MockNativeQueries` for deterministic arithmetic; native FFI
tests run only on macOS runners and validate that real Mach and sysctl calls
succeed.

For manual validation on a developer machine, compare sampled values with:

```sh
# CPU / load
top -l 1 -n 0 | head -12

# Memory page counts
vm_stat

# Swap
sysctl vm.swapusage

# Identity
sysctl kern.hostname kern.osrelease hw.machine hw.logicalcpu hw.memsize
sw_vers -productVersion
```

Allow sampling tolerance (typically < 2% for CPU) and semantic differences in
memory categorization as documented above.
