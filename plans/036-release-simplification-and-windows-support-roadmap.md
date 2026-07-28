# Roadmap: local-first release simplification and Windows support

## Purpose

This roadmap replaces Gregg's current release-control program with a deliberately small operating model and then adds Windows support without rebuilding a complex release pipeline around it.

Gregg is a narrow system-monitoring tool. Its release process must remain proportionate to that scope:

- development confidence comes primarily from fast local tests;
- GitHub Actions verifies source changes but does not publish, finalize, aggregate, authorize, or preserve release evidence;
- crates.io publication is performed manually by an operator;
- Git tags and GitHub Releases are created manually by an operator;
- a failed release attempt is handled by fixing the repository, incrementing the immutable crate version where necessary, and trying again;
- Windows support is implemented as ordinary product work, with enough native testing to prove correctness but no platform-evidence bureaucracy.

This roadmap supersedes the release-orchestration and evidence requirements in Plans 010 through 022 and 030 through 035. Those files remain historical records until Phase 37 archives or removes them from the active plan index. They are not requirements for future publication.

## Problem statement

The current repository has accumulated substantially more release machinery than product machinery requires. The active process includes staged dispatch workflows, candidate identity contracts, artifact retrieval, provenance merging, release selection documents, finalization modes, evidence role materialization, qualification contracts, and large negative-test suites for the release system itself.

That system creates several costs:

1. Every correction to the release process creates another round of release-process verification.
2. Version-specific workflow assumptions make ordinary maintenance expensive.
3. CI failures frequently represent evidence-orchestration defects rather than product defects.
4. Repository documentation and plans overemphasize release mechanics.
5. The process discourages rapid iteration on a small utility.
6. Adding Windows to the existing model would multiply the evidence and matrix burden.

The replacement model is intentionally modest:

```text
edit -> local check -> push/PR -> small source CI -> merge
     -> operator local release preflight
     -> manual crates.io publish in dependency order
     -> manual annotated tag and GitHub Release
```

No automated publication path exists.

## Governing principles

### 1. Local-first validation

The canonical comprehensive developer check is a repository-owned local command. CI runs a small, representative subset of the same underlying Cargo commands. Local validation must be easy to execute repeatedly and must not generate or require retained evidence bundles.

### 2. CI is a source gate, not a release system

CI may compile, lint, test, and optionally run short native smoke tests. CI must not:

- invoke `cargo publish`;
- create or push tags;
- create GitHub Releases;
- require crates.io credentials;
- require a protected release environment;
- select prior workflow runs or artifacts;
- aggregate cross-run evidence;
- upload release-provenance bundles;
- encode operator decisions as workflow inputs;
- hardcode a release version.

### 3. Manual release is explicit and reversible until publication

The operator performs a concise checklist locally. Once a crate version has been published to crates.io it is immutable. If publication partially succeeds, the next attempt uses a new version; no automation attempts to repair or overwrite a published version.

### 4. Product tests are retained; release-only tests are removed

Tests for collectors, protocol validation, config persistence, polling, HTTP behavior, TUI state, resource bounds, and installed-binary behavior remain valuable. Tests whose only purpose is validating release workflows, evidence schemas, provenance graphs, artifact selection, or finalizer contracts are removed with that machinery.

### 5. Windows semantics must be truthful

Windows support must not fabricate Unix load averages, label Windows commit accounting as Unix swap, or report unsupported metrics as measured zero. Protocol evolution must represent platform capabilities explicitly.

### 6. Windows support is staged by usable surfaces

The client can become Windows-capable before the daemon. The daemon requires protocol work, a native collector, Windows service integration, packaging guidance, and native tests.

### 7. Minimal dependencies and contained unsafe code

Windows API access should use a target-specific dependency and a contained module boundary. Unsafe FFI is permitted only where required and must not weaken the workspace-wide default for unrelated code.

## Target repository state

At completion:

- `.github/workflows/ci.yml` is the only required workflow;
- CI runs a small Linux/macOS/Windows source matrix and an MSRV check without publication or artifact choreography;
- one local validation entry point runs the complete practical test suite;
- `RELEASING.md` documents a manual crates.io and GitHub release in dependency order;
- active documentation contains no release-evidence phases, finalizer contracts, run-selection documents, or immutable artifact ledgers;
- the `gregg` client works on Windows;
- the protocol truthfully represents optional load, swap, I/O-wait, and Windows commit metrics;
- `greggd` collects and serves Windows metrics;
- `greggd` can run in the foreground and as a Windows service;
- Windows native CI covers compilation, unit tests, and a short runtime smoke without retaining special evidence artifacts;
- releases remain manual regardless of platform count.

## Phase map

| Phase | Plan | Outcome |
| --- | --- | --- |
| 37 | `037-remove-release-orchestration-and-archive-history.md` | Delete active release automation/evidence machinery and make old release plans historical. |
| 38 | `038-local-first-validation-and-minimal-ci.md` | Establish one fast local validation path and a small source-only CI workflow. |
| 39 | `039-manual-cratesio-and-github-release.md` | Add a concise, version-neutral operator runbook for manual crates.io and GitHub releases. |
| 40 | `040-windows-client-portability.md` | Make the `gregg` client correct on Windows, including paths, locking, editing, persistence, and TUI behavior. |
| 41 | `041-capability-aware-protocol-v2.md` | Add truthful optional metric semantics and v1 compatibility for heterogeneous fleets. |
| 42 | `042-windows-native-metrics-collector.md` | Implement Windows identity, CPU, memory, and commit collection behind the existing collector boundary. |
| 43 | `043-windows-service-lifecycle-and-packaging.md` | Add Windows service runtime/control, machine config, installation, and lifecycle behavior. |
| 44 | `044-windows-ci-integration-and-release-readiness.md` | Integrate Windows native testing, documentation, mixed-platform smoke coverage, and final closure. |

## Dependency graph

```text
37 -> 38 -> 39

37 -> 40
40 -> 41
41 -> 42
42 -> 43
40 + 41 + 42 + 43 -> 44

38 is required before 44 so Windows CI is added to the simplified workflow,
not to the retired release system.

39 may complete before Windows work and remains valid for later releases.
```

Phase 41 may begin in parallel with late Phase 40 work once Windows client persistence and build portability are understood. Phase 42 must not finalize its wire output before Phase 41 freezes the capability model.

## Program-level scope

### In scope

- removal or archival of release-only workflows, scripts, schemas, tests, evidence ledgers, and documentation;
- a small local validation command or script;
- simplification of ordinary GitHub Actions CI;
- manual crates.io publication documentation for all workspace crates;
- manual tag and GitHub Release documentation;
- Windows client support;
- protocol capability evolution necessary for Windows;
- Windows native collection;
- Windows service integration;
- Windows packaging and operational documentation;
- representative native Windows CI.

### Out of scope

- automated crates.io publication;
- automated GitHub Release creation;
- release candidates, promotion channels, provenance ledgers, attestation frameworks, or artifact signing infrastructure;
- binary distribution through MSI, MSIX, winget, Chocolatey, Homebrew, apt, RPM, or container registries;
- TLS, authentication, public-internet hardening, service discovery, dashboards, history, alerting, or per-process monitoring;
- emulating Unix load averages on Windows;
- treating pagefile/commit metrics as interchangeable with Unix swap;
- requiring every supported architecture in ordinary hosted CI;
- retaining CI artifacts solely to prove that CI ran;
- a generic cross-platform service framework beyond Gregg's three supported OS families.

## Release simplification invariants

1. `cargo publish` appears only in operator documentation and never in executable CI or repository automation.
2. A GitHub token is not required to validate or publish crates locally.
3. A crates.io token is used only by the operator's local Cargo credential mechanism.
4. The release checklist is version-neutral and uses a shell variable or explicit placeholder rather than hardcoded `1.0.1` logic.
5. Publication order is `gregg-protocol`, then `greggd`, then `gregg`.
6. Dependent crates are not published until the protocol version resolves from crates.io.
7. Git tagging and GitHub Release creation occur after the intended crate publications succeed.
8. No generated evidence directory is required before merge or publication.
9. CI logs are sufficient CI output; no release evidence artifact is uploaded.
10. A partial crates.io release is documented as an operator-visible state requiring a new version for any corrected republish.

## Windows product invariants

1. Windows support is explicit in `cfg(target_os = "windows")`; unsupported fallbacks do not silently claim success.
2. `gregg` uses a user-scoped Windows config directory.
3. `greggd` uses a machine-scoped Windows config directory suitable for a service.
4. Cross-process config mutation is serialized on Windows.
5. Atomic persistence has Windows-specific tests, including replacement of an existing file and sharing-violation behavior.
6. The Windows collector warms CPU counters before producing a ready utilization sample.
7. Unsupported I/O-wait, load, or swap metrics are absent and capability-declared, not zero-filled.
8. Windows commit usage is modeled and labeled as commit usage.
9. Windows service commands fail truthfully when the service is not installed or access is denied.
10. Foreground execution remains supported and testable independently of the service manager.
11. Native Windows tests do not require retained release artifacts.
12. The daemon remains private-network software with the existing security boundary.

## Validation strategy

### Local validation

The repository should expose a single documented local entry point, expected to run approximately:

```text
format check
workspace clippy
workspace tests
workspace docs
cargo deny
short product smokes supported by the current host
package dry-runs when explicitly requested
```

The normal development path must not run package publication checks or long soaks unless selected through an explicit optional flag.

### CI validation

The target CI shape is intentionally small:

- stable Linux: workspace format, lint, tests, docs, dependency policy;
- stable macOS: workspace build/tests and native macOS collector smoke;
- stable Windows: workspace build/tests and native Windows collector/runtime smoke once implemented;
- one Linux MSRV `cargo check` job;
- no artifact upload unless needed temporarily to debug a failing job;
- no release workflow.

Architecture labels must describe the actual hosted runner. CI need not reproduce every advertised architecture on every push.

### Manual release validation

Manual release preflight adds only package-specific checks that are inappropriate for every development commit:

```text
clean tree
version and changelog review
local validation
cargo package --list
cargo publish --dry-run --locked
manual publication
registry resolution check
manual tag
manual GitHub Release
post-release cargo install smoke
```

## Risks and controls

### Risk: deleting useful product verification with release tooling

Control: classify files by ownership before deletion. Retain any script that directly tests installed binaries, runtime behavior, collectors, sustained polling, or resource use independent of release evidence.

### Risk: CI becomes too weak

Control: keep deterministic source gates and representative native OS tests. Move exhaustive but fast tests into the local validation command rather than removing them.

### Risk: local script becomes another orchestration framework

Control: keep it a thin command runner. No JSON manifests, evidence schemas, run identifiers, archive aggregation, cross-run state, credentials, publication, or tag creation.

### Risk: protocol v2 fragments compatibility

Control: add explicit negotiation/fallback behavior and fixtures for v1 Linux/macOS responses and v2 Windows responses. Do not silently reinterpret v1 fields.

### Risk: Windows service work dominates the project

Control: separate foreground daemon support from SCM integration. Deliver the usable Windows client and foreground daemon before service packaging. Do not add installer ecosystems in this roadmap.

### Risk: unsupported Windows metrics are fabricated for UI symmetry

Control: make optionality part of the protocol and renderer acceptance tests.

## Program acceptance criteria

This roadmap is complete only when all of the following are true:

- [ ] Plans 037 through 044 meet their individual acceptance criteria.
- [ ] Active CI contains no publication, tagging, GitHub Release, release-finalization, artifact-selection, provenance, or evidence-retention logic.
- [ ] All release-only workflows and their validators/tests are removed from active source.
- [ ] Historical release plans are clearly archived or marked superseded and are no longer active dependencies.
- [ ] A single local validation entry point is documented and succeeds on a clean supported development host.
- [ ] Manual publication of all three crates is fully documented and has no executable automation in the repository.
- [ ] Manual annotated tagging and GitHub Release creation are documented separately from crates.io publication.
- [ ] `gregg` builds and behaves correctly on Windows.
- [ ] The protocol can truthfully represent Linux, macOS, and Windows metric capability differences.
- [ ] `greggd run` produces valid Windows snapshots on a native Windows host.
- [ ] Windows service lifecycle commands have tested, truthful semantics.
- [ ] The simplified CI workflow passes on Linux, macOS, and Windows.
- [ ] No acceptance criterion requires a retained CI evidence bundle, workflow artifact identity, exact-SHA qualification run, or cross-run manifest.

## Handoff rules for implementing agents

1. Do not preserve obsolete release complexity merely because tests currently cover it. Remove the tests with the retired behavior.
2. Do not replace one release framework with another helper framework.
3. Do not add publishing permissions or secrets to GitHub Actions.
4. Prefer deleting release-only code over deprecating it indefinitely.
5. Before deleting a script, identify whether it validates product behavior independently of release orchestration.
6. Keep each phase independently reviewable and green.
7. Use native Windows APIs behind narrow seams with deterministic test doubles.
8. Do not weaken telemetry semantics to avoid protocol work.
9. Update the plan index as each phase lands.
10. Stop and record a follow-up issue rather than expanding into package-manager distribution or unrelated monitoring features.