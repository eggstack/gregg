# gregg-host

Native host telemetry acquisition extracted from `greggd` (Plans 132-135).

Protocol-neutral, synchronous, and runtime-neutral: Linux (`/proc`, `/sys`),
macOS (Mach/sysctl/libc/IOKit), Windows (Win32), and FreeBSD (`sysctl`,
`libdevstat`, `ifmib`) native collection with explicit first-sample warming,
counter-reset re-baselining, bounded drive normalization, and slow-probe
isolation. No `gregg-protocol` dependency, no Tokio/EggServe/Clap, no external
metric commands, no new privilege requirements.

`greggd` consumes this crate through a compatibility adapter and remains the
owner of schema versions, v1/v2 conversion, readiness, timestamps, HTTP, and
service lifecycle.

## Absence is never a value

A missing measurement is an absent field, not a zero, and a total of zero is an
error on every backend. Three Linux edges this crate holds that line on:

- **An unreadable interface `flags` file is *unknown*, not "not loopback".** The
  `IFF_LOOPBACK` bit came from a sysfs read whose failure mode was `0`. Since
  `lo` has no `master` symlink, that made loopback an aggregate member whenever
  `/proc/net/dev` was readable but sysfs was masked, unmounted, or `EACCES`, and
  folded loopback traffic into the fleet aggregate. An unknown interface stays
  out of the aggregate; the record still publishes.
- **`logical_cores` is the kernel's count, never the process's CPU allotment.**
  `std::thread::available_parallelism` honours a `taskset` mask or cpuset, so a
  restricted daemon would publish `logical_cores: 1` on a many-core host and
  disagree with the Windows collector's `GetActiveProcessorCount`. Linux reads
  `/sys/devices/system/cpu/online` (the same cpulist grammar the CPUFreq
  weighting uses), falls back to `processor` entries in `/proc/cpuinfo`, and only
  then falls back to the allotment probe.
- **Swap fails closed on a truncated `/proc/meminfo`.** An absent `SwapTotal` —
  or an absent `SwapFree`, which would fabricate full usage — is a parse error,
  matching what `compute_memory` already does for `MemTotal`. A genuine
  `SwapTotal: 0` is a real reading and still yields the zero sample.

## Platform support

- Linux: qualified (native CI).
- macOS: qualified on arm64 + Intel (native CI).
- Windows: qualified (native CI + SCM smoke); single processor-group guard
  preserved.
- FreeBSD: `x86_64-unknown-freebsd` native backend (Plan 136); `aarch64`
  compiles. Full `greggd` FreeBSD service/install/release support is a later
  product plan.
- NetBSD/OpenBSD: explicit future backends, not implied.

## MSRV

Rust 1.89 (workspace).
