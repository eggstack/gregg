# Plan 134: gregg-host native telemetry crate extraction

Status: complete.

Depends on: completed Plan 133 characterization/boundary freeze.

Blocks: Plans 135-136.

## Objective

Create a reusable workspace crate containing Gregg's native Linux, macOS, and Windows host telemetry acquisition and stateful sampling implementation while preserving the behavior characterized by Plan 133.

The working package/library name is `gregg-host` / `gregg_host`. Confirm package-name availability before publication; an availability-driven rename is permitted if module ownership and compatibility remain unchanged.

This phase moves reusable implementation. It does not yet declare the extraction complete for external consumers; Plan 135 owns `greggd` cutover and qualification.

## Crate ownership

Create:

~~~text
crates/gregg-host/
  Cargo.toml
  README.md
  LICENSE
  src/
    lib.rs
    model.rs
    error.rs
    rate.rs
    drives.rs
    slow_probe.rs
    linux/
    macos/
    windows/
    test_fixtures/
~~~

Exact file names may follow the current module layout, but the ownership rules below are mandatory.

### gregg-host owns

- protocol-neutral host identity;
- protocol-neutral CPU/load/memory/swap/commit models;
- drive-capacity records;
- disk-I/O aggregate/detail records;
- network aggregate/detail records;
- CPU-frequency optional value;
- host/platform capabilities;
- native collection errors and warming/reset taxonomy;
- monotonic cumulative-counter baselines;
- drive candidate validation/deduplication;
- slow drive/filesystem refresh mechanism;
- Linux native source abstraction, parsers, collector, and fixtures;
- macOS native FFI/query seam, parser logic, collector, and mocks;
- Windows native source/FFI seam, collector, and mocks;
- deterministic platform collector tests.

### gregg-host must not own

- `gregg-protocol` schema types or validation;
- schema version constants;
- v1/v2 conversion;
- `supports_v1_snapshot()` as a wire-version concept;
- daemon readiness state;
- Unix wall-clock timestamps;
- HTTP;
- Tokio;
- EggServe;
- CLI/service/update/startup behavior;
- TUI/client normalization.

## Neutral model

Define explicit protocol-neutral types rather than re-exporting `gregg-protocol`.

Conceptually:

~~~rust
pub struct HostIdentity {
    pub name: String,
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_name: String,
    pub kernel_release: String,
    pub architecture: String,
}

pub struct HostCapabilities {
    pub cpu_iowait: bool,
    pub load_average: bool,
    pub swap: bool,
    pub memory_commit: bool,
    pub drives: bool,
    pub cpu_frequency: bool,
    pub disk_io: bool,
    pub network: bool,
}

pub struct HostSample {
    pub logical_cores: u32,
    pub cpu_usage_pct: Option<f32>,
    pub cpu_iowait_pct: Option<f32>,
    pub load: Option<LoadAverage>,
    pub memory: MemoryMetrics,
    pub swap: Option<SwapMetrics>,
    pub commit: Option<CommitMetrics>,
    pub drives: Option<Vec<DriveMetrics>>,
    pub cpu_frequency_hz: Option<u64>,
    pub disk_io: Option<DiskIoPayload>,
    pub network: Option<NetworkPayload>,
}
~~~

Exact names may change, but unsupported metrics must be representable without fake zero values.

The neutral model may use the existing `CollectErrorKind` names so `greggd` can preserve compatibility without lossy error mapping.

## Collection limits

Do not depend on `gregg-protocol` constants from the new crate.

Introduce a protocol-neutral `CollectionLimits` or equivalent for:

- max drive records;
- max drive-name bytes;
- max disk-I/O detail records;
- max disk id/name bytes where enforced;
- max network-interface records;
- max network id/name bytes where enforced.

During Plan 135, `greggd` must construct limits matching the current protocol constants exactly.

The standalone crate may provide defaults, but those defaults must not silently change Gregg's wire behavior.

## Preserve current timing during extraction

Do not move monotonic time ownership in this phase.

Current Linux/macOS/Windows collectors call `Instant::now()` at their existing disk/network collection boundary. Keep that location while moving code so Plan 133's rate characterization remains comparable.

A later additive API may accept an injected sample tick for deterministic consumers, but that belongs after Plan 135 qualification.

## Preserve slow drive probing

Move the existing `DriveRefreshCache` behavior into the reusable crate with semantics intact:

- one worker per collector/cache;
- immediate first request;
- 30-second steady refresh;
- bounded request/result channels;
- last-good retention;
- panic containment/backoff;
- nonblocking poll;
- drop does not join a worker blocked in an uninterruptible native filesystem call.

Do not introduce a configurable policy enum in this phase unless it can be added without changing the default path and without complicating equivalence. The simplest migration is preferred.

## Platform moves

### Linux

Move the current:

- proc/stat CPU parser and delta arithmetic;
- meminfo memory/swap parser;
- loadavg parser;
- identity/os-release logic;
- CPUFreq source;
- /sys/block disk-I/O source and layering exclusions;
- /proc/net/dev + sysfs network source and aggregate-member semantics;
- mountinfo + statvfs drive collection;
- `FileSource`/`ProcSource`/`MemorySource`;
- fixtures and collector tests.

Preserve every Plan-133 characterization and current filtering rule.

### macOS

Move the corrected post-Plans-128/129 implementation, not an older conceptual design.

Preserve:

- Mach CPU and VM sources;
- `vm.swapusage`;
- `getloadavg`;
- identity and product-version behavior;
- libc-owned `getmntinfo`/`statfs` ABI;
- `NET_RT_IFLIST2` heterogeneous-message parser;
- correctly typed `getifaddrs`/`if_data` fallback;
- IOKit disk counters;
- optional-family availability transition behavior or an equivalent diagnostic event consumed by `greggd`;
- Intel and Apple Silicon native testability.

Do not reintroduce private Darwin structure layouts already removed by Plan 128.

### Windows

Move the current direct native implementation and retain:

- `GetSystemTimes`;
- `GlobalMemoryStatusEx`;
- `GetPerformanceInfo` commit semantics;
- topology guard;
- `GetComputerNameExW`/identity handling;
- `CallNtPowerInformation` CPU frequency;
- logical drive capacity;
- `IOCTL_DISK_PERFORMANCE`;
- `GetIfTable2`;
- direct contained FFI and mock source seam.

Do not "fix" multi-processor-group support during the extraction. Preserve the current guard and defer expansion to a separately characterized change.

## Dependency boundary

The new crate must not depend on `gregg-protocol`.

Target dependencies should remain small and target-scoped. `libc` is acceptable on Unix. Do not adopt `sysinfo`, `systemstat`, `monitrs`, `netdev`, Tokio, or a Windows abstraction crate merely to simplify the move.

If `tracing` is currently needed only for macOS optional-family state transitions or slow-probe diagnostics, prefer one of:

1. keep diagnostics in the `greggd` facade via a small returned diagnostic state; or
2. make tracing an optional feature disabled by default.

Do not perform a broad logging-framework redesign.

## Unsafe boundary

The workspace root currently denies unsafe code except contained platform modules using local allowances.

Preserve or tighten that structure:

- safe public API;
- platform-specific unsafe confined to the smallest FFI/source modules;
- documented safety invariants;
- no raw pointer or borrowed native-buffer lifetime escapes.

Extraction must not make unsafe broadly allowed at crate level.

## Testing

Move deterministic collector/parser tests with the implementation so `gregg-host` can qualify independently.

Keep enough facade tests in `greggd` to prove adapter behavior.

Run at minimum:

~~~text
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --all-targets --all-features -- collector
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

The existing macOS and Windows native CI jobs must compile and exercise the moved code through workspace membership.

## Acceptance criteria

- [x] `crates/gregg-host` exists as a workspace crate with Rust 1.89 MSRV.
- [x] The crate has no dependency on `gregg-protocol`, Tokio, EggServe, Clap, updater, client, or TUI crates.
- [x] Protocol-neutral identity/capability/sample types cover every field currently collected by Linux/macOS/Windows.
- [x] Collection limits are no longer imported from `gregg-protocol` inside the reusable crate.
- [x] Shared rate/baseline semantics move without behavioral change.
- [x] Drive normalization and slow-refresh isolation move without behavioral change.
- [x] Linux native source/parsers/collector/fixtures move with Plan-133 characterization green.
- [x] Corrected macOS Mach/sysctl/libc/IOKit implementation moves with arm64+Intel tests green.
- [x] Windows native FFI/source/collector moves with current topology/commit/network/disk semantics green.
- [x] No external metrics command or privilege requirement is introduced.
- [x] Unsafe remains contained to documented platform FFI/source boundaries.
- [x] Existing workspace tests and Rust-1.89 compilation remain green.
- [x] `greggd` can still compile against a compatibility layer pending Plan 135 cutover.
- [x] Package-name availability is checked before any crates.io publication; publication itself is not required by this plan.

## Explicit non-goals

Do not include:

- full `greggd` compatibility closure;
- protocol changes;
- clock injection;
- configurable slow-probe policies;
- Windows processor-group support expansion;
- FreeBSD/NetBSD/OpenBSD implementation;
- process/GPU/sensor telemetry;
- crates.io release;
- client/TUI changes;
- binary-size optimization unrelated to extraction.

## Handoff note

Move tests and source seams with each platform rather than copying only production code.

If a platform behavior cannot be represented by the proposed neutral model without losing information, extend the neutral model. Do not force the native backend through Gregg's current wire limitations; the wire adapter belongs to Plan 135.

## Closure record

Implemented cumulatively at `a9dab65` plus `a5624a9` plus devstat fix `43b5cf3` (Plan-134-owned files:
`crates/gregg-host/` in full — `Cargo.toml`, `README.md`, `LICENSE`,
`src/lib.rs` (`HostCollector`, percentage helpers), `src/model.rs`
(`HostIdentity`, `LoadAverage`, `MemoryMetrics`, `SwapMetrics`,
`CommitMetrics`, `DriveMetrics`, `DiskIoPayload`, `NetworkPayload`,
`HostCapabilities` with 8 support flags, `HostSample` with `Option`
load/swap and no fabricated zeros, `CollectionLimits` with Gregg-matching
defaults), `src/error.rs`, `src/rate.rs`, `src/drives.rs`
(`normalize`/`normalize_with_limits`), `src/slow_probe.rs`
(`DriveRefreshCache`), `src/linux/`, `src/macos/`, `src/windows/` with
source seams/mocks/fixtures/tests — plus the `gregg-host` workspace
membership in the root `Cargo.toml` and `Cargo.lock`).

Neutral-model deltas from the pre-extraction shapes are intentional and
wire-invisible: `HostSample.load`/`swap` are `Option` (Windows reports
`None` instead of zeroed v1-convention values; the Plan-135 adapter maps
`None` back to the frozen zeros), `HostCapabilities` carries 8 flags
(v1/v2 wire mapping lives in the `greggd` adapter, never in the crate),
and drive/disk/network bounds come from `CollectionLimits` (Gregg
defaults equal the protocol constants; `greggd` constructs them from the
constants explicitly). Timing is preserved: `Instant::now()` stays at the
existing disk/network collection boundary. Slow-probe policy is unchanged
(one worker, immediate first request, 30s cadence, bounded channels,
last-good retention, panic backoff, nonblocking poll, drop never joins).
Unsafe remains confined to the documented platform FFI/source modules
(`linux/source.rs` statvfs, `macos/ffi.rs` Mach/sysctl/IOKit,
`windows/source.rs` Win32); the crate root carries no `forbid` that would
conflict with those local allowances, matching the workspace pattern.

Dependency review (`cargo tree -p gregg-host`): `libc` (Unix only),
`thiserror`, `tracing` — no `gregg-protocol`, Tokio, EggServe, Clap,
updater, client/TUI, `serde`, shell, or system-information crates.
Package-name availability: `gregg-host` returns 404 on crates.io
(available); publication is not required and was not performed.
`greggd` compiled unchanged throughout this phase (no dependency on the
new crate yet), so the compatibility layer requirement holds trivially:
all pre-extraction public paths still resolve to the original code.

Local verification: `cargo test -p gregg-host --all-targets --all-features`
(24 tests green on Linux), `cargo check -p gregg-host --target
x86_64-apple-darwin` and `--target x86_64-pc-windows-msvc` clean,
`RUSTFLAGS="-D warnings"` strict checks clean, plus the full workspace
fmt/clippy/tests/check-local suite green. Remote CI run `36220930632`
green including the macOS arm64/Intel native suites exercising the moved
macOS code and the Windows suite exercising the moved Windows code.

Acceptance: all boxes hold at the implementation SHA. No protocol,
clock, slow-probe-policy, processor-group, BSD, process/GPU/sensor,
publication, client/TUI, or footprint work was included.

## Host MSRV note

The crate inherits workspace `rust-version = "1.89"` and uses no
post-1.89 features; the MSRV job compiles and tests the full workspace
including `gregg-host`.
