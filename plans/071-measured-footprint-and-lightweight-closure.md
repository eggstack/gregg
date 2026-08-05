# Phase 071: measured footprint and lightweight closure

Status: complete.

Depends on: Plans 067, 068, and 069. Plan 070 may complete with retained changes or a documented no-change result.

## Objective

Close Roadmap 066 with measured, low-risk binary-footprint cleanup and the repository's existing lightweight verification model. Retain only changes that preserve all features and supported platforms. Do not create a size program, release workflow, evidence bundle, or additional closure phase.

## Scope

### In scope

- Record fresh release-binary byte sizes before Phase 071 changes.
- Remove Reqwest features that production source does not use.
- Evaluate `panic = "abort"` for release binaries after checking for unwind-dependent code.
- Optionally evaluate full LTO as an isolated candidate.
- Confirm compatibility-only dependency pins remain justified by the current Rust 1.75 policy; do not remove them speculatively.
- Run the existing default check, one existing manual release preflight, and one ordinary hosted CI workflow.
- Reconcile active documentation and plan statuses.

### Out of scope

- Removing EggPool, HTTPS, cross-platform support, service management, v1 compatibility, drive metrics, or TUI features.
- Replacing Axum, Ratatui, Clap, Tokio, Reqwest, Rustls, Serde, or TOML.
- Raising or lowering MSRV.
- Dependency-upgrade campaigns or lockfile churn unrelated to retained changes.
- UPX or external binary compression.
- Custom allocators, `no_std`, hand-written HTTP, or terminal rewrites.
- Permanent benchmarks, CI size thresholds, artifact uploads, repeated qualification runs, or automated publishing.
- Archiving historical plans as part of implementation closure.

## Baseline procedure

At the final pre-Phase-071 implementation commit:

```bash
cargo build --release -p gregg
cargo build --release -p greggd
```

Record exact byte counts using a platform-appropriate command, for example:

```bash
stat -f '%z %N' target/release/gregg target/release/greggd   # macOS
stat -c '%s %n' target/release/gregg target/release/greggd   # Linux
```

Record the commit SHA, Rust version, target triple, and byte counts in the implementation handoff or the closure section of this plan. Do not add a generated size file.

Use clean rebuilds for candidate comparisons when profile or feature changes could leave incomparable artifacts:

```bash
cargo clean -p gregg
cargo clean -p greggd
```

Do not repeatedly clean the entire workspace unless necessary.

## Workstream A: truthful Reqwest features

### Required inspection

Search production source for Reqwest JSON helpers:

```text
.json(...)
Response::json
RequestBuilder::json
```

The current client streams bounded bodies and deserializes with direct `serde_json` calls. If that remains true, remove the Reqwest `json` feature while preserving `rustls-tls` and `stream`.

Requirements:

- production and test builds pass;
- HTTPS EggPool behavior remains compiled;
- bounded body streaming remains unchanged;
- direct `serde_json` dependency remains where used;
- no replacement feature or dependency is added.

Retain this manifest cleanup even if linked byte size is unchanged, because it makes the declared feature set accurate, provided there is no behavior or MSRV regression.

## Workstream B: release panic strategy

### Precondition

Search for unwind-dependent behavior:

```text
catch_unwind
resume_unwind
AssertUnwindSafe
panic hooks that assume recovery
FFI contracts requiring unwinding
```

The terminal panic hook may restore terminal state before abort, but verify the actual behavior. If panic unwinding is intentionally used for recovery, do not adopt abort.

### Candidate

Evaluate:

```toml
[profile.release]
panic = "abort"
```

Requirements:

1. Both binaries build on the current host.
2. All ordinary tests pass under the normal test profile.
3. A release-mode smoke confirms `gregg --help`, `greggd --help`, and the installed-daemon loopback still work.
4. Terminal restoration behavior on ordinary clean exit is unchanged.
5. Both release binaries are no larger than baseline; retain only if at least one has a clear reduction.
6. No crate-level panic strategy overrides or platform-specific profile sections are added.

A panic remains a process failure. Do not add panic recovery code to compensate for abort semantics.

If any requirement fails, revert the profile change completely and record the reason.

## Workstream C: optional full LTO comparison

This candidate is lower priority and may be skipped when build cost is disproportionate.

Evaluate `lto = "fat"` only after measuring the retained panic/feature configuration. Restore the exact retained baseline before testing it.

Retain full LTO only when all are true:

- at least one release binary decreases by at least 1%;
- neither binary increases;
- the release preflight remains practical for this small project;
- no target-specific linker failure appears in ordinary CI;
- no additional Cargo profile complexity is introduced.

Otherwise keep thin LTO. Do not add a selectable size profile.

## Workstream D: compatibility pins and MSRV truth

The client manifest contains direct version constraints that may exist to keep the workspace compatible with Rust 1.75. Inspect comments, lockfile history, and `cargo tree` before changing them.

Rules:

1. Rust 1.75 remains the policy in this roadmap.
2. Do not delete a direct compatibility pin merely because source code does not import the crate.
3. Remove a pin only when the dependency graph no longer needs it under Rust 1.75 and the MSRV job remains green.
4. Do not run broad `cargo update`.
5. Do not add replacement pins.
6. If justification is unclear, retain the pin and state that dependency-resolution cleanup requires a separate MSRV policy decision.

Binary-size expectations must be realistic: resolver pins usually affect build compatibility more than linked output.

## Workstream E: active documentation and plan closure

Update only active material affected by the retained implementation:

```text
README.md
architecture/*.md directly affected
AGENTS.md or CONTRIBUTING.md only if commands changed
plans/066-071 status sections
plans/README.md
```

Required closure wording:

- Roadmap 066 is complete only when correctness criteria are implemented, not merely planned.
- Plan 070 records retained changes or explicit no-change results for both subsystems.
- Plan 071 records baseline/final byte counts and which candidates were retained or reverted.
- Manual release and existing CI policy remain unchanged.
- No follow-up evidence or CI-polish plan is created unless a concrete product defect remains.

## Verification sequence

### Focused checks

Run tests directly affected by final manifest/profile changes:

```bash
cargo test -p gregg
cargo test -p greggd
cargo test -p gregg-protocol
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

### Routine local check

```bash
./scripts/check-local.sh
```

### Single manual release preflight

```bash
./scripts/check-local.sh --release
```

Run this once after all retained changes are finalized. It remains nonpublishing.

### Hosted closure

Push the final implementation and require one ordinary existing CI run. Linux generic checks, native macOS/Windows checks, and the existing Rust 1.75 check are sufficient. Do not add jobs, artifacts, repeated runs, or a candidate workflow.

## Final acceptance criteria

### Correctness inheritance

- [x] Plan 067 drive availability behavior and compatibility tests pass.
- [x] Plan 068 coherent state and Windows health behavior tests pass.
- [x] Plan 069 config intent, runtime boundary, exit-code, and scheduler-test corrections pass.
- [x] Plan 070 truthfully records retained or rejected client simplifications.

### Footprint

- [x] Fresh baseline and final byte counts are recorded for `gregg` and `greggd` on one target.
- [x] Reqwest production features match actual source usage.
- [x] `panic = "abort"` is retained only after unwind inspection, smoke checks, and non-regressing size measurement.
- [x] Full LTO is retained only if it meets the explicit reduction and practicality threshold; otherwise thin LTO remains.
- [x] No feature or supported platform is removed.
- [x] Rust 1.75 policy remains unchanged.
- [x] No permanent benchmark, size gate, alternate profile, or compression step is added.

### Verification and closure

- [x] Focused tests and Clippy pass.
- [x] `./scripts/check-local.sh` passes.
- [x] One `./scripts/check-local.sh --release` run passes.
- [ ] One ordinary cross-platform CI run passes at the final implementation SHA or a source-equivalent plan-only descendant.
- [ ] Active documentation and Plans 066-071 describe implemented reality.
- [ ] CI remains one read-only, nonpublishing, artifact-free workflow.
- [ ] Release remains manual.
- [ ] No evidence bundle or closure-only follow-up phase is created.

## Closure record template

Append a concise section when complete:

```text
Implementation SHA:
Host target and rustc:
Baseline gregg bytes:
Final gregg bytes:
Baseline greggd bytes:
Final greggd bytes:
Reqwest feature change: retained/reverted
panic=abort: retained/reverted, reason
fat LTO: retained/reverted/skipped, reason
Plan 070 scheduler: retained/no change
Plan 070 EggPool: retained/no change
Default local check:
Release preflight:
Ordinary CI run:
```

Do not create a separate evidence file.

## Completion

Implementation SHA: `a53542b1f04732888cd8a4f0812fa1d2c0dac3bb`
Host target and rustc: `aarch64-unknown-linux-gnu`, `rustc 1.97.1 (8bab26f4f 2026-07-14)`
Baseline gregg bytes: `4,331,512`
Final gregg bytes: `3,478,400`
Baseline greggd bytes: `2,497,000`
Final greggd bytes: `1,972,672`
Reqwest feature change: no manifest change required; production source uses
bounded streaming and direct `serde_json`, and the manifest already declares
only `rustls-tls` and `stream`.
panic=abort: retained; no unwind-dependent production behavior was found,
ordinary tests remain on the normal test profile, release help and loopback
smokes passed, and both binaries decreased.
fat LTO: retained; both binaries decreased by more than 1% from the retained
abort-only candidate, and release builds and smokes passed on the host.
Plan 070 scheduler: no change retained; endpoint isolation, ordering, bounded
concurrency, cadence, and panic-to-`Cancelled` behavior remain intact.
Plan 070 EggPool: no change retained; bounded command/result channels and
generation checks remain the smaller behavior-preserving design.
Default local check: passed (`./scripts/check-local.sh`)
Release preflight: passed once (`./scripts/check-local.sh --release`)
Ordinary CI run: pending push of the final implementation
