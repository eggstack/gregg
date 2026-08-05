---
name: platform-collectors
description: Work with platform-specific metric collectors in greggd
---

## What I do

Guide agents through the platform-specific collector implementations (Linux, macOS, Windows).

## When to use me

Use this when modifying metric collection, adding new metrics, fixing collector bugs, or working with platform-specific code.

## Shared contract

### SystemCollector trait

```rust
pub trait SystemCollector {
    fn identity(&self) -> SystemIdentity;
    fn sample(&mut self) -> Result<CollectedMetrics, CollectError>;
    fn capabilities(&self) -> MetricCapabilities;      // v1
    fn capabilities_v2(&self) -> MetricCapabilitiesV2; // v2
    fn supports_v1_snapshot(&self) -> bool;             // false on Windows
}
```

One call to `sample()` produces `CollectedMetrics` which converts to both v1 and v2 wire formats without duplicate collection.

The daemon serves v2 status on every platform. Windows cannot produce a
truthful v1 snapshot, so `/`, `/v1/status`, and `/healthz` return HTTP 503 with
a v1 `NotServing` health response; `/v2/status` and `/v2/healthz` become ready
after a valid sample.

### CollectError taxonomy

| Kind | Meaning |
|------|---------|
| `Warming` | First sample; no delta available yet |
| `SourceUnavailable` | Kernel interface missing or unreadable |
| `Parse` | Content present but unparseable |
| `CounterReset` | Kernel counter decreased (wrap or reset) |
| `Numeric` | Arithmetic error (division by zero, overflow) |
| `IdentityFallback` | Identity field unreadable, fallback used |

### Common patterns

1. **Construct eagerly** — identity and static fields read in `new()`
2. **First `sample()` returns `Warming`** — CPU percentages require two readings
3. **Subsequent `sample()` returns `Ok(CollectedMetrics)`** — unless counter reset
4. **One native sample → both v1 and v2 wire representations**

## Linux collector

**Source:** `crates/greggd/src/collector/linux/`

- CPU: `/proc/stat` — cumulative ticks, delta percentages
- Memory: `/proc/meminfo` — prefers `MemAvailable`, falls back to `MemFree + Buffers + Cached + SReclaimable`
- Swap: `/proc/meminfo` — `SwapTotal` and `SwapFree`
- Load: `/proc/loadavg`
- Identity: `gethostname()`, `/proc/sys/kernel/osrelease`, `/etc/os-release`
- Drives: `/proc/self/mountinfo` + `statvfs` (the only unsafe in this module);
  retain both total-free (`f_bfree`) and caller-available (`f_bavail`) bytes

Capabilities: `cpu_iowait: true`, `load_average: true`, `swap: true`, `memory_commit: false`

## macOS collector

**Source:** `crates/greggd/src/collector/macos/`

- CPU: Mach `HOST_CPU_LOAD_INFO` ticks — `user`, `system`, `idle`, `nice`
- Memory: Mach `HOST_VM_INFO64` — availability-oriented (free + inactive)
- Swap: `sysctl vm.swapusage`
- Load: `getloadavg()`
- Identity: `kern.hostname` sysctl, `SystemVersion.plist`, `kern.osrelease` sysctl
- Drives: `getmntinfo()` — excludes devfs, autofs, `MNT_DONTBROWSE`; retain
  both `f_bfree` and `f_bavail`

Capabilities: `cpu_iowait: false`, `load_average: true`, `swap: false`, `memory_commit: false`

FFI seam: `MacNativeQueries` trait. Production: `FfiNativeQueries`. Test: `MockNativeQueries`.

## Windows collector

**Source:** `crates/greggd/src/collector/windows/`

- CPU: `GetSystemTimes` — idle, kernel (includes idle), user
- Memory: `GlobalMemoryStatusEx`
- Commit: `GetPerformanceInfo` — commit charge (distinct from swap)
- Identity: `GetComputerNameExW`, `RtlGetVersion`
- Drives: `GetLogicalDriveStringsW` + `GetDiskFreeSpaceExW` — fixed drives
  only; retain caller-available and total-free outputs separately

Capabilities: `cpu_iowait: false`, `load_average: false`, `swap: false`, `memory_commit: true`

**`supports_v1_snapshot()` returns `false`** — v1 requires non-optional load/swap which Windows cannot produce.

FFI seam: `WindowsSource` trait. Production: `NativeWindowsSource`. Test: `MockWindowsSource`.

## Key constraints

- No external command execution for metrics collection
- Use kernel interfaces (`/proc`), Mach APIs, or Windows native APIs
- Every unsafe block must have a safety comment
- Tests must not sleep for production refresh intervals
- Inject clocks or short intervals for deterministic testing

## Tests

- Unit tests in every module with deterministic fixtures
- 40+ JSON/text fixture files in `src/collector/test_fixtures/`
- Platform-native collector tests run only on the target OS
- `MemorySource` (Linux) — in-memory file map for deterministic tests
- `MockNativeQueries` (macOS) — injectable FFI with auto-increment CPU
- `MockWindowsSource` (Windows) — injectable API with auto-increment CPU
