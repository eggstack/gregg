# Gregg plan index

This directory contains Gregg's implementation roadmaps and execution-ready plans.

## Current direction

Gregg remains a small local/LAN system monitor:

- native Linux, macOS, and Windows metric collection;
- a cached read-only JSON daemon API;
- a compact terminal client for a small fleet;
- optional bounded EggPool summary integration;
- local-first verification and manual releases;
- no generalized observability platform, public-internet service, release orchestration, or evidence system.

Local tests remain the primary development path. The one existing GitHub Actions workflow provides Linux checks and native macOS/Windows truth. CI does not publish crates, create releases, upload artifacts, or enforce binary-size gates.

## Roadmap status

Plans 066-074 are complete. The existing Windows CI job is the authoritative
Administrator environment for the SCM lifecycle smoke; run `31040689848` at
implementation SHA `e754f3f6b17c14bfc71234459a15237fe042736f` passed all jobs,
including workspace tests, the release build, and the lifecycle smoke.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) | Correct concrete cross-platform defects, retain only justified simplification, and close through bounded verification | complete through 074; CI-backed Windows SCM verification passed |
| [`067-truthful-drive-capacity-semantics.md`](067-truthful-drive-capacity-semantics.md) | Preserve truthful used, free, and caller-available drive capacity | complete |
| [`068-coherent-daemon-state-and-health.md`](068-coherent-daemon-state-and-health.md) | Publish one coherent daemon state and truthful health responses | complete |
| [`069-daemon-cli-runtime-and-test-correctness.md`](069-daemon-cli-runtime-and-test-correctness.md) | Correct config intent, runtime/error boundaries, exit codes, and omitted tests | complete |
| [`070-bounded-client-async-simplification.md`](070-bounded-client-async-simplification.md) | Retain scheduler or EggPool simplification only when smaller and behavior-preserving | complete; no change |
| [`071-measured-footprint-and-lightweight-closure.md`](071-measured-footprint-and-lightweight-closure.md) | Measure safe manifest/profile reductions and retain only verified improvements | complete |
| [`072-windows-service-runtime-and-record-correction.md`](072-windows-service-runtime-and-record-correction.md) | Correct Windows service runtime ownership and nonblocking shutdown | complete |
| [`073-native-windows-scm-entry-and-readiness-correction.md`](073-native-windows-scm-entry-and-readiness-correction.md) | Add native SCM dispatcher/`ServiceMain`, config handoff, and post-bind readiness | complete; operationally verified by 074 |
| [`074-ci-backed-windows-scm-closure.md`](074-ci-backed-windows-scm-closure.md) | Correct the Windows lifecycle smoke and run it in the existing Windows CI job | complete; run `31040689848` passed |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073 -> 074
```

Plan 074 is a concrete CI-backed compatibility implementation, not an evidence-only closure phase. Do not create Plan 075 merely to record its result.

## Execution guidance for GPT-5.6 Luna

The executor should:

1. Inspect current HEAD, `scripts/smoke-windows.ps1`, and `.github/workflows/ci.yml` before editing.
2. Execute only Plan 074; do not reopen product implementation from earlier phases.
3. Make bind failure deterministic by holding an ephemeral loopback port open.
4. Recreate required directories, reapply `LocalService`, and check every native command result.
5. Extend only the existing Windows job: workspace tests, release build, then SCM smoke.
6. Pin that existing job to `windows-2022`; do not add a workflow, matrix tier, or runner.
7. Keep the smoke bounded, local to the runner, and unconditionally cleaned up.
8. Require one ordinary green workflow run before marking Plans 066, 073, and 074 closed. Completed by run `31040689848`.
9. Correct existing records in place; do not add artifacts, evidence files, or Plan 075.

## Verification model

Routine development:

```bash
./scripts/check-local.sh
```

Manual release preflight remains:

```bash
./scripts/check-local.sh --release
```

For Plan 074, the existing Windows CI job is the authoritative Windows service environment. It must run:

```text
cargo test --workspace --all-targets --all-features
cargo build --release -p greggd
scripts/smoke-windows.ps1 -ExePath target/release/greggd.exe
```

The SCM smoke must prove dispatcher startup, generated `ServiceMain` execution, custom config-path handling, post-bind `RUNNING`, health/status, stop, restart, deterministic bind failure, recovery, reinstall, and cleanup.

One green ordinary workflow run at the final implementation SHA, or a source-equivalent documentation-only descendant, is sufficient. No repeated qualification run or separately maintained Windows host is required.

A plan does not require:

- a dedicated qualification workflow;
- a second Windows job or matrix;
- a self-hosted or privileged runner;
- uploaded artifacts, logs, screenshots, or evidence bundles;
- immutable candidate SHAs or repeated green runs;
- crates.io publication, tags, or GitHub Releases.

## Completion rule

A phase is complete only when its explicit acceptance criteria are implemented and demonstrated by the lightest appropriate mechanism:

- deterministic unit/integration tests;
- the default local check;
- the release preflight only for release-facing changes;
- the existing native platform CI jobs;
- the SCM lifecycle smoke inside the existing Windows job for operational Windows service behavior;
- direct documentation inspection for scope and behavior claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier commit that no longer matches HEAD.

## Scope control for Plans 066-074

Allowed for Plan 074:

- deterministic corrections to `scripts/smoke-windows.ps1`;
- an occupied ephemeral loopback port for bind-failure proof;
- immediate checking of `sc.exe` and Gregg command failures;
- recreation of required temporary directories and reapplication of `LocalService`;
- a release build and SCM smoke appended to the existing Windows CI job;
- pinning that existing job to `windows-2022`;
- direct correction of Plans 066, 073, 074, and this index after a green run.

Not allowed:

- product, protocol, collector, TUI, scheduler, or EggPool changes;
- raw Windows dispatcher FFI or a second service implementation;
- pause/continue support or broader SCM lifecycle redesign;
- MSI/WiX packaging or installer redesign;
- a new workflow, new job, expanded matrix, self-hosted runner, or privileged infrastructure;
- CI publishing, artifacts, evidence bundles, performance suites, or binary-size gates;
- a Plan 075 created only to mark Plan 074 complete.

## Completed roadmap groups

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and runtime cleanup | completed; CI run `30964819950` passed |

Plans 010-035 describing retired staged release/evidence work remain archived under `plans/archive/v1.0.1-release/` and are not current requirements.
