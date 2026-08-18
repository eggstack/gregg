# Phase 086: Plan 085 renderer boundary corrective pass

Status: complete.

Depends on: Plan 085 implementation `f8be3cf2bf8240583dcc59ddc04261df2d1847f8` and clippy follow-up `29945c32e7965c71c222b03a147478c737d42d23`.

## Objective

Close three concrete client-renderer defects found in post-implementation review of Plan 085, then reconcile the Plan 085/086 planning record. Preserve the parts of Plan 085 that are already correct: fleet-wide normal metric geometry, normal DISK `used / total` semantics, explicit caller-available drive capacity, shared expanded-drive formatting, and shared condensed header/value geometry.

This is a narrow corrective pass. Do not reopen the daemon, protocol, drive collectors, normalized capacity model, scheduler, CLI, endpoint handling, release architecture, or CI design.

The remaining defects are:

1. condensed offline/pending rows can lose the device identity because `CondensedTableLayout.host_width` is derived only from online systems and `status_line()` then attempts to fit both the name and status inside that width;
2. expanded-drive width accounting does not exactly match the string that is rendered, and the Compact degradation path can be skipped when a long name would fit after truncation;
3. normal-view per-system suffix budgeting uses that system's local label width instead of the fleet-wide label width, creating an exact-boundary mismatch in mixed `SWP`/`COMMIT` fleets.

A fourth item is record reconciliation: Plan 085's implementation commit says "close Plan 085", while the plan file and plan index correctly remain active and its acceptance checklist is still unchecked. Plan 086 should close the product behavior first, then reconcile those records without rewriting historical commits.

## Baseline findings

### 1. Condensed offline/pending identity can collapse to an empty name

Current `compute_condensed_table_layout()` preformats only systems whose reachability is `Online` when calculating fleet column widths. Therefore, in an all-offline or all-pending fleet, HOST width is effectively bounded by the heading `HOST` rather than by any configured nickname or endpoint host.

Current `status_line()` then does the equivalent of:

```rust
let name_width = layout.host_width;
let truncated = truncate_width(
    name,
    name_width.saturating_sub(status.chars().count() + 2),
);
format!("{}  {status}", pad_right(&truncated, name_width))
```

With a four-cell HOST width and the seven-character word `offline`, the name budget saturates to zero. The row can therefore render the status while dropping the device identity entirely.

This is a product defect, not merely a cosmetic preference. Offline/pending rows must continue identifying which configured system the status belongs to.

The existing `status_line_does_not_show_stale_metrics` test proves only that `offline` is present and no percentage is shown. It does not assert that the system name/host survives.

### 2. Expanded-drive width math differs from the rendered structure

`render_drive_detail_row()` Full mode emits:

```text
<2-cell indent><name><2-cell gap><used> / <total><2-cell gap><remaining><2-cell gap><percent>
```

The current Full-mode fit calculation omits the two-cell leading indent from `full_width`. Compact-mode fit likewise omits the leading indent from its natural-width calculation. As a result, an exact boundary can be classified as fitting even though the rendered row is wider than the terminal and is then clipped by Ratatui.

The current Compact branch also first asks whether the entire untruncated natural name fits:

```text
natural name + remaining + percent <= available width
```

Only after that succeeds does it calculate a truncated name budget. A long mount name can therefore cause the code to skip directly from Full to Minimal even when Compact would fit cleanly after truncating the name. This loses `(remaining)` unnecessarily and violates Plan 085's documented degradation order.

The fit calculation and renderer must share one authoritative structural-width model so constants such as indent and gaps cannot diverge.

### 3. Mixed-platform suffix resolution uses local rather than fleet label width

`MetricFleetLayout` correctly computes a fleet-wide `label_width`, including `COMMIT` when a Windows-shaped snapshot participates. `render_metric_row()` also correctly pads every row to that fleet label width.

However, `resolve_system_suffixes()` currently recomputes label width from the current system's four rows. A Linux system with `SWP` can therefore budget suffix space as though its label column were narrower than the fleet's `COMMIT` column, even though final rendering uses the wider fleet label width.

At most widths this is hidden by spare space. At an exact detail-compaction boundary, the Linux suffix can be retained or truncated using a budget a few cells larger than the final rendered row actually has. Ratatui then clips the final line rather than the suffix resolver making the intended width-aware choice.

The fix is conceptual rather than architectural: suffix budgeting must consume the exact same fleet prefix geometry that rendering consumes.

### 4. Plan 085 record is intentionally not yet closed

Current `plans/085-fleet-wide-tui-column-and-storage-display-correction.md` still says `Status: ready for implementation` and retains unchecked acceptance criteria. Current `plans/README.md` lists Plan 085 as active. Those records are more truthful than the implementation commit message because the three defects above remain.

Do not mark Plan 085 complete before Plan 086's product corrections and focused verification pass.

## Authoritative behavior after Plan 086

### Condensed offline/pending rows

At every supported condensed width where a system row is rendered:

- the row must retain a visible configured nickname, or endpoint host when no nickname exists, to the extent allowed by the terminal width;
- `offline` or `pending` must remain visible;
- stale CPU/MEM/DISK/LOAD/IOWAIT values must not be shown;
- the row must remain terminal-cell width bounded;
- Unicode names must be truncated/padded using display-cell width;
- an all-offline or all-pending fleet must not collapse every system row to an anonymous status string.

Do not require offline/pending rows to mimic online numeric columns. They may remain status-oriented rows. The correction should be the smallest width-safe representation that preserves both identity and status.

Preferred implementation direction: decouple status-row name budgeting from the online HOST numeric-table cell if necessary. It is acceptable for `status_line()` to use the full row width rather than forcing `name + status` into `layout.host_width`, provided this does not alter online header/value alignment. If retaining the table HOST width, ensure the width population includes all displayed system names and the status itself still fits without erasing identity.

Do not add a new status table or separate layout framework.

### Expanded drive rows

The width decision must be based on the exact cell structure that `render_drive_detail_row()` emits.

Full mode remains:

```text
  <name>  <used> / <total>  (<remaining>)  <percent>
```

Compact remains:

```text
  <name>  (<remaining>)  <percent>
```

Minimal remains:

```text
  <name>  <percent>
```

Required degradation order:

1. Full with natural name when it fits;
2. Full with truncated name when all numeric fields can still be retained;
3. Compact with natural or truncated name whenever at least a usable name plus remaining and percent fit;
4. Minimal only when Compact genuinely cannot fit.

The two-cell indent and every inter-field gap must participate in the fit calculation. A row accepted as fitting in a mode must not exceed the requested display width when rendered.

Prefer defining small constants/helpers for the structural widths rather than repeating numeric `2` and `3` adjustments in multiple branches. Do not introduce a generic table-layout abstraction.

### Normal metric suffix budgeting

One `MetricFleetLayout` remains authoritative for the entire render.

Per-system suffix resolution must derive its budget from:

```text
terminal width
- fleet-wide indent/label/open-bracket prefix
- fleet-wide bar width
- closing-bracket separator
```

It must not substitute a narrower system-local label width after the fleet layout has been chosen.

For a mixed Linux/Windows fleet, a Linux `SWP` row and Windows `COMMIT` row therefore use the same structural prefix width for suffix budgeting and final rendering.

The existing correct invariants remain unchanged:

- opening `[` columns align across devices;
- closing `]` columns align across devices;
- off-viewport systems participate in fleet geometry;
- normal DISK remains `percentage used / total`;
- caller-available bytes remain an expanded-drive concern.

## Implementation sequence

### Step 1: add failing condensed status identity tests before changing layout code

Primary files:

```text
crates/gregg/src/ui/condensed.rs
crates/gregg/src/ui/mod.rs
```

Add deterministic coverage for at least:

1. one offline system with a configured nickname;
2. one offline system with no configured nickname, requiring endpoint host fallback;
3. an all-offline fleet with differing name lengths;
4. a mixed online/offline fleet where the online nickname is short and the offline nickname is longer;
5. pending status parity for at least one case;
6. one Unicode nickname whose UTF-8 byte length differs from terminal display width.

Required assertions:

- identity text is non-empty and recognizable after truncation;
- `offline` / `pending` remains present;
- no stale metric percentage is present;
- rendered display-cell width does not exceed terminal width;
- one system's online/offline state does not make another system anonymous.

Do not add screenshot/golden infrastructure.

### Step 2: correct condensed status-row budgeting narrowly

Primary file:

```text
crates/gregg/src/ui/condensed.rs
```

Keep `CondensedTableLayout` and online column geometry intact.

Choose the smallest implementation that guarantees identity + status. Acceptable shapes include:

```text
deadpool  offline
192.168.182.146  pending
```

with width-aware truncation of the identity when needed.

If `status_line()` is independent of numeric columns, calculate its available name budget directly from the row width and display width of `"  {status}"` rather than from `layout.host_width`.

If the implementation retains `layout.host_width`, include all configured display names (online/offline/pending) in HOST-width calculation and still prove the status suffix does not erase identity.

Do not change online column start/end positions except where a wider legitimate fleet HOST name requires them.

### Step 3: centralize drive-table structural width accounting

Primary file:

```text
crates/gregg/src/ui/text.rs
```

Introduce narrowly scoped constants or helpers for the structures actually emitted, for example:

```rust
const DRIVE_INDENT_CELLS: usize = 2;
const DRIVE_GAP_CELLS: usize = 2;
const DRIVE_SLASH_CELLS: usize = 3; // " / "
```

or equivalent.

Use those same structural definitions when:

- calculating Full natural width;
- calculating Full truncated-name budget;
- calculating Compact natural/truncated-name budget;
- calculating Minimal name budget;
- rendering the corresponding row.

The exact names are not important. The invariant is that fit math and emitted text cannot disagree about fixed-width structure.

Do not change byte formatting, percentages, remaining semantics, eligibility filtering, or drive ordering.

### Step 4: make Compact consider name truncation before falling to Minimal

Primary file:

```text
crates/gregg/src/ui/text.rs
```

Do not gate Compact solely on the untruncated natural name width.

Instead calculate Compact's fixed width first:

```text
indent + remaining + percent + required gaps
```

Then derive a name budget from the remaining cells. If at least one useful name cell is available, choose Compact and truncate/pad the name to that budget.

Only choose Minimal when Compact's fixed fields plus a usable name cannot fit.

Retain percentage visibility in Minimal at supported TUI widths.

### Step 5: add exact-boundary drive-layout tests

Primary files:

```text
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/mod.rs
```

Required cases:

#### Full exact-fit boundary

Construct one or more rows, determine their exact natural Full rendered width in terminal cells, and assert:

- width == exact rendered width -> Full;
- rendered line width <= requested width;
- width one cell smaller -> either Full with a one-cell smaller name or the next valid degradation mode, but never an overflowing Full row.

Do not hard-code a magic width if the test can derive it from the fixture/layout helpers.

#### Compact truncation boundary

Use a deliberately long mount name and a width where:

- Full cannot fit even with a usable name;
- Compact fixed fields fit with a truncated name;
- Minimal also could fit.

Assert Compact is selected, `(remaining)` survives, percentage survives, name is truncated, and the line is width-bounded.

#### Minimal boundary

Use a width where Compact genuinely cannot fit but Minimal can. Assert Minimal is selected and the rendered line remains bounded.

#### Unicode boundary

Retain/strengthen a Unicode drive-name case so exact-fit decisions are based on display cells rather than UTF-8 bytes.

Also strengthen `full_mode_renders_complete_columns`: do not calculate alignment positions and discard them. Assert the used/separator/total/remaining/percent columns actually agree across rows.

### Step 6: make suffix resolution use fleet label geometry

Primary file:

```text
crates/gregg/src/ui/system_block.rs
```

Change the per-system suffix resolver API so it consumes the fleet layout, or at minimum the fleet `label_width`, rather than recomputing a local maximum label width.

Preferred shape:

```rust
fn resolve_system_suffixes(
    rows: &[MetricRow; 4],
    width: u16,
    layout: &MetricFleetLayout,
) -> [String; 4]
```

Then derive the prefix width from `layout.label_width` and `layout.bar_width` exactly as `render_metric_row()` does.

Avoid creating two separate prefix-width formulas. If a tiny helper such as `metric_prefix_width(label_width)` removes duplication, use it.

Do not change `MetricFleetLayout` population, bar filling, percentage formatting, or the DISK data values.

### Step 7: add mixed-label exact-boundary renderer coverage

Primary file:

```text
crates/gregg/src/ui/mod.rs
```

Existing tests already prove mixed Linux/Windows bracket alignment at representative widths. Add one test aimed specifically at suffix-budget boundaries.

Construct a mixed fleet where:

- Windows contributes `COMMIT`, establishing the wider fleet label column;
- the Linux system has a suffix/detail whose natural form is near the available boundary;
- the terminal width is chosen so using Linux's local `SWP` label width would preserve more suffix text than the actual fleet geometry can fit.

Required assertions:

- every rendered metric line is <= terminal width in display cells;
- brackets remain aligned;
- suffix detail is deliberately dropped/truncated according to the resolver rather than clipped by the terminal backend;
- changing the Linux label from the fleet's structural perspective does not change its available suffix budget.

Do not require a screenshot or fixed full-line golden.

### Step 8: rerun the existing Plan 085 behavior tests

Ensure the corrective changes do not regress already-correct behavior:

- cross-device normal bracket alignment with different suffix widths;
- mixed Linux/Windows bracket alignment;
- off-viewport stability;
- normal DISK `used / total` denominator;
- explicit availability in expanded `(remaining)`;
- legacy remaining fallback;
- drive column alignment across mixed units;
- vertical clipping stability;
- condensed header/value alignment;
- no right-edge HOST inflation for online rows;
- tier degradation;
- Unicode nickname alignment.

Do not rewrite those tests wholesale. Adjust only where the corrected status/width behavior requires it.

### Step 9: reconcile Plan 085 and the plan index after product verification

Files:

```text
plans/085-fleet-wide-tui-column-and-storage-display-correction.md
plans/086-plan085-renderer-boundary-corrective-pass.md
plans/README.md
```

After the corrections pass:

- record the Plan 086 implementation SHA and exact focused/default local checks;
- mark Plan 086 complete;
- update Plan 085 to state that its main implementation landed in `f8be3cf2...`, clippy cleanup landed in `29945c32...`, and post-implementation boundary review found the three defects corrected by Plan 086;
- mark Plan 085 as complete only when Plan 086 acceptance criteria pass;
- reconcile Plan 085 checkboxes based on actual demonstrated behavior rather than merely checking every box wholesale;
- update `plans/README.md` to list Plan 085 as completed through corrective Plan 086 and Plan 086 as complete;
- preserve historical commit messages and completed Plan 067/083/084 records.

Do not create Plan 087 solely for closure bookkeeping.

## Expected production-code surface

Primary:

```text
crates/gregg/src/ui/condensed.rs
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/mod.rs
```

Planning record after implementation:

```text
plans/085-fleet-wide-tui-column-and-storage-display-correction.md
plans/086-plan085-renderer-boundary-corrective-pass.md
plans/README.md
```

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
Cargo.toml
Cargo.lock
```

If implementation begins changing those areas, first establish a separate concrete defect that requires it. The known issues are renderer geometry/budgeting defects only.

## Explicit non-goals

Do not introduce or redesign:

- drive collection or protocol capacity semantics;
- `available_bytes` behavior;
- binary versus decimal storage units;
- new storage fields, SMART data, topology, or sorting;
- endpoint/nickname configuration semantics;
- polling, scheduler, viewport, or selection behavior;
- a general table/layout framework;
- horizontal scrolling;
- mouse controls;
- colors/themes;
- new TUI panes;
- new dependencies;
- new CI workflows, jobs, matrices, or evidence artifacts;
- release automation;
- snapshot/golden/screenshot testing infrastructure;
- unrelated refactoring.

## Focused verification

Run the smallest useful checks first. Exact test filters may vary with implementation names.

Expected focused checks:

```bash
cargo fmt --all -- --check
cargo test -p gregg condensed
cargo test -p gregg drive_detail
cargo test -p gregg fleet
cargo test -p gregg ui
./scripts/check-local.sh
```

If those name filters do not match the final test names, run the nearest exact module/test filters and record the commands actually used.

The default local check is the completion gate for this client-only corrective pass. A release preflight is not required.

Do not add CI. If the existing workflow runs naturally after push, it should remain green, but native macOS/Windows CI is not required to establish these normalized-renderer geometry corrections. One ordinary existing run may be recorded if it occurs; it is not a standing requirement.

## Acceptance criteria

### Condensed offline/pending identity

- [x] An offline system with a configured nickname renders a recognizable non-empty nickname plus `offline` at representative widths.
- [x] An offline system without a configured nickname renders a recognizable endpoint host plus `offline`.
- [x] Pending rows preserve identity plus `pending` under the same rules.
- [x] An all-offline/all-pending fleet does not collapse system rows to anonymous status text.
- [x] A longer offline/pending name is not erased merely because online systems have shorter names.
- [x] Offline/pending rows continue to omit stale numeric metrics.
- [x] Unicode identities use terminal display-cell width for truncation/padding.
- [x] Status rows remain bounded to the available terminal width.

### Expanded-drive width/degradation correctness

- [x] Full-mode fit calculations include the leading indent and every emitted gap/separator.
- [x] A row classified as Full never renders wider than the requested width.
- [x] Exact-fit Full boundary is covered by a deterministic unit test.
- [x] One-cell-below-Full boundary degrades/truncates deliberately rather than relying on backend clipping.
- [x] Compact mode considers a truncated name before falling to Minimal.
- [x] A long-name fixture proves Compact survives when its fixed remaining/percent fields fit.
- [x] Minimal is selected only when Compact genuinely cannot fit with a usable name.
- [x] Compact and Minimal rows are width-bounded.
- [x] `(remaining)` semantics remain explicit availability first, legacy `total - used` fallback second.
- [x] Percentage remains `used / total`.
- [x] Existing mixed-unit, Unicode, and vertical-clipping column alignment remains intact.
- [x] The helper-level alignment test asserts the positions it computes instead of discarding them.

### Fleet suffix-budget consistency

- [x] Per-system suffix resolution uses the fleet-wide label width chosen by `MetricFleetLayout`.
- [x] Suffix budgeting and final rendering share the same structural prefix width.
- [x] Mixed `SWP`/`COMMIT` fleets remain bracket-aligned.
- [x] An exact-boundary mixed-platform test proves suffix detail is dropped/truncated before rendering overflow occurs.
- [x] All affected rendered metric rows remain width-bounded in terminal cells.
- [x] Normal DISK remains `percentage used / total`.
- [x] Off-viewport participation in fleet geometry remains unchanged.

### Scope, verification, and record closure

- [x] Production changes remain confined to the Gregg UI renderer/tests unless a new concrete defect is separately documented.
- [x] No daemon, protocol, collector, normalized-capacity, scheduler, CLI, endpoint, dependency, workflow, or release change is introduced.
- [x] Existing Plan 085 renderer behavior that was already correct remains covered and passing.
- [x] Focused renderer tests pass.
- [x] `./scripts/check-local.sh` passes.
- [x] Plan 085's status/checklist is reconciled only after the corrective behavior is demonstrated.
- [x] Plan 086 records the actual implementation SHA and verification commands before being marked complete.
- [x] `plans/README.md` accurately reflects Plan 085 closed through Plan 086.
- [x] No closure-only Plan 087 is created.

## Handoff guidance

Treat this as a corrective patch, not another UI redesign. The implementing agent should begin with failing regression tests for the three defects, make the smallest geometry changes necessary, rerun the existing Plan 085 tests, then reconcile the planning records.

Recommended execution order:

```text
1. lock condensed offline/pending identity failure with tests;
2. fix status-row width budgeting;
3. lock drive Full/Compact exact-boundary failures with tests;
4. centralize drive structural width math and correct degradation;
5. lock mixed COMMIT/SWP suffix-budget boundary with a test;
6. make suffix resolution consume fleet label geometry;
7. rerun existing Plan 085 renderer tests and default local check;
8. reconcile Plans 085/086 and plans/README.md.
```

Report at handoff:

- files changed;
- exact root cause/fix for each of the three renderer defects;
- focused test commands/results;
- default local check result;
- any naturally occurring existing CI run, without making it a new requirement;
- final Plan 085/086 status.

## Completion record

Implemented in commit `c69498a`. Files changed:

- `crates/gregg/src/ui/condensed.rs` — include all system names in HOST width, decouple status-row width from online numeric tables, add identity regression tests.
- `crates/gregg/src/ui/text.rs` — centralize structural width constants (`DRIVE_INDENT_CELLS`, `DRIVE_GAP_CELLS`, `DRIVE_SLASH_CELLS`), rewrite `compute_drive_table_layout` to use them, restructure Compact fallback to consider truncated names before Minimal, add exact-boundary tests.
- `crates/gregg/src/ui/system_block.rs` — change `resolve_system_suffixes` to accept the fleet layout, add `metric_prefix_width` helper, add mixed-label fleet suffix budget test.

Root cause and fix per defect:

1. **Condensed offline/pending identity collapse.** `compute_condensed_table_layout` only fed online systems into the HOST width pool, so an all-offline fleet left HOST at the heading width (4 cells). Decoupled status-row name budgeting from the online numeric table: HOST width now includes every system name (online/offline/pending), and `status_line()` consumes the full row width (`layout.total_width`) rather than `layout.host_width` so the status never consumes the identity budget.
2. **Expanded-drive structural width mismatch.** `compute_drive_table_layout`'s Full and Compact fit calculations omitted the 2-cell leading indent and the ` / ` separator width was miscounted. Promoted the indent/gap/separator cells to named constants and reused them across fit math and renderer. Restructured the Compact fallback to compute the fixed Compact width first, then derive a name budget, so a long mount name that fits Compact after truncation no longer falls to Minimal.
3. **Mixed-label suffix-budget mismatch.** `resolve_system_suffixes` recomputed the local label width from the current system's four rows, so a Linux `SWP` system in a mixed fleet budgeted the suffix as if its label column were narrower than the fleet's `COMMIT` column. Changed the resolver to consume the fleet `MetricFleetLayout` (factoring the fleet `label_width` through the new `metric_prefix_width` helper), so suffix budgeting and final rendering share the same structural prefix width.

Focused test commands and results (all run from repository root):

```bash
cargo fmt --all -- --check
cargo test -p gregg --lib condensed::tests
cargo test -p gregg --lib text::tests
cargo test -p gregg --lib system_block::tests
cargo test -p gregg --lib
./scripts/check-local.sh
```

All 448 `gregg` lib tests pass (2 ignored). The default local check passes. A naturally occurring CI run may be recorded below once the push completes; it is not a standing requirement.

Final status: Plan 085 is closed through Plan 086; Plan 086 is complete. No closure-only Plan 087 was created.
