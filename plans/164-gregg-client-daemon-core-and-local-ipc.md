# Plan 164: Gregg client-daemon core and local IPC

Status: planned.

Depends on: Plan 161 and the settled current Gregg client architecture through
Plan 160. It may proceed in parallel with Plans 162-163. Independent of Plan
091.

## Objective

Move Gregg's remote polling/data-plane ownership out of each TUI process and
into one config-specific per-user background client daemon, while keeping the
existing gregg executable as both daemon host and frontend.

The ordinary post-plan architecture is:

~~~text
gregg daemon run
  +-- Systems Poller + PollScheduler
  +-- normalized fleet state
  +-- EggPool worker/state
  +-- config reconciliation
  +-- local IPC publisher
          +-- gregg TUI
          +-- gregg TUI
~~~

This plan establishes the core process/data boundary and local protocol.
Automatic startup/install/update lifecycle belongs to Plan 165. Scheduler
history polling belongs to Plan 166.

## Same binary, separate process

Do not add a separately released fourth application executable.

Add a daemon command family under the existing binary, preferably:

~~~text
gregg daemon run
gregg daemon status
gregg daemon stop
~~~

Plan 165 may extend it with startup/restart commands.

run remains a foreground daemon entry suitable for manager supervision and test
harnesses. It does not fork/self-daemonize.

## Config-specific identity

One normalized Gregg config identity corresponds to one client-daemon instance.

Requirements:

- same existing config file reached through ordinary equivalent path spellings
  converges where safely possible;
- distinct config files do not share state or stop each other;
- default config has deterministic identity even before the file exists;
- identity logic should reuse/refactor the safe concepts from greggd's
  config-specific control identity rather than duplicate ad hoc hashing;
- no PID-file/process-name discovery.

Multiple TUIs using the same identity must attach to the same daemon.

## IPC transport

Use local OS-native IPC, never a LAN TCP listener.

### Unix

Use a Unix-domain socket:

- config-specific path with bounded length/fallback;
- 0600 permissions;
- owned cleanup on orderly/failure exit;
- conservative stale-socket cleanup;
- no unlink on ambiguous permission/timeout errors;
- same-user filesystem ownership is part of the security boundary.

### Windows

Use a named pipe:

- config-specific deterministic name;
- current-user access only;
- no Everyone/anonymous write access;
- fail closed if a same-user security descriptor cannot be established;
- no localhost TCP fallback merely to avoid native ACL work.

If the required Windows security API widens windows-sys features, measure and
document the feature change; do not add a broad framework dependency.

## Local protocol

The local protocol is internal to gregg, not gregg-protocol.

Use a small framed or newline-delimited serde representation with explicit
message kind and protocol version. Keep the parser bounded.

Handshake must exchange enough identity to reject:

- wrong local protocol major;
- wrong config identity;
- an unrelated service accidentally occupying the Unix path/pipe name.

The protocol is not an authentication token. Same-user IPC permissions are the
authority.

Minimum client-to-daemon requests:

- hello/subscribe;
- reload config;
- current state snapshot request if subscribe does not send it immediately;
- stop for the lifecycle command.

Minimum daemon-to-client messages:

- hello/ready response;
- complete latest frontend snapshot;
- reload accepted/rejected diagnostic;
- terminal daemon error before disconnect where feasible.

Do not design generic RPC/plugin infrastructure.

## Data ownership refactor

Move production ownership of:

- endpoint HTTP client;
- PollScheduler and generation loop;
- normalized Systems state;
- reachability/offline provenance;
- EggPool worker and dual summary/status plane;
- config reload/reconciliation;

into a daemon-side state engine.

Separate presentation-only state from fleet data.

The TUI should continue to own:

- selected system ID;
- viewport;
- active pane;
- normal/condensed mode;
- d and n expansion;
- future c expansion;
- transient highlight;
- terminal dimensions.

Do not serialize Ratatui-specific layout/cache types through IPC.

## Frontend snapshot model

Define one internal frontend DTO representing exactly the data a UI needs from
the daemon.

It should contain stable system IDs and normalized snapshots/reachability, plus
EggPool state when configured. Plan 166 extends it with cron state.

Avoid re-serializing the remote wire representation to the TUI. The client
daemon should normalize once, then publish its normalized local model.

A new subscriber receives the complete current snapshot immediately after a
successful handshake.

## Fan-out and backpressure

Multiple frontends must not multiply remote polling.

Do not enqueue every polling generation independently per slow TUI. Use
latest-state semantics:

- daemon reducer accepts coherent polling generations as today;
- frontend publication has a monotonically increasing local revision;
- each subscriber can skip superseded revisions;
- a slow/unread frontend cannot block polling or another TUI;
- disconnect cleans up subscriber state promptly;
- no unbounded per-client queue.

A watch-style latest-value channel or equivalent bounded replacement slot is
preferred internally.

## Polling invariants to preserve

The client daemon must retain all established Gregg behavior:

- one ordered result per endpoint per accepted generation;
- fixed bounded endpoint concurrency;
- fixed cadence;
- offline endpoints retried every cadence;
- v2-first, v1 only on v2 404;
- stale results cannot overwrite newer endpoint configuration;
- endpoint host/port validation gates mutations;
- no hidden endpoint pruning/backoff;
- existing request/body timeouts;
- one reusable lean EggFetch client where currently intended.

The daemon is not permission to redesign PollScheduler.

## EggPool ownership

Move the existing EggPool worker behind the client daemon with its current
convergent desired-state semantics.

Because current worker activation is pane-driven, define the background policy
explicitly:

- basic EggPool polling needed for configured summary/status remains owned by
  the daemon;
- frontend pane activation may still select period/cadence if that is part of
  existing behavior;
- multiple frontends must converge deterministically instead of racing worker
  activation state.

If current active-pane policy cannot be shared without ambiguity, choose a
small daemon-owned default policy and record the behavior. Do not create one
EggPool worker per frontend.

## Config reload/control

No filesystem watcher.

### Ctrl-R

The TUI sends a reload request to the client daemon. The daemon:

1. loads/validates ConfigStore;
2. preserves last-known-good state on failure;
3. reconciles stable system IDs/endpoints;
4. updates polling ownership;
5. sends a visible success/failure state transition;
6. polls immediately when accepted config changes endpoints.

### CLI mutation notification

After successful gregg add/remove/refresh config mutation, best-effort notify an
already-running matching daemon to reload. If no daemon is running, the command
remains successful.

After gregg edit returns successfully, perform the same bounded notification.
Invalid edited config follows current command semantics and must not replace the
daemon's last-known-good runtime state.

No config watcher thread/task is added.

## Direct TUI fallback

Do not keep a silent production fallback where a TUI that cannot reach clientd
simply resumes direct remote polling. That would recreate duplicate ownership
and hide daemon failures.

Tests may instantiate reducer/renderer state directly with synthetic data. A
developer-only explicit bypass, if absolutely necessary, must be named and not
the ordinary path.

## Daemon failure semantics

The daemon should remain useful with no TUI attached.

- frontend disconnect is not shutdown;
- polling continues;
- EggPool worker continues under its daemon policy;
- transient remote failures remain ordinary state;
- fatal internal scheduler/poller task failure terminates clientd so a later
  attach can restart a clean process under Plan 165;
- local IPC parse failure disconnects only that client unless it reveals daemon
  corruption.

## Tests

Deterministic tests must prove:

- same config identity -> same IPC endpoint;
- distinct configs -> distinct endpoint;
- Unix socket mode/cleanup/stale rules;
- Windows pipe name and access policy via native tests where required;
- handshake rejects protocol/config mismatch;
- first subscriber immediately gets full state;
- two subscribers receive one polling generation;
- slow subscriber skips revisions without blocking polling;
- subscriber disconnect does not stop polling;
- daemon continues after final TUI disconnect;
- reload success/failure behavior;
- CLI mutation notification is absence-tolerant;
- offline endpoint retry/recovery invariants survive refactor;
- EggPool still has one worker and convergent desired state.

Use existing native CI jobs; do not add another matrix by default.

## Documentation

On implementation update:

- architecture/gregg-client.md with the new process/data flow;
- architecture/overview.md;
- architecture/workspace.md only if module boundaries change;
- .opencode/skills/gregg-client/SKILL.md;
- AGENTS.md;
- crates/gregg/README.md for daemon diagnostic commands;
- CHANGELOG.md.

Do not yet claim automatic startup or cron TUI support; those belong to Plans
165-166.

## Acceptance criteria

- [ ] One config-specific client-daemon process owns production Systems polling.
- [ ] EggPool polling is daemon-owned and not duplicated per TUI.
- [ ] Same-binary foreground daemon mode exists.
- [ ] Local IPC is Unix socket / Windows named pipe, not TCP.
- [ ] IPC access is same-user/restrictive on supported platforms.
- [ ] Handshake is versioned and config-specific.
- [ ] Complete current state is sent immediately to a new frontend.
- [ ] Fan-out uses bounded latest-state semantics.
- [ ] Slow/disconnected TUI cannot stall daemon polling.
- [ ] TUI presentation state is no longer owner of remote polling state.
- [ ] Ctrl-R and CLI mutation reconciliation cross the daemon boundary.
- [ ] No filesystem watcher is added.
- [ ] No silent direct-polling production fallback remains.
- [ ] Existing Systems and EggPool regressions remain green.
- [ ] Default local checks and required existing native CI jobs pass.
- [ ] Active architecture/skill docs match the new ownership.

## Stop conditions

Open a corrective plan or revise 161 if:

- secure same-user Windows named-pipe setup requires a disproportionate
  dependency/runtime expansion;
- moving PollScheduler changes endpoint cadence or generation semantics;
- EggPool cannot be single-owner without a product-level behavior decision;
- local DTO fan-out grows into a generalized RPC framework;
- config identity cannot safely support multiple explicit configs.

## Handoff

Plan 165 adds lazy activation and durable startup lifecycle on this boundary.
Plan 166 extends the daemon state model with remote scheduler observability.
