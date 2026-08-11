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

Plans 066-077 are complete. Plan 076 implemented the Unix runtime/service-manager separation, HTTP `croncheck`, config-only Unix mutation, and explicit version commands. Plan 077 completed the strict bounded status-line correction, negative-path coverage, stale test cleanup, and planning reconciliation.

Plan 078 implements the stale client endpoint correction at the existing `Ctrl-R` boundary, adds HTTP URL input convenience to `gregg add`, and adds read-only `greggd configprint`. The repository implementation, local CI checks, and required real-host smoke are complete against `192.168.182.143:11310`.

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
| [`076-native-runtime-croncheck-and-version-correction.md`](076-native-runtime-croncheck-and-version-correction.md) | Separate Unix foreground runtime, health probing, config mutation, and version commands | complete; implementation and corrected strict-parser verification recorded |
| [`077-croncheck-strictness-test-cleanup-and-plan076-closure.md`](077-croncheck-strictness-test-cleanup-and-plan076-closure.md) | Bound and tighten `croncheck` status parsing, remove stale disabled tests, and close Plan 076 truthfully | complete |
| [`078-client-endpoint-url-config-reload-and-daemon-configprint.md`](078-client-endpoint-url-config-reload-and-daemon-configprint.md) | Reload stale client endpoints on `Ctrl-R`, accept HTTP URL input for `gregg add`, and add read-only daemon bind-address printing | complete |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073 -> 074 -> 075 -> 076 -> 077 -> 078
```

Plan 076 is concrete product-correctness work, not a closure-only record. Plan 077 is the single narrow corrective pass discovered during review of that implementation. Plan 078 is a separate live-testing correction and CLI ergonomics phase; do not create another phase solely to mark 078 complete if its acceptance criteria pass.

## Execution record for Plan 075

Execution completed:

1. Inspected the existing Windows source, startup paths, foreground smoke, SCM smoke, and relevant documentation.
2. Kept the completed SCM dispatcher, runtime ownership, readiness, and CI architecture unchanged.
3. Truncated `GetComputerNameExW` output using the successful call's returned UTF-16 length.
4. Passed `Some(config.name.as_str())` into native collector construction in foreground and SCM modes.
5. Strengthened the existing Windows foreground and SCM smoke assertions without adding a test harness.
6. Ran focused tests, the default and release local checks, exact Linux CI gates, Rust 1.75 compilation, and one ordinary existing CI run.
7. Recorded the green implementation SHA and workflow run in Plan 075; no corrective phase remained for the Plan 075 scope.

## Verification model

Routine development:

```bash
./scripts/check-local.sh
```

Manual release preflight remains:

```bash
./scripts/check-local.sh --release
```

For Plan 078, focused local verification is sufficient in addition to the default local check:

```text
cargo fmt --all -- --check
cargo test -p gregg endpoint
cargo test -p gregg state
cargo test -p gregg scheduler
cargo test -p gregg cli
cargo test -p gregg --bin gregg
cargo test -p greggd cli
cargo test -p greggd --bin greggd
./scripts/check-local.sh
```

Plan 078 additionally requires one narrow local Ubuntu operational smoke against the verified live `greggd` endpoint at `192.168.182.143:11310`: URL-form `gregg add`, running-TUI `.183` -> `.182` `Ctrl-R` reconciliation, and exact `greggd configprint` output. This remains local-only and is not moved into CI.

The existing Windows CI job remains the authoritative native Windows environment for Windows-specific SCM behavior. Plan 078 does not change SCM behavior or add CI coverage; common CLI compilation/tests are sufficient for the new `configprint` command, with existing native CI continuing to guard platform compilation.

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
- native platform CI only where native-platform truth is actually required;
- direct local operational smoke where explicitly required;
- direct documentation inspection for scope and behavior claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier commit that no longer matches HEAD.

## Active scope control for Plan 078

Allowed:

- reload the current Gregg system config at the existing `Ctrl-R` boundary;
- reconcile added/removed/changed system endpoints by stable ID;
- replace or narrowly restart only the system polling endpoint set;
- reset metrics when a stable ID changes host/port so old-machine data is not shown under a new endpoint;
- accept syntactically valid plain-HTTP URL input for `gregg add` and persist only host/port;
- preserve explicit URL ports and Gregg's configured default-port behavior when a URL omits a port;
- add one read-only `greggd configprint` command with exact canonical socket-address output;
- focused tests and one real Ubuntu smoke using `192.168.182.143:11310`;
- direct documentation and plan-index updates.

Not allowed:

- filesystem watcher libraries or continuous hot reload;
- a new background config-monitor subsystem;
- TLS/HTTPS polling or silent HTTPS-to-HTTP downgrade;
- persisting schemes, paths, or URL credentials in client config;
- endpoint discovery or network probing during `gregg add`;
- dynamic daemon reload, PID/process discovery, or service-manager changes;
- a full config dump under `configprint`;
- changes to `croncheck` semantics;
- broad TUI redesign, test-suite restructuring, or unrelated cleanup;
- new workflows, jobs, matrices, artifacts, evidence bundles, or CI gates;
- release automation or publication work;
- a Plan 079 created only to record Plan 078 closure.

## Completed roadmap groups

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and runtime cleanup | completed; CI run `30964819950` passed |

Plans 010-035 describing retired staged release/evidence work remain archived under `plans/archive/v1.0.1-release/` and are not current requirements.
