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
| `main` | `src/main.rs` | Entry point, event loop (`tokio::select!` biased), TUI wiring |
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `eggpool` |
| `config` | `src/config.rs` | Config model, validation, atomic I/O, cross-process locking |
| `state` | `src/state.rs` | `AppState` reducer, viewport logic, display order |
| `action` | `src/action.rs` | `Action` enum (13 variants) |

### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, `PollOutcome` (12 variants) |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, `SchedulerCommand` enum, generation-based concurrency |
| `endpoint` | `src/endpoint.rs` | Endpoint parsing: IPv4, IPv6, DNS; HTTP URL convenience adapter |
| `clock` | `src/clock.rs` | Clock trait; `RealClock` and `FakeClock` for testing |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI; `aggregate_drives()` |

### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs` | Key-to-action translation (Vim-style); 18 test cases |
| `input` | `src/input.rs` | Crossterm event stream adapter; dedicated thread, bounded channel |
| `terminal` | `src/terminal.rs` | Terminal lifecycle (raw mode, alt screen, cursor hiding, panic hook) |

### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs` | Render dispatcher; dispatches on `active_pane` and `system_view_mode` |
| `ui/layout` | `src/ui/layout.rs` | Viewport computation (visible systems, rect positions) |
| `ui/system_block` | `src/ui/system_block.rs` | Normal-view system rendering (5-row blocks) |
| `ui/condensed` | `src/ui/condensed.rs` | Condensed one-row fleet view (Wide/Medium/Narrow/Minimal tiers) |
| `ui/bar` | `src/ui/bar.rs` | Reusable ASCII usage bar widget |
| `ui/text` | `src/ui/text.rs` | Text formatting (bytes, percentages, load averages) |
| `ui/diagnostics` | `src/ui/diagnostics.rs` | Empty-config and terminal-too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering |

## Architecture

### Event loop

The main event loop uses `tokio::select!` biased to process:
1. **Poll batches** from the scheduler → apply to state
2. **EggPool results** from the worker → apply to state
3. **User input events** from crossterm → translate to actions → apply to state

After every state change, the TUI renders.

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

**Poller** (`poller.rs`):
- v2-first, v1 fallback only on 404
- 64 KiB body cap, no redirects
- `PollOutcome`: 2 success (`Online`/`OnlineV2`), 10 failure/cancellation

**Normalization** (`normalized.rs`):
- v1 and v2 wire formats → `NormalizedSnapshot` with capability flags
- Eliminates version-branching in the UI

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
    eggpool: Option<EggpoolState>,
}
```

**Display order:** Online systems first (stable order), then offline/pending.
**Viewport:** Computes visible range for mixed-height entries; selected system always visible.
**First-batch snap:** `AppState::apply_batch` snaps `selected_id` and
`viewport_top_id` to `display_order()[0]` only when
`last_applied_generation == 0` before the batch is applied. Later
batches and `Ctrl-R` reloads preserve ordinary selection/viewport.

### Key bindings

| Key | Action |
|-----|--------|
| `j`/`k` | Move down/up |
| `h`/`l` | Previous/next pane |
| `v` | Toggle normal/condensed view |
| `e` | Toggle drive expansion |
| `g`/`G` | First/last system |
| `f`/`b` | Page forward/back |
| `Ctrl-R` | Reload Systems config and replace/poll endpoints; on EggPool, refresh pane |
| `q`/`Esc`/`Ctrl-C` | Quit |

### Width degradation

Header line drops lower-priority segments as width decreases:
- < 32 cols: no load
- < 50 cols: no OS
- < 80 cols: no architecture

Condensed-view column priority (Wide ≥ 64, Medium 48-63, Narrow 30-47,
Minimal < 30) drops IOWAIT before LOAD before DISK before MEM.

### UI views

**Normal view** (`ui/system_block.rs`): 5-row blocks per system:
1. Header (name, IO, load, cores, OS, kernel, arch)
2. CPU bar
3. MEM bar
4. SWP or COMMIT bar (platform-dependent)
5. DISK aggregate bar + optional drive detail rows

The four metric rows share one fleet-wide label width and one
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

**Offline rendering** (`ui/system_block.rs::render_offline`):
- configured client name set:  `name@host:port offline`
- no configured name:          `host:port offline`
The host is never duplicated when a name is configured.

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

```toml
config_version = 1
refresh_seconds = 2
request_timeout_ms = 5000
max_concurrent_requests = 64
# Retained for configuration compatibility; `gregg add` requires an explicit port.
default_port = 11310

[[systems]]
id = "550e8400-e29b-41d4-a716-446655440000"
host = "web-01.example.com"
port = 11310
name = "Web Server 01"
```

Cross-process locking: `flock(2)` (Unix) / `LockFileEx` (Windows) on `<config>.lock`.

## Ctrl-R config reload

The Systems-pane `Ctrl-R` reloads the already-resolved `ConfigStore`, derives the replacement endpoint vector, and awaits delivery through the bounded scheduler command channel. A full channel creates backpressure; a closed receiver returns through the TUI error boundary. Failed config loads retain last-known-good state.

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

- Unit tests in every module (130+ total)
- `mixed_fleet_evidence.rs` — integration test with Python fixture servers
- `sustained_workload.rs` — `#[ignore]`, exercises full polling loop
- `FakeClock` for deterministic testing
- Cross-process lock contention covered by `lock_helper` binary behind `test-helper` feature

## Deep dive

See `architecture/gregg-client.md` for the full client architecture document.
