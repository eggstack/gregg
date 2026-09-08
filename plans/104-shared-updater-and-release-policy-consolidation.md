# Plan 104: shared updater and release-policy consolidation

Status: complete at implementation `27ec978`.

Depends on: Plan 103; existing update/release behavior from Plans 099-102.

## Objective

Eliminate duplicated updater and release-policy implementations while preserving the exact accepted behavior of `gregg update`, `greggd update`, installers, and the five-target release pipeline.

This is a maintenance/consolidation plan, not a release redesign.

## Baseline

The client and daemon updater modules independently implement nearly the same mechanism:

```text
version lookup
stable SemVer parsing/comparison
target detection
asset-name construction
GitHub URL construction
curl discovery/execution
Cargo fallback discovery/execution
private staging
SHA-256 verification
candidate `version` verification
self replacement
shared error classification
```

`greggd` legitimately adds daemon lifecycle state and restart/activation behavior after the shared candidate preparation/replacement work.

The release workflow separately embeds target/asset/version policy in shell, creating additional drift risk between:

- Rust updater code;
- Unix installer;
- Windows installer;
- release workflow.

The public contract should remain unchanged, but policy should have fewer authoritative copies.

## Required design

### 1. Introduce one private shared updater crate

Add a workspace-internal crate, suggested name:

```text
crates/gregg-update
```

This crate is implementation infrastructure, not a new user-facing product.

It should own only cross-program update mechanics. Appropriate responsibilities include:

- stable-version parsing/comparison;
- current/latest version decision helpers;
- supported target mapping;
- release asset naming/URL construction;
- download/build execution and timeout ownership;
- SHA-256 verification;
- candidate identity/version verification;
- staging lifetime/cleanup;
- executable replacement;
- shared update source/outcome primitives where useful.

Do not put updater code into `gregg-protocol`. The protocol crate must remain a pure wire-contract boundary.

Do not make the new crate public API-heavy. Keep its interface small and parameterized by program/crate identity.

Conceptually, a caller should be able to provide something equivalent to:

```rust
UpdateSpec {
    crate_name,
    program_name,
    current_version,
    repository,
}
```

and receive a prepared/replaced result without the shared crate knowing anything about systemd, launchd, cron, SCM, TUI state, or EggPool.

Exact naming is not prescribed.

### 2. Keep daemon activation policy in `greggd`

`greggd` must retain ownership of:

- `startup_state()` interpretation;
- Windows SCM stop/start ordering;
- systemd/launchd/cron/direct restart semantics;
- `UpdatedButRestartFailed` presentation if that remains the accepted public outcome;
- permission/elevation guidance specific to daemon installation location.

The shared updater must not learn service-manager concepts merely to maximize code sharing.

The accepted transaction rule from Plan 102 remains mandatory: fully prepare and verify the candidate before quiescing a running daemon service where required.

### 3. Preserve the exact release asset contract

The following remain unchanged unless a correctness bug is independently demonstrated:

```text
tag: vX.Y.Z
asset: <program>-<target>[.exe]
checksum: <asset>.sha256
```

Supported prebuilt targets remain:

```text
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
x86_64-apple-darwin
aarch64-apple-darwin
x86_64-pc-windows-msvc
```

Linux GNU artifacts retain the documented glibc 2.17 floor. ARMv7 remains source-build-only unless separately planned.

### 4. Reduce release workflow duplication through local scripts

Inspect `.github/workflows/release-binaries.yml` and move only clearly reusable policy/check logic into scripts, for example:

```text
scripts/release-preflight.sh
scripts/release-smoke.sh
scripts/release-stage-assets.sh
```

The exact split is flexible. The goals are:

- the workflow becomes orchestration rather than the sole implementation of release checks;
- target/asset/version/checksum rules can be exercised locally;
- shell duplication across native target jobs is reduced where practical;
- platform-specific build commands remain explicit where abstraction would obscure behavior.

Do not force the Linux Zig/glibc job, macOS jobs, and Windows job into one opaque abstraction solely to shrink YAML.

Do not add another release framework or third-party release orchestrator.

### 5. Establish one policy source where practical

Target and asset naming must not silently diverge across Rust and shell/PowerShell paths.

Use the smallest maintainable mechanism. Acceptable approaches include:

- one simple checked-in machine-readable target table consumed by scripts and tested against Rust constants;
- generated shell constants from one tiny file;
- deterministic cross-check tests that assert installer/workflow asset names match the Rust updater contract.

Do not introduce a code generator, build script, or serialization dependency unless it is clearly smaller than a direct cross-check.

## Implementation sequence

### Step 1: characterize existing behavior

Before moving code, add or preserve focused tests for:

- stable version parsing/comparison;
- host-to-target mapping;
- Windows `.exe` asset naming;
- exact GitHub release URL construction;
- binary path checksum mismatch rejection;
- candidate program/version mismatch rejection;
- Cargo fallback only on the already accepted absence condition;
- timeout child kill/reap semantics;
- staging cleanup;
- replacement error classification.

These tests become the behavioral harness for extraction.

### Step 2: extract shared update mechanics

Create the internal crate and move shared code mechanically.

Avoid combining extraction with behavioral changes. Prefer a sequence where existing tests continue to pass after each ownership move.

Both application crates should depend on the internal crate by workspace path. Decide separately whether the crate is publishable; if publishing all workspace dependencies is required by crates.io packaging, configure it deliberately rather than accidentally.

If publishing a fourth crate materially complicates the release model, an alternative private shared module arrangement may be used only if it truly provides one maintained implementation for both binaries and works with crates.io packaging. Record the packaging tradeoff in the plan closure.

### Step 3: reduce application updater modules

After extraction:

```text
crates/gregg/src/update.rs
```

should primarily adapt CLI-facing outcomes/errors to the shared mechanism.

```text
crates/greggd/src/update.rs
```

should primarily coordinate shared candidate/replacement behavior with daemon lifecycle state.

Do not retain copied helper bodies merely to avoid touching call sites.

### Step 4: consolidate release policy scripts

Move repeated release checks/staging/smokes into scripts and update the workflow to call them.

All scripts must:

- use strict shell/PowerShell error handling appropriate to the platform;
- produce actionable diagnostics;
- be runnable independently where platform permits;
- avoid network access in tests unless the existing release preflight explicitly requires it.

### Step 5: synchronize docs

Update at minimum as applicable:

```text
architecture/overview.md
architecture/gregg-client.md
architecture/greggd-daemon.md
architecture/scripts-and-packaging.md
AGENTS.md
.opencode/skills/* relevant to updater/release ownership
RELEASING.md
CHANGELOG.md
plans/README.md
```

Documentation must describe one shared updater mechanism plus daemon-specific lifecycle ownership.

## Verification

Mandatory local checks:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
```

Also run focused local checks that prove:

- both `gregg version` and `greggd version` still report the workspace version;
- updater target/asset helpers produce the exact five public names;
- release-policy scripts pass their local/preflight modes without creating releases;
- no updater path invokes `sudo` internally;
- a failed prepared update does not mutate the installed executable;
- daemon-specific restart behavior remains outside the shared updater crate.

On the local Ubuntu host, perform a daemon lifecycle smoke after the refactor:

```text
start greggd directly in foreground or controlled test process
wait for /v2/healthz
verify valid Gregg response
exercise stop/shutdown path
confirm no systemctl invocation is required for `run`
```

Use the existing native CI jobs for macOS/Windows compatibility after local completion. No new CI matrix is required.

## Acceptance criteria

Plan 104 is complete only when:

1. `gregg` and `greggd` no longer carry independent copies of version/target/asset/download/checksum/staging/replacement helpers.
2. The shared implementation has no dependency on protocol schema, TUI, EggPool, or service-manager concepts.
3. `greggd` still owns daemon activation/restart policy and preserves Plan 102 transaction ordering.
4. All five prebuilt target names and checksum names are unchanged.
5. Cargo fallback and binary-first semantics are unchanged.
6. Release workflow duplication is reduced without hiding platform-specific build behavior.
7. Release-policy checks are locally runnable where practical.
8. Installers, updater, and release workflow have an explicit drift-prevention test/check for asset naming.
9. Ordinary CI remains ordinary correctness CI; release workflow remains tag/manual release-only.
10. No automatic crates.io publication, tag creation, or automatic public release is introduced.
11. Full local verification passes and the existing native CI jobs are green.
12. Architecture and planning docs identify the new ownership boundary accurately.

## Closure record

Implemented in `27ec978` (September 2026):

- New publishable member `crates/gregg-update` owns version/target/asset/
  download/checksum/staging/replacement plus the shared `UpdateError` /
  `UpdateOutcome` / `UpdateSpec` / `UpdatePlan` / `prepare_candidate` /
  `run_simple_update` surface. It depends only on `serde_json`,
  `thiserror`, `sha2`, `self-replace`, `tempfile`: no protocol, TUI,
  EggPool, or service-manager concepts. The shared `RestartFailed` error
  variant is constructed only by `greggd` coordination (opaque string, no
  manager types leak into the crate).
- `crates/gregg/src/update.rs` is a thin adapter (identity + exact outcome
  strings, full flow via `run_simple_update`); `crates/greggd/src/update.rs`
  is a lifecycle coordinator (identity, `prepare_candidate` → Windows
  quiesce only after preparation → replace → manager-aware restart,
  `UpdatedButRestartFailed` preserved). Net deletion of ~1700 duplicated lines.
- Release policy: `scripts/release-targets.txt` (single table),
  `scripts/release-preflight.sh` (version/tag/registry, locally runnable),
  `scripts/release-check-assets.sh` (staged-asset validation),
  `scripts/release-install-zig.sh` (shared Zig setup); the workflow calls
  them, keeping platform build commands explicit. Drift prevention:
  `supported_targets_match_release_table` unit test plus the asset-check
  script deriving names from the same table.
- Contract proof: `gregg version` / `greggd version` report `1.0.12`, no
  `sudo` invocation in any updater path, Ubuntu foreground lifecycle smoke
  (`run` → `/v2/healthz` ready → `stop` → exit, no systemd), full local
  verification green (`fmt`, `clippy --all-targets --all-features
  -D warnings`, `test --workspace --all-targets --all-features`, `doc`).
  Preflight script passes locally; `release-check-assets.sh` verified
  against empty (fails loudly) and complete (20 files OK) fixtures.
- Publication order extended to
  `gregg-protocol` → `gregg-update` → `greggd` → `gregg`
  (`RELEASING.md`, release-process skill, `check-local.sh` package list).
