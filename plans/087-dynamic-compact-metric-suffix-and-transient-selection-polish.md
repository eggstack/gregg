# Phase 087: Dynamic compact metric suffix and transient selection polish

Status: ready for implementation.

Depends on: completed Plans 085 and 086, current `main` after Plan 086 record commit `a0aa28097617d880404aaa88850df767d028d66e`.

## Objective

Make Gregg's Systems TUI behave cleanly as a dynamically resizable compact surface in narrow terminal-multiplexer panes without disturbing the fleet-wide geometry and storage corrections completed in Plans 085/086.

This phase has three bounded client-side changes:

1. dynamically suppress normal-view metric suffix text after `]` when the fleet's longest natural suffix would consume more than one quarter of the current terminal width, leaving the aligned bars themselves as the compact representation;
2. separate persistent logical system selection from transient visual selection highlighting so Gregg starts visually unselected, shows the selected system while the operator is actively navigating, and removes only the highlight after approximately 10 seconds of inactivity while retaining `e` drive-detail behavior;
3. omit the normal-header `IO` field entirely when CPU I/O wait is unsupported or no real I/O-wait value is available, instead of rendering `IO —`/`IO -`.

This is a visual/client-state polish pass. Do not reopen the daemon, protocol, metric collectors, normalized snapshot schema, drive-capacity semantics, endpoint/configuration behavior, scheduler architecture, release process, or CI design.

## Baseline findings

### 1. Normal-view suffix degradation does not yet implement the desired compact-pane policy

`crates/gregg/src/ui/system_block.rs` currently builds each metric's natural suffix, computes one fleet-wide `MetricFleetLayout`, and degrades suffixes from full detail to percentage-only and finally truncation when the available suffix budget is too small.

That preserves width safety, but it still spends narrow-pane width on values after the closing `]`. The desired compact behavior is intentionally stronger: when the longest natural detail region would itself exceed one quarter of the current terminal width, the entire normal-view suffix region should disappear so the metric rows become bar-only.

The decision must be fleet-wide, not per-row or per-device. Plan 085/086 deliberately made metric geometry fleet-wide so `[` and `]` do not stagger between systems or reflow when scrolling. A single long DISK or memory suffix must therefore cause the same bar-only mode for every participating normal-view metric row in that render.

The width comparison must use terminal display cells (`unicode-width`), not UTF-8 byte length.

### 2. Logical selection and selection styling are currently conflated

`AppState.selected_id` is persistent application state. It controls navigation and identifies the system on which `ToggleDrives`/`e` operates.

`ui/layout.rs` currently derives `ViewportEntry.is_selected` from that same `selected_id`, and both normal and condensed renderers use the boolean directly to apply `Modifier::REVERSED`. Drive-detail visibility also depends on the selected system.

Clearing `selected_id` after a timeout would therefore be incorrect: it would remove both styling and the persistent target needed for `e` and viewport behavior.

The requested behavior needs two concepts:

```text
logical selection: persistent selected system ID
visual selection: temporary highlight visibility
```

Gregg should continue to have a deterministic logical selection at startup, but no system should be visually reversed until the operator navigates. After navigation, visual highlighting should remain for about 10 seconds and then disappear while the logical selection remains unchanged.

### 3. A render-only timestamp check is insufficient for highlight expiry

The TUI event loop redraws after actual events: poll batches, optional EggPool results, terminal/input events, configuration-related actions, and similar state transitions. It does not run a fixed frame ticker.

Therefore checking `Instant::now()` only inside `ui::render()` would not guarantee a ten-second visual expiry. If no redraw-triggering event occurs at the deadline, the stale highlight can remain on screen until the next unrelated event.

The event loop needs one bounded resettable timeout/deadline that triggers a redraw at expiration. Do not add a general animation/frame loop.

### 4. Normal headers fabricate an unsupported I/O-wait placeholder

The normalized snapshot already carries truthful capability/value state:

```rust
cpu_iowait_supported: bool
iowait_pct: Option<f32>
```

`ui/text.rs::header_line()` currently renders an `IO —` placeholder when I/O wait is unsupported or absent. This is wasted horizontal space in exactly the small-pane use case targeted by this phase.

No daemon or protocol change is necessary. Header composition should include an `IO <value>%` component only when a real value is present and the capability says it is supported.

The condensed fleet table is a separate presentation. Its explicit `IOWAIT` column and unavailable `—` cell are not part of this requirement and should remain unchanged unless implementation proves a direct regression.

## Authoritative behavior after Plan 087

### Normal metric suffix policy

For every normal-view render, compute the longest **natural/default suffix** across every metric row of every online system with a current snapshot.

The threshold is strict:

```text
hide suffixes when:

    longest_suffix_display_width > terminal_width / 4
```

Prefer an integer-safe comparison that avoids division-rounding ambiguity:

```text
longest_suffix_display_width * 4 > terminal_width
```

Use saturating/checked conversion where appropriate. Width is in terminal display cells.

If the threshold is exceeded:

- suppress all text after the closing `]` for normal metric rows in the current render;
- preserve one common fleet-wide label width and bar width;
- preserve aligned opening `[` and closing `]` columns across all devices;
- do not leave a trailing suffix separator after `]`;
- allow the cell that would otherwise be the `] ` separator space to return to the bar budget where the current geometry permits it;
- keep unavailable metric bars truthful according to existing behavior;
- do not alter expanded drive-detail rows.

Representative compact output:

```text
host1
    CPU  [|||||             ]
    MEM  [|||||||||||       ]
    SWP  [                  ]
    DISK [|||||||||||||||   ]
```

If the threshold is not exceeded, retain the existing Plan 085/086 suffix behavior and degradation order:

```text
natural detail -> percentage-only -> width-safe truncation
```

Do not replace that existing fallback machinery.

The mode must react immediately to terminal resize because fleet layout is recomputed from the current `Frame` width each render. Do not persist a separate compact-mode setting in configuration or `AppState`.

### Logical versus visual selection

`selected_id` remains the persistent logical selected system.

Startup behavior:

- if systems exist, retain the existing deterministic logical selection behavior;
- no system is visually highlighted solely because it is logically selected;
- viewport placement and first-poll reachability ordering remain as currently implemented.

Navigation behavior:

- `j` / Down and `k` / Up update logical selection exactly as today;
- PageUp/PageDown and `g`/`G`, which are also explicit system-selection actions, should use the same visual-highlight behavior unless there is a concrete reason in the current action reducer not to do so;
- after a selection-changing action, the selected system becomes visually highlighted;
- each subsequent selection-changing action resets the visual timeout;
- the visual highlight expires approximately 10 seconds after the most recent selection-changing action;
- expiration clears only the visual-highlight state, not `selected_id`;
- polling, resize, refresh, and ordinary background events do not extend the selection timeout;
- `e` continues to expand/collapse drives for the logically selected system even after the visual highlight has expired;
- expanded-drive ownership remains tied to logical selection and must not disappear merely because the reverse-video highlight times out.

If switching away from the Systems pane while a highlight is active, clear or suspend the visual highlight rather than allowing a stale reversed row to reappear later. Keep this implementation small; do not introduce per-pane focus history.

### Timeout/event-loop behavior

Implement one resettable deadline associated with the visual selection highlight.

Preferred architecture:

```text
AppState / reducer:
    owns whether selection highlight is visually active

run_event_loop:
    owns the transient deadline/timer that causes expiry to be dispatched
```

A small internal action such as `ClearSelectionHighlight` is acceptable and preferred if it preserves the existing typed-action state-transition model.

Do not:

- add a periodic frame ticker;
- poll time on every render without a wakeup mechanism;
- spawn one independent task per keypress;
- add a new runtime/dependency;
- clear logical selection at timeout.

A resettable `tokio::time::Sleep`, deadline future, or equivalent bounded event-loop branch is sufficient.

### Normal-header I/O wait

For the normal system header:

- render `IO <n.n>%` only when `cpu_iowait_supported == true` and `iowait_pct == Some(value)`;
- render no `IO` token at all when unsupported or missing;
- preserve the existing priority-aware width degradation of the remaining header fields;
- avoid doubled separators/spaces when the optional IO component is absent;
- do not fabricate `0.0%`, `—`, `-`, or `--` for unavailable I/O wait.

Examples:

```text
linux1  IO 1.7%  0.42/0.36/0.31  8c
mac1  0.42/0.36/0.31  10c
```

The exact fields retained at each terminal width remain controlled by the existing header priority behavior.

## Implementation sequence

### Step 1: add compact-suffix boundary tests around `MetricFleetLayout`

Primary file:

```text
crates/gregg/src/ui/system_block.rs
```

Before changing production behavior, add focused tests for the threshold calculation.

Required cases:

1. longest natural suffix width exactly one quarter of terminal width -> suffixes remain enabled;
2. longest natural suffix width one display cell above the one-quarter boundary -> suffixes are disabled;
3. one long suffix on one system causes fleet-wide suppression for all systems;
4. off-viewport systems still participate because fleet geometry is computed from the complete online fleet;
5. Unicode suffix content, if a suitable fixture can be introduced without artificial production changes, is measured in display cells rather than bytes; otherwise cover the width helper directly with existing Unicode-aware utilities.

Do not encode the compact policy separately in tests and production with two divergent formulas. Prefer a small helper whose returned mode/bool is directly asserted.

### Step 2: extend `MetricFleetLayout` with the bar-only suffix decision

Primary file:

```text
crates/gregg/src/ui/system_block.rs
```

Add the smallest state needed to carry the fleet decision, for example:

```rust
pub(crate) struct MetricFleetLayout {
    pub label_width: u16,
    pub bar_width: u16,
    pub show_suffix: bool,
}
```

An enum is unnecessary unless implementation finds more than two actual states.

Compute the decision from the widest natural/default suffix before existing budget-driven suffix degradation. The threshold should describe the content the user is trying to hide, not the already-truncated result of the current resolver.

When `show_suffix == false`:

- compute bar width against `]` as the row terminator rather than reserving `] ` plus suffix width;
- do not invoke percentage-only/truncation fallback for visible output because no suffix is rendered;
- keep the same fleet-wide `label_width` and one common `bar_width`.

When `show_suffix == true`, preserve Plan 086's corrected fleet-prefix suffix resolver unchanged except for signatures needed to carry the new layout flag.

### Step 3: make metric-row rendering emit a truly suffix-free compact shape

Primary file:

```text
crates/gregg/src/ui/system_block.rs
```

In bar-only mode, emit exactly the structural row required for the aligned bar:

```text
<label prefix> [<bar>]
```

Do not emit:

```text
<label prefix> [<bar>] 
```

with a trailing suffix separator.

Keep the current no-bar fallback width-safe for pathological widths, but do not broaden Plan 087 into redesigning the existing minimum-terminal diagnostic threshold.

Add renderer-level Ratatui `TestBackend` assertions that:

- suffix text is absent in compact mode;
- `[` and `]` columns remain aligned across CPU/MEM/SWP-or-COMMIT/DISK and across devices;
- final rendered line display width never exceeds terminal width;
- resizing to a wider width restores suffixes without changing application state.

### Step 4: introduce explicit visual-selection state without changing logical selection

Primary file:

```text
crates/gregg/src/state.rs
```

Add one reducer-owned visual state flag, using a name that cannot be confused with logical selection, for example:

```rust
pub selection_highlight_active: bool
```

Initialization must be `false`.

Do not change the existing `selected_id` initialization/reconciliation semantics merely to make startup look unselected. Logical selection remains available immediately for keyboard actions.

Add a narrow reducer action to clear the visual highlight if using the existing action architecture. Selection-changing actions should activate the flag. Non-selection actions should not refresh it.

Be careful with the optional EggPool pane: `j/k` there changes the time period, not the system selection, and must not activate a Systems-device highlight.

### Step 5: separate logical selection from rendering style

Primary files:

```text
crates/gregg/src/ui/layout.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/condensed.rs
```

Preserve `ViewportEntry.is_selected` as the logical-selection predicate if that is the least disruptive path, because drive-detail row allocation currently depends on it.

Derive the visual style separately, for example at the call site:

```text
is_visually_selected = entry.is_selected && state.selection_highlight_active
```

Pass that visual boolean to normal/condensed header-row rendering while retaining logical `entry.is_selected` for drive-detail visibility.

Required invariants:

- startup has no `REVERSED` system row;
- after `MoveDown`/`MoveUp`, exactly the logical selected system is reversed;
- when visual state clears, no system remains reversed;
- the logical selected ID is unchanged by visual expiry;
- `drives_expanded` rows remain attached to the logical selected system after visual expiry;
- offline/pending rows follow the same temporary highlight rule as online rows;
- condensed view uses the same temporary visual selection semantics.

Avoid duplicating selection state inside individual `SystemState` records.

### Step 6: add a resettable ten-second selection-highlight deadline to the event loop

Primary files:

```text
crates/gregg/src/main.rs
crates/gregg/src/action.rs
```

Use one event-loop-owned deadline. Recommended constant:

```rust
const SELECTION_HIGHLIGHT_DURATION: Duration = Duration::from_secs(10);
```

The precise location may be `main.rs`, `state.rs`, or a small UI-state helper, but do not turn the duration into user configuration in this phase.

When a Systems selection-changing action is successfully dispatched:

- set/retain `selection_highlight_active = true` through the reducer;
- reset the event-loop deadline to now + 10 seconds.

When the deadline fires:

- dispatch/apply the visual-clear action;
- clear/disarm the deadline;
- allow the ordinary bottom-of-loop `terminal.draw(...)` path to remove the reversed style immediately.

A new navigation action before expiry must replace/reset the deadline, not create concurrent expiration tasks.

If the active pane changes away from Systems, clear/disarm the highlight as part of the same bounded state transition or event-loop bookkeeping.

Tests should use Tokio's paused/advanced time where practical rather than a real ten-second sleep. Do not add slow wall-clock tests.

### Step 7: prove timeout semantics without breaking `e`

Primary files:

```text
crates/gregg/src/state.rs
crates/gregg/src/main.rs
crates/gregg/src/ui/mod.rs
```

Add deterministic coverage for:

1. startup: logical selection exists but visual highlight is false;
2. navigation: logical selection changes and visual highlight becomes true;
3. repeated navigation before expiry resets the deadline;
4. expiration: visual highlight becomes false while `selected_id` remains the same;
5. after expiration, `ToggleDrives` still toggles drive details for that selected system;
6. if drives were already expanded, expiry does not collapse them;
7. background poll batches before the deadline do not extend the deadline;
8. resize/refresh do not extend the deadline;
9. EggPool `j/k` period changes do not activate/reset the Systems selection highlight;
10. pane switch away from Systems does not leave a stale visual highlight to reappear later.

Do not test timeout behavior by sleeping ten real seconds.

### Step 8: omit unavailable normal-header I/O wait

Primary file:

```text
crates/gregg/src/ui/text.rs
```

Refactor `header_line()` so the IO component is optional instead of always materializing an unavailable placeholder.

Prefer composing header components or explicit branches that avoid separator artifacts. Do not introduce a generic formatting framework.

The value should be considered renderable only when both capability and value are truthful:

```text
cpu_iowait_supported && iowait_pct.is_some()
```

If capability says supported but the current value is `None`, omit IO rather than showing a placeholder. The UI should not infer a zero value.

### Step 9: update I/O-wait renderer tests

Primary files:

```text
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/mod.rs
```

The existing macOS renderer coverage explicitly expects `IO —`; invert that expectation.

Required cases:

- Linux/supported snapshot with a real value includes `IO <value>%`;
- macOS/unsupported snapshot contains no `IO` token;
- a v2 snapshot with `cpu_iowait_supported == true` but missing `iowait_pct` contains no `IO` token;
- removal does not leave leading/doubled separators;
- representative width tiers still stay within terminal width.

Do not change condensed-view `IOWAIT` behavior as part of these assertions.

### Step 10: run focused regression coverage for completed Plan 085/086 invariants

The new compact policy sits directly on top of the fleet layout, so rerun the existing focused tests that prove:

- fleet-wide `[` and `]` alignment across devices;
- mixed `SWP`/`COMMIT` label geometry;
- off-viewport systems participate in geometry;
- suffix budgeting uses fleet label width when suffixes are enabled;
- normal DISK remains `used / total`;
- expanded drive Full/Compact/Minimal width boundaries remain correct;
- condensed header/value geometry remains correct;
- offline/pending identity remains visible;
- Unicode display-width behavior remains correct.

Adjust assertions only where Plan 087 intentionally changes visible suffix or selection styling. Do not weaken unrelated Plan 085/086 coverage.

### Step 11: update the minimal active documentation surface

After behavior is implemented and verified, update only user/developer documentation that presently promises behavior contradicted by this phase.

Likely files:

```text
README.md
crates/gregg/README.md
architecture/gregg-client.md
.opencode/skills/gregg-client/SKILL.md
plans/087-dynamic-compact-metric-suffix-and-transient-selection-polish.md
plans/README.md
```

Document succinctly that:

- normal metric detail automatically disappears when the fleet's longest natural suffix exceeds one quarter of terminal width;
- resizing restores/removes suffixes dynamically;
- device selection highlighting is transient while logical selection remains active for `e`;
- unsupported normal-header I/O wait is omitted rather than represented by a placeholder.

Do not add configuration options or long design exposition to user-facing README files.

## Expected production-code surface

Primary:

```text
crates/gregg/src/ui/system_block.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src/main.rs
crates/gregg/src/action.rs
crates/gregg/src/ui/text.rs
```

Possible narrow supporting edits:

```text
crates/gregg/src/ui/layout.rs
crates/gregg/src/ui/condensed.rs
```

Expected documentation/planning reconciliation after implementation:

```text
README.md
crates/gregg/README.md
architecture/gregg-client.md
.opencode/skills/gregg-client/SKILL.md
plans/087-dynamic-compact-metric-suffix-and-transient-selection-polish.md
plans/README.md
```

No daemon/protocol files should need modification.

## Explicit non-goals

Do not add or redesign:

- daemon metric collection or sampling;
- protocol v1/v2 fields or capability semantics;
- system configuration schema;
- a configurable selection timeout;
- a manual compact-mode toggle;
- horizontal scrolling;
- mouse interaction;
- themes, colors, animation, fades, or interpolation;
- a continuous frame/tick loop;
- per-device independent metric bar geometry;
- a new table/layout framework;
- new dependencies;
- a new CI workflow, job, matrix, artifact, evidence bundle, or release gate;
- release automation;
- unrelated cleanup/refactors.

"Visually subside" in this plan means the existing selection reversal disappears at the timeout boundary. Do not implement a gradual color/fade animation, which would require a frame cadence and expand scope unnecessarily.

## Verification

Keep verification local-first and proportional to the change.

Focused checks should include the relevant targets, for example:

```bash
cargo fmt --all -- --check
cargo test -p gregg system_block
cargo test -p gregg state
cargo test -p gregg main
cargo test -p gregg ui
```

Use the actual test names/modules available after implementation rather than creating a harness merely to match these command examples.

Then run the repository's normal local gate:

```bash
./scripts/check-local.sh
```

A manual terminal smoke is appropriate because the core requirement concerns live resize behavior. On the available local Unix host, run Gregg against representative data and verify in a resizable terminal/tmux/zellij pane:

1. wide pane shows normal metric suffixes;
2. narrowing across the one-quarter threshold removes all normal metric suffixes while preserving aligned bars;
3. widening back restores suffixes without restart;
4. initial launch shows no device reversed;
5. `j/k` shows the selected device;
6. after approximately 10 seconds with no selection navigation, the reverse-video highlight disappears;
7. `e` still expands/collapses drives for the logically selected device after the highlight disappears;
8. unsupported-I/O-wait systems do not show `IO —`/`IO -` in the normal header.

Do not require release-mode preflight or a remote CI run for completion unless implementation unexpectedly touches platform-specific code outside the client-only scope. Existing CI may run normally after push, but no new CI requirement is created by Plan 087.

## Acceptance criteria

### Dynamic suffix suppression

- [ ] The compact threshold is based on the longest natural/default normal metric suffix across the entire online fleet, not on one row or the current viewport.
- [ ] Width is measured in terminal display cells.
- [ ] A suffix exactly one quarter of terminal width remains visible.
- [ ] A suffix greater than one quarter of terminal width activates fleet-wide bar-only normal metric rows.
- [ ] Bar-only rows contain no percentage/detail text and no trailing suffix separator after `]`.
- [ ] `[` and `]` remain aligned across metric rows and across devices in both suffix-enabled and bar-only modes.
- [ ] Off-viewport systems continue participating in the fleet decision/geometry.
- [ ] Resizing across the threshold dynamically suppresses/restores suffixes without restart or configuration change.
- [ ] Existing suffix degradation remains intact when suffixes are enabled.
- [ ] Expanded drive-detail formatting is unchanged by the normal metric suffix mode.

### Transient visual selection

- [ ] Gregg retains a deterministic logical `selected_id` at startup when systems exist.
- [ ] Startup renders no system with selection reversal solely because it is logically selected.
- [ ] Systems navigation activates visual highlighting for the logical selected system.
- [ ] Selection-changing navigation resets a single approximately ten-second deadline.
- [ ] Timeout expiry clears only visual highlighting; it does not clear/change `selected_id`.
- [ ] Timeout expiry triggers a redraw even when no poll/input event occurs at that moment.
- [ ] No periodic frame ticker or per-keypress background task is introduced.
- [ ] Poll, resize, refresh, and other non-selection events do not extend the timeout.
- [ ] EggPool period navigation does not activate a Systems-device highlight.
- [ ] Pane switching does not leave a stale Systems highlight that unexpectedly reappears.
- [ ] `e` continues to operate on the logical selected system after visual highlight expiry.
- [ ] Already-expanded drive details do not collapse solely because highlight visibility expires.
- [ ] Normal online, offline/pending, and condensed system rows share the same transient visual-selection semantics.

### I/O-wait omission

- [ ] A real supported I/O-wait value renders as `IO <value>%` in the normal header.
- [ ] Unsupported I/O wait renders no `IO` token or placeholder in the normal header.
- [ ] Supported capability with a missing current value also renders no fabricated placeholder/value.
- [ ] Header spacing remains clean when IO is omitted.
- [ ] Existing width-priority behavior for load, cores, OS, kernel, and architecture remains bounded.
- [ ] Condensed-view `IOWAIT` behavior is not unintentionally changed.

### Scope and regression

- [ ] Plan 085/086 fleet geometry and drive-table boundary tests continue to pass after intentional assertion updates.
- [ ] Normal DISK remains `percentage used / total` and expanded remaining-space semantics remain unchanged.
- [ ] No daemon, protocol, collector, normalized-capacity, scheduler, configuration, endpoint, or release architecture is redesigned.
- [ ] No new dependency or CI workflow/job/matrix is added.
- [ ] Focused tests and `./scripts/check-local.sh` pass.
- [ ] One local interactive resize/selection smoke demonstrates the intended dynamic behavior.
- [ ] Active documentation is updated narrowly after implementation.

## Handoff notes

Implement this as one bounded client polish phase. The highest-risk mistake is clearing or expiring `selected_id`; do not do that. The second highest-risk mistake is making suffix suppression a per-row/per-device decision, which would undermine the fleet-wide alignment work from Plans 085/086.

Prefer small helpers and explicit state over generalized abstractions. The desired result should reduce visual noise and improve narrow-pane utility with minimal additional runtime/state complexity.
