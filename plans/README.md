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

[`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) is reopened for one narrow verification-polish phase. Plans 057 through 060 added one optional EggPool summary endpoint to the `gregg` client configuration, a nested add/list/remove CLI, an authenticated bounded summary client, and one compact top-level EggPool pane. Original implementation commit `1406c2b` passed local checks and ordinary CI run `30660744394`. Phase 61 implementation commit `1b77da1` corrected timer-driven generation ownership, request-relative cadence, reliable command delivery, worker-channel closure handling, and stale client metadata; ordinary CI run `30681153449` passed all jobs. Later review found that Phase 61's required deterministic worker cadence, generation, command-pressure, deactivation, and cancellation tests were not added even though its closure criteria were checked. [`062-eggpool-worker-regression-coverage-and-closure-polish.md`](062-eggpool-worker-regression-coverage-and-closure-polish.md) is the sole active correction. This work remains client-only and must not modify EggPool, `greggd`, `gregg-protocol`, CI, or release infrastructure.

[`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) is the completed drive-metrics and multi-view product roadmap. Plans 049 through 053 implemented and closed that work through the existing lightweight verification model. [`054-drive-multiview-corrective-polish.md`](054-drive-multiview-corrective-polish.md) completed at implementation `561e398e` with ordinary CI run `30635971005`. [`055-phase-54-closure-record-correction.md`](055-phase-54-closure-record-correction.md) completed the closure-record correction. No corrective phase remains open for this line, and no product implementation, release-design, or CI-design work remains open.

[`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) is the completed release/platform umbrella roadmap. Plans 037 through 047 contain the implemented release-simplification, Windows, minimal-verification, and documentation-polish work. No verification or release corrective phase remains open.

## Current roadmap and execution phases

| Plan | Purpose | Status |
| --- | --- | --- |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) | Add one optional EggPool summary source and compact second top-level TUI pane without changing EggPool, greggd, protocol, CI, or release scope | verification Phase 62 open; runtime correction `1b77da1`, ordinary CI `30681153449`; original implementation `1406c2b`, CI `30660744394` |
| [`057-eggpool-config-and-cli.md`](057-eggpool-config-and-cli.md) | Add one optional validated EggPool entry plus nested add/list/remove commands through the existing atomic config store | completed |
| [`058-eggpool-summary-client-and-refresh.md`](058-eggpool-summary-client-and-refresh.md) | Add the typed summary client, environment-referenced Bearer auth, fixed periods, bounded failures, and active-pane refresh worker | implemented; refresh correctness corrected by 061; worker regression coverage owned by 062 |
| [`059-eggpool-pane-state-controls-and-rendering.md`](059-eggpool-pane-state-controls-and-rendering.md) | Separate top-level pane state from Normal/Condensed system layout, add context-sensitive controls, and render the four-value EggPool pane | completed; depends on 057 and 058 |
| [`060-eggpool-pane-integration-and-lightweight-closure.md`](060-eggpool-pane-integration-and-lightweight-closure.md) | Wire optional runtime lifecycle, active-pane refresh, docs, synthetic integration tests, and ordinary local/CI closure | original implementation completed at `1406c2b`; final closure superseded by 061 and 062 |
| [`061-eggpool-refresh-correctness-and-closure.md`](061-eggpool-refresh-correctness-and-closure.md) | Correct periodic generation ownership, request-relative cadence, reliable command delivery, deterministic timing tests, stale metadata, and Roadmap 56 closure truth | runtime correction implemented at `1b77da1`; original closure superseded because required worker regression tests were absent; verification owned by 062 |
| [`062-eggpool-worker-regression-coverage-and-closure-polish.md`](062-eggpool-worker-regression-coverage-and-closure-polish.md) | Add the missing deterministic worker generation, cadence, pressure, deactivation, cancellation, and no-config coverage, then close Roadmap 56 truthfully | planned; depends on 061 |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) | Add bounded per-drive v2 metrics, aggregate normal-view disk usage, reliable fleet scrolling, a condensed view, and selected-system drive expansion | completed |
| [`049-additive-v2-drive-protocol-and-normalization.md`](049-additive-v2-drive-protocol-and-normalization.md) | Add a bounded optional v2 drive representation, preserve v1/old-v2 compatibility, and centralize client aggregation | completed |
| [`050-native-cross-platform-drive-collection.md`](050-native-cross-platform-drive-collection.md) | Enumerate eligible mounted local filesystems natively on Linux, macOS, and Windows with best-effort failure semantics | completed |
| [`051-dynamic-viewport-and-normal-drive-rendering.md`](051-dynamic-viewport-and-normal-drive-rendering.md) | Correct logical-system viewport following, add dynamic row accounting, and render aggregate/selected drive details in normal view | completed; depends on 049 |
| [`052-condensed-view-and-view-controls.md`](052-condensed-view-and-view-controls.md) | Add the `condensed.txt`-style fleet view plus `h`/`l`/arrows and `e` controls | completed |
| [`053-drive-multiview-integration-and-lightweight-closure.md`](053-drive-multiview-integration-and-lightweight-closure.md) | Reconcile compatibility, mixed-fleet behavior, docs, response bounds, and ordinary local/CI closure | completed; CI run 30632762621 passed |
| [`054-drive-multiview-corrective-polish.md`](054-drive-multiview-corrective-polish.md) | Correct the normal-view four-row boundary, preserve native enumeration failure semantics, and reconcile completed-roadmap wording | completed; implementation `561e398e`; CI run `30635971005` passed |
| [`055-phase-54-closure-record-correction.md`](055-phase-54-closure-record-correction.md) | Record one ordinary cross-platform CI result, reconcile Phase 54 metadata, and close the plan index truthfully | completed; closure-record correction |
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
| [`046-minimal-cross-platform-verification-closure.md`](046-minimal-cross-platform-verification-closure.md) | Reduce local/CI verification to a proportionate two-tier, one-workflow contract with hosted native Windows/macOS truth | completed; ordinary CI and local release preflight passed |
| [`047-documentation-and-ci-polish-closure.md`](047-documentation-and-ci-polish-closure.md) | Remove stale full-tier contributor guidance and trivial CI boilerplate without changing verification coverage | completed; implementation `452f998`, ordinary CI run `30599181232` passed |

## Existing product plans retained for reassessment

These plans contain product/platform work. They are not release-gate blockers.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) | Original version-1 architecture and execution roadmap | historical initial umbrella; superseded for current direction by completed roadmaps 036 and 048 plus bounded closure correction 055; Roadmap 056 is a later optional client-only extension with active verification Phase 62 |
| [`001-foundation-workspace-protocol.md`](001-foundation-workspace-protocol.md) | Workspace, package metadata, protocol schema, fixtures, CI foundation | implemented baseline |
| [`002-linux-metrics-collector.md`](002-linux-metrics-collector.md) | Native Linux identity and metric sampling | implemented; retain as architecture history |
| [`003-macos-metrics-collector.md`](003-macos-metrics-collector.md) | Native Darwin/Mach/sysctl metric sampling | implemented; retain as architecture history |
| [`004-daemon-sampler-http-api.md`](004-daemon-sampler-http-api.md) | Cached sampler, readiness, HTTP API, shutdown | implemented; correctness tracked by product plans |
| [`005-daemon-config-service-packaging.md`](005-daemon-config-service-packaging.md) | Atomic config, lifecycle CLI, systemd, launchd, installation | implemented; Windows extension is Phase 43 |
| [`006-client-config-cli.md`](006-client-config-cli.md) | Endpoint model and configuration commands | implemented; Windows extension is Phase 40; optional EggPool extension is Phase 57 |
| [`007-polling-state-engine.md`](007-polling-state-engine.md) | Bounded polling, batch generations, state reduction, ordering | implemented; viewport behavior extended by Phase 51 and corrected by Phase 54; optional EggPool worker remains separate under Phases 58-62 |
| [`008-compact-ratatui-tui.md`](008-compact-ratatui-tui.md) | Four-line rendering, navigation, scrolling | implemented historical baseline; current system TUI expansion is Plans 051-052 with bounded correction in 054; optional top-level EggPool pane is Plans 059-062 |
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

Active optional EggPool summary-pane roadmap:

```text
56 -> 57
57 -> 58
57 -> 59
58 + 59 -> 60
60 -> 61
61 -> 62
```

Completed product roadmap with bounded closure correction:

```text
49 -> 50
49 -> 51
50 + 51 -> 52
49 + 50 + 51 + 52 -> 53
53 -> 54
54 -> 55 (both completed)
```

Completed release/platform roadmap:

```text
37 -> 38 -> 39
37 -> 40 -> 41 -> 42 -> 43
38 + 40 + 41 + 42 + 43 -> 44
39 + 40 + 41 + 42 + 43 + 44 -> 45
44 + 45 -> 46
46 -> 47
```

Phase 39 remains the manual release procedure. Phase 46 defines the completed minimal verification model. Phase 47 only corrected active documentation and trivial workflow indirection. Plans 48-62 must use that existing model and must not add verification or publication machinery.

## Completion rule

A plan is complete when its explicit product and process acceptance criteria are demonstrated by the lightest appropriate mechanism:

- deterministic local unit/integration tests;
- short bounded product smokes;
- the ordinary read-only CI workflow;
- hosted native macOS/Windows jobs for target-specific behavior;
- documentation and repository search for deletion/policy criteria.

A green ordinary CI run at the final implementation SHA or a documented source-equivalent plan-only descendant is sufficient hosted cross-platform proof. A separately maintained Windows host, elevated manual rehearsal record, or evidence bundle is not required.

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

- correctness of the current Linux/macOS/Windows product;
- the explicit mounted-local-filesystem and normal/condensed TUI scope in Plans 048-055;
- the explicit one-endpoint, four-metric optional EggPool summary-pane scope in Plans 056-062;
- security of the private-network operating model;
- publishability through the manual Phase 39 procedure;
- maintaining the compact monitoring product contract.

For Plans 056-062, the allowed EggPool boundary is one optional endpoint, one `/api/stats/summary` request, four fixed periods, four accurately labeled values, environment-referenced Bearer authentication, one compact pane, one active-only passive deadline, and one bounded worker channel. Multiple EggPool instances, aggregation, additional EggPool endpoints, direct database access, dashboard-disabled route changes, provider/model/account drill-down, costs, request logs, runtime diagnostics, charts, history, alerts, exports, retries/backoff, configurable cadence, and generalized dashboard/plugin/datasource/scheduler systems remain out of scope.

Package-manager distribution, automatic updates, public-internet hardening, general dashboards, historical telemetry, alerting, per-process monitoring, physical-disk/SMART/storage-topology inventory, configurable table layouts, generalized release infrastructure, and dedicated verification/evidence systems remain out of scope.
