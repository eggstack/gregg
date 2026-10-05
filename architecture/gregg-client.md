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
| `main` | `src/main.rs` | Entry point and the TUI event loop (3 `select!` biased arms in source order: shutdown, input events, daemon frames — input before frames so a flood of state documents cannot starve `Quit`). Reads no config file and opens no network connection; `update` is synchronous, before Tokio |
| `clientd/identity` | `src/clientd/identity.rs` | Per-config FNV-1a identity of the normalized config path; endpoint candidate paths (config-adjacent primary, temp-dir fallback) |
| `clientd/protocol` | `src/clientd/protocol.rs` | Local IPC wire contract: `PROTOCOL_VERSION`, 8-hex length prefix, `FrontendFrame` / `DaemonRequest`, encode/decode caps. Deliberately **not** in `gregg-protocol` — same binary, same host, version handshake instead of forward compatibility |
| `clientd/ipc` | `src/clientd/ipc.rs` | Endpoint transport. Unix: `0600` socket, non-blocking discipline, stale reclamation (must be a socket **and** unconnectable). Windows: named pipe `\\.\pipe\gregg-client-<id>` with an owner-only SDDL and `PIPE_REJECT_REMOTE_CLIENTS`; liveness is a `WaitNamedPipeW` probe, never a file check |
| `clientd/snapshot` | `src/clientd/snapshot.rs` | `FrontendSnapshot` and per-system / EggPool DTOs. Timestamps cross as Unix ms because `Instant` cannot be transported |
| `clientd/daemon` | `src/clientd/daemon.rs` | The client daemon: owns `FleetState`, the poll scheduler, the EggPool worker, and the `Ctrl-R` reload boundary; `watch`-channel fan-out; `attach` / `status` / `stop` |
| `clientd/frontend` | `src/clientd/frontend.rs` | The TUI's side: dial, handshake, request sender (`ControlSink`), and the single socket-owning frame reader |
| `clientd/launch` | `src/clientd/launch.rs` | Lazy activation, the config-specific launch lock, endpoint classification, version rotation, `restart` |
| `clientd/startup` | `src/clientd/startup.rs` | User-scoped startup render/parse/ownership plus the thin install/uninstall execution |
| `startup_support` | `src/startup_support.rs` | The only place a startup manager is executed: a fixed allowlist, no shell, bounded wait |
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `version`, `update` (thin adapter over `gregg-update`), `uninstall` (exact-exe removal, dry-run/purge), `eggpool` |
| `update` | `src/update.rs` | Thin `run_simple_update` adapter binding the client identity |
| `uninstall` | `src/uninstall.rs` | Exact-exe client removal plan/execution (`purge_empty_dir_for` only the standard parent) |
| `config` | `src/config.rs` | Façade re-exporting `config/{model,store,validation,lock}` |
| `config/model` | `src/config/model.rs` | Config model: entries, limits, defaults, load/validate/write primitives |
| `config/store` | `src/config/store.rs` | `ConfigStore` coordination, atomic persistence, staging I/O, `ConfigError`, `AtomicWriteError` |
| `config/validation` | `src/config/validation.rs` | `ConfigViolation` kinds and field checks |
| `config/lock` | `src/config/lock.rs` | Cross-process advisory file locking (`FileLockGuard`) |
| `state` | `src/state.rs` | AppState reducer, viewport logic; per-system `offline_reason` provenance set from accepted failures, cleared by accepted successes |
| `action` | `src/action.rs` | Action enum (15 variants including `Resize` and Plan 087's `ClearSelectionHighlight`) |
| `sanitize` | `src/sanitize.rs` | Pure terminal-control sanitizer; remote text is inert before it reaches a cell |
| `cron` | `src/cron.rs` | Local cron retention: `(epoch, sequence)` dedup, per-job depth, global record ceiling, union intent reduction |
| `qualification` | `src/qualification.rs` | Derives the Plan-161/167 memory and payload bounds from the real constants and asserts them |

### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, PollOutcome classification; `OfflineKind`/`OfflineReason` stable failure provenance (`PollOutcome::offline_reason`) |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, generation-based concurrency |
| `endpoint` | `src/endpoint.rs` | Canonical IPv4/IPv6/DNS endpoint parsing (`parse_add_input`); HTTP-URL/nickname adaptation lives in `cli.rs::parse_add_target` |
| `clock` | `src/clock.rs` | Clock trait for deterministic testing |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI consumption |
| `clientd/cron` | `src/clientd/cron.rs` | Daemon-owned `/v2/scheduler` client: 30s summary cadence with one startup round, target-keyed revision/epoch-gated history fetch, coherent pair check, four-reads-in-flight round, `CronWorker` task |

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
| `ui/cron` | `src/ui/cron.rs` | Cron detail block: job rows, selected-job history, staleness labelling, local escaping and a viewport row bound |

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

The main event loop in `main.rs` uses `tokio::select!` biased in this source
order:

1. Shutdown (`CancellationToken`)
2. **User input events** from crossterm → translate to actions → apply to presentation state, and forward anything that crosses the process boundary to the daemon (input before frames so a flood of state documents cannot starve `Quit`)
3. **Daemon frames** → `FrontendFrame::Snapshot` is adopted into the render model; `Hello` and `ControlAck` are not render-visible; `ShuttingDown`, `VersionMismatch`, and `ProtocolError` exit with the daemon's own reason
4. **Highlight deadline** (`tokio::time::Sleep` arm, parked far-future while dormant) — when armed, the loop dispatches `Action::ClearSelectionHighlight` and re-renders so the reverse-video styling disappears even when no other event fires

The loop keeps a local dirty flag. It draws the initial frame immediately,
then redraws only when a document is adopted with a render-visible change, or
after a mapped render-visible action, terminal resize, or highlight expiry.
`AppState::adopt_snapshot` is the sole decider: it compares what a renderer can
actually see (endpoint, configured name, reachability, normalized snapshot,
offline reason, EggPool window/worker/data planes) and explicitly ignores
timestamps and latency, so an age that advanced does not force a frame. A
document that is not newer than the last applied one is skipped, not rendered —
that is the point of a latest-state channel. Unmapped keys and channel wakeups
that do not change visible state do not rebuild a frame. Gregg still submits
complete frames to Ratatui; Ratatui remains responsible for cell diffing and
there is no partial-render architecture.

### Client daemon (Plan 164)

Ownership of everything derived from the network lives in `gregg daemon run`,
a foreground process per configuration file. A TUI that owned its own poller
would multiply the fleet's request budget by the number of open windows, and
closing one window would silently stop a fraction of what the operator is
watching. Here, a second window costs a socket and closing the last window
changes nothing about polling.

```
Config → PollScheduler + EggPool worker → FleetState → encode once
                                                     → watch::channel<Document>
Frontend A ─┐                                        ├─ write same bytes
Frontend B ─┴────────────────────────────────────────┘
```

- **Identity.** One daemon per config, keyed by an FNV-1a digest of the
  normalized config path, on a `0600` socket beside that config. Two configs
  never share a daemon; one daemon never serves another config's fleet.

- **Handshake.** Carries the protocol version *and* the expected config
  identity. A TUI is refused rather than fed another config's systems, and
  `gregg daemon status`/`stop` identify their target through the handshake
  rather than a process name or PID file.
- **Fan-out.** `watch`, not a queue: a frontend that cannot keep up observes a
  newer generation and skips the ones it missed, which is safe because every
  document is complete. Each document is serialized **once** and every frontend
  is handed the same bytes, so publication cost does not scale with open
  windows — the same discipline `greggd` uses for its scheduler cell. Control
  acknowledgements are the one non-latest-state message and ride a small
  per-connection channel, so one frontend's request cannot displace another's
  state.
- **`Hello` is always the first frame.** A frontend identifies its daemon by the
  first frame it receives, so a document may never be written before the
  handshake is answered — doing so makes a healthy daemon indistinguishable
  from a refusing one, and the connection is dropped. The handshake reply
  already carries the current state, so withholding the premature write costs
  the frontend nothing.
- **Ownership boundary in code.** `FleetState` holds the reducer that consumes
  poll batches, EggPool results, and reloads. `AppState` in a frontend has no
  such entry point; its only fleet writer is `adopt_snapshot`. The
  `#[cfg(test)] test_fleet` shadow on `AppState` lets renderer tests drive the
  *real* publish/adopt path, and does not exist in a production build.
- **First-document vs first-reachability.** The daemon publishes once at bind,
  before any poll has completed, with every system `pending`. A frontend
  places its selection on the first document that carries *polled*
  reachability (`FrontendSnapshot::poll_initialized`), not on the first
  document it hears. The daemon therefore publishes the initialization
  transition once even when it changed nothing visible — otherwise the
  placement would be deferred onto some unrelated later change and undo
  whatever the operator had selected in between.
- **EggPool convergence.** The worker is daemon-owned. Each frontend publishes
  its whole `(active, period)` intent, and the daemon reduces the set:
  `active` is true if any frontend has the pane open, `period` is the shortest
  window any of them asked for. Both are order-independent, so two windows
  cannot race the worker into two activations, and the last pane leaving always
  converges the worker to inactive. Intents are keyed by the accept loop's
  stable subscriber id, so a frontend that refreshes often cannot leave a stale
  active entry behind.
- **No fallback.** A frontend that cannot reach a compatible daemon reports the
  reason and exits. A silent direct-polling fallback would double the request
  budget exactly when the daemon is unhealthy and would make the failure
  invisible. Plan 165 turns an *absent* endpoint into a bounded lazy launch; it
  does not relax this.

### The Windows transport is not the Unix transport

`clientd/ipc` presents one `bind`/`connect`/`accept`/`cleanup` surface so neither
the daemon nor the frontend carries a `#[cfg]`. That uniformity is worth less
than the two facts below, which are Windows-only and load-bearing.

**The endpoint name is a pipe name, not a path.** `CreateNamedPipeW` requires the
`\\.\pipe\` prefix and rejects anything else, and the `\\.\pipe\` namespace is
flat. So Windows has one endpoint per config, `\\.\pipe\gregg-client-<id>`,
derived from the identity digest alone: no config-adjacent location to prefer
and no unwritable temp-directory fallback to reach for. The `sun_path` length
limit is a Unix constraint and must not suppress a Windows endpoint. Because a
pipe is not a filesystem entry, "is a daemon still holding this?" is
`WaitNamedPipeW(name, 0)`, not `path.exists()` — the latter answers "no" for a
daemon that is running, which would make the `stop` confirmation and every
startup readiness wait return immediately.

**Nothing may block the runtime.** `gregg daemon run` uses a *current-thread*
runtime, so a single blocking call freezes polling, cron, and every attached
window at once. A synchronous named pipe has no non-blocking read, so the two
places that would block are handled differently:

- *Accept.* `ConnectNamedPipe` is parked on the blocking pool with the pipe
  instance moved into the closure, so the handle keeps one owner for the whole
  wait. The accept loop therefore never busy-polls on Windows, where a Unix
  accept returns `WouldBlock` every 10 ms.
- *Read.* `PeekNamedPipe` reports the queued byte count without blocking, and
  `ReadFile` then asks for at most that many bytes and at most what the caller's
  buffer holds, so it has nothing to wait for. Asking for less than the queued
  count is deliberate: it keeps `ERROR_MORE_DATA` out of the picture, and the
  remainder is read on the next poll.
- *Write.* A full peer buffer is backpressure, never a disconnect. The unsent
  tail of the frame in flight stays in the connection's outbound buffer and is
  pushed on the next tick, because a length-prefixed frame with a hole in it
  desynchronises the peer's parser permanently. At most one document is queued
  behind it and a newer document replaces it, since the publication cell is a
  watch slot and a slow frontend skips to the newest state. A Windows pipe has
  no non-blocking write; the repeated flush is what keeps it bounded, not the
  single write call.

Each instance is created with the owner-only DACL `D:P(A;;GA;;;OW)`,
`PIPE_REJECT_REMOTE_CLIENTS` in the **pipe-mode** argument (folding it into
open-mode would OR incompatible access rights and leave the pipe reachable
off-box), and no `FILE_FLAG_OVERLAPPED`. The descriptor the SDDL conversion
allocates is self-relative and is released with `LocalFree` in the same block
that created the pipe.

**The parked accept is cancellable (Plan 171).** Parking the wait on the
blocking pool is what keeps the runtime free, but it made the wait immune to
`accept_task.abort()`: aborting a task cannot cancel a `spawn_blocking` job that
is already running, and the closure owns the pipe handle, so the thread stayed in
`ConnectNamedPipe` until a client arrived. `dispatch_daemon` drops the `Runtime`
at the end of the function and Tokio's shutdown waits for outstanding blocking
jobs without a timeout, so `gregg daemon run` acknowledged a stop request,
unwound, unlinked the endpoint — and then hung instead of exiting.

The wait is now stopped rather than abandoned. The closure duplicates a real
`THREAD_TERMINATE` handle to itself and publishes it to an `AcceptGate`;
`AcceptStop::request` cancels the pending `ConnectNamedPipe` with
`CancelSynchronousIo`, the documented mechanism for interrupting synchronous I/O
from another thread, and is *not* one of the four functions Win32 lists as
non-cancellable. `run_daemon` requests the stop, waits for the accept loop to
unwind, and only then releases the endpoint. `AcceptStop` is a no-op on Unix and
is exported from both transports, so this shutdown sequence carries no `#[cfg]`.

Three things keep it race-free rather than usually-right, and all three matter if
the code is ever edited:

- The stop is **recorded before** the parked record is inspected, and `begin`
  re-checks it under the same lock. A wait that starts after a stop declines to
  park rather than parking uncancellably.
- The lock is **held across** `CancelSynchronousIo` and released between
  attempts. Holding it is what proves the recorded thread is still the parked one
  and not some other blocking-pool job; releasing it is what lets the parked
  thread reach the `end` that clears the record.
- A cancel issued **before** the thread enters the kernel finds nothing pending
  and is not remembered, so it is re-issued while the record is up (bounded at
  `ACCEPT_CANCEL_ATTEMPTS`).

`ERROR_OPERATION_ABORTED` is reported as a disconnect, not a connection: no
client attached, and the accept loop's existing `Disconnected` arm already
tolerates that without giving up.

The handle must be a real handle, not a thread **id**: ids are recycled when a
thread exits, so cancelling by id could interrupt an unrelated thread. That is
also why `ipc.rs` carries the workspace's only `unsafe impl Send` — a `HANDLE` is
a process-wide kernel object identifier, and every access to it is serialized by
the gate's lock.

The signal path in `cli.rs` needed the same treatment and had the same defect:
returning from `select!` dropped `run_daemon`'s future, skipping the whole
teardown. It now awaits the daemon after cancelling it, so a supervisor stop and
Ctrl-C take exactly the path a stop request takes.

Unix has no equivalent problem: `tokio::net::UnixListener::accept` is a
cancellable async operation, and the loop reaches its own cancellation check in
milliseconds.

### Lifecycle (Plan 165)

`gregg` ensures its own daemon, so the client is usable by default without
anyone having to start a background process first.

```
gregg
  ├─ bounded handshake probe
  │    ├─ attached            → run the TUI
  │    ├─ absent              → launch (below)
  │    └─ refused/foreign/newer → report, start nothing
  └─ launch
       ├─ acquire the config-specific launch lock   (spawn_blocking)
       ├─ re-probe *and re-classify* under the lock
       ├─ detached spawn of the exact current executable
       └─ bounded readiness wait, then attach
```

- **The lock is an advisory OS lock**, held only across
  probe/spawn/readiness and never for the daemon's lifetime. Its file is never
  unlinked and is never read as a signal, so a crashed launcher cannot leave
  behind something that makes the next launch believe a daemon is starting.
  There is no PID file and no registry.
- **The lock wait runs on `spawn_blocking`.** It is a blocking sleep loop, and
  the TUI runs on a current-thread runtime, so waiting inline would starve the
  tasks that make the winner's daemon reach readiness.
- **Only plain absence authorizes a spawn**, and the classification is redone
  under the lock: someone else can bind a foreign service while this process
  waits, and re-probing alone would have spawned over it.
- **Rotation is directional.** A daemon *older* than this frontend is stopped and
  relaunched on the current binary. A daemon *newer* than this frontend is
  reported with upgrade guidance and left running — it may be serving a newer
  window elsewhere, and downgrading that session because one window is old would
  be a worse failure than an error message. Waiting for the old daemon to release
  the endpoint asks the transport, never the filesystem: a Windows named pipe is
  never a path that exists, so a path-existence check reports the endpoint as
  already free and the replacement races a daemon that is still unwinding.
- **User-scoped startup only.** A `systemctl --user` unit, a
  `~/Library/LaunchAgents` agent, a current-user Startup-folder entry, or a
  managed user crontab watchdog. Never a system unit, never `LocalService` SCM,
  never `sudo`; all manager execution goes through one allowlist with no shell
  and a bounded wait. A root install registers nothing, because there is no
  honest way to choose which human a shared binary should watch for.
- **Ownership is proven by parsing what would be written**, then re-read after
  writing as a self-check. `Unknown` (unreadable) is treated exactly like
  `Foreign`: "I could not read it" is not a licence to delete it. Ownership is
  also *bounded*: removing a managed crontab block consumes the marker and the
  one command line it announced, because that is all the block contains. Scanning
  forward to a blank line instead would eat whatever the operator appended after
  it with `crontab -e` — silent, unrecoverable loss of a job Gregg never owned.
- **Update is prepare-then-quiesce.** The daemon is identified first, the
  candidate is fully prepared and verified, and only then is the daemon stopped,
  the executable replaced, and the daemon relaunched. A failed relaunch is
  reported as partial success with the retry command.
- **Uninstall is ownership-first.** `--dry-run` names the startup entry and the
  running daemon; execution stops only an identified owned daemon, removes only
  a provably owned entry, and blocks the executable deletion when it cannot
  confidently stop a running daemon. There is still no idle shutdown: losing the
  last TUI is not a reason to stop.

The highlight deadline is the only transient timer the loop owns.
Selection-changing Systems actions (`j`/`k`, page movement, `g`/`G`)
arm or reset the deadline to ten seconds from now via
`SELECTION_HIGHLIGHT_DURATION`. Non-selection events (poll batches,
EggPool results, `Resize`, `RefreshNow`, `ToggleSystemView`,
`ToggleDrives`, `ToggleNetwork`) do not extend the deadline. The
`ClearSelectionHighlight`
arm is parked at a far-future sleep while no highlight is active so the
select branch never fires spuriously.

### Action/Reducer pattern

All state changes go through the `Action` enum:

```rust
pub enum Action {
    MoveDown, MoveUp, PageDown, PageUp,
    SelectFirst, SelectLast,
    PreviousPane, NextPane,
    ToggleSystemView, ToggleDrives, ToggleNetwork,
    RefreshNow,
    ClearSelectionHighlight,   // Plan 087: dispatched by the highlight timer
    Resize { width: u16, height: u16 }, Quit,
}
```

`AppState::apply_action()` and `apply_batch()` are pure, deterministic
functions. The renderer reads `AppState` projections without performing I/O.

### Polling pipeline

```
Config → Endpoint list → PollScheduler → PollBatch channel → FleetState reducer
                                                                    │ JSON
                                                                    ▼
                                                        AppState::adopt_snapshot
```

`FleetState` lives in the client daemon and is the only reducer that consumes a
network result. `AppState` lives in a TUI window and is the frontend render
model; its only fleet writer is `adopt_snapshot`. `CronWorker` runs a second,
slower plane over the Plan-162 scheduler routes and feeds `FleetState.cron`.

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

The Systems-pane `Ctrl-R` is the **client daemon's** reload boundary: the
daemon re-reads its own resolved `ConfigStore`, reconciles the fleet, and asks
its poll task for an immediate poll through the bounded scheduler command
channel. That ask is a `try_send` and never a blocking send. The engine is the
single task that owns the fleet, applies cron and EggPool results, and writes
every frontend's document, so parking it would freeze every attached TUI for the
rest of the generation and leave a `Shutdown` request unread. A refused ask
costs only the early poll, which the fixed cadence provides anyway. Failed
config loads retain the last-known-good fleet, publish a reload diagnostic, and
still ask for a poll.

A reload may also add, remove, or repoint the `[eggpool]` entry, so the worker
is wired from the *current* config rather than from the startup config. Removing
it already worked by accident; *adding* it used to publish a pane that no
worker could answer, which left the pane in `Refreshing` forever instead of
reporting `WorkerUnavailable`.

Plan 070 evaluated replacing the per-endpoint tasks and semaphore with a
buffered future stream. That candidate was rejected because it would remove
task isolation and the panic-to-`Cancelled` guarantee while still needing
explicit endpoint ordering and cancellation handling. The current bounded
design is retained intentionally.

**Poller** (`poller.rs`):
- v2-first, endpoint-bound schema parsing, v1 fallback only on 404
- Accepts only the schema matching the requested endpoint; malformed, invalid, and wrong-version responses never trigger fallback
- Dedicated `eggfetch_core::Client` (lean `standard-http1` + Rustls profile:
  standard DNS/TCP/TLS route only, redirect following not compiled so 3xx
  passes through with no second hop, four idle per host, explicit
  five-field whole-request deadline with absolute `total` through
  response-body EOF, 256 KiB decoded-body cap — a hostile-input ceiling that
  clears the 111,465-byte maximum valid v2 payload, not a wire-format limit —
  no automatic retry);
  typed `NetworkFailureKind` drives DNS/refused/connect mapping, never
  `Display` heuristics; body-stage typed timeouts map to `Timeout`
  while ordinary post-header body failures stay `NetworkError`
- `PollOutcome` classifies 12 outcome variants (2 success: `Online`/`OnlineV2`, 10 failure/cancellation)

**Normalization** (`normalized.rs`):
- v1 and v2 wire formats → `NormalizedSnapshot` with capability flags
- Eliminates version-branching in the UI
- `aggregate_drives()` with checked arithmetic
- Optional CPU frequency, normalized disk-I/O devices/aggregate, and
  normalized network interfaces/aggregate are copied from v2; v1 and
  pre-feature v2 payloads leave them absent
- `network_utilization_pct()` and
  `bytes_per_second_to_bits_per_second()` are pure helpers. Utilization uses
  the maximum valid directional percentage, never `rx + tx`, and returns
  `None` when capacities are unavailable

Compatibility is proven at the normalization boundary and does not require
wire-version branches in renderers: v1 fallback and legacy-v2 payloads retain
their historical core metrics, while current-v2 optional families appear or
degrade independently. CPU frequency is current OS-reported frequency, not a
base/max claim. Disk capacity is separate from disk I/O; `R/s`, `W/s`, `Rx/s`,
and `Tx/s` are byte-throughput labels. Loopback remains available to `n`
detail without contributing to aggregate network capacity, and daemon-version
transport is intentionally deferred.

### State model

```rust
struct AppState {
    systems: Vec<SystemState>,           // per-system state
    selected_id: Option<SystemId>,       // current selection
    viewport_top_id: Option<SystemId>,   // scroll position (first visible)
    last_applied_generation: u64,        // stale batch rejection
    refresh_status: RefreshStatus,       // currently always `Idle`; generations tracked via `last_applied_generation`
    config_reload_error: Option<String>, // last rejected `Ctrl-R` diagnostic, sanitized at adoption
    terminal_size: Option<(u16, u16)>,   // terminal dimensions
    active_pane: Pane,                   // Systems or Eggpool
    system_view_mode: SystemViewMode,    // Normal or Condensed
    drives_expanded: bool,               // selected-system drive detail
    network_expanded: bool,              // selected-system network detail
    selection_highlight_active: bool,    // transient reverse-video highlight
    eggpool: Option<EggpoolState>,       // EggPool pane state (None if unconfigured)
}
```

**Display order:** Online systems first (stable order), then offline/pending.
**Viewport:** Computes visible range for mixed-height entries (legacy normal =
5 rows, network-capable normal = 6 rows, condensed = 1 row). Selected-system
drive and network detail lines are added centrally, so resize and scrolling do
not rely on renderer-only offsets. Both view modes share
`valid_drive_detail_count`, so a legal v2 payload with `drives: None` and
`disk_io: Some(..)` reserves the same table-heading and `I/O TOTAL` rows in
normal and condensed. Selected system is always visible.

**First-batch snap:** `AppState::apply_batch` snaps `selected_id` and
`viewport_top_id` to `display_order()[0]` only when `last_applied_generation
== 0` before the batch is applied (the first accepted poll batch).
Subsequent batches preserve the existing selection/viewport semantics.
`Ctrl-R` does not re-snap.

**Visual vs. logical selection (Plan 087):** `selected_id` is the
persistent logical selection that drives `d` (drive expansion), `n` (network
expansion), and
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
| `j`/`k` (or `Up`/`Down`) | Move down/up |
| `h`/`l` (or `Left`/`Right`) | Previous/next pane |
| `v` | Toggle normal/condensed view |
| `d` | Toggle drive expansion |
| `n` | Toggle network detail expansion (legacy systems are a no-op) |
| `g`/`G` | First/last system |
| `f`/`b` (or `PageDown`/`PageUp`) | Page forward/back |
| `Ctrl-R` | Reload Systems config and reliably replace/poll endpoints, or refresh EggPool |
| `q`/`Esc`/`Ctrl-C` | Quit |

### Width degradation

The header line drops lower-priority segments as width decreases:
- < 32 cols: no load/cores
- < 50 cols: no OS
- < 80 cols: no kernel/arch

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

**Normal view** (`ui/system_block.rs`): each online block has five rows when
its current snapshot lacks network telemetry and six rows when its snapshot
exposes it. Mixed fleets may therefore have different vertical block heights:
1. Header (name, IO if available, load, cores, OS, kernel, arch)
2. CPU bar
3. MEM bar
4. SWP or COMMIT bar (platform-dependent)
5. DISK aggregate bar + optional drive detail rows
6. NET aggregate bar, only for systems with network telemetry

The mixed-fleet policy is deterministic: each system omits NET when its own
snapshot lacks network telemetry, while zero-throughput and unknown-capacity
snapshots retain a valid NET row. CPU frequency is per-system
optional and is formatted by the client after the core count.

The active metric rows share one fleet-wide label width and one
fleet-wide `bar_width`; their opening `[` and closing `]` always occupy
the same terminal column across every online system. Geometry is
computed once per render via `build_metric_rows`,
`compute_fleet_metric_layout`, and the private `resolve_system_suffixes` helper; the
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

Normal metric rows are cached cross-render in a renderer-local map keyed by
stable system ID and a compact render key containing only values that affect
row text and NET presence. Cached rows retain natural and percentage-only
suffix forms. Each render builds an index-aligned optional row table so
visible entries do not search the fleet repeatedly. Condensed values are
cached cross-render in `HashMap<stable ID, {CondensedRenderKey,
Rc<PreformattedValues>}>` (the key includes label and port, so a port-only
edit invalidates the memo), not reformatted every render; condensed
measurement borrows configured names/hosts.

Production polling consumes owned `PollBatch` payloads through an internal
reducer path, moving normalized identity/detail strings and collections. The
borrowed `apply_batch(&PollBatch)` and normalization constructors remain the
compatibility/reference paths. Ordered scheduler results use positional
stable-ID matching with a safe fallback for reordered or synthetic batches.

**Offline rendering (normal view, `ui/system_block.rs::render_offline`):** When the
configured client name is set the row reads `name@host:port offline`;
otherwise it reads `host:port offline` and never duplicates the host.
The configured client name persists on `SystemEntry.name`; the daemon's
`system.name` is not used for client-side display. When the accepted poll
failure carries provenance, the stable category is appended inside the
existing width budget (`offline (refused)`,
`offline (http) HTTP 503`); pending rows never carry a reason. Provenance
is stored in `AppState` (`SystemState::offline_reason`, set from
`PollOutcome::offline_reason()` on accepted failures and cleared by
accepted successes in the same generation), never recomputed by the
renderer and never sourced from transport error types.

**Expanded drive rows** (`d` in normal or condensed view, shared between
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

When v2 disk-I/O telemetry is present, `d` adds a table heading, optional
`R/s`/`W/s` columns (including the `Throughput` fallback tier: name + rates),
and an `I/O TOTAL` line. A drive receives a rate only
when exactly one daemon device record names that mount; ambiguous or missing
associations render `—`. The aggregate line is always taken from the daemon's
aggregate and is never recomputed from visible drive rows.

**Network detail** (`n`): the independent expansion adds one aggregate summary
followed by the normalized interface order, including loopback when supplied.
Each row includes Rx/s and Tx/s and shows link capacity where available.
Aggregate utilization uses the maximum valid directional percentage; an
unknown capacity leaves the percentage as `—` while preserving raw rates.

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

Online rows (normal header and condensed HOST) render the configured name, or
the bare host when no name is configured, **without** the port: the online row
is a one-line identity/metric summary and the condensed HOST column is the
most width-constrained. Normal-view offline/pending rows keep the full
`name@host:port` form because there is no metric row to disambiguate them.
Condensed status rows (`ui/condensed.rs::status_line`) render truncated
`name|host + status` only (no `@host:port` in text); identity is preserved via
the fleet-wide HOST budget. `CondensedRenderKey` still carries the label
*and* the port separately, so a port-only config edit still invalidates a
memoized row.

Condensed tiers add `NET` between `DISK` and `LOAD` where the tier fits:
Wide includes `NET` and `IOWAIT`, Medium includes `NET` and `LOAD`, Narrow
includes `NET`, and Minimal retains only HOST/CPU/MEM. Natural-width fallback
may choose a narrower tier or truncate HOST, but never clips a numeric cell.

## Configuration

Endpoint parsing accepts IPv6 link-local zone identifiers in either bare
`fe80::1%eth0` or URL-escaped `%25eth0` spelling. Persisted endpoint hosts use
the URL-safe `%25` separator so the poller can construct valid HTTP URLs.
Bracketed endpoint syntax is reserved for IPv6 literals; URL construction
returns normalization errors instead of falling back to raw host text. An
encoded `%25` separator without a zone name is rejected rather than persisted
as a malformed host.

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

Only contention is retried. The Unix path inspects `errno` and accepts
`EWOULDBLOCK`/`EAGAIN` as "someone else holds the lock"; anything else
(`EBADF`, `EINVAL`, `ENOLCK`, `ENOTSUP`) is surfaced immediately as
`ConfigError::Io` instead of being misreported as a 5-second
`LockTimeout`. The Windows branch makes the same distinction between
`LOCK_VIOLATION`/`ERROR_IO_INCOMPLETE` and other errors.

Every persistence path — `mutate`, `mutate_with_result`, `edit_transaction`,
and the public `ConfigStore::write` — takes the same in-process mutex and
cross-process file lock, so no API can bypass the documented protocol and
lose a concurrent update.

Config mutations are synchronous because they use bounded OS-lock polling and
filesystem I/O. The CLI performs them before creating its Tokio runtime;
library callers from async tasks must move the mutation to a blocking thread.

### Stored endpoint validation parity

`Config::validate` enforces the same host and name rules that `gregg add`
enforces through the CLI parsers, so a hand-edited config fails validation
instead of surfacing later as a `NetworkError` or an unrenderable
`nickname@host:port` string:

- `host` rejects a scheme, path, query, fragment (`host#frag`), credentials
  (`user@host`), IPv6 brackets, whitespace, and control bytes.
- `name` rejects surrounding whitespace, control bytes, and `@ : /`, matching
  `endpoint::validate_name` (reported as `ConfigViolation::InvalidName`).
- `gregg eggpool add` accepts IPv6 zone IDs (`fe80::1%eth0`,
  `[fe80::1%25eth0]:11300`) and stores the URL-safe `%25` form, exactly like
  `gregg add`.

### CLI subcommands

| Command | Purpose |
|---------|---------|
| `add <host:port or http://host:port/> or nickname@host:port>` | Add endpoint with required explicit port; `--name` and an inline `nickname@` are mutually exclusive; persisted fields are normalized `host`/`port` and optional `name` |
| `list` | List configured endpoints |
| `remove <host\|host:port>` | Host-only removes all endpoints on that host; `host:port` removes one exact endpoint |
| `refresh` | Set the global polling interval (seconds) |
| `edit` | Open config in editor |
| `version` | Print client version |
| `update` | Thin CLI adapter over the shared `gregg-update` mechanism (binds program identity, preserves variant structure with program-prefixed `Display`); full flow (`run_simple_update`) lives in `gregg-update` |
| `uninstall [--dry-run] [--purge]` | Remove only the exact invoked client executable (sibling `greggd` survives; no directory recursion); config preserved by default, `--purge` removes only the resolved config file after successful Unix Cargo package removal, `--dry-run` shows Cargo plus config intent without mutation; Windows Cargo-owned installs print the zero-mutation handoff; never inits the TUI runtime |
| `eggpool add/list/remove` | Manage the single EggPool endpoint; adding another requires `--replace` and reports a configuration conflict otherwise |

## EggPool

Optional summary pane for EggPool API metrics. Separated from greggd polling.

**Client** (`eggpool.rs`):
- Dedicated `eggfetch_core::Client` (lean `standard-http1` + Rustls
  profile: redirect following not compiled, two idle per host, explicit
  five-field whole-request deadline with absolute `total` through
  response-body EOF, 16 KiB client-wide decoded-body cap, no automatic
  retry), isolated from Systems polling
- Summary plane: `/api/stats/summary?period=...` with a per-request 16 KiB cap
- Health plane (Plan 152): `/api/status` with a per-request 1 MiB cap aligned
  with EggPool's own bounded status client. The client-wide default stays at
  the summary bound, so raising this one route never widens the summary route.
- Request-local `AuthScheme::bearer` (invalid values map to `InvalidSummary` for
  the summary plane and `InvalidApiKey` for the health plane; secrets never
  enter outcomes, logs, or URLs)
- Bearer token from environment variable (never stored in outcomes)
- An absent or empty key stops the summary request as `MissingApiKeyEnv` but
  still sends the health request unauthenticated, because EggPool keeps
  `/api/status` authenticated even when the dashboard is public; the server's
  own 401/403 answer is authoritative

**Health plane** (`/api/status`, Plan 152):
- `EggpoolProxyHealth` (`Ready`/`Degraded`/`Unready`),
  `EggpoolProviderHealth` (`Ready`/`Degraded`/`Unavailable`/`Disabled`/`Unknown`),
  and `EggpoolProviderObservation` (`Verified`/`Failed`/`Stale`/`Never`) are
  `EggPool`-reported facts, never inferred from a transport failure
- `schema_version` must be `1`; a future version is `UnsupportedSchema`, and an
  unrecognized proxy status or an out-of-contract payload is `InvalidStatus` —
  both explicit, nonfatal, and never invalidating the summary plane
- Bounded before rendering: provider rows ≤ 256, provider IDs ≤ 96 bytes,
  reason codes ≤ 64 bytes, finite non-negative uptime. These are the exact
  producer-owned bounds (`MAX_STATUS_PROVIDERS`, `MAX_PROVIDER_ID_CHARS`,
  `MAX_REASON_CODE_CHARS`), not conservative substitutes
- The decoder mirrors EggPool's serialized `ProxyStatusSnapshot` (Plan 153):
  account counts are read from `proxy.routable_accounts` /
  `proxy.enabled_accounts`, and provider identity/observation come from
  `providers[].provider_id` / `providers[].last_observation`. The fixture that
  qualifies the contract records its upstream repo/commit/type provenance, and
  no serde alias preserves any non-upstream field name
- `EggpoolHealthFetchOutcome` keeps 401 (`AuthenticationRequired`), 403
  (`Forbidden`), and 404 (`Unsupported`, not a statistics failure) distinct
- Status reads never trigger an outbound provider probe, quota use, or
  mutation
- `AppState` owns health separately (`health`, `last_health_success_at`,
  `last_health_attempt_at`, `last_health_error`). A failed health refresh keeps
  the previous snapshot visible but marks it stale; summary success/failure
  updates only summary state and vice versa. A period applies to the summary
  payload only.
- The worker reads both planes concurrently inside its single request task, so a
  slow or unavailable status route never delays a valid summary, and one result
  carries both outcomes rather than a collapsed success/failure.
- TUI: a compact header `Health: <ready|degraded|unready|unknown>` token
  (dropped before the identity/window tokens at narrow widths) plus an optional
  bounded footer count such as `Providers: 2 ready · 1 degraded · 1
  unavailable`. Footer priority: worker diagnostics, summary failure, provider
  counts. The four metric rows never change.
- Host normalization errors, including URL forms unsupported by the pinned URL
  parser, are reported as `InvalidEndpoint` rather than transport failures.
  Body-stage typed timeouts map to `Timeout`; ordinary post-header body
  failures stay `NetworkError`.

**Worker** (`spawn_worker`):
- Background task holding one `EggpoolDesiredState` watch receiver and a
  bounded result channel
- 60-second passive refresh when active
- Generation-based staleness like greggd polling
- Plan 151: desired state (`active`/`period`/`generation`) is published
  synchronously and capacity-free, so the input path never waits on the
  worker and no activation, period change, manual refresh, or deactivation
  can be discarded under pressure
- The worker converges on the newest desired state, coalescing states it did
  not observe individually, aborting obsolete in-flight work, and arming a
  fresh request-relative deadline only after a request completes
- Deactivation aborts in-flight work, clears the passive deadline, and emits
  no synthetic result; passive refresh reuses the reducer generation
- Cancellation aborts in-flight work and terminates the worker without a
  queued shutdown command; a closed control channel surfaces
  `EggpoolWorkerState::WorkerUnavailable`

**Local worker state** (`EggpoolWorkerState`):
- `Idle`, `Refreshing` (Gregg published a current desired request and awaits
  its result), `WorkerUnavailable` (Gregg's local worker/control path is
  gone)
- These describe Gregg machinery only. EggPool's own proxy and provider
  service health is a separate fact (Plan 152) and is never inferred here.
- The former `Busy` variant existed only because a full bounded command queue
  dropped the requested transition; it was removed with the queue.

The CLI permits one configured EggPool endpoint. A second `eggpool add`
without `--replace` returns the dedicated `EggpoolAlreadyConfigured`
configuration violation; `--replace` updates the existing entry.

Plan 070 evaluated replacing the command channel with a latest-state `watch`
channel and rejected it as an unreduced state machine; commit `d31d72f` then
adopted nonblocking `try_send` plus `Busy` so the input path could never wait
on a slow worker. The 2026-10-02 review found that drop-on-full can leave
reducer intent and worker intent divergent, including a dropped `Deactivate`
that leaves passive polling armed after returning to Systems. Plan 151
supersedes both designs: `watch` publication is synchronous and capacity-free,
so it cannot block the input path and cannot discard a state-changing
transition. Commit `d31d72f` remains a truthful historical record.

**Periods:** `Hour`, `Day`, `Week`, `Month` — cycled with `longer()`/`shorter()`

**The advertised window is the converged one.** With more than one frontend
attached, the worker is driven with the shortest *active* window across all of
them, and that same value is what fleet state carries. Fleet state must never
hold one frontend's requested period: the reducer accepts a result only when the
period matches, so writing a per-frontend value would reject every result the
worker actually fetched and leave both panes on `Refreshing` with no error. A
frontend that asked for a longer window than the convergence sees its request
recorded and the converged window displayed — the pane only ever shows a period
the daemon actually fetched.

Convergence has to reach the fleet on **every** path that changes the window,
not only on the request path. A disconnect removes an intent and has no request
of its own, so if the fleet learned about a departing window from requests alone
it would keep the old period while the worker switched: every later result is
rejected, the surviving panes freeze on their pre-disconnect summary, and the
worker burns a request per interval with all of them discarded. A converged
window that changes also mints a fresh worker generation, or the worker returns
the previous generation's result and the reducer rejects that as stale too.

**Summary fields:** accounted tokens, cache read ratio, output tok/s, avg TTFT

## Tests

### Unit tests

Every module has inline `#[cfg(test)]` tests:

| Module | ~Lines | Coverage |
|--------|--------|----------|
| `cli.rs` | 30+ tests | CLI parsing, add/remove/replace, port resolution |
| `config/` | 60+ tests | Validation, atomic writes, cross-process locking (`model`/`store`/`validation`/`lock` behind the `src/config.rs` façade) |
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

## Cron observability (Plan 166)

A third plane, owned by the client daemon like the other two and for the same
reason: a per-TUI poller would multiply the fleet's request budget by the number
of open windows, and closing one window would silently stop a fraction of what
the operator is watching.

### Two unequal fetch planes

`/v2/scheduler` is read on a 30-second cadence for every configured system. It
carries only scalar per-job state and no command output, so it is cheap.
`/v2/scheduler/history` is the largest document in the system and is fetched only
on first support discovery and when the summary's `history_revision` changes. A
cadence slower than the metrics interval is deliberate: a load-delayed transition
is visible on the order of a minute, which is fast enough for an operator
watching Gregg and slow enough that scheduler observability does not double a
five-second fleet's request count.

A fresh daemon runs **exactly one** round at startup and then one per period.
`tokio::time::interval`'s first tick is already due, so awaiting it straight
after a startup round fired a second back-to-back round and asked every endpoint
twice before the daemon had learned anything; `interval_at` puts the first
deadline one interval out instead. `MissedTickBehavior::Delay` stays, so a slow
round is never followed by a burst of catch-up rounds.

Each round observes endpoints with a **fixed bound of four reads in flight**,
never a sequential walk. Sequential order meant one endpoint's request deadline
delayed every system behind it, so the effective fleet cadence became a multiple
of the nominal 30 seconds for reasons that had nothing to do with the remote.
Four removes that head-of-line wait and still caps concurrent scheduler requests
far below the metrics scheduler's own in-flight bound, so the two planes cannot
add up to a burst. The bound is a constant, not configuration. Nothing is
spawned: a round is one bounded set of futures on the worker's own task, so a
reload or shutdown drops at most those few reads and none outlives the worker.

### One publication per real change

A frontend document is republished when the **operator-visible** scheduler state
changes — capability, the summary's job rows, the `(epoch, revision)` the
retained records are keyed by, or the stale marker — compared around the write
rather than guessed from a revision counter. `history_revision` is a *history*
revision: it does not move for an ordinary live transition (idle → waiting, a job
starting, a load gate appearing, a next-due advancing), and judging "changed"
from it alone let a valid newer summary sit in the cache unpublished until some
unrelated metrics event forced a document. The same blindness hid error recovery,
because a successful read clears the error while epoch and revision can both be
unchanged. `generated_at_unix_ms` and the local attempt/success timestamps are
excluded from that comparison, so an unchanged successful poll publishes nothing;
a successful read is still recorded as the latest *attempt*, which is bookkeeping
rather than something a row draws.

### A summary and its history are one pair

Summary and history are two independent requests, so a `greggd` restart or a
history change between them yields summary A with history B. That pair is not
history: it is never merged, never advances the history gate (which would tell
the next summary *for B* that its history was already downloaded), and is
reported as a scheduler-scoped `Incoherent` diagnostic while the valid summary is
kept. `generated_at_unix_ms` equality is deliberately not required — a
live-state-only publication can legitimately rebuild the pair without changing
retained history. The next cadence retries.

### Scheduler state is target-bound

A cron observation carries the endpoint it actually polled, exactly like a
metrics result. `Ctrl-R` can repoint a stable id while the old target's scheduler
request is still in flight, and that late answer belongs to a target the
configuration no longer contains, so the engine drops it at the boundary that
knows the current config. Repointing also clears that id's capability, summary,
error, and retained history: all of it described a machine that is no longer
configured here. The history gate is keyed by `(system id, normalized host,
port)` for the same reason — a gate keyed by id alone let a new target inherit
the previous target's "already fetched" answer whenever the two reported the
same epoch and revision. An equivalent spelling of the same normalized endpoint
changes neither, so rewriting a config by hand does not silently discard history.

### Epoch is part of every identity

The gate compares `(epoch, revision)`, not revision alone, and records are
deduplicated by `(epoch, sequence)`. A restarted `greggd` resets
`history_revision` and can reset it to the same small value it used before, so a
revision-only gate concludes "nothing changed" and never fetches the new epoch.
A remote also legitimately reissues sequences from zero, so a sequence-only
identity would drop the new epoch's records as duplicates. **Never key scheduler
state on `sequence` alone.**

A new epoch does not rewrite the previous one: previously observed records are
retained, tagged with the epoch they were observed under, and a job's *state*
comes only from the current epoch's summary. Claiming an old pending job is
still pending across a daemon restart is fabrication.

### Bounded, memory-only, and reproducible

Three bounds, because per-job depth alone does not survive
`endpoints × jobs × depth`:

1. per-job depth (`[cron] cache_history`, max 50),
2. a global record ceiling across every system, job, and epoch, and
3. the remote contract's per-stream output cap, which bounds a record's size and
   so makes a record *count* a real memory bound.

The global ceiling is a **constant, not a setting**: a user who raises the
per-job depth must not be able to turn a bounded cache into an unbounded one.
Eviction is oldest-first fleet-wide, tie-broken by `(system, job)`, so identical
inputs evict the same record. Nothing is written to disk; a client-daemon restart
reseeds from the remote. `display_history` is a *viewport* and is clamped by the
daemon when it publishes, so a frontend cannot make the document larger than the
local bound.

### Two-tier local publication

There is no second socket. The summary always rides along in the existing
document; history records are included only for systems a frontend has open. The
reduction is a union with a per-pair maximum depth, which is order-independent
and retires on disconnect.

**The intent governs transmission and never fetching.** This is the load-bearing
distinction: the daemon polls with no TUI attached, so opening the pane in ten
windows costs the fleet exactly what zero windows cost.

### Capability is not reachability

`Unknown` / `Unsupported` / `Supported` / per-route `CronFetchError`, published in
a vector parallel to `systems` so the scheduler plane stays visibly separate from
the normalized metrics payload. The two are fetched on different cadences, sized
differently, and fail independently, and folding them together would make
"metrics are fine but the cron route failed" hard to express. **Nothing in the
cron plane may reach `Reachability`.** A pre-scheduler `greggd` is a healthy
daemon; marking it errored would badge every old system in a mixed fleet.

### Rendering is bounded and inert

`ui/cron.rs` takes whatever vertical budget `d` and `n` did not claim, and
truncates rows from the end in display *cells* — not bytes and not `char` counts,
which a CJK or emoji job name silently overflows. Every string that becomes a
cell is passed through `sanitize` immediately before construction, so the
guarantee is local to the renderer rather than dependent on a distant chokepoint
staying correct. That includes the diagnostics pane, which renders a config
reload error verbatim: it is the daemon's config-parser output quoted from a
local file, so it is remote-shaped text and is escaped when the document is
adopted, not only where a metrics string is built.

The budget is measured, not estimated. The requested row count and the job
table are both derived from the renderer's own line-producing functions, and the
job table is capped by the same constant the budget is built from, so a wide cron
table cannot consume the whole block and push the selected job's history out of
view. Both sides of the comparison use the same function the frame is built
from: an estimate that drifts from the renderer silently drops the newest record.

The renderer enforces truthfulness rules that are easy to get wrong:

- a missing load observation is `—`, never `0.00` — a gate that fired because
  telemetry was unavailable must not read as a gate that saw a low load;
- a non-child outcome is a real terminal record, named, with `ran —` rather than a
  fabricated `0ms`;
- remote truncation (`stdout+`) and the pane's own row budget are reported
  separately;
- a failed scheduler read is labelled stale and never rendered as a system
  failure;
- a pane with a zero row budget draws nothing rather than indexing outside the
  buffer, and one that cannot be shown still gets a row that says so.

`c` / `Shift-J` / `Shift-K` are the whole key surface. Unshifted `j`/`k` stay
bound to the system list, so a cron-expanded system does not change what the
ordinary keys mean. Cron sub-selection is repaired by stable job name whenever
the selection, the fleet, or the remote's job list changes.
