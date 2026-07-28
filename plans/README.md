# Gregg plan index

This directory contains Gregg's implementation roadmaps and execution-ready plans.

## Current direction

Gregg now follows a deliberately small release and validation model:

- local tests are the primary comprehensive validation path;
- GitHub Actions performs source/product checks only;
- CI does not publish crates, push tags, create GitHub Releases, finalize candidates, or retain release-evidence bundles;
- crates.io publication is manual;
- annotated Git tags and GitHub Releases are manual;
- Windows support is ordinary product work and must not recreate the retired release-orchestration system.

[`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) is the active umbrella roadmap for this line of work. Plans 037 through 044 are the authoritative execution phases.

Plans 010 through 022 and 030 through 035 document a retired staged release/evidence model. They remain historical context until Phase 37 archives or marks them in place, but they are not active acceptance gates and must not block future manual releases.

## Active roadmap and execution phases

| Plan | Purpose | Primary output | Status |
| --- | --- | --- | --- |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) | Replace release orchestration with local-first checks/manual publication, then add truthful Windows support | Current program map and governing constraints | active umbrella roadmap |
| [`037-remove-release-orchestration-and-archive-history.md`](037-remove-release-orchestration-and-archive-history.md) | Delete release workflows/evidence machinery, retain product tests, and separate historical plans | Product-focused repository without automated release control | planned; first phase |
| [`038-local-first-validation-and-minimal-ci.md`](038-local-first-validation-and-minimal-ci.md) | Add one fast local check, an optional full tier, and a small source-only CI workflow | Quickly iterable validation model | planned; depends on 037 |
| [`039-manual-cratesio-and-github-release.md`](039-manual-cratesio-and-github-release.md) | Document manual crates.io publication, annotated tagging, and GitHub Release creation | Version-neutral `RELEASING.md` operator runbook | planned; depends on 037 and 038 |
| [`040-windows-client-portability.md`](040-windows-client-portability.md) | Make the client native and correct on Windows, including paths, locking, editing, polling, and TUI lifecycle | Supported Windows x86-64 client | planned; depends on 037, uses 038 conventions |
| [`041-capability-aware-protocol-v2.md`](041-capability-aware-protocol-v2.md) | Preserve v1 while adding optional load/swap/I/O-wait and distinct Windows commit semantics | Version-2 heterogeneous-platform protocol | planned; depends on 040 |
| [`042-windows-native-metrics-collector.md`](042-windows-native-metrics-collector.md) | Implement Windows identity, CPU delta, physical memory, and commit collection | Native Windows foreground daemon with valid v2 snapshots | planned; depends on 041 |
| [`043-windows-service-lifecycle-and-packaging.md`](043-windows-service-lifecycle-and-packaging.md) | Integrate SCM lifecycle, ProgramData config, least-privilege service account, and local PowerShell packaging | Installable native Windows service | planned; depends on 041 and 042 |
| [`044-windows-ci-integration-and-release-readiness.md`](044-windows-ci-integration-and-release-readiness.md) | Add representative Windows source CI, mixed-fleet tests, packaging/docs reconciliation, and final closure | Release-ready Linux/macOS/Windows product under the slim process | planned; depends on 038 and 040 through 043 |

## Existing product plans retained for reassessment

These plans contain product/platform work rather than the retired release-evidence system. Phase 37 must classify their current implementation status and either retain, supersede, or re-scope them. They are not automatically manual-release blockers.

| Plan | Purpose | Current registry status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Original version-1 architecture and execution roadmap | historical initial umbrella; superseded for current direction by 036 |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | implemented baseline |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | implementation landed; retain as architecture history/product reference |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | implementation landed; retain as architecture history/product reference |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | implementation landed; remaining correctness tracked by product plans |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | implementation landed; Windows extension is Phase 43 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and configuration commands | implementation landed; Windows extension is Phase 40 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | implementation landed; preserve product semantics through Windows work |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, navigation, scrolling | implementation landed; capability rendering extends in Phase 41 |
| [`009-testing-hardening-performance.md`](009-testing-hardening-performance.md) | Product tests, resource bounds, package validation | partially implemented; useful product checks retained/simplified by 037-038 |
| [`023-v1.0.1-macos-mach-counter-correctness.md`](023-v1.0.1-macos-mach-counter-correctness.md) | Preserve unsigned Mach counter bit patterns and reset/recovery behavior | planned in prior registry; reassess as product correctness, not release evidence |
| [`024-v1.0.1-sampler-readiness-and-freshness-correction.md`](024-v1.0.1-sampler-readiness-and-freshness-correction.md) | Separate collector state, retained snapshot, freshness, and failure count | planned in prior registry; reassess as product correctness |
| [`025-v1.0.1-endpoint-and-persisted-config-correctness.md`](025-v1.0.1-endpoint-and-persisted-config-correctness.md) | Canonical host/UUID/config validation | planned in prior registry; reassess before Windows config work |
| [`026-v1.0.1-macos-service-least-privilege.md`](026-v1.0.1-macos-service-least-privilege.md) | Harden macOS service account and LAN exposure | planned in prior registry; reassess as platform hardening |
| [`027-v1.0.1-four-architecture-ci-and-msrv-closure.md`](027-v1.0.1-four-architecture-ci-and-msrv-closure.md) | Original exhaustive architecture CI proposal | superseded in approach by representative minimal CI in 038/044; retain useful target facts only |
| [`028-v1.0.1-bounded-poll-scheduler-optimization.md`](028-v1.0.1-bounded-poll-scheduler-optimization.md) | Bounded ordered poll window | planned in prior registry; reassess as product optimization |
| [`029-v1.0.1-daemon-hotpath-and-runtime-isolation.md`](029-v1.0.1-daemon-hotpath-and-runtime-isolation.md) | Cached serialization and collection isolation | planned in prior registry; reassess as product optimization |

## Historical retired release-orchestration plans

The following plans are not active requirements. They describe the former automated/staged crates.io release, GitHub Actions evidence, provenance, candidate-finalization, and qualification design. Phase 37 owns their archival or explicit in-place historical marking.

| Plans | Historical subject | Status |
| --- | --- | --- |
| 010-015 | Initial crates.io release and repeated release-gate correction | historical; superseded by manual release Phase 39 |
| 016-019 | Candidate identity, provenance, artifact retrieval, aggregation, and orchestration | historical; superseded by deletion Phase 37 |
| 020-022 | Post-audit release roadmap, workflow DAG, sustained release evidence | historical; product smokes may be retained by 037-038 |
| 030-035 | Candidate freeze, finalizer correction, qualification contracts, evidence lineage | historical; explicitly retired and not a release gate |
| `v1.0.1-final-evidence.md` | Immutable release-evidence ledger | historical; archive/remove from active navigation in Phase 37 |

## Dependency summary

```text
37 -> 38 -> 39
37 -> 40 -> 41 -> 42 -> 43
38 + 40 + 41 + 42 + 43 -> 44
```

Phase 39 may finish and be used for manual releases before Windows work completes. Publishing is an operator action outside CI and outside implementation-plan completion.

## Completion rule

A plan is complete when its explicit product and process acceptance criteria are demonstrated by the lightest appropriate mechanism:

- deterministic local unit/integration tests;
- short native product smokes;
- the ordinary read-only CI workflow;
- concise manual platform rehearsal where elevation or a real service manager is required;
- documentation and repository search for deletion/policy criteria.

A plan does **not** require:

- an immutable candidate SHA;
- a dedicated qualification workflow;
- uploaded evidence artifacts;
- artifact IDs, ZIP digests, or cross-run selection documents;
- provenance/finalizer manifests;
- crates.io publication;
- a Git tag or GitHub Release;
- exhaustive hosted architecture evidence.

Implementation handoffs should record the commands run and their results concisely. Do not create evidence bundles merely to mark a plan complete.

## Scope control

Any discovered expansion should be recorded separately unless required for:

- correctness of the current Linux/macOS product;
- the explicit Windows client/daemon/service scope in Plans 040-044;
- security of the private-network operating model;
- publishability through the manual Phase 39 procedure;
- maintaining the compact monitoring product contract.

Package-manager distribution, automatic updates, public-internet hardening, dashboards, historical telemetry, alerting, per-process monitoring, and generalized release infrastructure remain out of scope.