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
| `action` | `src/action.rs` | Action enum (14 state transition triggers) |

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

After every state change, the TUI renders.

### Action/Reducer pattern

All state changes go through the `Action` enum:

```rust
enum Action {
    MoveDown, MoveUp, PageDown, PageUp,
    SelectFirst, SelectLast,
    PreviousPane, NextPane,
    ToggleSystemView, ToggleDrives,
    RefreshNow,
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

The Systems-pane `Ctrl-R` reloads the already-resolved `ConfigStore`, derives
the replacement endpoint vector, and awaits delivery through the bounded
scheduler command channel before reconciling `AppState`. A full channel creates
backpressure rather than dropping a replacement; a closed receiver returns
through the TUI's normal error boundary. Failed config loads retain the
last-known-good state and may issue an ordinary best-effort refresh.

Plan 070 evaluated replacing the per-endpoint tasks and semaphore with a
buffered future stream. That candidate was rejected because it would remove
task isolation and the panic-to-`Cancelled` guarantee while still needing
explicit endpoint ordering and cancellation handling. The current bounded
design is retained intentionally.

**Poller** (`poller.rs`):
- v2-first, endpoint-bound schema parsing, v1 fallback only on 404
- Accepts only the schema matching the requested endpoint; malformed, invalid, and wrong-version responses never trigger fallback
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
| `Ctrl-R` | Reload Systems config and reliably replace/poll endpoints, or refresh EggPool |
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
| `add <host:port or http://URL>` | Add endpoint (with optional name); persist only host and port |
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
- Background task with bounded command and result channels
- 60-second passive refresh when active
- Generation-based staleness like greggd polling
- In-flight requests are aborted on superseding commands and shutdown

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
- cross-process lock contention is covered by the test-only `lock_helper` target, gated behind the private `test-helper` feature
