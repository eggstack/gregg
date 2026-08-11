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

Plans 066-075 are complete. Plan 075 corrected the trailing NUL in the native Windows hostname and restored `Config::name` wiring after the Plan 074 native Windows run exposed both defects. Its implementation commit `069bf37` passed ordinary CI in run `31189587467`.

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
| [`075-configured-name-and-windows-hostname-correction.md`](075-configured-name-and-windows-hostname-correction.md) | Remove the native Windows hostname NUL and honor configured daemon names in foreground and SCM modes | complete; CI run `31189587467` |
| [`076-native-runtime-croncheck-and-version-correction.md`](076-native-runtime-croncheck-and-version-correction.md) | Separate Unix foreground runtime, health probing, config mutation, and version commands | complete; local workspace and Ubuntu E2E checks passed |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073 -> 074 -> 075
```

Plan 075 is a concrete product-correctness correction discovered through the successful Plan 074 native smoke. It is not a closure-only evidence phase. Do not create Plan 076 merely to record its result.

## Execution record for Plan 075

Execution completed:

1. Inspected the existing Windows source, startup paths, foreground smoke, SCM smoke, and relevant documentation.
2. Kept the completed SCM dispatcher, runtime ownership, readiness, and CI architecture unchanged.
3. Truncated `GetComputerNameExW` output using the successful call's returned UTF-16 length.
4. Passed `Some(config.name.as_str())` into native collector construction in foreground and SCM modes.
5. Strengthened the existing Windows foreground and SCM smoke assertions without adding a test harness.
6. Ran focused tests, the default and release local checks, exact Linux CI gates, Rust 1.75 compilation, and one ordinary existing CI run.
7. Recorded the green implementation SHA and workflow run in Plan 075; no corrective phase remains active.

## Verification model

Routine development:

```bash
./scripts/check-local.sh
```

Manual release preflight remains:

```bash
./scripts/check-local.sh --release
```

For Plan 075, focused verification is sufficient before ordinary CI:

```text
cargo fmt --all -- --check
cargo test -p greggd
./scripts/check-local.sh
```

The existing Windows CI job remains the authoritative native Windows environment and continues to run:

```text
cargo test --workspace --all-targets --all-features
cargo build --release -p greggd
scripts/smoke-windows.ps1 -ExePath target/release/greggd.exe
```

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

## Scope control for Plan 075

Allowed:

- local Windows `GetComputerNameExW` buffer-length correction;
- direct propagation of validated `Config::name` into already-existing collector display-name parameters;
- focused regression assertions in the existing Windows foreground and SCM smokes;
- direct closure updates to Plan 075 and this index after one green CI run.

Not allowed:

- protocol or config-schema changes;
- FQDN discovery, hostname rewriting, DNS/NetBIOS expansion, or new identity sources;
- collector factories, generic identity frameworks, or service abstractions solely for this correction;
- changes to SCM dispatcher, runtime ownership, post-bind readiness, installer architecture, or service commands;
- product changes to the TUI, scheduler, drives, EggPool, or release model;
- new dependencies, workflows, jobs, matrices, self-hosted runners, artifacts, evidence bundles, performance suites, or binary-size gates;
- a Plan 076 created only to mark Plan 075 complete.

## Completed roadmap groups

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and runtime cleanup | completed; CI run `30964819950` passed |

Plans 010-035 describing retired staged release/evidence work remain archived under `plans/archive/v1.0.1-release/` and are not current requirements.
