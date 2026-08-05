# Roadmap: bounded correctness and maintainability pass

Status: planned.

## Purpose

Correct the remaining concrete defects found in the current Gregg implementation, then perform only narrowly justified simplification and footprint work. Gregg remains a compact local/LAN system-monitoring daemon and terminal client. This roadmap does not add a monitoring category, a protocol generation, a public-internet deployment model, release automation, or a generalized framework.

The work is ordered so correctness lands before optional cleanup:

```text
066 roadmap
  -> 067 truthful drive used/available capacity
  -> 068 coherent daemon published state and health
  -> 069 daemon CLI/runtime and test correctness
  -> 070 bounded client asynchronous simplification
  -> 071 measured footprint and lightweight closure
```

Phases 067 and 068 may be implemented independently, but Phase 071 closes only after all retained phases are complete. Phase 070 is conditional: retain a rewrite only when it demonstrably reduces production code and preserves behavior. A documented no-change result is acceptable.

## Product contract retained

Gregg continues to provide:

- one native `greggd` collector for Linux, macOS, and Windows;
- cached read-only JSON status and health routes;
- v1 compatibility and v2-first client polling with 404-only fallback;
- bounded mounted-filesystem capacity records;
- one terminal client with normal, condensed, and optional EggPool panes;
- local configuration and native service lifecycle commands;
- a manual release procedure with one small read-only CI workflow.

No feature may be removed to reduce code or binary size.

## Findings owned by this roadmap

### Correctness blockers

1. Drive capacity currently conflates total free space with bytes available to the daemon identity. The v2 drive shape cannot faithfully represent reserved blocks or quota-restricted availability.
2. Windows v2-only publication can make `/healthz` return a success status while serializing a v1 `warming` body because readiness and health are stored separately.
3. `greggd host` and `greggd port` lose whether `--config` was explicitly supplied, so first-run mutation of an absent default configuration fails.
4. One scheduler generation test lacks a test attribute and is not executed.

### Maintainability findings

1. Daemon publication spans multiple locks and atomics, allowing cross-field inconsistency and unnecessary state-machine code.
2. Runtime/library code can terminate the process or initialize global logging unexpectedly instead of returning errors to the binary boundary.
3. The polling scheduler and EggPool worker use more task/channel machinery than the small product may require.
4. The client enables at least one Reqwest feature not used by production code, while release-size options have not been evaluated under a strict retain-only-if-better rule.

## Scope boundary

Allowed work:

- additive optional v2 drive availability data;
- Linux `f_bavail`, macOS `f_bavail`, and Windows caller-available capacity collection;
- client fallback for older v2 payloads without explicit availability;
- one coherent daemon published-state lock or equivalent single-generation state object;
- route status/body consistency for v1 and v2, including Windows v2-only operation;
- correct propagation of explicit versus implicit config paths;
- returning typed errors to the binary boundary and centrally applying existing exit codes;
- restoring omitted test execution and narrowing blanket dead-code allowances;
- measured reduction of scheduler/worker machinery without behavior change;
- measured Cargo feature/profile changes that do not alter supported behavior or MSRV unless a separate policy decision is explicitly made.

Not allowed:

- schema v3;
- authentication, TLS, remote mutation, public-internet hardening, or discovery;
- historical storage, alerts, dashboards, charts, exports, or per-process metrics;
- physical-disk topology, SMART data, RAID inventory, or filesystem-management features;
- multiple EggPool endpoints or expanded EggPool metrics;
- replacing Axum, Ratatui, Clap, Tokio, or Reqwest solely to reduce binary size;
- new CI tiers, release workflows, artifact uploads, evidence bundles, or binary-size gates;
- changing the manual publishing model;
- broad documentation rewrites or historical-plan cleanup during implementation.

## Execution profile for GPT-5.6 Luna

Each phase is written for direct execution by GPT-5.6 Luna or a comparable implementation model.

The executor must:

1. Read the current implementation and focused tests before editing. Do not assume the review description exactly matches later HEAD.
2. Work one phase at a time and do not opportunistically implement later phases.
3. Prefer deletion, consolidation, and direct data flow over new traits, generic frameworks, helper crates, or configuration switches.
4. Preserve public JSON compatibility unless the plan explicitly permits an additive optional field.
5. Add focused regression tests at the same time as each defect correction.
6. Run focused tests first, then `./scripts/check-local.sh`. Use the existing release preflight only in Phase 071.
7. Do not create evidence files. Record only concise commands, results, and measured byte counts in the final handoff or plan status.
8. Stop and document rather than forcing an optional simplification when it increases code, obscures behavior, changes timing semantics, or requires new architecture.
9. Do not mark acceptance criteria complete based on intent, comments, or compilation alone; inspect the implemented behavior and test coverage.

## Phase summary

### Phase 067: truthful drive used/available capacity

Add optional `available_bytes` to v2 drive records, collect the correct platform-native quantity, preserve old payload compatibility, and aggregate explicit availability in the client.

### Phase 068: coherent daemon state and health

Replace fragmented publication state with one coherent state snapshot and guarantee that every health/status response has matching HTTP status, schema, readiness, and cached snapshot semantics.

### Phase 069: daemon CLI/runtime and test correctness

Propagate explicit config intent, keep process termination and logging initialization at the binary boundary, wire the existing exit-code taxonomy or simplify it without adding a new error framework, and restore omitted test execution.

### Phase 070: bounded client asynchronous simplification

Evaluate the poll scheduler and EggPool worker independently. Retain only changes that reduce production machinery while preserving cadence, cancellation, generation, backpressure, and UI behavior. A no-change conclusion is valid.

### Phase 071: measured footprint and lightweight closure

Apply safe manifest cleanup, evaluate release-profile options by measurement, run the existing bounded verification path once, reconcile active documentation, and close this roadmap without adding permanent size or verification infrastructure.

## Verification policy

Routine implementation verification remains:

```bash
./scripts/check-local.sh
```

Use focused commands before the routine check, for example:

```bash
cargo test -p gregg-protocol drive
cargo test -p greggd server
cargo test -p greggd collector
cargo test -p greggd cli
cargo test -p gregg scheduler
cargo test -p gregg eggpool
```

Phase 071 alone runs:

```bash
./scripts/check-local.sh --release
```

One ordinary hosted CI run at the final implementation SHA is sufficient cross-platform closure. Do not require repeated runs, manual evidence records, or uploaded artifacts.

## Roadmap acceptance criteria

- [ ] Explicit drive availability is truthful on Linux, macOS, and Windows while old v2 payloads remain readable.
- [ ] Drive aggregation does not assume `used + available == total` when platform reservations or quotas make that false.
- [ ] Daemon responses are published from one coherent generation of state.
- [ ] Windows v2-only operation cannot produce a successful v1 health response with a non-ready body.
- [ ] Implicit default-config mutation and explicit missing-config behavior are both correct and tested.
- [ ] Runtime/library functions return errors instead of terminating the process.
- [ ] Existing exit-code behavior is either centrally implemented or deliberately simplified; no parallel error framework is introduced.
- [ ] The omitted scheduler test is executed and blanket dead-code suppression is narrowed where practical.
- [ ] Any retained scheduler or EggPool rewrite demonstrably reduces code and preserves behavior; otherwise no rewrite is made.
- [ ] Binary-size changes are measured and retained only when non-regressing and behavior-preserving.
- [ ] Default local checks, one manual release preflight, and one ordinary cross-platform CI run pass.
- [ ] No new product scope, release automation, evidence system, or permanent size gate is added.

## Expected plan files

```text
plans/066-bounded-correctness-and-maintainability-roadmap.md
plans/067-truthful-drive-capacity-semantics.md
plans/068-coherent-daemon-state-and-health.md
plans/069-daemon-cli-runtime-and-test-correctness.md
plans/070-bounded-client-async-simplification.md
plans/071-measured-footprint-and-lightweight-closure.md
plans/README.md
```
