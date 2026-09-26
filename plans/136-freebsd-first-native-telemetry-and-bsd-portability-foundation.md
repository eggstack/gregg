# Plan 136: FreeBSD-first native telemetry and BSD portability foundation

Status: complete.

Depends on: completed Plans 132-135 and a qualified `gregg-host` Linux/macOS/Windows baseline.

## Objective

Add FreeBSD as the first post-extraction native backend for `gregg-host`, proving that the reusable telemetry boundary can support a fourth OS family without introducing a generic-Unix abstraction or regressing the existing Linux/macOS/Windows implementations.

This plan targets the reusable telemetry crate first.

It does not by itself promise full first-class FreeBSD support for the complete `greggd` product, rc.d service lifecycle, release binaries, bootstrap installer, or client packaging. Those product-level concerns should be planned after the collector backend is qualified.

NetBSD and OpenBSD remain explicit follow-up targets. The architecture introduced here must leave room for them without claiming they are already supported.

## Platform support baseline

Rust currently provides `x86_64-unknown-freebsd` and `aarch64-unknown-freebsd` as Tier 2 targets with host tools. Use those as the intended FreeBSD architectures for this crate.

Do not make a Tier-3 architecture part of the initial acceptance contract.

References:

- Rust FreeBSD target support: https://doc.rust-lang.org/rustc/platform-support/freebsd.html
- Rust platform tiers: https://doc.rust-lang.org/rustc/platform-support.html

## Architecture rule

Add an explicit:

~~~text
gregg-host/src/freebsd/
~~~

backend.

Do not add a broad `unix/` or `bsd/` implementation and force Linux/macOS/FreeBSD through it.

A private helper shared by multiple BSDs may be introduced later only after NetBSD or OpenBSD implementation proves actual common code.

The FreeBSD backend should mirror the established source-seam pattern:

~~~rust
pub trait FreeBsdSource: Send + Sync + Debug {
    // native raw queries
}

pub struct NativeFreeBsdSource;

pub struct MockFreeBsdSource;
~~~

Exact methods should follow the native APIs chosen below.

## Initial metric contract

The first FreeBSD backend should target the same useful host-agent families where FreeBSD exposes truthful unprivileged native data:

- identity;
- logical CPU count;
- aggregate CPU utilization;
- load averages;
- physical memory utilization;
- swap utilization where a stable unprivileged source is demonstrated;
- mounted local filesystem capacity;
- CPU frequency only if a supported source is demonstrated;
- disk I/O cumulative counters/rates;
- network cumulative counters/rates and link capacity where available.

Do not block the entire backend on an optional metric family. Unsupported/permission-limited families should be represented through `HostCapabilities`/absence, not fabricated zeros.

Windows commit remains a Windows-specific concept; do not invent a FreeBSD "commit" equivalent without a native semantic match.

## Native source research/implementation direction

### CPU

Use FreeBSD native sysctl accounting rather than parsing `top`/`vmstat`.

Research and bind the stable FreeBSD CPU-time sysctls/structures for the supported release floor. Preserve Gregg's shared delta principle:

~~~text
busy_delta / total_delta * 100
~~~

Define explicitly which FreeBSD CPU states are busy, idle, interrupt, and any wait-like category. Do not map a state to Linux `iowait` merely because the name sounds similar. Report `cpu_iowait = false` unless FreeBSD exposes a semantically comparable aggregate state and the plan is amended with evidence.

The first sample warms and any cumulative decrease/reset re-baselines.

### Load averages

Use `getloadavg(3)` or the corresponding native libc interface.

Keep raw one/five/fifteen-minute load values, matching the existing Linux/macOS semantics.

### Physical memory

Use documented FreeBSD sysctl VM counters and physical-memory metadata.

Define the memory formula from FreeBSD VM semantics, not by transplanting Linux `MemAvailable` or macOS inactive-page policy blindly.

Before implementation is accepted, document:

- total physical memory source;
- page-size source;
- free/inactive/cache/laundry/wired treatment;
- exact `available` definition;
- overflow/clamping behavior.

Cross-check against `vmstat`/documented kernel counters during native qualification, but do not shell out in production.

### Swap

Prefer a documented native system-wide swap source.

`kvm_getswapinfo(3)` exposes per-device and grand-total swap summary information through base `libkvm`. Validate unprivileged behavior on the supported FreeBSD releases before adopting it.

Reference:
https://man.freebsd.org/cgi/man.cgi?query=kvm_getswapinfo&sektion=3

If the stable unprivileged contract is not suitable across the supported floor, report swap unsupported in the first backend and record a follow-up rather than scraping `swapinfo(8)`.

### Filesystem capacity

Use native mounted-filesystem enumeration plus `statfs`/equivalent libc data.

Apply a FreeBSD-specific local-filesystem selection policy. Do not copy Linux filesystem-name blacklists if native mount flags can express locality more accurately.

Feed candidates through the shared drive normalization/bounding layer so total/free/caller-available semantics stay consistent.

Retain slow-probe isolation when capacity queries can block.

### Disk I/O

Prefer base `libdevstat`.

`devstat_getdevs()` can read through sysctl when passed a null kvm handle, exposes device-list generation changes, and provides the system interface used by tools such as `iostat`.

Reference:
https://man.freebsd.org/cgi/man.cgi?query=devstat&sektion=3

Implementation requirements:

- bind only the required native surface;
- validate libdevstat/kernel interface version where required;
- copy owned records out of native buffers;
- use stable device identity;
- use shared `CounterBaselines` for rates;
- re-baseline on generation/device replacement or cumulative decrease;
- define an aggregate accounting set that avoids known duplicate provider/consumer layers;
- bound detail records with `CollectionLimits`;
- optional devstat failure does not fail core CPU/memory sampling.

Do not invoke `iostat`.

### Network

Prefer the FreeBSD `ifmib(4)` sysctl interface.

`ifmib` exists specifically to expose management information about logical network interfaces to applications such as netstat and SNMP agents.

Reference:
https://man.freebsd.org/cgi/man.cgi?query=ifmib&sektion=4

Implementation requirements:

- enumerate the bounded interface table;
- tolerate sparse rows as documented;
- extract stable name/index, flags, operational state, cumulative byte counters, and baud/link-rate data where available;
- identify loopback from native flags/type;
- define aggregate membership without double-counting subordinate/aggregate interfaces where FreeBSD exposes the relationship;
- retain all valid interfaces as detail where useful;
- use shared rate/reset semantics;
- source failure clears the affected network baselines and remains optional.

Do not parse `netstat` or `ifconfig` output.

### Identity

Use native sysctl/libc sources for:

- hostname;
- OS family/version;
- kernel name/release;
- machine architecture;
- logical processor count.

Preserve non-empty/NUL-free identity invariants expected by the host model.

## FFI and dependencies

Prefer `libc` definitions where they accurately cover the ABI.

For FreeBSD-specific library APIs not exposed sufficiently by `libc`, use a small contained FFI module rather than adding a broad dependency with a larger portability policy.

Potential native link libraries such as `devstat` or `kvm` must be target-specific.

Do not introduce a dependency on the Rust `sysctl` crate merely as the architecture foundation because the longer-term target includes NetBSD/OpenBSD and their support differs. A small internal sysctl helper is preferable if needed.

All unsafe remains confined to documented FreeBSD source/FFI modules.

## Capability semantics

The FreeBSD collector must declare actual support at runtime/construction rather than inheriting Linux assumptions.

Expected starting shape, subject to native evidence:

~~~text
cpu_iowait: false unless proven semantically comparable
load_average: true
swap: true only if qualified native source succeeds as a supported family
memory_commit: false
drives: true
cpu_frequency: optional/likely platform-source dependent
disk_io: true when devstat source available
network: true when ifmib source available
~~~

Capability flags describe support; transient source failure is distinct from permanent unsupported status where the host model permits that distinction.

## Deterministic tests

Add mock/source-level coverage for:

- CPU first-sample warming;
- CPU delta/busy arithmetic;
- CPU counter reset/recovery;
- load values;
- memory formula and extreme page counts;
- zero/nonzero swap where supported;
- malformed/overflowing native record normalization;
- local filesystem filtering and free/available semantics;
- devstat device appearance/disappearance/generation change;
- disk counter reset;
- sparse ifmib rows;
- loopback versus aggregate membership;
- network counter reset;
- optional-family source failures preserving core sample success;
- deterministic ordering and collection bounds;
- identity validation.

Do not require a FreeBSD host for parser/arithmetic tests.

## Native FreeBSD qualification

Because GitHub's ordinary hosted runners do not provide FreeBSD, add one bounded VM-based native job or equivalent existing-community FreeBSD action only after reviewing its provenance/pinning policy.

The job should run on a current supported FreeBSD release and execute at least:

~~~text
cargo test -p gregg-host --all-features
~~~

plus a native smoke that warms the collector and validates:

- nonempty identity;
- logical cores > 0;
- finite CPU percentage after warmup;
- total memory > 0 and used <= total;
- load present;
- at least one truthful local filesystem on an ordinary VM;
- network enumeration present on the ordinary VM;
- optional disk/swap/frequency handling truthful for that environment.

Do not make nonzero traffic or disk throughput mandatory; a valid zero-rate interval is acceptable.

Initially one `x86_64-unknown-freebsd` native job is sufficient for continuous qualification. `aarch64-unknown-freebsd` must at least compile when practical before claiming architecture support; native aarch64 CI may be added later if the runner path is reliable and proportionate.

Do not duplicate the entire Gregg workspace suite inside the FreeBSD VM when only `gregg-host` is being qualified.

## Existing-platform regression gate

Adding FreeBSD must leave the current ordinary CI unchanged and green for:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows;
- Rust 1.89 MSRV.

Any shared-helper change required for FreeBSD must pass Plan-133/135 characterization on all existing platforms.

## Documentation

Update:

- `crates/gregg-host/README.md`;
- `architecture/collectors.md` or the extraction architecture document;
- `CHANGELOG.md`;
- `plans/README.md`.

State support precisely:

- Linux/macOS/Windows qualified as established;
- FreeBSD support and qualified architectures/releases exactly as tested;
- NetBSD/OpenBSD planned, not implied.

Do not claim `greggd` FreeBSD service/install/release support unless a later product-level plan implements it.

## NetBSD/OpenBSD follow-up boundary

After Plan 136, research separate implementation plans rather than copying the FreeBSD backend.

Expected areas:

- NetBSD: UVM/sysctl memory accounting and NetBSD-native disk/network sources;
- OpenBSD: release-specific sysctl/native ABI qualification, with explicit attention to Rust's Tier-3 support and OpenBSD release ABI expectations.

Do not make Plan 136 depend on those future ports.

## Acceptance criteria

- [ ] `gregg-host::freebsd` exists as an explicit backend with injectable native source seam.
- [ ] No generic Unix/BSD collector abstraction is introduced.
- [ ] FreeBSD identity, CPU, load, and physical-memory collection are implemented from native APIs with documented semantics.
- [ ] Swap is implemented only if a stable unprivileged native source is qualified; otherwise it is truthfully unsupported with a recorded follow-up.
- [ ] Local filesystem capacity uses native records and shared drive normalization/slow-probe isolation.
- [ ] Disk I/O uses native devstat/sysctl behavior without shell commands and shared reset-safe rate logic.
- [ ] Network telemetry uses native ifmib/sysctl behavior without shell commands and shared reset-safe rate logic.
- [ ] Optional metric-family failure does not fail core sampling.
- [ ] Collection bounds and deterministic ordering are preserved.
- [ ] Unsafe code is confined to documented FreeBSD FFI/source modules.
- [ ] Deterministic mock/parser tests cover reset/hotplug/sparse/error/extreme cases.
- [ ] A bounded native FreeBSD CI/smoke path proves the collector on a supported release.
- [ ] Existing Linux/macOS/Windows/MSRV qualification remains green.
- [ ] Documentation states exact FreeBSD support without implying complete `greggd` product support.
- [ ] NetBSD/OpenBSD remain explicitly deferred follow-up backends.

## Explicit non-goals

Do not include:

- FreeBSD rc.d service installation;
- FreeBSD release binaries/bootstrap installer;
- package-manager integration;
- full `greggd` product support declaration;
- client/TUI changes;
- process/GPU/sensor telemetry;
- command scraping;
- root-only collection merely to populate optional fields;
- NetBSD/OpenBSD implementation;
- generic Unix/BSD collector unification;
- changing existing Linux/macOS/Windows semantics.

## Handoff note

Implement FreeBSD as a new native backend against the settled `gregg-host` model.

Where FreeBSD accounting does not map exactly to Linux/macOS/Windows, preserve the native meaning and use capability/absence semantics rather than forcing values into a misleading cross-platform equivalence. The purpose of Plan 136 is to validate the abstraction, not to make FreeBSD imitate Linux.

## Closure record

Implemented cumulatively at `a9dab65` plus `a5624a9` plus devstat fix `43b5cf3` (Plan-136-owned files:
`crates/gregg-host/src/freebsd/{mod,source,tests}.rs`, the
`#[cfg(target_os = "freebsd")] pub mod freebsd` registration, the
`freebsd` CI job in `.github/workflows/ci.yml`, the FreeBSD backend
section in `architecture/collectors.md`, and the shared CHANGELOG entry).

Backend scope as accepted: explicit `gregg-host::freebsd` module with an
injectable `FreeBsdSource` seam (`NativeFreeBsdSource`,
`MockFreeBsdSource`); no `unix/` or `bsd/` abstraction. Identity, logical
cores, aggregate CPU (`kern.cp_time` user/nice/sys/intr/idle with the
shared busy/total delta principle; `cpu_iowait=false`, first-sample
warming, decrease/zero-delta reset with spike-free recovery), load
(`getloadavg`), and physical memory (`hw.physmem` + `hw.pagesize` +
`v_free/inactive/cache/laundry_count` with the documented
`available=(free+inactive+cache+laundry)*page_size` definition,
overflow-checked and clamped) are native. Local filesystems use
`getmntinfo`/`statfs` with `MNT_LOCAL` selection plus the shared
normalization/bounding and slow-probe isolation. Disk I/O uses base
`libdevstat` (`devstat_checkversion` gate, null-kvm `devstat_getdevs`
through sysctl, generation-aware baselines, plausibility-gated entries:
printable name, sane unit, `sequence0==sequence1`); network uses `ifmib`
integer-MIB rows with count bounds, exact-length validation,
sparse-row tolerance, printable-name gating, native
loopback/operational/capacity semantics, and the shared reset-safe rate
logic. Swap is truthfully unsupported (`swap=false`, `None`; follow-up:
validate unprivileged `kvm_getswapinfo` across the supported floor before
adopting) and CPU frequency is truthfully unsupported (follow-up: find a
validated unprivileged source); Windows commit was not imitated.
Collection bounds and deterministic ordering hold; unsafe is confined to
the documented FreeBSD source/FFI module.

Deterministic tests (mock/parser, host-independent): CPU warming/delta/
reset/recovery, load rejection, memory formula/zero/extreme cases,
identity validation, drive failure/empty semantics, disk
appearance/disappearance/reset, network sparse/loopback/capacity/reset,
ordering/bounds — all green wherever the host suite runs, and the
FreeBSD test target cross-compiles strictly
(`--target x86_64-unknown-freebsd`, `--tests`, `RUSTFLAGS="-D warnings"`).

Native qualification: new bounded `freebsd` CI job (pinned
`vmactions/freebsd-vm@v1.1.9`, FreeBSD 14.2, `cargo test -p gregg-host
--all-features`) proved its value before any metric was read: the first
version linked a `devstat_free` helper that does not exist in libdevstat
(see BUGS in man `devstat(3)`), and the VM linker rejected it. The
backend now releases the `dinfo.mem_ptr` allocation with libc `free`
after owned records are copied out (fix `43b5cf3`). The job runs the
FreeBSD-only smoke (identity, cores, finite
CPU, memory bounds, load, local filesystem, network enumeration with a
non-loopback member; zero-rate intervals valid) plus traffic-direction
proofs (a known disk write advances write counters monotonically; a
loopback ping advances `lo` counters monotonically). Byte-index
direction for devstat/ifmib fields is covered by those direction proofs;
exact per-field cross-validation against `iostat(1)`/`netstat(1)` beyond
monotonicity/direction is recorded as a follow-up, not claimed here.
Existing Linux/macOS arm64/macOS Intel/Windows/MSRV qualification is
unchanged and green in remote CI run `36219678790`.

Documentation states exact support (`x86_64-unknown-freebsd` natively
qualified; `aarch64-unknown-freebsd` compiles; no `greggd` FreeBSD
service/install/release support claimed) with NetBSD/OpenBSD explicit
follow-ups. Architecture supported: Tier 2 `x86_64`/`aarch64` only; no
Tier-3 target was added to the acceptance contract.

Acceptance: all boxes hold at the implementation SHA subject to the two
recorded follow-ups (swap-source validation, extended counter cross-check),
neither of which blocks the portability foundation. Non-goals honored:
no rc.d/packaging/binaries, no client/TUI changes, no process/GPU/sensor
telemetry, no command scraping, no root-only collection, no
NetBSD/OpenBSD code, no Unix/BSD unification, no existing-platform
semantic change.

## Follow-ups (not blockers)

1. Validate unprivileged `kvm_getswapinfo` across the supported FreeBSD
   floor; adopt it for swap or keep `swap=false` with evidence.
2. Cross-check devstat/ifmib byte-field values (beyond monotonicity and
   direction, already proven natively) against `iostat(1)`/`netstat(1)`
   on the supported releases.
3. NetBSD (UVM/sysctl) and OpenBSD (release ABI, Tier-3 attention) backends
   as separate researched plans.
