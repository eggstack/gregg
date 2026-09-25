# Plan 128: macOS Intel disk/network collector corrective pass

Status: complete.

Depends on: completed Plans 109-111 and 114, the current post-Plan-127 daemon baseline, and the existing native macOS arm64 + Intel CI jobs. This work is independent of the remaining Plan 091 soak record.

## Objective

Correct the macOS native collection boundary so filesystem capacity and network telemetry remain truthful on both Intel and Apple Silicon Macs, including older supported Intel macOS hosts where greggd currently may publish no drive capacity and no network payload.

This is a bounded collector/verification corrective pass. Preserve the existing v2 protocol, client normalization, TUI semantics, sampling cadence, optional-telemetry failure isolation, and no-external-command policy.

The intended outcome is:

- DISK capacity is populated from a correctly bound Darwin statfs ABI when eligible local filesystems exist;
- NET telemetry is populated from correctly typed Darwin interface counters rather than an invalid getifaddrs data cast;
- 64-bit interface counters remain preferred where Darwin exposes them;
- unsupported or transient optional telemetry remains absent without making greggd unready;
- native macOS CI proves the v2 optional metric families on both arm64 and Intel instead of validating only the v1 snapshot path.

## Confirmed implementation findings

### A. getmntinfo currently owns an ABI that libc already handles

crates/greggd/src/collector/macos/ffi.rs declares a private StatFs layout and an unsuffixed getmntinfo extern.

Rust libc already exposes libc::statfs and libc::getmntinfo for Darwin. On macOS non-aarch64 targets its binding selects the getmntinfo$INODE64 symbol, matching the modern 64-bit inode statfs ABI. The current private binding bypasses that architecture-sensitive symbol selection.

That creates a concrete Intel compatibility risk: an unsuffixed legacy getmntinfo result can be interpreted through the private modern layout, causing fields such as flags, block counts, filesystem type, and mount point to be read at incorrect offsets. The collector then filters on MNT_LOCAL/MNT_DONTBROWSE and capacity validity, so a layout mismatch can collapse a real local filesystem set to an empty drive list.

Reference:
https://docs.rs/libc/latest/x86_64-apple-darwin/src/libc/unix/bsd/apple/mod.rs.html

Required direction: stop duplicating this ABI and use libc::getmntinfo with libc::statfs.

### B. getifaddrs ifa_data is currently interpreted as the wrong structure

network_interfaces() walks AF_LINK entries from getifaddrs and casts ifa_data to libc::if_data64.

Darwin's getifaddrs contract associates AF_LINK ifa_data with struct if_data, not struct if_data64. Darwin exposes the 64-bit interface statistics through the routing/sysctl interface-list-2 representation, where if_msghdr2 embeds if_data64.

libc already exposes the relevant Darwin types and constants, including:

- if_data;
- if_data64;
- if_msghdr2;
- NET_RT_IFLIST2;
- RTM_IFINFO2.

References:
https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/getifaddrs.3.html
https://docs.rs/libc/latest/x86_64-apple-darwin/libc/
https://github.com/apple/darwin-xnu/blob/main/bsd/net/if.h

Required direction: remove the getifaddrs -> if_data64 cast. Prefer NET_RT_IFLIST2 / if_msghdr2 for 64-bit counters, with a correctly typed getifaddrs / if_data compatibility fallback only where the preferred source is unavailable.

### C. current native CI does not prove these v2 families

The macOS matrix already runs on macos-15 and macos-15-intel, but the native collector smoke ultimately converts collected metrics through the v1 into_snapshot() path. It does not require a v2 StatusPayloadV2 with drives or network.

Therefore native Intel CI can remain green while the exact optional metric families in this corrective pass are absent.

Required direction: extend native macOS verification to exercise the complete v2 payload and explicitly check drive/network availability semantics.

## Scope decisions

### 1. Replace the private statfs/getmntinfo ABI

In crates/greggd/src/collector/macos/ffi.rs:

1. delete the private StatFs definition;
2. delete the private getmntinfo extern declaration;
3. call libc::getmntinfo with a *mut *mut libc::statfs;
4. keep the returned kernel-owned array lifetime contained inside mounted_filesystems();
5. copy every accepted record into RawMountedFilesystem before returning;
6. use checked conversions for block size/count fields where the libc field type requires them;
7. preserve UTF-8 validation and existing bounded record ownership;
8. preserve MNT_LOCAL and MNT_DONTBROWSE filtering at the collector layer unless native evidence proves one of those flags behaves differently on a supported host.

Do not maintain parallel Intel and arm64 StatFs definitions. libc is the platform ABI authority.

The existing mounted-filesystem-count validation should be retained/adapted for libc::statfs pointers.

### 2. Move primary network counters to NET_RT_IFLIST2

Implement a small native source inside the existing macOS FFI module using Darwin sysctl with this conceptual MIB:

~~~text
CTL_NET
PF_ROUTE
0
0
NET_RT_IFLIST2
0
~~~

Use libc::if_msghdr2 and its embedded libc::if_data64. Do not redeclare either structure.

Required parser behavior:

- perform the normal size-query + data-query sysctl sequence;
- tolerate a size race with a small bounded retry rather than an unbounded allocation loop;
- reject impossible/zero buffer sizes;
- walk variable-length messages by ifm_msglen;
- validate each message length before reading an if_msghdr2;
- accept only RTM_IFINFO2 records for interface metrics;
- use read_unaligned or another layout-safe copy where buffer alignment is not guaranteed by the parser offset;
- reject/truncate malformed messages without reading past the returned buffer;
- obtain a stable display name from the native interface index, preferably libc::if_indextoname;
- derive loopback and operational state from native flags, preserving the current aggregate-member rule unless a separate topology defect is demonstrated;
- retain ifi_ibytes / ifi_obytes as cumulative u64 counters;
- retain ifi_baudrate when nonzero as the directional capacity source;
- sort/deduplicate deterministically by stable interface identity before returning.

Do not infer interface identity from en0-style naming conventions.

### 3. Keep a safe compatibility fallback

If NET_RT_IFLIST2 is unavailable or returns a platform-level unsupported result on an older supported macOS release, fall back to getifaddrs.

The fallback must cast AF_LINK ifa_data to libc::if_data, exactly matching the documented API.

Because if_data byte counters are narrower, preserve truthful semantics:

- widening a 32-bit value to u64 does not create continuity across wrap;
- the shared CounterBaselines logic must treat a decrease/wrap as a reset and re-baseline;
- do not synthesize bytes across a wrap;
- if legacy ifi_baudrate is zero or cannot truthfully represent the link capacity, publish capacity as None rather than a fabricated value.

The fallback is for compatibility, not the preferred steady-state path.

Do not add SystemConfiguration, NetworkExtension, packet capture, netstat, or another dependency merely for basic host byte counters.

### 4. Preserve the IOKit disk-I/O path unless a separate defect is demonstrated

The reported DISK symptom is the filesystem-capacity row, not evidence that the existing IOKit cumulative disk-I/O source is wrong.

Keep the Plan-109 IOKit IOBlockStorageDriver statistics collector unless native testing demonstrates a distinct correctness issue.

As part of this pass:

- verify disk_io() still returns a typed optional result on both macOS CI architectures;
- retain malformed-device skipping and optional-family isolation;
- do not make disk-I/O availability a hard readiness or closure requirement because virtualized/hosted Macs may legitimately expose no usable block-driver counters.

A discovered independent IOKit ABI/lifecycle defect may be corrected in this plan only if the fix is local to the existing collector and does not broaden product scope. Otherwise record a follow-up.

### 5. Preserve drive refresh isolation

Do not remove the shared DriveRefreshCache or make filesystem enumeration a new sampler-critical synchronous dependency merely to hide the observed symptom.

First correct the native ABI and prove whether the long-lived Intel host still lacks drives.

The asynchronous first-refresh behavior is allowed to produce a transient drives: null before the worker completes. If implementation evidence shows the macOS native query is reliably bounded and the transient is materially visible, an eager initial macOS-only refresh may be considered, but it is not required for this corrective plan and must not weaken the dead-mount isolation that motivated the shared drive worker.

### 6. Add bounded optional-family diagnostics

Today optional-family failures can collapse to None with little operator-visible evidence.

Improve tracing without changing the wire protocol:

- include the metric family and CollectErrorKind/source context when a macOS drive/network/disk-I/O native query fails;
- avoid warning once per sample for a stable unsupported condition;
- prefer transition logging (available -> unavailable, unavailable -> available) or another small bounded mechanism over a general logging subsystem;
- successful empty enumeration must remain distinguishable internally from a source error where the collector contract already distinguishes Some(empty) from None;
- never fabricate a zero-valued metric to make diagnostics easier.

No new /diagnostics endpoint, protocol capability bit, persistent event history, or TUI error panel belongs in this plan.

## Native verification

### Deterministic unit/parser tests

Add/adjust macOS tests for:

- libc statfs record conversion with nonzero capacity;
- MNT_LOCAL retained and MNT_DONTBROWSE/devfs/autofs filtered as today;
- checked block-size/count conversion and overflow rejection;
- NET_RT_IFLIST2 parser with multiple RTM_IFINFO2 records;
- malformed/truncated message lengths;
- unrelated route messages skipped safely;
- zero-byte and counter values preserved;
- loopback flags excluded from aggregate membership;
- down interface retained in detail but excluded from active capacity;
- duplicate interface identities deduplicated deterministically;
- fallback getifaddrs data interpreted as if_data, never if_data64;
- legacy counter decrease/wrap re-baselines through the shared rate helper rather than spiking.

Prefer parser helpers that can consume synthetic byte buffers without invoking sysctl so malformed-buffer behavior is deterministic.

### Native FFI smoke

On real macOS, add focused tests that establish:

1. mounted_filesystems() succeeds;
2. at least one eligible local filesystem is returned on the ordinary hosted Mac image;
3. the root/local filesystem has nonzero total capacity and sane free/available <= total relationships;
4. the preferred network source returns at least one interface on the ordinary hosted image;
5. interface ids/names are nonempty and counters are readable;
6. the complete MacOsCollector can warm CPU and optional counter baselines, then produce StatusPayloadV2;
7. after bounded warmup, payload.drives is Some(nonempty) and payload.network is Some(nonempty);
8. the v2 payload validates through the existing protocol validator.

The network payload need not report nonzero traffic. A zero-rate interval is valid after a real baseline; absence is what this regression test is intended to catch.

Disk I/O may remain None on a hosted Mac whose IOKit storage statistics are genuinely unavailable.

### CI command

Keep the existing two-entry native macOS matrix. Do not add a new workflow or runner.

The current native command filters only collector::macos::ffi::native_tests. Widen the existing macOS job to the smallest command that actually runs the macOS collector/parser/native-v2 coverage, preferably:

~~~text
cargo test -p greggd --all-features -- collector::macos
~~~

If that proves disproportionately expensive, keep the existing filter and place the required native-v2 tests under the native_tests namespace while adding a second narrowly filtered parser/unit invocation. Do not duplicate the Linux-owned full workspace suite on each Mac runner.

Both macos-15 and macos-15-intel must pass before closure because the statfs issue is architecture-sensitive.

## Protocol and client invariants

Do not change:

- StatusPayloadV2 field names or serde shape;
- drives: None versus Some(empty) meaning;
- NetworkPayload schema;
- v1 snapshots;
- v2 negotiation/fallback;
- TUI DISK aggregation;
- per-system NET row presence policy;
- network utilization math;
- sample cadence;
- core readiness requirements.

This corrective pass should make existing optional fields available on hosts where the native OS provides the underlying data; it should not invent a new capability model.

## Documentation updates when implementation lands

Update current-state documentation only where the implementation changes the described collector source:

- architecture/collectors.md;
- architecture/macos-collector-notes.md;
- architecture/greggd-daemon.md if optional-family failure/diagnostic behavior is described there;
- .opencode/skills/platform-collectors/SKILL.md;
- AGENTS.md only if the concise collector guidance needs correction;
- CHANGELOG.md with a macOS Intel drive/network correctness note;
- plans/README.md closure state.

Do not rewrite Plans 109-111 as though the corrected Darwin ABI existed historically. They remain truthful implementation records; Plan 128 is the corrective record.

## Verification

Run focused tests first on the implementation host where applicable:

~~~text
cargo test -p greggd --all-targets --all-features -- collector::rate
cargo test -p greggd --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

Run the existing ordinary CI workflow at the final implementation SHA and require both native macOS jobs to pass the expanded collector/v2 checks. Linux, Windows, and MSRV must remain green because shared collector/rate/protocol code is touched.

No new qualification workflow, self-hosted Mac, uploaded evidence artifact, or permanent benchmark is required.

If the user's older Intel Mac remains available, a direct post-implementation smoke is valuable but not a mandatory repository gate:

~~~text
greggd run
GET /v2/status after warmup
confirm drives is nonempty
confirm network is present
generate bounded network traffic and confirm rates change on a later sample
~~~

Record that host evidence in the closure note if performed; do not block the plan on access to that machine when native Intel CI proves the corrected ABI path.

## Acceptance criteria

- [ ] The private macOS StatFs declaration and direct unsuffixed getmntinfo binding are removed.
- [ ] Filesystem enumeration uses libc::getmntinfo and libc::statfs so Intel receives libc's correct INODE64 symbol binding.
- [ ] Existing local/dontbrowse/filesystem filtering and capacity semantics remain truthful.
- [ ] AF_LINK getifaddrs data is never cast to libc::if_data64.
- [ ] The preferred macOS network path uses NET_RT_IFLIST2 / if_msghdr2 / if_data64 with bounded, length-checked parsing.
- [ ] A correctly typed getifaddrs / if_data fallback exists for older/unsupported cases or implementation documents why the preferred API is universal across the supported macOS floor.
- [ ] Counter wrap/reset cannot create a throughput spike; it re-baselines through the existing shared rate semantics.
- [ ] Network interface names/ids, flags, loopback state, operational state, and capacity remain native-derived and deterministic.
- [ ] Existing IOKit disk-I/O collection remains optional and unchanged unless a separately demonstrated local correctness defect requires a bounded fix.
- [ ] Optional macOS telemetry failures do not fail core daemon readiness and do not fabricate zero values.
- [ ] Optional-family diagnostics include enough bounded context to distinguish source failure from ordinary absence without per-sample warning spam.
- [ ] Native macOS v2 tests prove nonempty drive capacity and network telemetry after bounded warmup.
- [ ] The existing macOS arm64 and macOS Intel CI jobs execute the relevant collector/v2 tests and both pass.
- [ ] No external metrics command, privilege escalation, new metrics dependency, protocol change, or TUI semantic change is introduced.
- [ ] Focused greggd tests, default local check, strict clippy, full existing CI, and documentation reconciliation pass.
- [ ] Closure records the implementation SHA and exact CI run used, plus optional older-Intel host evidence if available.

## Explicit non-goals

Do not include:

- process-level disk/network accounting;
- per-flow network telemetry;
- packet capture;
- Wi-Fi-specific signal/PHY metrics;
- interface naming heuristics;
- shelling out to netstat, ifconfig, networksetup, diskutil, iostat, or powermetrics;
- privileged IOKit or NetworkExtension access;
- new protocol fields or capability bits;
- a new diagnostics API;
- sampler cadence changes;
- a generalized FFI abstraction crate;
- replacing the shared rate helper;
- changing Linux or Windows metric sources except for shared regression fixes required by a corrected helper.

## Handoff note

The implementation should start in crates/greggd/src/collector/macos/ffi.rs and crates/greggd/src/collector/macos/mod.rs.

Do not begin by changing the TUI. The client already renders the existing v2 fields correctly: DISK is unavailable when drives is None/empty after aggregation, and NET is omitted when network is None.

First prove the raw native records on macOS Intel and arm64, then prove the MacOsCollector publishes the existing v2 payload. Only after those are correct should diagnostics or any startup-transient polish be considered.

The key compatibility rule is to let libc own Darwin ABI selection wherever it already provides the binding instead of duplicating architecture-sensitive C layouts/symbol names in Gregg.

## Closure record

Implemented at `0f134b0`, verified by existing CI run `36170917401` green
across all five jobs (Linux fmt/clippy/tests, macOS arm64, macOS Intel,
Windows incl. SCM smoke, MSRV Rust 1.89). Both native macOS jobs ran the
widened `collector::macos` suite: deterministic `iflist2`/`statfs`/`if_data`
parser and conversion tests, mock-collector filtering/overflow/wrap tests,
and the bounded-warmup native v2 proof of nonempty drive capacity plus
network telemetry with protocol validation on each architecture. Disk I/O
remained optional per the plan. Local `./scripts/check-local.sh` and strict
Linux clippy passed; `cargo check --target x86_64-apple-darwin` passed for
the Intel collector path. No older-Intel host smoke was available, so closure
rests on native Intel CI per the plan's non-blocking rule. No external
metrics command, privilege escalation, new dependency, protocol change, or
TUI semantic change was introduced. Docs reconciled in the same pass:
`architecture/collectors.md`, `architecture/macos-collector-notes.md`,
`architecture/greggd-daemon.md`, the `platform-collectors` skill,
`crates/greggd/README.md`, and `CHANGELOG.md`. Plans 109-111 are untouched
as historical records. No future plan depends on Plan 128 (it is terminal in
the dependency chain and independent of the remaining Plan 091 soak record),
so no downstream status changes were required.

Acceptance: all boxes hold at the implementation SHA — private `StatFs` and
the unsuffixed `getmntinfo` binding are removed, enumeration uses
`libc::getmntinfo`/`libc::statfs`, local/dontbrowse/filesystem filtering and
capacity semantics are preserved and tested, `AF_LINK` data is never cast to
`if_data64`, the preferred path parses `NET_RT_IFLIST2`/`if_msghdr2` with
bounded length checks plus a typed `if_data` fallback, wraps re-baseline
without spikes, interface identity/flags/capacity stay native-derived and
deterministic, IOKit collection is unchanged, optional failures preserve
readiness without fabricated zeroes, diagnostics are transition-bounded with
family and error context, and native arm64+Intel CI proves the v2 families.


## Post-closure correction

A post-closure review found one narrow defect in the Plan-128 `NET_RT_IFLIST2`
parser. Darwin emits heterogeneous routing messages in the interface-list
buffer: `RTM_IFINFO2` records are interleaved with shorter message layouts
such as `RTM_NEWADDR` and multicast-address records. The landed parser checks
every message against `size_of::<libc::if_msghdr2>()` before examining the
message type, so a valid shorter unrelated record can terminate the walk and
hide later interfaces.

This does not invalidate the Plan-128 filesystem ABI correction, typed
`getifaddrs`/`if_data` fallback, counter-wrap behavior, diagnostics, or the
expanded native Intel/arm64 collector CI. It does mean the closure statement
that all acceptance boxes held was too broad for the preferred-network-parser
criterion. The plan file also retained its acceptance boxes unchecked despite
the closure narrative; that historical inconsistency is preserved here rather
than retroactively rewriting the closed checklist.

Plan 129 owns the heterogeneous route-message parser correction, stronger
native interface-completeness proof, and final registry reconciliation.
