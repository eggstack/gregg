# Plan 145: Linux CPUFreq online-membership freshness corrective pass

Status: planned.

Depends on: completed Plans 138-143 at current post-`07f353f` main, specifically Plan 141's Linux CPUFreq structural cache. Independent of Plan 091 and Plan 144.

Corrects: the CPUFreq membership-invalidation gap in completed Plan 141. Plan 141 remains complete as the historical implementation record; this plan owns only the online-membership freshness correction and its source-call qualification.

## Objective

Preserve Plan 141's useful reduction in repeated CPUFreq membership reads while making policy weighting responsive to CPU online/offline changes even when:

- the `policyX` directory set is unchanged;
- the total online/logical CPU count is unchanged; and
- different CPU identities move online/offline between samples.

The correction must preserve current-frequency freshness, weighted-average semantics, malformed-source isolation, immediate policy add/remove visibility, and the no-arbitrary-TTL rule.

## Confirmed gap

The landed `CpuFreqStructuralCache` keys cached policy weights by:

~~~text
policy directory identity set + logical_core_count()
~~~

and obtains those weights from `affected_cpus` first.

Linux defines:

- `affected_cpus` as the **online** CPUs currently belonging to the policy;
- `related_cpus` as **all online and offline** CPUs structurally belonging to the policy.

The kernel also keeps policy objects across ordinary CPU offline/online transitions; bringing a CPU online inside an already-active policy does not recreate the policy object.

Therefore `affected_cpus` is dynamic state, not safe structural cache content. A same-cardinality hotplug transition can change which policies have online members without changing either the `policyX` directory set or the existing cache's logical-core-count key.

Reference: https://docs.kernel.org/admin-guide/pm/cpufreq.html

## Preferred correction

### 1. Cache structural related_cpus membership, not affected_cpus weights

Change the private CPUFreq cache from precomputed policy weights to policy membership sets derived from `related_cpus`.

Conceptually:

~~~text
CpuFreqStructuralCache {
    policies: Vec<PathBuf>,
    related: Map<PolicyPath, CpuSet>,
}
~~~

The exact compact representation may be sorted CPU IDs, ranges, or another allocation-bounded private type.

On a cold cache or policy-directory-set change:

- read and parse `related_cpus` for each eligible policy;
- store the actual CPU identities, not only a count;
- preserve deterministic policy ordering.

Policy root enumeration remains live every sample so policy creation/removal still invalidates immediately.

### 2. Read the global online CPU identity set once per sample

Read and parse:

~~~text
/sys/devices/system/cpu/online
~~~

once per CPUFreq sample.

For each cached policy, derive the live weight as:

~~~text
count(related_cpus(policy) ∩ online_cpus)
~~~

This reconstructs the documented meaning of `affected_cpus` while replacing N dynamic per-policy membership reads with one global dynamic online-set read.

Do not use only the online CPU count. Preserve identities so same-cardinality swaps are visible.

Do not reuse `std::thread::available_parallelism()` as the online-set authority: it may reflect process affinity/cgroup restrictions and cannot identify which CPUs are online.

### 3. Keep current frequency fully live

Continue reading every sample, per eligible policy:

1. `cpuinfo_cur_freq`;
2. fallback `scaling_cur_freq`.

Do not cache either value.

A policy with zero currently online members contributes zero weight and must not distort the weighted average merely because its structural `related_cpus` set is non-empty.

### 4. Fail closed to the legacy per-policy membership path

The optimization must not make CPU frequency disappear or become stale when the preferred sources are unavailable or malformed.

If:

- `/sys/devices/system/cpu/online` cannot be read/parsed; or
- a policy's `related_cpus` cannot be read/parsed reliably;

fall back for that sample/policy to the pre-Plan-141 live membership rule:

~~~text
affected_cpus -> related_cpus -> existing bounded fallback
~~~

Do not use a previously cached dynamic weight after the online-set source fails.

The implementation may decide that a failed structural refresh invalidates the whole CPUFreq cache for that sample if that is simpler and preserves existing optional-family isolation.

### 5. Treat structural-policy stability as a qualified assumption

Kernel documentation describes the policy CPU mask as part of policy-object initialization and the policy object as surviving ordinary online/offline transitions. The implementation may therefore cache `related_cpus` while the same policy objects remain represented by the same policy directory set.

However, qualification must include driver/policy recreation behavior available in deterministic fixtures:

- remove/add a policy path;
- recreate a same-numbered policy after an observed disappearance;
- ensure cache refresh occurs after the observable directory-set transition.

Do not claim detection of an unobservable remove-and-recreate event that occurs entirely between two samples.

If implementation review finds a realistic supported-kernel path where `related_cpus` mutates in place for the same live policy object and cannot be detected without rereading it every sample, prefer correctness: record the finding and RETAIN LIVE MEMBERSHIP READS rather than keep a stale structural cache.

## Parser requirements

The current CPU-list helper primarily yields a count. Add or refactor a private parser that preserves CPU identities.

It must support the Linux cpulist grammar already accepted by Gregg's fixtures, including:

- single IDs;
- comma-separated IDs;
- inclusive ranges;
- surrounding whitespace/newlines.

It must reject or isolate malformed/overflowing ranges without panicking or allocating proportional to attacker-controlled enormous CPU IDs.

Prefer a sorted/range representation or enforce the existing `MAX_LOGICAL_CORES`/bounded-source policy appropriately.

Do not broaden this into a general public cpuset crate/API.

## Deterministic tests

Add tests that fail against current Plan-141 behavior.

Required fixture scenarios:

### Same-count membership swap

Use at least two policies with different current frequencies and structural memberships so:

- sample A online set produces weights such as policy0=1, policy1=1;
- sample B has the same total online CPU count but a different identity set producing different policy weights, such as policy0=2, policy1=0;
- policy directory names remain unchanged;
- current frequencies remain unchanged.

Prove the weighted result changes immediately on sample B.

### Ordinary steady state

Across multiple steady samples:

- policy root enumeration continues;
- global `cpu/online` read advances once per sample;
- `related_cpus` reads do not advance after structural cache warmup;
- current-frequency reads do advance every sample.

### Policy topology change

Add/remove/recreate a policy path and prove structural membership is reread immediately and the result matches a cold query.

### Preferred-source failure

Prove unreadable/malformed global online-set data cannot reuse stale dynamic weights and falls back to the live legacy membership path.

Prove malformed/missing `related_cpus` for one policy retains the current bounded fallback/isolation behavior.

### Zero-online policy

Characterize a policy whose structural CPUs are all offline. It must contribute zero live weight when the global online set is authoritative.

### Collector integration

Keep a `LinuxCollector`-level test proving the cache is used through ordinary sampling, not only through a source helper.

## Source-call accounting

Extend Plan 141's `CallCounts` evidence to distinguish:

- policy-root enumeration;
- `related_cpus` structural reads;
- `affected_cpus` fallback reads;
- global `/sys/devices/system/cpu/online` reads;
- current-frequency reads.

The intended steady state is:

~~~text
policy root:       +1/sample
cpu/online:        +1/sample
current frequency: +N/sample
related_cpus:      +0 after warm cache
affected_cpus:     +0 unless fallback
~~~

Do not add syscall-count timing gates to CI.

## Compatibility constraints

Preserve:

- `ProcSource::cpu_frequency_hz()` public behavior as a cold one-shot query;
- `cpu_frequency_hz_with_cache()` return type and optional-family semantics;
- preference for hardware `cpuinfo_cur_freq`;
- `scaling_cur_freq` fallback;
- overflow-safe weighted arithmetic;
- policy ordering;
- LinuxCollector API;
- protocol/capability surfaces;
- sample cadence;
- network/disk/macOS/Windows/FreeBSD behavior;
- Rust 1.89 MSRV.

No arbitrary TTL or timer-based invalidation is allowed.

## Retain/revert gate

Retain the revised structural cache only if all are true:

1. same-cardinality online membership changes are immediately visible;
2. steady-state per-policy dynamic membership reads are reduced;
3. source failures fall back without stale cached weights;
4. policy add/remove semantics remain exact;
5. current-frequency reads remain live;
6. no new dependency or unsafe code is needed;
7. native Linux/MSRV qualification is green.

If these gates cannot be met cleanly, revert CPUFreq membership caching to the live pre-Plan-141 path and record the performance tradeoff. Correct freshness is mandatory; cache retention is not.

The macOS page-size optimization and Plan-141 RETAIN CURRENT network/disk/baseline decisions remain untouched.

## Verification

~~~text
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --all-targets --all-features -- collector
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Final implementation must pass the existing Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.89, and FreeBSD native CI jobs. Only Linux is expected to receive production-source changes.

Measure stripped `greggd` before/after on the same toolchain if the private CPU-set representation materially changes linked size.

## Acceptance criteria

- [x] The CPUFreq cache no longer treats `affected_cpus` as immutable structural weight.
- [x] Cached structural membership preserves CPU identities, not only counts.
- [x] One live global online-CPU identity read per sample drives cached-policy live weights.
- [x] Same-cardinality online CPU membership swaps change weights/results immediately.
- [x] Current CPU frequency remains sampled every cycle.
- [x] Policy add/remove/recreation observable between samples invalidates structural membership.
- [x] Online-set failure cannot reuse stale dynamic weights and uses a live fallback.
- [x] Missing/malformed per-policy structural data preserves bounded fallback/isolation.
- [x] Zero-online policies contribute zero when the global online set is authoritative.
- [x] Deterministic call accounting proves the steady-state read reduction.
- [x] No arbitrary TTL, sleep, mtime heuristic, or periodic refresh interval is introduced.
- [x] Linux collector/public/protocol/capability behavior outside the corrected weighting remains unchanged.
- [x] macOS, Windows, FreeBSD, network, disk, and baseline-retention decisions remain unchanged.
- [x] Rust 1.89 and existing native CI remain green.
- [x] Plan 141 receives only a post-closure correction note pointing here; its historical closure evidence is not rewritten.

## Explicit non-goals

Do not include:

- generic CPU hotplug monitoring;
- uevent/netlink listener infrastructure;
- cpuset/cgroup reporting;
- changing Gregg's displayed logical-core count;
- CPU utilization formula changes;
- CPUFreq driver/governor control;
- network/disk cache work;
- FreeBSD/macOS/Windows telemetry changes;
- public cpuset parsing API/crate;
- protocol changes;
- new dependencies.

## Closed scope record

Completed at implementation `e7f6256`:

- replaced `CpuFreqStructuralCache`'s precomputed weights with structural
  `related_cpus` identity sets per policy (`CpuIdSet`), refreshed only
  when the policy directory set changes or a previously-unreadable
  policy becomes readable;
- added a live `/sys/devices/system/cpu/online` read once per sample,
  storing the parsed identity set and using it to intersect each cached
  policy's `related_cpus` for the dynamic policy weight;
- added a private bounded `parse_cpu_list_ids` parser accepting single
  IDs, comma/whitespace separators, and inclusive ranges, with IDs
  clamped to `MAX_CPU_ID = 8192` so a runaway file cannot allocate
  memory proportional to its byte length;
- failed closed to the pre-Plan-141 live `affected_cpus -> related_cpus`
  membership path when the global online source is unreadable or
  malformed, clearing the cache so a stale online cannot re-enter the
  structural path until the source is readable again; the same legacy
  fallback is used per-policy when `related_cpus` cannot be read/parsed
  for one policy;
- preserved `cpuinfo_cur_freq` / `scaling_cur_freq` preference,
  `ProcSource` public API, `CpuFreqStructuralCache::default()` empty
  state for cold calls, Plan-141 topology-change behavior, Plan-124
  stale/failure semantics, macOS/Windows/FreeBSD collectors,
  protocol/capability surfaces, sample cadence, and Rust 1.89 MSRV;
- added seven `plan145_*` source tests and updated four `plan141_*`
  tests for the new fixture shape (online + related_cpus), proving:
  same-count online membership swap changes the weighted average
  immediately while structural reads stay flat; ordinary steady state
  advances the global online read and current frequency every cycle
  but holds structural reads at zero; policy add/remove invalidates
  the structural cache and matches a cold query; online-set failure
  falls back to live membership and clears the cache; restoring the
  source re-populates the cache; zero-online policy contributes zero
  live weight under the global online set; per-policy `related_cpus`
  failure uses the legacy weight without breaking the rest of the
  cache; `parse_cpu_list_ids` handles single IDs, comma/whitespace,
  ranges, and clamps to `MAX_CPU_ID`;
- updated `plan141_collector_steady_cpufreq_reuses_structure`
  (collector-level integration) to use `related_cpus` and the online
  file.

Verification:

```text
cargo test -p gregg-host --lib --all-features -- linux
cargo test -p gregg-host --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
```

All checks pass locally. Existing CI is exercised separately.

Preserved exclusions:

- generic CPU hotplug monitoring, uevent/netlink listeners, cpuset /
  cgroup reporting;
- changing Gregg's displayed logical-core count or the CPU utilization
  formula;
- CPUFreq driver/governor control;
- network/disk/macOS/Windows/FreeBSD telemetry changes;
- public cpuset parsing API/crate, protocol changes, new dependencies;
- rewriting Plan 141's historical closure evidence;
- arbitrary TTL, sleep, mtime heuristic, or periodic refresh interval;
- reopening Plan 091, Plan 137, or Plan 144.

## Post-closure follow-ups

None. Plan 145 closes the post-Plan-141 corrective work. The remaining
Plan 091 soak record is independent and unaffected.

## Handoff note

Start with the same-cardinality swap regression fixture before changing the cache. Then implement the smallest structural-membership + live-online-set design that makes that fixture pass while preserving Plan 141's source-call reduction. If the structural assumption fails qualification, prefer the live legacy membership path and close truthfully with that result.


## Post-closure correction note (Plan 146)

Post-closure review confirmed the Plan-145 online-membership architecture and CI behavior, but found three narrow cleanup items: this file's top-level status still says `planned`; final all-platform CI run `36338426733` is not recorded here; and the new CPU-set implementation exposes unnecessary helper surface while `MAX_CPU_ID = 8192` conflates numeric CPU identity with the 8192-member safety limit and silently clamps oversized ranges. Planned Plan 146 owns only that record/API/parser-boundary cleanup. Plan 145's implementation at `e7f62565848b07ee99bc4070bf16ccf598b94e98` remains complete and its CPUFreq freshness design is not reopened.
