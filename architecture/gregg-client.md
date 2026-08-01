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
| `main` | `src/main.rs:1-514` | Entry point, event loop, TUI wiring |
| `cli` | `src/cli.rs:1-1267` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `eggpool` |
| `config` | `src/config.rs:1-1600+` | Config model, validation, atomic I/O, cross-process locking |
| `state` | `src/state.rs:1-1415+` | AppState reducer, viewport logic |
| `action` | `src/action.rs:1-133` | Action enum (14 state transition triggers) |

### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs:1-1218` | HTTP client, v2-first/v1-fallback, PollOutcome classification |
| `scheduler` | `src/scheduler.rs:1-1163` | Periodic poll scheduler, generation-based concurrency |
| `endpoint` | `src/endpoint.rs:1-778` | Endpoint parsing: IPv4, IPv6, DNS |
| `clock` | `src/clock.rs:1-100` | Clock trait for deterministic testing |
| `normalized` | `src/normalized.rs:1-310` | Normalized v1/v2 snapshot for UI consumption |

### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs:1-467` | Key-to-action translation (Vim-style) |
| `input` | `src/input.rs:1-238` | Crossterm event stream adapter |
| `terminal` | `src/terminal.rs:1-194` | Terminal lifecycle (raw mode, alt screen, panic hook) |

### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs:1-1391+` | Render dispatcher |
| `ui/layout` | `src/ui/layout.rs:1-85` | Viewport computation |
| `ui/system_block` | `src/ui/system_block.rs:1-248` | Normal-view system rendering |
| `ui/condensed` | `src/ui/condensed.rs:1-222` | Condensed one-row fleet view |
| `ui/bar` | `src/ui/bar.rs:1-97` | Reusable usage bar widget |
| `ui/text` | `src/ui/text.rs:1-156` | Text formatting (bytes, percentages) |
| `ui/diagnostics` | `src/ui/diagnostics.rs:1-59` | Empty-config, too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs:1-283` | EggPool summary pane rendering |

### EggPool

| Module | File | Purpose |
|--------|------|---------|
| `eggpool` | `src/eggpool.rs:1-900` | EggPool summary client and background worker |
| `eggpool_endpoint` | `src/eggpool_endpoint.rs:1-207` | EggPool-specific endpoint parsing |

### Test modules

| Module | File | Purpose |
|--------|------|---------|
| `mixed_fleet_evidence` | `src/mixed_fleet_evidence.rs` | Integration test with Python fixtures |
| `sustained_workload` | `src/sustained_workload.rs` | Long-running regression test |
| `bin/lock_helper` | `src/bin/lock_helper.rs` | Cross-process lock contention helper |

## Architecture

### Event loop

The main event loop in `main.rs` uses `tokio::select!` biased to process:

1. **Poll batches** from the scheduler → apply to state
2. **EggPool results** from the worker → apply to state
3. **User input events** from crossterm → translate to actions → apply to state

After every state change, the TUI renders.

### Action/Reducer pattern

All state changes go through the `Action` enum:

```rust
enum Action {
    MoveDown, MoveUp, PageDown, PageUp,
    SelectFirst, SelectLast,
    PreviousPane, NextPane,
    ToggleSystemView, ToggleDrives,
    RefreshNow, ConfigReloaded(Config),
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
- Concurrency bounded by semaphore
- Generation numbers increase monotonically; stale batches rejected

**Poller** (`poller.rs`):
- v2-first, v1-fallback on 404
- Rejects malformed v2 without fallback
- 64 KiB body cap, no redirects, bounded connection pool
- `PollOutcome` classifies 12 failure modes

**Normalization** (`normalized.rs`):
- v1 and v2 wire formats → `NormalizedSnapshot` with capability flags
- Eliminates version-branching in the UI
- `aggregate_drives()` with checked arithmetic

### State model

```rust
struct AppState {
    systems: Vec<SystemState>,      // per-system state
    selected: SystemId,             // current selection
    viewport: Viewport,             // scroll position
    pane: Pane,                     // Systems or Eggpool
    view_mode: SystemViewMode,      // Normal or Condensed
    drives_expanded: bool,          // drive detail rows visible
    eggpool: EggpoolState,          // EggPool pane state
}
```

**Display order:** Online systems first (stable order), then offline/pending.

**Viewport:** Computes visible range for mixed-height entries (normal = 5 rows,
condensed = 1 row). Selected system is always visible.

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
| `Ctrl-R` | Refresh now |
| `q`/`Esc`/`Ctrl-C` | Quit |

### Width degradation

The header line drops lower-priority segments as width decreases:
- < 32 cols: no load
- < 50 cols: no OS
- < 80 cols: no architecture

### UI views

**Normal view** (`ui/system_block.rs`): 5-row blocks per system:
1. Header (name, IO, load, cores, OS, kernel, arch)
2. CPU bar
3. MEM bar
4. SWAP or COMMIT bar (platform-dependent)
5. DISK aggregate bar + optional drive detail rows

**Condensed view** (`ui/condensed.rs`): One row per system with tier-appropriate
columns based on terminal width (Wide ≥ 64, Medium 48-63, Narrow 30-47,
Minimal < 30).

## Configuration

```toml
config_version = 1
refresh_seconds = 2
request_timeout_ms = 5000
max_concurrent_requests = 64
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
| `add <host:port>` | Add endpoint (with optional name) |
| `list` | List configured endpoints |
| `remove <host:port>` | Remove endpoint(s) |
| `refresh` | Force refresh all endpoints |
| `edit` | Open config in editor |
| `eggpool add/list/remove` | Manage EggPool endpoint |

## EggPool

Optional summary pane for EggPool API metrics. Separated from greggd polling.

**Client** (`eggpool.rs`):
- Reuses reqwest stack, disables redirects
- Sends `/api/stats/summary?period=...`
- 16 KiB body cap
- Bearer token from environment variable (never stored in outcomes)

**Worker** (`spawn_worker`):
- Background task with command channel
- 60-second passive refresh when active
- Generation-based staleness like greggd polling

**Periods:** `Hour`, `Day`, `Week`, `Month` — cycled with `longer()`/`shorter()`

**Summary fields:** accounted tokens, cache read ratio, output tok/s, avg TTFT

## Tests

### Unit tests

Every module has inline `#[cfg(test)]` tests:

| Module | ~Lines | Coverage |
|--------|--------|----------|
| `cli.rs` | 30+ tests | CLI parsing, add/remove/replace, port resolution |
| `config.rs` | 40+ tests | Validation, atomic writes, cross-process locking |
| `state.rs` | 30+ tests | Batch application, selection, viewport, config rebuild |
| `event.rs` | 18 tests | All key mappings, modifier handling |
| `poller.rs` | 25+ tests | Mock servers for all failure modes |
| `scheduler.rs` | 15+ tests | Generation monotonicity, concurrency bounds |
| `ui/mod.rs` | 40+ tests | Buffer tests for all view modes and widths |

### Integration tests

- `mixed_fleet_evidence.rs` — spawns 9 fixture modes + refused endpoint,
  verifies first-batch outcomes, state transitions, recovery
- `sustained_workload.rs` — `#[ignore]`, runs for configurable duration,
  exercises full polling loop, validates generation invariants

### Test helpers

- `FakeClock` — manually advancing clock
- `SyntheticClock` / `SyntheticCollector` — deterministic sampler testing
- `lock_helper` binary — cross-process lock contention testing
