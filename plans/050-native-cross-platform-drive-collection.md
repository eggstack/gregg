# Phase 50: native cross-platform drive collection

Status: completed.

## Objective

Implement best-effort native enumeration of eligible mounted local filesystems on Linux, macOS, and Windows, then publish those entries through the Phase 49 v2 drive representation.

This phase is collector work only. It must preserve the existing single sampling cadence, cached HTTP serving model, readiness semantics, and platform-specific source seams.

The result for each eligible drive is:

```text
public name
used bytes
total bytes
```

Aggregate values remain client-derived. TUI behavior remains out of scope.

## Dependencies and execution position

Depends on Phase 49 finalizing:

- `DriveMetrics` semantics;
- optional/unavailable collection behavior;
- list/name bounds;
- `CollectedMetrics` carriage;
- v2 serialization.

Must complete before Phase 52 can claim cross-platform condensed/expanded behavior and before Phase 53 closes the roadmap.

## Governing invariants

1. A drive means an eligible mounted local filesystem volume, not a physical disk.
2. No external command is executed.
3. Collection runs inside the existing sample and is served from cached snapshots.
4. Remote/network, pseudo, optical, RAM-backed, unknown, and unready storage is excluded.
5. Duplicate views of the same filesystem do not inflate aggregate totals.
6. Individual drive failures are skipped.
7. Top-level enumeration failure yields `drives = None` but does not fail CPU/memory readiness.
8. Successful enumeration with no eligible drives yields `Some(Vec::new())`.
9. Results are deterministic: valid, deduplicated, and sorted by public name.
10. Native unsafe code remains contained in the smallest platform module with owned safe outputs.
11. Native CI asserts structural invariants, not runner-specific topology.
12. No configurable filtering, polling interval, mount watcher, or storage graph is introduced.

## Scope

### In scope

- Linux `/proc/self/mountinfo` parsing and native filesystem statistics;
- Linux filtering, escape decoding, representative-name selection, and deduplication;
- macOS mounted-filesystem enumeration through the existing FFI trait;
- macOS filtering and duplicate suppression;
- Windows logical-drive enumeration through the existing Windows source trait;
- Windows drive-type filtering and capacity arithmetic;
- source mocks/fixtures and pure normalization tests;
- collector wiring into `CollectedMetrics.drives`;
- short native structural tests using existing CI jobs;
- collector documentation notes.

### Out of scope

- physical disk model/serial/health;
- SMART or NVMe telemetry;
- partitions, RAID, LVM, APFS container graphs, Storage Spaces, or volume-manager topology;
- filesystem labels, UUIDs, types, flags, mount source, inode counts, I/O throughput, or IOPS;
- network filesystem monitoring;
- mount management;
- drive aliases or user filtering;
- asynchronous enumeration or a second cadence;
- changes to API/TUI semantics defined in other phases;
- new CI workflows or evidence artifacts.

## Shared collector contract

Each platform implementation should expose a small internal result equivalent to:

```rust
fn collect_drives(&self) -> Result<Vec<DriveMetrics>, CollectError>
```

The exact method may live on an existing source trait or a platform helper, but production and tests must share the same filtering/normalization path.

At the outer collector level:

```rust
let drives = match self.source_or_helper.collect_drives() {
    Ok(drives) => Some(drives),
    Err(error) => {
        tracing::debug!(kind = ?error.kind, "drive collection unavailable");
        None
    }
};
```

Do not propagate drive-only failure through `SystemCollector::sample()` once core CPU/memory/load collection has succeeded.

Logging requirements:

- debug-level only for expected inaccessible/disappearing volumes;
- no repeated warning spam each sampling interval;
- no raw private native error chain in HTTP output;
- enough local context in logs to identify the skipped public volume where safe.

If repeated debug logs become noisy, prefer one summary count per sample rather than a cache/suppression framework.

## Workstream A: shared normalization helpers

Before platform wiring, add small pure helpers where useful:

```text
validate/convert native total/free values
compute used = total - free
skip zero total
sort by public name
deduplicate by platform identity
enforce Phase 49 entry/name bounds
```

Do not create a cross-platform `DriveProvider` framework unless the existing `SystemCollector` and source traits cannot express the work. Platform identity used for deduplication is internal and must not enter the wire type.

Required shared behavior:

- checked arithmetic;
- lossy native strings are either rejected or converted according to an explicit platform policy;
- duplicate public names are resolved deterministically;
- entries exceeding protocol bounds are skipped or make top-level drive collection unavailable according to one documented policy;
- entry-count overflow is handled deterministically before serialization.

Preferred bound policy:

- skip invalid individual names/values;
- sort and deduplicate;
- if more than the protocol maximum remain, retain the first entries in deterministic name order and emit one debug summary.

This avoids failing the entire metric family on a pathological mount namespace while preserving bounded output.

### Workstream A acceptance criteria

- [x] Shared arithmetic cannot underflow or wrap.
- [x] Output is deterministic and protocol-valid.
- [x] Bound handling is documented and tested.
- [x] No generic plugin/provider subsystem is introduced.

## Workstream B: Linux mount enumeration

### Source

Read `/proc/self/mountinfo` through the Linux source abstraction. Add a production path and fixture override in the same style as existing procfs paths.

Do not use:

```text
df
findmnt
lsblk
mount
blkid
```

### Parsing

Parse the mountinfo grammar sufficiently to obtain:

- mount ID;
- parent ID if useful for deterministic representative selection;
- `major:minor` filesystem device identity;
- root within filesystem;
- mount point;
- optional fields, skipped until the `-` separator;
- filesystem type;
- mount source.

Decode mountinfo octal escapes at least for:

```text
\040 space
\011 tab
\012 newline
\134 backslash
```

Reject malformed records individually. A malformed line should not necessarily discard all otherwise valid mounts; use a deterministic parse-error policy and test it.

### Filtering

Maintain a small explicit classification helper. Exclude known non-capacity/pseudo/memory/network filesystems such as representative families:

```text
proc, sysfs, devpts, cgroup, cgroup2, securityfs, debugfs, tracefs,
configfs, pstore, efivarfs, mqueue, hugetlbfs, bpf, fusectl,
tmpfs, devtmpfs, ramfs, overlay where it duplicates an underlying host view,
nfs, nfs4, cifs, smb3, sshfs/fuse.sshfs, 9p, ceph, glusterfs, afs
```

The implementation need not build an exhaustive global filesystem registry. Use a concise allow/exclude policy suitable for Gregg's local-host intent:

- explicitly exclude known pseudo/memory/network types;
- accept remaining local filesystem types if native stat succeeds and total is positive;
- document the list as intentionally conservative and extensible through code changes, not runtime config.

Container-specific caution:

- an overlay root may be the only meaningful root visible inside a container, but Gregg targets local LAN host deployment rather than containerized storage accounting;
- prefer excluding overlay duplicates when an underlying filesystem is also visible;
- do not add container namespace discovery.

### Deduplication

Use `major:minor` as the primary filesystem identity. Bind mounts and repeated mount points with the same identity must count once.

For duplicate identity, select a representative public name deterministically:

1. prefer root mount `/`;
2. otherwise prefer the mount whose filesystem root is `/`;
3. otherwise prefer the shortest mount-point path;
4. break ties lexicographically.

If a filesystem type legitimately reports unstable or synthetic `0:0`, combine identity with filesystem type/source/root or use a documented fallback key. Test the fallback.

### Native capacity query

Call `statvfs` or the smallest equivalent native API for the chosen representative mount point.

Derive:

```text
total_bytes = blocks * fragment_size
free_bytes  = free_blocks * fragment_size
used_bytes  = total_bytes - free_bytes
```

Choose `f_frsize` when nonzero, with a documented fallback to `f_bsize` if required. Use checked multiplication. Use total free blocks, not privilege-dependent caller-available blocks, so `used + available == total` for cross-platform aggregate arithmetic.

Contain unsafe FFI in a narrow module with:

- a NUL-safe path conversion policy;
- initialized output structure;
- return-code validation;
- owned numeric output;
- no pointer escape.

A small target-specific `libc` dependency is acceptable if it reduces handwritten ABI risk and remains compatible with the workspace MSRV. Do not add `sysinfo` or a broad Unix abstraction crate solely for this call.

### Linux tests

Pure fixtures must cover:

- root ext4/xfs/btrfs entry;
- separate `/home`;
- bind mount duplicate;
- escaped mount point;
- pseudo/memory/network exclusions;
- malformed optional fields before separator;
- malformed line among valid lines;
- duplicate representative selection;
- synthetic `0:0` fallback;
- zero-capacity native result skipped;
- stat failure for one mount skipped;
- deterministic sorted output;
- maximum-entry bound behavior.

One native smoke may call the production helper and assert only structural invariants.

### Workstream B acceptance criteria

- [x] Linux reads mountinfo and native filesystem stats without commands.
- [x] Escape decoding, filtering, and deduplication are fixture-tested.
- [x] Bind mounts do not inflate totals.
- [x] Per-mount errors are skipped.
- [x] Output is valid and sorted.

## Workstream C: macOS mounted-volume enumeration

### Existing seam

Extend `MacNativeQueries` with an owned result such as:

```rust
fn mounted_filesystems(&self) -> Result<Vec<RawMountedFilesystem>, CollectError>;
```

A raw record should contain only what filtering and arithmetic require:

```text
mount point
filesystem type
filesystem identity/fsid or stable equivalent
flags identifying local/readiness where available
total blocks
free blocks
block size
```

Do not expose raw C structs outside `ffi.rs`.

### Native API

Use `getmntinfo`, `getfsstat`, or the existing platform-compatible native mounted-filesystem API. Validate returned counts and buffer boundaries. Convert names to owned Rust strings inside the FFI module.

### Filtering

Include:

- `/`;
- ordinary local user-visible mounted volumes;
- available local removable volumes under `/Volumes/...`.

Exclude:

- network mounts;
- devfs and other pseudo/memory-backed filesystems;
- hidden/system helper mounts that duplicate the user-visible root/data filesystem;
- unready or zero-capacity volumes;
- duplicate filesystem identities.

Keep the rules compact and based on native type/flags/mount path. Do not attempt to fully model APFS roles or containers.

### APFS semantics

Document and test the intended boundary:

- deduplicate repeated views of the same filesystem identity;
- exclude common hidden/system helper mounts;
- aggregate remains the sum of displayed mounted filesystem capacities;
- separately mounted APFS volumes can share container free space, so the aggregate is not guaranteed to equal unique physical-device capacity;
- resolving container topology is explicitly out of scope.

### Capacity arithmetic

Use checked block-size multiplication and total-free subtraction. Prefer total free blocks over caller-specific available blocks for consistency with Linux/Windows aggregate arithmetic.

### macOS tests

Mock tests must cover:

- root volume;
- visible `/Volumes/Backup` volume;
- removable local volume;
- network exclusion;
- pseudo/helper exclusion;
- duplicate fsid suppression;
- zero total and arithmetic overflow;
- invalid/native string conversion behavior;
- per-record failure/invalidity skipped;
- deterministic sorting;
- enumeration failure -> `None` at collector boundary.

One existing macOS native job may assert structural invariants only.

### Workstream C acceptance criteria

- [x] Native mounted volumes are collected through the existing FFI trait.
- [x] Unsafe pointers/structs do not escape `ffi.rs`.
- [x] Network/helper/duplicate mounts are excluded.
- [x] APFS limitation is documented without adding topology machinery.
- [x] Output is valid and sorted.

## Workstream D: Windows logical-drive enumeration

### Existing seam

Extend `WindowsSource` with a method such as:

```rust
fn logical_drives(&self) -> Result<Vec<RawLogicalDrive>, CollectError>;
```

Production implementation remains inside the current unsafe Windows source module. Tests extend `MockWindowsSource` with drive records and an enumeration-error flag.

### Native APIs

Use the narrow Windows API set:

```text
GetLogicalDriveStringsW
GetDriveTypeW
GetDiskFreeSpaceExW
```

Implementation requirements:

- handle the multi-string buffer correctly;
- preserve canonical root paths with trailing backslash;
- validate API return values;
- use UTF-16 conversion with an explicit invalid-data policy;
- do not retain native pointers;
- bound buffer allocation and retry once if the required size changes;
- avoid probing network drives.

### Filtering

Include:

- `DRIVE_FIXED`;
- `DRIVE_REMOVABLE` only when capacity query succeeds and total is positive.

Exclude:

- `DRIVE_REMOTE`;
- `DRIVE_CDROM`;
- `DRIVE_RAMDISK`;
- `DRIVE_UNKNOWN`;
- `DRIVE_NO_ROOT_DIR`;
- any unready drive whose capacity query fails.

### Arithmetic

From `GetDiskFreeSpaceExW`, use total bytes and total free bytes:

```text
used_bytes = total_bytes - total_free_bytes
```

Do not use caller-available bytes for the wire because quotas can make it inconsistent with total free space and cross-platform aggregate semantics.

### Windows tests

Mock/pure tests must cover:

- fixed C drive;
- second fixed drive;
- ready removable drive;
- unready removable drive skipped;
- network/optical/RAM/unknown exclusions;
- zero total skipped;
- free greater than total rejected/skipped;
- Unicode-safe root handling where applicable;
- duplicate root suppression;
- deterministic sorting;
- top-level enumeration failure -> `None` at collector boundary.

One existing Windows native job may assert structural invariants only.

### Workstream D acceptance criteria

- [x] Windows uses native logical-drive and capacity APIs.
- [x] Network/optical/RAM/unready drives are not probed or published.
- [x] Used bytes are based on total free bytes.
- [x] Existing Windows source mocks remain deterministic.
- [x] Output is valid and sorted.

## Workstream E: integrate with platform collectors

For Linux, macOS, and Windows `sample()` implementations:

1. collect existing core metrics exactly as before;
2. collect drives through the new helper/source method;
3. map drive-only top-level failure to `None`;
4. attach the result to `CollectedMetrics`;
5. preserve existing CPU warming/counter-reset behavior.

Drive collection timing relative to CPU baseline:

- the first sample may continue returning the existing warming error before publishing any snapshot;
- do not create an independent drive-only snapshot during warming;
- after readiness, drive unavailability must not change readiness.

Do not duplicate drive collection for v1/v2 conversion. The sampler receives one `CollectedMetrics` value and Phase 49 conversion handles the rest.

### Workstream E acceptance criteria

- [x] All supported collectors populate `CollectedMetrics.drives` best-effort.
- [x] Core collector failures retain existing semantics.
- [x] Drive-only failures do not transition readiness to failed.
- [x] No second sample/cadence/request path exists.

## Workstream F: native behavior and performance bounds

Gregg's daemon is lightweight and commonly runs on SBCs. Keep collection simple:

- one mount/drive enumeration per existing sample;
- no directory traversal;
- no spawning;
- no network access;
- no persistent mount graph;
- bounded vector/string allocation;
- no blocking retry loops.

Measure only enough to detect obvious regression:

- unit tests should not sleep;
- a local debug/release run should show stable sampling and no log flood;
- do not add benchmarks unless a specific hot-path regression appears;
- do not add configurable drive sampling intervals.

If per-second enumeration proves materially expensive on a supported platform during implementation, the only permitted optimization in this phase is a small in-collector time-based reuse of the last successful drive list using the existing sample clock/cadence assumptions. Do not implement such a cache preemptively, and do not add mount invalidation machinery. Record the measured reason and use a short fixed refresh multiple.

### Workstream F acceptance criteria

- [x] Normal operation performs bounded local native calls only.
- [x] No new task/thread/process is introduced.
- [x] No cache exists without measured justification.
- [x] Repeated failure does not flood warning/error logs.

## Expected files

Likely change surface:

```text
crates/greggd/Cargo.toml                         # only if a narrow target dependency is needed
crates/greggd/src/collector/mod.rs
crates/greggd/src/collector/linux/mod.rs
crates/greggd/src/collector/linux/source.rs
crates/greggd/src/collector/linux/drives.rs      # preferred new focused module
crates/greggd/src/collector/linux/tests.rs
crates/greggd/src/collector/macos/mod.rs
crates/greggd/src/collector/macos/ffi.rs
crates/greggd/src/collector/macos/tests.rs
crates/greggd/src/collector/windows/mod.rs
crates/greggd/src/collector/windows/source.rs
crates/greggd/src/collector/windows/tests or inline tests
architecture/protocol.md or focused collector notes
```

Do not modify TUI files.

## Implementation sequence

1. Add shared drive-result normalization/bound helpers if needed.
2. Implement Linux pure mountinfo parser and fixtures.
3. Add Linux native capacity query and collector wiring.
4. Extend macOS raw record/source trait and mocks.
5. Add macOS FFI enumeration, filtering, and collector wiring.
6. Extend Windows raw record/source trait and mocks.
7. Add Windows native enumeration, filtering, and collector wiring.
8. Add cross-platform `CollectedMetrics`/sampler tests for `Some`, empty, and `None`.
9. Run focused crate tests on the current host.
10. Push and use existing native CI jobs for platform compilation/tests.
11. Update concise collector semantics documentation.

Do not combine the three platforms into one large refactor. Land each behind the same Phase 49 contract and keep the tree compiling after each platform change.

## Required verification

Focused local checks:

```text
cargo fmt --all -- --check
cargo test -p greggd collector --all-features
cargo test -p gregg-protocol --all-targets --all-features
cargo clippy -p greggd --all-targets --all-features -- -D warnings
```

Use the existing ordinary CI workflow for native macOS and Windows compilation/tests. No exact topology assertions and no additional workflow are required.

Suggested native structural assertion helper:

```text
for every returned drive:
  name is nonempty
  total_bytes > 0
  used_bytes <= total_bytes
names are sorted
dedup key count equals output count where observable
```

## Phase acceptance criteria

Phase 50 is complete only when:

- [x] Linux, macOS, and Windows collectors publish eligible local filesystem entries through the Phase 49 v2 model.
- [x] Linux uses mountinfo plus a native stat API and executes no commands.
- [x] Linux bind/repeated mounts are deduplicated.
- [x] macOS uses the existing contained FFI source seam.
- [x] macOS excludes network/helper/duplicate mounts and documents APFS aggregate semantics.
- [x] Windows uses logical-drive/type/free-space native APIs through the existing source seam.
- [x] Windows excludes remote, optical, RAM, unknown, and unready drives.
- [x] Every published entry has nonempty name, positive total, and used <= total.
- [x] Output is deterministic and bounded.
- [x] Individual invalid/inaccessible volumes are skipped.
- [x] Top-level drive enumeration failure yields unavailable drive data without failing core readiness.
- [x] One sample cadence and cached HTTP path remain intact.
- [x] Existing native CI jobs pass without new workflow/evidence infrastructure.
- [x] No TUI, physical-storage topology, configuration, history, or alerting scope was added.

## Handoff guidance for a smaller implementation model

- Implement and test Linux parsing before touching FFI.
- Keep raw native records owned and minimal.
- Use the existing macOS/Windows mock traits; do not bypass them in production code.
- Treat filtering as explicit pure functions so tests do not depend on host topology.
- Never fail the entire sample solely because drives are unavailable.
- Do not solve APFS containers, Linux volume managers, or Windows storage pools.
- Do not add `sysinfo` or shell commands for convenience.
- Stop if implementation starts adding a mount watcher, cache invalidation service, or configurable filesystem registry.
