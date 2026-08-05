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

[`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) and Plans 067-072 implemented the original correctness, state, runtime, and footprint work. [`073-native-windows-scm-entry-and-readiness-correction.md`](073-native-windows-scm-entry-and-readiness-correction.md) is the sole active follow-up. It adds the required native SCM dispatcher/`ServiceMain` boundary, makes service readiness and config-path handling truthful, and closes through the existing manual Windows lifecycle smoke.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) | Correct concrete cross-platform metric/API defects, then retain only justified simplification and footprint changes | implementation complete through 072; final Windows service closure pending 073 |
| [`067-truthful-drive-capacity-semantics.md`](067-truthful-drive-capacity-semantics.md) | Add additive optional v2 availability, distinguish total-free from caller-available space on all platforms, and preserve old-daemon compatibility | complete |
| [`068-coherent-daemon-state-and-health.md`](068-coherent-daemon-state-and-health.md) | Publish one coherent server-state generation and make v1/v2 status and health semantics consistent, including Windows v2-only operation | complete |
| [`069-daemon-cli-runtime-and-test-correctness.md`](069-daemon-cli-runtime-and-test-correctness.md) | Fix implicit/default config mutation, keep exit/logging at the binary boundary, make exit codes truthful, and restore an omitted scheduler test | complete; service-runtime follow-up corrected by 072 |
| [`070-bounded-client-async-simplification.md`](070-bounded-client-async-simplification.md) | Independently evaluate scheduler and EggPool worker simplification under strict retain-only-if-smaller behavior-preserving gates | complete; no change |
| [`071-measured-footprint-and-lightweight-closure.md`](071-measured-footprint-and-lightweight-closure.md) | Measure safe feature/profile changes and close once through the existing local preflight and ordinary CI | complete; records corrected by 072 |
| [`072-windows-service-runtime-and-record-correction.md`](072-windows-service-runtime-and-record-correction.md) | Ensure foreground and Windows SCM modes each own one nonblocking runtime, then correct Plans 066/069/071 and the index in place | implementation complete; missing native dispatcher/readiness closure owned by 073 |
| [`073-native-windows-scm-entry-and-readiness-correction.md`](073-native-windows-scm-entry-and-readiness-correction.md) | Add the SCM dispatcher and generated `ServiceMain`, honor service config paths, report `RUNNING` after bind, and prove the lifecycle with the existing Windows smoke | planned; active |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073
```

Plans 067 and 068 remain complete and independent. Plan 070 correctly concluded with no retained rewrite. Plan 073 is a concrete product correction discovered after Plan 072, not an evidence-only closure phase, and must not spawn Plan 074 merely to record results.

## Execution guidance for GPT-5.6 Luna

Plans 066-073 are written for direct handoff to GPT-5.6 Luna or a comparable implementation model. The executor should:

1. Inspect current HEAD and focused tests before editing.
2. Execute only Plan 073; do not opportunistically reopen earlier completed phases.
3. Use the `windows-service` dispatcher and generated `ServiceMain` directly; do not add raw Win32 FFI or a service framework.
4. Preserve the Plan 072 single-runtime and nonblocking one-shot shutdown design.
5. Pass the selected config path through one small process-local launch context and report `RUNNING` only after listener bind.
6. Add focused control-mapping and readiness-order tests without installing a service in unit tests.
7. Run focused tests and `./scripts/check-local.sh`, then one ordinary existing CI run.
8. Use the existing manual `scripts/smoke-windows.ps1` on an Administrator Windows host as the final operational SCM proof.
9. Correct existing plan records in place; do not create evidence files or another closure plan.
10. Do not mark operational service criteria complete based on compilation or comments alone.

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

This remains a nonpublishing preflight for Clippy, documentation, package/version checks, installed-daemon smoke, and the protocol publish dry-run. Plan 073 does not require another full release preflight unless implementation changes release-facing behavior beyond the directly corrected Windows scripts.

One green ordinary CI run at the final implementation SHA, or a source-equivalent plan-only descendant, is sufficient hosted cross-platform compilation/test closure. Plan 073 additionally requires one successful real Windows SCM run through the existing manual lifecycle smoke because dispatcher invocation cannot be demonstrated by ordinary unit tests alone.

A plan does not require:

- an immutable candidate SHA;
- a dedicated qualification workflow;
- repeated green runs;
- uploaded artifacts or evidence bundles;
- artifact IDs, digests, provenance, or finalizer documents;
- a separately maintained Windows host record;
- real Windows service installation in ordinary CI;
- crates.io publication;
- a Git tag or GitHub Release.

## Completion rule

A phase is complete only when its explicit product acceptance criteria are implemented and demonstrated by the lightest appropriate mechanism:

- deterministic unit/integration tests;
- a short bounded product smoke;
- the default local check;
- the existing release preflight only when release-facing behavior changes;
- one ordinary hosted CI run for native platform compilation/test truth;
- one real Windows SCM lifecycle smoke when operational native service behavior is the feature under correction;
- direct documentation inspection for behavior and scope claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier commit that no longer matches HEAD.

## Scope control for Plans 066-073

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
- `service_dispatcher::start` and one generated Windows `ServiceMain` callback;
- one small process-local service config-path handoff;
- SCM `RUNNING` publication only after successful listener bind;
- minimal corrections to the existing Windows install and manual smoke scripts needed to prove the service lifecycle;
- direct correction of Plans 066, 072, 073, and this index;
- concise directly affected documentation.

Not allowed:

- protocol v3;
- authentication, TLS for greggd, remote mutation, discovery, or public-internet hardening;
- history, alerts, dashboards, charts, exports, or per-process metrics;
- SMART, physical-disk, partition, RAID, LVM, or storage-topology inventory;
- expanded EggPool endpoints, instances, metrics, periods, retries, or configuration;
- replacement of core crates solely for size;
- MSRV changes;
- raw Windows dispatcher FFI, multiple services per process, service DLLs, pause/continue support, or broad SCM lifecycle redesign;
- MSI/WiX packaging or general installer redesign;
- automatic publishing or release creation;
- new CI tiers, real-service installation in ordinary CI, privileged runner infrastructure, artifacts, evidence records, performance suites, or binary-size gates;
- feature removal to reduce binary size;
- a Plan 074 created only to mark Plan 073 complete.

Any broader finding should be reported separately and left unimplemented unless it is necessary to preserve current-product correctness or safety.
