# Plan 161: Cron observability and Gregg client-daemon roadmap

Status: complete. Closed at `154ab36`.

Depends on: completed scheduler line through Plan 160 and current main at
e399a4e05fa95b73823f7bef19de051685685bb4. Independent of the remaining
Plan 091 sustained-soak record.

## Objective

Add an operator-facing cron observability path to Gregg while correcting the
current client process architecture so one background Gregg client daemon owns
fleet polling and multiple TUI processes consume its cached state.

The operator goal is deliberately narrow:

- greggd exposes which local maintenance jobs are configured, when they are
  due, whether they are running or load-delayed, and a bounded in-memory record
  of recent terminal outcomes;
- each greggd keeps a configurable number of recent records per job in memory,
  defaulting to five, with no scheduler-history disk writes;
- recent records include bounded stdout and stderr tails sufficient to diagnose
  ordinary maintenance jobs without allowing an unbounded child pipe or log
  database;
- gregg gains plain c as the cron detail control beside d (drives) and n
  (network);
- a selected system's cron view makes configured schedules, next run, last run,
  recent outcomes/output, and current load deferral visible at a glance;
- the polling/data plane moves out of each TUI instance into one config-specific
  background Gregg client daemon so multiple TUIs do not duplicate endpoint
  polling;
- the client daemon can remain running after all TUIs exit and therefore retain
  a longer bounded in-memory cron backlog than any one remote greggd response;
- invoking bare gregg ensures the correct client daemon is running before
  attaching the TUI;
- user-scoped startup installation gives the daemon a persistent background
  lifecycle without turning Gregg into a system-wide privileged service.

This is an observability and client-process-boundary line. It is not a remote
scheduler-management line.

## Current baseline

The post-Plan-160 scheduler already provides:

- strict five-field local-civil cron semantics plus the four existing aliases;
- one pending occurrence per job;
- one global scheduled child;
- cached sampler load gating with 1m/5m/15m selection;
- bounded retry and maximum wait;
- coalescing instead of missed-run replay;
- same-principal direct argv execution;
- a two-second direct-child shutdown bound;
- minute-scale civil-clock reconciliation;
- no persistent queue or job database;
- no scheduler task at all when jobs is empty.

The missing observability is structural rather than an execution bug:

- scheduler state lives only in crates/greggd/src/scheduler.rs;
- stdout/stderr are currently Stdio::null();
- completed executions are logged and discarded;
- the daemon HTTP surface has no scheduler routes;
- gregg polls every configured endpoint itself;
- every additional TUI process therefore owns another polling scheduler and
  another EggPool worker;
- AppState currently mixes fleet data with TUI-only selection/view state.

Plan 159 also established a scheduler-specific stripped greggd baseline of
3,261,664 bytes. Scheduler-related growth beyond that value requires a new
measured review. This line owns that review rather than silently spending the
old budget.

## Architecture to establish

The target process graph is:

~~~text
remote hosts
  greggd ---+
  greggd ---+-- read-only HTTP --> gregg client daemon -- local IPC --> gregg TUI
  greggd ---+                         |                         +-----> gregg TUI
                                      +-- bounded in-memory fleet/cron cache
~~~

The client daemon remains a mode of the existing gregg executable for this
line. Do not introduce a separately distributed greggc/fourth application
binary merely to achieve process separation.

A future GUI should be able to consume the same local daemon boundary, but GUI
implementation is explicitly out of scope.

## Ownership boundaries

### greggd owns

- execution of locally configured jobs;
- authoritative current scheduler state;
- the remote per-job history ring;
- bounded output capture;
- scheduler summary/history serialization;
- all load-delay reasoning.

### Gregg client daemon owns

- endpoint polling and generation/concurrency control;
- v2-first metrics normalization;
- optional scheduler-summary/history polling;
- EggPool polling/control already owned by the current client;
- the longer bounded local cron cache;
- config reconciliation for the polling plane;
- fan-out of latest coherent state to local frontends.

### Gregg TUI owns

- terminal lifecycle;
- selected system and viewport;
- normal/condensed presentation;
- drive/network/cron expansion state;
- cron sub-selection/scrolling needed only for presentation;
- transient selection highlighting;
- rendering and keyboard input.

The TUI must not regain direct remote polling as the ordinary production path
after the client-daemon cutover. A focused test/helper path may still inject
state directly.

## Remote API boundary

Scheduler observability is additive and read-only. Do not merge history into
ordinary /v2/status: metrics are polled frequently and scheduler output may be
materially larger.

The intended shape is:

~~~text
GET/HEAD /v2/scheduler
GET/HEAD /v2/scheduler/history
~~~

Plan 162 owns the exact wire types, bounds, compatibility behavior, and
footprint qualification before Plan 163 lands product behavior.

No endpoint may:

- create/edit/delete jobs;
- start/cancel a job;
- mutate daemon configuration;
- accept command input;
- expose a shell;
- weaken the existing private-LAN/read-only server model.

## Scheduler history policy

The default remote history depth is five completed/terminal occurrences per
configured job. The depth is configurable but must have a small hard maximum
chosen and recorded by Plan 162.

History is memory-only:

- daemon restart clears it;
- no sqlite/database/journal/log-spool directory;
- no replay after restart;
- no periodic history write;
- no fsync/write amplification from scheduler execution.

Terminal records must include non-child outcomes relevant to operator
correctness, including load-wait expiry and spawn failure. A recent-runs display
that silently omits failed-to-start or expired occurrences is not truthful.

Output capture is bounded. Plan 162 must select exact stdout/stderr byte limits
and prove a worst-case memory/body-size budget before implementation. Plan 163
must drain child pipes concurrently while the child runs; wait_with_output or
another whole-output accumulator is not acceptable.

## Security boundary for output

Scheduler output is more sensitive than CPU/memory telemetry. The line must
make the trust model explicit.

At minimum:

- command argv/working-directory values are not newly exposed merely to render
  cron status; the operator-facing job name and schedule are sufficient;
- output is bounded before publication;
- arbitrary output bytes are converted to a bounded JSON-safe representation;
- the TUI sanitizes terminal control/ANSI sequences before rendering;
- documentation states that any principal able to reach the configured greggd
  HTTP listener can read the scheduler history/output endpoint;
- no credentials, auth scheme, TLS system, or secret store are introduced by
  this line.

If Plan 162 concludes that default output publication is unacceptable under the
existing unauthenticated LAN boundary, it may add one narrow explicit
configuration policy, but it must preserve the requested default operator UX
unless a concrete security reason is recorded.

## Local client-daemon boundary

The daemon is per-user and config-specific. Two gregg processes using the same
normalized config identity attach to the same daemon. Distinct explicit configs
may have distinct daemon instances.

Use native local IPC:

- Unix: config-specific Unix-domain socket with restrictive ownership/mode;
- Windows: config-specific named pipe with a same-user access policy.

Do not use a discoverable unauthenticated TCP listener for the local client
daemon.

The local protocol is internal to the gregg crate and versioned by a small
handshake. Do not put TUI/client-daemon implementation DTOs into gregg-protocol,
whose purpose is the independently deployed remote greggd wire contract.

The local stream should publish latest coherent state, not queue every metrics
sample. Slow frontends may skip superseded generations. Cron history remains
available from the daemon-owned cache and therefore is not lost merely because
a TUI skipped intermediate UI updates.

## Configuration and reload boundary

The existing config remains authoritative.

- No filesystem watcher is added.
- Ctrl-R becomes a request to the client daemon to reload/reconcile the selected
  config.
- Successful non-TUI config mutations (add, remove, refresh, and edit after the
  editor exits) should notify an already-running matching daemon to reconcile;
  absence of a daemon is not an error.
- Invalid reload keeps the last-known-good polling state and returns a visible
  diagnostic to attached frontends.
- Stable endpoint IDs/generation protections remain authoritative.

## Lifecycle direction

Bare gregg performs a bounded local-daemon handshake before entering the TUI.
If the matching daemon is absent, it launches the exact current executable in
client-daemon mode under a launch lock, waits boundedly for readiness, then
attaches.

Plan 165 owns durable per-user startup semantics. Reuse/refactor Gregg's
existing lifecycle primitives where they genuinely apply, but do not copy
system-level greggd privilege assumptions into a user-level client daemon.

Expected managers:

- Linux: user systemd when available, with a bounded fallback appropriate to a
  user process;
- macOS: LaunchAgent, not system LaunchDaemon;
- Windows: user-session startup mechanism, not a LocalService SCM service.

System-wide binary installation must not guess which human account should own a
client daemon.

## Planned sequence

### Plan 162 - scheduler observability contract and resource qualification

Freeze scheduler status/history wire types, state/outcome vocabulary,
history-depth bounds, output-tail limits, body caps, in-memory cost, and
stripped-binary budget before product implementation.

### Plan 163 - greggd scheduler history and read-only API

Implement live scheduler publication, bounded history/output capture, the new
read-only routes, config validation, protocol fixtures, and deterministic
scheduler/server tests.

### Plan 164 - Gregg client-daemon core and local IPC

Move remote Systems/EggPool polling ownership behind one config-specific local
daemon, establish same-user IPC/handshake, latest-state fan-out, and
config-reload/control semantics. Keep the TUI presentation-only.

This plan may be developed in parallel with Plans 162-163 after the Plan-161
boundary is accepted because it does not require scheduler wire details.

### Plan 165 - client-daemon lifecycle, install, update, and uninstall

Make bare gregg lazily ensure/attach the daemon, add per-user startup
registration and ownership-safe stop/status/restart behavior, and reconcile
bootstrap/update/uninstall semantics.

### Plan 166 - cron cache and TUI observability

Add scheduler polling/history merge to the client daemon, a longer bounded
local in-memory cache, plain c cron detail, load-delay presentation, recent
outcomes/output, mixed-version compatibility, and control-sequence
sanitization.

### Plan 167 - correctness, multi-client, and footprint closure

Prove one polling plane with multiple TUIs, daemon/TUI/restart behavior,
bounded memory/output, mixed old/new daemons, load delay and output-flood
behavior, native IPC/lifecycle truth, and measured binary/runtime impact.
Reconcile active docs and this roadmap.

## Dependency graph

~~~text
160 -> 161
161 -> 162 -> 163
161 -> 164 -> 165
163 + 164 -> 166
163 + 165 + 166 -> 167
~~~

Plan 091 remains independent.

## Preserved exclusions

This line does not add:

- remote scheduler mutation;
- arbitrary command execution from Gregg;
- persistent scheduler history;
- persistent client cron history;
- job dependency graphs;
- catch-up replay;
- alerts/notifications;
- Prometheus/exporter support;
- web UI;
- GUI implementation;
- public-internet hardening/TLS/auth;
- process monitoring;
- a generalized task runner;
- a separately distributed client-daemon executable unless Plan 164 proves the
  same-binary mode impossible and records why.

## Completion criteria

This roadmap closes only when Plans 162-167 demonstrate all of the following:

- [x] greggd exposes bounded read-only scheduler summary/history without
      changing /v2/status metrics semantics.
- [x] Default remote history depth is five and is configurable within a hard
      bound.
- [x] No scheduler history is persisted to disk.
- [x] Child stdout/stderr are drained without deadlock and retained only under
      fixed byte bounds.
- [x] Current load-delayed/unavailable/slot-wait/running state is truthful.
- [x] Terminal history includes success, command failure, spawn failure, and
      load-expiry outcomes as applicable.
- [x] Old greggd versions without scheduler routes remain ordinary online
      systems with cron observability marked unsupported.
- [x] One config-specific Gregg client daemon owns polling for multiple TUIs.
- [x] Closing all TUIs does not stop the background daemon.
- [x] Bare gregg starts an absent matching daemon safely and attaches.
- [x] Per-user startup ownership is available on supported platforms without
      creating a privileged system client service.
- [x] Client update/uninstall do not leave a knowingly incompatible owned
      daemon attached to the new binary.
- [x] The client daemon maintains only bounded in-memory cron history.
- [x] Plain c provides the requested scheduler view beside d and n.
- [x] Output rendered in the terminal cannot inject terminal control sequences.
- [x] Multiple TUIs do not multiply remote Systems or EggPool polling cadence.
- [x] greggd growth beyond the Plan-159 scheduler baseline and gregg client
      growth are measured and explicitly accepted or corrected.
- [x] No new workflow/job/matrix is required unless native-platform truth
      cannot be obtained from the existing CI jobs.

## Handoff

Begin with Plan 162 and Plan 164. Plan 162 must freeze the remote scheduler
observability/resource contract before Plan 163 changes greggd. Plan 164 may
proceed independently because the client-daemon split is already justified by
duplicate polling and future frontend reuse. Do not start with TUI rendering:
the c view belongs after both data planes exist.

## Closure record

The 161 line is complete. Every child plan is closed:

| Plan | Scope | Closed at |
|---|---|---|
| 162 | Scheduler observability contract and resource qualification | `42e2fc2` |
| 163 | greggd scheduler history and the read-only API | `4fd0158` |
| 164 | Client daemon core and local IPC | `b25ca04` |
| 165 | Lazy activation, user-scoped startup, update/uninstall | `c8f2542`, `ee6ad7e` |
| 166 | Cron cache, the `c` view, terminal sanitization | `53c06e5` |
| 167 | Correctness and footprint closure | `154ab36` |

All eighteen completion criteria above are met. The evidence for each lives in
its own plan's closure record; the two that this roadmap most depends on are
restated here because they are the ones that would have invalidated the whole
design.

**One polling plane.** A per-config client daemon owns every remote request.
Counted rather than asserted: with N frontends attached the fleet receives one
generation of Systems requests per configured cadence and one scheduler summary
read per 30 s, and ten windows with the cron pane open cost the remote exactly
what zero windows cost. The cron intent governs transmission and never
fetching. Closing the last window changes nothing; the daemon keeps polling.

**No hidden persistence.** Verified by inspection, not by convention: there is
no `File::create`, `fs::write`, or `OpenOptions` anywhere under
`crates/greggd/src/scheduler/`, `crates/gregg/src/cron.rs`, or
`crates/gregg/src/clientd/cron.rs`. History is memory-only on both sides. A
`greggd` restart starts a new epoch and a client-daemon restart reseeds from the
remote without duplication.

**No remote control plane.** The two scheduler routes are `GET`/`HEAD` only;
POST/PUT/DELETE/PATCH are 405 and `/v2/scheduler/{run,jobs,cancel}` are 404.
`argv` and `working_dir` are not on the wire. No job can be created, edited,
started, or cancelled by anything that can reach the listener.

**Cost, recorded.** `greggd` is 3,316,568 bytes stripped, inside Plan 162's
3,400,000 rule. `gregg` grew +989,752 bytes (+22.76%) across 164-166 with a
**completely empty** `Cargo.toml`/`Cargo.lock` diff, so the entire increase is
application code. Idle cost with three systems is 0.533% of one core with no
window attached and 0.622% with two.

**Defects found while closing the line**, all fixed and covered: `Ctrl-R` was
dead without an `EggPool` entry; `watch::Sender::send` discarded documents
published while no window was attached, so a late-attaching TUI saw stale state
forever; the global cron record counter was never incremented, so the ceiling
bounded nothing; `compose` budgeted rows by `chars()` instead of display cells;
and a state document could be published ahead of the handshake `Hello`, which
made a healthy daemon look incompatible and made a real TUI exit.

**Carried forward, honestly.** The macOS, Windows, and MSRV CI jobs were not run
by this closure — there is no remote-runner access here, so no CI run ID exists.
They must be confirmed on the final implementation SHA before release. One
unrelated pre-existing flake in `gregg-update` recurred under full-workspace
load; it was root-caused to a transient `ETXTBSY` on the test's own
write-then-`exec` stub, the product code handled it correctly, and the
diagnosis plus the minimal test-only repair are in Plan 167's closure.

Independent of Plan 091 throughout: no part of this line depended on the
remaining sustained-soak record, and no part of it satisfies or substitutes for
that record.

## Post-closure correction (added after `4fc70a5`)

The "Carried forward, honestly" paragraph above recorded that the Windows job had
not been run and therefore had to be confirmed before release. Plan 168 ran it,
and the result was worse than an unconfirmed job: **the Windows client daemon
had never been compiled.** The Windows half of the Plan-164 transport was
written but never type-checked, so the Windows job had been red continuously
since this roadmap's fourth child plan landed.

The gap was stated accurately but read too gently. The accurate reading is that
this roadmap closed a cross-platform feature with one of its three platforms
unbuilt, and that the local verification loop — which is Linux-only by design —
could not have caught it. The absence of a CI run ID was a tooling limit; the
absence of a working Windows build was a fact about the product.

Two roadmap-level invariants survive unchanged and are unaffected: one polling
plane, and memory-only history with no remote control plane. The Windows fix
changed no scheduling, publication, caching, or history behaviour, and
`greggd` was not touched at all. What changed is that the transport those
invariants ride on now exists on all three platforms, and that the endpoint name
on Windows is a `\\.\pipe\` name rather than a Unix socket path.

Plan 168 also found a second defect with no platform connection:
`FrontendFrame::ProtocolError(String)` cannot be serialized under an internally
tagged enum, so every refusal reason this architecture tried to send was
silently discarded. That is a consequence of the handshake/refusal contract
this roadmap established, so it is recorded here rather than only in 168.

The `gregg-update` `ETXTBSY` flake noted above is still unfixed and still out of
scope; Plan 168 does not touch `gregg-update`.
