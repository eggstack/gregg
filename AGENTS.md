# AGENTS.md

This file defines the working contract for contributors and coding agents operating in this repository.

## Project objective

Build `gregg` as a narrow, low-overhead system-observation tool composed of three independently publishable Rust crates:

- `gregg-protocol`: dependency-light versioned wire types and compatibility rules.
- `greggd`: Linux/macOS/Windows metrics daemon and native service-management CLI.
- `gregg`: endpoint-management CLI, polling/state engine, and compact Ratatui TUI.

The design target is a small terminal-multiplexer pane and lightweight daemon deployment on Linux servers, ARM64 single-board computers, Intel Macs, Apple Silicon Macs, and Windows x86-64 machines. Do not broaden the project into a process monitor, historical telemetry service, remote administration system, or general monitoring platform.

The client optionally supports exactly one EggPool statistics source through
`gregg eggpool add`, `list`, and `remove`. It is client-only, defaults to HTTP
port `11300`, accepts only `http`/`https`, and persists an API-key
environment-variable name, never a resolved secret. Configuration commands do
not perform network or environment lookups. The client resolves the API-key
environment reference only while constructing a request. The EggPool path
uses only the existing summary endpoint, a fixed active-only 60-second cadence,
and four fixed periods; it is not a general dashboard integration. Passive refresh reuses the current state generation and resets its fixed 60-second deadline on activation, period changes, and manual refresh. Phase 61
owns the optional worker and event-loop wiring while rendering remains I/O-free.

## Source of truth

Read these before implementation:

1. `README.md` for public scope and command behavior.
2. `plans/000-roadmap-v1.md` for sequencing and release gates.
3. The applicable phase plan in `plans/` for detailed requirements and acceptance criteria.

When implementation reveals a conflict, preserve the narrow product objective and update the relevant plan in the same change. Do not silently diverge from documented behavior.

## Intended workspace boundaries

The workspace is already established as:

```text
crates/
├── gregg-protocol/
├── greggd/
└── gregg/
```

Root manifests live in `Cargo.toml`. The Rust toolchain is pinned in
`rust-toolchain.toml`. CI lives in `.github/workflows/ci.yml`. Phase-level
architectural decisions are recorded under `architecture/`.

Dependency direction is one-way:

```text
gregg-protocol  ◄── greggd
gregg-protocol  ◄── gregg
```

`gregg-protocol` must not depend on either application crate and must not acquire runtime, HTTP-server, terminal, or platform-collector dependencies. `greggd` and `gregg` must not depend on each other.

Keep these internal boundaries explicit:

- Native collection is separate from sampling and HTTP serving.
- Service management is separate from the foreground daemon process.
- Client polling is separate from application-state reduction.
- Rendering reads state; it does not perform I/O or mutate polling internals.
- Platform-specific code remains under narrow `cfg(target_os = ...)` modules.

## Rust and dependency policy

Prefer stable Rust and declare a workspace `rust-version` before publication. Avoid nightly-only language or Cargo features.

The workspace pins `rust-version = "1.75"` in `[workspace.package]` and
inherits it into every member manifest. `rust-toolchain.toml` pins the
current stable channel so formatting and lint behaviour match local
development and CI.

Dependencies must solve a concrete version-1 requirement. Disable unused default features, especially in HTTP clients and servers. Compatibility-only upper bounds are permitted when fresh transitive resolution would exceed the declared Rust 1.75 MSRV; document those bounds in the manifest and verify them from unpacked packages. The daemon needs plain HTTP/1 on a trusted local network; do not add TLS, cookies, proxy support, HTTP/2, multipart handling, compression, or remote-control surfaces without an approved scope change.

The daemon now uses axum, tokio, tracing, serde_json, serde, toml, and clap for the HTTP server, async runtime, structured logging, JSON serialization, configuration serialization/parsing, and CLI argument parsing respectively.

The client crate (`gregg`) uses clap, serde, serde_json, toml, uuid, ratatui, crossterm, futures-util, and (on Windows) `windows-sys` for CLI argument parsing, configuration serialization/parsing, JSON output, stable endpoint identity, terminal rendering, terminal I/O, async event bridging, and Windows-native file locking respectively.

`greggd` now exposes a `lib` target so integration tests can exercise the
collector without depending on internal-only paths.

The workspace enables `clippy::pedantic` as a warning (not an error) so
contributors see style suggestions without breaking the build on unrelated
changes. Workspace crates deny `unsafe_code` through `[workspace.lints.rust]`.
The Linux `statvfs` wrapper (`crates/greggd/src/collector/linux/source.rs`),
the macOS collector FFI module (`crates/greggd/src/collector/macos/ffi.rs`),
the Windows source module (`crates/greggd/src/collector/windows/source.rs`),
and the client's narrowly scoped Unix `flock` wrapper are the only exceptions;
each uses `#![allow(unsafe_code)]` with documented safety invariants on every
unsafe block. No unsafe pointers or borrowed foreign buffers cross any of these
boundaries.

Avoid external command execution for metrics collection. Linux metrics should come from kernel interfaces such as `/proc`; macOS metrics should come from Mach and sysctl APIs; Windows metrics should come from native system APIs such as `GetSystemTimes`, `GlobalMemoryStatusEx`, and `GetPerformanceInfo`. External tools may be used only as diagnostic references in tests or development documentation.

Unsafe Rust is permitted only where required for Linux `statvfs`, macOS FFI, or the client's
narrow Unix file-lock wrapper and Windows file-lock adapter. Contain it
in small modules, document every safety invariant, validate returned
lengths/status values, and expose owned safe Rust values. No unsafe
pointers or borrowed foreign buffers may cross either boundary.

## Protocol rules

The HTTP schema is a compatibility contract, not an incidental serialization format.

- Carry an explicit schema version.
- Use numeric bytes and percentages, not human-formatted strings.
- Distinguish an unsupported metric from a measured zero with `Option` values and capability metadata.
- Do not make platform identity a condition for interpreting metrics in the TUI.
- Additive compatible changes are preferred within schema version 1.
- Breaking semantic or structural changes require an explicit schema-version decision and migration tests.

Schema-version-2 drive capacity is carried only by the flat `/v2/status`
`StatusPayloadV2` wrapper. `StatusSnapshotV2` remains source-compatible for
downstream struct literals; `drives` is optional, bounded, and contains only
display name plus numeric used/total bytes. The client owns aggregate
used/total/available/percentage arithmetic and must use overflow-safe sums.

macOS has no Linux-equivalent aggregate CPU `iowait` state. Report it as unsupported/null; never fabricate `0.0`.

Windows has no Linux-equivalent load average, swap, or CPU I/O-wait state. Report them as unsupported/null; never fabricate values. Windows reports memory commit charge as a separate metric.

The schema-version-1 wire types are implemented in `gregg-protocol` and
documented in [`architecture/protocol.md`](architecture/protocol.md) and in
the rustdoc on each public type. Validation lives behind a `validate()`
method that returns structured `ValidationViolation`s rather than failing
through serde, so additive forward-compatible fields do not silently tighten
or loosen existing validation.

## CLI and configuration rules

Commands must be deterministic, scriptable, and return meaningful exit codes. Human-readable output goes to stdout; diagnostics go to stderr.

Configuration writes must be atomic: serialize to a temporary file in the same directory, flush as appropriate, rename, then validate/reload. Do not leave a partially written configuration after interruption.

The daemon remains a foreground process under `greggd run`. `start`, `stop`, `restart`, and `croncheck` delegate to systemd on Linux, launchd on macOS, and the Windows SCM on Windows. Do not add self-daemonization or PID-file ownership.

## TUI rules

The normal online rendering contract is five base rows per system (header, CPU,
memory, swap/commit, and disk); offline/pending rendering is one row per
system. The selected system may add bounded per-drive detail rows when the
transient expansion state is active. Scrolling is by logical system entry, not
raw row count, and a base online block must never be partially rendered. Avoid
borders that consume vertical space.

The renderer must adapt from `Frame::area()` on every draw. Width degradation is semantic: preserve system name, I/O-wait availability, load, and core count before lower-priority OS detail. Scrolling is by logical system entry, not raw row count.

Required navigation is `j`/Down and `k`/Up. On Systems they select entries; on
EggPool they change the fixed 1-hour/1-day/7-day/30-day window. `h`/Left and
`l`/Right cycle only configured top-level panes, and `v` toggles the Systems
Normal/Condensed layout. `e` toggles drive details for the selected system.
The condensed view uses one row per system, a two-row header/separator, and
fixed semantic width tiers that drop lower-priority columns without horizontal
scrolling. Pane, layout, expansion, and period state are transient. The
terminal must be restored on normal exit, errors, and panics. Rendering
functions must not perform network or filesystem I/O. Drive expansion is not
persisted to configuration. EggPool renders pending, failure, refreshing, and
retained-stale states without exposing raw errors or secrets.

## Testing expectations

Every phase must satisfy its plan-specific acceptance criteria. At minimum, the repository should enforce:

```text
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows (PowerShell)
```

Which runs:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

Plus platform-native collector tests. The `--release` preflight adds clean-tree
and version checks, package lists, source installation of `greggd`, one bounded
v2 loopback smoke, and the nonpublishing
`cargo publish -p gregg-protocol --dry-run --locked`; dependent-crate dry-runs
remain manual until the new `gregg-protocol` version is visible on crates.io.

Platform-specific CI should run on Linux, macOS, and Windows. Linux collector semantics require fixture-driven tests. macOS FFI wrappers require native tests plus pure tests for normalization/calculation logic. Windows native collector and daemon smoke tests exercise real Windows APIs. HTTP tests should use synthetic collectors so server behavior is deterministic. TUI buffer tests should cover narrow, medium, wide, mixed old/new cross-platform fleets, and resize cases. Protocol/poller tests should cover v1 fallback, old v2 payloads, unavailable/empty/populated drives, invalid-drive rejection without fallback, and the bounded maximum response.

The installed-daemon verification script (`scripts/verify-installed-daemon.sh`)
verifies a supplied executable by starting it, validating `/v2/healthz` and
`/v2/status` protocol fields, and asserting a clean shutdown. It does not
perform package installation or depend on release metadata. `/v2/status` is
the universal cross-platform status endpoint; `/v1/status` remains a
Linux/macOS compatibility endpoint and is intentionally unavailable on
Windows.

The v2 status response may include a bounded `drives` collection. Missing or
null means drive data is unavailable/legacy, while an empty collection means
successful enumeration with no eligible filesystems. Phase 50 collectors
enumerate eligible local filesystems natively on each supported OS; physical
disk topology remains out of scope.

Do not make tests sleep for production refresh intervals. Inject clocks, sample sources, schedulers, or short test intervals where timing behavior must be verified.

## crates.io release constraints

All three crates must be independently packageable with `cargo package`. Publication order is:

1. `gregg-protocol`
2. `greggd`
3. `gregg`

Before release, manifests must use crates.io-resolvable dependency versions rather than path-only dependencies, while retaining local `path` entries where Cargo permits combined `version` and `path` declarations. Each package needs complete metadata, included files, license expression, repository URL, readme, keywords/categories, and an intentional feature set.

Never publish from a dirty tree. Verify package contents with `cargo package --list` and
install packaged binaries into clean temporary environments before tagging a release.

## Release policy

Do not add automated publication, tagging, or GitHub Release creation to CI
or repository scripts. Publication is a manual operator action documented in
`RELEASING.md`. CI verifies source and product correctness only. This
decision requires an explicit new plan to change.

`cargo publish` must never appear in workflows, scripts, or checked-in
automation. The only acceptable `cargo publish` invocations are in
`RELEASING.md` as manual operator instructions and in `check-local.sh` as
`--dry-run` checks.

## Change discipline

Keep commits scoped to one plan or one coherent corrective pass. Update documentation and tests with behavioral changes. Avoid opportunistic refactors across crate boundaries unless they are necessary to satisfy current acceptance criteria.

Do not claim a phase complete because code exists. A phase is complete only when its explicit tests, platform checks, documentation, and acceptance criteria are satisfied with recorded command results or ordinary CI status; do not create a separate evidence system.
