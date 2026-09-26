# Plan 132: native host telemetry extraction and BSD portability roadmap

Status: complete.

Depends on: the current post-Plan-131 main state, the settled native collector and live-metrics work from Plans 109-111, the macOS ABI/parser corrections from Plans 128-129, the Rust 1.89 baseline from Plan 117, and the current daemon publication/performance baseline from Plans 120-124. This work is independent of the remaining Plan 091 soak record.

Coordinates: Plans 133-136.

## Objective

Extract Gregg's native host telemetry acquisition and sampling primitives from `greggd` into a reusable, dependency-light Rust crate without changing observable `greggd` behavior, protocol output, readiness, cadence, platform support, or operational failure isolation.

The working crate name for planning is `gregg-host`. The implementation may select another available package name before publication, but the ownership boundary described here must not change merely for naming.

The longer-term goal is to make the extracted collector boundary suitable for additional native operating-system backends, beginning with FreeBSD and leaving explicit seams for NetBSD and OpenBSD. BSD support must not be entangled with the initial Linux/macOS/Windows extraction: existing platforms are first moved and qualified without semantic change, then FreeBSD proves that the boundary is genuinely portable.

## Why this extraction is justified

The current `crates/greggd/src/collector/` tree already contains a cohesive subsystem distinct from daemon transport and service lifecycle:

- Linux procfs/sysfs collection, CPU/memory/swap/load parsing, filesystem capacity, CPU frequency, disk I/O, and network telemetry;
- macOS Mach/sysctl/libc/IOKit collection behind a contained native-query seam;
- Windows Win32 collection for CPU, physical memory, commit, drives, CPU frequency, disk I/O, network, and identity;
- shared monotonic cumulative-counter baselines and rate derivation;
- shared drive validation/deduplication/bounding;
- typed warming/reset/source/parse/numeric errors;
- optional-family isolation and daemon-grade slow filesystem probing.

The reusable value is not generic system inspection. It is native, continuously sampled host telemetry with explicit reset/warmup semantics and bounded failure behavior. Those primitives can be useful to other Rust agents without importing Gregg's HTTP server, Tokio runtime, CLI, updater, or wire protocol.

## Governing compatibility rule

The extraction is complete only if a supported Linux, macOS, or Windows host running the post-extraction `greggd` is observationally equivalent to the pre-extraction daemon for the same native measurements.

In particular, preserve:

1. CPU formulas, first-sample warming, counter-reset handling, and Linux hotplug core-count behavior.
2. Memory, swap, Windows commit, and load-average definitions.
3. Existing optional CPU-frequency behavior, including truthful macOS absence.
4. Existing disk and network accounting-set policy and actual-elapsed-time rate arithmetic.
5. Existing drive filtering, free/available semantics, ordering, deduplication, and protocol bounds.
6. `drives: None` before first successful enumeration, `Some(empty)` for successful empty enumeration, and last-success retention after later failure.
7. Isolation of potentially blocking drive/filesystem enumeration from the core sample path and daemon shutdown.
8. Optional disk/network/drive source failures remaining non-fatal to core readiness.
9. Exact current `CollectErrorKind` meaning at the `greggd` compatibility boundary.
10. Linux/macOS v1 publication and Windows v2-only behavior.
11. Existing schema-v1/v2 Rust types, JSON shapes, validation, status codes, and mixed-version client behavior.
12. Current public `greggd::collector` source paths during the compatibility period.
13. Current native source/mock seams used by deterministic tests.
14. Rust 1.89 MSRV.
15. No external metrics commands or new privilege requirements.

Do not combine the extraction with rate-formula cleanup, protocol-v3 work, richer metric-state enums, sampling-cadence changes, process telemetry, GPU/sensor work, or service-manager redesign.

## Target architecture

The intended ownership is:

~~~text
gregg-host
  model / error / rate / limits / slow probes
  linux/
  macos/
  windows/
  freebsd/      # Plan 136, after extraction qualification
  [future netbsd/]
  [future openbsd/]
        |
        | neutral HostSample / HostIdentity / HostCapabilities
        v
greggd::collector compatibility facade + gregg-protocol adapter
        |
        v
greggd::sampler
        |
        v
gregg-protocol v1/v2
~~~

The reusable crate must not depend on `gregg-protocol`, Tokio, EggServe, Clap, `gregg-update`, or the client/TUI crates.

The reusable layer owns native acquisition and sampling state. Gregg-specific schema conversion, v1 support policy, v2 validation, readiness publication, timestamps, HTTP, and service lifecycle remain in `greggd`.

## Phase ownership

### Plan 133: compatibility characterization and boundary freeze

Before code moves, establish the exact behavioral and public compatibility contract. Add deterministic characterization where current tests do not prove sequence behavior, wire equivalence, public path continuity, or optional-probe isolation.

This plan should make later source movement mechanically testable. It must not redesign the collector.

### Plan 134: standalone native telemetry crate extraction

Create the new workspace crate and move/copy the reusable model, errors, rate logic, drive/slow-probe machinery, native source seams, platform implementations, fixtures, and platform collector tests behind a protocol-neutral API.

Preserve existing acquisition timing and worker behavior during this phase. Do not "clean up" semantics while moving them.

### Plan 135: greggd compatibility adapter, cutover, and qualification

Make `greggd` consume the extracted crate through a narrow compatibility facade. Keep current public collector paths and current protocol conversion behavior. Run old-versus-new deterministic characterization, exact wire/golden checks, native CI, MSRV, feature/dependency review, and footprint sanity before declaring the extraction reusable.

### Plan 136: FreeBSD-first native telemetry and BSD portability foundation

After the existing three platforms are qualified, add a native FreeBSD backend to the reusable crate as the first proof that the abstraction is not Linux/Darwin/Windows-specific.

FreeBSD support in `gregg-host` is distinct from first-class FreeBSD support for the complete `greggd` daemon, packaging, release binaries, or rc.d lifecycle. Those product-level additions require a later plan after the collector backend is stable.

## Portability rules for BSD

Do not create a broad `unix` collector abstraction.

Linux procfs/sysfs, Darwin Mach/sysctl/IOKit, FreeBSD sysctl/libdevstat/ifmib, NetBSD UVM/sysctl, and OpenBSD sysctl interfaces overlap conceptually but differ materially in accounting and ABI details.

Share only semantics proven to be platform-independent:

- normalized model types;
- percentage validation;
- cumulative-counter baseline/rate logic;
- deterministic bounding/dedup helpers;
- slow-probe policy/mechanism where applicable;
- source-injection/testing patterns.

Each OS keeps an explicit native source seam. A private BSD-common helper may be introduced later only for code genuinely shared by at least two BSD implementations.

## Dependency and footprint policy

The extracted crate should remain synchronous and runtime-neutral.

Initial target dependency shape:

- `std`;
- `libc` on Unix targets where native ABI access requires it;
- no all-in-one system-information dependency;
- no async runtime;
- no serialization dependency required for core collection;
- no shell-command dependency.

Current tracing behavior may remain temporarily in the `greggd` adapter or behind a small optional diagnostic seam. Do not force a logging-framework redesign into the extraction merely to achieve a dependency-count target.

Measure the actual dependency graph and stripped `greggd` binary before/after. Do not claim a footprint improvement unless measured.

## Required verification model

The campaign must use the repository's existing local and native CI structure.

For Plans 133-135:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

The existing native jobs remain authoritative:

- Linux full workspace;
- macOS arm64 collector/native tests;
- macOS Intel collector/native tests;
- Windows workspace + release/SCM smoke;
- MSRV Rust 1.89.

No new workflow is required merely for the extraction.

Plan 136 may add one bounded FreeBSD native qualification path because the current hosted matrix has no BSD runner. That addition belongs to Plan 136, not Plans 133-135.

## Global non-goals

Do not include:

- protocol-v3 or new public Gregg wire fields;
- client/TUI feature work;
- process/socket/GPU/sensor/battery telemetry;
- persistent history, exporters, alerts, or databases;
- eBPF/ETW/DTrace collection;
- subprocess scraping;
- privilege escalation;
- generic "Unix" implementation unification;
- immediate NetBSD/OpenBSD implementation;
- FreeBSD rc.d/service packaging or release binaries before the collector backend is independently qualified;
- fixing the Windows processor-group limitation inside the extraction campaign unless existing behavior regresses;
- moving monotonic clock ownership as part of the source move;
- changing the drive worker into a new policy API before compatibility qualification.

## Acceptance criteria

- [ ] Plan 133 records and tests the exact compatibility surface before source movement.
- [ ] Plan 134 creates a protocol-neutral native telemetry crate and moves the reusable Linux/macOS/Windows collection implementation without semantic drift.
- [ ] Plan 135 makes `greggd` consume the extracted implementation while preserving public collector compatibility, protocol output, readiness, native behavior, MSRV, and operational failure isolation.
- [ ] Existing Linux, macOS arm64, macOS Intel, and Windows native qualification remains green.
- [ ] No new external metrics command, privilege requirement, async/runtime framework, or generic system-information dependency is introduced.
- [ ] `greggd` retains exact current v1/v2 semantics, including Windows v2-only behavior.
- [ ] The extracted crate is not coupled to `gregg-protocol`.
- [ ] Plan 136 proves the design with a native FreeBSD backend after existing-platform qualification.
- [ ] NetBSD/OpenBSD remain explicit future backends rather than being falsely claimed through a generic BSD flag.
- [ ] Current architecture and planning documentation are reconciled after each phase.

## Handoff note

Start with Plan 133. Do not begin by moving files.

The main risk in this campaign is not compilation; it is silent semantic drift in warmup/reset behavior, optional-family absence, aggregation, slow-probe isolation, or v1/v2 conversion. Characterize those behaviors first, move the reusable implementation second, and cut `greggd` over only after both implementations can be compared deterministically.

## Closure record

Coordination complete with Plans 133-136, implemented cumulatively at
`a9dab65` plus `a5624a9` plus devstat fix `43b5cf3`. All four acceptance boxes hold:
133 recorded and tested the compatibility surface; 134 created the
protocol-neutral `gregg-host` crate; 135 cut `greggd` over through the
facade with exact native/v1/v2/MSRV equivalence (stripped release
`greggd` byte-identical at 2,629,008 bytes); 136 proved the boundary with
a native FreeBSD backend. Linux, macOS arm64, macOS Intel, Windows, MSRV
Rust 1.89, and the new FreeBSD native qualification are green in remote
CI run `36219678790`. No external metrics command, privilege, async
runtime, or system-information dependency was introduced; `greggd`
retains exact v1/v2 semantics including Windows v2-only behavior; the
extracted crate is not coupled to `gregg-protocol`; NetBSD/OpenBSD remain
explicit future backends. Independent of the remaining Plan 091 soak
record; no downstream plan status changes required.
