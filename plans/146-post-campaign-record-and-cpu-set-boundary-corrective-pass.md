# Plan 146: post-campaign record and CPU-set boundary corrective pass

Status: complete at implementation `9651b68ac44511851a3a4dde2bd2ef516c35dd1c` with CI run `36457308977` green across all six jobs.

Depends on: completed Plans 144-145 and their settled Plans 138-143 baseline. Independent of Plan 091.

Corrects: post-closure planning/evidence inconsistencies in Plans 144-145 plus a narrow Plan-145 internal API/parser-boundary cleanup. This plan does not reopen the successful single-flight or CPUFreq online-membership architecture.

## Objective

Close the remaining post-Plan-145 cleanup without changing Gregg's protocol, telemetry meaning, sampling cadence, supported platforms, or public product behavior.

The required work is deliberately bounded to three areas:

1. reconcile Plan 144/145 status and final CI evidence;
2. remove accidental Plan-145 helper surface that is not part of Gregg's intended reusable API;
3. replace the current CPU-list "maximum ID" clamp with an exact bounded-cardinality policy that preserves Linux CPU identity semantics and fails closed instead of silently truncating data.

Current `main` before this plan is `6752ad704cd06c31bcde3ad061051ce9c999162a`. Existing CI run `36338426733` is green across Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.89, and FreeBSD native.

## Confirmed residual findings

### A. Plan 144/145 status headers are stale

Both completed plan files still begin with:

~~~text
Status: planned.
~~~

while:

- all acceptance criteria are checked;
- both files contain closed-scope records;
- `plans/README.md` marks Plan 144 complete at `808f44e`;
- `plans/README.md` marks Plan 145 complete at `e7f6256`;
- current HEAD CI `36338426733` is green.

This can make header-based tooling report a false active-plan state.

### B. Plan 145 introduced more helper visibility than needed

`CpuFreqStructuralCache` was already part of the Linux source/cache boundary and should remain available as before, but Plan 145 added helper surface around the new identity-set implementation.

Current examples include:

- public `CpuIdSet` inside the private Linux source module;
- public `CpuFreqStructuralCache::last_online()`, used only by in-module tests;
- public `CpuFreqStructuralCache::live_weight()`, with no current production caller;
- public `CpuFreqStructuralCache::intersection_count()`, although the calculation is an implementation detail.

Even where a private parent module limits practical reachability, these methods are unnecessary on an exported cache type and create avoidable API/documentation/maintenance surface.

### C. The CPU-list bound mixes CPU identity with CPU count

Plan 145 currently defines:

~~~text
MAX_CPU_ID: u16 = 8192
~~~

and `parse_cpu_list_ids(raw, max_id)` clamps range endpoints to that value.

That has three problems:

1. `0..=8192` contains 8193 identities, one more than Gregg's existing `MAX_LOGICAL_CORES = 8192` safety target;
2. documentation says IDs above the bound are treated as malformed, but the code silently truncates them;
3. Linux CPU numbers are identities, not a guaranteed dense `0..N-1` count. Kernel documentation explicitly notes CPU numbers need not be continuous, and exposes `kernel_max` separately as the maximum configured CPU index.

Therefore the safety invariant should be the maximum number of distinct CPU identities Gregg is willing to materialize, not an artificial maximum CPU number.

References:

- https://docs.kernel.org/admin-guide/pm/cpufreq.html
- https://docs.kernel.org/admin-guide/cputopology.html
- https://docs.kernel.org/kernel-hacking/hacking.html

## Required correction 1: reconcile completed-plan records

Update Plans 144 and 145 in place without rewriting their historical implementation narratives.

Required changes:

- change top-level `Status: planned.` to `Status: complete.`;
- record current HEAD qualification run `36338426733` as green across:
  - Linux;
  - macOS arm64;
  - macOS Intel;
  - Windows;
  - MSRV Rust 1.89;
  - FreeBSD native;
- preserve implementation SHAs `808f44e4dd27de1c3ae2eb3238ce627696bb95f9` and `e7f62565848b07ee99bc4070bf16ccf598b94e98`;
- replace stale "existing CI is exercised separately" wording with the actual recorded run;
- update their post-closure-follow-up sections to point to Plan 146 until this plan closes, then reconcile them to "none" with Plan-146 closure evidence.

Do not rewrite Plan 139/141 historical closure records beyond any already-added follow-up note.

## Required correction 2: narrow Plan-145 helper visibility

Preserve the already-existing externally relevant type/function boundary:

- `CpuFreqStructuralCache` remains usable wherever current Gregg code/tests require it;
- `ProcSource::cpu_frequency_hz_with_cache(&mut CpuFreqStructuralCache)` remains source-compatible;
- `CpuFreqStructuralCache::default()` remains available;
- Linux collector construction and protocol behavior remain unchanged.

Narrow the new Plan-145 internals:

- make `CpuIdSet` private to the Linux source implementation unless a real non-test consumer proves otherwise;
- delete `live_weight()` if it remains unused;
- make `intersection_count()` private;
- remove `last_online()` as public surface and let in-module tests inspect private cache state directly, or make a test-only helper under `#[cfg(test)]`;
- do not add a new public cpuset parser/type to compensate.

Before deletion, search all workspace consumers. If a helper has a real production consumer, keep the smallest visibility required (`pub(crate)` preferred over `pub`).

## Required correction 3: make CPU-list safety cardinality-based

### Exact policy

Replace the "maximum CPU ID" clamp with a maximum distinct-member count.

Use Gregg's existing Linux collector safety bound:

~~~text
MAX_CPU_SET_MEMBERS = MAX_LOGICAL_CORES = 8192
~~~

or one shared equivalent constant owned in a non-cyclic location.

The parser must:

- preserve numeric CPU identities rather than remapping or clamping them;
- support sparse IDs;
- accept at most 8192 distinct CPU identities;
- reject the entire list if adding a token/range would exceed the 8192-member limit;
- reject reversed ranges;
- reject numeric parse overflow;
- reject empty/non-cpulist input;
- never return a silently truncated partial set;
- never allocate or iterate proportional to an arbitrarily large declared range once the cardinality limit is known to be exceeded.

CPU identity storage should use an ordinary integer width sufficient for Linux CPU indices (for example `u32` or `usize`), not `u16` merely because Gregg limits the number of materialized members.

### Do not add a hot-path kernel_max read

Linux exposes `/sys/devices/system/cpu/kernel_max` as the kernel-configured maximum CPU index, but Plan 146 does not need a new per-sample read solely to validate trusted sysfs CPU IDs.

The resource-safety contract is bounded cardinality, not local revalidation of the kernel's own topology files.

If implementation wants to use `kernel_max` for a cold diagnostic/parser assertion, it must not add a steady-state source call or create a new failure mode for CPUFreq.

## Required correction 4: converge count-only and identity parsers

The older private `parse_cpu_list(raw, logical_cores)` still uses the passed core count as if it were a maximum numeric CPU ID.

That shares the same dense-ID assumption and is used by the Plan-145 legacy live fallback.

Within this plan:

- consolidate CPUFreq membership parsing onto one bounded identity parser where practical;
- derive weights with `.len()` from the parsed set rather than independently interpreting ranges as dense IDs;
- preserve the fallback sequence `affected_cpus -> related_cpus -> existing bounded default`;
- preserve malformed-source isolation;
- do not change current-frequency selection or weighted-average arithmetic.

A valid sparse list such as `0,2,10000` must be representable if its distinct cardinality stays within the resource bound. A range whose cardinality exceeds the bound must fail rather than truncate.

## Deterministic tests

Add/adjust focused tests for the parser and CPUFreq fallback boundary.

Required cases:

1. exactly 8192 distinct members succeeds;
2. 8193 distinct members fails cleanly;
3. a sparse high numeric ID as a singleton is preserved exactly;
4. sparse IDs across comma/whitespace/range forms retain identity and count correctly;
5. an oversized range fails without constructing a partial 8192/8193-member result;
6. a reversed range fails;
7. integer overflow input fails;
8. duplicate IDs/ranges count only distinct members and cannot bypass the cardinality bound;
9. the legacy `affected_cpus -> related_cpus` fallback produces the same weight for dense ordinary fixtures as before;
10. a sparse fallback fixture no longer drops valid identities merely because their numeric ID exceeds the online/logical CPU count;
11. Plan-145 same-cardinality online-swap, zero-online-policy, cache-clear-on-online-failure, and source-call-accounting tests remain green.

Do not add fuzzing infrastructure solely for this correction.

## Source-call and behavior invariants

Plan 146 must not change the intended Plan-145 steady-state source pattern:

~~~text
policy root:       +1/sample
cpu/online:        +1/sample
current frequency: +N/sample
related_cpus:      +0 after warm cache
affected_cpus:     +0 unless fallback
~~~

No new `kernel_max`, `possible`, or `present` read is required per sample.

Preserve:

- live `/sys/devices/system/cpu/online` identity handling;
- structural `related_cpus` caching;
- same-cardinality online membership freshness;
- zero-online policy weight 0;
- fail-closed live fallback;
- current `cpuinfo_cur_freq -> scaling_cur_freq` order;
- overflow-safe weighted average;
- policy add/remove invalidation;
- optional-family isolation;
- Plan-141 macOS page-size cache;
- Plan-141 RETAIN CURRENT network/disk/baseline decisions;
- Plan-144 ready-health single-flight implementation;
- protocol/capability/public model behavior;
- Rust 1.89 MSRV.

## Documentation/evidence cleanup

After implementation:

- update Plan 146 acceptance/closure record;
- update `plans/README.md` row/status and dependency prose;
- ensure Plan 144 and 145 top-level status lines are `complete`;
- add their final CI evidence without rewriting the original local verification;
- correct Plan-145 wording that currently says CPU IDs are "clamped to `MAX_CPU_ID = 8192`";
- update `CHANGELOG.md` only if current wording would otherwise continue to describe the incorrect clamp/public helper policy.

Do not claim a user-visible telemetry change unless a real supported sparse-CPU fixture demonstrates one; this is primarily correctness/hardening of an internal boundary.

## Verification

~~~text
cargo test -p gregg-host --lib --all-features -- linux
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --lib --all-features -- server::tests::plan144
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc -p gregg-host --no-deps
./scripts/check-local.sh
~~~

Final qualification uses the existing CI matrix only:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows;
- MSRV Rust 1.89;
- FreeBSD native.

No new workflow is required.

## Acceptance criteria

- [x] Plans 144 and 145 top-level status lines are truthful and say complete.
- [x] CI run `36338426733` is recorded as the post-145 all-platform qualification evidence.
- [x] `CpuFreqStructuralCache`'s pre-existing usable boundary remains source-compatible.
- [x] New Plan-145 identity-set helpers expose no unnecessary public surface.
- [x] Unused `live_weight()` is removed unless a real production consumer is found.
- [x] Test-only cache inspection does not require public production API.
- [x] CPU-list resource safety is defined as at most 8192 distinct identities, not numeric ID <= 8192.
- [x] Exactly 8192 distinct IDs succeeds; 8193 fails.
- [x] Oversized input fails atomically rather than being silently truncated.
- [x] Sparse CPU numbering is preserved.
- [x] Reversed/overflowing/malformed input fails without panic or unbounded allocation.
- [x] Legacy CPUFreq fallback uses the same bounded identity semantics where practical.
- [x] Plan-145 same-cardinality freshness and source-call reductions remain intact.
- [x] No extra steady-state topology/sysfs read is introduced.
- [x] No protocol, cadence, readiness, capability, platform, network, disk, or Plan-144 behavior changes.
- [x] No new dependency or unsafe code.
- [x] Rust 1.89 and all existing native CI jobs remain green.
- [x] Plan 146 closure leaves Plan 091 as the only independent active historical plan unless unrelated work lands concurrently. (One pre-existing caveat: the retired Plan 064 file still says `Status: planned` even though `plans/README.md` records the 063-065 group complete; reconciling that unenumerated stale header is out of this corrective plan's scope and is noted below.)

## Closed scope record

Completed at implementation `9651b68ac44511851a3a4dde2bd2ef516c35dd1c`:

### Record reconciliation (correction 1)

- changed Plans 144 and 145 in place to truthful `complete` status lines
  without rewriting their historical implementation narratives; both keep
  their original implementation SHAs
  (`808f44e4dd27de1c3ae2eb3238ce627696bb95f9`,
  `e7f62565848b07ee99bc4070bf16ccf598b94e98`);
- recorded CI run `36338426733` (verified green: Linux, macOS arm64,
  macOS Intel, Windows, MSRV Rust 1.89, FreeBSD native; same run HEAD
  `6752ad704cd06c31bcde3ad061051ce9c999162a`) as the post-145
  qualification evidence in both plan files, replacing the stale
  "existing CI is exercised separately" wording;
- reconciled both post-closure-follow-up sections to point at Plan 146
  and record its closure (Plan 145's note additionally owns the parser
  correction evidence); Plan 139/141 historical closure records were not
  rewritten beyond the Plan-144/145 correction notes they already carry.

### Helper-visibility narrowing (correction 2)

- `CpuIdSet` is private to the Linux source implementation; a workspace
  search confirmed no non-test consumer outside
  `crates/gregg-host/src/linux/source.rs`;
- `live_weight()` had no production caller and was deleted;
- `intersection_count()` is a private associated function;
- the public `last_online()` accessor was removed; in-module tests
  inspect the private `online` field directly, so test-only cache
  inspection needs no public production API;
- no new public cpuset parser/type was added to compensate;
- `CpuFreqStructuralCache`, `CpuFreqStructuralCache::default()`, and
  `ProcSource::cpu_frequency_hz_with_cache(&mut CpuFreqStructuralCache)`
  remain source-compatible; `LinuxCollector` construction, protocol
  behavior, and the `greggd::collector::linux` facade (`CpuFreqStructuralCache`
  was never re-exported there) are unchanged.

### Cardinality-based CPU-list safety (corrections 3-4)

- one shared bound owned by the Linux collector module,
  `MAX_CPU_SET_MEMBERS = MAX_LOGICAL_CORES = 8192`; the removed
  `MAX_CPU_ID: u16 = 8192` numeric-ID clamp is gone (8193 identities in
  `0..=8192` is fixed as a side effect);
- `parse_cpu_list_ids` preserves numeric CPU identities (`u32`) with no
  remapping or clamping, accepts at most 8192 distinct members, rejects
  the entire list when a token/range would exceed the bound, rejects
  reversed ranges, values beyond `u32`, and empty/non-cpulist input, and
  never returns a silently truncated partial set. An oversized declared
  range is rejected from its own span before any member is materialized,
  and ranges overlapping already-parsed members count only the
  identities they would actually add, so duplicates can neither inflate
  nor bypass the bound;
- the legacy count-only `parse_cpu_list(raw, logical_cores)` (which
  treated the core count as a maximum numeric ID) was deleted; the
  fallback weight derives from the parsed set's `.len()`, preserving the
  exact `affected_cpus -> related_cpus -> bounded default` sequence
  (`Some(1)` only when `affected_cpus` is unreadable and `related_cpus`
  unusable, `0` when both are readable but unusable). Dense ordinary
  fixtures keep their pre-Plan-141 weights; a sparse fixture such as
  `0,2,10000` is now representable by identity;
- no `kernel_max`/`possible`/`present` read was added per sample or
  otherwise.

### Deterministic tests (all Plan-146 required cases)

- seven new `plan146_*` tests plus the revised Plan-145 parser test cover:
  exactly 8192 distinct members succeeds / 8193 fails; sparse high-ID
  singletons and comma/whitespace/range forms preserve identity and
  count; oversized ranges fail without materializing a partial set;
  reversed ranges, `u32` overflow, `-1`/`0-`/empty input fail; duplicate
  IDs/ranges count distinct members only (a fully duplicated full-bound
  list succeeds, one fresh ID past the bound fails); the legacy fallback
  keeps dense-fixture weights, preserves the
  `affected_cpus -> related_cpus -> bounded default` order, and no longer
  drops sparse identities above the core count; a sparse structural
  fixture keeps identity weights through the structural path;
- all Plan-145 same-cardinality-swap, zero-online-policy,
  cache-clear-on-online-failure, source-call-accounting, steady-state,
  and topology-change tests remain green, as do the Plan-141
  structural-cache fixtures and the Plan-144 server tests.

Verification:

```text
cargo test -p gregg-host --lib --all-features -- linux
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --lib --all-features -- server::tests::plan144
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc -p gregg-host --no-deps
./scripts/check-local.sh
```

All pass locally (the only `cargo doc` warning is the pre-existing
`drives.rs` redundant-link note, untouched by this plan). CI run
`36457308977` at the implementation SHA `9651b68...` is green across all
six jobs: Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.89, and
FreeBSD native. No new workflow was added; the existing CI matrix is the
only final qualification.

Preserved exclusions:

- reopening Plans 138-145 architecture;
- changing CPU frequency formulas;
- CPU hotplug notification infrastructure;
- adding a `kernel_max` polling cache;
- changing Gregg's displayed logical-core count;
- cpuset/cgroup reporting;
- generic public CPU-set library/API;
- network/disk telemetry optimization;
- macOS/Windows/FreeBSD collector changes;
- protocol changes;
- EggServe/server changes;
- performance CI;
- Plan 091 work.

### Future-plan status

No future plan was waiting on Plan 146. Plans 144-146 are terminal
follow-ups to the completed 138-143 campaign; the dependency chain ends
here. Plan 091 remains in progress on its own soak evidence and is
structurally independent — nothing in this plan unblocks it and nothing
further was required to be unblocked by it.

One truthfulness note for future record work: the retired Plan 064 file
(`plans/064-status-protocol-package-correctness.md`) still begins with
`Status: planned` although `plans/README.md` records its 063-065 roadmap
group as completed with CI run `30964819950` (Plan 064 has no closed-scope
record of its own, confirming the stale header predates the Plans
workflow's current closure discipline). It was not enumerated in Plan
146's findings, so it is left untouched and flagged here rather than
fixed through scope creep.

## Handoff note (retained from planning)

Implement the parser boundary first with failing boundary/sparse-ID tests, then narrow helper visibility, then reconcile the plan headers/evidence. The implementation should be mechanically small: if the CPU-list cleanup starts changing collector architecture or adding new native reads, stop and reduce scope.

(The handoff order is what landed: the cardinality parser plus failing
boundary tests came first, visibility narrowing and the header/evidence
reconciliation followed, and no collector architecture or native read
change was needed.)

## Explicit non-goals

Do not include:

- reopening Plans 138-145 architecture;
- changing CPU frequency formulas;
- CPU hotplug notification infrastructure;
- adding a `kernel_max` polling cache;
- changing Gregg's displayed logical-core count;
- cpuset/cgroup reporting;
- generic public CPU-set library/API;
- network/disk telemetry optimization;
- macOS/Windows/FreeBSD collector changes;
- protocol changes;
- EggServe/server changes;
- performance CI;
- Plan 091 work.

## Handoff note

Implement the parser boundary first with failing boundary/sparse-ID tests, then narrow helper visibility, then reconcile the plan headers/evidence. The implementation should be mechanically small: if the CPU-list cleanup starts changing collector architecture or adding new native reads, stop and reduce scope.
