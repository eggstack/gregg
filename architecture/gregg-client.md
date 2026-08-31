# gregg client deep dive

The client crate is the user-facing TUI application that monitors one or more
`greggd` instances. It manages endpoints via CLI, polls them over HTTP, and
renders a Ratatui-based terminal UI.

**Source:** `crates/gregg/`

## Purpose

- Manage monitored endpoints (add, remove, list, edit)
- Poll multiple greggd instances concurrently
- Reduce poll results into application state
- Render a terminal UI with normal and condensed fleet views
- Support an optional EggPool summary pane

## Module map

### Core

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Entry point, event loop, TUI wiring |
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `eggpool` |
| `config` | `src/config.rs` | Config model, validation, atomic I/O, cross-process locking |
| `state` | `src/state.rs` | AppState reducer, viewport logic |
| `action` | `src/action.rs` | Action enum (14 variants including `Resize` and Plan 087's `ClearSelectionHighlight`) |

### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, PollOutcome classification |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, generation-based concurrency |
| `endpoint` | `src/endpoint.rs` | Canonical IPv4/IPv6/DNS endpoint parsing plus HTTP URL adaptation for `add` |
| `clock` | `src/clock.rs` | Clock trait for deterministic testing |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI consumption |

### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs` | Key-to-action translation (Vim-style) |
| `input` | `src/input.rs` | Crossterm event stream adapter |
| `terminal` | `src/terminal.rs` | Terminal lifecycle (raw mode, alt screen, panic hook) |

### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs` | Render dispatcher |
| `ui/layout` | `src/ui/layout.rs` | Viewport computation |
| `ui/system_block` | `src/ui/system_block.rs` | Normal-view system rendering |
| `ui/condensed` | `src/ui/condensed.rs` | Condensed one-row fleet view |
| `ui/bar` | `src/ui/bar.rs` | Reusable usage bar widget |
| `ui/text` | `src/ui/text.rs` | Text formatting (bytes, percentages) |
| `ui/diagnostics` | `src/ui/diagnostics.rs` | Empty-config, too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering |

### EggPool

| Module | File | Purpose |
|--------|------|---------|
| `eggpool` | `src/eggpool.rs` | EggPool summary client and background worker |
| `eggpool_endpoint` | `src/eggpool_endpoint.rs` | EggPool-specific endpoint parsing |

### Test modules

| Module | File | Purpose |
|--------|------|---------|
| `mixed_fleet_evidence` | `src/mixed_fleet_evidence.rs` | Integration test with Python fixtures |
| `sustained_workload` | `src/sustained_workload.rs` | Long-running regression test |

## Architecture

### Event loop

The main event loop in `main.rs` uses `tokio::select!` biased to process:

1. **Poll batches** from the scheduler → apply to state
2. **EggPool results** from the worker → apply to state
3. **User input events** from crossterm → translate to actions → apply to state
4. **Highlight deadline** (`tokio::time::Sleep` arm) — when armed, the loop dispatches `Action::ClearSelectionHighlight` and re-renders so the reverse-video styling disappears even when no other event fires

After every state change, the TUI renders.

The highlight deadline is the only transient timer the loop owns.
Selection-changing Systems actions (`j`/`k`, page movement, `g`/`G`)
arm or reset the deadline to ten seconds from now via
`SELECTION_HIGHLIGHT_DURATION`. Non-selection events (poll batches,
EggPool results, `Resize`, `RefreshNow`, `ToggleSystemView`,
`ToggleDrives`) do not extend the deadline. The `ClearSelectionHighlight`
arm is parked at a far-future sleep while no highlight is active so the
select branch never fires spuriously.

### Action/Reducer pattern

All state changes go through the `Action` enum:

```rust
enum Action {
    MoveDown, MoveUp, PageDown, PageUp,
    SelectFirst, SelectLast,
    PreviousPane, NextPane,
    ToggleSystemView, ToggleDrives,
    RefreshNow,
    ClearSelectionHighlight,   // Plan 087: dispatched by the highlight timer
    Resize, Quit,
}
```

`AppState::apply_action()` and `apply_batch()` are pure, deterministic
functions. The renderer reads `AppState` projections without performing I/O.

### Polling pipeline

```
Config → Endpoint list → PollScheduler → PollBatch channel → AppState reducer
```

**Scheduler** (`scheduler.rs`):
- Produces `PollBatch`es on a configurable interval
- Accepts bounded `Refresh` and atomic endpoint-replacement commands; a replacement polls immediately
- Spawns one isolated poll task per endpoint; a semaphore bounds active polls
- A task panic is converted into that endpoint's `Cancelled` result
- Generation numbers increase monotonically; stale batches rejected
- Fixed-cadence ticks skip missed deadlines, and manual refresh does not reset
  the periodic cadence
- Offline endpoints are kept in the endpoint list and retried on every
  generation; reachability state does not prune or suppress them. The
  regression tests `offline_endpoint_is_retried_and_recovers_on_next_generation`
  and `offline_endpoint_remains_in_scheduler_across_generations` lock in
  that one ordered result per endpoint per generation.

The Systems-pane `Ctrl-R` reloads the already-resolved `ConfigStore`, derives
the replacement endpoint vector, and awaits delivery through the bounded
scheduler command channel before reconciling `AppState`. A full channel creates
backpressure rather than dropping a replacement; a closed receiver returns
through the TUI's normal error boundary. Failed config loads retain the
last-known-good state, issue an ordinary best-effort refresh, and display the
reload error in the existing diagnostic line until a later reload succeeds.

Plan 070 evaluated replacing the per-endpoint tasks and semaphore with a
buffered future stream. That candidate was rejected because it would remove
task isolation and the panic-to-`Cancelled` guarantee while still needing
explicit endpoint ordering and cancellation handling. The current bounded
design is retained intentionally.

**Poller** (`poller.rs`):
- v2-first, endpoint-bound schema parsing, v1 fallback only on 404
- Accepts only the schema matching the requested endpoint; malformed, invalid, and wrong-version responses never trigger fallback
- 64 KiB body cap, no redirects, bounded connection pool
- `PollOutcome` classifies 12 outcome variants (2 success: `Online`/`OnlineV2`, 10 failure/cancellation)

**Normalization** (`normalized.rs`):
- v1 and v2 wire formats → `NormalizedSnapshot` with capability flags
- Eliminates version-branching in the UI
- `aggregate_drives()` with checked arithmetic

### State model

```rust
struct AppState {
    systems: Vec<SystemState>,           // per-system state
    selected_id: Option<SystemId>,       // current selection
    viewport_top_id: Option<SystemId>,   // scroll position (first visible)
    last_applied_generation: u64,        // stale batch rejection
    refresh_status: RefreshStatus,       // idle or polling
    terminal_size: Option<(u16, u16)>,   // terminal dimensions
    active_pane: Pane,                   // Systems or Eggpool
    system_view_mode: SystemViewMode,    // Normal or Condensed
    drives_expanded: bool,               // drive detail rows visible
    selection_highlight_active: bool,    // transient reverse-video highlight
    eggpool: Option<EggpoolState>,       // EggPool pane state (None if unconfigured)
}
```

**Display order:** Online systems first (stable order), then offline/pending.
**Viewport:** Computes visible range for mixed-height entries (normal = 5 rows,
condensed = 1 row). Selected system is always visible.

**First-batch snap:** `AppState::apply_batch` snaps `selected_id` and
`viewport_top_id` to `display_order()[0]` only when `last_applied_generation
== 0` before the batch is applied (the first accepted poll batch).
Subsequent batches preserve the existing selection/viewport semantics.
`Ctrl-R` does not re-snap.

**Visual vs. logical selection (Plan 087):** `selected_id` is the
persistent logical selection that drives `e` (drive expansion) and
viewport behavior. `selection_highlight_active` is the transient
visual-highlight flag that drives the reverse-video styling. Startup
sets both: the logical selection is deterministic but the highlight
is `false`, so the renderer never opens with a reversed row.
Selection-changing Systems actions (`j`/`k`, page movement, `g`/`G`)
set the highlight to `true`; the event loop arms a one-shot ten-second
deadline. When the deadline fires, the loop dispatches
`Action::ClearSelectionHighlight`, which flips the flag back to
`false` without touching `selected_id`. Pane changes away from Systems
also clear the flag immediately so a stale reverse-video row cannot
reappear when the operator comes back.

### Terminal lifecycle

- `terminal.rs` — raw mode, alternate screen, cursor hiding, panic hook
- `input.rs` — dedicated thread reading crossterm events, bounded channel
- Restore on normal quit, error, signal, and panic paths

### Key bindings (Vim-style)

| Key | Action |
|-----|--------|
| `j`/`k` | Move down/up |
| `h`/`l` | Previous/next pane |
| `v` | Toggle normal/condensed view |
| `e` | Toggle drive expansion |
| `g`/`G` | First/last system |
| `f`/`b` | Page forward/back |
| `Ctrl-R` | Reload Systems config and reliably replace/poll endpoints, or refresh EggPool |
| `q`/`Esc`/`Ctrl-C` | Quit |

### Width degradation

The header line drops lower-priority segments as width decreases:
- < 32 cols: no load
- < 50 cols: no OS
- < 80 cols: no architecture

Plan 087 adds a strict integer-safe compact-mode policy for the normal
metric rows: when the longest *natural* suffix across the entire
online fleet satisfies `longest * 4 > terminal_width`, every metric
row in the current render drops the entire suffix region (percentage,
core counts, byte counts). The `[` and `]` columns still align, the
bar gains the cells that would otherwise be the `]` separator, and
resizing wider dynamically restores the suffix without touching
application state. The decision is made per render from
`should_suppress_suffix(width, longest_natural_suffix)` and lives on
the fleet-wide `MetricFleetLayout { label_width, bar_width, show_suffix }`.

Plan 087 also changes the header line: the `IO` token is omitted
entirely (no placeholder, no doubled separator) when the snapshot is
unsupported (`cpu_iowait_supported == false`) or when the
capability is supported but the current `iowait_pct` value is missing.
The UI never infers a zero from a missing measurement.

### UI views

**Normal view** (`ui/system_block.rs`): 5-row blocks per system:
1. Header (name, IO if available, load, cores, OS, kernel, arch)
2. CPU bar
3. MEM bar
4. SWP or COMMIT bar (platform-dependent)
5. DISK aggregate bar + optional drive detail rows

The four metric rows share one fleet-wide label width and one
fleet-wide `bar_width`; their opening `[` and closing `]` always occupy
the same terminal column across every online system. Geometry is
computed once per render via `build_metric_rows`,
`compute_fleet_metric_layout`, and `resolve_system_suffixes`; the
layout population includes every online system with a current
normalized snapshot, not only the entries returned by `compute_viewport`,
so scrolling does not cause horizontal reflow. Metric rows are indented
by exactly four spaces. The disk aggregate suffix is rendered as
`<used bytes> / <total bytes>` so the slash denominator matches the
percentage calculation; explicit caller-available capacity remains
preserved by the normalized model and is surfaced only through the
expanded drive detail rows. Unavailable metrics render `—` rather than
fabricating a `0.0%`. Plan 086 threads the fleet `MetricFleetLayout`
through `resolve_system_suffixes` (via the shared `metric_prefix_width`
helper) so mixed `SWP`/`COMMIT` fleets budget and render suffixes
against the same structural prefix width.

**Offline rendering** (`ui/system_block.rs::render_offline`): When the
configured client name is set the row reads `name@host:port offline`;
otherwise it reads `host:port offline` and never duplicates the host.
The configured client name persists on `SystemEntry.name`; the daemon's
`system.name` is not used for client-side display.

**Expanded drive rows** (`e` in normal or condensed view, shared between
`ui/system_block.rs` and `ui/condensed.rs`): one table layout per
selected system, computed from every eligible drive before the visible
subset is rendered. The full shape is
`<name>  <used> / <total>  (<remaining>) <percent>`. Remaining uses
explicit `available_bytes` when present, otherwise the compatibility
fallback `total_bytes - used_bytes`. Percent always uses
`used / total`. Narrow terminals degrade through Compact
(`name  (remaining) percent`) and Minimal (`name  percent`) without
overflow. Plan 086 centralizes the indent/gap/separator cells as named
constants (`DRIVE_INDENT_CELLS`, `DRIVE_GAP_CELLS`, `DRIVE_SLASH_CELLS`)
shared between the fit calculation and the renderer, and rewrites the
Compact fallback so Compact considers a truncated name before falling to
Minimal.

**Condensed view** (`ui/condensed.rs`): One row per system with tier-appropriate
columns based on terminal width (Wide ≥ 64, Medium 48-63, Narrow 30-47,
Minimal < 30). Header and online rows use one shared
`CondensedTableLayout` (`compute_condensed_table_layout` +
`render_header_line` + `render_online_row`) so headings and values
always occupy the same terminal cell. HOST is the flexible/truncatable
column; numeric columns remain intact whenever the natural fleet widths
fit, and the layout falls back to the next narrower tier before any
numeric column is clipped. Plan 086 widens the HOST budget to include
every visible system name (online/offline/pending) so offline/pending
rows never collapse to anonymous status text, and decouples status-row
width budgeting from the online numeric table so the status never
erases the device identity.

Plan 087 keeps the condensed `IOWAIT` column unchanged: an unsupported
or missing value still renders the unavailable em-dash inside its own
column, distinct from the normal-header `IO` token which is now
omitted entirely.

## Configuration

Endpoint parsing accepts IPv6 link-local zone identifiers in either bare
`fe80::1%eth0` or URL-escaped `%25eth0` spelling. Persisted endpoint hosts use
the URL-safe `%25` separator so the poller can construct valid HTTP URLs.

```toml
config_version = 1
refresh_seconds = 5
request_timeout_ms = 1500
max_concurrent_requests = 16
# Retained for configuration compatibility; `gregg add` requires an explicit port.
default_port = 11310

[[systems]]
id = "550e8400-e29b-41d4-a716-446655440000"
host = "web-01.example.com"
port = 11310
name = "Web Server 01"

[eggpool]
scheme = "http"
host = "localhost"
port = 11300
api_key_env = "EGGPOOL_API_KEY"
```

Platform defaults:
- Linux: `$XDG_CONFIG_HOME/gregg/gregg.toml`
- macOS: `~/Library/Application Support/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

### Cross-process locking

- Unix: `flock(2)` advisory lock on `<config>.lock`
- Windows: `LockFileEx` exclusive lock on `<config>.lock`
- Timeout: 5 seconds

### CLI subcommands

| Command | Purpose |
|---------|---------|
| `add <host:port or http://host:port/> or nickname@host:port>` | Add endpoint with required explicit port; `--name` and an inline `nickname@` are mutually exclusive; persisted fields are normalized `host`/`port` and optional `name` |
| `list` | List configured endpoints |
| `remove <host>` | Host-only remove is still supported |
| `refresh` | Set the global polling interval (seconds) |
| `edit` | Open config in editor |
| `version` | Print client version |
| `eggpool add/list/remove` | Manage the single EggPool endpoint; adding another requires `--replace` and reports a configuration conflict otherwise |

## EggPool

Optional summary pane for EggPool API metrics. Separated from greggd polling.

**Client** (`eggpool.rs`):
- Reuses reqwest stack, disables redirects
- Sends `/api/stats/summary?period=...`
- 16 KiB body cap
- Bearer token from environment variable (never stored in outcomes)

**Worker** (`spawn_worker`):
- Background task with bounded command and result channels
- 60-second passive refresh when active
- Generation-based staleness like greggd polling
- In-flight requests are aborted on superseding commands and shutdown
- Command dispatch uses `try_send` and never blocks the event loop: a
  momentarily full queue drops the command and surfaces `EggpoolStatus::Busy`
  ("worker busy") in the pane; a closed channel still marks the worker
  unavailable

The CLI permits one configured EggPool endpoint. A second `eggpool add`
without `--replace` returns the dedicated `EggpoolAlreadyConfigured`
configuration violation; `--replace` updates the existing entry.

Plan 070 also evaluated replacing the command channel with a latest-state
`watch` channel. It was rejected because refresh nonces, generation ownership,
period changes, deactivation, and request-relative deadlines would remain a
state machine without reducing the production or test surface. The bounded
command channel preserves ordered commands and responsive systems polling.

**Periods:** `Hour`, `Day`, `Week`, `Month` — cycled with `longer()`/`shorter()`

**Summary fields:** accounted tokens, cache read ratio, output tok/s, avg TTFT

## Tests

### Unit tests

Every module has inline `#[cfg(test)]` tests:

| Module | ~Lines | Coverage |
|--------|--------|----------|
| `cli.rs` | 30+ tests | CLI parsing, add/remove/replace, port resolution |
| `config.rs` | 60+ tests | Validation, atomic writes, cross-process locking |
| `state.rs` | 35+ tests | Batch application, selection, viewport, config rebuild |
| `event.rs` | ~21 tests | All key mappings, modifier handling |
| `poller.rs` | 30+ tests | Mock servers for all failure modes |
| `scheduler.rs` | 20+ tests | Generation monotonicity, concurrency bounds |
| `ui/mod.rs` | 40+ tests | Buffer tests for all view modes and widths |

### Integration tests

- `mixed_fleet_evidence.rs` — spawns 9 fixture modes + refused endpoint,
  verifies first-batch outcomes, state transitions, recovery
- `sustained_workload.rs` — `#[ignore]`, runs for configurable duration,
  exercises full polling loop, validates generation invariants

### Test helpers

- `FakeClock` — manually advancing clock for deterministic testing
- cross-process lock contention is covered by the test-only `lock_helper` target, gated behind the private `test-helper` feature
