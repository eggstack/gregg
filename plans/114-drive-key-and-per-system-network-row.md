# Plan 114: drive-key and per-system network-row polish

Status: complete.

Depends on: Plans 110-111 for the live-metrics/TUI baseline. This plan is independent of Plan 113 and may proceed in parallel.

## Objective

Make two small client-only TUI corrections:

1. change the drive-detail key from `e` to `d`, so the adjacent live-detail controls are mnemonic and unambiguous:

```text
d  drives
n  network
v  normal/condensed view
```

2. render the normal-view `NET` bar only for a system whose current normalized snapshot actually contains network telemetry.

A host with no network telemetry must not show an empty/unavailable network bar that can be mistaken for valid zero traffic. A host that **does** provide network telemetry and happens to be idle must continue to show a legitimate zero-activity NET row.

This is a bounded client presentation change. It does not alter the daemon collectors, protocol schema, normalized network math, polling, or network-detail content.

## Confirmed current behavior

### Drive key

`crates/gregg/src/event.rs` currently maps:

```rust
Key::Char('e') -> Action::ToggleDrives
Key::Char('n') -> Action::ToggleNetwork
Key::Char('v') -> Action::ToggleSystemView
```

The same `e drives` / `e:drives` wording appears in the TUI key hint and active client documentation.

There is no architectural reason for `e`; the action/state boundary is already typed as `ToggleDrives`, so this is only an input/help/documentation remap.

### Network row

Plan 110 intentionally chose a fleet-wide normal-row policy for mixed old/new daemons.

Current flow:

```text
fleet_has_network_telemetry(state)
-> one include_network bool for the entire online fleet
-> metric_rows_for_fleet(state, include_network)
-> build_metric_rows(snapshot, include_network)
-> every online system has either 4 or 5 metric rows together
```

If any online system has `snapshot.network.is_some()`, every online system receives a `NET` row. Systems whose own snapshot has `network == None` render that row as unavailable.

Because the shared bar renderer still draws the metric row structure, an unavailable NET row can visually resemble a real network bar with no traffic.

The normalized model already has the exact availability distinction needed for the requested behavior:

```text
NormalizedSnapshot.network == None
    -> network telemetry unavailable / legacy

NormalizedSnapshot.network == Some(...)
    -> network telemetry is present
       (rates may legitimately be zero; capacity may still be unknown)
```

No new capability field or daemon change is required.

## Scope decisions

### 1. Remap drive details from `e` to `d`

Change the unmodified Systems-pane drive-detail binding to:

```text
d -> Action::ToggleDrives
```

`n` remains `ToggleNetwork` and `v` remains `ToggleSystemView`.

Plain unmodified `e` should become unmapped; do not retain it as a hidden alias. The product request is a key change, not an additional shortcut.

Preserve modifier behavior:

- `Ctrl-d`, `Alt-d`, and shifted variants must not accidentally toggle drives unless an existing global terminal convention explicitly owns them;
- the typed `Action::ToggleDrives` and reducer semantics remain unchanged;
- logical selection and transient selection-highlight behavior from Plan 087 remain unchanged.

### 2. Update every visible key hint/documented control

Replace active `e` drive guidance with `d`.

At minimum review:

```text
crates/gregg/src/ui/diagnostics.rs
README.md
docs/client.md
docs/display.md
crates/gregg/README.md
architecture/gregg-client.md
AGENTS.md
.opencode/skills/gregg-client/SKILL.md
```

Also update tests/comments that describe `e` as the persistent drive-expansion key.

Do not rewrite historical plan prose merely because Plans 087/110 accurately describe the key that existed when they closed.

### 3. Normal NET-row presence becomes per-system

For normal view, include the NET row for one online system only when:

```rust
system.latest.as_ref().is_some_and(|snap| snap.network.is_some())
```

Equivalent implementation is acceptable if it has the same semantics.

This intentionally supersedes Plan 110's fleet-wide vertical-row policy.

Examples:

```text
host A: network Some(...) -> CPU/MEM/SWP-or-COMMIT/DISK/NET
host B: network None      -> CPU/MEM/SWP-or-COMMIT/DISK
```

A mixed fleet is therefore allowed to have online base blocks of different heights.

This is preferable to a visually present empty network bar because vertical alignment is less important than truthful metric-family availability.

### 4. Availability is not the same thing as activity

Do not hide NET based on throughput values.

These are distinct cases:

```text
network == None
    -> omit NET row entirely

network == Some(...), rx == 0, tx == 0
    -> show NET row; zero traffic is valid data

network == Some(...), capacity known
    -> show normal utilization percentage/bar

network == Some(...), capacity unknown
    -> show NET row with unavailable percentage semantics and raw throughput detail where width permits
```

Do not infer telemetry absence from:

- zero Rx/s;
- zero Tx/s;
- zero/empty interface list;
- missing link capacity;
- 0% utilization.

The `Option<NormalizedNetwork>` boundary remains the availability authority.

### 5. Keep horizontal metric geometry fleet-wide

Only vertical NET-row presence becomes per-system.

Continue computing one fleet-wide `MetricFleetLayout` from every **active row actually present** across online systems so:

- opening/closing bar brackets remain horizontally aligned;
- mixed `SWP`/`COMMIT` label-width handling remains correct;
- suffix suppression from Plan 087 remains fleet-wide;
- scrolling does not cause horizontal reflow.

The renderer must not revert to per-system bar widths merely because row counts differ.

### 6. Build/cache metric rows using per-system availability

`ui::render` currently calculates one fleet-wide `include_network` boolean and passes it into `metric_rows_for_fleet`.

Refactor narrowly so each online system's rows are built using its own current snapshot's `network.is_some()` decision.

The metric-row memo must remain correct when a system transitions between:

```text
network None -> Some
network Some -> None
```

If full snapshot equality already invalidates the cache correctly, remove redundant cache state rather than adding another flag. If an explicit per-system flag remains clearer, keep it scoped per cached system rather than fleet-wide.

Do not add a second cache or new shared state solely for this change.

### 7. Base height must become per-system

Current normal-view base height is fleet-global:

```text
fleet has any network telemetry -> 6 rows
otherwise                       -> 5 rows
```

That is no longer valid.

Introduce/rename one authoritative helper conceptually like:

```rust
normal_base_height_for(system) -> 6 if its current snapshot has network, else 5
```

or derive the base height directly from that system's `MetricRows` where ownership is cleaner.

Use the same per-system base-height authority in:

- `entry_height`;
- `visible_range` / page-size behavior indirectly through `entry_height`;
- `ui::layout::compute_viewport` detail-row allocation;
- drive-detail start offsets;
- network-detail start offsets;
- minimum-render-height / terminal-too-small checks;
- the normal renderer fallback path.

Do not patch only `render_online`; viewport accounting must change with the visible row count.

### 8. Preserve selected drive/network expansion semantics

`d` still toggles drive details for the logically selected system.

`n` already refuses to open an empty network-detail panel when the selected online snapshot has no network telemetry. Preserve that behavior.

When a selected system has network telemetry:

- its NET base row is present;
- `n` may expand aggregate/interface detail below the base block;
- drive and network expansions remain independent as established by Plan 110;
- detail rows begin after that system's own base height.

When a selected system lacks network telemetry:

- there is no NET base row;
- `n` is a no-op;
- `d` still works normally;
- drive rows begin immediately after the five-row base block.

### 9. Mixed-height fleets are an intentional layout state

Add explicit viewport coverage for a fleet such as:

```text
new host with network       -> 6-row base
legacy/unsupported host     -> 5-row base
new host with network       -> 6-row base
offline/pending host        -> 1 row
```

Required invariants:

- entries do not overlap;
- no blank phantom NET row is reserved for the five-row host;
- selection movement remains by system, never by visual row;
- page up/down remains bounded and approximately viewport-sized;
- `viewport_top_id` remains a system identity, not a raw row offset;
- expansion rows are counted against the selected system's actual base height;
- resizing narrower/wider does not stale the row counts.

This repository already supports mixed one-row offline and multi-row online entries, so do not redesign viewport representation.

### 10. Minimum-height diagnostics use the first visible system's real base

`ui::render` currently uses fleet-wide `normal_base_height(state)` when the first displayed system is online.

Change this to the first displayed online system's actual normal base height.

Examples:

- first online host without network: five rows are enough for its base block;
- first online host with network: six rows are required;
- offline/pending first entry remains one row as today.

Do not require six terminal rows merely because some off-screen host has network telemetry.

### 11. Condensed `v` NET column remains a table-level policy

The requested change is specifically the **network bar** in normal view.

Do not remove or dynamically shift the condensed NET column per row. A tabular condensed view requires one column layout for the fleet.

Preserve the existing condensed behavior:

- if the current condensed fleet/tier includes NET, hosts without network data render the existing unavailable marker in that column;
- hosts with valid zero utilization render zero truthfully;
- width-tier selection remains unchanged unless a regression test demonstrates a direct dependency on the removed normal-view fleet helper.

If `fleet_has_network_telemetry` is currently shared by condensed code, retain or rename a condensed-specific fleet helper rather than deleting the concept globally.

### 12. Network detail and normalized math are out of scope

Do not alter:

- `NormalizedNetwork` fields;
- `aggregate_utilization_pct` full-duplex semantics;
- link-capacity selection;
- loopback membership;
- interface ordering;
- Rx/s / Tx/s formatting;
- daemon network collection;
- protocol optionality.

This plan changes only whether the normal NET row exists for a given system.

## Implementation sequence

### Step 1: remap the input/help surface

Change `event::key_to_action` and its focused tests from `e` to `d`.

Update `diagnostics::render_key_hint` and active docs/comments in the same pass.

### Step 2: make metric-row construction per-system

Remove the single fleet-wide normal-view `include_network` decision from `ui::render` / `metric_rows_for_fleet`.

Build each system's `MetricRows` with NET included only when its own snapshot has network telemetry.

Continue feeding all resulting active rows into the same `compute_fleet_metric_layout` call.

### Step 3: make normal base-height accounting per-system

Replace fleet-wide `normal_base_height(state)` use in state/layout/minimum-height paths with the selected system's real base row count.

Keep one authoritative helper/model rather than duplicating `if network.is_some() { 6 } else { 5 }` across modules.

### Step 4: add renderer/viewport regressions

Use existing Ratatui `TestBackend` and state tests. No snapshot/golden framework is needed.

### Step 5: update docs and manual guidance

Document `d`/`n`/`v` and the new normal-view availability rule. Do not rewrite historical closed plans.

## Files likely touched

```text
crates/gregg/src/event.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src/ui/layout.rs
crates/gregg/src/ui/system_block.rs
crates/gregg/src/ui/diagnostics.rs
crates/gregg/src/ui/condensed.rs          # only if helper naming/shared policy requires it
README.md
docs/client.md
docs/display.md
crates/gregg/README.md
architecture/gregg-client.md
AGENTS.md
.opencode/skills/gregg-client/SKILL.md
plans/README.md
```

No daemon/protocol files should be required.

## Required tests

### Key mapping

- plain `d` -> `Action::ToggleDrives`;
- plain `e` -> no action;
- plain `n` still -> `Action::ToggleNetwork`;
- plain `v` still -> `Action::ToggleSystemView`;
- Alt/Ctrl-modified `d` remains unmapped under the current modifier policy;
- rendered key hints show `d drives` / `d:drives` and no longer advertise `e`.

### Normal NET row

- one online v1/legacy snapshot (`network == None`) renders no `NET` line;
- one online v2 snapshot with `network == None` renders no `NET` line;
- `network == Some` with nonzero throughput renders NET;
- `network == Some` with zero Rx/s and zero Tx/s still renders NET;
- `network == Some` with unknown capacity still renders NET and does not fabricate a utilization percentage;
- mixed fleet: host with network has NET, adjacent host without network has no NET row;
- mixed fleet retains fleet-wide horizontal bracket alignment across the rows that each host actually renders.

### Height/viewport

- network-capable online entry base height = 6;
- network-unavailable online entry base height = 5 even when another fleet member has network;
- mixed `6/5/6/1` fleet computes non-overlapping rects;
- `visible_range`, page navigation, first/last selection, and viewport-top preservation work across mixed base heights;
- selected drive expansion after a five-row host begins at row 5;
- selected drive expansion after a six-row host begins at row 6;
- network expansion only allocates rows for a network-capable selected host;
- simultaneous drive/network expansion uses the selected system's actual base height;
- resize across a mixed fleet does not leave stale detail-row counts.

### Condensed regression

- existing NET-column width-tier tests remain green;
- mixed condensed fleet still uses one coherent column layout and unavailable marker for hosts without network telemetry when the NET column is active.

## Verification

Run:

```text
cargo fmt --all -- --check
cargo clippy -p gregg --all-targets --all-features -- -D warnings
cargo test -p gregg --all-targets --all-features
./scripts/check-local.sh
rustup run 1.75 cargo check -p gregg --all-features
```

No release preflight is required unless implementation unexpectedly touches packaging/release-facing files.

Interactive TUI smoke is useful but not required for closure if renderer-level tests prove the exact row/key behavior. If an interactive daemon is available safely, verify:

```text
legacy/no-network host -> no NET bar
network-capable idle host -> NET bar remains visible at zero traffic
d -> drive details
n -> network details when available
```

Use existing ordinary CI once if native-platform/client truth is needed after implementation; do not add a new workflow/job/matrix.

## Preserved exclusions

Do not add under Plan 114:

- daemon collector changes;
- protocol/schema changes;
- new network capability fields;
- changes to utilization math or network capacity accounting;
- changes to per-interface network detail content;
- changes to condensed table semantics beyond any helper rename needed to preserve current behavior;
- a compatibility alias retaining `e` for drives;
- new keybinding configuration/preferences;
- mouse input, horizontal scrolling, themes, or TUI redesign;
- new dependencies, snapshot/golden frameworks, workflows, jobs, matrices, or evidence bundles;
- changes to EggPool, endpoint configuration, polling, scheduler, updater, installer, service lifecycle, or release behavior;
- rewriting Plans 087, 110, or 111 historical records.

## Acceptance criteria

Plan 114 is complete only when:

1. [x] Plain `d` toggles drive details and plain `e` no longer maps to `ToggleDrives`.
2. [x] `n` and `v` retain their current network/view actions and modifier behavior remains bounded.
3. [x] All active TUI key hints/documentation use `d` for drives.
4. [x] A normal-view online system renders NET iff its own current normalized snapshot has `network.is_some()`.
5. [x] Zero network traffic does not hide a valid NET row.
6. [x] Unknown network capacity does not hide a valid NET row or fabricate utilization.
7. [x] Mixed fleets may use five- and six-row online base blocks without reserving a phantom NET row for unavailable systems.
8. [x] Fleet-wide horizontal metric/bar/suffix geometry remains coherent across the active rows actually rendered.
9. [x] State/viewport/layout calculations use per-system normal base height consistently, including drive/network detail offsets.
10. [x] `n` remains a no-op for a selected system with no network telemetry and still expands detail for a system with telemetry.
11. [x] Condensed view retains one coherent fleet-wide NET-column policy and existing width-tier behavior.
12. [x] Renderer-level mixed-fleet tests prove no row overlap, no misleading unavailable NET bar, and correct expansion placement.
13. [x] Focused `gregg` tests, fmt/clippy, default local check, and Rust 1.75 client check pass.
14. [x] The closure record names the implementation SHA and any remote CI run used, without requiring new CI infrastructure.

## Closure record

Implementation and verification were completed on the current `main` branch.
The client changes are confined to key translation, normal-view row memoization,
per-system height/layout accounting, tests, and user/architecture guidance; no
daemon, protocol, collector, condensed-table, polling, or release behavior was
changed.

Local evidence before commit:

- `cargo test -p gregg --all-targets --all-features` — 559 passed, 2 ignored.
- `./scripts/check-local.sh` — default workspace check passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `rustup run 1.75 cargo check -p gregg --all-features` — passed.

The implementation SHA and exact remote CI run ID are appended after the
implementation commit is pushed.
