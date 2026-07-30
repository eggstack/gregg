# Phase 38: local-first validation and minimal source-only CI

## Objective

Replace the current broad, release-adjacent verification model with one fast local validation entry point and one small GitHub Actions workflow that verifies source changes only.

The result must be easy to run repeatedly during development, easy to understand, and cheap to maintain. CI should detect ordinary regressions; it should not reproduce every supported architecture, preserve evidence artifacts, execute publication dry-runs on every change, or act as a release authority.

## Dependency and execution position

Depends on Phase 37 removing the retired release system.

Must complete before:

- Phase 39 finalizes the manual release runbook;
- Phase 44 adds Windows native coverage to the simplified CI workflow.

Phase 40 may begin in parallel once the simplified local command contract is stable.

## Governing invariants

1. Local validation is the primary comprehensive check.
2. CI is a representative source gate, not an exhaustive release qualification matrix.
3. The local entry point is a thin command runner, not a framework.
4. CI and local commands share underlying Cargo commands to avoid semantic drift.
5. No validation path publishes, tags, creates releases, retrieves prior artifacts, or writes evidence manifests.
6. Normal local validation completes quickly enough for repeated use.
7. Long soaks, packaging checks, and publish dry-runs are explicit opt-in modes.
8. CI output is ordinary job logs; successful jobs do not upload evidence bundles.
9. Platform-native tests run only where meaningful.
10. The workflow contains no hardcoded release version.

## Scope

### In scope

- a repository-owned local validation script or task entry point;
- ordinary CI restructuring;
- a small Linux/macOS matrix initially, with an obvious Windows extension point;
- an MSRV compile check;
- product smoke-test selection;
- shell/Python helper syntax checks where those helpers remain active;
- documentation for common validation modes;
- cache simplification;
- removal of CI-only architecture assertions that do not add product confidence.

### Out of scope

- crate publication or dry-run publication on every CI run;
- GitHub Releases;
- release artifact creation;
- release evidence uploads;
- exact-SHA qualification;
- every architecture on every push;
- 24-hour or similarly long soak tests;
- third-party code-coverage services;
- benchmark trend infrastructure;
- Windows collector tests before Phase 42;
- package-manager distribution testing.

## Workstream A: define validation tiers

Create three explicit tiers.

### Tier 1: fast developer check

Target command:

```text
./scripts/check-local.sh
```

Equivalent platform-appropriate invocation is acceptable, but there must be one canonical documented entry point.

The default should run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

Run `cargo deny check` by default if its typical runtime is acceptable. If dependency-index refresh makes it materially slow or unreliable offline, provide a clearly named opt-out or separate `--deps` tier, but document the canonical pre-merge command that includes it.

The default may also run short product smoke tests that:

- complete in seconds;
- are deterministic;
- require no elevated privileges;
- require no external network beyond loopback;
- apply to the current host OS.

### Tier 2: pre-merge/full local check

Target:

```text
./scripts/check-local.sh --full
```

In addition to Tier 1, this may run:

- retained mixed-fleet smoke;
- short sustained workload/resource checks;
- shell syntax and shellcheck for active scripts;
- Python unit tests for retained product helpers;
- package content/build checks that do not publish;
- installed-binary loopback smoke using local package output.

The full tier must still avoid release metadata, artifact bundling, external registries where possible, and long soaks.

### Tier 3: release preflight

This tier belongs conceptually to Phase 39 and may be invoked through:

```text
./scripts/check-local.sh --release
```

or documented as explicit commands in `RELEASING.md`.

It may add:

- clean-tree validation;
- version consistency checks (workspace inheritance + member crate
  consistency with the published `gregg-protocol` constraint);
- `cargo package --list` for each member crate;
- `cargo publish -p gregg-protocol --dry-run --locked` (only). Dependent
  crates depend on a protocol version not yet on crates.io, so their
  dry-runs remain manual until the protocol version is visible on
  crates.io. Do not execute them in the local release preflight.
- local install smoke from the current checkout using
  `cargo install --path crates/greggd --locked`.

It must not:

- contain credentials;
- call real `cargo publish`;
- create tags;
- create GitHub Releases;
- wait on or query GitHub Actions;
- generate provenance/evidence manifests.

### Workstream A acceptance criteria

- [ ] The three tiers are explicitly documented.
- [ ] The default tier is suitable for repeated development use.
- [ ] The full tier includes retained product smokes without release metadata.
- [ ] The release tier is nonpublishing.
- [ ] All tiers use ordinary exit codes and human-readable output.

## Workstream B: implement a thin local command runner

Preferred implementation: a portable shell script plus small platform-specific branches, or a small Rust/Python runner only if shell portability becomes materially awkward.

The runner must:

1. enable fail-fast command execution;
2. print each high-level command before running it;
3. preserve the failing command's exit code;
4. avoid parsing Cargo JSON unless a specific product check requires it;
5. avoid temporary state outside `target/` or the OS temporary directory;
6. clean temporary processes and files on failure;
7. skip non-applicable native tests with a clear message;
8. accept only a small stable option set;
9. include `--help`;
10. avoid dependencies that require separate installation beyond tools already documented for development.

Suggested interface:

```text
scripts/check-local.sh [--full] [--release] [--skip-deny] [--help]
```

Do not add configuration files, JSON task graphs, plugin systems, parallel executors, evidence directories, or workflow emulation.

### Platform handling

- Linux: run Linux collector/native smokes.
- macOS: run macOS collector/native smokes.
- Windows: Phase 40/44 may provide `scripts/check-local.ps1` or make the main entry point invoke a PowerShell equivalent. The command contract should remain aligned.

A Windows-native script is acceptable if a single cross-platform shell script would require Git Bash. Do not require Git Bash for supported Windows usage.

### Failure output example

Acceptable:

```text
==> cargo test --workspace --all-targets --all-features
error: test failed, to rerun pass `-p greggd --lib`
local check failed: workspace tests
```

Unacceptable:

```text
wrote target/evidence/check-run-2026-.../manifest.json
validation stage 17 failed contract binding
```

### Workstream B acceptance criteria

- [ ] One canonical local command exists and is documented.
- [ ] The runner is under roughly a few hundred straightforward lines rather than a framework.
- [ ] `--help` describes every mode.
- [ ] Failures return nonzero and identify the failed high-level check.
- [ ] Temporary product-smoke processes are cleaned up on failure.
- [ ] No evidence manifests or persistent run records are produced.

## Workstream C: simplify `.github/workflows/ci.yml`

### Target initial job structure

Before Windows implementation:

```text
linux:
  format
  clippy
  tests
  docs
  cargo deny
  short Linux native smoke

macos:
  build/tests
  short macOS native collector smoke

msrv:
  cargo check on Linux with Rust 1.75
```

After Phase 44:

```text
windows:
  build/tests
  short Windows native collector/runtime smoke
```

The exact split may use a matrix, but readability is more important than maximal YAML deduplication.

### CI commands

CI should call either:

- the same local script with a deterministic `--ci` or default subset; or
- the same Cargo commands directly when that is clearer.

Do not make CI dependent on a large local orchestration script if direct commands are simpler. Semantic alignment matters more than textual reuse.

### Remove unnecessary CI complexity

Remove or avoid:

- release workflow validators;
- package provenance checks;
- artifact upload for successful jobs;
- explicit architecture assertions using `uname`;
- matrix entries whose only purpose is duplicate evidence;
- cross-run dependencies;
- environment approvals;
- release-version inputs;
- source/package candidate reconstruction;
- publication dry-runs on every push;
- exact test-count validation;
- redundant full workspace checks repeated in every matrix member.

### Cache policy

Use simple per-OS Cargo caches if they produce reliable savings. Cache failure must not fail CI. Avoid complex cache keys tied to architecture evidence or release identities.

A valid minimal key shape:

```text
${runner.os}-cargo-${hashFiles('**/Cargo.lock')}
```

If target-directory caching causes stale or oversized caches, cache only Cargo registry/git directories.

### Trigger policy

Retain:

```text
push to main
pull_request
workflow_dispatch
```

No tag-triggered publication workflow is added.

### Permissions

Set minimal read-only permissions where practical:

```yaml
permissions:
  contents: read
```

No `id-token: write`, `packages: write`, `contents: write`, or release environment is required.

### Workstream C acceptance criteria

- [ ] CI contains only source/product verification jobs.
- [ ] CI has no write permissions needed for publication or releases.
- [ ] CI has no artifact upload step for successful verification evidence.
- [ ] Linux runs the complete representative source gate.
- [ ] macOS runs native macOS tests without duplicating every Linux-only gate.
- [ ] MSRV is checked once.
- [ ] The workflow is version-neutral.
- [ ] A new contributor can understand the workflow without reading helper contracts.

## Workstream D: right-size the test matrix

The repository advertises multiple architectures, but ordinary CI does not need to execute every architecture on every change.

### Required ordinary coverage

- one current hosted Linux runner;
- one current hosted macOS runner;
- one current hosted Windows runner after Phase 44;
- one MSRV compile check.

### Optional/manual coverage

Document but do not require on every PR:

- Linux ARM64 native smoke on owned hardware;
- Intel macOS native smoke when available;
- Windows ARM64 compile or native smoke in a future plan;
- long-duration resource/soak tests;
- package-install smokes on all architectures.

Do not claim CI proves an architecture it does not run. README wording should distinguish supported/tested behavior from the ordinary CI matrix if necessary.

### Workstream D acceptance criteria

- [ ] The matrix is representative rather than exhaustive.
- [ ] CI labels accurately describe the actual runner OS/architecture.
- [ ] No fake architecture proof is inferred from cross-compilation alone.
- [ ] Optional hardware tests are documented as optional, not release gates.

## Workstream E: retain meaningful product smokes

Select a small number of high-value smokes:

1. daemon starts on loopback using a temporary config;
2. `/healthz` and `/v1/status` or `/v2/status` behave according to readiness;
3. collector reaches ready state after warm-up;
4. client can poll a fixture/mocked daemon and reduce state;
5. terminal-independent rendering tests cover supported/unsupported fields;
6. short sustained polling remains bounded.

Avoid end-to-end tests that require service installation, root privileges, launchd/systemd mutation, or LAN access in ordinary CI.

Service lifecycle testing belongs in platform-specific unit tests and optional elevated manual checks.

### Smoke duration limits

- ordinary CI smoke: target under 30 seconds per job;
- local full smoke: target under a few minutes total;
- longer tests: ignored/explicit command only.

### Workstream E acceptance criteria

- [ ] Every CI smoke asserts product behavior.
- [ ] No smoke exists solely to generate evidence.
- [ ] Smokes use loopback and temporary directories.
- [ ] Smokes clean child processes on success and failure.
- [ ] Default CI duration remains appropriate for quick iteration.

## Workstream F: documentation

Update contributor documentation with:

```text
Fast local check:
  ./scripts/check-local.sh

Full local check:
  ./scripts/check-local.sh --full

Release preflight:
  see RELEASING.md
```

Explain that:

- local checks are canonical for development;
- CI is intentionally small;
- publication is never performed by CI;
- optional native hardware checks are welcome but not part of every PR;
- failures should be reproduced locally with the underlying command.

Remove exact claims about hundreds of release-tooling tests or retained evidence.

### Workstream F acceptance criteria

- [ ] README or CONTRIBUTING links to the local validation command.
- [ ] The distinction between default, full, and release checks is clear.
- [ ] No document implies that CI publication exists.
- [ ] No document requires downloading CI artifacts to validate a change.

## Test cases and failure scenarios

The implementation must explicitly test or manually exercise:

1. default local check success;
2. default local check failure propagates nonzero;
3. `--help` succeeds without running checks;
4. unknown option fails clearly;
5. full mode invokes retained product smokes;
6. release mode stops before real publication;
7. child daemon cleanup occurs after a failing smoke;
8. CI runs from a clean checkout without generated evidence files;
9. CI succeeds without repository write permissions;
10. cache miss does not affect correctness;
11. non-native smoke is skipped rather than falsely reported as passed;
12. MSRV job performs only the intended compile check.

## Phase acceptance criteria

Phase 38 is complete only when:

- [ ] A canonical fast local validation command exists.
- [ ] A full local validation mode exists for retained product smokes.
- [ ] Any release-preflight mode is nonpublishing and credential-free.
- [ ] The local runner is thin, readable, and has no evidence framework.
- [ ] `.github/workflows/ci.yml` is source-only and version-neutral.
- [ ] CI has minimal read permissions and no release environment.
- [ ] CI uploads no success evidence artifacts.
- [ ] Linux, macOS, and MSRV checks are right-sized and passing.
- [ ] Windows has a clear insertion point without matrix redesign.
- [ ] Product smokes are short, loopback-only, and cleanup-safe.
- [ ] Local and CI documentation is current.
- [ ] The workflow can be understood and modified without specialized release knowledge.

## Evidence required for completion

Only:

- passing local command output summarized in the handoff;
- a passing ordinary CI run;
- the simplified workflow diff;
- smoke cleanup verification.

Do not create a qualification artifact, checksummed evidence directory, or cross-run record.

## Handoff notes for a smaller implementation model

1. Implement the local command first so CI decisions can reuse its command set.
2. Keep options minimal; reject requests for generalized task configuration.
3. Simplify CI after the local command passes.
4. Run the workflow validator only if it is a generic YAML/syntax check; do not recreate the deleted release validator.
5. Prefer one Linux comprehensive job and lighter native jobs over repeating all gates everywhere.
6. Do not add Windows placeholders that falsely pass; Phase 44 adds real Windows coverage.
7. Measure actual runtime once and remove or move any unexpectedly slow default check to `--full`.
8. End with documentation and a clean `git status`.