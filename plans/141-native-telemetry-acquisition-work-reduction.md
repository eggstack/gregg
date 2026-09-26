# Plan 141: native telemetry acquisition work reduction

Status: planned.

Depends on: Plan 138 and the completed `gregg-host` extraction/qualification baseline from Plans 132-137.

Blocks: Plan 142.

## Objective

Reduce repeated native filesystem/FFI work in `gregg-host` while preserving exact metric meaning, first-sample/reset behavior, optional-family isolation, current-frequency/counter freshness, hotplug/topology visibility, and platform capability truth.

This plan is deliberately stricter than a generic caching pass. No dynamic telemetry field may gain an arbitrary stale window merely to reduce syscalls.

## Phase A: establish source-call accounting

Before changing acquisition, add deterministic test-source counters or equivalent instrumentation for the current hot paths.

At minimum account for Linux:

- `/proc/stat`;
- `/proc/loadavg`;
- `/proc/meminfo`;
- CPUFreq root enumeration;
- CPUFreq policy membership reads;
- CPUFreq current-frequency reads;
- `/sys/block` enumeration;
- per-device `slaves` enumeration;
- per-device `stat`;
- `/proc/net/dev`;
- per-interface flags/speed/operstate/master metadata.

Record representative call counts for:

- one warmup sample;
- one steady sample with unchanged topology;
- CPU policy/core-count change;
- network interface add/remove;
- block-device add/remove/topology change where the fixture supports it.

Use deterministic fixture counts as the primary evidence, not strace timing in CI.

## Required candidate 1: Linux CPUFreq structural cache

Split CPUFreq policy structure from current frequency.

Per sample:

- still enumerate the policy root so policy creation/removal is visible immediately;
- still read `cpuinfo_cur_freq` or `scaling_cur_freq` for every eligible policy;
- reuse parsed membership/weight only when the policy set and relevant logical-core-count state are unchanged;
- invalidate/re-read membership when policy identity set changes or logical-core count changes.

Do not cache the current frequency.

Preserve:

- preference for `cpuinfo_cur_freq`;
- fallback to `scaling_cur_freq`;
- affected_cpus -> related_cpus fallback;
- weighted policy average;
- malformed-policy isolation;
- overflow behavior.

Add fixtures proving steady-state membership reads disappear while policy/core changes force refresh and produce the same values as a cold query.

## Required candidate 2: macOS immutable page-size cache

`vm_info64()` currently obtains host page size through Mach on every sample.

Cache only a successful host page-size result using an MSRV-safe std primitive or collector-owned immutable value.

Requirements:

- a failed first page-size query must remain retryable;
- no panic poisoning;
- no public `FfiNativeQueries` construction/API break;
- RawVmStats and mock behavior remain unchanged;
- Intel and Apple Silicon native tests remain authoritative.

This is an immutable process/host property; no freshness tradeoff is accepted or required.

## Measured candidate 3: Linux network metadata consolidation

The current implementation efficiently gets interface identity plus counters from one `/proc/net/dev` read, then performs several sysfs reads per interface for link metadata.

Investigate a native one-dump link-info path, preferably rtnetlink `RTM_GETLINK`, to obtain dynamic flags/operational/master information without per-interface sysfs reads.

Retain the candidate only if it proves exact current semantics for:

- interface add/remove visibility;
- loopback classification;
- operational state;
- master/slave aggregate membership;
- 64-bit counters or continued use of `/proc/net/dev` counters;
- capacity behavior (speed may remain a separate native/sysfs read if no equivalent bounded source is used);
- deterministic ordering;
- source failure isolation.

Do not add a broad netlink framework dependency solely for this optimization. A small contained native source implementation is acceptable only if complexity and binary footprint remain justified.

If exact dynamic semantics cannot be demonstrated, record RETAIN CURRENT NETWORK METADATA.

Never replace the single `/proc/net/dev` read with per-stat sysfs files; kernel documentation notes that per-stat sysfs reads are inefficient for multiple statistics.

## Measured candidate 4: Linux block counter/topology separation

Investigate reducing repeated block-device enumeration/stat work, including a possible single counter source such as `/proc/diskstats`, while preserving Gregg's current one-accounting-layer policy.

The candidate must preserve:

- loop/ram/zram exclusions;
- layered-device/slave exclusion;
- device identity/name;
- 512-byte sector conversion semantics if the source reports sectors;
- immediate device add/remove visibility;
- topology changes that can occur without a device-name-set change;
- deterministic order;
- counter reset/rebaseline behavior.

A simple cache of `slaves` state keyed only by device name is not sufficient because device-mapper topology can change while the name remains present.

If there is no low-complexity invalidation mechanism that retains exact semantics, record RETAIN CURRENT DISK TOPOLOGY.

## Optional mechanical candidate: baseline-retention scratch allocation

After native I/O work is measured, inspect `CounterBaselines::retain_ids()`, which currently builds a temporary `HashSet<&str>` per family/sample.

Only change it if allocation remains material. Preserve the public method and all disappearance/reappearance/reset behavior; an additive generation-mark implementation is acceptable internally if simpler than retaining scratch storage.

Do not trade collision safety for hashed-ID shortcuts.

## Platform boundaries

Do not alter FreeBSD formulas/sources established by Plans 136-137 unless a shared mechanical change is demonstrably behavior-neutral.

Do not alter Windows telemetry semantics.

Do not move `Instant::now()` ownership in this plan.

## Measurement

Record:

- deterministic before/after source-call counts;
- optional release-mode sample-loop comparison on Linux/macOS;
- stripped `greggd` size;
- any added native code/dependency footprint.

No syscall-count threshold belongs in ordinary CI after closure; fixture assertions should lock only intentional structural behavior.

## Verification

~~~text
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --all-targets --all-features -- collector
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Final qualification must use the existing Linux, macOS arm64, macOS Intel, Windows, MSRV 1.89, and FreeBSD native jobs.

## Acceptance criteria

- [ ] Deterministic source-call accounting exists for the targeted Linux hot paths.
- [ ] Steady Linux CPUFreq sampling avoids repeated structural membership reads while current frequency remains live.
- [ ] CPU policy/core changes invalidate CPUFreq structure without a stale-value window.
- [ ] macOS page size is not re-queried after a successful immutable value is established.
- [ ] A failed macOS page-size read remains retryable.
- [ ] Any retained network consolidation preserves immediate link/topology semantics; otherwise the closure explicitly records RETAIN CURRENT NETWORK METADATA.
- [ ] Any retained disk consolidation preserves immediate device/topology/accounting semantics; otherwise the closure explicitly records RETAIN CURRENT DISK TOPOLOGY.
- [ ] Counter warmup/reset/disappearance/reappearance behavior is unchanged.
- [ ] Core readiness and optional-family isolation are unchanged.
- [ ] No arbitrary TTL is introduced for dynamic metrics.
- [ ] No protocol, capability, public model, or supported-platform regression occurs.
- [ ] Existing native CI and MSRV remain green.

## Explicit non-goals

Do not include:

- generic Unix collector abstraction;
- protocol changes;
- new telemetry families;
- async conversion of `gregg-host`;
- eBPF/ETW/DTrace;
- shell-command scraping;
- privilege escalation;
- netlink dependency adoption without measured justification;
- FreeBSD/Windows semantic cleanup unrelated to structural work reduction.

## Handoff note

Begin with call-count instrumentation and CPUFreq/page-size changes. Treat network and disk work as retain-or-reject candidates with correctness gates; do not start by inserting a TTL cache.
