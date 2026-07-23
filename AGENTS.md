# AGENTS.md

This file defines the working contract for contributors and coding agents operating in this repository.

## Project objective

Build `gregg` as a narrow, low-overhead system-observation tool composed of three independently publishable Rust crates:

- `gregg-protocol`: dependency-light versioned wire types and compatibility rules.
- `greggd`: Linux/macOS metrics daemon and native service-management CLI.
- `gregg`: endpoint-management CLI, polling/state engine, and compact Ratatui TUI.

The design target is a small terminal-multiplexer pane and lightweight daemon deployment on Linux servers, ARM64 single-board computers, Intel Macs, and Apple Silicon Macs. Do not broaden the project into a process monitor, historical telemetry service, remote administration system, or general monitoring platform.

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

Dependencies must solve a concrete version-1 requirement. Disable unused default features, especially in HTTP clients and servers. The daemon needs plain HTTP/1 on a trusted local network; do not add TLS, cookies, proxy support, HTTP/2, multipart handling, compression, or remote-control surfaces without an approved scope change.

The daemon now uses axum, tokio, tracing, and serde_json for the HTTP server, async runtime, structured logging, and JSON serialization respectively.

`greggd` now exposes a `lib` target so integration tests can exercise the
collector without depending on internal-only paths.

The workspace enables `clippy::pedantic` as a warning (not an error) so
contributors see style suggestions without breaking the build on unrelated
changes. Workspace crates deny `unsafe_code` through `[workspace.lints.rust]`;
the macOS collector FFI module (`crates/greggd/src/collector/macos/ffi.rs`)
is the only exception and uses `#![allow(unsafe_code)]` with documented
safety invariants on every unsafe block.

Avoid external command execution for metrics collection. Linux metrics should come from kernel interfaces such as `/proc`; macOS metrics should come from Mach and sysctl APIs. External tools may be used only as diagnostic references in tests or development documentation.

Unsafe Rust is permitted only where required for macOS FFI. Contain it in a small module, document every safety invariant, validate returned lengths/status values, and expose owned safe Rust values. No unsafe pointers or borrowed foreign buffers may cross the FFI boundary.

## Protocol rules

The HTTP schema is a compatibility contract, not an incidental serialization format.

- Carry an explicit schema version.
- Use numeric bytes and percentages, not human-formatted strings.
- Distinguish an unsupported metric from a measured zero with `Option` values and capability metadata.
- Do not make platform identity a condition for interpreting metrics in the TUI.
- Additive compatible changes are preferred within schema version 1.
- Breaking semantic or structural changes require an explicit schema-version decision and migration tests.

macOS has no Linux-equivalent aggregate CPU `iowait` state. Report it as unsupported/null; never fabricate `0.0`.

The schema-version-1 wire types are implemented in `gregg-protocol` and
documented in [`architecture/protocol.md`](architecture/protocol.md) and in
the rustdoc on each public type. Validation lives behind a `validate()`
method that returns structured `ValidationViolation`s rather than failing
through serde, so additive forward-compatible fields do not silently tighten
or loosen existing validation.

## CLI and configuration rules

Commands must be deterministic, scriptable, and return meaningful exit codes. Human-readable output goes to stdout; diagnostics go to stderr.

Configuration writes must be atomic: serialize to a temporary file in the same directory, flush as appropriate, rename, then validate/reload. Do not leave a partially written configuration after interruption.

The daemon remains a foreground process under `greggd run`. `start`, `stop`, `restart`, and `croncheck` delegate to systemd on Linux and launchd on macOS. Do not add self-daemonization or PID-file ownership.

## TUI rules

The online rendering contract is four rows per system; offline rendering is one row per system. Avoid borders that consume vertical space.

The renderer must adapt from `Frame::area()` on every draw. Width degradation is semantic: preserve system name, I/O-wait availability, load, and core count before lower-priority OS detail. Scrolling is by logical system entry, not raw row count.

Required navigation is `j`/Down and `k`/Up. The terminal must be restored on normal exit, errors, and panics. Rendering functions must not perform network or filesystem I/O.

## Testing expectations

Every phase must satisfy its plan-specific acceptance criteria. At minimum, the repository should eventually enforce:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

Platform-specific CI should run on Linux and macOS. Linux collector semantics require fixture-driven tests. macOS FFI wrappers require native tests plus pure tests for normalization/calculation logic. HTTP tests should use synthetic collectors so server behavior is deterministic. TUI buffer tests should cover narrow, medium, wide, mixed online/offline, and resize cases.

Do not make tests sleep for production refresh intervals. Inject clocks, sample sources, schedulers, or short test intervals where timing behavior must be verified.

## crates.io release constraints

All three crates must be independently packageable with `cargo package`. Publication order is:

1. `gregg-protocol`
2. `greggd`
3. `gregg`

Before release, manifests must use crates.io-resolvable dependency versions rather than path-only dependencies, while retaining local `path` entries where Cargo permits combined `version` and `path` declarations. Each package needs complete metadata, included files, license expression, repository URL, readme, keywords/categories, and an intentional feature set.

Never publish from a dirty tree. Verify package contents with `cargo package --list` and install packaged binaries into clean temporary environments before tagging version 1.0.0.

## Change discipline

Keep commits scoped to one plan or one coherent corrective pass. Update documentation and tests with behavioral changes. Avoid opportunistic refactors across crate boundaries unless they are necessary to satisfy current acceptance criteria.

Do not claim a phase complete because code exists. A phase is complete only when its explicit tests, platform checks, documentation, and acceptance criteria are satisfied with evidence.
