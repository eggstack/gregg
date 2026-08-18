# Phase 085: fleet-wide TUI column and storage display correction

Status: complete; closed through Plan 086.

Depends on: completed Plans 083 and 084. Implementation landed in `f8be3cf2` and clippy cleanup in `29945c3`. Post-implementation review found three narrow boundary defects corrected by Plan 086 (condensed status identity, expanded-drive structural width, mixed-label suffix budget). Plan 086 records the corrections and reconciles this plan's acceptance checklist.

## Objective

Correct four concrete client-side display defects in the current Gregg TUI without changing the daemon, protocol, drive collectors, endpoint model, scheduler, or release/CI architecture:

1. normal-view metric bars currently align `[` and `]` only within one device block, not across the fleet;
2. the normal DISK row displays `used / available` even though its percentage is calculated from `used / total`, making the second quantity look like an incorrect denominator;
3. expanded per-drive rows (`e`) format each drive independently, so mount names, capacities, remaining space, and percentages do not form stable columns;
4. condensed view (`v`) uses a hard-coded header but separately pushes metric values toward the terminal's right edge, so values do not align under their headings and devices with different name lengths are visually staggered.

The intended correction is a small client-rendering pass built around shared, precomputed display geometry. Do not expand this into a general table framework or a redesign of Gregg's TUI.

## Baseline findings at Plan 085 creation

Current `main` is based on the Plan 083/084 normal-view renderer. The implementation correctly introduced one `MetricGroupLayout` per online system so CPU/MEM/SWP-or-COMMIT/DISK share a label width and bar width inside that system block. However, `system_block::render_online()` still builds its own rows and calls `compute_metric_group_layout()` independently for each system. A system whose longest suffix is `25.2% 8 cores` therefore chooses a different bar width from a system whose longest suffix is `83.7% 767.7 GiB / 102.8 GiB`. The result is internally aligned blocks whose closing `]` columns drift between devices.

The current normal DISK row derives the percentage from `DriveAggregate.used_bytes / DriveAggregate.total_bytes`, but formats its detail as `used_bytes / available_bytes`. The byte formatter itself correctly uses binary units (KiB/MiB/GiB/TiB). The defect is semantic presentation, not unit conversion. Gregg deliberately distinguishes filesystem allocation from caller-available space because `available_bytes` may differ from `total_bytes - used_bytes` due to reserved blocks, quotas, or platform semantics. Plan 067 established this model and it must remain intact.

The current `text::drive_detail_line()` measures and renders one drive at a time. Because each row independently calculates how much width to give the name after formatting that row's own numeric text, a row containing MiB, GiB, or TiB quantities begins its numeric values at a different column from neighboring rows.

The current condensed renderer has two unrelated geometry sources. `header_line()` emits fixed strings such as `HOST CPU   MEM   DISK  LOAD  IOWAIT`, while `online_line()` calculates `host_width` from the full terminal width and fixed suffix constants. This intentionally consumes spare terminal width in the HOST field and pushes values to the far right, so the row values cannot reliably sit under the header labels.

## Authoritative behavior after Plan 085

### Normal view

For a terminal width and current fleet state, all online normal-view metric rows use one fleet-wide geometry:

```text
host-a
    CPU    [||||||||||||||        ] 37.8% 8 cores
    MEM    [||||||||||||          ] 31.0% 5.0 GiB / 16.0 GiB
    SWP    [                      ] 0.0%
    DISK   [|||||||||||||||||     ] 83.7% 767.7 GiB / 917.2 GiB
host-b
    CPU    [||||                  ] 20.0% 4 cores
    MEM    [||||||||              ] 40.0% 3.2 GiB / 8.0 GiB
    COMMIT [||||||||||            ] 50.0% 4.0 GiB / 8.0 GiB
    DISK   [||||||||||||          ] 60.0% 1.2 TiB / 2.0 TiB
```

Exact values and bar widths vary. Required invariants are:

- the four-space indent remains;
- all applicable metric labels share one fleet-wide label width, including mixed `SWP` and `COMMIT` systems;
- every rendered opening `[` is in the same terminal column across all online systems using the normal bar layout;
- every rendered closing `]` is in the same terminal column across all online systems using the normal bar layout;
- the geometry is calculated from the whole configured/displayed fleet state, not only the rows currently visible in the viewport, so scrolling does not cause horizontal reflow;
- terminal display-cell width, not UTF-8 byte length, is authoritative for geometry.

The normal DISK suffix must be:

```text
<usage percentage> <used bytes> / <total bytes>
```

The slash denominator must match the denominator used by the percentage calculation. `available_bytes` remains part of the normalized model and is still used where remaining/caller-available space is explicitly shown.

### Expanded drive view (`e`)

Expanded drive rows for the selected system should form one compact aligned table. The full-width representation is:

```text
  <name>  <used> / <total>  (<remaining>) <percent>
```

For example:

```text
  /              767.7 GiB / 917.2 GiB  (102.8 GiB) 83.7%
  /mnt/archive   142.0 GiB / 477.0 GiB  (335.0 GiB) 29.8%
  /mnt/backup      1.2 TiB /   2.0 TiB  (800.0 GiB) 60.0%
```

Whitespace in the example is illustrative. The implementation must calculate column widths from the eligible drives belonging to the expanded system and then render every visible drive with the same layout.

Column semantics:

- `name`: drive/mount name, left aligned and truncatable;
- `used`: `drive.used_bytes`, right aligned;
- `/`: fixed separator;
- `total`: `drive.total_bytes`, right aligned;
- `remaining`: explicit `drive.available_bytes` when present, otherwise compatibility fallback `total_bytes - used_bytes`; render in parentheses immediately before percentage;
- `percent`: `used_bytes / total_bytes * 100`, right aligned.

Calculate the layout from all eligible drives for the selected system, not only the subset that happens to fit vertically in the current terminal. Vertical scrolling/height clipping must not change horizontal columns.

For narrow terminals, degrade only as much as required. Preferred order:

1. full `name  used / total  (remaining) percent`;
2. if necessary, truncate the name column first while retaining all numeric columns;
3. if the full numeric form genuinely cannot fit, fall back to `name  (remaining) percent`;
4. final minimal fallback may be `name  percent`.

Do not introduce horizontal scrolling or a table widget dependency.

### Condensed view (`v`)

The header and online rows must use the same computed `CondensedTableLayout`. Preserve the existing Wide/Medium/Narrow/Minimal tier concept and its priority-based dropping of lower-value columns, but stop deriving header spacing and row spacing independently.

For each active tier, calculate widths from the fleet data and headings:

```text
HOST    width = max(display width of "HOST", widest displayed device name)
CPU     width = max(display width of "CPU", widest formatted CPU value)
MEM     width = max(display width of "MEM", widest formatted memory value)
DISK    width = max(display width of "DISK", widest formatted disk value)
LOAD    width = max(display width of "LOAD", widest formatted load value)
IOWAIT  width = max(display width of "IOWAIT", widest formatted I/O-wait value)
```

Only columns present in the current width tier participate.

The HOST column is the flexible/truncatable column when total width exceeds the terminal. Numeric columns should remain intact whenever possible. Do not expand HOST to consume all otherwise-unused terminal width. Spare width may remain as trailing whitespace.

The header must be rendered through the exact same column formatter/layout used for online values so headings and values occupy identical starting/ending columns.

Example target shape:

```text
HOST       CPU  MEM  DISK  LOAD IOWAIT
deadpool    8%  42%   84%  0.38    1.2
pi5        21%  67%   51%  1.03    0.4
server3   100%  91%   17%  4.72    8.7
```

Do not redesign offline/pending status semantics in this phase. They may remain status-oriented rows, but changes made to shared name truncation/padding must remain width-safe and Unicode-aware.

## Implementation sequence

### Step 1: preserve the current data semantics and lock the DISK denominator with tests

Primary files:

```text
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/mod.rs
```

Before changing geometry, add or update a focused rendering test using a drive whose explicit `available_bytes` is deliberately not equal to `total_bytes - used_bytes`.

Example logical values:

```text
used      = 80 GiB
total     = 100 GiB
available = 10 GiB
```

Required normal-view assertions:

```text
percentage = 80.0%
detail contains "80.0 GiB / 100.0 GiB"
detail does not use "10.0 GiB" as the slash denominator
```

Keep a separate normalized/drive-detail assertion proving explicit availability remains available for the expanded remaining-space field.

Then change the normal DISK detail from `aggregate.available_bytes` to `aggregate.total_bytes`.

Do not modify `format_bytes()`, `DriveAggregate`, `NormalizedDrive`, protocol fields, or collectors for this display correction unless source inspection reveals a separate proven defect. The Plan 085 baseline indicates none is required.

### Step 2: separate metric-row content from fleet-wide geometry

Primary file:

```text
crates/gregg/src/ui/system_block.rs
```

Refactor only enough to allow `ui::render()` to prepare one geometry object and pass it to each online normal-view system.

Preferred shape:

```rust
pub(crate) struct MetricFleetLayout {
    label_width: u16,
    bar_width: u16,
}
```

The exact name is not important. Avoid storing per-system suffix strings in the fleet layout if they can remain derived from each system's own `MetricRow`s.

Useful seams may include:

```rust
pub(crate) fn metric_rows(snapshot: &NormalizedSnapshot) -> [MetricRow; 4]
pub(crate) fn compute_fleet_metric_layout<'a>(
    rows: impl Iterator<Item = &'a [MetricRow; 4]>,
    width: u16,
) -> MetricFleetLayout
```

or a similarly small API.

Requirements:

- one global `label_width` includes `COMMIT` whenever any participating system uses it;
- resolve/truncate each system's suffix strings with the existing width-safe policy;
- find the maximum display-cell width of the resolved suffixes across every participating system;
- derive one `bar_width` from terminal width, structural prefix width, and that global suffix maximum;
- render each device with the shared `label_width` and `bar_width` while retaining its own suffix text;
- preserve the existing narrow-width path where a bar cannot be rendered at all.

Do not create a new generic layout crate or duplicate the existing suffix-compaction logic.

### Step 3: compute normal-view geometry before rendering viewport entries

Primary file:

```text
crates/gregg/src/ui/mod.rs
```

`render()` currently computes viewport entries and then invokes `system_block::render_online()` independently.

Before entering the render loop, compute the fleet metric layout from all online systems with a current normalized snapshot when `SystemViewMode::Normal` is active.

Important invariant:

```text
layout population = all online systems with usable snapshots
not merely entries returned by compute_viewport()
```

This prevents `j`/`k`, page movement, or a short terminal from changing the bar columns simply because a different subset is visible.

Pass the resulting shared layout to every `render_online()` call.

Pending/offline systems do not contribute metric suffixes because they do not render metric bars.

### Step 4: add fleet-level renderer tests for normal view

Primary file:

```text
crates/gregg/src/ui/mod.rs
```

Use the existing `TestBackend`, `render_state()`, and terminal-cell helpers rather than adding screenshot/golden infrastructure.

Required cases:

#### Different suffix lengths across devices

Create at least two online systems whose natural suffix lengths differ substantially, for example:

```text
system A: small memory/disk values, 4 cores
system B: TiB-scale disk values, 128 cores
```

Render both in one terminal and collect CPU/MEM/SWP-or-COMMIT/DISK rows from both blocks.

Assert that all rows with bars have identical terminal columns for:

```text
[
]
```

This is the regression test missing from Plan 083/084.

#### Mixed Linux/Windows label width

Use one Linux-shaped snapshot and one Windows-shaped snapshot so `SWP` and `COMMIT` coexist in the fleet.

Assert every opening and closing bracket remains aligned across both systems and that `COMMIT` does not shift its device block.

#### Off-viewport stability

Create enough systems that not all fit vertically. Give an off-viewport system the longest suffix and verify the visible systems still use the geometry implied by the whole fleet. Move the viewport and verify bracket columns do not change.

Do not assert a fixed numeric bar width; assert equality/invariance.

#### Representative terminal widths

Retain existing narrow-width behavior and test at least one normal width and one constrained width where bars still exist. If the intentional fallback removes brackets at very small widths, assert the fallback rather than forcing impossible alignment.

### Step 5: replace independent drive-detail rows with one selected-system table layout

Primary files:

```text
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/condensed.rs
```

Prefer a small shared structure in `text.rs` or a narrowly named UI module rather than teaching `drive_detail_line()` to guess geometry from one row.

Suggested shape:

```rust
struct DriveTableLayout {
    name_width: usize,
    used_width: usize,
    total_width: usize,
    remaining_width: usize,
    pct_width: usize,
    mode: DriveDetailMode,
}
```

Build the layout from formatted fields for every eligible drive in the selected system.

Use `UnicodeWidthStr` for names and every formatted field. Rust format-string width counts characters/bytes rather than terminal cells for some Unicode cases, so where necessary use existing explicit cell-padding helpers instead of assuming `:<width$` is terminal-cell correct.

The remaining quantity is:

```rust
drive.available_bytes.unwrap_or(drive.total_bytes - drive.used_bytes)
```

Only compute the fallback after the existing eligibility guard ensures `used_bytes <= total_bytes`.

The row renderer must be deterministic for a given layout and drive. Both normal and condensed expanded-drive paths should use the same table layout/formatter rather than diverging.

### Step 6: add expanded-drive table tests

Primary files:

```text
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/text.rs
```

Use drives deliberately chosen to produce differing field widths:

```text
short and long mount names
MiB-scale quantity
GiB-scale quantity
TiB-scale quantity
explicit available_bytes != total - used
```

Required assertions at a width where the full representation fits:

- the first terminal column of `used` is consistent across rows after right alignment;
- `/` separators occupy the same terminal column;
- total quantities end in the same terminal column;
- opening/closing parentheses around remaining space occupy consistent columns;
- percentage fields end in the same terminal column;
- drive names are left aligned;
- explicit `available_bytes` is shown inside `(...)` when present;
- compatibility fallback `total - used` is shown when availability is absent;
- percentages still use `used / total`;
- every line is width-bounded.

Add one constrained-width case demonstrating the planned degradation order without panicking or corrupting adjacent columns.

Do not require every possible terminal width to have a golden string.

### Step 7: introduce one shared condensed-view column layout

Primary file:

```text
crates/gregg/src/ui/condensed.rs
```

Keep the existing `Tier` decision unless implementation inspection proves a direct bug in the thresholds.

Replace the separate hard-coded `header_line()` spacing and `online_line()` `host_width = terminal_width - constant` logic with one computed layout.

Preferred flow:

```text
render(...)
  -> determine Tier from terminal width
  -> compute CondensedTableLayout from all systems for that Tier
  -> render header with layout
  -> render each online row with same layout
```

Because `ui::render()` already owns fleet state, either compute the condensed layout there and pass it down, or expose a `condensed::compute_layout(state, width)` helper and call it once. Do not recompute it separately per entry.

For each online system, preformat the same values used today:

```text
CPU percentage
memory percentage
disk percentage
load one-minute value
I/O wait value or em dash
```

Column widths must be based on terminal display width of both the heading and all formatted participating values.

The display-name width must be based on configured nickname when present, otherwise endpoint host, consistent with current behavior.

When total natural table width exceeds the available width:

1. shrink/truncate HOST first;
2. if still impossible, use the existing tier's column-dropping policy by moving to the narrower tier rather than clipping numeric columns;
3. preserve the existing minimum-width diagnostics/fallback behavior.

Avoid a second independent set of magic subtraction constants.

### Step 8: add condensed header/value alignment tests

Primary file:

```text
crates/gregg/src/ui/mod.rs
```

Required cases:

#### Different nickname lengths

Create systems such as:

```text
pi5
server3
deadpool-longer-name
```

with deliberately different CPU/MEM/DISK/load/I/O values.

For the active tier, locate each header label's terminal start/end columns and assert the corresponding values use the same column bounds/alignment.

Do not merely assert the row has terminal width N; prove column correspondence.

#### No right-edge host inflation

At a wide terminal, assert the CPU column begins shortly after the width actually required by the longest displayed host plus the defined inter-column gap, rather than being displaced toward the far right because spare terminal width was assigned to HOST.

Do not hard-code an exact absolute CPU column if natural fleet data changes; derive the expected host width from the fixture names.

#### Tier degradation

Retain tests proving Wide/Medium/Narrow/Minimal drop the same lower-priority columns as today. Update them to use the shared layout/header formatter.

#### Unicode width

Include at least one configured display name whose byte length and terminal-cell width differ, and assert it does not shift the numeric columns relative to ASCII rows.

### Step 9: reconcile comments and active documentation only where needed

Expected files to inspect:

```text
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/condensed.rs
crates/gregg/src/ui/text.rs
README.md
crates/gregg/README.md
architecture/gregg-client.md
.opencode/skills/gregg-client/SKILL.md
plans/README.md
this plan
```

Update comments that still claim normal metric geometry is shared only within one block or describe the aggregate DISK second value as available space.

If user documentation shows the old `used / available` normal DISK example, change it to `used / total` and describe remaining/caller-available space only in the expanded drive context.

Do not rewrite Plan 067's historical capacity semantics. It remains correct: Gregg should preserve caller-available space independently even though normal-view DISK now displays total capacity.

Do not rewrite Plans 083/084 merely because their earlier geometry was block-local. They are historical records of the work that landed. Plan 085 should be listed as the subsequent product correction.

## Expected production-code surface

Primary:

```text
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/condensed.rs
crates/gregg/src/ui/text.rs
```

Possible small helper-only changes:

```text
crates/gregg/src/ui/layout.rs
crates/gregg/src/ui/bar.rs
```

Only touch those helper files if doing so reduces duplication in the concrete Plan 085 implementation.

Files that should not require product changes:

```text
crates/gregg/src/normalized.rs
crates/gregg/src/state.rs
crates/gregg/src/scheduler.rs
crates/gregg/src/cli.rs
crates/gregg/src/endpoint.rs
crates/greggd/**
crates/gregg-protocol/**
.github/workflows/**
scripts/**
packaging/**
```

If implementation begins modifying daemon collectors, protocol schemas, scheduler behavior, or CI, stop and establish a separate concrete defect before broadening scope. The known storage issue is a formatter choosing `available_bytes` where it should choose `total_bytes`, not a collector/unit-conversion failure.

## Explicit non-goals

Do not introduce or redesign:

- drive collection semantics;
- `available_bytes` protocol fields or Plan 067 invariants;
- decimal SI storage units;
- SMART/health/temperature/partition topology;
- sorting or filtering of drives;
- alternate drive labels;
- a general table/layout framework;
- horizontal scrolling;
- mouse controls;
- colors/themes;
- additional TUI panes;
- endpoint/nickname behavior;
- polling cadence, scheduler, or state architecture;
- daemon behavior;
- new dependencies;
- new CI workflows/jobs/matrices;
- snapshot/screenshot/golden-test infrastructure;
- release automation or evidence bundles;
- unrelated cleanup.

## Focused verification

Use existing tests and the lightest appropriate local verification. Expected commands:

```bash
cargo fmt --all -- --check
cargo test -p gregg ui
cargo test -p gregg normalized
./scripts/check-local.sh
```

`cargo test -p gregg normalized` is included only to demonstrate the drive aggregate/availability model remains intact; Plan 085 should not require normalized production changes.

If test filters differ after implementation, run the nearest exact module/test filters and record the actual commands in this plan's completion section.

Do not add a release preflight requirement for this client-only visual correction.

Do not add new CI. The existing workflow may run naturally after push and should remain green, but completion is established by deterministic renderer tests plus the default local check. Native Windows/macOS CI is not required to prove terminal spacing because the renderer operates on normalized snapshots and existing cross-platform fixtures already allow Linux/Windows-shaped data to be exercised locally.

## Acceptance criteria

### Normal fleet-wide metric geometry

- [x] Normal-view metric geometry is computed once per render/fleet width and reused by every online system block.
- [x] Layout population includes all online systems with usable snapshots, not only currently visible viewport entries.
- [x] Global label width accounts for the widest participating label, including `COMMIT` in a mixed Linux/Windows fleet.
- [x] All rendered normal-view opening `[` characters occupy one terminal column across devices where bars are present.
- [x] All rendered normal-view closing `]` characters occupy one terminal column across devices where bars are present.
- [x] Different device suffix lengths do not produce different bracket columns.
- [x] Scrolling or moving the viewport does not change bracket columns merely because a longest-suffix system moves on/off screen.
- [x] Narrow-width fallback remains width-safe and does not force brackets where no bar budget exists.
- [x] Geometry uses terminal display-cell widths rather than UTF-8 byte lengths.

### Normal DISK semantics

- [x] Normal DISK percentage remains `used_bytes / total_bytes * 100`.
- [x] Normal DISK detail renders `used_bytes / total_bytes`.
- [x] Explicit `available_bytes` is not used as the slash denominator.
- [x] A regression fixture with `available_bytes != total_bytes - used_bytes` proves the denominator correction.
- [x] Existing binary KiB/MiB/GiB/TiB conversion remains unchanged.
- [x] No protocol, normalized-capacity, or collector semantics are weakened to make the display complementary.

### Expanded drive table

- [x] Expanded rows render the full-width shape `name  used / total  (remaining) percent` when it fits.
- [x] Remaining space uses explicit `available_bytes` when present.
- [x] Remaining space falls back to `total_bytes - used_bytes` for legacy records with no explicit availability.
- [x] Percentage remains based on `used / total`.
- [x] One layout is calculated from all eligible drives in the selected system.
- [x] Visible-row clipping does not change horizontal column positions.
- [x] Name, used, separator, total, remaining, and percentage fields align consistently across rows.
- [x] Numeric quantities and percentages are right aligned; names are left aligned/truncatable.
- [x] Unicode drive names are measured in terminal cells.
- [x] Narrow terminals degrade in the defined order without overflow or panic.
- [x] Normal and condensed expanded-drive rendering reuse the same drive-table layout/formatter.

### Condensed (`v`) view

- [x] Header and online rows use the same `CondensedTableLayout`.
- [x] HOST, CPU, MEM, DISK, LOAD, and IOWAIT headings align with their value columns whenever the tier includes them.
- [x] Column widths are derived from headings and fleet values using terminal display width.
- [x] HOST width is based on the longest required displayed name and is not inflated to consume all spare terminal width.
- [x] Spare width may remain at the right side rather than pushing metrics away from headings.
- [x] The HOST column is the first column truncated when natural table width is too large.
- [x] Existing Wide/Medium/Narrow/Minimal column-priority behavior remains intact.
- [x] Systems with short and long nicknames remain column-aligned.
- [x] A Unicode nickname does not shift numeric columns relative to ASCII names.
- [x] Offline/pending status rendering remains truthful and width-safe; no redesign is required. *(Plan 086 strengthens the offline/pending identity guarantee with explicit tests.)*

### Scope and verification

- [x] Production changes remain confined to the client UI unless a new concrete defect is documented.
- [x] `crates/greggd/**` and `crates/gregg-protocol/**` require no Plan 085 product changes.
- [x] No new dependency, generic table framework, CI job, workflow, release step, or evidence artifact is added.
- [x] Existing Plan 067 truthful availability semantics remain preserved.
- [x] Existing Plan 083/084 CLI, polling, viewport-initialization, offline rendering, and validation behavior remain unchanged.
- [x] Focused `gregg` UI tests pass.
- [x] Relevant normalized tests pass without semantic changes.
- [x] `./scripts/check-local.sh` passes.
- [x] Active documentation/comments accurately describe fleet-wide geometry and `used / total` normal DISK display.

## Handoff guidance

Implement this as one bounded phase. Do not split the work into additional planning phases unless implementation discovers an independently justified correctness defect outside the client renderer.

Recommended order for the implementing agent:

```text
1. lock DISK used/total semantics with a regression test;
2. implement fleet-wide normal metric layout and renderer tests;
3. implement shared expanded-drive table layout and tests;
4. implement shared condensed layout/header formatting and tests;
5. run focused checks and default local verification;
6. update active docs and this plan's completion record.
```

When complete, record:

- files changed;
- the final shared-layout seams introduced;
- renderer tests proving cross-device bracket alignment, drive-table alignment, and condensed header/value alignment;
- exact focused/default local test commands and results;
- any existing CI run that occurred naturally, without making it a new standing requirement.

Do not create a separate evidence document or a closure-only Plan 086.

## Completion record

Implemented in `f8be3cf2` (renderer changes) and `29945c3` (clippy cleanup). Focused deterministic tests and the default local check (`./scripts/check-local.sh`) passed. Three narrow boundary defects were discovered in post-implementation review; they are corrected by Plan 086 without reopening the daemon, protocol, scheduler, or release architecture. Plan 086 also reconciles this plan's acceptance checklist once the corrected behavior is demonstrated.
