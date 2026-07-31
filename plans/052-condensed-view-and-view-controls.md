# Phase 52: condensed view and view controls

Status: completed.

## Objective

Add a compact one-row-per-system fleet view modeled on `condensed.txt`, complete the requested view-cycling and drive-expansion key behavior, and reuse the Phase 51 dynamic viewport rather than creating a second navigation system.

The condensed view is a presentation alternative over the same `AppState`, system ordering, selection, normalized metrics, drive aggregate helper, and polling data. It is not a separate screen, dashboard engine, or configurable table.

## Dependencies and execution position

Depends on:

- Phase 49 normalized drive and aggregate semantics;
- Phase 50 cross-platform drive data for final native behavior, although synthetic fixtures can support early UI work;
- Phase 51 explicit `ViewMode`, global selected-system expansion, dynamic entry heights, and reliable viewport-following.

Must complete before Phase 53 final integration and documentation closure.

## Governing invariants

1. Normal and condensed views consume the same `AppState` and display order.
2. `h`/Left selects the previous view; `l`/Right selects the next view.
3. With two views, both directions wrap to the other view without a generalized registry.
4. `e` toggles drive details for the currently selected system in either view.
5. `j`/Down and `k`/Up retain existing logical-system selection semantics in both views.
6. Only the selected system expands.
7. Condensed collapsed online entries use one row.
8. Condensed offline/pending entries use one row and remain visibly unavailable.
9. Unsupported load, I/O-wait, or disk data renders `—`, never fabricated zero.
10. Width degradation drops low-priority columns; it does not horizontally scroll.
11. The renderer remains pure and adds no polling, sorting, filtering, or configuration state.
12. Key hints remain truthful and compact.

## Scope

### In scope

- `PreviousView`, `NextView`, and `ToggleDrives` input mapping;
- `h`/`l`, Left/Right, and `e` tests;
- view-mode reducer behavior and wraparound;
- condensed renderer;
- condensed header/separator and fixed width tiers;
- condensed offline/pending rows;
- condensed selected-system drive-detail rows;
- dynamic condensed entry-height behavior;
- view-change viewport correction;
- key-hint update;
- focused Ratatui/state tests;
- README/key documentation.

### Out of scope

- more than two views;
- runtime-configurable columns;
- sorting, filtering, searching, grouping, pinning, or pagination modes;
- mouse navigation;
- persistent view/expansion settings;
- horizontal scrolling;
- per-drive selection or actions;
- color themes, charts, sparklines, alerts, or thresholds;
- changes to protocol/native collection;
- a generic table/widget framework;
- new CI or release machinery.

## Workstream A: complete input-to-action mappings

Extend `key_to_action` with:

```text
h or Left   -> PreviousView
l or Right  -> NextView
e           -> ToggleDrives
```

Modifier rules:

- plain keys only;
- existing Ctrl-C and Ctrl-R precedence remains unchanged;
- uppercase `H`, `L`, or `E` need not map unless crossterm reports ordinary shifted characters and project conventions deliberately accept them;
- Alt/Option-modified variants remain unmapped;
- no conflict with existing `j`, `k`, `g`, `G`, `f`, `b`, `q`, or Esc behavior.

Required tests:

- each character binding;
- each arrow binding;
- `e` toggle action;
- Ctrl/Alt variants do not accidentally map;
- existing key tests remain unchanged/passing.

### Workstream A acceptance criteria

- [ ] All requested keys map exactly once.
- [ ] Existing navigation/quit/refresh mappings are preserved.
- [ ] No multi-key command parser or mode-specific input layer is introduced.

## Workstream B: finalize view-mode reducer behavior

`AppState::apply_action` must implement:

```text
Normal + NextView     -> Condensed
Normal + PreviousView -> Condensed
Condensed + NextView  -> Normal
Condensed + PreviousView -> Normal
ToggleDrives -> invert drives_expanded
```

Use directional action names even though two modes currently make both operations equivalent. Implement wraparound with a direct `match`, not a dynamic vector or registry.

After every view or expansion action:

1. update state;
2. call the Phase 51 selection-visibility correction using the new geometry;
3. preserve selected system ID;
4. preserve viewport top only when still valid and selection remains visible.

Expansion remains active while switching views. This gives `e` consistent toggle semantics and avoids implicit state changes.

Required reducer tests:

- initial mode is normal;
- next and previous wrap both directions;
- repeated direction cycles deterministically;
- selection survives view changes;
- expansion survives view changes;
- `e` toggles on/off;
- view/expansion geometry adjusts viewport;
- empty config and tiny terminals do not panic.

### Workstream B acceptance criteria

- [ ] View changes are deterministic and preserve selection.
- [ ] Expansion state is not reset implicitly.
- [ ] View changes keep selection visible.
- [ ] No view history stack or persistence exists.

## Workstream C: define condensed row semantics

Create one focused module, preferably:

```text
crates/gregg/src/ui/condensed.rs
```

The wide display follows the supplied model:

```text
HOST          CPU   MEM   DISK   LOAD   IOWAIT
-----------------------------------------------
deadpool      12%   41%    25%   0.32      0.0
wolverine      4%   67%    34%   0.05      0.0
pi-kitchen    91%   95%    87%   7.43     18.2
nas           22%   84%    52%   1.10      1.5
```

### Column semantics

`HOST`

- configured display name preferred;
- endpoint host fallback;
- truncated by display width, not byte count;
- selected row uses the existing reverse-style convention.

`CPU`

- normalized CPU usage;
- rounded to nearest whole percentage for density;
- clamp defensively to `0..=100` through existing formatting policy.

`MEM`

- physical-memory usage percentage;
- whole percentage.

`DISK`

- aggregate drive usage percentage from Phase 49 helper;
- whole percentage;
- `—` when drives are unavailable, empty, or aggregate calculation fails.

`LOAD`

- one-minute load average only;
- two decimals;
- `—` when unsupported.

`IOWAIT`

- one decimal percentage without repeating the `%` symbol if column width requires the supplied compact style;
- `—` when unsupported or absent;
- do not infer from OS name.

### Header and separator

- render one header row and one separator row at the top of condensed mode;
- these rows reduce the usable viewport height and must be accounted for by layout/state calculations;
- if the terminal is too short for header, separator, and one logical row, use the existing too-small diagnostic rather than partially rendering the table.

### Workstream C acceptance criteria

- [ ] Wide condensed output matches the supplied semantic layout.
- [ ] Every metric is capability/normalized-data driven.
- [ ] Header/separator height is accounted for.
- [ ] Selected style applies to one logical system row.

## Workstream D: implement fixed width tiers

Use a small number of explicit tiers. Suggested minimums may be adjusted after buffer tests:

```text
wide    >= 64 columns: HOST CPU MEM DISK LOAD IOWAIT
medium  >= 48 columns: HOST CPU MEM DISK LOAD
narrow  >= 30 columns: HOST CPU MEM DISK
minimal >= 24 columns: HOST CPU MEM
```

Below the repository's existing minimum terminal width, retain the too-small diagnostic.

Priority order:

1. host;
2. CPU;
3. memory;
4. disk;
5. load;
6. I/O-wait.

This differs slightly from the normal header priority because condensed mode exists specifically for fleet resource comparison. Disk remains present before load/I/O-wait.

Implementation guidance:

- use fixed label/value widths for metric columns;
- assign remaining width to host;
- build one line from spans or a preformatted string;
- use Unicode display-width helpers for truncation;
- do not add Ratatui's stateful table machinery unless plain lines become materially more complex;
- do not expose horizontal scrolling.

Required buffer tests:

- wide exact headers/columns;
- medium excludes I/O-wait;
- narrow excludes load and I/O-wait;
- minimal remains aligned;
- long host truncation;
- Unicode host truncation;
- percentages at 0, single digit, two digits, and 100;
- unavailable symbols align.

### Workstream D acceptance criteria

- [ ] Width degradation is deterministic.
- [ ] Host remains visible at every supported width.
- [ ] No horizontal scroll or generic column system exists.
- [ ] Unicode truncation cannot corrupt alignment or panic.

## Workstream E: render offline and pending systems

Offline/pending systems still occupy one condensed row and remain in existing display order after online systems.

Preferred wide forms:

```text
backup        offline
new-pi        pending
```

or aligned metric placeholders:

```text
backup        —     —     —      offline
new-pi        —     —     —      pending
```

Choose the simpler form that remains clearly associated with the host and preserves selected-row styling. Do not display stale metric values as current when reachability is offline, even if `latest` retains the last successful snapshot.

The row must remain one line across all width tiers.

Required tests:

- mixed online/offline/pending ordering;
- selected offline/pending style;
- no stale percentages shown;
- narrow width;
- viewport includes one-row offline entries correctly.

### Workstream E acceptance criteria

- [ ] Offline/pending hosts remain visible and one row high.
- [ ] Stale metrics are not presented as current.
- [ ] Existing online-first ordering remains unchanged.

## Workstream F: condensed drive expansion

When `drives_expanded` is true and an online system is selected with populated drives, render detail rows immediately beneath that condensed host row.

Reuse the same detail formatting helper introduced in Phase 51 where possible. Do not duplicate used/total/percentage arithmetic.

Example:

```text
deadpool      12%   41%    25%   0.32      0.0
  /                  96.0 GiB / 475.0 GiB  20.2%
  /mnt/archive      142.0 GiB / 477.0 GiB  29.8%
```

Condensed height rules:

```text
offline/pending                         1
online, collapsed                       1
online, selected, expanded              1 + visible_drive_detail_count
online, unselected, expansion enabled   1
```

The condensed header/separator are not part of an individual system height but reduce usable content area.

Small-terminal behavior follows Phase 51:

- system row must be complete;
- detail rows may be clipped as a suffix;
- no nested drive scrolling;
- selected system remains visible.

Required tests:

- expansion in condensed mode;
- only selected system expands;
- selection movement transfers details;
- mode switch normal -> condensed retains expansion;
- unavailable/empty drives add no rows;
- detail clipping preserves base row;
- mixed row heights scroll correctly.

### Workstream F acceptance criteria

- [ ] `e` has the same selected-system semantics in both views.
- [ ] Detail formatting/arithmetic is shared with normal view.
- [ ] Condensed viewport accounts for detail rows and header rows.
- [ ] No drive-selection submode exists.

## Workstream G: route rendering by view mode

Update `ui::render` narrowly:

```rust
match state.view_mode {
    ViewMode::Normal => render_normal(...),
    ViewMode::Condensed => condensed::render(...),
}
```

Keep empty-config and too-small diagnostics centralized before mode dispatch where possible.

Layout may expose one view-aware computation function or separate small normal/condensed helpers that both use the authoritative state height function. Avoid a trait-based renderer hierarchy.

Normal view output from Phase 51 must remain unchanged when `view_mode == Normal`.

### Workstream G acceptance criteria

- [ ] View dispatch is one direct enum match.
- [ ] Normal mode regression tests remain unchanged/passing.
- [ ] Diagnostics remain consistent.
- [ ] No renderer trait/registry is added.

## Workstream H: update key hints and public documentation

Update the footer hint to include the new controls while remaining compact. Suggested wide hint:

```text
j/k:select  h/l:view  e:drives  g/G:first/last  Ctrl-R:refresh  q:quit
```

At narrow widths, truncate or use a shorter truthful form such as:

```text
j/k select  h/l view  e drives  q quit
```

Do not let the hint overwrite system content. Preserve the existing rule that it renders only when an extra row is available.

Update:

- root README TUI keys and condensed example;
- `AGENTS.md` TUI rules;
- crate README if it documents keyboard behavior;
- no historical plan rewrites.

### Workstream H acceptance criteria

- [ ] Hints and docs list all requested controls.
- [ ] Arrow-key equivalents are documented where appropriate.
- [ ] No unsupported keys are advertised.

## Expected files

Likely change surface:

```text
crates/gregg/src/action.rs
crates/gregg/src/event.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/layout.rs
crates/gregg/src/ui/condensed.rs
crates/gregg/src/ui/system_block.rs or shared drive-detail helper
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/diagnostics.rs
crates/gregg/src/ui tests
README.md
AGENTS.md
crates/gregg/README.md if applicable
```

Do not change protocol or collector logic except fixture construction needed by tests.

## Implementation sequence

1. Add and test key mappings.
2. Finalize direct enum reducer behavior and view/expansion visibility correction.
3. Define condensed header height and view-aware usable area.
4. Implement wide online row from normalized fixture data.
5. Add fixed width tiers and Unicode truncation tests.
6. Add offline/pending row behavior.
7. Reuse drive-detail formatting and implement expansion heights/clipping.
8. Route `ui::render` by direct enum match.
9. Add mixed-fleet/view-switch/scroll buffer tests.
10. Update hints and active docs.
11. Run focused checks and inspect for accidental table/config scope.

## Required verification

Focused checks:

```text
cargo fmt --all -- --check
cargo test -p gregg event state ui --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

If filtered invocation is awkward:

```text
cargo test -p gregg --all-targets --all-features
```

Manual local inspection should be limited to launching the TUI with several fixture/live endpoints and pressing the requested keys. Do not create a screenshot/evidence artifact requirement.

## Phase acceptance criteria

Phase 52 is complete only when:

- [ ] `h` and Left select the previous view.
- [ ] `l` and Right select the next view.
- [ ] Two-view wraparound is deterministic.
- [ ] `e` toggles selected-system drive details in both views.
- [ ] `j`/Down and `k`/Up retain logical selection and viewport-following in both views.
- [ ] Condensed wide mode displays HOST, CPU, MEM, DISK, LOAD, and IOWAIT with the intended semantics.
- [ ] Condensed collapsed online entries occupy one row.
- [ ] Offline/pending entries occupy one row and do not show stale metrics.
- [ ] Unsupported load/I/O-wait/disk renders `—` rather than zero.
- [ ] Fixed width tiers degrade without horizontal scrolling.
- [ ] Only the selected system expands, and detail rows reuse normal-view formatting/arithmetic.
- [ ] Switching views preserves selection and expansion state.
- [ ] Normal view output remains correct.
- [ ] Key hints and active docs are accurate.
- [ ] Focused state/input/Ratatui tests pass.
- [ ] No configurable table, third view, persistence, mouse, sorting, filtering, or new infrastructure was added.

## Handoff guidance for a smaller implementation model

- Implement a direct `match` over two enum variants; do not generalize.
- Use plain Ratatui lines unless a concrete alignment problem requires more.
- Keep exactly the requested column priorities and a few fixed width tiers.
- Reuse normalized aggregate and drive-detail helpers.
- Preserve selection/expansion when switching views.
- Do not expose stale metrics for offline systems.
- Stop if implementation starts adding configurable columns, table state, or horizontal scrolling.
