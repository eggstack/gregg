# Phase 067: truthful drive capacity semantics

Status: planned.

Depends on: Plan 066.

## Objective

Correct Gregg's drive-capacity model so the TUI can report both bytes used by the filesystem and bytes available to the daemon identity without assuming they are complements. Preserve v1 and existing v2 compatibility through one additive optional field; do not create schema v3.

## Problem statement

The current platform collectors retain total bytes and total free bytes, then derive:

```text
used_bytes = total_bytes - free_bytes
available_bytes = total_bytes - used_bytes
```

That loses the distinction between total free space and caller-available space. POSIX `f_bfree` and `f_bavail` may differ because of reserved blocks or quotas. Windows `GetDiskFreeSpaceExW` likewise returns caller-available bytes separately from total free bytes. The current v2 record cannot represent both values.

The corrected invariant is:

```text
used_bytes      = total_bytes - total_free_bytes
available_bytes = bytes available to the daemon/service identity
```

`used_bytes + available_bytes` may be less than `total_bytes`. Do not force equality.

## Scope

### In scope

- Add `available_bytes: Option<u64>` to `gregg_protocol::v2::DriveMetrics`.
- Accept old v2 JSON with no `available_bytes`.
- Emit explicit availability from new Linux, macOS, and Windows daemons.
- Preserve `used_bytes` as total filesystem allocation, not `total - caller_available`.
- Normalize old records with `total_bytes - used_bytes` as a compatibility fallback.
- Aggregate used, total, and effective available independently.
- Update only directly affected tests, examples, fixtures, and active documentation.

### Out of scope

- A new protocol major version.
- Filesystem quota inventory or user-specific quota APIs.
- SMART, physical disk, partition, RAID, volume-group, or storage topology data.
- Root-versus-user comparison, reserved-block display, or a new TUI row.
- Changing drive eligibility, deduplication, ordering, or the 32-entry bound unless required by a discovered correctness defect in touched code.
- Treating network, pseudo, or excluded filesystems differently.

## Expected files

Inspect current HEAD before editing; likely files include:

```text
crates/gregg-protocol/src/v2.rs
crates/gregg-protocol/src/validate_v2.rs
crates/gregg-protocol/src/test_support.rs
crates/gregg-protocol/tests/fixtures/*.json
crates/greggd/src/collector/drives.rs
crates/greggd/src/collector/linux/source.rs
crates/greggd/src/collector/linux/drives.rs
crates/greggd/src/collector/macos/ffi.rs
crates/greggd/src/collector/macos/mod.rs
crates/greggd/src/collector/windows/source.rs
crates/greggd/src/collector/windows/mod.rs
crates/gregg/src/normalized.rs
crates/gregg/src/ui/system_block.rs
architecture/protocol.md
architecture/collectors.md
README.md
```

Do not edit every listed file automatically. Touch only files required by the implemented data path.

## Implementation sequence for GPT-5.6 Luna

### Step 1: lock the wire contract with tests

Add the field as:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub available_bytes: Option<u64>,
```

Required compatibility tests:

1. An existing/legacy v2 fixture without the field deserializes successfully with `None`.
2. A new v2 payload with the field round-trips.
3. Re-serializing an old-style record with `None` omits the field.
4. Unknown additive fields remain governed by the existing serde policy; do not add `deny_unknown_fields` if it is absent.

### Step 2: extend validation narrowly

Add validation only for invariants that are always true:

```text
used_bytes <= total_bytes
available_bytes <= total_bytes, when present
```

Do not require:

```text
used_bytes + available_bytes == total_bytes
```

Do not require available bytes to be less than or equal to total free bytes because total free is not transmitted.

Use the existing structured violation style. Prefer one new violation kind only if the current kinds cannot accurately describe `available_bytes > total_bytes`.

### Step 3: carry total-free and available values through the shared collector seam

The existing `DriveCandidate` should carry the quantities needed to construct a truthful record. Prefer a direct shape such as:

```rust
struct DriveCandidate {
    identity: String,
    name: String,
    total_bytes: u64,
    total_free_bytes: u64,
    available_bytes: u64,
}
```

Normalization computes `used_bytes` from `total_free_bytes` and writes explicit `available_bytes`. Do not overload one `free_bytes` field with platform-dependent meaning.

Retain checked arithmetic and existing deduplication/truncation behavior.

### Step 4: correct Linux collection

Extend `RawStatvfs` to retain both:

```text
f_bfree
f_bavail
```

Use the same selected block unit already used by the collector. Validate each multiplication independently and skip only the affected candidate when values are invalid.

Required Linux tests:

- `f_bfree == f_bavail` produces the familiar complementary result.
- `f_bavail < f_bfree` produces `used = total - total_free` and a smaller independent availability value.
- overflow, zero block unit, `free > total`, or `available > total` remains safely rejected.
- mount filtering, preferred mount selection, and ordering remain unchanged.

### Step 5: correct macOS collection

Extend `RawMountedFilesystem` to retain `available_blocks` from `StatFs::f_bavail` in addition to `free_blocks` from `f_bfree`.

Convert total, total-free, and caller-available counters using checked multiplication. Preserve existing local/dont-browse/devfs/autofs filtering.

Required macOS mock tests:

- distinct free and available block values are preserved.
- native record conversion rejects invalid capacity without dropping unrelated valid drives.
- current mount ordering and deduplication remain unchanged.

### Step 6: correct Windows collection

`GetDiskFreeSpaceExW` already returns:

```text
free_bytes_available
 total_number_of_bytes
 total_number_of_free_bytes
```

Retain both free outputs in `RawLogicalDrive`. Construct used from total free and availability from caller-available bytes.

Validate each value against total. Do not substitute one for the other.

Required Windows tests should use the existing mock/source seams where possible; do not introduce a new FFI wrapper framework. Hosted Windows CI remains the native compilation/runtime truth.

### Step 7: normalize compatibility in the client

Extend `NormalizedDrive` with an effective availability value or retain the optional wire value plus a helper. For old daemons:

```text
effective_available = total_bytes - used_bytes
```

For new daemons:

```text
effective_available = available_bytes
```

Aggregate with checked additions:

```text
aggregate_used      = sum(used)
aggregate_total     = sum(total)
aggregate_available = sum(effective available)
```

Do not recompute aggregate availability from aggregate total and aggregate used when any explicit availability exists.

The existing TUI labels and layout should remain unchanged. Only the numeric value becomes truthful.

### Step 8: reconcile documentation

Document in one concise location that:

- used space is based on total filesystem free space;
- available space is what the daemon/service identity can allocate;
- reservations or quotas can make used plus available less than total;
- older daemons lack explicit availability and use a compatibility fallback.

Do not write a general filesystem-accounting essay.

## Focused verification

Run focused tests during implementation:

```bash
cargo test -p gregg-protocol drive
cargo test -p greggd collector::drives
cargo test -p greggd collector::linux
cargo test -p greggd collector::macos
cargo test -p greggd collector::windows
cargo test -p gregg normalized
cargo test -p gregg ui
```

Then run:

```bash
./scripts/check-local.sh
```

Do not add CI jobs or platform evidence files. Native macOS and Windows behavior is closed by the existing ordinary workflow after the full roadmap lands.

## Acceptance criteria

- [ ] `DriveMetrics` has one additive optional `available_bytes` field.
- [ ] Old v2 payloads without the field deserialize and render correctly.
- [ ] New payloads omit the field only when it is genuinely unavailable.
- [ ] Validation permits `used + available < total` while rejecting each value above total.
- [ ] Linux retains and distinguishes `f_bfree` and `f_bavail`.
- [ ] macOS retains and distinguishes `f_bfree` and `f_bavail`.
- [ ] Windows retains and distinguishes total free and caller-available bytes.
- [ ] `used_bytes` is derived from total free on all three platforms.
- [ ] Client aggregate availability sums explicit availability and uses the old complement only as a compatibility fallback.
- [ ] Existing drive filtering, ordering, bounds, v1 behavior, and TUI layout remain unchanged.
- [ ] Focused tests and the default local check pass.
- [ ] No schema v3, quota subsystem, storage topology feature, or new verification machinery is introduced.

## Handoff format

Report:

- files changed;
- the exact cross-platform capacity semantics implemented;
- focused and default local test results;
- any platform behavior left to ordinary hosted CI.

Do not create a separate evidence document.
