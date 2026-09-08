# Plan 103: maintenance consolidation and bounded diagnostics roadmap

Status: complete. Plans 104-106 closed with evidence in their own records;
acceptance mapping is appended below rather than rewriting the roadmap.

Depends on: current `main` after Plans 098-102 and the September 2026 repository review.

## Objective

Reduce maintenance entropy that accumulated while Gregg gained cross-platform runtime, binary distribution, self-update, and optional EggPool support, then add only the narrow diagnostic improvements that increase operational clarity without expanding Gregg into a generalized observability platform.

This roadmap coordinates Plans 104-106. It does not itself mandate broad product work.

## Product boundary

Gregg remains a small local/LAN system monitor:

- `greggd` collects native host metrics and serves a cached read-only JSON API;
- `gregg` polls a small configured fleet and renders a terminal UI;
- Linux, macOS, and Windows remain supported;
- the optional EggPool summary pane remains bounded and first-party;
- startup/service-manager integration remains explicit and separate from `greggd run`;
- releases remain manually initiated; ordinary CI remains correctness-oriented rather than release orchestration;
- no database, historical telemetry store, alerting engine, generalized exporters, service discovery, user/account system, plugin framework, or public-internet hardening is introduced by this roadmap.

## Baseline findings

The September 2026 review found four classes of actionable work.

### 1. Self-update is duplicated across both application crates

`crates/gregg/src/update.rs` and `crates/greggd/src/update.rs` independently own substantially the same logic for:

- stable-version parsing/comparison;
- target mapping and supported-target tables;
- release asset naming and URL construction;
- `curl` and Cargo discovery;
- bounded download/build execution;
- SHA-256 verification;
- staged candidate validation;
- executable replacement;
- shared update error categories.

The daemon adds legitimate daemon-specific activation/restart behavior, but the transport/staging/replacement mechanism should have one source of truth.

### 2. Several source files now combine too many policies

The top-level crate split remains sound, but some modules have become large policy aggregators, especially daemon startup/update and client config/CLI/polling areas. This increases review cost and makes later changes more likely to couple unrelated behavior.

The goal is mechanical module decomposition, not abstraction for its own sake.

### 3. Repository/release hygiene has obvious duplication and one stray binary

`crates/gregg/src/bin/probe_top.rs` is an old standalone connectivity probe with a historical default LAN address and is auto-built as a normal binary.

The release workflow also embeds substantial repeated shell logic that is difficult to exercise locally. Reusable release-policy checks should live in small scripts where that reduces duplication while preserving the existing five-target release matrix.

### 4. Operational diagnostics are useful but fragmented

Operational truth currently exists across:

- `/v2/healthz`;
- `croncheck`;
- `configprint`;
- startup-state detection;
- `version`;
- client poll error classes.

A bounded read-only diagnostic command and better client offline provenance can compose existing information without creating another monitoring subsystem.

## Workstreams

### Plan 104: shared updater and release-policy consolidation

Plan 104 owns:

- one shared internal updater implementation for `gregg` and `greggd`;
- preserving daemon-only restart/activation logic outside the shared core;
- eliminating duplicated target/asset/version/checksum/Cargo-fallback policy;
- moving clearly reusable release verification logic out of the large Actions workflow into locally runnable scripts where this reduces workflow duplication;
- proving the exact existing update/install/release contract is unchanged.

### Plan 105: source-boundary, repository-hygiene, and dependency/MSRV cleanup

Plan 105 owns:

- removing or explicitly feature-gating `probe_top` so it is not a production auto-built binary;
- mechanically splitting the largest policy-heavy modules where that makes ownership clearer without behavior changes;
- reviewing direct compatibility-only dependency pins and documenting which are still required by Rust 1.75;
- making an explicit keep/raise decision for the MSRV based on measured impact rather than convenience;
- keeping EggPool bounded and preventing new application-specific integrations from entering this pass;
- reconciling architecture/docs affected by module moves.

### Plan 106: bounded diagnostic status and offline provenance

Plan 106 owns:

- a read-only `greggd status` surface that composes existing local config, health, version, and startup-state information;
- concise, actionable client offline/error provenance using already typed polling failures;
- no new daemon write/control path;
- no persistent history or alerting.

## Ordering

Implement in this order unless a concrete dependency requires otherwise:

1. Plan 104 — remove duplicated updater/release policy first.
2. Plan 105 — clean module/repository/dependency boundaries after updater ownership is settled.
3. Plan 106 — add bounded diagnostics on the simplified structure.

Do not interleave broad refactors with user-visible diagnostic changes in one commit series. Each plan should be reviewable and revertible independently.

## Global non-goals

The following are explicitly outside this roadmap:

- replacing HTTP/JSON with another transport;
- adding TLS/authentication simply because diagnostics are being touched;
- changing metric semantics or schema versions;
- adding process inspection, logs, historical graphs, alerts, dashboards, databases, Prometheus/OpenTelemetry exporters, discovery, or plugins;
- expanding EggPool into a generic integration framework;
- restoring service-manager coupling to `greggd run`;
- adding new permanent CI matrices solely for this maintenance pass;
- redesigning the TUI.

## Verification policy

Local verification remains primary. Every implementation plan must require at minimum:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

Use the existing `scripts/check-local.sh` path when applicable.

Cross-platform behavior that cannot be truthfully exercised on the local Ubuntu host should use the existing native CI jobs; do not create a new compatibility pipeline for this roadmap.

Any daemon-runtime change must additionally receive a local Ubuntu foreground lifecycle smoke proving that `greggd run` starts the server directly, `/v2/healthz` becomes a valid Gregg endpoint, and shutdown still works without requiring systemd.

## Roadmap acceptance criteria

Plan 103 is complete only when all of the following are true:

1. Plans 104-106 are individually completed or explicitly rejected with recorded evidence explaining why implementation would not improve the repository.
2. Shared updater/release policy has one authoritative implementation per policy rather than two application copies.
3. `probe_top` is no longer an ordinary production binary.
4. Large-module cleanup reduces policy concentration without introducing unnecessary trait/object/framework complexity.
5. Every compatibility-only dependency pin has a documented reason or is removed.
6. The MSRV is either deliberately retained with evidence or deliberately raised with migration/release notes; it is not changed incidentally.
7. `greggd run`, `croncheck`, direct control-socket stop, startup-manager behavior, installers, and self-update preserve their existing accepted semantics.
8. The client retains bounded polling and the optional EggPool pane without new integration categories.
9. Diagnostic improvements remain read-only and do not introduce persistent telemetry, alerts, or remote control.
10. Active architecture/documentation and the plan index reflect the final structure and status truthfully.

## Roadmap closure (September 2026)

Plans were implemented in the prescribed order (104 → 105 → 106) as
independent commits (`27ec978`, `2e4c4a1`, `a243162`); no broad refactor
was interleaved with user-visible diagnostic changes.

1. Plans 104-106 are individually complete with closure records and
   implementation SHAs; nothing was rejected.
2. Shared updater/release policy has one authoritative implementation per
   policy: `gregg-update` owns version/target/asset/download/checksum/
   staging/replacement; `scripts/release-targets.txt` is the single
   target table (Rust drift test + script derivation).
3. `probe_top` is deleted; the normal package exposes only `gregg`
   (+ feature-gated `lock_helper`).
4. `greggd` startup and client config are split at ownership seams behind
   path-preserving façades; no trait/object/framework layer was added.
5. All 13 compatibility-only pins have documented KEEP reasons
   (`architecture/workspace.md`, Plan 105 audit with relax evidence).
6. MSRV 1.75 is deliberately retained: `cargo check --workspace
   --all-features` passes under Rust 1.75 on the consolidated tree; the
   decision and evidence are recorded in `architecture/workspace.md`.
7. `greggd run`, `croncheck`, direct control-socket stop,
   startup-manager behavior, installers, and self-update preserve their
   accepted semantics (full suites green; Ubuntu run/healthz/stop,
   status running/stopped/occupied, and updater lifecycle smokes pass;
   scheduler/runtime/server/sampler/installers untouched).
8. The client retains bounded polling and the optional EggPool pane
   without new integration categories (both untouched by this roadmap).
9. `greggd status` is strictly read-only (no start/stop/restart/install,
   no `sudo`); offline provenance adds no history, alerts, or control.
10. `README.md`, crate READMEs, `docs/`, `architecture/`, skills,
    `AGENTS.md`, `CHANGELOG.md`, `RELEASING.md`, and `plans/README.md`
    reflect the final structure and status truthfully.
