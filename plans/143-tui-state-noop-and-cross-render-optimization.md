# Plan 143: TUI/state no-op and cross-render optimization

Status: complete.

Depends on: Plan 138 and the settled Plan-122 TUI optimization baseline. It may proceed independently of Plans 139-142.

## Objective

Remove remaining bounded client-side state/render work that survives Plan 122 without changing any visible terminal output, interaction, fleet-wide geometry, polling semantics, or public reducer APIs.

This is deliberately a polish pass. Do not redesign the TUI around retained widgets or a new rendering framework.

## Current residual work

### Event-loop redraws

The event loop correctly uses a `dirty` gate, but an accepted select branch can still mark dirty before knowing whether state changed.

Examples include:

- a stale/rejected poll batch;
- navigation actions already at a boundary;
- repeated state-clearing actions that are already clear;
- some config/worker status transitions that may be logical no-ops.

### Normal-view fleet preparation

Normal view caches `MetricRows` by per-system render key, but each render still:

- allocates an index-aligned `Vec<Option<Rc<MetricRows>>>`;
- scans the fleet;
- derives drive/network aggregate values for the key and may derive related values again when rebuilding rows.

Fleet-wide scanning itself is required because off-screen systems affect geometry.

### Condensed preformatting

Condensed view formats up to seven Strings per online system on each real redraw. Plan 122 removed duplicate formatting within one render, but identical snapshots still reformat across later redraws.

### Config reconciliation

`AppState::reconcile_systems` currently clones retained `SystemState` values from its old-ID map. Plan 140 may own this move optimization if it lands first; Plan 143 must not duplicate it.

## Implementation

### 1. Add internal changed-result reducer seams

Preserve public borrowed/owned reducer methods.

Add private/internal variants or result helpers that report whether render-visible state changed.

At minimum:

- a batch rejected by generation logic reports unchanged;
- stale-target results ignored by host/port guard do not force a redraw unless another result changed state;
- boundary navigation can report unchanged;
- clearing an already-clear highlight reports unchanged.

Do not perform expensive full-`AppState` equality snapshots merely to obtain the bool.

If a field rendered by the TUI changes (including latency/error text where applicable), report changed.

### 2. Drive event-loop dirty state from reducer result

Use the changed result for Systems batches/actions.

Preserve:

- initial draw;
- highlight deadline behavior;
- terminal Resize redraw;
- EggPool state transitions;
- config reload diagnostics;
- shutdown behavior.

Add event-loop tests with a draw counter or a small test terminal seam showing no extra frame for a rejected stale batch and a boundary no-op action.

### 3. Extend condensed formatting memo across renders

Add a bounded per-system condensed cache keyed only by values that affect condensed output:

- display host/name;
- CPU/memory percentages;
- drive aggregate percentage;
- network aggregate utilization;
- load.1;
- iowait capability/value;
- reachability where needed.

Reuse preformatted Strings when the key matches.

Prune departed system IDs under the same bounded-membership principle as the existing metric-row cache.

Do not make cache identity depend on pointer equality alone; config reload/mutation must invalidate correctly.

### 4. Avoid duplicate aggregate work in normal cache misses

Where `MetricRenderKey::from_snapshot` computes an aggregate that `build_metric_rows` immediately recomputes on the same cache miss, pass/reuse the already-derived value or restructure the private key/build helper.

Do not store a full cloned `NormalizedSnapshot` merely to avoid recomputation.

### 5. Keep fleet-wide geometry authoritative

Do not cache a layout in a way that ignores:

- terminal-width change;
- off-screen online systems;
- SWP versus COMMIT label width;
- NET-row availability;
- display-name changes;
- condensed online/offline/pending host-width needs.

A per-render O(N) geometry scan is acceptable. The target is expensive formatting/recomputation and provable no-op frames, not eliminating the semantic fleet scan.

## Deterministic evidence

Required tests:

- stale generation batch => no state-change signal/no draw;
- stale endpoint target ignored => no draw if nothing else changed;
- boundary MoveUp/MoveDown => no draw when selection truly unchanged;
- actual selection change still redraws and resets highlight timer;
- Resize always redraws as required;
- identical condensed snapshots reuse cached formatted values;
- changing each condensed key field invalidates its row;
- config removal prunes cache;
- normal cache miss computes drive/network aggregate once through the chosen helper structure;
- existing renderer TestBackend output remains identical at representative widths and mixed fleets.

Do not make tests dependent on wall-clock renderer timing.

## Measurement

Use deterministic format/build/draw counters first.

Optionally run the existing sustained workload or a large synthetic fleet in release mode to record descriptive before/after CPU/render preparation.

Record stripped `gregg` size.

## Verification

~~~text
cargo test -p gregg --all-targets --all-features -- state
cargo test -p gregg --all-targets --all-features -- ui
cargo test -p gregg --bin gregg
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

## Acceptance criteria

- [x] Rejected stale batches no longer force a frame.
- [x] Proven boundary/no-op Systems actions no longer force a frame.
- [x] Real state changes still redraw immediately.
- [x] Condensed formatting is reused across unchanged redraws.
- [x] Cache invalidation covers every render-relevant condensed field and config membership change.
- [x] Normal-view cache misses avoid duplicate aggregate calculation where structurally practical.
- [x] Fleet-wide/off-screen geometry semantics are unchanged.
- [x] Existing TestBackend presentation output remains unchanged.
- [x] Public AppState/reducer APIs remain source-compatible.
- [x] No new dependency or TUI framework is introduced.
- [x] Focused tests, workspace gates, and Rust 1.89 remain green.

## Explicit non-goals

Do not include:

- viewport-local layout;
- partial terminal-region redraw architecture;
- mouse support/themes;
- Ratatui replacement;
- scheduler/poller changes;
- new TUI features;
- animation or refresh-cadence changes;
- history graphs;
- global allocator instrumentation;
- timing gates.

## Handoff note

Start with changed-result plumbing and draw-count tests. Add cross-render formatting caches only after exact TestBackend output is frozen; the cache is an implementation detail and must never become a second source of presentation truth.

## Closure record

Implemented at `83df89e` with toolchain `rustc 1.98.1`. Local
verification: `cargo test -p gregg --lib --all-features`
(586 passed, including 6 new `state::tests::plan143_*` and 5 new
`ui::tests::plan143_*`), `cargo test -p gregg --all-targets
--all-features --bin gregg` (9 passed), `cargo fmt --check`,
workspace clippy `-D warnings`, and `./scripts/check-local.sh`
green. Final campaign CI run is recorded in Plan 138.

Deterministic evidence (`crates/gregg/src/state.rs`, `ui/mod.rs`,
`ui/condensed.rs`, `ui/system_block.rs`, `src/main.rs`):

- `apply_batch_changed`/`apply_batch_owned_changed` return `false`
  for rejected generations and for accepted batches where every
  result was ignored (host/port guard) or left reachability, latest,
  and offline provenance identical; timestamps/latency alone never
  force a frame (draw-counter pattern: `if changed { draws += 1 }`
  stays 0 for stale/ignored/identical batches);
- `apply_action_changed` returns `false` for boundary
  `MoveUp`/`MoveDown` with unchanged selection and highlight,
  clearing an already-clear highlight, and other logical no-ops;
  `Resize` always `true`; real selection change redraws and arms the
  highlight timer;
- event loop drives `dirty` from `apply_batch_owned_changed`,
  `apply_eggpool_result_changed` (stale/cancelled `EggPool` results
  no longer force a frame), `dispatch_action_with_store →
  Result<bool>`, `begin_system_refresh → Result<bool>`, and
  highlight-expiry `apply_action_changed`; initial draw, `Resize`,
  config-reload diagnostics, and shutdown preserved;
- condensed `CondensedRenderKey` (host, reachability, CPU/MEM/drive/
  network/load/iowait) memoizes `Rc<PreformattedValues>` across
  redraws (`ptr_eq` reuse on identical keys, invalidation on host or
  metric change, bounded prune of departed IDs); consecutive
  `TestBackend` renders identical;
- normal cache misses pass the key's drive/network aggregates into
  `build_metric_rows_with_aggregates`, so the same snapshot is not
  aggregated twice (`format!("{direct:?}") ==
  format!("{shared:?}")`).

Public `AppState::apply_batch/apply_batch_owned/apply_action` and
`apply_eggpool_result` shapes preserved (new `*_changed` methods are
additive); fleet-wide/off-screen geometry, Ratatui diffing, and
existing `TestBackend` output unchanged; no new dependencies.
