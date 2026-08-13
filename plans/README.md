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

Plans 066-079 are complete. Plan 076 implemented the Unix runtime/service-manager separation, HTTP `croncheck`, config-only Unix mutation, and explicit version commands. Plan 077 completed the strict bounded status-line correction, negative-path coverage, stale test cleanup, and planning reconciliation.

Plan 078 implemented the stale client endpoint correction at the existing `Ctrl-R` boundary, HTTP URL input convenience for `gregg add`, and read-only `greggd configprint`. Plan 079 then made replacement delivery reliable under bounded command pressure and corrected Plan 078's live-host record without rewriting the original `.183`/`.182` observation.

Plan 080 is active product-correctness work. It must reproduce/classify the current Ubuntu `croncheck` refusal, preserve passive health-probe semantics, add a direct Linux/macOS `greggd stop` command without restoring service-manager coupling, preserve Windows SCM stop behavior, and close only after a real release-binary Ubuntu lifecycle smoke.

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
| [`078-client-endpoint-url-config-reload-and-daemon-configprint.md`](078-client-endpoint-url-config-reload-and-daemon-configprint.md) | Reload stale client endpoints on `Ctrl-R`, accept HTTP URL input for `gregg add`, and add read-only daemon bind-address printing | complete; historical wording corrected by 079 |
| [`079-scheduler-replacement-delivery-and-plan078-record-correction.md`](079-scheduler-replacement-delivery-and-plan078-record-correction.md) | Guarantee scheduler endpoint replacement under bounded command pressure and correct Plan 078's environment record | complete; implementation `49c4c7d` |
| [`080-greggd-runtime-croncheck-and-direct-stop-correction.md`](080-greggd-runtime-croncheck-and-direct-stop-correction.md) | Diagnose/correct the daemon refusal and add direct local Unix `greggd stop` without restoring service-manager coupling | ready for implementation |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073 -> 074 -> 075 -> 076 -> 077 -> 078 -> 079 -> 080
```

Plan 076 is concrete product-correctness work, not a closure-only record. Plan 077 corrected the remaining bounded `croncheck` issues. Plan 078 added separate live-tested product functionality. Plan 079 is justified by a concrete runtime divergence edge found in source review. Plan 080 is separately justified by the observed daemon refusal and new direct-stop product requirement; it is not a closure-only continuation of Plan 079.

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

For Plan 079, focused deterministic local verification was run in addition to the default local check:

```text
cargo fmt --all -- --check
cargo test -p gregg main
cargo test -p gregg scheduler
cargo test -p gregg state
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

The key Plan 079 proof is a bounded scheduler-command channel test that fills capacity, performs a valid endpoint reload, and demonstrates that `ReplaceEndpoints` is delivered rather than silently dropped. A second A -> B -> C test proves convergence to the latest accepted replacement under bounded capacity.

A second external private-LAN smoke was optional for Plan 079 and did not determine completion. Plan 078 already demonstrated the address-replacement path against a live daemon in the environment available at that time. Plan 079 corrected command-delivery semantics deterministically and corrected the record to preserve both the originating `.183`-working/`.182`-stale report and the later `.182`-reachable smoke environment.

Plan 080 requires a direct lifecycle proof in addition to focused tests and the default local check. The real release binary on the current Ubuntu host must demonstrate:

```text
greggd run -> croncheck succeeds -> greggd stop -> daemon exits -> croncheck fails
```

The smoke must verify the expected TCP listener and local Unix control socket exist while running and disappear after stop. It must also record the actual cause of the original connection refusal. No new CI job is required.

The existing Windows CI job remains the authoritative native Windows environment for Windows-specific SCM behavior. Plan 080 preserves the current Windows SCM stop path and does not add Windows control-socket work.

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

Plan 080 cannot be closed from compilation/unit tests alone; its mandatory Ubuntu release-binary lifecycle smoke and refusal root-cause record must be present before marking it complete.

## Closed scope record for Plan 079

Completed:

- make successful Systems endpoint replacement delivery reliable through the existing bounded scheduler command channel;
- use bounded async backpressure or an equivalently small latest-replacement mechanism;
- explicitly handle a closed scheduler command receiver;
- retain the existing state reconciliation, endpoint host/port stale-result guard, scheduler generation model, and immediate replacement poll;
- add deterministic capacity-pressure tests and an A -> B -> C convergence test;
- correct Plan 078 so it separately records the originating `.183`-working/`.182`-stale report and the later `.182`-reachable closure environment;
- focused local checks and direct planning-record updates.

Preserved exclusions:

- an unbounded scheduler command channel;
- filesystem watcher libraries or continuous hot reload;
- a new background config-monitor subsystem;
- scheduler/actor rewrite, generic priority queue, or watch-channel architecture unless a tiny equivalent is strictly smaller than awaited delivery;
- changes to poll concurrency or endpoint schemas;
- TLS/HTTPS polling or changes to URL-form `gregg add`;
- changes to `greggd configprint` or `croncheck`;
- EggPool redesign;
- broad TUI redesign, test-suite restructuring, or unrelated cleanup;
- new workflows, jobs, matrices, artifacts, evidence bundles, or CI gates;
- release automation or publication work;
- a Plan 080 created only to record Plan 079 closure.

## Completed roadmap groups

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and runtime cleanup | completed; CI run `30964819950` passed |

Plans 010-035 describing retired staged release/evidence work remain archived under `plans/archive/v1.0.1-release/` and are not current requirements.
