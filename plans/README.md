# Gregg plan index

This directory contains Gregg's implementation roadmaps and execution-ready plans.

## Current direction

Gregg is a small local/LAN system monitor. Planning must preserve that boundary:

- native Linux, macOS, and Windows metric collection;
- a cached read-only JSON daemon API;
- a compact terminal client for a small fleet;
- optional bounded EggPool summary integration;
- local-first verification and manual releases;
- no generalized observability platform, public-internet service, release orchestration, or evidence system.

Local tests are the primary development path. The one existing GitHub Actions workflow provides generic Linux checks and native macOS/Windows truth. CI does not publish crates, create tags or releases, upload evidence bundles, or enforce binary-size gates.

## Active roadmap

[`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) implemented its original corrective phases through Plan 071. [`072-windows-service-runtime-and-record-correction.md`](072-windows-service-runtime-and-record-correction.md) is the sole active follow-up. It corrects the Windows SCM runtime boundary and reconciles the existing closure records without reopening drive metrics, server state, client scheduling, EggPool behavior, or footprint work.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) | Correct concrete cross-platform metric/API defects, then retain only justified simplification and footprint changes | implementation complete through 071; final closure pending 072 |
| [`067-truthful-drive-capacity-semantics.md`](067-truthful-drive-capacity-semantics.md) | Add additive optional v2 availability, distinguish total-free from caller-available space on all platforms, and preserve old-daemon compatibility | complete |
| [`068-coherent-daemon-state-and-health.md`](068-coherent-daemon-state-and-health.md) | Publish one coherent server-state generation and make v1/v2 status and health semantics consistent, including Windows v2-only operation | complete |
| [`069-daemon-cli-runtime-and-test-correctness.md`](069-daemon-cli-runtime-and-test-correctness.md) | Fix implicit/default config mutation, keep exit/logging at the binary boundary, make exit codes truthful, and restore an omitted scheduler test | implementation complete; Windows service runtime boundary correction owned by 072 |
| [`070-bounded-client-async-simplification.md`](070-bounded-client-async-simplification.md) | Independently evaluate scheduler and EggPool worker simplification under strict retain-only-if-smaller behavior-preserving gates | complete; no change |
| [`071-measured-footprint-and-lightweight-closure.md`](071-measured-footprint-and-lightweight-closure.md) | Measure safe feature/profile changes and close once through the existing local preflight and ordinary CI | implementation and hosted verification complete; record corrections owned by 072 |
| [`072-windows-service-runtime-and-record-correction.md`](072-windows-service-runtime-and-record-correction.md) | Ensure foreground and Windows SCM modes each own one nonblocking runtime, then correct Plans 066/069/071 and the index in place | planned; active |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072
```

Plans 067 and 068 remain complete and independent. Plan 070 correctly concluded with no retained rewrite. Plan 072 is a concrete product correction, not an evidence-only closure phase, and must not spawn Plan 073 merely to record results.

## Execution guidance for GPT-5.6 Luna

Plans 066-072 are written for direct handoff to GPT-5.6 Luna or a comparable implementation model. The executor should:

1. Inspect current HEAD and focused tests before editing.
2. Execute only Plan 072; do not opportunistically reopen earlier completed phases.
3. Prefer direct command dispatch, one runtime per async mode, and one nonblocking shutdown signal over new traits, frameworks, helpers, configuration, or dependencies.
4. Preserve all product features, supported platforms, v1 compatibility, and the existing manual release model.
5. Add focused regressions for command/runtime separation and SCM shutdown signaling.
6. Run focused tests first and `./scripts/check-local.sh` after implementation.
7. Use one ordinary existing CI run for Windows production compilation and native cross-platform closure.
8. Correct existing plan records in place; do not create evidence files or another closure plan.
9. Do not mark acceptance criteria complete based on intent, comments, or compilation alone; inspect implemented behavior and test coverage.

## Completed roadmap groups

The following roadmap groups remain implementation history and are not active acceptance gates.

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and current-thread runtime cleanup | completed; ordinary CI run `30964819950` passed at `aaf0cab` |

Plans 010-035 describing retired staged release/evidence work remain archived under:

```text
plans/archive/v1.0.1-release/
```

They are historical references, not current requirements.

## Verification model

Routine development:

```bash
./scripts/check-local.sh
```

This remains the short format-and-workspace-test loop.

Manual release preflight:

```bash
./scripts/check-local.sh --release
```

This remains a nonpublishing preflight for Clippy, documentation, package/version checks, installed-daemon smoke, and the protocol publish dry-run. Plan 072 does not require another release preflight unless implementation changes release profile, dependencies, packaging, or release behavior.

One green ordinary CI run at the final implementation SHA, or a source-equivalent plan-only descendant, is sufficient hosted cross-platform closure.

A plan does not require:

- an immutable candidate SHA;
- a dedicated qualification workflow;
- repeated green runs;
- uploaded artifacts or evidence bundles;
- artifact IDs, digests, provenance, or finalizer documents;
- a separately maintained Windows host record;
- real Windows service installation in CI;
- crates.io publication;
- a Git tag or GitHub Release.

## Completion rule

A phase is complete only when its explicit product acceptance criteria are implemented and demonstrated by the lightest appropriate mechanism:

- deterministic unit/integration tests;
- a short bounded product smoke;
- the default local check;
- the existing release preflight only when release-facing behavior changes;
- one ordinary hosted CI run for native platform truth;
- direct documentation inspection for behavior and scope claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier commit that no longer matches HEAD.

## Scope control for Plans 066-072

Allowed:

- additive optional v2 drive availability;
- Linux/macOS/Windows caller-available filesystem capacity;
- client fallback for old v2 drive records;
- one coherent daemon published-state object;
- consistent v1/v2 route status and health bodies;
- correct implicit versus explicit config handling;
- returning runtime errors to the binary boundary;
- truthful use of the existing exit-code taxonomy;
- restoring missing test execution and narrowing blanket warning allowances;
- conditional scheduler/EggPool simplification only when production machinery is reduced;
- measured Reqwest feature and release-profile cleanup;
- synchronous command dispatch before runtime creation;
- exactly one current-thread runtime for foreground mode and one for Windows SCM mode;
- nonblocking SCM Stop/Shutdown signaling into `run_with_shutdown()`;
- direct correction of Plans 066, 069, 071, and this index;
- concise directly affected documentation.

Not allowed:

- protocol v3;
- authentication, TLS for greggd, remote mutation, discovery, or public-internet hardening;
- history, alerts, dashboards, charts, exports, or per-process metrics;
- SMART, physical-disk, partition, RAID, LVM, or storage-topology inventory;
- expanded EggPool endpoints, instances, metrics, periods, retries, or configuration;
- replacement of core crates solely for size;
- MSRV changes;
- SCM lifecycle redesign, pause/continue support, service recovery policy, or packaging changes;
- automatic publishing or release creation;
- new CI tiers, real-service installation, privileged runners, artifacts, evidence records, performance suites, or binary-size gates;
- feature removal to reduce binary size;
- a Plan 073 created only to mark Plan 072 complete.

Any broader finding should be reported separately and left unimplemented unless it is necessary to preserve current-product correctness or safety.
