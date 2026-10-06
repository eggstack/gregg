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
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `update` (thin adapter over `gregg-update`), `uninstall` (exact-exe removal, `--dry-run`/`--purge`), `version`, `eggpool` |
| `update` | `src/update.rs` | Thin CLI adapter over the shared `gregg-update` mechanism (binds program identity, preserves exact outcome strings) |
| `uninstall` | `src/uninstall.rs` | Component-safe client uninstall: exact-exe plan/render/execute, default config preservation, Unix Cargo-owned package removal followed by post-success purge (Windows handoff remains zero-mutation) via shared `gregg-update` primitives; never inits the TUI |
| `config/*` | `src/config/*.rs` | Config ownership split (façade `src/config.rs` re-exports `crate::config::X`): model entries/limits/primitives, store coordination + atomic persistence + errors, violation kinds, cross-process locking |
| `state` | `src/state.rs` | `AppState` reducer, fleet-aware mixed-height viewport logic, display order, independent drive/network expansions, transient selection highlight, and offline provenance |
| `action` | `src/action.rs` | `Action` enum including `ToggleDrives`, `ToggleNetwork`, and Plan 087's `ClearSelectionHighlight` |
| `sanitize` | `src/sanitize.rs` | Pure terminal-control sanitizer (caret notation); the only function allowed to turn remote bytes into renderable cells |
| `cron` | `src/cron.rs` | `CronCache`/`CronSystemState` local retention: `(epoch, sequence)` dedup, per-job depth, global 4096-record ceiling, union intent reduction |
| `qualification` | `src/qualification.rs` | Derives every Plan-161/167 memory and payload bound from the real constants and asserts them, so the documented numbers cannot drift from the code |

### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, `PollOutcome` (12 variants); `OfflineKind`/`OfflineReason` stable failure provenance |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, `SchedulerCommand` enum, generation-based concurrency |
| `endpoint` | `src/endpoint.rs` | Endpoint parsing: IPv4, IPv6, DNS; HTTP URL convenience adapter |
| `clock` | `src/clock.rs` | Clock trait; `RealClock` and `FakeClock` for testing |
| `clientd/cron` | `src/clientd/cron.rs` | Daemon-owned `/v2/scheduler` client: 30s cadence with one startup round, target-keyed revision/epoch-gated history committed only after delivery, coherent summary+history pair, four reads in flight, reload/cancel-preemptible rounds (remote reads *and* the bounded observation hand-off), `CronWorker` task; the only code that fetches cron |
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
| `ui/cron` | `src/ui/cron.rs` | Cron detail block: one `block_rows` builder feeding both heights, selected-job section reserved before the job window, load relations stated by meaning, UTC-labelled record clocks, elapsed-vs-countdown grammar, explicit viewport truncation, locally escaped text |

## Architecture

Client configuration bounds `request_timeout_ms` to 100..=60,000 milliseconds
so a malformed timeout cannot hold bounded polling permits indefinitely.

### Client daemon (Plan 164) — the TUI is a frontend

`gregg` owns **no** remote polling. A separate foreground process per config,
`gregg daemon run`, holds the endpoint HTTP client, `PollScheduler`, normalized
fleet state, the `EggPool` worker, and the `Ctrl-R` reload boundary. The TUI
reads no config file and opens no network connection.

- **Identity/endpoint:** FNV-1a of the normalized config path, `0600` Unix
  socket beside that config (temp-dir fallback), Windows named pipe with an
  owner-only SDDL and `PIPE_REJECT_REMOTE_CLIENTS`. One daemon per config; two
  configs never share one.
- **Handshake:** carries `PROTOCOL_VERSION` *and* the expected config identity.
  A mismatch is refused, so a TUI is never fed another config's fleet and
  `daemon status`/`stop` identify their target without a PID file.
- **Fan-out:** `watch` slot, serialized **once** per publication. A slow
  frontend skips to the newest generation (documents are complete). Control
  acks ride a small per-connection channel.
- **Ordering:** the `Hello` is always the first frame. Never write a document
  before the handshake is answered — a frontend identifies its daemon by its
  first frame, and the handshake reply already carries the current state.
- **Ownership boundary:** `FleetState` holds the reducers. `AppState` in a TUI
  has no batch/EggPool/reload entry point; `AppState::adopt_snapshot` is its
  only fleet writer. `#[cfg(test)] test_fleet` lets renderer tests drive the
  real publish/adopt path and is absent from production builds.
- **Windows accept is cancellable, not abandoned (Plan 171).** The wait is still
  parked on the blocking pool, so `accept_task.abort()` is *not* sufficient
  teardown on Windows — but the parked thread now publishes a `THREAD_TERMINATE`
  handle to itself and `AcceptStop::request` cancels the wait with
  `CancelSynchronousIo`. `run_daemon` requests the stop and waits for the accept
  loop to unwind before releasing the endpoint. A real handle, never a thread
  **id**: ids are recycled on thread exit, so cancelling by id could interrupt an
  unrelated thread. `AcceptStop` is a no-op on Unix and exported from both
  transports, so keep this shutdown sequence `#[cfg]`-free.
- **A stop must be awaited, not dropped.** `cli.rs`'s signal path used to return
  from `select!`, which dropped `run_daemon`'s future and skipped the entire
  teardown. Any path that stops the daemon must `cancel` **and then await** it.
- **No fallback.** A frontend that cannot reach a compatible daemon reports the
  reason and exits. Never add a direct-polling fallback.

### Lifecycle (Plan 165)

Bare `gregg` ensures the daemon exists, so nobody has to start a background
process first.

1. Bounded handshake probe.
2. **Absent only** -> take the config-specific launch lock (an advisory OS lock
   via `spawn_blocking`, held only across probe/spawn/readiness), **re-probe and
   re-classify** under it, detached-spawn the exact current executable with
   `--config <path> daemon run`, wait boundedly for readiness, attach.
3. Anything else — refusal, silence, malformed frame, foreign peer — is reported
   and starts nothing. A newer owned daemon is reported with upgrade guidance
   rather than killed; an older one is rotated.

Never add a PID file or a global daemon registry. The lock file is never unlinked
and never read as a signal.

**User-scoped startup only** (`clientd::startup`): `systemctl --user`, a
`~/Library/LaunchAgents` agent, a current-user Startup-folder entry, or a managed
user crontab watchdog. Never a system unit, never `LocalService` SCM, never
`sudo`; a root install registers nothing. Render/parse/ownership are pure and
tested; only the manager calls touch the OS, through `startup_support`'s fixed
allowlist with no shell and a bounded wait. `Unknown` ownership is preserved
exactly like `Foreign`.

**Update** identifies, prepares, verifies, then stops/replaces/relaunches, and
reports a failed relaunch as partial success. **Uninstall** plans first (the
plan names the startup entry and the running daemon), stops only an identified
owned daemon, removes only a provably owned entry, and blocks executable
deletion when it cannot confidently stop a running daemon.

### Event loop

The main event loop uses `tokio::select!` biased to process:
1. **User input events** from crossterm → translate to actions → apply to presentation state, forward daemon-bound requests (input before frames so a flood of documents cannot starve `Quit`)
2. **Daemon frames** → `Snapshot` is adopted; `Hello`/`ControlAck` are not render-visible; `ShuttingDown`/`VersionMismatch`/`ProtocolError` exit with the daemon's reason
3. **Highlight deadline** (`tokio::time::Sleep` arm; Plan 087) — when armed by a selection-changing Systems action, it dispatches `Action::ClearSelectionHighlight` roughly ten seconds later so the reverse-video styling disappears even when no other event fires.

The loop draws the initial frame immediately and then gates complete-frame
draws with a local dirty flag. An adopted document sets it when
`adopt_snapshot` sees a render-visible change — endpoint, configured name,
reachability, normalized snapshot, offline reason, EggPool window/worker/data
planes. Timestamps and latency are excluded on purpose, so an advancing age
does not force a frame. A document that is not newer than the last applied one
is skipped entirely. Mapped render-visible actions, resize, and highlight
expiry also set it; unmapped keys and no-op wakeups do not draw. The highlight
timer is parked at a far-future sleep when inactive; the select arm never fires
spuriously. Do not introduce partial-region rendering.

### Action/Reducer pattern

All presentation changes go through the `Action` enum. `AppState::apply_action_changed()` is pure and deterministic and is the only writer of presentation state; `AppState::adopt_snapshot()` is the only writer of fleet state. The renderer reads `AppState` projections without performing I/O. Poll batches, `EggPool` results, and config reloads are reduced by `FleetState` inside the daemon and reach a frontend as complete documents.

### Polling pipeline

```
Config → Endpoint list → PollScheduler → PollBatch channel → FleetState (daemon)
      → encode once → watch::channel<Document> → AppState::adopt_snapshot (TUI)
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
- Dedicated `eggfetch-core` client (lean `standard-http1` 0.2 + Rustls:
  standard route only, redirect following not compiled so 3xx passes through
  with no second hop, four idle per host, explicit whole-request deadline with
  absolute `total` through response-body EOF, no retry)
- 64 KiB decoded-body cap owned by eggfetch; typed `NetworkFailureKind` drives DNS/refused/connect mapping
- Body-stage typed timeouts map to `Timeout`; ordinary post-header body failures stay `NetworkError`
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

The production event loop uses owned normalization constructors for successful
poll payloads, moving identity/detail strings and collections. Borrowed
constructors and `AppState::apply_batch(&PollBatch)` remain compatibility paths.
Ordered batches use positional stable-ID matching first and fall back to the
existing ID search for reordered or synthetic batches; endpoint host/port
validation remains mandatory.

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
**One-time placement:** `AppState::adopt_snapshot` snaps `selected_id` and
`viewport_top_id` to `display_order()[0]` on the first document carrying
`poll_initialized == true` — the first *polled* reachability, not the first
document. The daemon publishes once at bind with every system `pending`, so
keying on the first document would pin selection to the first configured
system and never move it when the first batch reveals that system is offline.
The daemon therefore publishes that one-time transition even when it changed
nothing visible, so the placement cannot be deferred onto an unrelated later
change. Every later document preserves ordinary selection/viewport.

**Plan 087 logical vs visual selection:** `selected_id` is the
persistent logical selection (drives `d` and viewport behavior).
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
| `d` | Toggle drive expansion |
| `n` | Toggle network detail expansion; legacy systems are a no-op |
| `g`/`G` | First/last system |
| `f`/`b` | Page forward/back |
| `Ctrl-R` | Ask the daemon to re-read the config and poll; on EggPool, refresh pane |
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

**Normal view** (`ui/system_block.rs`): each online block has five rows when
its current snapshot lacks network telemetry and six rows when its snapshot
exposes it. Mixed fleets may therefore have different vertical block heights:
1. Header (name, IO if available, load, cores, OS, kernel, arch)
2. CPU bar
3. MEM bar
4. SWP or COMMIT bar (platform-dependent)
5. DISK aggregate bar + optional drive detail rows
6. NET aggregate bar, only for systems with network telemetry

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

When v2 disk-I/O telemetry is present, `d` adds a heading, optional `R/s` and
`W/s` columns, and an independent `I/O TOTAL` aggregate line. A drive gets a
rate only when exactly one normalized device record names that mount;
ambiguous or missing associations render `—`, and the aggregate is never
recomputed from visible drive rows. `n` independently adds an aggregate
network summary followed by interface rows, including loopback when supplied;
unknown capacity preserves Rx/s and Tx/s while leaving utilization `—`.
Both view modes share `valid_drive_detail_count` for entry height, so a legal
v2 payload with `drives: None` plus `disk_io: Some(..)` reserves the same
heading and I/O-total rows in normal and condensed.

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

Condensed layout preparation preformats each online system once per render and
borrows configured names/hosts for width measurement. Normal metric rows use a
renderer-local stable-ID cache with a compact render key, cached suffix forms,
and an index-aligned per-render table for visible entries. Online rows (normal
header and condensed HOST) render the configured name or the bare host
**without** the port, because the condensed HOST column is the most
width-constrained; offline/pending rows keep `name@host:port`.
`CondensedRenderKey` carries the label *and* the port, so a port-only config
edit still invalidates a memoized row.

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
Only contention is retried: the Unix path inspects `errno` and accepts
`EWOULDBLOCK`/`EAGAIN`, surfacing `EBADF`/`EINVAL`/`ENOLCK`/`ENOTSUP` as
`ConfigError::Io` instead of a 5-second `LockTimeout`. Every persistence path
(`mutate`, `mutate_with_result`, `edit_transaction`, and the public
`ConfigStore::write`) takes the same in-process mutex and file lock.
Config mutation is synchronous and potentially blocking; the CLI runs it before
starting Tokio, and async callers must use a blocking thread.

`Config::validate` enforces the same host/name rules as the `gregg add` parsers,
so a hand-edited config fails validation rather than surfacing later as a
`NetworkError` or an unrenderable `nickname@host:port` string: `host` rejects
scheme/path/query/fragment/credentials/brackets/whitespace/control, and `name`
rejects surrounding whitespace, control bytes, and `@ : /` (matching
`endpoint::validate_name`). `gregg eggpool add` accepts IPv6 zone IDs exactly
like `gregg add` and stores the URL-safe `%25` form.

Only one EggPool endpoint may be configured. `eggpool add` without
`--replace` reports the dedicated `EggpoolAlreadyConfigured` configuration
violation when one already exists.
EggPool URL-normalization and URL-representation failures are reported as
`InvalidEndpoint`, not generic transport failures.

## Ctrl-R config reload

Still the only reload boundary; there is no filesystem watcher. The Systems-pane
`Ctrl-R` sends `DaemonRequest::ReloadConfig` over the local channel. The daemon
re-reads the file **from its own resolved startup path** (never a
frontend-supplied one), reconciles retained systems by stable id, swaps or
clears the `EggPool` entry, and republishes. A failed load retains
last-known-good fleet state, still issues an ordinary refresh so a bad file
cannot freeze metrics that were already being collected, and publishes a
diagnostic the renderer shows until a later reload succeeds.

The request rides a bounded channel and is written by the frame reader, so it
interleaves with incoming documents; a full channel is reported, never waited
on. The reload's outcome is visible only in the next document, so `ControlAck`
is not render-visible.

`gregg add` / `remove` / `refresh` send the same request after a successful
mutation, best-effort and silently — absence is the common case and must never
turn a successful mutation into an error.

## EggPool convergence (Plan 164)

The worker is daemon-owned. Each frontend publishes its **whole**
`(active, period, refresh)` intent — a replacement, not a delta — and the daemon
reduces every attached frontend's intent: `active` is true if any frontend has
the pane open, `period` is the shortest window any of them asked for. Both are
order-independent, so two windows cannot race the worker into two activations,
and the last pane leaving always converges it to inactive. Intents are keyed by
the accept loop's stable subscriber id and retired on disconnect, so a frontend
that refreshes often cannot leave a stale active entry behind. `j`/`k` records a
*request* in `AppState::eggpool_period_request`; the pane only ever shows a
period the daemon actually fetched. `refresh: true` is the manual-refresh escape
hatch, without which a re-sent identical intent could not mint a new worker
generation.

The worker's latest-desired-state convergence must stay live **while a completed
result is backpressured**, not only while an HTTP request is in flight. The
bounded four-slot result channel is the one place it parks after a fetch has
already finished, and Plan 176 made only shutdown observable there — so closing
the pane or switching window could sit behind a result slot nobody drains. Deliver
through `deliver_result_or_interrupt`, selecting cancellation, then
`control_rx.changed()`, then the result reservation (`reserve()`, so a lost race
returns the value). Inspect the newest retained state with `borrow_and_update()`:
inactive, or active with a different period/generation, abandons the completed
result and is returned to the worker loop to converge on; an **equivalent**
publication is consumed and the wait continues, because that result still answers
it. Ordering is `biased` — shutdown, then superseding state, then send — and the
passive refresh deadline is armed only after a *delivered* current result. Keep
the channel bounded and one-request-at-a-time; do not make delivery lossy to
"fix" this, and do not add a second desired-state queue: latest-value coalescing
is the contract.

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

## Cron observability (Plan 166)

`c` expands a cron detail block inside the selected system; `Shift-J` and
`Shift-K` move between jobs. The three expansions (`d`, `n`, `c`) are
independent and share one vertical budget.

Ten rules govern this plane and are easy to break:

1. **The intent governs transmission, never fetching.** The client daemon polls
   `/v2/scheduler` on its own cadence whether or not a TUI is attached, and
   fetches `/v2/scheduler/history` only on first support discovery and when
   `history_revision` changes. A frontend's `SetCronIntent` decides which
   retained records ride along in the document and nothing else. A per-TUI
   poller, or making the gate depend on the intent, would multiply the fleet's
   request budget by the number of open windows.
2. **Key scheduler state on `(epoch, sequence)`, never `sequence`.** A restarted
   `greggd` reissues sequences from zero and resets `history_revision` to a small
   value, so a sequence-only identity drops the new epoch's records as duplicates
   and a revision-only gate concludes "nothing changed" and never fetches the new
   epoch at all.
3. **Nothing in the cron plane may reach `Reachability`.** A 404 is a healthy
   older daemon (`Unsupported`), not an error; a transport or 5xx failure is
   `Failed` and retains the last known data; a document that fails its own wire
   validation is `Invalid`, because a daemon producing a document that breaks its
   own contract is a different problem from a flaky network.
4. **Publish on operator-visible change, not on `history_revision`.** Republish
   when capability, the job rows, the `(epoch, revision)` identity, or the stale
   marker change — an idle → running transition and a cleared error both leave
   `history_revision` alone, and a revision-only predicate hid both until some
   unrelated event forced a document. Never republish because
   `generated_at_unix_ms` or the local attempt/success timestamps moved; they are
   bookkeeping, not something a row draws.
5. **A summary and its history are one pair, or neither.** Two independent
   requests can straddle a restart, so apply history only when
   `history.epoch == summary.epoch && history.history_revision ==
   summary.history_revision`. A mismatch is never merged, never advances the
   gate (that would tell the next summary for the newer lifetime that its history
   was already fetched), and is reported as an `Incoherent` diagnostic with the
   summary retained. Do not require `generated_at_unix_ms` equality.
6. **Cron state is target-bound, like metrics.** An observation carries the host
   and port it polled; the engine drops one whose target is no longer configured
   for that stable id, repointing clears that id's capability/summary/error/
   history, and the history gate is keyed by `(system id, normalized host, port)`
   so a new target re-discovers even when its epoch/revision numerically collides.
   An equivalent spelling of the same endpoint changes nothing.
7. **One startup round, then one per period, with a four-read window.** Use
   `interval_at` (not `interval`, whose first tick is already due) so startup does
   not poll twice, keep `MissedTickBehavior::Delay`, and observe the fleet with at
   most `CRON_MAX_IN_FLIGHT` (4) requests in flight rather than sequentially — a
   sequential walk made the effective cadence a multiple of the nominal one behind
   any slow endpoint. The bound is a constant, never configuration, and the round
   spawns nothing, so a reload or shutdown drops at most those few reads.
8. **A reload preempts the whole round it lands in — remote reads *and* the
   observation hand-off.** Select reload and cancellation *inside*
   `CronWorker::round`, not only between rounds. Waiting for the round to
   finish made reaction time proportional to the superseded fleet —
   `ceil(fleet / 4)` timeout waves — so a newly added or repointed system went
   unobserved for minutes. On reload: stop enqueueing, drop the in-flight futures
   (they *are* the requests), return a typed outcome, re-read the endpoint list,
   prune gate keys absent from the new target set, and start a fresh round at once
   without waiting out the cadence. Cancellation ends the worker the same way.
   Keep the bound at four, never spawn per endpoint, and never overlap rounds.
   The finished-fetch `send` is the second place a round can wait — the bounded
   64-slot engine channel is the only receiver — so deliver through
   `deliver_observation`, which selects cancellation, then reload, then the send.
   Never `await` that `send` bare in the round body, or a reload waits on receiver
   capacity for work already superseded; order the signals ahead of the send so a
   slot freeing alongside a stored signal cannot let the obsolete observation win.
   This is bounded loss of *superseded* work only: with no signal the send still
   backpressures, and `CRON_CHANNEL_CAPACITY` (64) must not grow to hide the gap.
9. **Commit the history gate only after delivery.** `settle` returns the
   observation plus a `PendingGateCommit`; the round applies the commit only
   after the bounded channel accepts the observation. Advancing the gate during
   settlement let a document that was dropped (preempted round, abandoned
   hand-off, closed engine channel) claim "already fetched" and suppress history
   forever. Every non-delivery path returns before the commit line. The
   invariant: **"the gate says fetched" implies the coherent document reached the
   engine.** `observe()` commits inline because returning the observation *is* the
   delivery. Keep the gate worker-private; add no acknowledgement traffic.
10. **Never render a load relation that is not true.** `greggd` retains the gate
   decision that admitted a job, so a running or idle row can carry a reading
   *below* the threshold. Only `load delayed` prints `load15m 9.24 > 8.00`; a
   running row prints `start load15m 1.20 <= 8.00`; an idle or slot-waiting row
   prints `last gate ...` so an old reading is not presented as current load; and
   with no reading there is no comparison at all — `load15m unavailable (max
   8.00)`, never `— > 8.00` and never `0.00`. A time-only job has no load token.
11. **Elapsed grammar, and two time bases.** Elapsed states read `for 3m`,
   `pending 17m`, `queued 2m`; countdowns read `next 11h`, `retry 20s`. Never
   `… ago` on a duration, and let clock skew saturate at `0ms` rather than
   underflow. A record's clock is labelled `Z` (`10-05 07:00Z`) because it is a
   UTC instant while the schedule beside it is the remote's local civil cron; do
   not "fix" that by inventing a remote timezone — that needs a protocol change.
   The **schedule column** is compacted only when it fits: `0 3 * * *` renders
   `03:00` and `30 2 * * 3` renders `weekly Wed 02:30`, everything else stays
   verbatim. "Compacted in shape" is not permission to rewrite — an all-digit
   field can still overflow `u32`, and defaulting it to `0` rendered
   `99999999999999999999` as a confident `00:00`. A field that does not parse
   as a time keeps the operator's own text.
12. **One row builder, one reserved section, one window.** `block_rows` feeds
    both `desired_rows` and `render`, so the requested height includes the stale
    notice and cannot drift from what is emitted. The selected job's header and
    newest record are reserved before any job row, the job table is a window
    *around the selection* (clamped at both ends, hidden jobs counted), and
    truncation is reported with `… more cron rows not shown` — a different fact
    from remote `stdout+` truncation. Never add a second scroll model, a
    scrollbar, or mouse handling to solve this.
Remote text is inert before it reaches a cell: controls render in `cat -v` caret
notation rather than being stripped, escaping happens at the `adopt_snapshot`
chokepoint *and* locally in the renderer, and the line bound in `sanitize` is
never used to shorten stored data — only the viewport may truncate.
