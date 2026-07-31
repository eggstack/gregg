# Roadmap: drive metrics and multi-view TUI

Status: completed with Phases 49–53.

## Purpose

Extend Gregg with mounted-local-filesystem capacity reporting and a second compact fleet view while preserving the project's narrow private-LAN monitoring model.

The work adds:

- per-drive name, used bytes, and total bytes to the universal `greggd` v2 status API on Linux, macOS, and Windows;
- aggregate drive use in the existing normal TUI;
- reliable all-system scrolling in the normal view;
- a one-row-per-system condensed view modeled on the supplied `condensed.txt` layout;
- `h`/Left and `l`/Right view cycling;
- `e` as a selected-system drive-detail toggle in both views.

The implementation must reuse the current collector, sampler, protocol-normalization, state-reducer, viewport, and Ratatui boundaries. It must not turn Gregg into a storage inventory platform, mount-management tool, historical telemetry service, or generalized dashboard.

## Problem statement

The daemon currently reports CPU, memory, load, I/O-wait, swap, and Windows commit data, but no filesystem capacity data. The normal TUI already owns a multi-system state and viewport, yet online systems consume four rows and selection changes do not always move the viewport, making a small pane appear effectively locked to one host. There is also no dense fleet summary for quickly comparing several LAN systems.

The requested behavior can be delivered with a small additive protocol surface, one native drive-enumeration seam per supported OS, two TUI view modes, and dynamic entry-height accounting. No new daemon route, service, database, background worker, configuration subsystem, or CI workflow is required.

## Governing principles

### 1. Define a drive as a mounted local filesystem

The API reports eligible mounted local filesystem volumes, not physical disks or storage-controller topology.

Representative names are:

```text
Linux:    /, /home, /mnt/archive
macOS:    /, /Volumes/Backup
Windows:  C:\, D:\
```

This keeps `used_bytes` and `total_bytes` semantically meaningful across ordinary filesystems, LVM, RAID, APFS, removable media, and Windows volumes.

### 2. Keep protocol evolution additive and v2-only

Version 1 remains unchanged. The universal `/v2/status` payload gains optional drive data. Old clients ignore the additive JSON field; new clients treat an omitted field as unavailable. Do not add a v3 endpoint, `/drives` endpoint, content negotiation, or an additional request per poll.

Because `gregg-protocol` is a published Rust crate, implementation must explicitly review public struct-literal source compatibility. Prefer a thin v2 status payload wrapper with `#[serde(flatten)]` if adding a field directly to `StatusSnapshotV2` would unnecessarily break downstream source construction.

### 3. Collect once on the existing cadence

Drive enumeration occurs inside the existing collector sample and is included in the cached v2 status payload. The HTTP server remains read-only over cached state and never performs filesystem I/O per request.

Do not add:

- a mount watcher;
- a second sampler task;
- a separate storage refresh interval;
- historical capacity tracking;
- drive include/exclude configuration;
- remote filesystem probing.

### 4. Use best-effort drive collection

One inaccessible, disappearing, or unready volume must not make CPU and memory monitoring unavailable. Skip invalid individual volumes. If top-level enumeration fails, expose drive data as unavailable and keep the rest of the snapshot healthy.

### 5. Aggregate only eligible displayed volumes

The client computes aggregate used, total, available, and percentage from the normalized drive list. The daemon does not duplicate aggregate fields.

Network, pseudo, optical, and RAM-backed filesystems are excluded. Duplicate views of the same filesystem must not inflate totals. Platform-specific filtering remains contained inside collector modules.

### 6. Expansion follows selection

`e` is one global toggle. When enabled, only the currently selected system displays individual drive rows. Moving selection moves the expanded detail to the newly selected system. Do not add a persistent per-host expansion map.

### 7. Verification remains proportionate

Use pure parser/filter/arithmetic tests, existing native CI jobs, focused state tests, and Ratatui buffer tests. Do not add workflows, evidence bundles, long-running mount tests, exact runner drive-count assertions, or release gates.

## Target product behavior

### Universal v2 API

Conceptual response addition:

```json
{
  "schema_version": 2,
  "system": { "name": "deadpool" },
  "cpu": { "usage_pct": 12.0 },
  "memory": { "used_bytes": 6600000000, "total_bytes": 16000000000, "usage_pct": 41.25 },
  "drives": [
    { "name": "/", "used_bytes": 103079215104, "total_bytes": 510027366400 },
    { "name": "/mnt/archive", "used_bytes": 152471339008, "total_bytes": 512110190592 }
  ]
}
```

Semantics:

```text
drives absent/null  = old daemon or collection unavailable
[]                   = enumeration succeeded with no eligible volumes
nonempty list        = eligible mounted local filesystems
```

Each entry must have a nonempty name, `total_bytes > 0`, and `used_bytes <= total_bytes`.

### Normal view

Online systems gain one aggregate disk row:

```text
deadpool  IO 0.0%  0.32/0.20/0.15  16c  linux
CPU   [|||||               ] 25.0% 16 cores
MEM   [||||||||            ] 41.0% 6.6 GiB/16.0 GiB
SWP   [                    ] 0.0% 0 B/4.0 GiB
DISK  [|||||               ] 25.0% 238.0 GiB used / 714.0 GiB avail
```

When selected and expanded:

```text
  /                 96.0 GiB / 475.0 GiB  20.2%
  /mnt/archive     142.0 GiB / 477.0 GiB  29.8%
```

Unavailable drive data renders `DISK —`, not `0%`.

### Condensed view

The wide form is:

```text
HOST          CPU   MEM   DISK   LOAD   IOWAIT
-----------------------------------------------
deadpool      12%   41%    25%   0.32      0.0
wolverine      4%   67%    34%   0.05      0.0
pi-kitchen    91%   95%    87%   7.43     18.2
nas           22%   84%    52%   1.10      1.5
```

Unsupported load or I/O-wait renders `—`. Narrow widths degrade by dropping lower-priority columns rather than creating horizontal scrolling.

### Key behavior

```text
j / Down    select next system
k / Up      select previous system
h / Left    previous view
l / Right   next view
e           toggle selected-system drive details
q / Esc     quit
Ctrl-R      refresh
```

With two view modes, previous and next both wrap to the other view. Actions should still be named directionally so adding a third view later would not require changing input semantics; no generalized view registry is needed now.

## Phase map

| Phase | Plan | Outcome |
| --- | --- | --- |
| 49 | `049-additive-v2-drive-protocol-and-normalization.md` | Define bounded drive wire types, preserve v1, resolve v2 public API compatibility, validate drive entries, and normalize/aggregate in the client. |
| 50 | `050-native-cross-platform-drive-collection.md` | Collect eligible mounted local filesystems on Linux, macOS, and Windows behind existing native seams. |
| 51 | `051-dynamic-viewport-and-normal-drive-rendering.md` | Correct viewport-following, introduce dynamic entry heights, and add aggregate/expanded disk rendering to the normal view. |
| 52 | `052-condensed-view-and-view-controls.md` | Add the condensed fleet renderer and `h`/`l`/arrow/`e` interaction model. |
| 53 | `053-drive-multiview-integration-and-lightweight-closure.md` | Reconcile API/docs/examples and close with existing local/native CI only. |

## Dependency graph

```text
49 -> 50
49 -> 51
50 + 51 -> 52
49 + 50 + 51 + 52 -> 53
```

Phase 51 may begin after Phase 49 freezes the normalized drive representation using fixture data; it does not need to wait for every native collector. Phase 52 depends on dynamic row accounting from Phase 51 and normalized aggregate helpers from Phase 49.

## Program scope

### In scope

- bounded v2 drive wire values;
- protocol validation and compatibility fixtures;
- client normalization and aggregate arithmetic;
- Linux mounted-local-filesystem enumeration and deduplication;
- macOS mounted-volume enumeration and filtering;
- Windows logical-drive enumeration and filtering;
- best-effort collection behavior;
- aggregate disk row in normal view;
- selected-system drive-detail rows;
- reliable all-device viewport scrolling;
- normal and condensed view modes;
- required keyboard bindings;
- compact width degradation;
- focused tests and documentation updates.

### Out of scope

- physical-disk identity, SMART, temperature, health, model, serial, partitions, RAID/LVM/APFS container graphs, Windows Storage Spaces, or storage-controller topology;
- filesystem type, mount flags, UUIDs, labels, inode counts, read/write throughput, IOPS, latency, queue depth, or wear metrics;
- network filesystem capacity;
- mount/unmount or remote administration;
- configurable filters or aliases;
- historical storage data, alerts, thresholds, trend charts, exports, or dashboards;
- mouse input;
- column sorting, searching, grouping, or user-defined layouts;
- persistence of view or expansion state;
- protocol v3, extra HTTP routes, streaming, compression, authentication, or TLS;
- new CI workflows, retained artifacts, special platform evidence, or release automation.

## Cross-platform drive rules

### Linux

- source mount topology from `/proc/self/mountinfo`;
- decode mountinfo escaped path sequences correctly;
- exclude pseudo, memory-backed, and network filesystems;
- deduplicate repeated views of the same filesystem using stable mount/device identity available in mountinfo;
- call a native filesystem-stat API for the representative mount point;
- choose a deterministic display mount point and sort by public name;
- do not invoke `df`, `findmnt`, `lsblk`, or other commands.

### macOS

- enumerate mounted filesystems through the existing contained FFI boundary;
- include `/` and ordinary local user-visible volumes, including mounted removable local media;
- exclude hidden system/helper, network, memory-backed, and duplicate mounts;
- document that aggregate totals are sums of displayed mounted volumes and may not equal unique physical APFS-container capacity;
- do not add Disk Arbitration or a generalized storage graph solely to solve APFS container topology.

### Windows

- enumerate logical drive roots with native Windows APIs;
- include fixed and currently available removable storage;
- exclude network, optical, RAM, unknown, and unready drives;
- compute `used_bytes = total_bytes - total_free_bytes` so used plus available equals total;
- expose canonical root names such as `C:\`;
- extend the existing Windows source trait and mock rather than creating a second FFI subsystem.

## Core invariants

1. V1 wire types and endpoints do not change.
2. New drive data is available through `/v2/status` on every supported daemon OS.
3. Old v2 JSON without drives remains accepted by the new client.
4. One native sample produces all cached status representations; HTTP requests never enumerate drives.
5. A drive collection failure does not invalidate otherwise healthy CPU/memory metrics.
6. No eligible filesystem is counted more than once within one host snapshot.
7. Aggregate totals are derived from exactly the drive entries displayed by the client.
8. Unsupported/unavailable drive data is visually distinct from measured zero use.
9. `j`/`k` and arrow selection keep the selected logical system visible.
10. Entry-height calculation is shared by viewport, paging, and rendering.
11. Only the selected system expands when `e` is active.
12. Rendering and state reduction perform no filesystem or network I/O.
13. No additional CI/release infrastructure is introduced.

## Lightweight validation strategy

### Deterministic local tests

- protocol serialization, old-payload compatibility, and invalid drive-entry validation;
- aggregate arithmetic including overflow-safe sums and zero totals;
- Linux mountinfo parser/filter/dedup fixtures;
- macOS and Windows source-mock filter/arithmetic tests;
- viewport movement and dynamic-height state tests;
- key mapping tests;
- Ratatui buffer tests for normal, condensed, narrow, unsupported, offline, and expanded cases.

### Existing native CI

Use the current Linux, macOS, and Windows jobs. Native tests should assert structural invariants only:

- returned names are nonempty;
- totals are positive;
- used does not exceed total;
- results are sorted/deduplicated;
- collection does not panic.

Do not assert exact drive counts, exact mount names, or exact capacities on hosted runners.

### Existing repository checks

Implementation phases should use the smallest applicable crate-level commands during development. Final closure uses the existing default local check and ordinary CI. No new full tier, qualification workflow, evidence file, or manual platform record is required.

## Risks and controls

### Risk: duplicate mount accounting inflates totals

Control: deduplicate inside each native collector before serialization and test bind/subvolume/duplicate fixtures.

### Risk: a removable or failing volume stalls/fails the daemon

Control: skip per-volume errors, exclude network mounts, keep collection synchronous and bounded to local mounted volumes, and report top-level unavailability without failing other metrics.

### Risk: APFS shared-space semantics make aggregate capacity imperfect

Control: define the aggregate as the sum of displayed mounted filesystems and document the limitation. Do not expand into container topology discovery.

### Risk: public v2 Rust type changes break downstream struct literals

Control: make an explicit wrapper-versus-field decision in Phase 49, test old JSON compatibility, and document the source-compatibility impact before publishing.

### Risk: dynamic drive rows break scrolling

Control: replace hardcoded row assumptions with one state-aware entry-height function consumed by page-size, visible-range, selection visibility, and layout.

### Risk: condensed view becomes a table framework

Control: implement one renderer with three fixed width tiers and existing formatting helpers. No user-configurable columns or generic layout abstraction.

## Program acceptance criteria

This roadmap is complete only when:

- [ ] Plans 49 through 53 meet their individual acceptance criteria.
- [ ] `/v2/status` exposes valid per-drive names, used bytes, and total bytes on Linux, macOS, and Windows.
- [ ] V1 behavior remains unchanged.
- [ ] Old v2 payloads without drive data remain readable.
- [ ] Native collectors exclude remote/pseudo/unsupported storage and prevent duplicate aggregate counting.
- [ ] Drive collection failure does not make unrelated metrics unavailable.
- [ ] Normal view displays aggregate used and available capacity.
- [ ] Normal view renders all systems that fit and scrolls logically with `j`/`k` and arrows.
- [ ] Condensed view matches the one-row fleet-summary intent of `condensed.txt`.
- [ ] `h`/Left and `l`/Right cycle views.
- [ ] `e` toggles individual drive rows for the selected system in both views.
- [ ] Unsupported metrics continue to render as unavailable rather than measured zero.
- [ ] Existing local checks and ordinary cross-platform CI pass without new workflows or retained evidence.
- [ ] Documentation accurately states drive and aggregate semantics, including the APFS mounted-volume limitation.

## Handoff rules

1. Implement only the active phase; do not opportunistically refactor unrelated protocol, collector, polling, or UI code.
2. Prefer small pure helpers and existing seams over new frameworks.
3. Do not add dependencies unless a small target-specific native binding is necessary and materially simpler than handwritten FFI.
4. Do not execute external commands for metrics collection.
5. Keep one existing sample cadence and one HTTP polling request per endpoint.
6. Treat individual drive failures as skippable and top-level drive enumeration failure as drive-data unavailability.
7. Keep the selected system visible after every action or state change that can alter row positions/heights.
8. Do not persist view/expansion state.
9. Do not add test evidence files; ordinary tests and CI status are sufficient.
10. Record genuine physical-storage-topology requests as separate future work rather than expanding this roadmap.
