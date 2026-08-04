# Roadmap: narrow correctness and simplification pass

Status: Phase 64 completed; Phase 65 remains pending.

## Purpose

Close the small set of correctness, packaging, and maintenance issues found by the August 2026 repository review without expanding Gregg's product scope or redesigning its architecture.

Gregg already meets its core goal: a compact private-LAN system monitor with native Linux, macOS, and Windows collection, a cached read-only daemon API, and a keyboard-first multi-system TUI. This roadmap is not a new feature program. It is a bounded corrective pass over defects and unnecessary verification/package weight that remain after the completed Windows, drive, multiview, and EggPool roadmaps.

The work is intentionally limited to:

- correct v2 snapshot staleness behavior on Windows;
- strict schema parsing for `/v2/status` and `/v1/status`;
- removal of one test-only binary from normal installation/package output;
- removal of unused config-reload state machinery if repository search confirms it has no production caller;
- correction of active documentation that disagrees with implementation;
- reduction of duplicated local and CI checks while preserving truthful native-platform coverage;
- low-risk client runtime/dependency feature cleanup with measured before/after binary sizes.

No user-facing monitoring feature is added by this roadmap.

## Current assessment

The current architecture remains the baseline:

```text
gregg-protocol <- greggd
gregg-protocol <- gregg
```

The following boundaries remain correct and must not be reopened:

- three-crate workspace separation;
- native platform collectors;
- cached snapshots rather than collection on request;
- v1 compatibility plus capability-aware v2;
- normal/condensed systems views and optional drive expansion;
- optional one-source EggPool pane;
- private-network deployment model;
- manual crates.io and GitHub release process.

The review found two product-correctness defects and several smaller maintenance issues.

### Correctness defect A: Windows v2 snapshot age

Windows publishes only a v2 snapshot. The current stale-age check reads the v1 snapshot slot, so age-based expiration can fail to apply to Windows v2 data after collection stops. The correction must make one staleness decision from the latest available observation timestamp and apply it consistently to both v1 and v2 status/health routes.

### Correctness defect B: endpoint/schema mismatch acceptance

The client currently uses a shared parser that attempts v2 and then v1 regardless of which endpoint produced the response. A valid v1 payload returned from `/v2/status` can therefore be accepted instead of rejected. The correction must bind the expected schema to the requested endpoint and preserve the existing v2-first, 404-only v1 fallback.

### Packaging issue

`lock_helper` is a cross-process test helper but is declared as a normal binary target. It must not be installed or published as a user-facing Gregg executable.

### Dead-path issue

The reducer contains config-reload/rebuild machinery without a production event source. If a complete repository search confirms that the path is test-only and not part of the documented product contract, remove it rather than implementing live reload, watcher lifecycle, scheduler replacement, or worker reconstruction.

### Verification issue

The default local script and ordinary CI repeat tests and include release-oriented checks in normal iteration paths. The correction must remove duplication, not replace it with another tier, workflow, evidence bundle, or qualification system.

### Footprint issue

The client enables Tokio features used only by tests and uses the multithread runtime despite a small I/O-bound event loop. These are candidates for low-risk cleanup, but changes must be retained only when focused tests pass and release binary size does not regress.

## Governing principles

### 1. Correct defects before simplifying machinery

Phase 64 owns correctness and package truth. Phase 65 begins only after Phase 64 behavior is covered by focused tests.

Do not combine correctness fixes with broad scheduler, server-state, protocol, or UI rewrites. Local refactoring is allowed only when it is the smallest reliable way to remove the defect.

### 2. Preserve product behavior

The following remain unchanged:

- configured endpoints and config schema;
- CLI commands and key bindings;
- v1 fallback for old daemons;
- v2 capability semantics;
- displayed metrics and layouts;
- EggPool behavior;
- service lifecycle commands;
- default ports and configuration paths;
- private-LAN security posture.

### 3. Prefer deletion over completing unused abstractions

If config reload is not reachable from production, remove the unused action and rebuild path. Do not add file watching, hot reload, scheduler replacement, worker migration, or a general effect system merely to justify existing dead code.

### 4. Verification must remain proportionate

Use focused regression tests for the two correctness defects, package listing for the helper binary, and the existing ordinary native CI jobs. Do not add:

- a second workflow;
- release artifacts;
- evidence files;
- repeated green-run requirements;
- live multi-host test infrastructure;
- long-running soak tests;
- coverage thresholds;
- benchmark gates.

### 5. Binary-size work must be measured and reversible

Measure the release binaries before and after Phase 65. Accept only changes that preserve behavior and do not increase binary size. Do not replace Reqwest, Rustls, Axum, Clap, Ratatui, Crossterm, Serde, or Tokio in this roadmap.

### 6. Do not reopen MSRV policy

The declared Rust 1.75 contract and its compatibility pins are not changed by this roadmap. A future MSRV policy change would affect dependency resolution and downstream compatibility and therefore requires a separate explicit decision. Phase 65 may simplify duplicated checks around the MSRV job but must not silently remove the compatibility promise.

## Phase map

| Phase | Plan | Outcome |
| --- | --- | --- |
| 64 | `064-status-protocol-package-correctness.md` | Correct Windows v2 staleness, enforce endpoint-specific schema parsing, remove the helper binary from normal packages, remove confirmed dead reload machinery, and reconcile active documentation. |
| 65 | `065-proportionate-verification-and-footprint-cleanup.md` | Remove duplicate local/CI work and apply only measured low-risk runtime/dependency-feature reductions without changing product features or compatibility policy. |

## Dependency graph

```text
64 -> 65
```

Phase 65 must not start by restructuring production code that Phase 64 is still correcting.

## Program scope

### In scope

- one shared v1/v2 snapshot-age source for server staleness decisions;
- Windows v2 stale-status and stale-health regression coverage;
- endpoint-specific response parsing;
- v2-first and 404-only v1 fallback regression coverage;
- feature-gating or otherwise excluding `lock_helper` from normal installation/package output;
- removing unreachable config-reload/rebuild code after repository-wide caller confirmation;
- correcting active README and architecture statements about macOS swap capability, Windows drive eligibility, and Windows v1/v2 health behavior;
- removing duplicated native test reruns from local scripts and CI;
- keeping one ordinary read-only workflow with representative native jobs;
- moving test-only Tokio features out of production dependencies;
- evaluating the current-thread client runtime;
- recording concise before/after release binary sizes in the Phase 65 closure note or commit message.

### Out of scope

- new metrics, panes, key bindings, CLI commands, configuration fields, or protocol versions;
- historical telemetry, alerts, charts, exports, discovery, plugins, or dashboards;
- public-internet TLS/authentication/rate-limit hardening;
- changing EggPool integration;
- replacing the scheduler with streams, actors, or a task framework;
- redesigning daemon supervision;
- consolidating all server locks/state as an architecture project;
- replacing core HTTP/TLS/TUI/CLI dependencies;
- changing the Rust 1.75 compatibility promise;
- adding package-manager distribution or automatic updates;
- release automation, qualification workflows, evidence artifacts, or publication changes;
- performance tuning without a demonstrated Gregg workload problem.

## Core invariants

1. Linux and macOS v1/v2 behavior remains compatible.
2. Windows continues to publish v2 only.
3. `/v2/status` accepts only a valid v2 payload.
4. `/v1/status` accepts only a valid v1 payload.
5. The client falls back from v2 to v1 only after an HTTP 404 from `/v2/status`.
6. Snapshot age is evaluated from the latest published snapshot regardless of wire version.
7. Stale v2 data cannot remain indefinitely available on Windows.
8. `cargo install gregg` exposes only the intended `gregg` executable.
9. Removing dead config-reload code does not remove a documented user feature.
10. The ordinary CI workflow never publishes or creates release artifacts.
11. Native Windows and macOS behavior remains checked on native hosted runners.
12. Default local iteration is shorter than the current script and contains no duplicated test invocation.
13. Production dependency features contain no test-only Tokio feature.
14. Any runtime/feature cleanup retains all existing client behavior and does not increase release binary size.
15. No new generalized abstraction is introduced.

## Lightweight validation strategy

### Phase 64 focused checks

```text
cargo fmt --all -- --check
cargo test -p greggd server
cargo test -p gregg poller
cargo test -p gregg config
cargo package --list -p gregg
```

Use exact test filters based on the final test names. One Windows hosted run is sufficient native confirmation for the v2-only path. The stale logic itself should be unit-testable without a manually maintained Windows machine.

### Phase 65 focused checks

```text
cargo check --workspace
cargo test --workspace
cargo build --release -p gregg -p greggd
```

Record file sizes for the two release binaries before and after retained footprint changes. Do not add a benchmark harness or binary-size CI gate.

### Final closure

- the revised default local check passes;
- the manual release preflight remains available and passes when intentionally invoked;
- one ordinary CI run passes at the implementation SHA or a source-equivalent descendant;
- plan statuses and `plans/README.md` are updated concisely;
- no evidence bundle is created.

## Closure criteria

This roadmap is complete when:

- [ ] Windows v2 stale snapshots use the configured age policy correctly.
- [ ] Endpoint responses are parsed only as the schema expected for that endpoint.
- [ ] v1 fallback remains 404-only and is regression-tested.
- [ ] `lock_helper` is absent from normal package/install output.
- [ ] unreachable config-reload machinery is removed or a production caller is documented and the deletion is explicitly skipped.
- [ ] active documentation matches implemented platform capabilities and route behavior.
- [ ] the default local check no longer repeats collector tests after workspace tests.
- [ ] ordinary CI contains no duplicated native reruns or release/publish behavior.
- [ ] native Windows and macOS source truth remains represented.
- [ ] production Tokio features exclude test-only support.
- [ ] any current-thread runtime change is supported by focused tests and non-regressing size measurements.
- [ ] the Rust 1.75 compatibility policy is unchanged.
- [ ] no new product scope, workflow, evidence system, or generalized framework was added.

## Handoff guidance

Implement Phase 64 and Phase 65 as narrow commits. During implementation, discovered unrelated issues should be recorded separately rather than folded into this roadmap unless they directly prevent one of the acceptance criteria above.

A clean implementation may touch:

```text
crates/greggd/src/server/mod.rs
crates/gregg/src/poller.rs
crates/gregg/src/action.rs
crates/gregg/src/state.rs
crates/gregg/Cargo.toml
crates/gregg/src/bin/lock_helper.rs
scripts/check-local.sh
scripts/check-local.ps1
.github/workflows/ci.yml
README.md
architecture/*.md
plans/README.md
```

This list is a boundary, not a mandate. Avoid unrelated formatting or documentation rewrites.