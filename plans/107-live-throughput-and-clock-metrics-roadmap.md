# Plan 107: live throughput and clock metrics roadmap

Status: complete; closed by Plan 111.

Depends on: current main after completed Plan 106.

Coordinates: Plans 108-111.

## Objective

Extend Gregg's existing lightweight system-monitoring model with three operator-facing metric families:

1. current CPU clock/frequency where the platform can expose it truthfully without privilege escalation;
2. disk read/write throughput, with aggregate and per-drive/device detail integrated into the existing `e` expansion;
3. network throughput and link-capacity utilization, with a normal-view `NET` metric row, a new `n` expansion, and a `NET` column in the condensed `v` view.

The implementation must preserve Gregg's mixed-version fleet behavior: a new `gregg` client must continue to work against older `greggd` daemons. A metric that is absent because the daemon predates it or because the platform cannot expose it must simply be omitted from presentation rather than rendered as zero or treated as a protocol error.

## Explicitly deferred

Daemon binary-version transport/display is not part of this roadmap.

`greggd version` and the daemon's existing local status/version reporting remain unchanged. A later plan may expose daemon build version over the wire so one `gregg` client can inventory fleet versions, but this work must not pre-empt that design or add a version tag merely because the payload is already being extended.

## Product boundary

Keep Gregg a small local/LAN monitor. This roadmap must not add:

- persistent metric history, databases, retention, graphing, alerts, or exporters;
- packet capture, per-flow network inspection, socket/process attribution, or DPI;
- filesystem-level I/O tracing or eBPF requirements;
- privileged helper daemons, `sudo`, or command scraping to obtain metrics;
- undocumented Apple Silicon CPU-frequency internals;
- a broad cross-platform monitoring dependency solely to avoid the existing native collector boundaries;
- a new protocol major version when additive optional v2 fields can represent the data truthfully;
- daemon-version fleet management or update orchestration.

## Current architecture and why it fits

The present data path is already suitable:

```text
native platform collector
    -> CollectedMetrics
    -> sampler conversion
    -> additive /v2/status payload
    -> client endpoint parser
    -> NormalizedSnapshot
    -> state/reducer
    -> TUI
```

`StatusPayloadV2` already carries optional drive data outside the required base v2 snapshot. `NormalizedSnapshot` already converts v1/v2 into a single client-owned representation, and the client already falls back from `/v2/status` to v1 on old daemons. New telemetry should extend those seams rather than introduce version branches throughout rendering.

## Research conclusions that govern implementation

### Linux CPU frequency

Use Linux CPUFreq sysfs when present. Kernel documentation defines `cpuinfo_cur_freq` as current hardware-reported frequency where supported and `scaling_cur_freq` as the current frequency requested/reported by the scaling driver; the latter may not equal the actual instantaneous hardware frequency. Prefer `cpuinfo_cur_freq` when readable, fall back to `scaling_cur_freq`, and document the distinction.

Policy directories under `/sys/devices/system/cpu/cpufreq/policy*` expose the CPUs associated with each scaling policy. Weight a policy frequency by the number of currently online CPUs represented by that policy so heterogeneous systems are not biased merely by policy count.

Reference: Linux kernel CPUFreq documentation:
https://docs.kernel.org/admin-guide/pm/cpufreq.html

### macOS CPU frequency

Do not promise current CPU frequency on macOS in this pass.

Apple Silicon does not expose a stable public unprivileged API for true current CPU frequency. `powermetrics` can report sampled active frequency but commonly requires elevated privilege. Third-party tools that derive frequency/residency from IORegistry or power-management internals rely on undocumented interfaces that have changed across M-series generations.

Therefore:

- `cpu_frequency_hz = None` is truthful and expected on macOS when no supported source is available;
- do not spawn `powermetrics`;
- do not require root;
- do not encode nominal/base/max clock as though it were current clock;
- do not read undocumented Apple Silicon DVFS tables.

A future macOS-specific enhancement can revisit this if Apple exposes a supported API.

### Windows CPU frequency

Use the documented `CallNtPowerInformation(ProcessorInformation)` path and `PROCESSOR_POWER_INFORMATION::CurrentMhz`. Average valid processor entries using checked arithmetic. Treat call failure or empty/invalid data as unavailable rather than failing the whole sample.

Reference:
https://learn.microsoft.com/en-us/windows/win32/api/powerbase/nf-powerbase-callntpowerinformation
https://learn.microsoft.com/en-us/windows/win32/power/processor-power-information-str

### Linux disk I/O

Use cumulative kernel block statistics, not filesystem capacity deltas. `/sys/block/<dev>/stat` and equivalent sysfs block-device stats expose completed read/write sectors; the kernel documents these sectors as 512-byte sectors. Rates are derived from counter deltas over measured elapsed time.

Reference:
https://docs.kernel.org/block/stat.html

Do not equate the current `DriveMetrics` mount/filesystem abstraction with a physical disk. Device-mapper, LVM, RAID, partitions, overlay filesystems, containers, and multiple mounts can make that false. Plan 109 owns a bounded association model and aggregate de-duplication rules.

### macOS disk I/O

Use IOKit block-storage statistics. `kIOBlockStorageDriverStatisticsKey` contains cumulative driver statistics including bytes read and bytes written since driver instantiation. Derive rates from deltas, and re-baseline when identities disappear/reappear or counters decrease.

References:
https://developer.apple.com/documentation/iokit/kioblockstoragedriverstatisticskey
https://developer.apple.com/documentation/iokit/kioblockstoragedriverstatisticsbytesreadkey
https://developer.apple.com/documentation/iokit/kioblockstoragedriverstatisticsbyteswrittenkey

### Windows disk I/O

Use documented disk performance counters through `IOCTL_DISK_PERFORMANCE` / `DISK_PERFORMANCE`, which expose cumulative `BytesRead` and `BytesWritten`. Collection failure for one device must not fail CPU/memory/network sampling.

References:
https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_disk_performance
https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-disk_performance

### Linux network counters and topology

Use kernel interface byte counters. `/proc/net/dev`, sysfs statistics, or rtnetlink can expose the standard `rtnl_link_stats64` counters; the kernel recommends netlink for efficient multi-interface collection, but a bounded native procfs/sysfs implementation is acceptable if it is simpler and measured to be negligible at Gregg's cadence.

Reference:
https://docs.kernel.org/networking/statistics.html

Topology matters for capacity aggregation. Linux bonding exposes master/slave relationships in sysfs (`master`, `bonding/slaves`). Avoid counting both a bond/team/bridge-facing logical interface and its lower interfaces in the capacity denominator. Detail may still show interfaces that are excluded from aggregate-capacity accounting.

Reference:
https://docs.kernel.org/networking/bonding.html

### macOS network counters/capacity

Use BSD interface data from `getifaddrs`/AF_LINK or another existing native interface path. `if_data64` exposes 64-bit byte counters and `ifi_baudrate` for link rate where available. Prefer 64-bit counters.

References:
https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/freeifaddrs.3.html
https://developer.apple.com/documentation/kernel/if_data64

### Windows network counters/capacity

Use documented IP Helper interface rows (`GetIfEntry2`/`MIB_IF_ROW2`) for per-interface byte counters, operational/media state, and current receive/transmit link speeds. Do not infer capacity from adapter names.

Reference:
https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getifentry2

## Shared metric semantics

### Rates

All disk and network throughput values are bytes per second on the wire/internal model.

Do not label operation counts as `R/s` or `W/s`. In the UI, `R/s`, `W/s`, `Rx/s`, and `Tx/s` refer to byte throughput formatted with units such as `KiB/s`, `MiB/s`, or `GiB/s`.

Rates must use the actual elapsed monotonic duration between counter observations, not assume that the configured nominal sampler interval elapsed exactly.

First observation, counter decrease/reset, identity replacement, or invalid elapsed time re-baselines that series and yields no rate for that cycle. It must never emit a huge synthetic spike.

### CPU-frequency aggregation

Where multiple current processor/policy frequencies are available, report one host-level current-frequency summary in Hz. Prefer a logical-CPU-weighted arithmetic mean. Invalid/zero source entries are excluded; if no valid entries remain, the metric is absent.

The normal CPU suffix should become conceptually:

```text
20% 16 cores 2.40GHz
```

Frequency formatting belongs to the client, not the daemon.

### Network capacity/utilization

The normal and condensed `NET` percentage represents utilization of available aggregate interface capacity, not a percentage of observed historical traffic.

For eligible capacity-bearing interfaces:

```text
rx_util = total_rx_bits_per_sec / total_rx_link_capacity_bits_per_sec
tx_util = total_tx_bits_per_sec / total_tx_link_capacity_bits_per_sec
net_util = max(rx_util, tx_util)
```

This is directional because ordinary Ethernet is full duplex. Do not compute `(rx + tx) / link_speed`, which can exceed 100% under valid full-duplex traffic.

For interfaces whose receive/transmit speeds differ, retain directional capacities and compute each direction against its own denominator.

Loopback and interfaces with unknown/non-physical capacity may appear in the `n` detail view but do not fabricate capacity. When no eligible interface exposes capacity, aggregate Rx/Tx may still be shown in detail, but the `NET` progress bar/percentage is unavailable.

### Network aggregate membership

Capacity aggregation must avoid obvious topology double counting. At minimum:

- exclude loopback from aggregate capacity;
- ignore administratively/down or disconnected interfaces from available active capacity unless the platform semantics clearly indicate otherwise;
- on Linux, do not count an enslaved lower interface in addition to the selected active bond/team/bridge aggregate representation;
- retain virtual/loopback adapters in detail when counters are available;
- deterministic ordering is required.

Do not attempt a generalized network topology engine. Implement bounded platform rules with tests for the common master/slave cases Gregg needs.

### Disk aggregate membership

Capacity (`used/total`) remains the current mounted-filesystem aggregate. Throughput is a distinct kernel/device metric.

The UI may display disk throughput alongside drive rows where the daemon can associate a displayed filesystem/mount with an I/O accounting identity truthfully. It must not invent one-to-one physical-drive attribution.

The daemon should provide a separately correct aggregate disk-I/O total so the client does not have to sum potentially overlapping per-mount records.

## Plan sequence

### Plan 108: additive protocol and client normalization

Owns wire types, optional-field compatibility, normalized client structures, bounds, validation, and old-daemon fixtures. It must land first.

### Plan 109: native live metric collectors

Owns Linux/macOS/Windows CPU-frequency support where truthful, disk-I/O counters/rates, network counters/link capacities/topology filtering, monotonic baseline state, and daemon publication.

### Plan 110: TUI integration

Owns CPU clock suffix, fifth normal `NET` metric row, enriched `e` disk detail, new `n` network detail, condensed `NET` column, viewport/selection geometry, and responsive degradation.

### Plan 111: compatibility, operational verification, and documentation closure

Owns mixed-version evidence, cross-platform CI truth, Ubuntu live smoke, docs/architecture reconciliation, and final acceptance against this roadmap. It must not create a new CI matrix.

## Global acceptance criteria

This roadmap is complete only when:

1. A new `gregg` works unchanged with v1-only and pre-feature v2 `greggd` daemons.
2. Missing CPU-frequency, disk-I/O, network-throughput, or network-capacity data is represented as absent, never fabricated as zero.
3. Linux and Windows expose a current CPU-frequency summary through supported native interfaces when available; macOS remains truthfully absent unless a supported unprivileged source is found during implementation.
4. Disk aggregate and per-device/detail throughput are delta-derived from native cumulative counters with reset/re-baseline handling.
5. Network detail includes per-interface Rx/s and Tx/s; loopback may appear in detail but does not inflate aggregate link capacity.
6. Aggregate NET utilization is directional/full-duplex-safe and cannot exceed 100% due merely to simultaneous Rx and Tx.
7. Common bond/master/lower-interface arrangements do not double-count aggregate capacity.
8. Normal view adds NET under DISK without breaking selection, scrolling, fleet-wide bar geometry, or expanded-drive clipping.
9. `e` exposes disk read/write throughput and `n` exposes aggregate plus per-interface network details.
10. Condensed `v` view gains a NET column at widths where it can fit while preserving bounded fallback tiers.
11. No privileged subprocesses, daemon-version transport, persistent telemetry, or generalized observability subsystem is introduced.
12. Full local verification, Ubuntu runtime evidence, and the existing native macOS/Windows CI jobs pass.

## Closure record

The roadmap is complete through Plans 108-111. Plan 108 added the additive v2
wire fields and client normalization, Plan 109 added native monotonic live
collection, Plan 110 integrated the telemetry into the TUI, and Plan 111 closed
the mixed-version, runtime, documentation, and verification work. The only
implementation correction found during final closure was the Linux CPUFreq
parser accepting the whitespace-separated `affected_cpus` form emitted by the
current Ubuntu host (`efb18dc`).

Daemon-version transport, persistent history, process attribution, and the
other roadmap exclusions remain deferred.
