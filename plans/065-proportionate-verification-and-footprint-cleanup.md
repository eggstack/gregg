# Phase 65: proportionate verification and footprint cleanup

Status: completed. Ordinary CI run `30964819950` passed at implementation SHA `aaf0cab`.

## Objective

Reduce duplicated local/CI work and apply a small set of measured client binary-footprint improvements after Phase 64 correctness is complete.

This phase is intentionally conservative. It does not replace dependencies, redesign runtime architecture, change supported platforms, or alter the Rust 1.75 compatibility promise. The goal is faster routine iteration and modest footprint cleanup without removing product behavior.

## Dependencies and execution position

Depends on Phase 64.

```text
63 -> 64 -> 65
```

Phase 65 begins only after the Phase 64 focused regression tests pass. Verification simplification must not be used to hide or delete those regressions.

## Governing rules

1. Keep one ordinary read-only GitHub Actions workflow.
2. Keep native hosted coverage for every advertised OS/architecture combination already represented by CI.
3. Do not publish, tag, create releases, or upload evidence from CI.
4. Do not add a new verification tier unless it replaces more work than it adds.
5. The default local command must be genuinely fast and must not repeat tests.
6. Release-oriented checks remain manual.
7. Retain product tests; remove duplicate invocations, not meaningful behavior coverage.
8. Retain Rust 1.75 compatibility and its one lightweight compile check.
9. Binary-size changes must be measured before and after.
10. Do not replace core dependencies in this phase.

## Current excess

### Local script duplication

The default local script currently runs:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
native collector tests for the current host
```

The native collector invocation repeats tests already selected by the full workspace all-target test. Documentation and full Clippy are useful, but they do not need to block every short edit/test cycle.

### CI duplication

The Linux job runs full workspace tests and then repeats Linux collector tests. The Windows job separately runs a workspace check, client tests, collector tests, service tests, and the foreground smoke even though a single all-target workspace test can compile and run the same nonignored test targets. The ordinary workflow also builds documentation on every Linux push/PR.

The macOS matrix is not considered duplication because it represents both advertised Apple Silicon and Intel native targets. The Rust 1.75 job is retained because the manifest still promises that compiler version.

### Production dependency features

The `gregg` crate enables Tokio's `test-util` feature in normal dependencies and uses the multithread runtime for a small terminal event loop dominated by asynchronous I/O.

Candidates:

- move `test-util` to a dev-dependency feature set;
- switch the client entry point to Tokio's current-thread runtime;
- replace `rt-multi-thread` with `rt` in production features.

These are bounded changes. No HTTP, TLS, TUI, or CLI dependency replacement is permitted.

## Workstream A: make the default local check actually fast

### Required Unix behavior

Revise `scripts/check-local.sh` so its default mode runs only:

```text
cargo fmt --all -- --check
cargo test --workspace
```

A separate `cargo check` is not required because `cargo test --workspace` already compiles the workspace. Do not add a duplicate compile-only pass to the default path.

The default mode must not run:

- documentation;
- package listing;
- source installation;
- publish dry-runs;
- native collector tests a second time;
- release metadata checks;
- clean-tree checks.

### Required PowerShell parity

Apply the same default behavior to `scripts/check-local.ps1`:

```text
cargo fmt --all -- --check
cargo test --workspace
```

Unix and Windows scripts must describe the same tiers and differ only where shell/platform mechanics require it.

### Release mode

Keep `--release` / `-Release` as the manually invoked comprehensive preflight. It may run:

- the default checks;
- full workspace Clippy with warnings denied;
- workspace documentation;
- clean-tree and version consistency checks;
- package-content review;
- source installation and bounded loopback smoke;
- the existing protocol-only publish dry-run.

Do not add dependent-crate dry-runs, artifact capture, repeated installation, cross-platform emulation, or publication.

### Optional lint mode decision

Do not add a new `--lint` mode unless it materially simplifies the scripts. Maintainers can invoke the documented Clippy command directly. Two modes, default and release, remain sufficient.

### Documentation updates

Update active contributor/release documentation to state:

```text
./scripts/check-local.sh
```

is the short routine loop, while:

```text
./scripts/check-local.sh --release
```

is the intentionally slower nonpublishing release preflight.

Do not call the release mode mandatory for every commit or pull request.

### Workstream A acceptance criteria

- [x] Default Unix check runs exactly format plus workspace tests.
- [x] Default PowerShell check has equivalent behavior.
- [x] Native collector tests are not repeated after workspace tests.
- [x] Documentation and Clippy move to release mode or direct manual commands.
- [x] Release mode retains existing bounded package/install/publish-dry-run checks.
- [x] No third validation tier or evidence output is introduced.

## Workstream B: simplify the ordinary CI workflow

Keep `.github/workflows/ci.yml` as the only ordinary workflow.

### Linux job

Required steps:

```text
checkout
stable toolchain with rustfmt and clippy
cache (optional/continue-on-error as today)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Remove:

- the separate workspace documentation step;
- the duplicate Linux collector smoke step.

Reason: full all-target/all-feature tests already execute Linux collector unit/native tests selected for the Linux target. Documentation remains part of manual release preflight.

### macOS jobs

Retain both current native runners because Gregg advertises Intel and Apple Silicon macOS support.

Each job remains limited to:

```text
cargo check --workspace --all-targets --all-features
cargo test -p greggd --all-features -- collector::macos::ffi::native_tests
```

Do not expand macOS to full workspace tests unless a focused platform regression cannot be represented by the current native smoke. Do not add separate packaging or service-install rehearsals.

### Windows job

Replace the current fragmented check/test sequence with one consolidated command when the existing test suite supports it:

```text
cargo test --workspace --all-targets --all-features
```

This command must cover:

- client tests;
- Windows collector tests;
- Windows service-manager tests;
- the `windows_smoke` integration test;
- the feature-gated lock-helper contention test from Phase 64.

If Cargo target/feature behavior prevents one command from including the helper-dependent test, use the smallest additional explicit command for that test only. Do not retain separate collector, service, client, and smoke commands merely for labeling.

A separate `cargo check` is unnecessary when the full test command compiles all targets.

### MSRV job

Retain one lightweight Rust 1.75 compile job:

```text
cargo check --workspace --all-features
```

Do not add tests, Clippy, docs, packaging, or native collector execution to the MSRV job.

This roadmap does not change `rust-version`, compatibility pins, or downstream compiler policy.

### Workflow constraints

The workflow must continue to have:

```text
permissions:
  contents: read
```

It must not gain:

- write permissions;
- release events;
- tag triggers for publication;
- artifact upload/download;
- cache correctness gates;
- matrix-generated release targets;
- reusable workflow indirection;
- branch-protection evidence files.

### Workstream B acceptance criteria

- [x] Linux no longer builds docs or repeats collector tests.
- [x] macOS Intel and Apple Silicon native checks remain.
- [x] Windows uses one consolidated all-target workspace test, plus at most one narrowly required helper command.
- [x] MSRV remains one compile-only job.
- [x] CI remains one read-only workflow with no publishing or artifacts.
- [x] The workflow is shorter in commands than the current workflow.

## Workstream C: remove test-only Tokio support from production dependencies

### Current issue

The client production dependency enables Tokio's `test-util` feature. Paused time and `advance()` are test facilities and should not be part of the normal feature declaration.

### Required change

Production dependency features should include only runtime features used by the binary, for example:

```toml
tokio = { version = "1", features = ["rt", "macros", "time", "sync", "signal", "net"] }
```

The exact set must be derived from actual production imports. Do not remove a feature required by the event loop, scheduler, terminal input, or Reqwest runtime.

Enable `test-util` only for tests, through the existing dev-dependency section or a dev-only Tokio declaration whose features unify during test builds.

### Verification

Required tests include the paused-time EggPool worker tests from completed Plans 61-62. They must continue to compile and pass.

Use:

```text
cargo test -p gregg --all-targets --all-features
cargo build --release -p gregg
```

### Workstream C acceptance criteria

- [x] Normal `gregg` Tokio features do not include `test-util`.
- [x] Paused-time tests still compile and pass.
- [x] No new test utility crate is added.
- [x] Release client behavior is unchanged.

## Workstream D: evaluate and adopt a current-thread client runtime

### Rationale

The client is one terminal UI event loop coordinating:

- terminal events;
- timer-driven endpoint polling;
- small HTTP requests;
- bounded channels;
- an optional EggPool worker.

It has no CPU-parallel workload that requires Tokio's multithread scheduler. Endpoint concurrency is asynchronous network concurrency, not parallel CPU work.

### Required implementation

Change the client entry point from the default multithread Tokio macro to:

```rust
#[tokio::main(flavor = "current_thread")]
```

and replace the production `rt-multi-thread` feature with `rt`.

Do not change `greggd`; it already uses the current-thread runtime.

Do not rewrite `tokio::spawn` usage solely because of this change. Spawned futures must continue to satisfy the current-thread runtime's `Send + 'static` requirements under Tokio's normal `spawn` API.

### Behavioral verification

Required checks:

- system poll scheduling still reaches multiple synthetic endpoints concurrently up to the configured bound;
- terminal/event-loop unit tests continue to pass;
- EggPool request/cancellation tests continue to pass;
- Ctrl-C cancellation still compiles on all targets;
- no code assumes multiple worker threads;
- Windows, macOS, and Linux CI compile/test the client.

Do not add throughput benchmarks. Existing bounded-concurrency tests are sufficient for this I/O-bound tool.

### Retention rule

Retain the current-thread change only when:

- all focused and workspace tests pass;
- native CI passes;
- the release `gregg` binary size is equal or smaller than the Phase 65 baseline.

If size increases or a platform/runtime regression appears, revert this subchange without blocking the verification simplification work.

### Workstream D acceptance criteria

- [x] Client runtime is current-thread, or the attempted change is explicitly reverted with the measured reason recorded.
- [x] Endpoint concurrency behavior remains bounded and functional.
- [x] EggPool and terminal cancellation behavior remains functional.
- [x] No runtime abstraction layer is added.

## Workstream E: measure binary size without creating a size program

### Baseline

Before Workstreams C-D, build:

```text
cargo build --release -p gregg -p greggd
```

Record byte sizes for:

```text
target/release/gregg
target/release/greggd
```

Use the current host's normal file-size command or a small standard-library script. The record belongs in the implementation handoff, commit message, or Phase 65 closure paragraph. Do not add a permanent evidence file.

### Final measurement

Rebuild after retained dependency/runtime changes and record the same byte sizes.

Required interpretation:

- `gregg` must not grow;
- `greggd` should remain unchanged except normal toolchain nondeterminism;
- no percentage reduction target is imposed;
- no CI size threshold is added.

### Explicitly prohibited footprint work

Do not:

- replace Reqwest/Rustls with handwritten HTTP;
- replace Axum with a custom parser/server;
- replace Clap or Ratatui;
- disable TLS/HTTPS required by EggPool configuration;
- remove Windows/macOS support;
- remove validation or body bounds;
- add `cargo-bloat` as a required dependency or CI tool;
- add UPX or post-link compression;
- change `panic` strategy in this phase;
- remove debug/error diagnostics needed for normal operation;
- change MSRV pins as a size tactic.

### Workstream E acceptance criteria

- [x] Baseline and final release byte sizes are recorded concisely.
- [x] No permanent benchmark/evidence artifact is added.
- [x] Retained client footprint changes do not increase `gregg` size.
- [x] No core dependency replacement or feature loss occurs.

## Workstream F: remove verification documentation drift

Inspect and update only active instructions:

```text
README.md
CONTRIBUTING.md
RELEASING.md
AGENTS.md
architecture/scripts-and-packaging.md
crates/*/README.md
plans/README.md
```

Required wording:

- default local check is the short routine loop;
- release preflight is manual and nonpublishing;
- ordinary CI performs generic Linux checks plus native macOS/Windows truth and a compile-only MSRV check;
- CI never publishes;
- documentation builds are release-preflight work, not ordinary CI work;
- no evidence artifact is required for closure.

Do not rewrite architecture documents unrelated to validation or runtime features.

### Workstream F acceptance criteria

- [x] Active documentation lists commands that exist.
- [x] No active document claims docs or duplicate native tests run in ordinary CI.
- [x] Release remains manual.
- [x] Completed historical plan text is not broadly rewritten.

## Expected files

```text
scripts/check-local.sh
scripts/check-local.ps1
.github/workflows/ci.yml
crates/gregg/Cargo.toml
crates/gregg/src/main.rs
README.md
CONTRIBUTING.md
RELEASING.md
AGENTS.md
architecture/scripts-and-packaging.md
plans/063-narrow-correctness-and-simplification-roadmap.md
plans/065-proportionate-verification-and-footprint-cleanup.md
plans/README.md
```

Touch only files whose active content changes. Avoid repository-wide formatting churn.

## Lightweight verification sequence

### During script/workflow edits

```text
bash -n scripts/check-local.sh
./scripts/check-local.sh
```

On Windows or hosted Windows CI, exercise the PowerShell script's default path. A separate manually retained Windows transcript is not required.

### During runtime/dependency edits

```text
cargo test -p gregg --all-targets --all-features
cargo build --release -p gregg -p greggd
```

### Final local pass

```text
./scripts/check-local.sh
./scripts/check-local.sh --release
```

The release preflight is run once for final closure, not repeatedly during every edit.

### Hosted pass

One ordinary CI run at the final implementation SHA or a source-equivalent descendant is sufficient.

## Phase acceptance criteria

### Local verification

- [x] Default Unix script runs only format and workspace tests.
- [x] Default PowerShell script is equivalent.
- [x] Release preflight retains bounded manual release checks.
- [x] No collector test is repeated in the default path.

### CI

- [x] Linux runs format, Clippy, and full workspace tests without docs or duplicate collector tests.
- [x] macOS Intel and Apple Silicon native coverage remains.
- [x] Windows test commands are consolidated.
- [x] Rust 1.75 remains compile-checked once.
- [x] CI remains read-only, nonpublishing, artifact-free, and one workflow.

### Footprint

- [x] Tokio `test-util` is dev-only.
- [x] The current-thread runtime is retained only if behavior and size criteria pass.
- [x] Baseline/final binary sizes are recorded without a new artifact.
- [x] No core dependency is replaced.
- [x] No product feature or supported platform is removed.

### Closure

- [x] Phase 64 regressions remain in the normal test suite.
- [x] Default local check passes.
- [x] Manual release preflight passes once.
- [x] One ordinary CI run passes.
- [x] Roadmap 063, Phase 65, and `plans/README.md` are updated truthfully.
- [x] No follow-up evidence or CI-polish phase is created unless a concrete defect remains.

### Closure record

Local default and release preflight checks passed. The `gregg` release binary
measured 4,397,048 bytes before and after the retained Tokio/runtime changes;
`greggd` measured 2,628,080 bytes both times. Ordinary CI run `30964819950`
passed Linux, Windows, macOS arm64, macOS Intel, and Rust 1.75 jobs.

## Handoff notes

Prefer one commit for verification/script changes and one commit for runtime/dependency cleanup, followed by a concise planning-status update. Do not split this phase into more plans solely to record CI results.

If the current-thread runtime is reverted, Phase 65 can still close when the test-only feature cleanup and verification simplification meet their criteria. A measured no-op is an acceptable result; an architecture rewrite is not.
