# Plan 110: TUI clock, disk-I/O, and network integration

Status: ready for implementation after Plan 109.

Depends on: Plans 107-109.

Blocks: Plan 111.

## Objective

Integrate the new optional telemetry into `gregg` without weakening the compact/fleet geometry established by Plans 083-087.

Required user-facing behavior:

- CPU normal-view suffix includes current clock after core count when available, e.g. `20% 16 cores 2.40GHz`;
- normal view gains a `NET` metric row immediately below `DISK`;
- `e` expands drive/storage detail and includes `R/s` and `W/s` throughput columns plus an aggregate throughput line/row;
- `n` toggles a network detail expansion with aggregate throughput/capacity plus per-interface `Rx/s` and `Tx/s`;
- condensed `v` view gains a `NET` column at widths where it fits;
- old daemons or unsupported platforms simply omit unavailable information rather than rendering fabricated zeroes.

## Preserve current UI architecture

Reuse the current typed action/state/render flow:

```text
KeyEvent
 -> Action
 -> AppState reducer
 -> viewport/layout calculation
 -> renderer
```

Do not put input-state mutation directly into renderers.

Reuse existing fleet-wide metric-row geometry and suffix degradation rather than creating separate bar logic for NET.

## Normal-view base block

The current online block is:

```text
header
CPU
MEM
SWP/COMMIT
DISK
```

It becomes:

```text
header
CPU
MEM
SWP/COMMIT
DISK
NET
```

This changes the base online height from five rows to six rows. Update every shared height/viewport calculation that currently assumes four metric rows or five total base rows.

Do not hard-code another unrelated height constant in the renderer. Prefer a named base-row count or a function shared with viewport planning.

## Metric-row representation

The current `build_metric_rows`/`MetricRow` model is fixed around four rows. Generalize it narrowly enough to support five rows without giving up compile-time clarity.

Acceptable implementations include:

- `[MetricRow; 5]`, if the order remains fixed and this is the smallest change;
- a small fixed-capacity vector/list only if it materially simplifies optional rows.

NET itself should still occupy a stable row. When network telemetry or capacity is unavailable, render it as unavailable only if the daemon actually exposes the metric family but no percentage can be derived; for truly legacy daemons, follow the requested compatibility behavior and omit the metric rather than consuming a blank row if the surrounding layout can do so without cross-fleet ambiguity.

Because mixed fleets may have old and new daemons, decide one deterministic fleet policy and test it. Recommended:

- if no online system in the current fleet has network telemetry, keep the historical four-metric-row layout;
- if at least one online system has network telemetry, include NET fleet-wide so bars remain vertically aligned; systems without it show unavailable `—` in that row.

This reconciles “do not display unavailable legacy metrics” with multi-host row alignment: the metric family is absent when the fleet cannot show it at all, but mixed fleets retain stable row positions.

Document this behavior explicitly.

## CPU suffix

Current CPU detail is approximately:

```text
16 cores
```

When `cpu_frequency_hz` is available, render:

```text
16 cores 2.40GHz
```

Formatting rules:

- client owns formatting;
- use GHz with two decimals for ordinary GHz-scale values;
- a sub-GHz value may use MHz if that is materially clearer, but keep output bounded and deterministic;
- no frequency placeholder is appended when absent;
- the existing suffix-degradation policy may drop the detail at narrow widths; do not force GHz to remain visible at the expense of the bar.

Add focused format tests for MHz/GHz boundaries and very large but valid values.

## NET normal row

NET percentage comes from the normalized helper introduced in Plan 108.

Suggested normal row:

```text
    NET  [|||||       ] 42% 39MiB/s rx 5MiB/s tx
```

Exact suffix wording can be kept shorter if needed. The important priorities are:

1. percentage/bar when capacity is known;
2. optional bounded throughput detail when width allows;
3. absence/unavailable semantics when capacity is unknown.

If network throughput exists but capacity is unknown, the row must not show `0%`. It may show an unavailable bar percentage with throughput detail if the existing row model supports that cleanly; otherwise leave the aggregate percentage unavailable and rely on `n` detail for raw rates.

Do not derive percentage from the highest observed historical throughput.

## `e` drive/storage expansion

The current expansion already renders per-drive detail after the DISK row. Extend that table to include throughput without confusing filesystem capacity with physical-device identity.

Recommended presentation shape:

```text
     DRIVE/MOUNT         USED       TOTAL      REMAIN      R/s       W/s
     /                   ...        ...        ...         ...       ...
     /data               ...        ...        ...         —         —
```

Only show a per-drive R/s/W/s value when Plan 109 supplied a trustworthy association. Ambiguous rows show `—`, not a guessed value.

Also expose daemon-computed aggregate disk throughput in the expanded area, conceptually:

```text
     I/O TOTAL                                      92MiB/s   22MiB/s
```

or an equivalent bounded summary.

The aggregate throughput is not computed by summing visible drive rows.

### Responsive table behavior

Preserve the existing shared detail-table layout philosophy:

- compute widths from the full eligible set before vertical clipping;
- use terminal display-cell width, not UTF-8 byte count;
- keep columns aligned across all visible detail rows;
- degrade optional columns in a deterministic order when width is tight.

Recommended degradation order for disk details:

1. full: name + used + total + remaining + percent + R/s + W/s;
2. drop remaining/percent if existing policy already permits it;
3. preserve name + R/s + W/s where possible because `e` now exists partly for live I/O detail;
4. at very narrow widths, fall back to the existing minimal capacity representation rather than overflowing.

Do not silently rename byte throughput to IOPS.

## New `n` network expansion

Add a new typed action:

```text
ToggleNetwork
```

Map unshifted `n` to it.

State should track network-detail expansion in the same ownership layer as drive expansion. Prefer one selected-system-local boolean/set representation consistent with the existing `e` behavior. If `e` and `n` can both be active simultaneously, viewport height must account for both. If the design chooses mutual exclusion to keep vertical growth bounded, make that explicit and test it; do not let one key accidentally clear the other without a documented rule.

Recommended behavior: allow both independently unless existing state/viewport complexity becomes disproportionate. They expose different data and there is no inherent conflict.

### Network detail content

At the top show aggregate totals and capacity when known, followed by interfaces:

```text
     NETWORK TOTAL   RX 39MiB/s   TX 5MiB/s   CAP 1.0Gb/s   31%
     eth0            RX 38MiB/s   TX 4MiB/s   LINK 1.0Gb/s
     wlan0           RX 1MiB/s    TX 1MiB/s   LINK 433Mb/s
     lo              RX 2MiB/s    TX 2MiB/s   LINK —
```

Exact column order may be optimized for width, but include:

- interface name;
- Rx/s;
- Tx/s;
- link capacity when available;
- aggregate summary at the top.

Loopback may appear in detail.

Do not visually imply that an interface excluded from aggregate capacity is broken; aggregate membership is an accounting decision, not necessarily a health state.

### Ordering

Use deterministic order from normalized data, preferably daemon-defined stable ordering. A useful default is aggregate first, then ordinary interfaces sorted by name with loopback following non-loopback, but do not reorder if it would destroy an intentionally stable platform order without benefit.

## Condensed `v` view

Add `Column::Net` and preformatted network percentage.

The condensed value is the aggregate capacity utilization percentage, not raw throughput:

```text
HOST          CPU   MEM   DISK   NET   LOAD   IOWAIT
```

Missing/legacy capacity renders `—` only when the NET column is active for the fleet/tier.

Rebalance tiers after measuring actual natural widths. Do not mechanically add NET to every existing tier if it causes useful columns to disappear too early.

Recommended starting point:

```text
Wide:    HOST CPU MEM DISK NET LOAD IOWAIT
Medium:  HOST CPU MEM DISK NET LOAD
Narrow:  HOST CPU MEM DISK NET
Minimal: HOST CPU MEM
```

Then let the existing natural-width/fallback machinery narrow further when values do not fit.

If testing shows NET should be dropped before LOAD at medium widths, choose the order based on the product goal of this roadmap: live resource saturation is more central than preserving every historical column. Record the final policy in docs/tests.

## Mixed-version rendering

Required scenarios:

### All old daemons

No CPU-frequency suffix, no NET normal row, no useful `n` expansion, historical DISK detail remains valid.

Pressing `n` on a selected legacy system should be a no-op or show no expansion; do not render an empty panel.

### New client + old v2 daemon with drives

Drive capacity continues unchanged. No R/s/W/s columns should be forced if there is no disk-I/O data unless the table policy can show them as optional without degrading legacy readability.

### Mixed old/new fleet

CPU suffix is per-host optional.

Normal NET row follows the deterministic fleet policy described above so vertical bar geometry remains coherent.

Condensed NET column should be included only according to the fleet/tier policy; old hosts show `—` when the column is active because another host supports it.

### New daemon on unsupported platform metric

Treat the same as absence. Do not expose protocol version or OS-specific placeholder text merely to explain it.

## Viewport/selection accounting

This is the highest-risk TUI part of the plan.

Audit every calculation that assumes:

- online base block height is five;
- metric row count is four;
- drive expansion begins at `base_y + 5`;
- selected-system expansion has only one optional detail family.

Update shared viewport entries so:

- scrolling never lands inside the wrong system block;
- page up/down still approximate one viewport;
- first/last selection works;
- expansion for a selected system remains visible when space permits;
- resize cannot cause stale detail-row counts;
- visual-selection timeout from Plan 087 remains independent from logical selection and expansions.

Do not patch renderer offsets without updating viewport planning/tests.

## Keyboard/help/documentation surface

Update any displayed key hints/help text and client docs to include:

```text
e  toggle drive details
n  toggle network details
v  toggle normal/condensed system view
```

Do not overload `n` elsewhere.

## Required tests

### Metric rows

- CPU with and without frequency;
- NET percentage present;
- throughput present but capacity absent;
- all-old fleet omits NET row;
- mixed fleet keeps aligned NET row with `—` for old hosts;
- fleet-wide suffix/bar geometry still aligns with five rows.

### Disk detail

- aggregate R/s/W/s shown from daemon aggregate;
- associated drive shows R/s/W/s;
- ambiguous/unassociated drive shows `—`;
- aggregate is not recomputed by visible-row sum;
- Unicode names and clipping preserve columns;
- width degradation does not overflow.

### Network detail

- `n` action mapping;
- toggle state for selected system;
- aggregate summary;
- physical adapter rows;
- loopback row;
- unknown capacity still shows Rx/s/Tx/s;
- legacy selected system does not open an empty panel;
- `e` and `n` interaction follows documented independent/mutual-exclusion policy.

### Condensed

- NET heading/value alignment;
- wide/medium/narrow/minimal tier behavior;
- mixed old/new fleet `—` behavior;
- long HOST shrink still happens before unsafe overflow;
- terminal widths around each tier boundary.

### Viewport

- one online new-system block with no expansions;
- old-daemon block height under all-old fleet policy;
- drive expansion only;
- network expansion only;
- both expansions if supported;
- offline/pending rows adjacent to expanded online rows;
- resize narrower/wider while expanded;
- page navigation and first/last selection.

Use renderer-level buffer assertions where Plans 083-087 already do so; pure string-helper tests alone are not enough for geometry.

## Files likely touched

```text
crates/gregg/src/action.rs
crates/gregg/src/event.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/condensed.rs
crates/gregg/src/ui/text.rs
crates/gregg/src/ui/* viewport/layout modules
crates/gregg/src/main.rs only where event-loop wiring/help requires it
docs/client.md
docs/display.md
architecture/gregg-client.md
```

## Local verification

Mandatory:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p gregg --all-targets --all-features
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
```

Also run the TUI manually against:

1. one new local daemon with live disk/network activity;
2. one deterministic old-daemon fixture or compatibility server;
3. a mixed fleet containing both;
4. narrow and wide terminal sizes;
5. `e`, `n`, and `v` toggles during resize and selection movement.

## Acceptance criteria

Plan 110 is complete only when:

1. CPU normal suffix renders `<cores> <current frequency>` when frequency exists and omits frequency otherwise.
2. Normal view adds NET directly below DISK according to a documented mixed-fleet row policy.
3. Fleet-wide bar/suffix geometry remains aligned after moving from four to five metric rows.
4. Base block and viewport height calculations are updated centrally; no stale `+5`/four-row assumptions break scrolling.
5. `e` exposes aggregate disk R/s/W/s and per-drive values only where association is trustworthy.
6. `n` toggles a bounded network detail view with aggregate plus per-interface Rx/s/Tx/s and link capacity where available.
7. Loopback can be displayed in network detail without affecting aggregate capacity percentage.
8. Condensed `v` gains a NET utilization column with width-tier fallback that never overflows.
9. Old daemons do not cause empty/fabricated metrics; all-old fleets retain the historical display shape as closely as practical.
10. Mixed old/new fleets remain vertically/horizontally coherent.
11. `e`/`n` expansion state and Plan 087 logical/visual selection behavior do not interfere.
12. Renderer-level width/viewport regressions are covered and full local verification passes.
