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
