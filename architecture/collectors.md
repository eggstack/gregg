# Platform collectors deep dive

Each platform collector implements the `SystemCollector` trait and reads only
native kernel interfaces. No external commands are executed for metric collection.

**Source:** `crates/greggd/src/collector/`

## Shared contract

### SystemCollector trait

```rust
pub trait SystemCollector: Send {
    fn identity(&self) -> Result<SystemIdentity, CollectError>;
    fn sample(&mut self) -> Result<CollectedMetrics, CollectError>;
    fn capabilities(&self) -> MetricCapabilities;      // v1
    fn capabilities_v2(&self) -> MetricCapabilitiesV2; // v2 (all three platforms override explicitly; Linux/macOS mirror the default)
    fn supports_v1_snapshot(&self) -> bool;             // default true, false on Windows
}
```

### CollectedMetrics

Daemon-internal normalized sample. One native `sample()` returns
`CollectedMetrics`; a separate `into_snapshot_pair()` conversion yields both
v1 `StatusSnapshot` and v2 `StatusPayloadV2` without duplicate collection.
All collector byte-ratio percentages use the shared
`collector::clamped_usage_pct` helper so v1 and v2 platform paths have the
same zero, clamp, and non-finite behavior.

Optional live telemetry uses the shared collector/rate.rs baseline helper.
Cumulative two-direction counters are keyed by native identity and divided by
actual monotonic elapsed time. First observations, counter decreases, zero or
backward elapsed time, and identity disappearance/reappearance all establish a
fresh baseline. Rates never use the nominal sampler interval.

### CollectError taxonomy

| Kind | Meaning |
|------|---------|
| `Warming` | First sample; no delta available yet |
| `SourceUnavailable` | Kernel interface missing or unreadable (also used for unreadable identity fields) |
| `Parse` | Content present but unparseable |
| `CounterReset` | Kernel counter wrapped, decreased, or produced a zero delta (identical counters / suspend) since the last sample, invalidating the delta |
| `Numeric` | Arithmetic error (division by zero, overflow) |
| `IdentityFallback` | Reserved; currently unconstructed — identity failures surface as `SourceUnavailable`/`Parse` and sampling fails without publishing a fabricated identity |

### Common patterns

1. **Construct eagerly** — identity and static fields read in `new()`
2. **First `sample()` returns `Warming`** — CPU percentages require two readings
3. **Subsequent `sample()` returns `Ok(CollectedMetrics)`** — unless counter reset
4. **One native sample → both v1 and v2 wire representations**

### Drive normalization and refresh isolation

Drive enumeration is optional and never runs in the core `sample()` call. Each
platform collector owns at most one standard-thread refresh worker with an
immediate first request and a 30-second cadence. Core samples poll completed
results without waiting, retain the last successful list through failures, and
use `None` until the first result. A worker is detached during drop so a native
filesystem call cannot extend critical daemon shutdown.

### Drive normalization

`collector/drives.rs` provides shared normalization (46 `test_fixtures/` files;
39 tests in `linux/tests.rs` plus source/drive helpers):
- Validate candidates (non-empty identity/name, name ≤512 bytes, positive total, `free ≤ total`, `available ≤ total`)
- Deduplicate by identity (`sort_by(identity,name)` → `dedup_by(identity)`, keeping the smallest name)

Capacity candidates retain total filesystem free space and caller-available
space separately. Linux and macOS use `f_bfree`/`f_bavail`; Windows retains
both free values from `GetDiskFreeSpaceExW`. Used space is `total − total_free`
(not available), so reservations or quotas may make used plus available less than total.
- Sort and truncate to `MAX_DRIVE_ENTRIES` (32)

---

## Linux collector

**Source:** `crates/greggd/src/collector/linux/`

| Module | File | Purpose |
|--------|------|---------|
| `mod` | `mod.rs` | `LinuxCollector` struct, `SystemCollector` impl |
| `source` | `source.rs` | `FileSource` trait, `HostSource` (prod inner), `ProcSource` wrapper, `MemorySource` (test) |
| `cpu` | `cpu.rs` | `/proc/stat` parsing, delta percentages |
| `memory` | `memory.rs` | `/proc/meminfo` parsing, memory + swap |
| `identity` | `identity.rs` | hostname, kernel, `/etc/os-release` |
| `drives` | `drives.rs` | `/proc/self/mountinfo` + `statvfs` |
| `fixtures` | `fixtures.rs` | Test fixture loader |
| `tests` | `tests.rs` | `#[cfg(test)]` fixture-driven collector tests |

### CPU (`/proc/stat`)

Parses cumulative ticks: `user`, `nice`, `system`, `idle`, `iowait`, `irq`,
`softirq`, `steal`. Computes delta percentages between two samples:

```
busy = user + nice + system + irq + softirq + steal
total = sum(all)
usage_pct = delta(busy) / delta(total) * 100
```

Guest counters are excluded from both busy and total to avoid double-counting.

### Memory (`/proc/meminfo`)

Prefers `MemAvailable` (kernel-computed). Falls back to
`MemFree + Buffers + Cached + SReclaimable` on older kernels.

```
used = total - available
usage_pct = used / total * 100
```

Swap uses same formula with `SwapTotal` and `SwapFree`.

### Identity

- hostname: `/proc/sys/kernel/hostname` (empty → `SourceUnavailable`; no `gethostname()` fallback)
- kernel: `/proc/sys/kernel/osrelease`
- architecture: `/proc/sys/kernel/arch`, falling back to the `machine` field in `/proc/cpuinfo`
- OS: `/etc/os-release` (handles quoted/escaped values)
- All identifiers clipped to 128 bytes (protocol cap is 512)

### Drives

- Parses `/proc/self/mountinfo` on the optional refresh worker
- Excludes pseudo, network, `autofs`, and generic FUSE filesystem types
- Ranks candidates so a record with `mount_point == "/"` or `root == "/"` wins over other mounts
- Uses `statvfs` for capacity (the contained `unsafe` FFI lives in
  `source.rs`, with a documented safety comment)
- Handles octal escapes in mount paths

### Capabilities

```rust
MetricCapabilities { cpu_iowait: true }
MetricCapabilitiesV2 { cpu_iowait: true, load_average: true, swap: true, memory_commit: false }
```

### Tests

40+ unit/property tests using fixture files from `test_fixtures/`:
- Ubuntu x86_64, arm64, container, high-memory, zero-swap
- Malformed inputs, CPU hotplug, suspend/resume
- Counter reset, swap changes, large uptime

---

### Linux live telemetry

CPU frequency reads CPUFreq policy files with hardware-current preference and
scaling-current fallback, converts kHz to Hz with checked arithmetic, and
weights valid policies by represented logical CPUs. CPUFreq CPU-list values are
accepted in the kernel's range, comma-separated, and whitespace-separated
forms. Disk rates use cumulative
sector fields from top-level /sys/block/*/stat with the kernel 512-byte unit;
loop, RAM, zram, and layered devices with slaves are excluded from the
accounting set. Network counters use /proc/net/dev and sysfs speed, state,
flags, and master membership. Slaves are not added to their master, down links
do not add capacity, and loopback is detail-only for capacity.

## macOS collector

**Source:** `crates/greggd/src/collector/macos/`

| Module | File | Purpose |
|--------|------|---------|
| `mod` | `mod.rs` | `MacOsCollector` struct, `SystemCollector` impl |
| `ffi` | `ffi.rs` | Mach FFI, sysctl, RAII HostPort, `MacNativeQueries` trait |
| `cpu` | `cpu.rs` | Mach CPU tick deltas |
| `memory` | `memory.rs` | VM page counts → memory metrics |
| `swap` | `swap.rs` | sysctl `vm.swapusage` |
| `identity` | `identity.rs` | sysctl + SystemVersion.plist |
| `normalize` | `normalize.rs` | `percent()`, `clip_identifier()` |

### FFI seam

`MacNativeQueries` trait is the test seam. Production implementation
`FfiNativeQueries` wraps:

- `mach_host_self()` — RAII `HostPort` wrapper
- `host_statistics()` — `HOST_CPU_LOAD_INFO` (CPU ticks)
- `host_statistics64()` — `HOST_VM_INFO64` (VM page counts)
- `host_page_size()` — page size
- `sysctlbyname()` — kernel parameters
- `getloadavg()` — load averages
- `libc::getmntinfo` with `libc::statfs` — mounted filesystems (libc owns the
  Darwin ABI and selects the architecture-correct `INODE64` symbol; Gregg keeps
  no private `StatFs` layout)
- `SystemVersion.plist` — marketing version

`MockNativeQueries` provides `auto_increment_cpu` for successive-sample testing.

### CPU

Mach `HOST_CPU_LOAD_INFO` ticks: `user`, `system`, `idle`, `nice`.

```
busy = user + system + nice
total = sum(all)
usage_pct = delta(busy) / delta(total) * 100
```

No I/O wait — macOS has no aggregate equivalent. Activity Monitor's per-thread
heuristic is not comparable.

### Memory

Availability-oriented definition (matches Linux `free` semantics):

```
available = (free_count + inactive_count) * page_size
used = total - min(available, total)
```

This reports **less** used memory than Activity Monitor because inactive memory
is counted as available. Compressed memory is not treated as available.

### Swap

From `sysctl vm.swapusage`. Clamps used ≤ total. A host with no swap reports
`total = 0, used = 0, usage_pct = 0.0`. Compressed pages are reported as swap
on macOS 10.6+.

### Load averages

From `getloadavg()` — same source as `top`. Values match exactly.

### Identity

- hostname: `kern.hostname` sysctl
- os_version: `SystemVersion.plist` ProductVersion
- kernel_release: `kern.osrelease` sysctl
- architecture: `hw.machine` sysctl (`x86_64` or `arm64`)

### Drives

From `libc::getmntinfo()` with `libc::statfs`. Requires `MNT_LOCAL` (implicit
network exclusion) plus `MNT_DONTBROWSE == 0`, non-empty mount,
`fstype != devfs/autofs`, positive block size, and
`total > 0 && free ≤ total && avail ≤ total`; identity is `fsid.0:fsid.1`
read from the opaque `fsid_t` value. APFS container free space is shared, not
unique per volume.

### Capabilities

```rust
MetricCapabilities { cpu_iowait: false }
MetricCapabilitiesV2 { cpu_iowait: false, load_average: true, swap: true, memory_commit: false }
```

Swap comes from `vm.swapusage`, so the explicit v2 override reports
`swap: true` (meaningful native swap accounting); Linux/macOS mirror the
`capabilities_v2()` default values with explicit overrides, only Windows differs.

### Tests

- Mock-based tests using `MockNativeQueries` for deterministic arithmetic
- Native FFI smoke tests (macOS-only): CPU, VM, swap, load, identity, drives
- Sleep/wake transitions, recovery, unbounded growth

---

Live disk records use IOKit block-storage statistics and stable registry-entry
identities. Network records prefer `NET_RT_IFLIST2` / `if_msghdr2` 64-bit
counters and `ifi_baudrate`, with a correctly typed `getifaddrs` / `if_data`
32-bit fallback for older or unsupported hosts; legacy counter wraps
re-baseline through the shared rate helper without spikes. Optional
drive/network/disk-I/O source failures log once per availability transition
with family and error context instead of warning per sample. Current CPU
frequency remains unavailable because no supported public unprivileged source
was found.

## Windows collector

**Source:** `crates/greggd/src/collector/windows/`

| Module | File | Purpose |
|--------|------|---------|
| `mod` | `mod.rs` | `WindowsCollector` struct, `SystemCollector` impl |
| `source` | `source.rs` | `WindowsSource` trait, `NativeWindowsSource` (prod), `Mock` |
| `cpu` | `cpu.rs` | `GetSystemTimes` delta percentages |
| `memory` | `memory.rs` | `GlobalMemoryStatusEx` |
| `commit` | `commit.rs` | `GetPerformanceInfo` (commit charge) |
| `identity` | `identity.rs` | `GetComputerNameExW`, `RtlGetVersion` |

### FFI seam

`WindowsSource` trait is the test seam. Production implementation
`NativeWindowsSource` wraps:

- `GetSystemTimes` — CPU kernel/idle/user times
- `GlobalMemoryStatusEx` — physical memory
- `GetPerformanceInfo` — commit charge
- `GetActiveProcessorCount` / `GetActiveProcessorGroupCount` — topology
- `GetComputerNameExW` — hostname
- `RtlGetVersion` — OS version
- `GetLogicalDriveStringsW`, `GetDriveTypeW`, `GetDiskFreeSpaceExW` — drives

`MockWindowsSource` provides `auto_increment_cpu` for deterministic testing.

### CPU

`GetSystemTimes` returns cumulative `idle`, `kernel` (includes idle), `user`.

```
total = kernel + user
busy = total - idle
usage_pct = delta(busy) / delta(total) * 100
```

Note: Windows kernel time includes idle time, unlike Linux.

### Memory

From `GlobalMemoryStatusEx`:

```
used = total - available
usage_pct = used / total * 100
```

### Commit

From `GetPerformanceInfo`. Commit charge is distinct from swap:

```
used = commit_total_pages * page_size
limit = commit_limit_pages * page_size
usage_pct = used / limit * 100
```

A transient commit charge above the commit limit (pagefile resize windows,
kernel over-commit before expansion) is clamped to the limit for that sample
(`usage_pct` saturates at 100 %) instead of failing the entire collection
cycle; this also preserves the wire invariant that committed bytes never
exceed the reported limit.

### Identity

- hostname: `GetComputerNameExW(ComputerNameDnsHostname)`
- name: configured display name when supplied by daemon startup; otherwise the
  native hostname
- os_name: hardcoded `"windows"`
- os_version: `RtlGetVersion`
- kernel_name: hardcoded `"Windows NT"`
- architecture: compile target (`x86_64`/`aarch64`); processor topology only guards multi-group / >64-processor configurations

The successful `GetComputerNameExW` call reports the number of UTF-16 code
units written. The native source truncates its allocated buffer to that length
before decoding, so API padding cannot become a NUL in the published hostname.
Empty hostnames are rejected (returns error).

### Drives

From `GetLogicalDriveStringsW` + `GetDiskFreeSpaceExW`. Fixed and removable drives (`DRIVE_FIXED` and `DRIVE_REMOVABLE`) with positive capacity are candidates.

### Capabilities

```rust
MetricCapabilities { cpu_iowait: false }
MetricCapabilitiesV2 { cpu_iowait: false, load_average: false, swap: false, memory_commit: true }
```

**`supports_v1_snapshot()` returns `false`** — v1 requires non-optional
load/swap which Windows cannot produce.

### Tests

- Normal delta, zero/full busy, identical counters, counter decrease
- Topology guards: multi-group, >64 processors
- Structural invariants: identity, cores, memory, warming, ready, commit
- Error propagation: cpu/memory/commit/drives
- v2 capabilities verification
- Windows foreground and SCM smokes: configured name, nonempty hostname, and
  NUL-free identity strings

---

Current frequency uses documented ProcessorInformation / CurrentMhz data. Disk
performance uses bounded direct physical-disk handles and closes each handle.
Network uses GetIfTable2 rows for InOctets, OutOctets, directional speeds,
operational state, and native loopback type. Failed optional queries are
omitted without changing core readiness.

## Fixture files

Located in `crates/greggd/src/collector/test_fixtures/`:

40+ text files covering:
- Ubuntu x86_64 and arm64 `/proc/stat` and `/proc/meminfo`
- Container environments (cgroups-limited)
- High-memory (>128 GiB) systems
- Zero-swap configurations
- Malformed inputs (for parse error testing)
- CPU hotplug and suspend/resume scenarios
- Counter reset and swap change scenarios
