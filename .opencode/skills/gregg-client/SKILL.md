---
name: gregg-client
description: Work with the gregg client crate (TUI, polling, state engine, CLI)
---

## What I do

Guide agents through the gregg client crate: the TUI application that monitors greggd instances.

## When to use me

Use this when modifying the client's TUI, polling pipeline, state engine, action handling, input processing, or CLI commands.

## Key modules

### Core

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Entry point, event loop (`tokio::select!` biased + 10-second selection-highlight deadline), TUI wiring (update is synchronous, before Tokio) |
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `update` (thin adapter over `gregg-update`), `version`, `eggpool` |
| `update` | `src/update.rs` | Thin CLI adapter over the shared `gregg-update` mechanism (binds program identity, preserves exact outcome strings) |
| `config/*` | `src/config/*.rs` | Config ownership split (façade `src/config.rs` re-exports `crate::config::X`): model entries/limits/primitives, store coordination + atomic persistence + errors, violation kinds, cross-process locking |
| `state` | `src/state.rs` | `AppState` reducer, fleet-aware mixed-height viewport logic, display order, independent drive/network expansions, transient selection highlight, and offline provenance |
| `action` | `src/action.rs` | `Action` enum including `ToggleDrives`, `ToggleNetwork`, and Plan 087's `ClearSelectionHighlight` |

### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, `PollOutcome` (12 variants); `OfflineKind`/`OfflineReason` stable failure provenance |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, `SchedulerCommand` enum, generation-based concurrency |
| `endpoint` | `src/endpoint.rs` | Endpoint parsing: IPv4, IPv6, DNS; HTTP URL convenience adapter |
| `clock` | `src/clock.rs` | Clock trait; `RealClock` and `FakeClock` for testing |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI; `aggregate_drives()` |

### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs` | Key-to-action translation (Vim-style); 20+ test cases |
| `input` | `src/input.rs` | Crossterm event stream adapter; dedicated thread, bounded channel |
| `terminal` | `src/terminal.rs` | Terminal lifecycle (raw mode, alt screen, cursor hiding, panic hook) |

### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs` | Render dispatcher; dispatches on `active_pane` and `system_view_mode` |
| `ui/layout` | `src/ui/layout.rs` | Viewport computation (visible systems, rect positions) |
| `ui/system_block` | `src/ui/system_block.rs` | Normal-view system rendering (legacy 5-row or network-capable 6-row blocks) |
| `ui/condensed` | `src/ui/condensed.rs` | Condensed one-row fleet view with NET-aware Wide/Medium/Narrow/Minimal tiers |
| `ui/bar` | `src/ui/bar.rs` | Reusable ASCII usage bar widget |
| `ui/text` | `src/ui/text.rs` | Text formatting (bytes, percentages, load averages) |
| `ui/diagnostics` | `src/ui/diagnostics.rs` | Empty-config and terminal-too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering |

## Architecture

Client configuration bounds `request_timeout_ms` to 100..=60,000 milliseconds
so a malformed timeout cannot hold bounded polling permits indefinitely.

### Event loop

The main event loop uses `tokio::select!` biased to process:
1. **Poll batches** from the scheduler → apply to state
2. **EggPool results** from the worker → apply to state
3. **User input events** from crossterm → translate to actions → apply to state
4. **Highlight deadline** (`tokio::time::Sleep` arm; Plan 087) — when armed by a selection-changing Systems action, it dispatches `Action::ClearSelectionHighlight` roughly ten seconds later so the reverse-video styling disappears even when no other event fires.

After every state change, the TUI renders. The highlight timer is parked at a far-future sleep when inactive; the select arm never fires spuriously.

### Action/Reducer pattern

All state changes go through the `Action` enum. `AppState::apply_action()` and `apply_batch()` are pure, deterministic functions. The renderer reads `AppState` projections without performing I/O.

### Polling pipeline

```
Config → Endpoint list → PollScheduler → PollBatch channel → AppState reducer
```

**Scheduler** (`scheduler.rs`):
- `SchedulerCommand::Refresh` and `SchedulerCommand::ReplaceEndpoints(Vec<Endpoint>)`
- One isolated poll task per endpoint; semaphore bounds active polls
- Task panic converted to `Cancelled` result
- Generation numbers increase monotonically; stale batches rejected
- Fixed-cadence ticks skip missed deadlines; manual refresh does not reset cadence
- Offline endpoints are kept in the endpoint list and retried on every
  generation; reachability state never suppresses or prunes them. The
  `offline_endpoint_is_retried_and_recovers_on_next_generation` and
  `offline_endpoint_remains_in_scheduler_across_generations` tests in
  `scheduler.rs` lock in one ordered result per endpoint per generation.

Endpoint normalization rejects an IPv6 `%25` zone separator with no zone
name, and invalid hosts are excluded from duplicate-address indexing so one
malformed entry does not produce a misleading duplicate diagnostic.

**Poller** (`poller.rs`):
- v2-first, v1 fallback only on 404
- 64 KiB body cap, no redirects
- `PollOutcome`: 2 success (`Online`/`OnlineV2`), 10 failure/cancellation

**Normalization** (`normalized.rs`):
- v1 and v2 wire formats → `NormalizedSnapshot` with capability flags
- Eliminates version-branching in the UI
- Optional CPU frequency, disk-I/O aggregate/devices, and network
  aggregate/interfaces are copied from v2; v1 and legacy v2 leave them
  absent
- `network_utilization_pct()` evaluates receive/transmit directions
  independently and takes the maximum valid percentage, so full-duplex
  traffic is not double-counted; missing capacity yields no percentage while
  raw throughput remains available
- Formatting (`GHz`, `MiB/s`, and percentage strings) belongs to renderers,
  not normalization

Compatibility is normalized without wire-version renderer branches. v1-only
and pre-feature v2 daemons retain their historical core metrics and simply
provide no live fields. CPU frequency is current OS-reported frequency, not a
base/max claim. Disk capacity is separate from disk I/O; `R/s`/`W/s` and
`Rx/s`/`Tx/s` are byte-throughput labels. Network utilization uses the maximum
valid directional percentage, never `rx + tx`, and loopback may appear in
`n` detail without contributing aggregate capacity. Daemon-version transport
remains deferred.

### State model

```rust
struct AppState {
    systems: Vec<SystemState>,
    selected_id: Option<SystemId>,
    viewport_top_id: Option<SystemId>,
    last_applied_generation: u64,
    refresh_status: RefreshStatus,
    terminal_size: Option<(u16, u16)>,
    active_pane: Pane,              // Systems or Eggpool
    system_view_mode: SystemViewMode, // Normal or Condensed
    drives_expanded: bool,
    network_expanded: bool,
    selection_highlight_active: bool,  // Plan 087: transient reverse-video highlight
    eggpool: Option<EggpoolState>,
}
```

**Display order:** Online systems first (stable order), then offline/pending.
**Viewport:** Computes visible range for mixed-height entries; selected system always visible.
**First-batch snap:** `AppState::apply_batch` snaps `selected_id` and
`viewport_top_id` to `display_order()[0]` only when
`last_applied_generation == 0` before the batch is applied. Later
batches and `Ctrl-R` reloads preserve ordinary selection/viewport.

**Plan 087 logical vs visual selection:** `selected_id` is the
persistent logical selection (drives `e` and viewport behavior).
`selection_highlight_active` is the transient reverse-video flag.
Startup leaves the highlight `false`, so the TUI never opens with a
reversed row. Selection-changing Systems actions arm a resettable
ten-second deadline owned by the event loop. Expiry dispatches
`Action::ClearSelectionHighlight` (or a pane change away from
Systems) without touching `selected_id`. EggPool `j/k` period changes
never activate the Systems-device highlight.

### Key bindings

| Key | Action |
|-----|--------|
| `j`/`k` | Move down/up |
| `h`/`l` | Previous/next pane |
| `v` | Toggle normal/condensed view |
| `e` | Toggle drive expansion |
| `n` | Toggle network detail expansion; legacy systems are a no-op |
| `g`/`G` | First/last system |
| `f`/`b` | Page forward/back |
| `Ctrl-R` | Reload Systems config and replace/poll endpoints; on EggPool, refresh pane |
| `q`/`Esc`/`Ctrl-C` | Quit |

### Width degradation

Header line drops lower-priority segments as width decreases:
- < 32 cols: no load
- < 50 cols: no OS
- < 80 cols: no architecture

Condensed-view tiers (Wide ≥ 64, Medium 48-63, Narrow 30-47, Minimal < 30)
place NET between DISK and LOAD where it fits: Wide has NET/LOAD/IOWAIT,
Medium has NET/LOAD, Narrow has NET, and Minimal keeps HOST/CPU/MEM. Natural
width fallback may narrow the tier or truncate HOST, never numeric cells.

Plan 087 also adds a strict integer-safe compact-mode policy for the
normal metric rows: when the longest *natural* suffix across the
entire online fleet satisfies `longest * 4 > terminal_width`, the
entire suffix region disappears (`MetricFleetLayout::show_suffix =
false`). The `[` and `]` columns stay aligned, the bar gains the
cells that would otherwise be the `]` separator, and resizing wider
restores suffixes dynamically. Plan 087 also omits the normal-header
`IO` token entirely when the snapshot is unsupported or has no real
I/O-wait value, instead of rendering a placeholder.

### UI views

**Normal view** (`ui/system_block.rs`): legacy blocks have five rows; when any
online snapshot exposes network telemetry, all online blocks have six aligned
rows:
1. Header (name, IO if available, load, cores, OS, kernel, arch)
2. CPU bar
3. MEM bar
4. SWP or COMMIT bar (platform-dependent)
5. DISK aggregate bar + optional drive detail rows
6. NET aggregate bar, fleet-wide for mixed old/new systems

The active metric rows share one fleet-wide label width and one
fleet-wide bar width via `build_metric_rows`,
`compute_fleet_metric_layout`, `resolve_system_suffixes`, and
`render_metric_row`. The opening `[` and closing `]` columns always
align across every online system, including mixed `SWP`/`COMMIT`
fleets and across systems with very different suffix widths. Scrolling
the viewport does not change bar columns because the fleet layout is
computed once per render. Rows are indented by exactly four spaces.
The DISK aggregate suffix is rendered as `<used bytes> / <total bytes>`
so the slash denominator matches the percentage; explicit
caller-available capacity remains part of the normalized model and is
surfaced through the expanded drive detail rows. Unavailable rows
render `—` rather than fabricating `0.0%`. Plan 086 threads the fleet
`MetricFleetLayout` through `resolve_system_suffixes` (via the shared
`metric_prefix_width` helper) so mixed `SWP`/`COMMIT` fleets budget
and render suffixes against the same structural prefix width.
Plan 087 extends `MetricFleetLayout` with `show_suffix` and switches
between natural-detail rendering and bar-only rendering from the
length of the longest natural suffix across the whole fleet.

**Offline rendering** (`ui/system_block.rs::render_offline`):
- configured client name set:  `name@host:port offline`
- no configured name:          `host:port offline`
The host is never duplicated when a name is configured. When the accepted
poll failure carries provenance, the stable category is appended inside the
existing width budget (`offline (refused)`, `offline (http) HTTP 503`);
pending rows never carry a reason. Provenance is `OfflineKind`/`OfflineReason`
(`poller.rs`, via `PollOutcome::offline_reason()`), stored in `AppState` and
cleared by accepted successes — never recomputed by the renderer, never
sourced from transport error types.

**Expanded drive rows** (shared between normal and condensed views):
`text::build_drive_detail_row` + `text::compute_drive_table_layout` +
`text::render_drive_detail_row` produce one table layout from every
eligible drive in the selected system. The full shape is
`<name>  <used> / <total>  (<remaining>) <percent>` with explicit
`available_bytes` inside `(...)` when present, otherwise the
compatibility fallback `total_bytes - used_bytes`. The percentage is
always `used / total`. Layouts are computed before the visible subset
is taken so vertical clipping never shifts horizontal columns. Narrow
terminals degrade through Compact (`name  (remaining) percent`) and
Minimal (`name  percent`). Plan 086 centralizes the
`DRIVE_INDENT_CELLS` / `DRIVE_GAP_CELLS` / `DRIVE_SLASH_CELLS`
constants so the fit calculation and renderer share the same
structural cells, and rewrites the Compact fallback so Compact
considers a truncated name before falling to Minimal.

When v2 disk-I/O telemetry is present, `e` adds a heading, optional `R/s` and
`W/s` columns, and an independent `I/O TOTAL` aggregate line. A drive gets a
rate only when exactly one normalized device record names that mount;
ambiguous or missing associations render `—`, and the aggregate is never
recomputed from visible drive rows. `n` independently adds an aggregate
network summary followed by interface rows, including loopback when supplied;
unknown capacity preserves Rx/s and Tx/s while leaving utilization `—`.

**Condensed view** (`ui/condensed.rs`): One row per system with
tier-appropriate columns (Wide ≥ 64, Medium 48-63, Narrow 30-47,
Minimal < 30). Header and online rows share one
`CondensedTableLayout` (`compute_condensed_table_layout` +
`render_header_line` + `render_online_row`) so heading and value
cells line up. HOST is the flexible/truncatable column; numeric
columns stay intact whenever the natural fleet widths fit, and the
layout falls back to the next narrower tier before any numeric column
is clipped. Plan 086 widens the HOST budget to include every visible
system name (online/offline/pending) and decouples status-row width
budgeting from the online numeric table so offline/pending rows never
collapse to anonymous status text.

## Configuration

Endpoint parsing canonicalizes IPv6 link-local zone identifiers to the
URL-safe `%25` separator before persistence, accepting both `%eth0` and
`%25eth0` input spellings.
Bracketed endpoint syntax is reserved for IPv6 literals. URL helpers propagate
host-normalization errors rather than constructing URLs from raw invalid text.

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
```

Cross-process locking: `flock(2)` (Unix) / `LockFileEx` (Windows) on `<config>.lock`.
Config mutation is synchronous and potentially blocking; the CLI runs it before
starting Tokio, and async callers must use a blocking thread.

Only one EggPool endpoint may be configured. `eggpool add` without
`--replace` reports the dedicated `EggpoolAlreadyConfigured` configuration
violation when one already exists.
EggPool URL-normalization and URL-representation failures are reported as
`InvalidEndpoint`, not generic transport failures.

## Ctrl-R config reload

The Systems-pane `Ctrl-R` reloads the already-resolved `ConfigStore`, derives the replacement endpoint vector, and awaits delivery through the bounded scheduler command channel. A full channel creates ordered pending backpressure without blocking input, rendering, or poll-batch processing; a closed receiver returns through the TUI error boundary. Failed config loads retain last-known-good state, issue an ordinary refresh, and display the reload error in the diagnostic line until a later reload succeeds.

## Key constraints

- One ordered result per endpoint, the semaphore limit, panic-to-`Cancelled` conversion, fixed periodic cadence, and cancellation behavior are all intentional.
- EggPool commands remain on a separate bounded channel with generation checks.
- Do not replace either state machine to reduce line count without a smaller behaviorally equivalent design.
- `gregg add` requires an explicit port. Accepted: `host:port`,
  `[ipv6]:port`, `http://host:port/`, and `nickname@host:port`. Rejected:
  host-only (`host`, `192.168.182.146`, `::1`), HTTP URL without a port,
  `nickname@host` without a port, `nickname@`, and inline `nickname@`
  combined with `--name`. HTTPS is never accepted or downgraded. The
  inline `nickname@` form populates the existing `SystemEntry.name`
  field; persisted fields remain normalized `host` and `port`.
- `default_port` remains in the configuration schema for compatibility but is
  not used by `gregg add`, which requires an explicit port.
- `gregg remove` still accepts host-only input.
- Do not introduce implicit-port `gregg add` examples anywhere in the repo.

## Tests

- Unit tests in every module (400+ `#[test]`/`#[tokio::test]`)
- `mixed_fleet_evidence.rs` — integration test with Python fixture servers
- `sustained_workload.rs` — `#[ignore]`, exercises full polling loop
- `FakeClock` for deterministic testing
- Cross-process lock contention covered by `lock_helper` binary behind `test-helper` feature

## Deep dive

See `architecture/gregg-client.md` for the full client architecture document.
