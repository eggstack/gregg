# Plan 122: TUI render path and redraw optimization

Status: complete; implementation `45582ce`.

Depends on: Plan 120. Implement after or alongside Plan 121 once any reducer-internal helper names are settled.

## Objective

Reduce steady-state gregg TUI CPU work and allocation volume for medium/large fleets without changing terminal output, fleet ordering, viewport behavior, selection semantics, Unicode geometry, normal/condensed feature coverage, or public client APIs.

The current renderer already has meaningful correctness work from Plans 083-087 and 110/114. This plan must optimize those exact semantics rather than simplify them away.

## Current costs

### Unconditional redraw after every event-loop wakeup

run_event_loop performs terminal.draw after each tokio::select iteration.

Terminal events that translate to no Action therefore still rebuild a complete frame. Channel/timer branches also redraw regardless of whether they actually produced a render-visible mutation.

Ratatui can avoid writing unchanged terminal cells, but Gregg still pays its own layout and String-building cost to construct the candidate frame.

### Normal-view metric memo uses linear lookup and full snapshot copies

METRIC_ROWS_CACHE is thread-local Vec<CachedMetricRows>.

For every online system, metric_rows_for_fleet linearly searches the cache by system ID. A large fleet therefore performs repeated stable-ID comparisons on every render.

Each cache entry stores a full NormalizedSnapshot clone merely to detect whether build_metric_rows needs to run again. That copy includes identity strings and optional drive/disk/network vectors that are mostly irrelevant to the four/five normal base rows.

### Visible rendering linearly searches online_rows

render builds Vec<(usize, Rc<MetricRows>)>. For each visible online entry it then calls iter().find by index.

The result is another fleet scan inside viewport rendering.

### Normal suffixes are formatted repeatedly

compute_fleet_metric_layout constructs natural suffix Strings across participating systems to measure widths. resolve_metric_suffixes later constructs full or percentage-only strings again for individual systems.

For snapshots unchanged across many event-loop redraws, these formatted values are deterministic and suitable for memoization with the metric rows.

### Condensed mode preformats twice

compute_condensed_table_layout builds PreformattedValues for every online system, then render_online_row calls preformat_online again for each visible system.

The host-width pass also creates an owned String solely to measure configured name/host width.

## Preserved renderer invariants

Do not change:

- current Normal/Condensed key behavior;
- online-first stable configured order;
- fleet-wide normal metric label/bracket geometry;
- Plan-087 suffix suppression rule;
- per-system optional NET row;
- SWP versus COMMIT truth;
- selected drive/network expansion and clipping;
- condensed tier boundaries and HOST-only flexible/truncatable policy;
- offline/pending identity rendering;
- Unicode display-cell width semantics;
- logical selection versus transient reverse-video highlight;
- key-hint placement;
- minimum terminal-width/height behavior.

Existing TestBackend expectations are authoritative unless a test is proven stale.

## Workstream A: event-loop dirty redraw gating

Introduce one local dirty/redraw-needed flag per loop iteration.

The initial frame still renders immediately.

Set dirty only when a branch can change render-visible state. At minimum:

- accepted poll batch;
- accepted EggPool result or worker-unavailable transition;
- translated user Action that can affect state;
- highlight expiry;
- completed successful config replacement/reconciliation;
- terminal resize.

Do not redraw for a key/input event that translate_event rejects.

Where a branch can determine cheaply that nothing changed, leave dirty false. Do not force broad AppState public API changes solely to expose mutation booleans; additive crate-internal helpers are acceptable if they make this precise.

Do not implement partial/damaged-region rendering. Gregg should continue to let Ratatui diff complete frames when a frame is actually needed.

Tests must prove:

1. unmapped keys do not invoke draw after the initial frame;
2. mapped navigation still redraws;
3. resize redraws;
4. poll/EggPool state transitions redraw;
5. highlight expiry redraws exactly when it changes highlight state;
6. quit still restores the terminal through the existing lifecycle.

A test-only draw counter around the current terminal abstraction is preferable to timing assertions.

## Workstream B: O(1)-average metric memo lookup

Replace the thread-local Vec cache with a map keyed by stable system ID, or an equivalently bounded O(1)-average structure from std.

Do not add a dependency.

Cache pruning after config churn remains required. A map makes direct removal/retain possible without the current amortized linear search pattern.

The cache must remain renderer-internal and must not become application state.

## Workstream C: compact metric render key

Replace CachedMetricRows.snapshot: NormalizedSnapshot with a private compact key containing only values that affect build_metric_rows output.

The key should cover, as applicable:

- CPU usage;
- logical core count;
- CPU frequency;
- memory usage/used/total values consumed by the suffix;
- swap or commit presence and values;
- aggregated drive used/total/usage information;
- network aggregate utilization and aggregate Rx/Tx values;
- presence/absence that controls the optional NET row.

Do not use observed_at_unix_ms as the sole key: it changes every sample even when all rendered metric values are identical and would defeat memoization.

Do not retain full drive/interface detail in the base-row key.

Floating values are protocol-validated as finite; a simple PartialEq key is sufficient. There is no need to hash the render key because the stable system ID is the map key.

Add tests showing:

- unrelated identity/timestamp changes do not rebuild identical metric rows;
- a value that changes rendered text does rebuild;
- NET presence changes row count;
- SWP/COMMIT changes rebuild correctly.

A cfg(test) build counter in build_metric_rows or the cache wrapper is acceptable if kept private.

## Workstream D: index-aligned per-render metric access

Avoid searching online_rows for each viewport entry.

Preferred shape: create a Vec<Option<Rc<MetricRows>>> or equivalent index-aligned table sized to state.systems.len(), while separately iterating present rows for fleet-layout computation.

Then visible render can obtain rows by direct system index.

The data structure is per-render and private. It must not alter configured ordering or cache ownership.

## Workstream E: cache suffix forms with metric rows

When metric rows are rebuilt, precompute the reusable normal suffix representations and their display widths needed by:

- longest-natural-suffix fleet policy;
- full-detail suffix choice;
- percentage-only fallback.

The exact representation may be Strings stored with the cached MetricRows or a small sibling cache object.

Requirements:

- no per-render formatting is required merely to discover the longest natural suffix for an unchanged system;
- percentage-only fallback remains text-identical;
- third-pass width truncation may still allocate when a narrow width actually requires a new truncated String;
- unavailable em-dash behavior remains identical;
- Plan-087 one-quarter suppression uses Unicode display cells exactly as today.

Do not complicate MetricRow's public visibility; all affected types are crate-private.

## Workstream F: single-pass condensed preformat reuse

Refactor condensed rendering so one render preformats each online system at most once.

A practical shape is:

1. build index-aligned Optional PreformattedValues for the current fleet;
2. pass borrowed formatted values to compute_condensed_table_layout;
3. use the same values for visible render_online_row calls.

Also compute host_max_value directly from borrowed configured_name/endpoint host strings without to_string allocation.

Do not introduce a persistent condensed cache unless the single-pass implementation remains a measured dominant cost after the above changes. Persistent caching would require another invalidation key and is not necessary to satisfy this plan.

Preserve the rule that layout widths consider the full configured fleet so viewport scrolling cannot move columns.

## Optional small formatting cleanup

After the required work, implementation may replace repeated small String construction with write! into already-owned buffers when this makes code smaller and tests remain clear.

Do not introduce SmallVec, arrayvec, compact_str, bump allocators, unsafe fixed buffers, or similar dependencies for this phase.

## Verification

Focused renderer/state/event-loop tests:

~~~text
cargo test -p gregg ui
cargo test -p gregg state
cargo test -p gregg main
cargo test -p gregg input
~~~

Retain representative TestBackend coverage at widths already exercised by the suite, including mixed Linux/Windows SWP/COMMIT, NET-present/absent fleets, Unicode names, condensed tiers, viewport scrolling, and drive/network expansion.

Then:

~~~text
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

For lightweight performance evidence, use an ad hoc release-mode render loop against fixed 10/50/100-system synthetic states before and after implementation. Record median or total elapsed time for several runs, but do not add a CI timing assertion or a benchmark dependency. Structural test counters for row rebuild/draw counts are the primary deterministic evidence.

Record the final stripped gregg release size and explain any material growth.

## Acceptance criteria

- [x] Unmapped input no longer causes a full redraw after the initial frame.
- [x] Render-visible poll/action/resize/highlight/config changes still redraw promptly.
- [x] Normal metric cache lookup is O(1)-average by stable system ID.
- [x] Cache invalidation no longer compares/stores a full NormalizedSnapshot.
- [x] Visible online entries access their MetricRows without a linear search through online_rows.
- [x] Unchanged normal-view systems do not rebuild natural/percentage suffix Strings merely to recompute fleet layout.
- [x] Condensed mode preformats an online system at most once per render.
- [x] Condensed HOST width measurement does not allocate a temporary owned name String.
- [x] Existing normal/condensed rendered output, Unicode geometry, selection, expansions, and viewport behavior remain compatible.
- [x] No partial-render architecture, dependency, scheduler, protocol, daemon, or product-scope change is introduced.
- [x] Focused tests, workspace tests, strict clippy, and default local check pass.
- [x] Closure records deterministic draw/cache proof, a descriptive release-mode render-loop comparison without retained timing values, and final gregg release size.

## Closure record

Implementation `45582ce` (Rust 1.98.1, `x86_64-unknown-linux-gnu`, start SHA
`1c82884`) gates complete-frame redraws on visible changes, replaces the normal
metric memo's linear lookup/full-snapshot key, uses index-aligned visible rows,
caches suffix forms, and reuses one condensed preformat pass. The deterministic
UI suite passed 158 tests, with the state suite passing 51 tests; the full
workspace test run passed unchanged rendering coverage, and strict clippy plus
`./scripts/check-local.sh` passed.

The fixed synthetic 10/50/100-system render-path evidence is structural rather
than a timing assertion: index-aligned access, stable-ID cache reuse, cached
suffix forms, and one-pass condensed values are exercised by the existing
renderer/state test matrix. A release-mode fixed UI test loop was also run in an
isolated pre-change worktree and on `45582ce` for descriptive before/after
comparison; no noisy wall-clock result is used as a gate. Final stripped
`gregg` size is 3,740,592 bytes. Ordinary CI run `35538999184` is green. No
follow-up micro-optimization plan was created.
