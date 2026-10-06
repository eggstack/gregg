---
name: platform-collectors
description: Work with native host telemetry in gregg-host and the greggd collector adapters
---

## What I do

Guide agents through the native telemetry implementations (Linux, macOS,
Windows, FreeBSD) in `gregg-host` and the thin platform adapters in
`greggd/src/collector/`.

## When to use me

Use this when modifying metric collection, adding new metrics, fixing collector bugs, or working with platform-specific code.

Native acquisition lives in `crates/gregg-host/src/{linux,macos,windows,freebsd}/`
plus shared `src/rate.rs` and `src/slow_probe.rs`. `crates/greggd/src/collector/`
(`linux`/`macos`/`windows` only — no FreeBSD adapter; FreeBSD coverage is
`gregg-host`-only on FreeBSD CI) holds the Gregg-owned `SystemCollector` trait,
`CollectedMetrics`, v1/v2 conversion, and readiness mapping. See
`architecture/collectors.md` for the full deep dive.

## Shared contract

### SystemCollector trait

```rust
pub trait SystemCollector: Send {
    fn identity(&self) -> Result<SystemIdentity, CollectError>;
    fn sample(&mut self) -> Result<CollectedMetrics, CollectError>;
    fn capabilities(&self) -> MetricCapabilities;      // v1
    fn capabilities_v2(&self) -> MetricCapabilitiesV2; // v2
    fn supports_v1_snapshot(&self) -> bool;             // default true, false on Windows
}
```

One call to `sample()` produces `CollectedMetrics` which converts to both v1 and v2 wire formats without duplicate collection.

Byte-ratio percentages must use the shared `gregg_host::clamped_usage_pct`
helper (re-exported via `greggd::collector`) so all platform collectors share
the same zero, clamp, and non-finite behavior.

The daemon serves v2 status on every platform. Windows cannot produce a
truthful v1 snapshot, so `/`, `/v1/status`, and `/healthz` return HTTP 503 with
a v1 `NotServing` health response; `/v2/status` and `/v2/healthz` become ready
after a valid sample.

### CollectError taxonomy

| Kind | Meaning |
|------|---------|
| `Warming` | First sample; no delta available yet |
| `SourceUnavailable` | Kernel interface missing or unreadable (also used for unreadable identity fields) |
| `Parse` | Content present but unparseable |
| `CounterReset` | Kernel counter wrapped, decreased, or produced a zero delta (identical counters / suspend) since the last sample |
| `Numeric` | Arithmetic error (division by zero, overflow) |
| `IdentityFallback` | Reserved; currently unconstructed — identity failures surface as `SourceUnavailable`/`Parse`, never a fabricated identity |

### Common patterns

1. **Construct eagerly** — identity and static fields read in `new()`
2. **First `sample()` returns `Warming`** — CPU percentages require two readings
3. **Subsequent `sample()` returns `Ok(CollectedMetrics)`** — unless counter reset
4. **One native sample → both v1 and v2 wire representations**

## Linux collector

**Source:** `crates/gregg-host/src/linux/` (adapter: `crates/greggd/src/collector/linux/`)

- CPU: `/proc/stat` — cumulative ticks, delta percentages
- Memory: `/proc/meminfo` — prefers `MemAvailable`, falls back to `MemFree + Buffers + Cached + SReclaimable`
- Swap: `/proc/meminfo` — `SwapTotal` and `SwapFree`; an absent key fails
  closed (`Parse`) rather than becoming `0`, because the protocol requires a
  swap sample whenever the swap capability is asserted, so there is no
  representation for "could not read swap". A real `SwapTotal: 0` is valid.
- Load: `/proc/loadavg`
- Identity: `gethostname()`, `/proc/sys/kernel/osrelease`, `/etc/os-release`
- Drives: `/proc/self/mountinfo` + `statvfs` on the optional refresh worker
  (the only unsafe in this module); retain both total-free (`f_bfree`) and
  caller-available (`f_bavail`) bytes; skip `autofs`, generic `fuse`, and
  `fuse.*` filesystems

Capabilities: `cpu_iowait: true`, `load_average: true`, `swap: true`, `memory_commit: false`

Live telemetry uses CPUFreq policy files (hardware-current first, scaling
current fallback, weighted by affected CPU membership), top-level
/sys/block/*/stat sector counters, and /proc/net/dev plus sysfs link metadata.
CPUFreq membership parsing must accept the kernel's range, comma-separated, and
space-separated CPU-list forms. The aggregate disk set is separate from
mounted capacity rows. Network slaves are not added to their master, down
links do not contribute capacity, and loopback remains detail-only for
capacity. All cumulative counters use the shared monotonic baseline helper in
`gregg-host/src/rate.rs`.

Two Linux edges are "unknown is not a value":

- **An unreadable or unparseable `/sys/class/net/<if>/flags` is unknown, not
  "not loopback".** Defaulting the failed read to `0` made `lo` an aggregate
  member (it has no `master` symlink) on any host where `/proc/net/dev` is
  readable but sysfs is masked, unmounted, or `EACCES`, folding loopback traffic
  into the fleet aggregate. An unknown interface stays out of the aggregate; the
  record still publishes with `is_loopback: false`, which the validator accepts.
- **`logical_cores` is the kernel's count, not the process CPU allotment.**
  `std::thread::available_parallelism` honours a `taskset` mask or cpuset, so a
  restricted daemon published `1` on a many-core host and disagreed with the
  Windows `GetActiveProcessorCount` total. Linux reads
  `/sys/devices/system/cpu/online` via the existing cpulist grammar, falls back
  to `processor` entries in `/proc/cpuinfo`, and only then to the allotment
  probe. `logical_cores` is a required `> 0` field, so it cannot express
  absence; the required-field floor is reachable only when both procfs and
  sysfs are unreadable.

## macOS collector

**Source:** `crates/gregg-host/src/macos/` (adapter: `crates/greggd/src/collector/macos/`)

- CPU: Mach `HOST_CPU_LOAD_INFO` ticks — `user`, `system`, `idle`, `nice`
- Memory: Mach `HOST_VM_INFO64` — availability-oriented (free + inactive)
- Swap: `sysctl vm.swapusage`
- Load: `getloadavg()`
- Identity: `kern.hostname` sysctl, `SystemVersion.plist`, `kern.osrelease` sysctl
- Drives: `libc::getmntinfo()` with `libc::statfs` (libc owns the Darwin
  `INODE64` ABI; no private `StatFs` layout) — excludes devfs, autofs,
  `MNT_DONTBROWSE`; retain both `f_bfree` and `f_bavail`

Current CPU frequency remains unavailable because no supported public
unprivileged source was found. Disk counters come from IOKit block-storage
driver statistics and network counters/capacity prefer `NET_RT_IFLIST2` /
`if_msghdr2` 64-bit counters with a correctly typed `getifaddrs` / `if_data`
fallback (never `if_data64` from `getifaddrs`).
Malformed storage records are skipped independently; loopback is retained only
as detail and never contributes aggregate capacity. Optional drive/network/
disk-I/O failures use bounded transition logging with family and error context.

Capabilities: `cpu_iowait: false`, `load_average: true`, `swap: true`, `memory_commit: false`

FFI seam: `MacNativeQueries` trait. Production: `FfiNativeQueries`. Test: `MockNativeQueries`.

## Windows collector

**Source:** `crates/gregg-host/src/windows/` (adapter: `crates/greggd/src/collector/windows/`)

- CPU: `GetSystemTimes` — idle, kernel (includes idle), user
- Memory: `GlobalMemoryStatusEx`
- Commit: `GetPerformanceInfo` — commit charge (distinct from swap)
- Identity: `GetComputerNameExW`, `RtlGetVersion`
- Drives: `GetLogicalDriveStringsW` + `GetDiskFreeSpaceExW` — fixed and
  removable drives (`DRIVE_FIXED`, `DRIVE_REMOVABLE`) with positive capacity;
  retain caller-available and total-free outputs separately

Live telemetry uses CallNtPowerInformation / ProcessorInformation and
CurrentMhz, direct per-disk IOCTL_DISK_PERFORMANCE, and documented IP Helper
MIB_IF_ROW2 data. Inaccessible disks and failed optional API calls are skipped
or omitted without failing the core sample; disconnected adapters and loopback
do not contribute aggregate capacity.

Capabilities: `cpu_iowait: false`, `load_average: false`, `swap: false`, `memory_commit: true`

**`supports_v1_snapshot()` returns `false`** — v1 requires non-optional load/swap which Windows cannot produce.

FFI seam: `WindowsSource` trait. Production: `NativeWindowsSource`. Test: `MockWindowsSource`.

The collector accepts an optional display-name override: daemon foreground and
Windows SCM startup pass the validated `Config::name` as `system.name`, while
`system.hostname` remains the native hostname. `GetComputerNameExW` callers
must decode only the UTF-16 units reported by the successful call so API
buffer padding cannot produce a NUL in the wire identity.

## FreeBSD collector

**Source:** `crates/gregg-host/src/freebsd/` (native only — there is no
`crates/greggd/src/collector/freebsd/` adapter; the full `greggd` product
remains Linux/macOS/Windows).

First post-extraction backend (Plan 136): FreeBSD sysctl-based collection
through the same `SystemCollector`-compatible native surface, shared
`rate.rs` baselines, and the `DriveRefreshCache` slow probe. Qualified on
FreeBSD CI (`cargo test -p gregg-host`). NetBSD/OpenBSD remain explicit later
ports.

## Key constraints

- No external command execution for metrics collection
- Use kernel interfaces (`/proc`), Mach APIs, or Windows native APIs
- Current CPU frequency is optional OS-reported frequency, never a base/max
  claim; macOS remains absent when no supported unprivileged source exists
- Disk capacity is distinct from disk I/O; `R/s`/`W/s` and `Rx/s`/`Tx/s` are
  byte-throughput rates
- Network utilization is directional/full-duplex-safe and loopback is detail
  only, never aggregate capacity
- Every unsafe block must have a safety comment
- Tests must not sleep for production refresh intervals
- Inject clocks or short intervals for deterministic testing

## Tests

- Unit tests in every module with deterministic fixtures
- Facade freeze fixtures in `crates/greggd/src/collector/test_fixtures/`
- Platform-native collector tests run only on the target OS
- `FileSource` (Linux) — file-read seam; `MemorySource` in-memory map for deterministic tests
- `MockNativeQueries` (macOS) — injectable FFI with auto-increment CPU
- `MockWindowsSource` (Windows) — injectable API with auto-increment CPU
- FreeBSD native tests run on FreeBSD CI via `cargo test -p gregg-host --all-features`
