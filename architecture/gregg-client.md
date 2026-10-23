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
timestamps and latency, so an age that advanced does not force a frame.

The one exception is the EggPool footer's `Updated for Nm`. That age *is*
rendered, and the footer's entire purpose is to say that a summary which last
succeeded hours ago is stale, so an advanced last-success instant is a visible
change and forces a repaint. Attempt ages stay excluded — no renderer reads
them. A
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
  watch slot and a slow frontend skips to the newest state.

  This is the one place the "nothing may block the runtime" rule is **not** yet
  upheld on Windows, and the claim must not be restated as if it were. A
  byte-mode pipe created without `FILE_FLAG_OVERLAPPED` has no non-blocking
  write: `WriteFile` blocks until the peer has consumed every byte, and Win32
  exposes no way to ask a synchronous pipe how much write space it has —
  `PeekNamedPipe` reports only bytes *readable*, which is precisely why the
  read arm above can be bounded and this one cannot. A frontend that stops
  draining its pipe while a larger-than-buffer document is in flight therefore
  stalls the whole current-thread daemon, every other frontend, and
  `gregg daemon stop` with it. Bounding it requires overlapped I/O, which
  changes how the instance is created, accepted, and read as well as written.
  Only a native Windows run can establish that this is correct; treat it as a
  known platform limitation, not a design choice.

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
  ├─ bounded, side-effect-free handshake probe
  │    ├─ attached            → run the TUI
  │    ├─ absent              → launch (below)
  │    ├─ ours but replaceable → launch (below, to rotate under the lock)
  │    └─ refused/foreign/newer → report, start nothing
  └─ launch
       ├─ acquire the config-specific launch lock   (spawn_blocking)
       ├─ re-probe *and re-classify* under the lock
       ├─ rotate (stop + wait gone), *or* detached spawn
       └─ bounded readiness wait, then attach
```

- **The lock is an advisory OS lock**, held only across
  probe/rotate/spawn/readiness and never for the daemon's lifetime. Its file is
  never unlinked and is never read as a signal, so a crashed launcher cannot
  leave behind something that makes the next launch believe a daemon is
  starting. There is no PID file and no registry.
- **The lock's path must be a path a file can really be created at.** On Unix
  it is the endpoint's own file name with a `.launch.lock` extension, beside the
  socket it guards. On Windows it is a real file beside the **config**, named
  after the same identity digest — because the endpoint there is a named pipe,
  the whole pipe namespace is a *device* namespace rather than a filesystem, and
  a path inside it can never exist: opening one asks for a pipe instance with
  that name and fails with `ERROR_FILE_NOT_FOUND`. Deriving the lock from the
  endpoint therefore produced a lock that could never be taken, and the launcher
  failed *before it ever spawned a daemon* — so on Windows neither the TUI nor
  the GUI could start a client daemon at all. The lesson is general: **a lock
  path is not derivable from an endpoint on every platform; derive it from the
  config identity, and prove on the target that the path is one a file can
  occupy.**
- **The lock wait runs on `spawn_blocking`.** It is a blocking sleep loop, and
  the TUI runs on a current-thread runtime, so waiting inline would starve the
  tasks that make the winner's daemon reach readiness.
- **The daemon is spawned detached, with every standard stream null.** On Unix
  that is exactly what it should be: the daemon's streams are `/dev/null`, it
  inherits no descriptor of the launcher's, and it can neither hold a terminal
  nor hold a pipe open.
- **On Windows the same spawn is not equivalent, and the difference is an open
  defect.** Rust has to pass the three null handles to the child, so it creates
  the process with `bInheritHandles = TRUE`, and Windows then hands the child
  **every** inheritable handle the parent holds — not only the standard ones.
  A launcher whose stdout is a pipe therefore leaves that pipe's write end
  open in the daemon, and the pipe never reaches EOF: the command finishes,
  but a reader waits for as long as the daemon lives, which is forever.
  `gregg daemon restart | tee log` hangs this way. The GUI is not affected (its
  streams are a console or a file), and neither is the terminal client in
  normal use; the exposure is a CLI command that spawns the daemon with a
  piped stdout. Closing it needs a handle *list* —
  `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` through `STARTUPINFOEX` — which the
  standard library does not expose, so it means `windows-sys` and `unsafe`
  code in the launcher. It is recorded as outstanding in `plans/README.md` and
  in `apps/gregg-gui/EVIDENCE.md`, and it is **not** fixed by the GUI line.
- **The probe is side-effect-free.** It returns a classification and stops
  nothing. Only *plain absence* authorizes a spawn, and the classification is
  redone under the lock: someone else can bind a foreign service while this
  process waits, and re-probing alone would have spawned over it. The same
  discipline covers the **destructive** step, which is at least as damaging as
  the spawn — rotating on a classification taken *before* waiting on the lock let
  a second launcher tear down the current daemon the winner had just installed,
  because it was still acting on a stale `Replaceable`. So `probe` never stops a
  daemon; `rotate` runs only after the lock is held and the classification has
  been redone.
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
  A crontab is also **never rewritten from an unverified read**: `crontab -`
  replaces the whole table, so a manager failure must not be mistaken for "you
  have no jobs". A timeout, a spawn failure, or output that is not valid UTF-8
  is an error with the exact manual instruction, never an empty table. This is
  why the shared bounded runner drains the child's pipes *while it runs* — a
  pipe holds about 64 KiB, so a table larger than that would otherwise block the
  child in `write(2)` and be killed at the timeout, which is precisely the
  "failure reads as absence" case.
- **Update is prepare-then-quiesce.** The daemon is identified first, the
  candidate is fully prepared and verified, and only then is the daemon stopped,
  the executable replaced, and the daemon relaunched. A failed relaunch is
  reported as partial success with the retry command — including a relaunch that
  started but never became ready, because readiness is the result of a restart.
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

### Shared display vocabulary (Plan 189)

A second frontend — the optional desktop GUI — has to describe the same
scheduler state and the same `EggPool` failures as the TUI. A second
implementation of "load delayed" or "authentication required" is a second
thing that can drift from the truth, so the words are **shared functions**
rather than duplicated strings:

- `gregg::cron::state_label`, `outcome_label`, `load_gate_label`,
  `duration_label`, `elapsed_since`, `countdown_to`, `job_is_running` and
  `job_is_load_deferred` — the scheduler vocabulary. `load_gate_label` is the
  one that matters most: it renders the gate **by meaning** (the load is only
  ever compared above the threshold in the one state where it really is above
  it; a retained observation is labelled as the admission or last gate it was;
  a missing one is spelled unavailable), and that is a rule a second
  implementation would get wrong.
- `gregg::eggpool::outcome_text` and `health_error_text` — the bounded
  `EggPool` failure vocabulary. Raw bodies, credentials and upstream error
  text are never rendered.

The TUI's own output is unchanged: `ui/cron.rs` and `ui/eggpool.rs` call
these, and their existing tests still assert the same words.

### The optional GUI frontend (Plans 189 and 191)

`apps/gregg-gui` is a second frontend for the same client daemon. It is a
**frontend, never a second poller**, and the architecture that makes that true
is a split into layers:

- **A pure projection.** `apps/gregg-gui/src/model.rs` maps one
  `FrontendSnapshot` to one `GuiViewModel` and touches nothing else. It has no
  threads, no channels and no Slint. The truthfulness rules live here — absent
  is absent, unpolled is not offline, a missing value is `—` and never `0`, the
  selection is keyed by the stable system id — so they are testable **without a
  desktop session**. A rule that is only enforced by a renderer is a rule that
  will be broken by the next renderer.
- **A protocol owner.** `apps/gregg-gui/src/bridge.rs` owns the client-daemon
  attachment and the intents. It holds **no HTTP client, no poll scheduler, no
  parsed remote configuration and no history store**; the daemon owns all of
  that. A GUI that opened its own remote connection would double the fleet's
  request budget exactly when the daemon is unhealthy — the failure this
  architecture exists to prevent.
- **A presentation owner.** `apps/gregg-gui/src/main.rs` owns the Slint
  components. Slint components are not `Send`, so every mutation happens on the
  UI thread, and the bridge's notification callback only *schedules* work with
  `slint::invoke_from_event_loop`. There is **no polling timer**: an idle GUI
  redraws for no reason at all, and a redraw only ever happens because the
  bridge published something.

The `EggPool` and cron intents are **desired state**, not a command queue. The
bridge keeps the latest desired state, republishes it whenever it changes,
republishes it when a write is rejected by the bounded request channel, and
republishes the **new full** desired state on a reconnect — the daemon that
answers a new attachment has never seen the old one, so a delta would be
meaningless. An intent governs what the daemon *transmits*, never what it
*fetches*, so a released intent costs the fleet nothing.

The GUI has **one surface**: a tray pulldown (Plans 191-192). The separate monitor
window is gone. The pulldown is a borderless, always-on-top panel with **two
views**: the **fleet**, which is one line per host, and the **detail**, which
is the selected host's metrics, drives and interfaces, its scheduler jobs and
records, and the `EggPool` plane. The pulldown's own footer moves between
them. The host list is bounded and scrollable, so a large fleet cannot push
the panel off the desktop.

**The tray's click opens the pulldown, and the pulldown carries the content**
(Plan 192). The native tray menu is **not** the dropdown: it is the context
menu, and on macOS it is **detached entirely**, because an attached menu
swallows the click that has to open the pulldown. The mechanism is Slint's
`SystemTrayIcon.clicked` — a native menu with no entries **detaches itself**,
so the platform routes the tray's click to that callback instead of popping the
menu — on macOS, on Windows (where a right-click still opens the native menu)
and on Linux. There is **no launcher entry**: the pulldown carries `Reload`,
`About` and `Quit` itself. The pulldown remains a Slint window, because a
native menu cannot host Slint widgets, and it remains anchored to the
platform's trailing corner, because Slint 1.18 does not expose the tray item's
position. Two consequences for this document's concerns: the protocol is
untouched, because the pulldown is a frontend over the same client daemon and
the GUI still opens no HTTP client of its own; and the `DaemonRequest` set
still gains no message.

Closing the panel **hides** it and leaves the tray and the observation
running; Quit is the only thing that ends the GUI, and it never ends the client
daemon. `Esc`, the tray's click and the panel's own close control all dismiss
it. There is **no click-away dismissal**, because Slint 1.18
exposes no focus-lost event: a window cannot be told that the click landed
elsewhere, and the panel is always-on-top, so it stays over whatever is
behind it. That limit is stated in the user documentation rather than papered
over.

The panel is placed on the display it opens on, and it keeps the position and
size the operator gave it. Slint 1.18 exposes no tray-icon position and no
pointer position, and `winit` 0.30 knows a monitor's bounds but not its work
area, so the panel is anchored to the **trailing edge of the display, on the
side where the desktop's own shell lives** — the top on macOS and Linux, the
bottom on Windows — and clamped so it is always fully on the display. The
operator can drag the panel by its header strip and resize it by the grip in
its corner, which is what a borderless panel needs and what a window frame
would otherwise have provided.

Two placement facts are load-bearing and were **defects** before Plan 192 fixed
them. Slint's `with_winit_window` "will only succeed when the event loop is
active", so `placement::work_area()` answers `None` before the loop runs: the
**first open is deferred into the event loop**, or the panel is never
positioned, never clamped, and never size-clamped. And the geometry is
**persisted**: it lives in a small state file beside the config —
`<config>.gui-state`, mirroring the `<config>.lock` convention — read bounded
(4 KiB) and validated, and written atomically, so a corrupt or out-of-range
file falls back to the default rather than failing the GUI. The size is
computed by **Rust**, not by Slint, because Slint 1.18 cannot expose a
layout's preferred size to a `.slint` file; `placement::metrics` holds the
numbers and a test asserts them against the `.slint` literals, so the two
cannot drift. The GUI prints one machine-readable line —
`pulldown=placed x=… y=… width=… height=… area=…` — so the native exercises
assert the **position**, not only that a window exists.

**The persisted placement is an offset, not a position.** The anchor is
computed from the work area of the display the window is on, so it is already
aware of the display's size and scale — but a remembered **position** was
allowed to win over it, and a position names a place on *one* display. A
pulldown placed on a 1512-point laptop display and opened on a 3840×1080 one
landed 2382 points from the tray: on screen, the right size, and nowhere near
the icon whose click opened it. What is persisted is therefore the **gap at
the trailing edge** and the **gap at the edge the panel hangs from**, measured
against the display the panel was placed on, and the file's older `position=`
key is **not read** — a place on the display it was measured on cannot be
carried to a display it was never measured against, and reading it would
reproduce the defect the offset exists to fix. The offset is measured against
the position the GUI **asked for**, never against the position the window
reports back: in another application's full-screen space macOS draws the
window 26 points lower than it was asked to be, and a placement measured from
the window's own report would carry that shift into the memory, so the panel
would open a little lower every time and never come back.

That "hide" is only safe when there **is** a tray to bring the window back. On
Linux the tray needs a `StatusNotifierItem` host, and a plain GNOME session has
none — Slint's `ksni` backend then fails to register, silently, and the GUI is
left with a panel it can hide and nothing that can show it again. An operator
in that state would have an invisible, unquittable process. So the GUI **probes
for a tray host** before it hides anything: on Linux it looks for the
`org.kde.StatusNotifierWatcher` name on the session bus, which is exactly what
Slint's own backend requires, and it reports the answer in one machine-readable
line. When there is **no** tray, closing the panel **quits** the GUI instead
of hiding it, and the panel says so. With no tray the panel is also opened at
startup, so it is never a hidden surface. The probe is re-run at close time rather
than taken from startup, because a tray host can appear after the GUI started,
and it is bounded, because a wedged bus must not hang the close. The decision
itself is a pure function, so both branches are unit-tested without a desktop
session.

The panel's lists are **bounded and scrollable**. The host list, the metric
rows and the cron job list grow with the fleet, and a fixed-size panel cannot
hold an unbounded list: before, a 25-host fleet pushed the metrics, cron and
`EggPool` panes out of the window entirely. Each list therefore lives in a
scroll container with a bounded height, so the panel keeps its size and every
pane stays reachable.

The pulldown's **appearance** is a load-bearing part of the same story, and one
of its facts was a defect. A macOS dropdown is a rounded card of translucent
material with a hairline edge and a soft shadow, and the pulldown is built to
match that: the card's edge is a **measured macOS hairline**
(`NSColor.separatorColor`: `rgba(255,255,255,0.098)` dark, `rgba(0,0,0,0.098)`
light) rather than Slint's `Palette.border`, which is 45 % black in one pixel
in the light scheme and reads as a hard dark line; the fill is
`Palette.background` at 94 % opacity, the closest the software renderer comes
to a vibrant material; the shadow is **fourteen one-pixel rings** with a
Gaussian alpha profile, because Slint's `draw_box_shadow` is an empty stub in
the software renderer and a few hard-edged rings band visibly; and the pointer
sits on the **trailing edge** — the edge the tray is on — because the card
anchors to the trailing corner and a pointer at the card's centre pointed at
nothing.

That work was blocked by a defect in the **presentation layer**, not in Slint
and not in the GUI: on macOS a frameless Slint window *is* a transparent
`NSWindow`, and `winit` asks for exactly that. But the framebuffer that
carries the window's pixels goes through `softbuffer`, whose macOS backend
declares the pixel format as `CGImageAlphaInfo::NoneSkipFirst` — **no alpha
channel at all** — so CoreGraphics presents every transparent pixel opaque.
The pulldown's shadow margin, which is transparent by design so the desktop
shows through it, was therefore presented as a **thick black frame**, and the
card's rounded corners were black squares. The fix is one word —
`CGImageAlphaInfo::PremultipliedFirst`, the same 32-bit layout with the alpha
the buffer already carries — and it is carried by a **vendored copy** of
`softbuffer` 0.4.8 under `apps/gregg-gui/vendor/softbuffer`, because 0.4.8 is
the latest release and the defect is still in it. The change is inside Apple's
platform only; every other backend is byte-identical to upstream. The full
rationale, the measurements that established the defect, and the removal
condition are in `apps/gregg-gui/vendor/VENDORED-PATCH.md`. **Do not drop that
copy**: without it the pulldown cannot have a soft shadow or rounded corners
on macOS at all, because the alternative is an opaque square window.

The pulldown's **fleet view** is the TUI's condensed fleet row **turned on its
side**: every host is a **block of its own**, so the fleet is a table of blocks
rather than a table of rows. A block is a **header line** and the **five used
shares** under it.

The header line **names the host and carries the readings that are not used
shares** — the I/O wait, the load, the core count and the frequency — in the
TUI's own words and the TUI's own order: `Deadpool  IO 40.7%  9.87/11.42/12.26
16C  912.96MHz`. The block names the host and **not where it is**: the endpoint
is the detail view's business, the way the TUI's condensed row does not carry
it either.

The five rows are the TUI's five used shares — `CPU`, `MEM`, `SWP`/`COMMIT`,
`DISK` and `NET` — and each row carries **only its share**: the bar takes the
**full width of the row** less the percentage, and the percentage is the last
thing in the row. The bar carries the precision and the number carries the
glance. A reading that is **not** a used share has no bar, so it can never be
drawn as one: the I/O wait in particular is a fraction of *time*, and a bar
that meant something else would misread.

A host that is not online has **no rows at all** — its header says why it is
offline and nothing else, which is the TUI's own rule. The list is **bounded**
and scrolls: the pulldown is a popover, and a fleet can be large.

The **selected** host is marked with a quiet tint and by its name in the accent
colour, and deliberately **not** with the platform's selection colour: that
colour is a saturated accent, and the palette's own foreground for it is black
on macOS, which draws a bar as what reads as a hole. A bar is a *measurement*,
and a measurement looks the same wherever it is.

The **detail pane** is **the fleet view's own block, opened up** — not a second
rendering of the same numbers. The selected host's block is drawn by the
**same `HostBlock` component** the fleet view draws every host with, so the
two views cannot drift apart and there is **exactly one bar per used share in
the whole panel**. The detail view has no second set of bars, no second set of
column widths and no second set of headings for the readings: the block says
`MEM 39%` and the detail says the same thing in the same bar, in the same
columns, at the same widths.

Under the block, the extra facts are **one condensed list**: a quiet heading
and a two-column row under it — the name on the left, the value on the right,
in the quiet colour, because the name is what the row is about and the value
is what the name says. The groups are:

- **`RESOURCES`** — the numbers **behind the shares** the block already shows.
  The block says `MEM 39%`; this says `MEM  6.0 / 15.5 GiB`. The row's name is
  the block's own label, so the two are obviously the same measure. The swap
  or commit row is named by what the host reported, like the block's.
- **`DRIVES`** — each mount's own facts in the TUI's own words: the used and
  the total, the drive's own share, and what is left of it. The free space
  falls back to what is left of the total when the platform reported none,
  which is the TUI's own rule. The read and write rates are added when the
  platform reported them, and **not** invented when it did not.
- **`NETWORK`** — the **aggregate first** and then each interface, because the
  block's network share is the aggregate's share of its link. A link capacity
  that was **not measured** is **not stated**: the row says `RX` and `TX` and
  no `LINK`. A capacity of zero is a capacity, and a missing one is a missing
  one; they are not the same fact.

The detail is **one scroll container**, so it scrolls as a whole and there is
one scroll model.

**A hidden pulldown does not rebuild the view, and the window is not built
until it is opened.** The daemon publishes a document every few seconds, and
the whole model is a few thousand formatted strings. Building that for a panel
nobody is looking at is work nobody can see, so a hidden panel is **not**
re-rendered: the tray keeps its two facts — the fleet's counts and the summary
line — straight from the document, and the pulldown is rebuilt **the moment it
is opened**, so what it shows is the newest document and not the last one it
happened to be open for.

The pulldown's **component** is built on the **first tray click**, not at
start-up: a window costs about thirty megabytes before it draws anything, and
a background GUI that nobody has opened yet should not be paying for it.
`--panel` and a desktop with no tray still open it at start-up.

**What the background GUI costs, measured.** On macOS 15 (arm64) with the real
daemon and a seven-host fleet, the idle GUI's resident set is **45.5 MB** and
its CPU is **about 1.6 %** of one core; the same binary with the panel built
at start-up measured **82 MB**, so the deferred window is worth about
**37 MB**. The rest of the cost is **Slint's software renderer and the window
server, not the model**: the same measurements on a *trivial* Slint app give
**27–35 MB** for an event loop with no component at all, **44 MB** with a tray
icon and no window, and **62–74 MB** with a window that is never shown. The
hidden-panel rule itself is **measurably neutral** (the same binary before and
after differs by 0.6 MB of resident set and 0.06 s of CPU over a minute), and
that is reported as what it is: the cost of an idle GUI is the daemon's
document arriving and being decoded, which the TUI pays too. There is **no
leak** (`leaks` reports 287 leaks for 14 KB, all in Apple's own XPC and
Objective-C machinery), and **dropping the window does not give the memory
back** — a dropped component's resident set stayed flat over ninety-six
seconds — so the deferred window is a **one-off** saving. macOS reclaims most
of the rest over hours: a five-hour-old instance of the same binary measured
**14–30 MB**.

A tray application is a **background** application, and on macOS that has a
concrete consequence: an `NSStatusItem` menu does not activate the app that
owns it, and a background app cannot bring its own window to the front with
`orderFront:`. A GUI launched the way a real one is launched — a login item, a
launcher, an agent — is a background app from its first moment, so opening the
pulldown would silently do nothing. The GUI therefore **activates itself**
before it raises the panel, through `apps/gregg-gui/src/platform.rs`. The
activation is the only platform-specific code in the GUI, it is macOS-only
(`objc2`/`objc2-app-kit`, `NSApplication` and `NSWindow` features only), and
it is a no-op on Windows and Linux, where showing a window the process owns is
enough.

**A tray pulldown has to reach another application's full screen.** A
full-screen application owns a **space** of its own, and a window that is not
in that space is not on the screen at all: the click reaches the tray, the
pulldown is built, and the operator sees nothing happen. macOS keeps a **Dock**
application's windows out of another application's space, and that is fixed
when the window is created — no window level and no collection behaviour
changes it. So the GUI installs the **`Accessory`** activation policy (a
menu-bar-only application) before any window exists, through winit's
event-loop builder, and then tells the pulldown's **native window** to join
every space (`CanJoinAllSpaces`, `FullScreenAuxiliary`, `Stationary`,
`IgnoresCycle`), to stay visible when the application is deactivated, and to
sit at **`NSPopUpMenuWindowLevel`** — the level a native status item's own
menu is drawn at, because `always-on-top` only reaches the *floating* level
and the floating level is below full-screen content. The native window is
found by its **title**, and that title is **owned by the `.slint` file** —
bound to a property the Rust side reads — so the two cannot drift apart: a
title that lived in two places would be a title that could drift, and the
overlay would silently stop working. The consequences are deliberate: the GUI has **no Dock icon**, and
the menu bar stays the other application's, which is what a native menu-bar
extra does. The menu bar itself is macOS's own business — in full screen it
is hidden, and the operator brings it back by moving the pointer to the top of
the screen.

### The GUI's qualification record (Plan 190)

The GUI is a **source build**. It has no installer, no bundle and no release
asset, and it requires an existing Gregg installation rather than bundling a
second `gregg`.

What was proven, and how:

- **The request budget is unchanged.** `apps/gregg-gui/src/evidence.rs` runs
  the real daemon against two counted remotes — one scheduler-capable, one
  answering `404` for the scheduler routes — with a terminal-style frontend and
  the GUI's `Bridge` attached to the same config. `/v2/status` is flat at four
  polls per window in every phase. Natively, on 25 synthetic endpoints, the
  counts are identical with the daemon alone, with the GUI as a tray icon and
  with the panel visible.
- **The intents govern transmission, not fetching.** An opened cron job grew
  the local IPC document and left the remote's history-fetch count at one. The
  `EggPool` pane drove one daemon-owned worker on the converged window.
- **The cost is a fraction of a core.** The GUI's CPU is roughly 0.5%–1.5% of
  one core, and the daemon's wakeup count rises by about 2,150 over a 44-second
  period with a frontend attached — the "one timer wake per attached frontend
  every 20 ms" that `clientd/daemon.rs` already documents. The GUI adds no
  *new kind* of cost.
- **The memory is Slint's, not the protocol's.** Hiding the panel saves under
  a megabyte; the rest is the software renderer and the system-font machinery.

The decision is **accept**, and it is a decision about a **source build on a
developer's machine**: 72–83 MB resident and a ten-megabyte binary are
unremarkable for Slint and comparatively substantial for a utility meant to
sit in the menu bar all day, which is a **release-time** question rather than a
defect. What is **not** claimed is the **interactive** Windows and Linux
native acceptance: a Windows run needs a Windows host and a Linux tray run
needs a desktop session with a `StatusNotifierItem` host, so those two targets
are not shippable on the strength of this line. Plan 190's status is therefore
**implemented, pending release qualification**, not closed. The measured
artifact is `apps/gregg-gui/EVIDENCE.md`.

### The GUI's own CI, and what it caught (Plans 187-190, corrective pass)

The GUI is a **separate Cargo workspace** with its own lockfile and its own
Rust 1.92 floor, and the root workspace excludes it. That separation is
correct — it is what keeps Slint out of the Rust 1.89 MSRV job — but it has a
consequence that is easy to forget: **no root job builds or tests the GUI**.
A `#[cfg(windows)]` branch that nothing else compiles, and a UI requirement
that nothing asserts, both ship unnoticed.

That is not hypothetical. The `#[cfg(windows)]` branch of `is_executable_file`
in `crates/gregg/src/clientd/launch.rs` asked `std::fs::Metadata` for its
`extensions()` — a method that does not exist — and broke the **whole** `gregg`
crate on Windows; the Windows CI job was red for every commit of the GUI line,
and the GUI could not have been built for Windows at all. Meanwhile the
no-tray-host fallback this architecture describes above was not implemented, so
a plain GNOME session would have had a GUI that hid its only window and left
the operator with nothing to press.

The corrective pass fixes the branch (the extension lives on the **path**, and
the rule is the one Windows itself applies when it is handed a path to run),
locks it with Windows-gated tests, and gives the GUI workspace **its own CI**:
one job that formats, lints, tests and release-builds it on macOS, Windows and
Linux **natively**, plus native tray exercises on the Windows and Linux legs.
`scripts/check-gui.sh` runs the same checks on a developer machine.

### Daemon executable selection (Plan 188)

The client daemon is the `gregg` binary. A frontend that *is* `gregg` can launch
its own image; a frontend that is **not** — a separately named desktop GUI —
must name the client explicitly, because spawning itself with `daemon run`
produces a process that exits immediately and a readiness timeout that blames
the daemon for a mistake the launcher made.

```rust
pub enum DaemonExecutable {
    Current,        // this frontend's own image (the `gregg` TUI)
    Path(PathBuf),  // an explicit path to a Gregg client
}
```

- `ensure_running(store)` is the TUI's entry point and is exactly
  `ensure_running_with_executable(store, &DaemonExecutable::Current)`. There is
  one launcher, not two: `probe`, `classify`, `rotate`, `wait_ready` and the
  lock are shared, and the ownership rules do not vary with the executable.
- **The exact resolved path is spawned**, never a name from `PATH`. A stale or
  shadowing `gregg` earlier in the path must never become the daemon a frontend
  then attaches to. The path is made absolute before it is used, so a spawn can
  never fall back to a `PATH` lookup.
- **The executable is resolved before anything destructive, and a resolution
  failure is raised only where a spawn is authorized.** A daemon that is
  already running needs no helper at all, so "the GUI packaged the wrong
  helper" must never stop or refuse a working daemon. A daemon that *is*
  replaced must be replaced by something, so an unusable executable is reported
  *before* the old daemon is stopped.
- **A frontend may not launch itself.** `DaemonExecutable::Path` refuses a path
  that canonicalizes to this frontend's own executable, so a symlink, a
  relative spelling or a `..` segment cannot slip past. `Current` is exempt:
  for the `gregg` TUI its own image is exactly the right binary.
- **A missing or unusable executable is an actionable diagnostic**, not a
  readiness timeout. The path, the reason, and the fact that the file must be
  executable are all reported.
- **Identity is the caller's decision.** `verify_gregg_client_executable` runs
  `<exe> version` under a bounded deadline and requires the answer `gregg
  <version>`. A user-selected external executable must pass it before it is
  trusted; an installer-owned adjacent or bundled `gregg` needs no probe,
  because it is owned by the same installation.

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
- **A dead poll or `EggPool` task stops the daemon.** Per-endpoint panics are
  already converted to `Cancelled`, so a closed channel means the task itself
  ended: the scheduler deliberately stays alive with zero endpoints so `Ctrl-R`
  can add systems, and the `EggPool` receiver is only replaced when the worker is
  rewired. Under shutdown that is orderly; at any other time the daemon exits
  with `SchedulerStopped` / `EggpoolWorkerStopped` rather than retiring the
  receiver, because continuing would serve documents whose numbers can never
  change again — indistinguishable, to an operator, from a healthy quiet fleet.

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
- **An unrepresentable optional capacity is omitted, not fatal.** A v2 payload
  whose only fault is a network capacity the protocol cannot represent — a
  producer that mis-casts a collector structure publishes a packet counter as
  a link capacity — is **repaired**: the capacity is dropped and the payload is
  accepted only if the *repaired* payload validates. Every other fault is
  still rejected, so a repair can never smuggle an invalid payload through.
  This is the collectors' own rule for a family they cannot measure: omit it,
  and never fabricate a zero for it. Without it, one unrepresentable capacity
  failed the whole snapshot and a reachable host read as `offline (invalid)`,
  while the rest of the fleet — and every other reading on that host — was
  sound.

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

**Re-anchoring on a live→dead transition:** `viewport_top_id` and
`selected_id` both name a *host*, but the display order is reachability-sorted,
so a host that fails is pushed to the bottom of the list. An identity anchor
therefore follows the host it names down, and takes the whole window with it:
when the anchored host is the one that fails, every live host above it leaves
the screen. `AppState::repair_viewport_anchor` re-anchors the window to the
host that now occupies the anchor's previous display position (preferring a
live one), so the dead host moves to the bottom of the list and the window
stays where it was.

The selection needs the same repair, because the window is kept on the
selected system: `AppState::repair_selection` moves the selection to the live
host that took the failed host's place. Without it, a selection left on a
failed host drags the window down to the bottom of the list and undoes the
anchor repair — which is what let the defect survive in a pane too small to
hold the fleet, the case a fleet that fits on one screen cannot show. The
anchor and the selection are repaired independently (they name different hosts
in general) but by one rule, `AppState::live_replacement`, so when they name
the same host — the usual case, since the top of the list is both — they
agree.

Both repairs fire only when the host **failed**: it was `Online` and is now
`Offline`. `Pending` means "no poll result received yet", so a host that was
reset to pending has not failed — a reload repointed it on purpose, and a
client daemon that restarts rebuilds the fleet as pending. They also fire
only when the host's **target is unchanged**, so a host a reload repointed is
not treated as a failed host. An anchor that merely shifted *up*, because
hosts above it went offline, still names the same top row and is left alone.
The window must not slide when unrelated hosts change state. Both repairs are
part of the single fleet writer (`AppState::adopt_snapshot`), so there is
still exactly one scroll model. The selection repair runs before
`repair_cron_selection`, because the retained cron job is derived from the
selected system.

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
| `Ctrl-D`/`Ctrl-U` | Page forward/back (aliases of `PageDown`/`PageUp`) |
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
memoized row. The stable failure category is **not** condensed away: an offline
status row appends the same ` (refused)` / ` (http) HTTP 503` suffix the normal
view renders, inside the same width budget, because the compact view is the
same operator surface and must not drop the only statement of *why* a system is
offline. A pending row has no poll result and so never carries a reason.

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
- Convergence is live **while a completed result is backpressured**, not just
  while an HTTP request is in flight. The bounded four-slot result channel is
  the one place the worker can park after a fetch has already finished, and
  Plan 176 had made only shutdown observable there. `deliver_result_or_interrupt`
  therefore selects cancellation, then `control_rx.changed()`, then the result
  reservation, and inspects the newest retained state with `borrow_and_update()`.
  A state that is inactive, or active with a different period or generation,
  abandons the completed result and is handed straight back to the worker to
  converge on — so leaving the pane, or switching window, is never delayed
  behind a result slot nobody is draining
- An *equivalent* publication is consumed and the wait continues: the completed
  result is still the answer for that state, and discarding it would turn a
  redundant repaint into a silently lost result
- Ordering is `biased`: shutdown first, then the superseding state, then the
  send. The newest intent is authoritative, so an obsolete completed result
  need not be delivered ahead of intent already known to be newer
- The slot is taken with `reserve()` rather than by sending directly, so a lost
  race returns the value instead of consuming it
- Passive refresh is armed **only** after a delivered current result; an
  abandoned one arms nothing, because a deadline for superseded intent would
  schedule a fetch nobody asked for
- This is not license to make delivery lossy: with no shutdown and no
  superseding state the four-slot channel applies ordinary bounded backpressure
  and results are never dropped
- Deactivation aborts in-flight work, clears the passive deadline, and emits
  no synthetic result; passive refresh reuses the reducer generation
- Cancellation aborts in-flight work and terminates the worker without a
  queued shutdown command; a closed control channel surfaces
  `EggpoolWorkerState::WorkerUnavailable`

**Local worker state** (`EggpoolWorkerState`):
- `Idle`, `Refreshing` (Gregg published a current **active** desired request and
  awaits its result), `WorkerUnavailable` (Gregg's local worker/control path is
  gone)
- `Refreshing` is only ever published for a converged-*active* intent. An
  inactive desired state aborts in-flight work and emits no synthetic result,
  so a deactivation must return the field to `Idle` rather than await an answer
  that cannot arrive — the reducer's only escape for "a request that will never
  arrive" is `EggPoolFetchOutcome::Cancelled`, which is never constructed. Both
  ordinary deactivations reach this path: a fresh client whose pane is closed
  (its first intent is `{active: false}`) and the pane closing on the last
  attached frontend. Deactivation still supersedes, so the generation moves and
  a result the worker already fetched is rejected as stale.
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

### A reload preempts the round it lands in

Reload and cancellation are selected **inside** an active round, not only between
rounds. Waiting until the round finished made reaction time proportional to the
superseded fleet: with a window of four and a request deadline that can reach a
minute, an accepted reload sat behind a stored permit while the worker burned
through `ceil(fleet / 4)` timeout waves — so "reload wakes the worker
immediately" was materially false, and a newly added or repointed system could go
unobserved for minutes.

When reload wins, the round stops enqueueing work, drops its in-flight requests
(the futures are the requests, so dropping them cancels them), and returns a typed
outcome. The worker then re-reads the endpoint list, prunes history-gate keys
absent from the new normalized target set, and starts a fresh round at once —
without waiting out the cadence. Cancellation ends the worker the same way, so
shutdown does not wait on a remote request deadline either. The concurrency bound
is unchanged and rounds never overlap: a tick that comes due during a long round
is handled by `MissedTickBehavior::Delay`.

"Preemptible inside the round" covers **both** places a round can wait, not just
the HTTP reads. A fetch that has already finished still has to reach the bounded
64-slot worker-to-engine channel, and that send is the one await that can park
for an unbounded time — the engine is the only receiver, so a full channel means
"not draining". `deliver_observation` therefore selects cancellation, then
reload, then the send, rather than awaiting `send` bare. Without it a reload
could sit behind receiver capacity for work the operator had already superseded,
which made the claim above false under exactly the pressure it exists for.
Reload and cancellation are ordered *ahead* of the send so a slot freeing at the
same instant as a stored signal cannot let the obsolete observation win, and its
pending history-gate commit with it.

This is bounded loss of superseded work, not lossy steady-state delivery: with
no reload and no cancellation the send still applies ordinary backpressure and
the round waits for a slot rather than dropping an observation. The channel stays
bounded at 64 and the in-flight window at four.

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

A coherent pair advances the gate **only after its observation has been handed to
the engine**. Settling a fetch and delivering what it produced are two steps, and
settling used to advance the gate itself: a document that was then dropped — by a
reload preempting the round, by cancellation, by a blocked hand-off abandoned to a
signal, or by the engine channel closing — still told the
next round "already fetched", so an equivalent or unchanged target could suppress
its history fetch indefinitely while no cache ever received a byte. `settle` now
returns the observation plus a *pending* gate commit, and the round applies that
commit only after the bounded worker-to-engine channel accepts the observation.
Every non-delivery path — superseded, cancelled, or undeliverable — returns before
that line, so no abandoned observation can ever reach it. The invariant is
simply: **"the gate says fetched" implies the coherent document reached the
engine.** The gate stays worker-private; "delivered" means accepted by that one
bounded channel, and no acknowledgement traffic was added.

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

A fourth bound sits beside them: **a per-system ceiling on retained job
histories** (`MAX_CRON_JOBS_PER_SYSTEM`, 64). A remote configured with more jobs
than that is a supported configuration, not a misconfiguration, so the client
must bound it rather than grow per remote configuration. It is enforced
unconditionally on every history merge — separately from the stale-job sweep,
which answers a different question (has this job gone away) and is a no-op when
every cached job is still live.

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

**A newline means two different things, and the sanitizer has two modes for
them.** `sanitize` treats `\n` as a *separator*, because job stdout and stderr
are multi-line bodies. `sanitize_single_line` treats it as an *anomaly* and
renders it `^J`, because a drive name, job name, interface name, or hostname
occupies exactly one row. That distinction is load-bearing rather than stylistic:
the cell builder drops a control grapheme instead of breaking the row, so an
unescaped newline in a one-row field is *deleted* and the text on either side
fuses into an identifier the remote never reported — `/dev/sda1\nroot` renders as
`/dev/sda1root`, a device that does not exist, with no marker that anything was
lost. `truncate_width` is the chokepoint that applies the one-row mode, because
every caller builds a single terminal line and that is the last point where the
field is known to be one row.

The budget is measured, not estimated. `block_rows` builds the block once, as
ordered groups, and *both* the requested height (`desired_rows`) and the emitted
height (`render`) read that same list — so the two cannot drift, and an estimate
that no longer matches the renderer cannot silently drop the newest record. The
groups are spent in a deliberate order:

1. the `CRON` header and, when the last read failed, the stale-scheduler notice —
   which is therefore *counted*, not bolted on at render time;
2. the selected job's header and its **newest** record. This section is
   *reserved*, not merely ordered first: it is what the operator opened the pane
   for, so a sixty-job table must never be the part that gets cut;
3. the bounded window of job rows;
4. the older records.

When the rows do not fit, one row is reserved for `… more cron rows not shown`,
so the block always states what it left out, and the job window shrinks *around
the selection* rather than being cut at a fixed boundary.

The job table is a **window, not a prefix**. `job_window` follows the selected
job and clamps at both ends, so `Shift-J` past the old 24-row slice cannot make
the highlighted row disappear, and the hidden jobs are counted
(`… N more jobs above` / `below`). There is still exactly one scroll model: the
window is derived from the selection the operator already has, so no second
persistent scroll state, mouse handling, or inner scrollbar was introduced.

The renderer enforces truthfulness rules that are easy to get wrong:

- **a load row states the relation it actually means.** `greggd` deliberately
  retains the gate decision that admitted a job, so a running or idle row can
  legitimately carry a reading *below* the threshold; rendering every gate as
  `observed > threshold` printed lines like `1.20 > 8.00`, which are not merely
  ugly but false. Only `load delayed` compares above the threshold
  (`load15m 9.24 > 8.00`); a running row names the gate it started under
  (`start load15m 1.20 <= 8.00`) and an idle or slot-waiting row names the
  previous one (`last gate load15m 1.20 <= 8.00`) so an old reading is not
  presented as current load. A job with no load gate at all renders no load token.
- a missing load observation is `unavailable`, never `0.00`, and never a
  comparison at all — `load15m unavailable (max 8.00)` is the whole claim. A gate
  that fired because telemetry was missing must not read as a gate that saw a
  low load, and there is no number to compare in that state;
- **elapsed states use elapsed grammar.** `for 3m`, `pending 17m`, `queued 2m`,
  and countdowns `next 11h`, `retry 20s`. The old shared `age()` helper produced
  `running for 3m ago`, which reads as a contradiction. An *elapsed* age that
  lands in the future (clock skew, a transition captured before its own start)
  degrades to a saturated `0ms` rather than underflowing a `u64`, because there
  it means "just began". A *countdown* whose instant has already passed is a
  different relation, not skew, and reads `due` / `retry due`: `next 0ms` is a
  confident duration for a schedule that has not run yet, and on a pane whose
  purpose is to say when a job runs it is the fabricated-value class;
- **record clocks are labelled `Z`.** Those fields are UTC instants while the
  schedule column beside them is the remote's *local* civil cron, so an
  unlabelled `07:00` next to a `03:00` schedule read as a contradiction across a
  timezone, with nothing to explain it. The protocol carries no remote timezone
  history, so the honest fix is to label the clock; reconstructing
  scheduler-local civil time is a separate protocol change, not a renderer one.
- a non-child outcome is a real terminal record, named, with `ran —` rather than a
  fabricated `0ms`;
- **the schedule column is compacted only when it fits.** `schedule_label` renders
  `03:00` for `0 3 * * *` and `weekly Wed 02:30` for `30 2 * * 3`, and keeps the
  operator's own text verbatim everywhere else. "Compacted in shape" is not
  permission to rewrite: an all-digit field can still overflow `u32`, and
  defaulting it to `0` rendered `99999999999999999999` as a confident `00:00` —
  a fabricated midnight on a pane whose whole purpose is to say when a job runs. A
  field that does not parse as a time keeps the verbatim string. A current
  `greggd` rejects such a schedule at config validation, so this needs an older,
  faulted, or hostile remote, and remote scheduler data is untrusted input;
- remote tail truncation (`stdout+`), the pane's own record-depth drop
  (`more lines not shown`), and the pane's viewport truncation
  (`… more cron rows not shown`) are three different facts with three different
  markers. The middle one deliberately carries **no** stream name or `+`: it
  reports a local budget, not a cut the remote made, and reusing `stdout+` for it
  would collapse two facts into one label;
- a failed scheduler read is labelled stale and never rendered as a system
  failure;
- a pane with a zero row budget draws nothing rather than indexing outside the
  buffer. When the card is *exactly* full there is no row to say "needs more
  room" in, and the first cron row would land on the next system or the footer,
  so the layout allocates zero and `cron::render` clips to the rect it was handed
  rather than trading a missing pane for a corrupted neighbour.

`c` / `Shift-J` / `Shift-K` are the whole key surface. Unshifted `j`/`k` stay
bound to the system list, so a cron-expanded system does not change what the
ordinary keys mean. Cron sub-selection is repaired by stable job name whenever
the selection, the fleet, or the remote's job list changes.
