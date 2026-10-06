# greggd daemon deep dive

The daemon crate is the metrics collection agent that runs on each monitored
host. It collects system metrics, samples them on a timer, serves them over
HTTP. Foreground `run` works on every OS; Windows additionally supports the
SCM `service` / `start` path.

**Source:** `crates/greggd/`

## Purpose

- Collect CPU, memory, swap, load, and drive metrics using native OS interfaces
- Sample metrics at a configurable interval with delta-based CPU computation
- Serve cached snapshots over HTTP (v1 and v2 endpoints)
- Expose CLI for configuration mutation, health probing, bind-address inspection, and runtime control

## Module map

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Binary boundary: CLI parsing, logging, error reporting, exit-code classification, and platform collector dispatch |
| `lib` | `src/lib.rs` | Library root, re-exports all modules |
| `cli` | `src/cli.rs` | Clap CLI: `run`, `stop`, `croncheck` (bounded `/v2/healthz` watchdog; spawns `run` only on refusal), `configprint`, `status` (read-only diagnostic composition), `host`, `port`, `version`, `update` (daemon lifecycle coordination over `gregg-update`), `uninstall [--dry-run] [--purge]` (exact-exe removal + owned startup teardown, dry-run/purge), `startup install`/`instructions` (`--method auto|systemd|launchd|cron`), `restart` (universal manager-aware; SCM on Windows / direct on Unix); Windows adds `start` and hidden `service` SCM entry; global `--config/-c`; authoritative bounded health fetch (`fetch_health_bytes`) with detail (`probe_health`) and watchdog (`probe_greggd`) classifications |
| `run` | `src/run.rs` | Foreground daemon: wiring + supervision loop; entry points `run()`, `run_with_shutdown()`, Unix `run_with_control_path()`, cross-platform `run_with_control_path_or_default()`, all funneling into the shared `run_with_shutdown_on_ready()` core (foreground passes a no-op `on_ready`; Windows SCM passes the `RUNNING` publisher invoked only after listener bind); `RunOutcome`, 10s graceful shutdown deadline, Unix control-socket + Windows SCM entry alongside SIGTERM/SIGINT |
| `config` | `src/config.rs` | TOML config, validation, atomic writes; `ConfigViolation`, `AtomicWriteError` |
| `control` | `src/control.rs` (Unix-only) | Unix-domain control socket for `greggd stop`; normalized config identity (FNV-1a digest) computed once per operation, config-adjacent primary + temp-dir fallback paths; `ControlSocketGuard` is owned by the caller (`run_with_control_path`) so socket-file removal happens on every exit path, including signal-driven shutdown where the stop task is still parked in `accept()` |
| `net` | `src/net.rs` | Local-network address resolution for `configprint`: resolves a wildcard bind host to the primary local IP via a transient UDP `connect()` (no packets sent) |
| `sampler` | `src/sampler.rs` | Periodic sampling loop, readiness lifecycle; `SamplerError`, `Clock`/`RealClock` (`SyntheticClock` is test-only) |
| `scheduler` | `src/scheduler.rs`, `src/scheduler/schedule.rs` | Optional local maintenance engine: strict five-field cron wrapper, bounded pending state, cached sampler load feed, one Tokio child slot, direct-child shutdown |
| `server/mod` | `src/server/mod.rs` | EggServe direct H1 service, endpoints, staleness; `ServerState`, public `Config`, private `PublishedState` (`ServerConfigError` in `server/error`); `server/tests.rs` holds the handler tests |
| `server/error` | `src/server/error.rs` | Server error types |
| `collector/mod` | `src/collector/mod.rs` | Gregg-owned `SystemCollector` trait, `CollectedMetrics`, authoritative `into_snapshot_pair()` (plus `into_snapshot` / `into_snapshot_v2` / `into_status_payload_v2`); `CollectError`/`CollectErrorKind`, `clamped_usage_pct`/`finalize_percentage`, `CounterBaselines`, `DriveCandidate`/`normalize`, and `DriveRefreshCache` re-exported from `gregg-host` at the same paths; platform `linux`/`macos`/`windows` facades delegate production sampling to `gregg-host` (Plans 133-135) |
| `collector` native acquisition | `crates/gregg-host/src/` | Reusable Linux/macOS/Windows/FreeBSD collection, sampling state, rate baselines, drive normalization, slow-probe isolation (protocol-neutral; see `architecture/collectors.md`) |
| `startup/method` … `startup/install` | `src/startup/*.rs` | Startup installation, teardown, and restart split by ownership (Plan 105, behavior-preserving): method identity/paths/detection (`method`), bounded child execution (`process`), systemd unit/install/restart/uninstall plus narrow `ExecStart` ownership parsing (`systemd`), launchd plist/install/restart/uninstall plus `ProgramArguments` ownership parsing (`launchd`), shell quoting + cron block/install/uninstall plus command-target ownership parsing (`cron`), `StartupState` detection (`state`; `ArtifactOwnership` lives in the `src/startup.rs` façade), errors/atomic writes/privilege/install dispatch/instructions/restart coordination (`install`). `src/startup.rs` is a façade re-exporting the historical `crate::startup::X` paths |
| `status` | `src/status.rs` | Read-only `status` model: `StatusReport`, `health_token`, `status_outcome`, `Display`, injected `gather_status`, stable `render_status`, `status_is_present` (valid endpoint = ready/warming/failed, same running definition as `croncheck`) |
| `update` | `src/update.rs` | Exact-executable-aware lifecycle coordinator over the shared `gregg-update` mechanism: binds daemon identity, prepares the candidate via `prepare_candidate`, observes `UpdateLifecycle` after preparation (Unix systemd/launchd ownership + selected-config health; Windows `query_registration()` revalidated immediately before quiescence **and re-queried again inside `quiesce_windows_service_if_needed`**, with owned-to-foreign failing before replacement), quiesces only an owned Windows SCM running service (owned stop-pending waits stopped without restart; foreign/unknown/not-installed perform zero SCM mutation), replaces, then restarts only `ManagedRunning`/`DirectRunning` through `restart_daemon()` with `UpdatedButRestartFailed` partial-success; preserves the Plan 102 prepare-before-quiesce transaction rule |
| `uninstall` | `src/uninstall.rs` | Component-safe daemon uninstall: independent read-only discovery per artifact, pure `plan_from_discovery` shared by `--dry-run` and execution, preflight before teardown, manager teardown via the startup owners + SCM `unregister`, direct control-stop with uncertain-stop blocking deletion, config preserved by default with `--purge` removing only resolved files |
| `service/mod` | `src/service/mod.rs` (`cfg(any(target_os = "windows", test))`) | `ServiceManager` trait and bounded `ServiceRegistration` observations |
| `service/windows` | `src/service/windows.rs` (Windows; fake adapter under `cfg(test)`) | Windows: SCM integration, native state plus bounded parsing of the registered `lpBinaryPathName` command into an exact image-path ownership target |

## Architecture

### Supervision loop

The `run()` function in `run.rs` wires everything together:

```
┌─────────────────────────────────────────────────┐
│  run()                                          │
│                                                  │
│  ┌──────────┐  ┌─────────┐  ┌────────────────┐ │
│  │ Collector │  │ Sampler │  │  HTTP Server   │ │
│  │ (native)  │  │ (timer) │  │ (EggServe H1)  │ │
│  └─────┬────┘  └────┬────┘  └───────┬────────┘ │
│        │             │               │           │
│        └──────┬──────┘               │           │
│               ▼                      │           │
│        ┌──────────────┐              │           │
│        │ Cached v1+v2 │◀─────────────┘           │
│        │  snapshots   │                          │
│        └──────────────┘                          │
│                                                  │
│  tokio::select! on:                              │
│  - shutdown signal (SIGTERM/SIGINT)              │
│  - server task                                   │
│  - sampler task                                  │
└─────────────────────────────────────────────────┘
```

Graceful shutdown with a 10-second deadline. Tasks that don't finish are
aborted.

The bound listener is handed to EggServe before the readiness callback. Gregg
keeps EggServe's `ServerControl` and `ServerCompletion` separate: shutdown is
requested through the control handle while completion remains a critical task
whose clean exit, error, or panic is supervised. Its eight-second drain window
fits inside Gregg's outer ten-second cleanup deadline. The H1 runtime explicitly
keeps connection and request admission at semaphore maximums, sets 100 header
fields and a 417,792-byte parser/header ceiling, and bounds total connection
lifetime so a keep-alive connection cannot remain indefinitely.
EggServe 0.4 uses the direct H1 service boundary with `eggserve-primitives`
0.2.2. Gregg's explicit limits include a 300-second connection lifetime and a
1000-request per-connection cap. EggServe requires finite header, handler,
body, idle-keepalive, and response-write deadlines; the selected values are
10, 30, 30, 60, and 30 seconds. GET bodies are ignored up to 64 KiB. These
limits bound stalled or oversized transport work and do not change the status
protocol.

### Runtime ownership and Windows SCM shutdown

The `greggd` binary dispatches synchronously before creating Tokio. Foreground
`run` creates exactly one current-thread runtime at the binary boundary;
Windows `service` first calls `service_dispatcher::start`, which connects the
process to the SCM and invokes the generated `ServiceMain` callback. The
callback reads the resolved config path from one process-local launch context;
the service worker then creates exactly one current-thread runtime. No service
path enters or blocks a second runtime.

The worker reports `START_PENDING`, loads the selected config, constructs the
collector and runtime, and runs the shared daemon core. That core binds the
listener before invoking its readiness callback, so Windows reports `RUNNING`
only after binding succeeds. Any post-registration startup or runtime failure
makes a best-effort `STOPPED` report with a nonzero exit code. SCM Stop and
Shutdown callbacks only consume a shared one-shot sender; the async receiver
supplies a stable reason to `run_with_shutdown()`. Interrogate succeeds without
stopping the daemon, duplicate stop controls are harmless, and dispatcher
errors return to the executable while callback/worker errors are logged once
by `ServiceMain`.

If the platform shutdown source itself fails while being awaited, the shared
runtime returns that error through the ordinary runtime failure boundary; it
does not panic inside reusable daemon code.

### HTTP endpoints

| Route | Handler | Response |
|-------|---------|----------|
| `GET`/`HEAD /` | `status_handler` | v1 snapshot (200) or health (503) |
| `GET`/`HEAD /v1/status` | `status_handler` | Same as `/` |
| `GET`/`HEAD /v2/status` | `status_handler_v2` | v2 payload (200) or v2 health (503) |
| `GET`/`HEAD /healthz` | `health_handler` | v1 health (200 if ready, 503 otherwise) |
| `GET`/`HEAD /v2/healthz` | `health_handler_v2` | v2 health (200 if ready, 503 otherwise) |
| `GET`/`HEAD /v2/scheduler` | `scheduler_summary_response` | `SchedulerSummaryV2` (200, always) |
| `GET`/`HEAD /v2/scheduler/history` | `scheduler_history_response` | `SchedulerHistoryV2` (200, always) |
| Known route, other method | — | 405 + `Allow: GET,HEAD` |
| Other | `fallback_handler` | 404 `text/plain; charset=utf-8` |

**Published state:** Typed v1/v2 snapshots, compact successful status bytes,
minimal health metadata, observation time, and failure count are published
under one state lock. Sampler-owned `Arc` snapshots cross the publication
boundary without deep cloning. Status JSON is serialized once per successful
publication and repeated fresh requests clone `Bytes`; if preparation fails,
the typed snapshot remains authoritative and the request path serializes it on
demand. Ready-health bodies use a separate per-publication `OnceCell` memo
(`health_cell`/`health_cell_v2`), populated via borrowed serialization and
revalidated after the await so a concurrent failure transition cannot make a
waiter answer `200` for a superseded publication. Each handler takes one coherent generation, so its HTTP status and
JSON body cannot describe different publications. Windows publishes v2
metrics and returns a v1 `not_serving` health response with `503` because v1
is structurally unavailable.

Health endpoints and public `ServerState::health()`/`health_v2()` getters
reconstruct the existing typed envelopes on demand. Status handlers do not
construct or clone a ready health envelope on the fresh path. Staleness and
failure thresholds are evaluated for every request before cached bytes are
served; stale data returns the existing collector-failure health response even
when old successful bytes remain stored.

**Staleness policy:** The daemon wires `max_consecutive_failures = 3`
(`run.rs::DEFAULT_MAX_CONSECUTIVE_FAILURES`) alongside `stale_after_ms`, so a
short burst of collector failures marks the preserved snapshot stale (503)
instead of serving `200` while health reports failure
(`ServerState::new()`/`Config::default()` use `0`; the count policy is
production-live, not test-only). If `max_snapshot_age > 0` and the
latest published observation is too old, the server
returns 503, including for v2-only Windows publication. The snapshot is preserved (not cleared) for stale serving. A 503 body is always a failed health response: if staleness trips while the stored health state still says `ready`, the handlers substitute a `CollectorFailure` failure ("cached snapshot is stale"), so the body can never contradict the status code.

When the wall clock moves backward and a cached observation timestamp is in the
future, the age check treats that snapshot as stale rather than serving an
unverifiable age. If the clock is before the Unix epoch, the sampler pauses
publication rather than emitting timestamp `0`, and an enabled age-based
server policy treats any cached snapshot as stale until the clock is corrected.

### Sampler

The sampler owns the clock and cadence. Key behaviors:

- First `sample()` returns `Warming` — CPU percentages require two readings
- Subsequent samples return `Ok(CollectedMetrics)` with delta-based percentages
- Produces both v1 `StatusSnapshot` and v2 `StatusPayloadV2` from one ownership-aware collection conversion; v2-only drives, disk-I/O, and network collections move into the v2 payload
- Manages readiness lifecycle: `Warming` → `Ready` (on first delta) or `Failed`
  (on collector or identity error); identity failures preserve any previously
  published snapshot and never publish a blank identity
- `Clock` trait for deterministic testing with `SyntheticClock`
- The runtime loop runs each core collection cycle on Tokio's blocking thread pool
  (`spawn_blocking`). Optional drive capacity runs in one collector-owned
  standard thread with a bounded result channel and a 30-second cadence, so a
  slow native filesystem call cannot stall fresh CPU/memory/load snapshots or
  Tokio runtime shutdown.
  Live CPU frequency, disk counters, and network counters are observed at
  ordinary sample cadence. Shared identity-keyed baselines use actual monotonic
  elapsed time and discard the interval on reset, backwards/zero elapsed time,
  disappearance, or reappearance. Optional source failures yield absent live
  fields while core CPU/memory sampling remains eligible for Ready.
  CPU frequency is the current OS-reported value, not a base or maximum-clock
  claim; macOS leaves it absent because no privileged or undocumented source is
  used. Disk `R/s`/`W/s` and network `Rx/s`/`Tx/s` are byte rates. Filesystem
  capacity and disk-I/O accounting are separate, while network utilization is
  computed directionally and takes the maximum valid Rx/Tx percentage. Loopback
  may be published for interface detail but cannot contribute aggregate link
  capacity. Older clients and daemons remain compatible because these fields
  are additive; daemon-version transport remains deferred.
  The collector is shared with the blocking task behind a mutex; a panicked
  task poisons it, the panic is logged and reported as a source failure for
  that cycle only, and later ticks recover the lock and resume sampling.

Shutdown is bounded twice, because `spawn_blocking` cannot abort started work.
The loop observes shutdown *while* a cycle is in flight
(`COLLECTION_TIMEOUT`, 5s), so a hung native read cannot stall the loop. A cycle
that outlives the loop is then **joined** under `SHUTDOWN_JOIN_TIMEOUT` (1s) and
only detached past that bound, with an explicit warning. Dropping a `JoinHandle`
detaches rather than cancels, so without that join `Runtime::drop` blocked on the
blocking-pool drain with no bound and no diagnostic — long after the daemon had
logged a clean sampler shutdown.

The loop's collection branch is polled **first** (`biased`). A cycle that
completed in the same poll a stop signal became ready in must still be applied
and published; letting the shutdown branch win discarded a real sample, and a
discarded *failure* was never counted, so the daemon exited reporting readiness
the collector had contradicted. Reaching the shutdown arm therefore means the
cycle did not finish: readiness and the snapshot are left untouched and the last
snapshot keeps serving per the stale policy. This is the same "resolved before
the signal, never after it" ordering the scheduler and the EggPool worker already
rely on.

### Scheduler observability (Plan 162/163)

The scheduler publishes a **separate** read-only snapshot. It is deliberately
not part of the metrics `PublishedState`: metrics are republished every sample
while scheduler state changes only at transitions, so one lock would couple a
low-frequency document to a high-frequency write path for no benefit.

Ownership and shape:

~~~text
execution Engine
  +-- authoritative per-job scheduling state (incl. the load decision read)
  +-- active child
  +-- bounded per-job terminal history
        |
        +-- publish one coherent SchedulerPublication
              (serialized summary Bytes + serialized history Bytes)
~~~

The handle is created once in `run.rs` and shared by the scheduler and the
server state, so the live job set replaces the initial empty document without
re-wiring anything. Handlers clone one `Arc` and stream already-serialized
known-length bytes: they never serialize, never await scheduler mutation, never
hold a scheduler lock, and never touch config files, telemetry, or processes. A
serialization failure keeps the previous publication rather than degrading into
a fabricated empty scheduler, and a scheduler task failure stays a supervised
daemon failure (a panic at the existing fatal boundary), not an empty document.

**Publication happens only on externally visible state change.** The observer
compares the derived `(epoch, history_revision, jobs)` against the last
publication and skips the swap when they match, so the Plan-160 one-minute
civil-clock reconciliation wake publishes nothing when nothing changed. The
`generated_at_unix_ms` stamp is deliberately excluded from that comparison.
The published job state is the truth about the slot, not a momentary one: the
job actually holding the global child slot carries `running_since_unix_ms` for
its whole run, and a load-deferred job carries its real future retry rather
than the second the tick began in.

**Cache eviction is oldest-first.** When a global record ceiling forces a
drop, the victim is the record with the smallest `finished_unix_ms` across
every system and job, with `(system, job)` as the stable tie breaker — the
same order the bounded per-job `VecDeque` uses.

**Bounded terminal history** is one `VecDeque` per configured job at the
configured depth (`scheduler_history_limit`, default 5, hard maximum 10, `0`
meaning no retention). Oldest is evicted first, `sequence` is monotonic within
the scheduler lifetime, `history_revision` advances on every retained-history
mutation, and there is no filesystem I/O anywhere in the path — a restart
clears history and starts a new `SchedulerEpochV2` with no replay.

**Non-child outcomes are first-class records.** A `spawn_failed` occurrence
(missing executable, permission) and a `load_expired` occurrence (`max_wait`
elapsed with no child) both become terminal records. Omitting them would leave
the recent-runs display quietly dishonest: an operator would see an empty list
for a job that never ran. `load_expired` is only reachable while the load gate
is actually refusing the occurrence: expiry is evaluated before the global
child-slot check, so a job merely waiting for the slot must not be dropped with
a record blaming load for a slot the gate never saw. A slot-delayed occurrence
stays pending and runs when the slot frees.

**Output capture** replaces `Stdio::null()` only when history is enabled. Both
pipe handles are taken immediately after the spawn, and both streams drain
concurrently with the child wait on one task — neither stream can block the
other, and neither can fill the child's pipe, so the classic deadlock cannot
occur. The *order* is load-bearing, not incidental: awaiting the child first
would let a job that writes more than one pipe buffer block in `write(2)` while
nothing drains it, so the wait would never resolve, the one global child slot
would never free, and no further job could start. Draining one stream to EOF
before touching the second is the same deadlock wearing a different hat: a child
blocked writing to the second pipe can never reach the EOF that ends the first.
The drain
futures **borrow** the streams instead of
spawning tasks: a cancelled select (deadline or shutdown) simply stops draining
and leaves the handles in place for the next wake, so no drain task can outlive
the scheduler or delay shutdown, and the existing two-second direct-child
shutdown bound is unchanged. That cancellation cancels the *read*, not the
capture: each stream's bounded tail is owned by the `RunningChild`, not by the
drain future, so the bytes already read survive a wake. A wake is capped at the
60-second civil-clock reconciliation bound, so any job that outlives one wake
would otherwise publish only the bytes read since that wake — or none at all —
while still reporting `truncated: false`. Tails are fixed-capacity (1024 raw
bytes per
stream) and fold each read chunk in with a single bounded drain, so retention is
independent of how much a child writes. A read error ends that one stream and
keeps what was already captured rather than failing the scheduler.

**The direct child is the execution boundary, not pipe EOF.** The direct
child's wait result is the terminal execution event: `finished_unix_ms` and the
recorded duration are frozen at that instant and are never inflated by draining
bytes after it exited. A scheduled command can legitimately spawn a descendant
that inherits fd 1 or 2 and then exit; that descendant is not greggd's child and
must not keep the one global child slot occupied, so greggd never waits for pipe
EOF as its completion signal. After the wait resolves, output capture continues
for at most one fixed, non-configurable settle bound
(`POST_EXIT_OUTPUT_SETTLE`, 250 ms), ending early the moment both streams reach
EOF. At the bound greggd preserves every byte already folded into the
fixed-capacity tails, closes the read handles, finalizes the terminal record,
and frees the slot — a descendant that writes after that point is outside the
direct-child execution/history contract, and greggd does not block the scheduler,
spawn a process group, or kill anything it did not start to capture it.

Within that one settle budget the two streams make progress **independently**.
One stream can be open and permanently idle — inherited by a descendant that has
nothing left to say — so each iteration selects among the frozen deadline and one
pending `drain_step` per unfinished stream. The preferred stream branch alternates,
so when both are continuously ready neither starves the other and a pending read
on one stream never withholds bytes already available on the other. Only bytes
returned by a completed read are folded into the `RunningChild`-owned tails, so
cancelling a losing read loses nothing.

That independence holds **only until the frozen deadline fires**. The deadline is
the *first* branch of the biased select, which is what makes the 250 ms bound
authoritative rather than decorative: a descendant that inherits a descriptor and
keeps writing leaves a `drain_step` ready on every poll, so a drain ordered ahead
of the timer would win every iteration and extend the settle for as long as output
stayed available — turning the fixed budget into fiction and letting one inherited
writer hold the global child slot indefinitely. At or after the deadline the timer
wins over either stream and no further output read happens, no matter how ready
it is. An `Instant::now()` comparison would not be enough on its own: it could
pass and then lose the next poll to a drain that completed in between, so the
priority lives in the select ordering and is tested against a deadline that is
already ready.

Descendant output is still not guaranteed: greggd never waits for descendant EOF,
and the deadline is fixed at the direct child's exit and never restarted by a wake,
by a per-stream read, or by rebuilding the completion future. Normal executor
scheduling overhead after the timer wakes is not a new published duration and does
not change the direct-child timestamp.

The frozen exit, its status, and its settle deadline all live on the
`RunningChild` rather than in the completion future, so a completion future
cancelled and rebuilt by a wake keeps the first instant and cannot restart the
settle budget. Terminal attribution uses the configuration index the child was
launched with; the running child carries it authoritatively, so there is no
job-name search and no index-zero fallback.

Wire conversion happens after the byte bound: lossy UTF-8, then the frozen
JSON-escaped length budget (512 escaped bytes per stream), with independent
`truncated` flags. greggd never interprets ANSI escapes; terminal sanitization
is a client-side responsibility.

The published resource constants and the client body caps are frozen in
`architecture/protocol.md` and `plans/162-scheduler-observability-contract-and-resource-qualification.md`.

### Scheduled maintenance

The scheduler is spawned only when the validated config has jobs. It receives
the sampler's latest readiness and optional v2 load scalar through a Tokio
watch channel. It only borrows that state at due/retry decisions; sampler
publication does not wake the scheduler. Warming, failed, or missing load is
unavailable and fails closed. It never probes collectors or HTTP.

The engine owns one parsed schedule and at most one pending occurrence per
configured job, and borrows operator configuration instead of copying it. It
scans at most 64 jobs for the next cron/retry/expiry deadline, sleeps until
that deadline, child completion, or shutdown, and has one global child slot.
Cron semantics read local civil time while load-retry/max-wait/child
lifecycle use monotonic time, and the event loop never trusts a semantic
deadline for more than a 60-second civil-clock reconciliation bound without
re-reading wall time: the actual sleep is `min(semantic deadline, now + 60s)`.
A forward wall-clock jump is therefore observed within about a minute and
skipped occurrences coalesce rather than replay; a backward jump cannot
launch before the stored civil occurrence is due. Reconciliation wakes run
the existing bounded scan silently and perform no telemetry, HTTP,
filesystem, or process work. A daemon with no configured jobs spawns no
scheduler task and pays zero cost.
Pending selection is an allocation-free bounded scan over job state: the
oldest pending time wins with config order as the stable tie breaker, and
no candidate vector is built merely to choose a job. Every load-gated launch
rechecks the latest cached load, and **every** load-blocked occurrence is
rescheduled whether or not a winner was chosen: a job that becomes due while
the gate is closed must never keep an elapsed `retry_at`, or that past instant
would become the `sleep_until` deadline and the loop would spin. A wake
deadline is therefore only ever a retry that is still in the future, and a
load-delayed job is published as load-delayed for its whole wait with a real
`next_retry_unix_ms` countdown. No occurrence history is persisted or
replayed after restart. The process adapter uses direct Tokio argv execution
with stdin null and `kill_on_drop`; stdout/stderr are null unless history
capture is enabled, in which case they are piped and concurrently drained (see
Scheduler observability above). Active-child termination and wait stay inside
the shared shutdown deadline.

Configuration loading already proves that a parsed schedule can be satisfied by
at least one Gregorian date, using a fixed calendar epoch and the same
day-matching semantics as runtime scheduling, so a calendar-impossible
expression such as `0 0 31 2 *` is an `InvalidJobs` violation before the
listener binds. A later schedule-arithmetic failure is an internal time-domain
error: it propagates through the existing scheduler fatal task boundary instead
of substituting a fabricated retry date.

### Configuration

```toml
# Optional: retained terminal scheduler records per job, in memory.
# Omit for the default of 5; hard maximum 10; 0 disables record retention.
scheduler_history_limit = 5

name = "greggd"           # display name, max 128 characters
host = "0.0.0.0"          # bind address
port = 11310              # TCP port (1-65535)
sample_interval_ms = 1000 # 250-60000
stale_after_ms = 10000    # 0 = disabled, else > sample_interval_ms
allow_privileged_jobs = false

[[jobs]]
name = "cargo-cleanme-deep"
schedule = "0 3 * * 0"
command = ["/usr/local/bin/cargo-cleanme", "scan", "--deep"]
working_dir = "/home/user/projects"
max_load = 8.0
load_window = "15m"
retry_interval_ms = 300000
max_wait_ms = 86400000
```

Scheduled commands run under the existing OS principal and do not use an
implicit shell. The readable config is not a secret store. Linux systemd's
`ProtectHome=true` may make developer paths inaccessible; rootless user-local
daemon execution is the supported path for those jobs. Unix euid 0 requires
the explicit privileged-job flag; Windows rejects load gates.

`name` is the human-readable `system.name` in published snapshots. It must be
non-empty, at most 128 characters, and contain no control characters. Foreground
startup and the Windows SCM worker load and validate the config before creating
their native collector, then pass that name as the collector display-name
override. `system.hostname` is collected independently from the native host
interface and is never replaced by the configured name.

Validation produces structured `ConfigViolation` values. Atomic writes use
write-flush-verify-rename: serialize to a `0600` temp file, `fsync`, load back
and compare, then rename; the final daemon config is relaxed to `0644` because it carries no secrets
and read-only `croncheck`/`status`/`configprint` must work for unprivileged
operators and cron. Systemd/launchd installs repair older `0600` system
configs to `0644` with a traversable (`0755`) parent; the Unix control
socket stays `0600`.

Platform defaults:
- Linux: `/etc/gregg/greggd.toml`
- macOS: `/Library/Application Support/gregg/greggd.toml`
- Windows: `%ProgramData%\gregg\greggd.toml`

When `host` or `port` is run without `--config`, a missing default file is
initialized from `Config::default()`. A missing explicitly supplied path is a
configuration error and is neither written nor followed by process management.

### CLI subcommands

| Command | Purpose |
|---------|---------|
| `run` | Start foreground daemon |
| `stop` | Stop a running daemon via local Unix-domain control socket (Linux/macOS) or Windows SCM; idempotent when already stopped |
| `croncheck` | Watchdog for cron and other non-systemd supervisors: bounded raw HTTP `/v2/healthz` probe on the configured local bind (wildcards normalized to loopback); valid Gregg Ready/Warming/Failed means running, refusal alone permits a detached `<current_exe> run` spawn, and unrelated/malformed/silent/ambiguous peers return nonzero without spawning |
| `configprint` | Read configured bind address and print one canonical `host:port` line; bind wildcards (`0.0.0.0`, `::`) are resolved to the host's primary local IP so the output is a usable address, and the original wildcard is preserved if the local IP cannot be resolved; no network I/O beyond a local route lookup, no listener bind, no service, no config mutation |
| `status` | Read-only local diagnostics: version, config path, canonical bind `host:port`, bounded `/v2/healthz` classification (`ready`/`warming`/`failed`/`unreachable`/`not-gregg`, same probe authority as `croncheck`), detected startup-manager state. Exit 0 only when a valid Gregg endpoint answered; never starts/stops/restarts/installs, never infers process ownership from port occupancy, never invokes `sudo` |
| `startup install` | Install and enable automatic startup (`auto` default; `--method auto|systemd|launchd|cron`). Systemd uses `/usr/local/bin/greggd` + the selected `--config` (default `/etc/gregg/greggd.toml`, rendered into `ExecStart`) + `greggd` user/group + `/etc/systemd/system/greggd.service` (atomic, `daemon-reload`/`enable`/`start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist` with the selected `--config` rendered into `ProgramArguments`; cron uses idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab, never edits `/var/spool/cron`). Auto picks Windows→SCM, macOS→launchd, Linux systemd→systemd else cron. Identified systemd/launchd never silently falls back to cron on permission failure; prints exact `sudo <exe> startup install --method <...>` and returns `PermissionDenied` without internal `sudo` |
| `startup instructions` | Read-only: prints exact commands/paths for the detected or specified method without mutating state |
| `restart` | Exact-executable-aware manager restart: only an owned systemd/launchd/SCM registration may receive manager mutation; foreign same-config and unknown Unix ownership fail closed, foreign different-known-config registrations permit only the selected config's direct path, and Windows has no direct fallback for missing/foreign/unknown SCM. Owned managers use `systemctl restart greggd`, `launchctl kickstart -k`, or SCM; otherwise Unix uses config-specific `stop` + detached `run`. Permission failures print exact elevated command and return `PermissionDenied` without competing fallback |
| `update` | Daemon lifecycle coordination over the shared `gregg-update` mechanism (version/target/asset/download/checksum/staging/replacement): exact `vX.Y.Z` asset + `.sha256`, staged temp, candidate `version` check, Cargo `=X.Y.Z` fallback only on 404; fully prepares before observing exact-executable `UpdateLifecycle` and before any stop (Windows SCM stop only when `query_registration()` still proves owned running/start-pending, owned stop-pending waits stopped without restart, foreign/unknown/not-installed do zero SCM mutation, owned-to-foreign fails before replacement; Unix owned/foreign/unknown + selected health decide managed/direct/stopped/preserved); preserves config/registration and restarts only `ManagedRunning`/`DirectRunning` via `restart_daemon()`, leaves stopped/foreign untouched; `UpdatedButRestartFailed` partial-success with exact restart command and nonzero exit |
| `uninstall [--dry-run] [--purge]` | Remove only the exact invoked daemon executable plus independently discovered startup artifacts whose parsed command target matches it (systemd `ExecStart`, launchd `ProgramArguments`, managed cron command, SCM image path); foreign/ambiguous artifacts are preserved, SCM query uncertainty blocks mutation, preflights permissions first, never `sudo`s internally, blocks deletion on uncertain direct stop, fails closed (blocking deletion) when the post-teardown endpoint re-probe cannot load the config at all, preserves config by default, `--purge` removes only resolved config/data files, and Unix Cargo-owned installs complete owned lifecycle before Cargo removal and post-success purge (Windows prints the zero-mutation handoff) |
| `host` | Atomically mutate bind host; applies on next start (Windows also restarts the SCM service via config-intent dispatch) |
| `port` | Atomically mutate port; applies on next start (Windows also restarts the SCM service via config-intent dispatch) |
| `version` | Print compile-time daemon version |

The binary boundary owns logging initialization and error presentation. The
runtime and CLI library functions return errors and never call
`std::process::exit()`. `main` installs tracing with non-panicking `try_init()`,
prints one diagnostic for failures, and applies the exit-code taxonomy: `0`
success, `1` configuration, `2` service management, `3` runtime, and `4`
permission denied.

### Optional drive refresh

Drive capacity is deliberately outside the critical sampler path. Each native
collector lazily creates at most one private standard-thread worker
(`DriveRefreshCache`) on its first `sample()`; its first request is immediate
and later requests use a 30-second cadence.
The collector polls a capacity-one result channel without waiting, retains the most
recent successful drive list through failures, and publishes `drives: null` until
a first result exists. A contained collection panic is reported and retried
with bounded backoff. Dropping the collector does not join a worker that may be
inside an uninterruptible filesystem syscall. The sampler only converts the
collector-owned `CollectedMetrics.drives` into the wire payloads.

### Unix control socket

`greggd run` on Linux/macOS binds a local Unix-domain control socket
alongside the TCP listener. The socket identity is derived from a normalized
config identity path via a deterministic 64-bit FNV-1a hex digest. Existing
config files are filesystem-canonicalized, so relative, absolute, and symlink
spellings of the same file produce the same control paths; an absent implicit
default uses a deterministic lexical absolute path without requiring the TOML
file to exist. The canonicalization happens exactly once per `run`/`stop`
operation and the resulting identity is threaded through the primary and
fallback derivations, so a file appearing or disappearing between calls cannot
flip the canonical/lexical branch and a differing `cwd` for `run` vs `stop`
cannot produce two `<id>` values for one spelling. Two different config files in the same directory still produce
different control paths (`greggd-<id>.control.sock`). Editing `host` or
`port` inside the same TOML does not change the `<id>`, so the same daemon
continues to advertise `greggd stop` at the same path. The socket file lives at
the canonical config-adjacent path when that directory is writable;
otherwise a deterministic fallback under the standard temp directory is
used. The chosen socket is reserved by the kernel's exclusive `bind`
operation, then restricted to `0600` and verified before the listener is
returned, so a concurrent creator can make the candidate fail but cannot be
displaced by a rename. A failed `chmod` causes
the candidate to be discarded before the next
candidate is tried, and if neither candidate yields a secure listener the
foreground entry point returns a clear runtime error rather than silently
losing stop capability. The control socket is removed on orderly shutdown
(SIGTERM/SIGINT or `greggd stop`), on startup failure, and on runtime
errors. Socket-file removal is owned by a `ControlSocketGuard` held by
`run_with_control_path` for the daemon's whole lifetime, not by the control
task: on the signal-driven path that task is still parked in `accept()` when
the runtime is torn down, so it can never reach its own cleanup.

`greggd stop` deliberately does not load or validate the config: control
identity is a function of the config *path* only (FNV-1a of the normalized
path, never `host`/`port`), so a corrupt or unreadable TOML cannot block
stopping a running daemon. It tries the config-adjacent path first, then the
temp-dir fallback. It sends `STOP\n`, reads `OK\n`, and exits 0. Missing or
unreachable sockets result in idempotent not-running output. Permission
errors map to exit code 4. Unexpected I/O conditions — for example a
daemon that accepts `STOP\n` but never replies — are reported as an
uncertain outcome with a warning and exit code 3 rather than being
conflated with "not running". The HTTP API remains read-only and is unrelated
to the control socket. Stale socket cleanup is conservative: only
`ConnectionRefused` and `NotFound` connect failures, after metadata has
confirmed the entry is a socket, authorize unlinking. `PermissionDenied`,
`TimedOut`, or any other unexpected error never unlinks an existing
entry.

### Windows service management

The Windows-only `ServiceManager` trait provides `start`, `stop`, `restart`,
`is_active`, `unregister`, and a bounded registration query (full
`ServiceState` plus the registered image target). The native query parses the
SCM `lpBinaryPathName` launch command narrowly: quoted absolute image paths
may have arguments, while ambiguous/unparseable commands fail closed so
uninstall reports unknown ownership instead of guessing. Unregister
stop-when-running, waits stopped, and deletes only the `greggd` registration;
missing registration is idempotent:

- **Windows SCM** (`windows.rs`) — uses the `windows-service` dispatcher and
  generated `ServiceMain` for the daemon entry, a one-shot control signal for
  Stop/Shutdown, a fail-closed image-command parser for ownership, and an
  `ScmAdapter` trait for lifecycle-manager testability

The existing `windows-2022` CI job builds the release daemon and runs
`scripts/smoke-windows.ps1` as the operational SCM proof. The bounded smoke
uses an occupied ephemeral loopback port for bind-failure verification and
checks service creation, `LocalService` configuration, custom config-path
handoff, post-bind readiness, restart/recovery, reinstall, install.ps1 helper
classification, component-safe CLI uninstall (sibling `gregg.exe` survives),
and cleanup.

## Collector architecture

### SystemCollector trait

```rust
pub trait SystemCollector: Send {
    fn identity(&self) -> Result<SystemIdentity, CollectError>;
    fn sample(&mut self) -> Result<CollectedMetrics, CollectError>;
    fn capabilities(&self) -> MetricCapabilities;
    fn capabilities_v2(&self) -> MetricCapabilitiesV2 { /* default */ }
    fn supports_v1_snapshot(&self) -> bool { true }
}
```

`capabilities_v2()` and `supports_v1_snapshot()` have default implementations
that derive from v1 capabilities. Windows overrides `supports_v1_snapshot()`
to return `false`. One call to `sample()` produces `CollectedMetrics` which
converts to both v1 and v2 wire formats without duplicate collection.

### CollectErrorKind

| Kind | Meaning |
|------|---------|
| `Warming` | First sample not yet available |
| `SourceUnavailable` | procfs/sysfs entry missing or unreadable |
| `Parse` | Metric file present but unparseable |
| `CounterReset` | Kernel counter wrapped or decreased |
| `Numeric` | Arithmetic error during normalization |
| `IdentityFallback` | Identity field unreadable; sampling fails without publishing a fabricated identity |

These are crate-local typed errors. Wire responses carry only `HealthCategory`.

### Platform collectors

See [collectors.md](collectors.md) for detailed platform-specific analysis. The
sampler validates the complete v2 payload, including optional live-telemetry
bounds and invariants, before publication; an invalid optional record is
discarded at the collector boundary and does not fabricate a zero value.

| Platform | Source | Key interfaces |
|----------|--------|---------------|
| Linux | `gregg-host/src/linux/` (facade `collector/linux/`) | `/proc/stat`, CPUFreq sysfs, `/sys/block/*/stat`, `/proc/net/dev`, network sysfs, mounts, `statvfs` |
| macOS | `gregg-host/src/macos/` (facade `collector/macos/`) | Mach/sysctl, `getloadavg`, `libc::getmntinfo`/`statfs`, `NET_RT_IFLIST2`/`if_msghdr2` with `getifaddrs`/`if_data` fallback, IOKit block statistics |
| Windows | `gregg-host/src/windows/` (facade `collector/windows/`) | `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo`, processor power, disk IOCTL, IP Helper |

## Tests

### Unit tests (counts as of 2026-10-01; approximate, not a contract)

Most modules have inline `#[cfg(test)]` tests (`server` keeps its handler
tests in the separate `server/tests.rs` file):

| Module | ~Test lines | Coverage |
|--------|--------|----------|
| `cli.rs` | ~280 | CLI parsing, config resolution, exit codes, mutations |
| `run.rs` | 400 | Supervision select, task joining, non-cooperative abort |
| `config.rs` | ~430 | Validation, atomic writes, TOML round-trips |
| `sampler.rs` | ~550 | Interval validation, readiness lifecycle, counter reset |
| `server/tests.rs` | ~1050 | All handlers, staleness, concurrency (50 parallel) |
| `service/windows.rs` | ~480 | MockScmAdapter for all states |
| `collector/linux/tests.rs` | ~970 | 40+ tests with fixture-driven invariants |
| `collector/macos/tests.rs` | ~620 | Mock-based + native smoke tests |
| `collector/windows/mod.rs` | ~300 | Topology guards, structural invariants |

### Integration tests

- `tests/linux_collector.rs` — live `/proc` smoke test
- `tests/windows_smoke.rs` — binary help + foreground daemon + v2 health polling
- `tests/installer_rerun.rs` — installer rerun / upgrade contract

### Test infrastructure

- 46 JSON/text fixture files in `src/collector/test_fixtures/`
- `FileSource`/`MemorySource` (Linux) — file seam plus in-memory map for deterministic tests
- `MockNativeQueries` (macOS) — injectable FFI with auto-increment CPU
- `MockWindowsSource` (Windows) — injectable API with auto-increment CPU
- `MockScmAdapter` (Windows SCM) — injectable service state
