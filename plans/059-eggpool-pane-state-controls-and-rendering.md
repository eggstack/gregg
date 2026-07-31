# Phase 59: EggPool pane state, controls, and rendering

Status: planned.

## Objective

Add the top-level Systems/EggPool pane model, context-sensitive keyboard behavior, period-aware EggPool state, and a compact Ratatui renderer while preserving existing system selection, scrolling, normal/condensed layouts, and drive expansion.

This phase ends when:

- the available pane set is derived from configuration;
- `h`/Left and `l`/Right cycle only available top-level panes;
- `j`/Down and `k`/Up retain system selection on Systems and change the EggPool period on EggPool;
- Normal/Condensed system presentation remains available through one replacement key;
- EggPool pending, online, stale/error, and unavailable states render compactly and truthfully;
- rendering remains pure and consumes synthetic state without performing I/O.

HTTP worker/event-loop wiring is completed in Phase 60.

## Dependencies and execution position

Depends on:

- Phase 57 optional EggPool configuration;
- the existing Phase 51/52 viewport and normal/condensed system-layout implementation;
- Phase 58's period, summary, outcome, and result types for final compilation, although synthetic equivalents may be used during parallel work.

Must complete before Phase 60 integration closure.

## Governing invariants

1. `Pane` and `SystemViewMode` are separate state dimensions.
2. Systems are the initial pane when at least one system exists.
3. EggPool is the initial pane only when it is configured and no systems exist.
4. No EggPool config means no EggPool pane can be selected.
5. `h`/Left and `l`/Right operate on top-level panes only.
6. `j`/Down and `k`/Up remain context-sensitive but deterministic.
7. Normal/Condensed remains transient, selection-preserving, and system-only.
8. Drive expansion remains transient and system-only.
9. Period defaults to 1 hour and clamps at 1 hour/30 days.
10. Changing period cannot display a previous period's summary as current.
11. Rendering performs no network, environment, filesystem, or configuration writes.
12. Existing system viewport/entry-height behavior remains authoritative inside Systems.
13. No generic screen stack, focus framework, keymap, widget registry, or dashboard abstraction is introduced.
14. No new dependency is required.

## Scope

### In scope

- `Pane` enum and available-pane derivation;
- renaming/separating existing view state as `SystemViewMode` or equivalent;
- pane navigation actions;
- system-layout toggle action/key;
- context-sensitive vertical actions;
- EggPool period and request-status state;
- result application/stale-result rejection;
- compact EggPool renderer;
- wide/narrow/error/pending/stale buffer tests;
- contextual key hints and empty-config diagnostic text.

### Out of scope

- HTTP client/worker implementation beyond synthetic result application;
- generalized focus/input modes;
- multiple EggPool endpoints;
- multiple metric pages inside EggPool;
- charts, tables, history, scrolling metrics, drill-down, or custom layout;
- persistent pane/layout/period state;
- configurable keybindings;
- mouse input;
- changes to greggd/protocol/EggPool;
- new CI/release/evidence infrastructure.

## Workstream A: separate pane and system-layout state

Replace the overloaded current concept with two direct enums:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Systems,
    Eggpool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemViewMode {
    Normal,
    Condensed,
}
```

`AppState` should contain:

```rust
pub active_pane: Pane,
pub system_view_mode: SystemViewMode,
pub eggpool: Option<EggpoolState>,
```

A minimal `EggpoolState`:

```rust
pub struct EggpoolState {
    pub period: EggpoolPeriod,
    pub request_generation: u64,
    pub status: EggpoolStatus,
    pub summary: Option<EggpoolSummary>,
    pub last_success_at: Option<Instant>,
    pub last_attempt_at: Option<Instant>,
    pub last_error: Option<EggpoolFetchOutcome>,
}
```

`EggpoolStatus` may be:

```rust
Idle
Refreshing
```

or another equally small representation. Do not duplicate outcome information unnecessarily.

Construction from config:

```text
systems nonempty                    -> active Systems
systems empty + eggpool configured -> active EggPool
neither                            -> active value may remain Systems internally, but renderer shows empty config
```

When EggPool is absent, `eggpool = None`.

### Workstream A acceptance criteria

- [ ] Pane and system layout are independent.
- [ ] Existing Normal/Condensed state remains available.
- [ ] Initial pane follows configured-source rules.
- [ ] EggPool state is absent when unconfigured.
- [ ] No dynamic pane collection or registry is introduced.

## Workstream B: define available-pane cycling

Add actions:

```rust
PreviousPane
NextPane
ToggleSystemView
```

The available pane order is fixed:

```text
Systems, EggPool
```

but only configured panes participate.

Behavior:

```text
systems only       -> PreviousPane/NextPane remain Systems
eggpool only       -> PreviousPane/NextPane remain EggPool
both, on Systems   -> previous/next -> EggPool
both, on EggPool   -> previous/next -> Systems
```

With two available panes, both directions reach the other pane. Keep directional action names for clarity; implement with direct matches/availability checks, not a generic vector registry.

Switching pane:

- preserves selected system, viewport, system layout, and drive-expansion state;
- preserves current EggPool period and same-period last successful summary;
- does not itself mutate configuration;
- allows Phase 60 to observe activation/deactivation and command the worker.

### Workstream B acceptance criteria

- [ ] Only configured panes are selectable.
- [ ] One-pane configurations do not expose blank panes.
- [ ] Two-pane cycling is deterministic in both directions.
- [ ] Pane changes preserve each pane's transient state.
- [ ] No back-stack, tabs collection, or focus graph is added.

## Workstream C: update key-to-action translation without embedding state

Raw key translation should remain state-independent where practical. Map:

```text
h / Left    -> PreviousPane
l / Right   -> NextPane
j / Down    -> MoveDown
k / Up      -> MoveUp
v           -> ToggleSystemView
```

Then `AppState::apply_action` interprets `MoveDown`/`MoveUp` by active pane:

```text
Systems:
    MoveDown -> select next system
    MoveUp   -> select previous system

EggPool:
    MoveDown -> period.longer()
    MoveUp   -> period.shorter()
```

This is preferable to letting the event translator inspect mutable state and avoids introducing modal input layers.

Existing controls remain:

```text
e           toggle selected-system drives (Systems only)
g / G       first/last system (Systems only)
f / b       page down/up (Systems only)
PageDown/Up system paging (Systems only)
Ctrl-R      refresh active pane (runtime-specific handling in Phase 60)
q / Esc     quit
```

When a system-only action occurs on EggPool, it should be a no-op. Do not reinterpret `e`, `g`, `G`, `f`, or `b` for new EggPool functions.

Modifier rules remain unchanged: plain keys only, Ctrl/Alt variants not accidentally mapped except existing Ctrl-C/Ctrl-R.

Required tests:

- all new key mappings;
- old key mappings still parse;
- modifier rejection;
- context-specific reducer behavior;
- system-only actions no-op on EggPool;
- `v` no-op on EggPool and toggles system layout on Systems.

### Workstream C acceptance criteria

- [ ] Event translation remains pure/state-independent.
- [ ] `j`/`k` semantics are selected by reducer state.
- [ ] Existing system navigation remains unchanged on Systems.
- [ ] No modal keymap framework is added.

## Workstream D: preserve system-layout behavior under `v`

Move current Normal/Condensed transitions from `PreviousView`/`NextView` to `ToggleSystemView`:

```text
Normal    + ToggleSystemView -> Condensed
Condensed + ToggleSystemView -> Normal
```

Requirements:

- selection remains stable;
- viewport correction runs after geometry changes;
- drive expansion remains stable;
- all existing normal/condensed buffer tests continue to pass after mechanical action/key updates;
- public docs and hints stop advertising `h/l:view` and advertise `v:view` or `v:layout` instead.

Do not add separate previous/next system-layout keys. Two modes require only one toggle.

### Workstream D acceptance criteria

- [ ] Both system layouts remain reachable.
- [ ] No system renderer semantics change merely because the key changed.
- [ ] Selection/viewport/drive expansion are preserved.
- [ ] Existing h/l system-layout assumptions are fully removed from active docs/tests.

## Workstream E: apply period movement and request-state transitions

When active pane is EggPool:

```text
MoveDown -> longer period
MoveUp   -> shorter period
```

If movement changes the period:

- increment/advance desired request generation or mark a new request desired;
- clear the visible summary if it belongs to the old period;
- set status to refreshing/pending as appropriate once Phase 60 dispatches;
- preserve no old-period metric values under the new period label;
- allow Phase 60 to send an immediate `SetPeriod` command.

If movement clamps at an endpoint, no new request should be required.

Result application should accept only the current/latest request identity:

```rust
pub fn apply_eggpool_result(&mut self, result: &EggpoolResult)
```

Rules:

- ignore results when EggPool is no longer configured;
- ignore generations older than current desired/applied generation;
- ignore period-mismatched results;
- `Online` stores summary, clears error, records attempt/success time, sets idle;
- first failure stores error with no summary;
- same-period later failure retains previous summary, stores error and last attempt, sets idle/stale presentation;
- `Cancelled` does not overwrite success or display an error;
- no outcome can change active pane.

Required reducer tests:

- initial 1-hour period;
- all movement/clamp cases;
- no request intent on clamped movement;
- success;
- first failure;
- later same-period failure retains summary;
- period change hides old summary;
- stale generation ignored;
- mismatched period ignored;
- cancelled ignored;
- config reload adding/removing EggPool reconciles pane safely.

### Workstream E acceptance criteria

- [ ] Period movement is exact and bounded.
- [ ] Old-period values never display under a new period.
- [ ] Same-period last success survives transient refresh failure.
- [ ] Stale/cancelled results cannot regress state.
- [ ] No four-period cache or history is added.

## Workstream F: reconcile configuration reload behavior

Existing `ConfigReloaded(Config)` rebuilds systems. Extend it narrowly:

```text
EggPool absent -> present
    create default 1-hour EggPool state
    preserve active Systems when systems exist
    if no systems, activate EggPool

EggPool present -> absent
    drop EggPool state
    if active EggPool and systems exist, activate Systems
    if active EggPool and no systems, renderer shows empty config

EggPool present -> changed
    reset request generation/status/summary because endpoint/auth identity changed
    preserve selected period unless simpler/safer to reset to 1 hour; choose and test one policy
```

Preferred changed-entry policy: preserve the selected period but clear summary/error and request an immediate fetch when active. This respects the user's current window without showing old endpoint data.

System rebuilding remains unchanged.

### Workstream F acceptance criteria

- [ ] Add/remove/change reloads cannot leave an unavailable active pane.
- [ ] Old endpoint summary is cleared on endpoint change.
- [ ] System selection/viewport preservation remains unchanged.
- [ ] No file watcher or reload mechanism expansion is introduced.

## Workstream G: create one compact EggPool renderer

Create:

```text
crates/gregg/src/ui/eggpool.rs
```

The renderer consumes an area, the configured endpoint/display name if needed, and `EggpoolState`. It performs no I/O.

### Header

Display:

```text
EggPool — <name-or-host>    Window: <period display label>
```

At narrow widths:

- preserve `EggPool`, identity, and period before low-priority status text;
- truncate identity by Unicode display width;
- do not horizontally scroll.

### Metrics

Render exactly four values:

```text
Accounted tokens
Cache read share
Output tok/s
Avg TTFT
```

Formatting:

- accounted tokens use compact SI-style count formatting (`12.5K`, `8.2M`, `1.1B`) or existing count helper if available;
- cache read share is percentage with one decimal, or `—`;
- output tok/s uses one decimal at ordinary values and bounded compact formatting at large values;
- TTFT uses ms below a sensible threshold and optional seconds formatting only if an existing duration helper already supports it; otherwise always bounded ms is acceptable;
- avoid scientific notation for normal operator values;
- formatting functions handle maximum `u64` and large finite `f64` without panic/overflow.

Wide layout may use a 2x2 grid. Narrow layout stacks one metric per row. Use direct Ratatui paragraphs/lines and simple rectangles, not a reusable card framework.

### Workstream G acceptance criteria

- [ ] Exactly four requested metrics render.
- [ ] Labels match EggPool semantics.
- [ ] Wide and narrow layouts are deterministic.
- [ ] Null/no-sample values render `—`.
- [ ] Large values and Unicode identity do not panic or corrupt alignment.
- [ ] No chart/table/card framework is introduced.

## Workstream H: render pending, failure, and stale states

Required states:

### Never requested / first activation pending

```text
EggPool — Main EggPool    Window: 1 hour
Loading summary…
```

### First request failure, no summary

Show one bounded actionable line plus normal footer hint. Map outcomes to text without raw errors:

```text
MissingApiKeyEnv -> API key environment variable <name> is not set
Unauthorized     -> authentication required or key rejected
Forbidden        -> access forbidden
StatsUnavailable -> stats unavailable — enable EggPool dashboard/statistics routes
Timeout          -> request timed out
ConnectionRefused-> connection refused
DnsFailure       -> DNS lookup failed
NetworkError     -> network error
HttpStatus(n)    -> HTTP <n>
BodyTooLarge     -> response too large
DecodeError      -> invalid JSON response
InvalidSummary   -> invalid summary response
```

### Same-period refresh pending with prior success

Continue showing prior metrics and add a small `refreshing` marker.

### Same-period refresh failure with prior success

Continue showing prior metrics and add bounded stale/error text such as:

```text
Updated 17:42:18 — refresh failed: request timed out
```

Do not call data stale solely because 60 seconds elapsed; stale presentation here means the latest refresh attempt failed while prior values are retained.

### Workstream H acceptance criteria

- [ ] First-load pending/failure is distinguishable from zero metrics.
- [ ] Prior values remain visible during same-period refresh.
- [ ] Failed refresh retains prior values with an explicit warning.
- [ ] No raw body/error/secret is rendered.
- [ ] All outcome variants have bounded text.

## Workstream I: route top-level UI and diagnostics

Update `ui::render`:

```rust
if no systems and no eggpool {
    render_empty_config(...)
    return;
}

match state.active_pane {
    Pane::Systems => render existing system view,
    Pane::Eggpool => eggpool::render(...),
}
```

Requirements:

- EggPool-only config must not trigger `No systems configured`;
- systems-only output remains unchanged except key hints and pane indicator as needed;
- too-small rules are pane-specific;
- EggPool pane uses a sensible low minimum, e.g. enough for header, four stacked metrics, and optional hint;
- no base system block is rendered while EggPool is active;
- no system layout/header geometry contaminates EggPool layout.

Add a compact pane indicator only when both panes exist, e.g.:

```text
Systems 1/2
EggPool 2/2
```

Do not consume a permanent row if the current header can contain it without harming narrow layouts. The exact placement should be pinned by buffer tests.

Update empty diagnostic:

```text
No sources configured. Use: gregg add HOST[:PORT] or gregg eggpool add HOST[:PORT]
```

Contextual hints:

Systems wide:

```text
j/k:select  h/l:pane  v:view  e:drives  Ctrl-R:refresh  q:quit
```

EggPool wide:

```text
h/l:pane  j/k:window  Ctrl-R:refresh  q:quit
```

When only one pane exists, h/l may be omitted from the hint to avoid advertising a no-op.

### Workstream I acceptance criteria

- [ ] Empty-source detection includes both source types.
- [ ] EggPool-only config renders EggPool.
- [ ] Systems-only buffers remain semantically unchanged.
- [ ] Hints are pane- and availability-aware.
- [ ] No hint overwrites content.

## Workstream J: focused state and Ratatui tests

Required state tests:

- systems-only initial state;
- EggPool-only initial state;
- both sources initial Systems;
- pane cycling for each availability combination;
- state preservation across pane switches;
- context-sensitive vertical movement;
- `v` behavior;
- system-only action no-ops on EggPool;
- period/result/reload rules from Workstreams E/F.

Required buffer tests:

- systems-only normal and condensed regression;
- EggPool-only pending;
- EggPool success wide;
- EggPool success narrow;
- null cache/no TTFT samples;
- first-load 401/404/missing env/network/decode errors;
- prior success plus refreshing;
- prior success plus failed refresh;
- very large count/rate/TTFT formatting;
- long ASCII and Unicode identity;
- terminal too small;
- both-pane indicator/hints;
- no secret-shaped string in rendered buffers.

Use synthetic state only. Do not run a server from UI tests.

### Workstream J acceptance criteria

- [ ] Reducer and renderer behavior is deterministic without I/O.
- [ ] Existing system regressions remain covered.
- [ ] Every operator-visible EggPool state has a buffer test.
- [ ] No screenshot/golden-file evidence system is added unless the repository already uses compact inline expected buffers; prefer existing test style.

## Expected files

Likely files:

```text
crates/gregg/src/action.rs
crates/gregg/src/event.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/eggpool.rs
crates/gregg/src/ui/diagnostics.rs
crates/gregg/src/ui/text.rs
crates/gregg/src/ui tests
```

Do not change the HTTP worker except shared type imports or test constructors.

## Implementation sequence

1. Add pane/layout enums and construction tests.
2. Replace raw `SelectNext/Previous` input with context-neutral movement actions where needed.
3. Map h/l pane controls and v system-layout toggle.
4. Implement direct pane availability/cycling.
5. Add period and result reducer behavior.
6. Add config-reload reconciliation.
7. Create simple EggPool formatting helpers.
8. Render pending and successful wide/narrow states.
9. Add failure/stale states.
10. Route `ui::render`, diagnostics, and contextual hints.
11. Re-run all existing system state/UI tests and inspect for behavior changes beyond controls/hints.
12. Inspect the diff for generic screen/widget/input abstractions and remove them.

## Required verification

Focused checks:

```text
cargo fmt --all -- --check
cargo test -p gregg action event state --all-features
cargo test -p gregg ui --all-features
cargo test -p gregg --all-targets --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

Do not add a browser/screenshot test or live EggPool dependency.

## Phase acceptance criteria

Phase 59 is complete only when:

- [ ] `Pane` is separate from `SystemViewMode`.
- [ ] Available panes are derived from configured systems/EggPool.
- [ ] Systems is initial when systems exist; EggPool is initial for EggPool-only config.
- [ ] `h`/Left and `l`/Right cycle only available top-level panes.
- [ ] `j`/Down and `k`/Up select systems on Systems and change periods on EggPool.
- [ ] Period defaults to 1 hour and clamps at 1 hour/30 days.
- [ ] Normal/Condensed remains reachable with `v` and preserves selection/viewport/drive expansion.
- [ ] System-only actions do not acquire unrelated EggPool meanings.
- [ ] Old-period summaries are hidden after period change.
- [ ] Same-period last success survives a failed refresh.
- [ ] Stale generation, mismatched period, and cancellation cannot regress state.
- [ ] EggPool-only config renders without the old empty-systems diagnostic.
- [ ] Exactly four accurately labeled metrics render in wide/narrow layouts.
- [ ] Null/no-sample values render `—`.
- [ ] Pending, first failure, refreshing, and retained-stale states are distinct and nonfatal.
- [ ] Hints are truthful for active/available panes.
- [ ] Existing normal/condensed system rendering and viewport tests pass.
- [ ] No I/O in rendering, generalized screen/keymap/widget framework, persistence, extra page, new dependency, or new infrastructure was added.

## Handoff guidance for a smaller implementation model

- Add two enums and direct matches; do not build a screen registry.
- Keep input translation state-neutral and make the reducer context-sensitive.
- Preserve all existing system state fields when switching panes.
- Use `v` as one direct Normal/Condensed toggle.
- Render four plain values with a wide and narrow branch.
- Keep last-success retention only for the current period.
- Do not perform any network/environment/config work in UI modules.
