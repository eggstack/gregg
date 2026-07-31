# Gregg plan index

This directory contains Gregg's implementation roadmaps and execution-ready plans.

## Current direction

Gregg follows a small, manual release model:

- Local tests are the primary comprehensive validation path.
- GitHub Actions performs source/product checks only.
- Linux owns generic source checks; hosted macOS and Windows jobs provide native-platform verification.
- CI does not publish crates, push tags, create GitHub Releases, or retain release-evidence bundles.
- crates.io publication is manual.
- Annotated Git tags and GitHub Releases are manual.

[`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) is the completed umbrella roadmap. Plans 037 through 045 contain the implemented release-simplification and Windows work. [`046-minimal-cross-platform-verification-closure.md`](046-minimal-cross-platform-verification-closure.md) is the only remaining corrective phase and supersedes the disproportionate verification/closure requirements left by Plans 044 and 045.

## Active roadmap and execution phases

| Plan | Purpose | Status |
| --- | --- | --- |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) | Replace release orchestration with local-first checks/manual publication, then add truthful Windows support | completed; verification model refined by Phase 46 |
| [`037-remove-release-orchestration-and-archive-history.md`](037-remove-release-orchestration-and-archive-history.md) | Delete release workflows/evidence machinery, retain product tests, and separate historical plans | completed |
| [`038-local-first-validation-and-minimal-ci.md`](038-local-first-validation-and-minimal-ci.md) | Add one fast local check, an optional full tier, and a small source-only CI workflow | implementation completed; tier complexity reduced by Phase 46 |
| [`039-manual-cratesio-and-github-release.md`](039-manual-cratesio-and-github-release.md) | Document manual crates.io publication, annotated tagging, and GitHub Release creation | completed |
| [`040-windows-client-portability.md`](040-windows-client-portability.md) | Make the client native and correct on Windows | completed |
| [`041-capability-aware-protocol-v2.md`](041-capability-aware-protocol-v2.md) | Preserve v1 while adding optional load/swap/I/O-wait and distinct Windows commit semantics | completed |
| [`042-windows-native-metrics-collector.md`](042-windows-native-metrics-collector.md) | Implement Windows identity, CPU delta, physical memory, and commit collection | completed |
| [`043-windows-service-lifecycle-and-packaging.md`](043-windows-service-lifecycle-and-packaging.md) | Integrate SCM lifecycle, ProgramData config, least-privilege service account, and local PowerShell packaging | implementation completed; hosted verification boundary finalized by Phase 46 |
| [`044-windows-ci-integration-and-release-readiness.md`](044-windows-ci-integration-and-release-readiness.md) | Add representative Windows source CI, mixed-fleet tests, packaging/docs reconciliation, and final closure | implementation completed; excessive closure requirements superseded by Phase 46 |
| [`045-release-script-and-windows-closure.md`](045-release-script-and-windows-closure.md) | Correct local release scripts/runbook and complete native Windows, package, CI, and registry closure | corrective implementation completed; excessive evidence/manual-rehearsal requirements superseded by Phase 46 |
| [`046-minimal-cross-platform-verification-closure.md`](046-minimal-cross-platform-verification-closure.md) | Reduce local/CI verification to a proportionate two-tier, one-workflow contract with hosted native Windows/macOS truth | implementation complete; hosted CI confirmation pending |

## Existing product plans retained for reassessment

These plans contain product/platform work. They are not release-gate blockers.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Original version-1 architecture and execution roadmap | historical initial umbrella; superseded for current direction by 036 |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | implemented baseline |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | implemented; retain as architecture history |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | implemented; retain as architecture history |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | implemented; correctness tracked by product plans |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | implemented; Windows extension is Phase 43 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and configuration commands | implemented; Windows extension is Phase 40 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | implemented; preserve product semantics through Windows work |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, navigation, scrolling | implemented; capability rendering extends in Phase 41 |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Product tests, resource bounds, package validation | partially implemented; useful checks retained/simplified by 037-038 and 046 |

## Historical retired release-orchestration plans

Plans 010 through 022 and 030 through 035 describe a retired automated staged release/evidence model. They have been physically archived and are not active acceptance gates.

```text
plans/archive/v1.0.1-release/
```

Plans 023 through 029 contained product/platform corrections originally framed as release gates. They are archived with the following classifications:

| Plan | Subject | Classification |
| --- | --- | --- |
| 023 | macOS Mach counter correctness | superseded; FFI rewrite landed |
| 024 | Sampler readiness and freshness | superseded; correction landed |
| 025 | Endpoint and config correctness | re-scope; IPv6 zone-ID validation deferred to Windows config work |
| 026 | macOS service least privilege | superseded; launchd rewrite landed |
| 027 | Four-architecture CI and MSRV | superseded; representative CI in 038/044/046 |
| 028 | Bounded poll-scheduler | superseded; scheduler rewrite landed |
| 029 | Daemon hotpath and runtime isolation | re-scope; cached serialization deferred as product optimization |

## Dependency summary

```text
37 -> 38 -> 39
37 -> 40 -> 41 -> 42 -> 43
38 + 40 + 41 + 42 + 43 -> 44
39 + 40 + 41 + 42 + 43 + 44 -> 45
44 + 45 -> 46
```

Phase 39 remains the manual release procedure. Phase 46 only simplifies verification and closure; it does not automate publishing.

## Completion rule

A plan is complete when its explicit product and process acceptance criteria are demonstrated by the lightest appropriate mechanism:

- deterministic local unit/integration tests;
- short bounded product smokes;
- the ordinary read-only CI workflow;
- hosted native macOS/Windows jobs for target-specific behavior;
- documentation and repository search for deletion/policy criteria.

A green ordinary CI run at the final implementation SHA is sufficient hosted cross-platform proof. A separately maintained Windows host, elevated manual rehearsal record, or evidence bundle is not required.

A plan does **not** require:

- an immutable candidate SHA;
- a dedicated qualification workflow;
- repeated green runs;
- uploaded evidence artifacts;
- artifact IDs, ZIP digests, or cross-run selection documents;
- provenance/finalizer manifests;
- a manual platform evidence record;
- crates.io publication;
- a Git tag or GitHub Release.

Implementation handoffs should state results concisely. Do not create evidence files merely to mark a plan complete.

## Scope control

Any discovered expansion should be recorded separately unless required for:

- correctness of the current Linux/macOS product;
- the explicit Windows client/daemon/service scope in Plans 040-046;
- security of the private-network operating model;
- publishability through the manual Phase 39 procedure;
- maintaining the compact monitoring product contract.

Package-manager distribution, automatic updates, public-internet hardening, dashboards, historical telemetry, alerting, per-process monitoring, generalized release infrastructure, and dedicated verification/evidence systems remain out of scope.
