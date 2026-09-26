# Plan 143: TUI/state no-op and cross-render optimization

Status: planned.

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

- [ ] Rejected stale batches no longer force a frame.
- [ ] Proven boundary/no-op Systems actions no longer force a frame.
- [ ] Real state changes still redraw immediately.
- [ ] Condensed formatting is reused across unchanged redraws.
- [ ] Cache invalidation covers every render-relevant condensed field and config membership change.
- [ ] Normal-view cache misses avoid duplicate aggregate calculation where structurally practical.
- [ ] Fleet-wide/off-screen geometry semantics are unchanged.
- [ ] Existing TestBackend presentation output remains unchanged.
- [ ] Public AppState/reducer APIs remain source-compatible.
- [ ] No new dependency or TUI framework is introduced.
- [ ] Focused tests, workspace gates, and Rust 1.89 remain green.

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
