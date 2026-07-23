# Phase 1: foundation, workspace, and protocol

## Objective

Establish a clean Cargo workspace and freeze the minimum cross-crate contracts needed for independent Linux collector, macOS collector, daemon-server, client-state, and TUI work. At phase completion, `gregg-protocol` must be a small, documented, independently packageable crate whose serialized output is protected by deterministic tests.

This phase must not implement native metric collection, HTTP serving, polling, or terminal rendering beyond compile-safe crate skeletons.

## Required repository structure

Create:

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml or documented toolchain policy
crates/
├── gregg-protocol/
│   ├── Cargo.toml
│   └── src/lib.rs
├── greggd/
│   ├── Cargo.toml
│   └── src/main.rs
└── gregg/
    ├── Cargo.toml
    └── src/main.rs
.github/workflows/ci.yml
```

The root manifest should use resolver version 2, centralize shared package metadata where practical, and declare all three workspace members. Decide and document an initial MSRV; prefer a reasonably current stable compiler rather than an artificially old baseline that constrains ecosystem dependencies.

All package names must be checked for crates.io availability before manifests are treated as final. The intended names are `gregg-protocol`, `greggd`, and `gregg`. If any is unavailable, stop and document the naming conflict rather than silently selecting a different published name.

## Package metadata

Prepare manifests for eventual crates.io publication from the first commit. Each package should declare or inherit:

- Version, edition, and rust-version.
- Authors/maintainers as appropriate.
- Repository and homepage URLs.
- Description.
- License expression after the project license is selected.
- Readme path.
- Keywords and categories within crates.io limits.
- Explicit include/exclude policy where needed.

Workspace-local dependencies must use both `path` and a compatible `version`, for example:

```toml
[dependencies]
gregg-protocol = { version = "0.1.0", path = "../gregg-protocol" }
```

This keeps local development convenient while allowing `cargo package` to validate crates.io resolution.

## Protocol schema version 1

Define public serde types representing one complete daemon snapshot. Exact names may vary, but the model must include these concepts:

```rust
pub const SCHEMA_VERSION_V1: u16 = 1;

pub struct StatusSnapshot {
    pub schema_version: u16,
    pub observed_at_unix_ms: u64,
    pub sample_interval_ms: u64,
    pub capabilities: MetricCapabilities,
    pub system: SystemIdentity,
    pub cpu: CpuMetrics,
    pub load: LoadAverage,
    pub memory: MemoryMetrics,
    pub swap: SwapMetrics,
}

pub struct MetricCapabilities {
    pub cpu_iowait: bool,
}

pub struct SystemIdentity {
    pub name: String,
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub kernel_name: String,
    pub kernel_release: String,
    pub architecture: String,
}

pub struct CpuMetrics {
    pub logical_cores: u32,
    pub usage_pct: f32,
    pub iowait_pct: Option<f32>,
}

pub struct LoadAverage {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
}

pub struct MemoryMetrics {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub usage_pct: f32,
}

pub struct SwapMetrics {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub usage_pct: f32,
}
```

Use naming that serializes consistently in lower snake case. Do not transport human-formatted units or combined identity strings. Keep identity fields separable so the TUI can degrade by width priority.

Add a readiness/health response type sufficient to distinguish at least:

- Ready with a valid cached snapshot.
- Warming up while the first counter delta is unavailable.
- Collector failure with a concise machine-readable category and safe human-readable message.

Do not expose internal error chains, filesystem paths, or platform-private structures in the wire contract.

## Invariants and validation

Provide constructors or validation helpers that enforce protocol-level invariants where useful:

- `schema_version` is supported.
- Logical core count is nonzero.
- Percentages are finite and clamped or rejected outside `0..=100` according to one documented policy.
- Used bytes do not exceed total bytes after normalization.
- Zero total swap yields zero usage percentage rather than NaN or infinity.
- `iowait_pct` is `None` when `cpu_iowait` is false.
- `iowait_pct` may not be `None` in a ready Linux snapshot when `cpu_iowait` is true.

Avoid overloading serde deserialization with opaque validation that makes forward compatibility difficult. A separate `validate()` method returning structured violations is preferable.

## Compatibility policy

Document schema-version handling in rustdoc and a short protocol note. Version-1 rules should be:

- Unknown additive JSON fields are ignored by default.
- Required version-1 fields remain required unless explicitly changed to optional under an additive compatibility decision.
- The client rejects unsupported schema majors with a per-host error rather than terminating the entire TUI.
- Enum-like fields intended for future extension should use representations that permit unknown values where necessary.
- Capability flags control interpretation of optional metrics.

Create canonical JSON fixtures:

```text
crates/gregg-protocol/tests/fixtures/linux-v1.json
crates/gregg-protocol/tests/fixtures/macos-v1.json
crates/gregg-protocol/tests/fixtures/health-ready-v1.json
crates/gregg-protocol/tests/fixtures/health-warming-v1.json
```

The macOS fixture must contain `cpu_iowait: false` and `iowait_pct: null`. The Linux fixture must demonstrate a measured value, including a legitimate zero if desired.

## Synthetic test support

Expose minimal fixture builders behind either a test-support feature or test-only module. Do not force production users to compile large mocking dependencies. Later daemon and client tests need deterministic snapshots without native collection.

Prefer plain constructors/builders over random generation for canonical tests. Property tests may be added for numeric invariants if they remain lightweight and reproducible.

## Error conventions

Each binary crate should establish a crate-local typed error boundary, but do not put application errors into `gregg-protocol`. Public wire errors should be separate serializable response types. Internal errors can use `thiserror`; command entry points should render concise diagnostics and preserve sources for tracing/debug logs.

## CI foundation

Create a workflow that initially runs on Linux and macOS and performs:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
cargo package -p gregg-protocol --allow-dirty
```

Use dependency caching conservatively. Do not allow cache restoration failures to fail the build. Pin GitHub Actions by stable major versions or commit SHAs according to repository policy.

Add a Linux ARM64 compile check if practical through cross compilation, but do not claim runtime support from compile-only evidence.

## Documentation outputs

Update the root README only where implementation details become concrete. Add rustdoc examples that serialize and deserialize a complete snapshot. Document units for every numeric field and explicitly state that CPU percentages derive from intervals rather than instantaneous single reads.

## Acceptance criteria

Phase 1 is complete when:

1. The workspace builds with all three members and no cyclic dependencies.
2. `gregg-protocol` depends only on narrowly required serialization/error crates.
3. Canonical Linux and macOS schema fixtures round-trip without semantic changes.
4. Validation tests cover NaN/infinity, out-of-range percentages, zero swap, used-greater-than-total, capability/value mismatch, and unsupported schema versions.
5. The macOS fixture represents I/O wait as unsupported/null, not zero.
6. `cargo package -p gregg-protocol` succeeds and the package contents contain no unrelated workspace files.
7. Linux and macOS CI pass formatting, linting, tests, and documentation.
8. The protocol crate’s public items have useful rustdoc and units/semantics are unambiguous.
9. Binary crate skeletons compile but contain no premature collector, server, polling, or TUI coupling.
10. The roadmap and subsequent plans remain accurate after any naming or MSRV decisions.

## Handoff to phases 2 and 3

The collector phases may proceed in parallel after this phase. They must consume protocol types or a daemon-internal normalized sample that maps one-to-one to them. Any requested protocol change must be reviewed for both platforms and client compatibility rather than made in one collector branch alone.
