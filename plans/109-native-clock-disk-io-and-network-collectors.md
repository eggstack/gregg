# Plan 109: native clock, disk-I/O, and network collectors

Status: implementation complete; closure record pending the implementation commit and remote CI.

Depends on: Plans 107-108.

Blocks: Plans 110-111.

## Objective

Implement native `greggd` collection and publication for:

- current CPU frequency where the host OS exposes it through a supported, unprivileged interface;
- aggregate and per-device disk read/write throughput;
- aggregate and per-interface network receive/transmit throughput plus current link capacities.

The implementation must preserve the daemon's existing collector ownership, bounded sampling behavior, optional-metric semantics, and cross-platform native approach.

## Non-goals

Do not add:

- shelling out to `powermetrics`, `ethtool`, `ip`, `iostat`, `diskutil`, PowerShell, WMI commands, or other external executables;
- root/admin requirements merely to populate optional metrics;
- eBPF, ETW sessions, packet capture, process attribution, per-flow accounting, or persistent history;
- undocumented Apple Silicon DVFS parsing;
- a third-party all-in-one metrics crate unless implementation proves a concrete gap that cannot be met cleanly by the existing native seams;
- daemon-version transport.

## Collector architecture

Keep platform-specific source acquisition inside:

```text
crates/greggd/src/collector/linux/
crates/greggd/src/collector/macos/
crates/greggd/src/collector/windows/
```

Keep shared validation/rate/baseline helpers under `collector/` rather than duplicating arithmetic in each OS module.

`SystemCollector::sample()` remains the high-level sampling boundary. The collector may retain prior cumulative-counter observations internally so rates can be calculated before the sample reaches `CollectedMetrics`.

Do not move rate derivation into the client.

## Shared baseline/rate primitive

Introduce a small reusable monotonic-counter baseline helper rather than hand-coding subtraction separately for every metric.

Conceptually it needs:

```rust
CounterSample {
    observed_at: Instant,
    rx_or_read_bytes: u64,
    tx_or_write_bytes: u64,
}
```

and an operation that returns `None` when no valid previous baseline exists.

Required behavior:

- first observation: store baseline, no rate;
- current timestamp <= prior timestamp: re-baseline, no rate;
- either cumulative counter decreased: re-baseline that identity, no rate;
- identity disappeared: remove/age out its baseline;
- identity reappeared: treat as a fresh baseline unless the collector can prove continuity;
- checked subtraction/multiplication/division only;
- no division by nominal `sample_interval_ms`;
- rate based on actual monotonic elapsed time;
- no synthetic spike after sleep/resume, device reset, daemon startup, or hotplug.

Prefer integer-safe nanosecond/microsecond arithmetic or a documented `Duration`-based helper. Do not use wall-clock Unix timestamps for deltas.

## CPU frequency

### Linux

Preferred source order:

1. `/sys/devices/system/cpu/cpufreq/policy*/cpuinfo_cur_freq` where readable;
2. the same policy's `scaling_cur_freq` when hardware-reported frequency is unavailable.

Use policy membership (`affected_cpus`/equivalent) to weight frequencies by online logical CPUs represented by each policy.

Required semantics:

- input is kHz; checked conversion to Hz;
- zero, malformed, unreadable, or overflowed policy values are ignored;
- a policy with no valid frequency contributes nothing;
- if no valid policy remains, CPU frequency is absent;
- do not parse `/proc/cpuinfo` as the primary current-frequency source;
- do not confuse `cpuinfo_max_freq`, base frequency, or scaling maximum with current frequency.

Kernel reference:
https://docs.kernel.org/admin-guide/pm/cpufreq.html

Tests should include multiple policies with unequal CPU membership so weighting is proven rather than assumed.

### macOS

Return frequency as unavailable in this plan unless implementation finds a supported public, unprivileged API that reports current frequency accurately.

Explicitly forbidden:

- spawning `powermetrics`;
- requiring root;
- parsing undocumented `voltage-states*-sram` IORegistry properties;
- substituting nominal/base/max frequency.

Do not treat absence as a collector failure.

### Windows

Use `CallNtPowerInformation` with `ProcessorInformation`, yielding one `PROCESSOR_POWER_INFORMATION` per processor.

Use `CurrentMhz`, exclude invalid/zero entries, and derive a checked arithmetic mean over valid processors. Convert MHz to Hz after overflow checks.

References:
https://learn.microsoft.com/en-us/windows/win32/api/powerbase/nf-powerbase-callntpowerinformation
https://learn.microsoft.com/en-us/windows/win32/power/processor-power-information-str

A failure of this optional call must not fail the entire sample.

## Disk-I/O collection

### Shared semantics

The daemon publishes:

- a de-duplicated aggregate read bytes/s;
- a de-duplicated aggregate write bytes/s;
- bounded per-device/logical-device records;
- optional association to the current drive/mount display rows only where trustworthy.

The aggregate accounting set must be defined independently from the UI list so overlapping parent/partition/logical-device rows do not get summed twice.

### Linux

Use kernel block statistics. `/sys/block/<device>/stat` is preferred for top-level aggregate candidates; mount-source mapping may use `/sys/dev/block/<major>:<minor>` and existing mount metadata when associating drive rows.

Kernel semantics:

- read sectors and write sectors are cumulative;
- sectors are standardized 512-byte sectors for these statistics;
- convert sectors to bytes with checked multiplication;
- completed I/O counters are not byte throughput and should not be substituted for sector fields.

Reference:
https://docs.kernel.org/block/stat.html

#### Aggregate de-duplication

Define a deterministic aggregate set that avoids obvious parent/child double counting.

Recommended starting rule:

- aggregate over eligible top-level block devices enumerated from `/sys/block`;
- exclude loop/ram/zram and other obviously synthetic devices unless a real deployed target demonstrates they should count;
- do not additionally sum child partitions when their parent already contributes to the aggregate;
- device-mapper/MD layering requires care: choose one accounting layer, document it, and test that common LVM/RAID layouts are not doubled.

Do not try to model every possible storage topology. Prefer a small truthful aggregate over a broad but duplicative one.

#### Per-drive association

The existing `DriveMetrics` is mounted-filesystem capacity data. Associate disk-I/O to a drive row only when a unique accounting identity can be derived from the mount source/device identity.

If association is ambiguous:

- keep the capacity row;
- keep the disk-I/O device record;
- leave the association absent;
- never assign physical-device throughput heuristically from a mount name.

Tests should cover ordinary partition mounts and at least one ambiguous device-mapper-style fixture.

### macOS

Use IOKit storage statistics under `kIOBlockStorageDriverStatisticsKey`, including cumulative bytes read/written.

References:
https://developer.apple.com/documentation/iokit/kioblockstoragedriverstatisticskey
https://developer.apple.com/documentation/iokit/kioblockstoragedriverstatisticsbytesreadkey
https://developer.apple.com/documentation/iokit/kioblockstoragedriverstatisticsbyteswrittenkey

Required behavior:

- stable registry identity or another native identity keys baselines;
- bytes are already cumulative byte counts; do not sector-convert;
- storage-service disappearance/reappearance re-baselines;
- malformed/missing one device's properties skips that device without failing CPU/memory/network.

Keep FFI ownership contained like the existing macOS collector design.

### Windows

Use `IOCTL_DISK_PERFORMANCE` and `DISK_PERFORMANCE` on eligible disk devices.

References:
https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_disk_performance
https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-disk_performance

Use cumulative `BytesRead` and `BytesWritten`; rate derivation follows the shared baseline logic.

Requirements:

- inaccessible devices are skipped as optional telemetry;
- do not turn optional disk-performance failure into daemon unready/failed state;
- device enumeration and handles are bounded/closed correctly;
- avoid adding a continuously enabled global performance facility if direct per-disk querying suffices.

## Network collection

### Shared semantics

For each interface collect where available:

- stable id;
- display name;
- cumulative RX bytes;
- cumulative TX bytes;
- current RX/TX link capacity in bits/s;
- loopback status;
- operational/connected state;
- platform-selected aggregate-membership status.

Rates are derived from cumulative bytes using the shared baseline helper.

Aggregate throughput includes all intended traffic interfaces according to the platform rule; aggregate capacity includes only eligible active capacity-bearing members.

Loopback may be shown in detail, but is never part of capacity aggregation.

### Linux

Use native kernel statistics; choose one implementation after measuring complexity:

- rtnetlink `RTM_GETLINK` / `IFLA_STATS64`, or
- `/proc/net/dev` for byte counters plus sysfs for speed/topology/state.

The kernel identifies rtnetlink as the preferred multi-interface statistics API, but Gregg may keep a simpler procfs/sysfs implementation if it remains bounded and dependency-light.

Reference:
https://docs.kernel.org/networking/statistics.html

For current link speed use `/sys/class/net/<if>/speed` where supported. Treat unsupported/malformed/negative values as unknown, not zero capacity.

Use `/sys/class/net/<if>/operstate` and native topology symlinks/files for eligibility.

#### Linux topology rules

At minimum handle:

- loopback excluded from aggregate capacity;
- a lower interface with `/sys/class/net/<if>/master` is not counted in addition to the active master representation when that would double capacity;
- bond slave/master relationships exposed through sysfs;
- disconnected/down links excluded from currently available aggregate capacity;
- detail list may retain excluded interfaces with their own Rx/s and Tx/s.

Bonding reference:
https://docs.kernel.org/networking/bonding.html

Bridge/team/container interfaces should follow the same bounded principle: prefer a single capacity-bearing accounting level and avoid obvious double counting. Do not infer topology from interface-name prefixes alone when sysfs exposes ownership.

### macOS

Use AF_LINK interface records from `getifaddrs` or the existing equivalent native seam. Prefer `if_data64` to obtain 64-bit byte counters and `ifi_baudrate` where available.

References:
https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/freeifaddrs.3.html
https://developer.apple.com/documentation/kernel/if_data64
https://developer.apple.com/documentation/kernel/if_data64/1492491-ifi_baudrate

Use native flags/type information to identify loopback and operational state rather than interface-name heuristics when possible.

If link baud is unavailable for an interface, retain throughput but omit its capacity contribution.

### Windows

Use documented IP Helper APIs, preferably interface-table enumeration plus `MIB_IF_ROW2` data/`GetIfEntry2` semantics.

Use:

- `InOctets`/`OutOctets` or their corresponding 64-bit row fields for cumulative byte counters;
- `ReceiveLinkSpeed` and `TransmitLinkSpeed` for directional capacities;
- media/operational state for aggregate eligibility;
- native interface type to identify loopback where available.

Reference:
https://learn.microsoft.com/en-us/windows/win32/api/netioapi/nf-netioapi-getifentry2

## CollectedMetrics and sampler publication

Extend `CollectedMetrics` with optional fields matching Plan 108's protocol model.

Do not require these optional metrics for readiness. Existing CPU usage remains the essential sampled metric. A host whose network API temporarily fails can still publish a Ready snapshot with `network: None`.

The sampler conversion should:

- copy optional data into `StatusPayloadV2`;
- keep v1 snapshots unchanged;
- preserve current v1/v2 readiness semantics;
- validate the final v2 payload before publication using the extended validator.

Do not add these fields to v1.

## Refresh cadence

CPU frequency and cumulative disk/network counters are cheap enough to observe at the ordinary sample cadence when platform calls are bounded.

Do not reuse the existing 30-second drive-capacity refresh cache for throughput counters: that cadence is too slow for live rates.

If a platform-specific enumeration is expensive, separate slow identity enumeration from cheap counter refresh behind a small cache, but do not prematurely add worker infrastructure without measurement.

## Failure isolation

Optional telemetry must degrade independently.

Examples:

- CPU frequency unavailable -> only frequency absent;
- one block device inaccessible -> skip that device, keep others;
- network link speed unavailable -> throughput retained, percentage may be absent;
- all network counters unavailable -> network absent, daemon still Ready if core metrics are valid.

Log optional-source failures at debug/trace or bounded warning levels appropriate to existing collector practice. Avoid log spam every second for a stable unsupported condition.

## Required tests

### Shared rate helper

- first sample warms;
- exact 1-second delta;
- 250 ms and non-integral-second delta;
- long scheduler delay uses actual elapsed time;
- counter decrease/reset re-baselines;
- zero elapsed re-baselines;
- overflow-safe conversion;
- identity disappearance/reappearance.

### Linux fixtures

- CPUFreq `cpuinfo_cur_freq` preferred;
- fallback to `scaling_cur_freq`;
- weighted multiple CPU policies;
- partial malformed policies;
- `/sys/block/.../stat` sector conversion;
- parent/partition aggregate de-duplication;
- mount-to-device association success and ambiguity;
- ordinary Ethernet rate/capacity;
- unknown speed retains throughput;
- loopback detail but no capacity;
- bond master/slave no double-counted capacity;
- link down removes available capacity.

### macOS tests

- CPU frequency absent without failure;
- IOKit disk-byte delta conversion;
- malformed one-device stats skipped;
- `if_data64` Rx/Tx delta;
- `ifi_baudrate` capacity preserved;
- loopback excluded from aggregate capacity.

### Windows tests

- `CurrentMhz` mean and zero-entry filtering;
- failed frequency API -> absent only;
- `DISK_PERFORMANCE` byte deltas;
- inaccessible disk skipped;
- interface byte deltas and directional link capacities;
- disconnected adapter excluded from capacity;
- loopback/detail behavior.

Use injectable/native-source seams and fixtures rather than relying on host-specific devices in unit tests.

## Files likely touched

```text
crates/greggd/src/collector/mod.rs
crates/greggd/src/collector/drives.rs
crates/greggd/src/collector/linux/*
crates/greggd/src/collector/macos/*
crates/greggd/src/collector/windows/*
crates/greggd/src/sampler.rs
crates/greggd/Cargo.toml only if documented Windows/macOS bindings require existing-platform feature additions
```

Prefer extending existing platform modules over creating many one-function files.

## Local verification

Mandatory:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p greggd --all-targets --all-features
cargo test -p gregg-protocol --all-targets --all-features
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
```

On the current Ubuntu host, add a focused runtime probe after implementation:

1. run `greggd` directly on loopback with a temporary config;
2. fetch `/v2/status` twice or more after warmup;
3. confirm CPU frequency is present when host CPUFreq exposes it;
4. generate bounded disk I/O and confirm read/write rates change without impossible spikes;
5. generate bounded loopback and, where practical, physical-interface traffic and confirm network rates change;
6. verify loopback does not create aggregate capacity utilization;
7. stop and restart the daemon and confirm first post-start counter observation re-warms rather than inheriting stale rates.

Do not add this live smoke to CI. Existing macOS/Windows CI remains compile/unit-test truth for those platforms.

## Acceptance criteria

Plan 109 is complete only when:

1. Linux CPU frequency uses CPUFreq policy data with hardware-current preference, scaling-current fallback, and logical-CPU weighting.
2. Windows CPU frequency uses documented `ProcessorInformation`/`CurrentMhz` data.
3. macOS current frequency remains absent unless a supported unprivileged public source is found; no privileged/undocumented workaround is introduced.
4. Disk throughput is derived from native cumulative byte/sector counters using actual monotonic elapsed time.
5. Disk aggregate accounting avoids obvious parent/child double counting and stays separate from filesystem-capacity aggregation.
6. Per-drive I/O association is optional and never fabricated when storage mapping is ambiguous.
7. Network throughput is derived per interface from cumulative native byte counters.
8. Directional current link capacities are captured where supported.
9. Loopback can appear in detail but never contributes aggregate capacity.
10. Linux common master/slave arrangements do not double-count aggregate capacity.
11. Counter resets, hotplug, and daemon restart re-baseline instead of producing spikes.
12. Failure of any optional metric family does not fail an otherwise valid Ready daemon snapshot.
13. No external command, privilege escalation, persistent history, or daemon-version transport is added.
14. Local verification and Ubuntu live smoke pass; existing native-platform CI remains green.
