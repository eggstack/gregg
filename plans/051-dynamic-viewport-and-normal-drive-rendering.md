# Phase 51: dynamic viewport and normal drive rendering

Status: completed.

## Objective

Correct the existing multi-system viewport behavior and extend the normal TUI with aggregate and selected-system drive detail while keeping the renderer compact and state-driven.

The normal view already iterates over all visible systems. The core defect is that selection changes can move beyond the viewport because visibility correction is not consistently applied. Adding a fifth aggregate disk row and optional per-drive rows requires replacing hardcoded online-entry height assumptions with one dynamic height function shared by state paging, viewport computation, and rendering.

This phase does not add the condensed view or view-switching keys. Those are Phase 52.

## Dependencies and execution position

Depends on Phase 49 providing:

- normalized optional drive data;
- one aggregate helper;
- stable drive order;
- unavailable/empty semantics.

May use synthetic normalized drive fixtures before Phase 50 finishes native collection.

Must complete before Phase 52 because condensed expansion and view changes rely on dynamic row accounting and corrected viewport-following.

## Governing invariants

1. The normal view shows every system that fits, in existing display order.
2. `j`/Down and `k`/Up select logical systems, not raw rows.
3. The selected system is kept visible after every relevant action or batch-induced ordering change.
4. Paging, visible-range computation, layout, and rendering use the same entry-height rules.
5. Online normal entries use five base rows: header, CPU, memory, swap/commit, and disk.
6. Offline and pending entries remain one row.
7. Drive data unavailable or successfully empty does not fabricate zero use.
8. `e` expansion state may be introduced in state here for layout tests, but key binding is Phase 52.
9. Only the selected system gains per-drive detail rows when expansion is active.
10. Rendering remains pure and performs no I/O.
11. No border, panel framework, horizontal scrolling, or persistent UI preference is introduced.

## Scope

### In scope

- one `ViewMode` state enum containing at least `Normal` for forward compatibility with Phase 52;
- one `drives_expanded` state flag;
- dynamic state-aware entry-height calculation;
- viewport correction after selection, resize, config reload, poll batches, expansion changes, and future view changes;
- corrected page sizing;
- fifth aggregate disk row in normal online blocks;
- selected-system per-drive text rows;
- unavailable/empty disk rendering;
- key-hint text preparation for later Phase 52 controls if needed;
- focused reducer/layout/Ratatui tests;
- update of the documented four-row contract.

### Out of scope

- condensed renderer;
- `h`/`l`/Left/Right mappings;
- final `e` input mapping;
- mouse behavior;
- sorting/filtering/searching systems or drives;
- persistent expansion or view state;
- changes to polling order or online-first semantics;
- drive collection or wire changes;
- new UI frameworks or generalized widget abstractions.

## Workstream A: define minimal UI state

Add a small view-state model in `state.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Normal,
    Condensed,
}

pub struct AppState {
    // existing fields
    pub view_mode: ViewMode,
    pub drives_expanded: bool,
}
```

Phase 51 initializes `view_mode` to `Normal`. Phase 52 implements actual mode switching and condensed rendering.

`drives_expanded` semantics:

- default `false`;
- not stored per system;
- when `true`, detail rows belong only to the currently selected online system;
- moving selection does not disable it;
- offline/pending selected systems show no detail rows;
- config reload preserves it unless state reconstruction policy already resets transient UI state globally;
- no config persistence.

Add reducer actions now if useful for isolated tests:

```rust
PreviousView
NextView
ToggleDrives
```

Phase 52 maps keys and completes view behavior. If actions are introduced here, `PreviousView`/`NextView` may switch enum values immediately, but normal-only rendering tests must remain valid. Avoid a temporary action shape that Phase 52 must replace.

### Workstream A acceptance criteria

- [ ] View/expansion state is explicit and minimal.
- [ ] Expansion is global-to-selection, not a per-system collection.
- [ ] No state is persisted to config.
- [ ] Existing config/polling state fields remain unchanged.

## Workstream B: replace hardcoded entry heights

The pre-drive height model assumed online = four rows and offline/pending = one row. Replace it with one function that can inspect state, system, selection, view, and expansion:

```rust
pub fn entry_height(state: &AppState, system_index: usize) -> u16
```

Equivalent argument shapes are acceptable, but do not duplicate logic across modules.

Required normal-view rules:

```text
offline/pending                         1
online, collapsed                       5
online, selected, expanded              5 + valid_drive_detail_count
online, unselected, expansion enabled   5
```

`valid_drive_detail_count` should be:

- drive list length when `Some(nonempty)`;
- zero for `None` or empty;
- already bounded by protocol/normalization.

The same function must drive:

- `AppState::page_size`;
- `visible_range` or its replacement;
- `ensure_selected_visible`;
- `ui::layout::compute_viewport`;
- tests of rendered row positions.

Do not let the renderer independently decide to add rows that layout did not reserve.

### Small-terminal policy

The existing renderer rejects areas below a minimal height. Preserve simple behavior:

- if a complete selected online entry cannot fit, render as many complete logical entries as current viewport policy permits;
- do not partially render drive detail rows if the base five-row block fits but all details do not;
- detail rows may be clipped as a contiguous suffix if the layout deliberately reserves only those that fit, but the selected system base block must remain complete;
- avoid introducing nested scrolling for drives.

Preferred implementation: layout computes one entry rectangle for the rows that fit; the selected expanded entry may include only the leading detail rows that fit after its base block. Record a boolean/count on `ViewportEntry` if needed. Do not create a second drive viewport.

### Workstream B acceptance criteria

- [ ] One authoritative height calculation exists.
- [ ] Base online normal height is five rows.
- [ ] Only selected expansion affects height.
- [ ] Small terminals never render a partial base online block.
- [ ] No nested drive scrolling state exists.

## Workstream C: correct selection visibility

Call `ensure_selected_visible` or a corrected equivalent after every state transition that can make selection invisible or change row geometry/order:

```text
SelectNext
SelectPrevious
PageDown
PageUp
SelectFirst
SelectLast
Resize
ConfigReloaded
ToggleDrives
PreviousView
NextView
successful/failed poll batch application when display order can change
```

Current online-first ordering means poll results can move an entry between groups. Preserve that ordering but repair viewport anchoring.

### Visibility algorithm requirements

The algorithm must:

1. derive current display order;
2. resolve selected and top positions by stable system ID;
3. calculate the visible logical range using dynamic heights and usable terminal area;
4. leave the top ID unchanged if selection is already fully visible;
5. move top upward when selection is above;
6. move top just enough when selection is below, rather than always placing it at the first row if a less disruptive top exists;
7. remain safe after config removal/reorder;
8. handle empty systems without panic;
9. handle terminals too small for any full online entry.

A simple backward scan from the selected position to find the earliest top that still fits the selected entry is sufficient. Do not introduce pixel/row scrolling.

### Required state tests

- select down beyond first viewport and observe top change;
- select up above viewport and observe top change;
- selection already visible leaves top stable;
- page down/up with mixed online/offline heights;
- online-first reorder after poll keeps selected visible;
- expansion makes selected entry taller and viewport adjusts;
- collapse permits more systems without invalid top;
- resize smaller/larger;
- selected system removed by config reload;
- empty state;
- tiny terminal.

### Workstream C acceptance criteria

- [ ] `j`/`k` selection cannot disappear off-screen.
- [ ] Poll-induced ordering changes keep selection visible.
- [ ] View/expansion geometry changes keep selection visible.
- [ ] Scrolling remains by logical system.

## Workstream D: render aggregate disk row

Extend `render_online` to render five base rows.

The new row occupies base row 4:

```text
DISK  [|||||               ] 25.0% 238.0 GiB used / 714.0 GiB avail
```

Use the Phase 49 aggregate helper. Do not recompute sums inside `system_block.rs`.

Required states:

### Populated drives

- label `DISK`;
- percentage from aggregate helper;
- detail includes total used and total available;
- binary units through existing `format_bytes`;
- detail degrades/truncates through existing bar behavior.

### Unavailable drives (`None`)

Render a clear unavailable row, for example:

```text
DISK [                    ] —
```

Prefer extending `bar::render_bar` with an explicit unavailable mode or rendering a small dedicated line. Do not pass `0.0` in a way that visually appears measured.

### Successfully empty drive list

Use the same visual unavailable/no-eligible result as `None` unless product wording can distinguish it compactly without adding state complexity. The API distinction remains available to non-TUI clients.

### Overflow/invalid aggregate

Treat aggregate failure as unavailable and optionally debug-assert in tests. Never display wrapped values.

### Workstream D acceptance criteria

- [ ] Every online normal entry reserves/renders one disk row.
- [ ] Used and available are shown for populated data.
- [ ] Unavailable/empty does not display measured zero.
- [ ] Existing bar/format helpers are reused where appropriate.
- [ ] Narrow widths remain legible.

## Workstream E: render selected-system drive details

When `drives_expanded` is true and the system is selected and online with populated drives, render one plain text row per drive immediately after the aggregate disk row.

Preferred wide format:

```text
  /                 96.0 GiB / 475.0 GiB  20.2%
  /mnt/archive     142.0 GiB / 477.0 GiB  29.8%
```

Required semantics:

- values are `used / total`, not `used / available`;
- percentage is per-drive derived from valid wire values;
- names remain in normalized deterministic order;
- selected reverse style may apply to the base header only, preserving current convention;
- detail rows use subtle indentation and no borders;
- no separate bars per drive;
- names are Unicode-width truncated to leave value columns visible;
- very narrow widths degrade to name plus percentage, then name only if necessary;
- no detail rows for offline, pending, unavailable, or empty drives.

Create one focused text/layout helper rather than embedding several width branches in `render_online`.

### Required rendering tests

- one drive expanded;
- several drives expanded;
- unselected systems remain collapsed;
- moving selection moves details without retaining old details;
- unavailable/empty produces no detail rows;
- long mount name truncation;
- Unicode name truncation;
- narrow/medium/wide values;
- details clipped only after complete base block;
- selected header style remains correct.

### Workstream E acceptance criteria

- [ ] Only the selected system expands.
- [ ] Detail rows use used/total and per-drive percentage.
- [ ] Long names cannot overwrite values or panic.
- [ ] Expansion adds no bars, borders, or nested state.

## Workstream F: update layout and renderer interfaces narrowly

Likely interface changes:

```rust
pub struct ViewportEntry {
    pub index: usize,
    pub rect: Rect,
    pub is_selected: bool,
    pub drive_rows_visible: usize, // only if clipping details is needed
}
```

Keep `ui::render` responsible only for selecting the appropriate renderer and iterating entries. Do not move state mutation into UI.

If drive rows are clipped, pass the visible count to `render_online`; do not have it inspect terminal-global geometry independently.

Update diagnostics key hints only after Phase 52 input bindings land, or use a provisional hint that remains truthful. Do not show undocumented keys.

### Workstream F acceptance criteria

- [ ] Layout fully determines allocated rows.
- [ ] Renderer does not mutate state or query I/O.
- [ ] Interfaces remain specific to current two-view needs.

## Workstream G: reconcile documentation contract

Update active documentation that states online entries are always four rows.

Required changes:

- `AGENTS.md`: normal online base is five rows; selected drive details add bounded dynamic rows; scrolling remains logical-system based;
- root README TUI description if it states four rows;
- architecture notes or Phase 8 references only where active text would mislead implementers.

Do not rewrite historical completed plans merely to match new behavior. The new roadmap supersedes the old fixed-row product contract for current implementation.

### Workstream G acceptance criteria

- [ ] Active contributor guidance matches dynamic normal-entry behavior.
- [ ] Historical plan files remain historical rather than rewritten.

## Expected files

Likely change surface:

```text
crates/gregg/src/action.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/layout.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/bar.rs                 # only if explicit unavailable rendering is added
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/diagnostics.rs         # only if hints remain truthful
crates/gregg/src/ui tests
AGENTS.md
README.md
```

Do not modify native collectors or protocol types in this phase except test fixtures needed to construct normalized data.

## Implementation sequence

1. Add explicit view/expansion state and action variants without key mappings.
2. Replace `entry_height` with one state-aware implementation.
3. Update page-size and visible-range helpers.
4. Rewrite selection-visibility correction and apply it after all relevant mutations/batches.
5. Add state/layout tests before rendering changes.
6. Extend normal online base block to five rows.
7. Add explicit disk-unavailable rendering.
8. Add selected-system detail row helper and width degradation.
9. Add Ratatui buffer tests for populated/unavailable/expanded/multi-system cases.
10. Update active documentation.
11. Run focused checks and inspect for accidental condensed/input scope.

## Required verification

Focused checks:

```text
cargo fmt --all -- --check
cargo test -p gregg state --all-features
cargo test -p gregg ui --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

If test filtering is not useful, run:

```text
cargo test -p gregg --all-targets --all-features
```

No new visual snapshot framework is required. Existing `TestBackend` string/style assertions are sufficient.

## Phase acceptance criteria

Phase 51 is complete only when:

- [ ] Normal view shows all logical systems that fit rather than behaving as a fixed single-host panel.
- [ ] `j`/Down and `k`/Up keep selection visible across mixed-height entries.
- [ ] Poll-induced online/offline reorder keeps selection visible.
- [ ] One authoritative dynamic entry-height function drives paging, viewport, layout, and rendering.
- [ ] Online normal entries have a five-row base including `DISK`.
- [ ] Populated disk rows show aggregate used, available, and percentage.
- [ ] Unavailable/empty disk data is visibly unavailable, not measured zero.
- [ ] Expansion adds per-drive used/total/percentage rows only for the selected system.
- [ ] Small terminals preserve complete base blocks and do not require nested drive scrolling.
- [ ] Offline/pending one-row rendering and current ordering semantics remain intact.
- [ ] Active documentation no longer claims a fixed four-row online contract.
- [ ] Focused state/layout/Ratatui tests pass.
- [ ] No condensed renderer, key mapping, native collection, persistent UI state, or new framework was added.

## Handoff guidance for a smaller implementation model

- Fix scrolling and dynamic height tests before changing rendering.
- Pass state into height calculations; do not let layout and renderer maintain separate formulas.
- Keep aggregate arithmetic in `normalized.rs` from Phase 49.
- Use one expansion boolean and current selection; do not add a map/set.
- Render simple text drive rows, not bars.
- Preserve logical-system navigation and online-first ordering.
- Stop if implementation starts adding nested panes, drive selection, or configurable layouts.
