---
name: greggd-daemon
description: Work with the greggd daemon crate (collectors wiring, sampler, HTTP server, control socket, croncheck, SCM service)
---

## What I do

Guide agents through the `greggd` daemon crate: runtime wiring, the sampler and
HTTP server, the Unix control socket behind `greggd stop`, the `croncheck`
watchdog, `configprint`, read-only `status`, `startup install`/`instructions`
and manager-aware `restart`, and Windows SCM service management.

## When to use me

Use this when modifying daemon runtime code (`run.rs`, `control.rs`, `net.rs`,
`sampler.rs`, `server/`), CLI subcommands, exit-code classification, or
service lifecycle. For platform metric collection itself, use the
`platform-collectors` skill instead.

## Key modules

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Binary boundary: logging init, diagnostics, exit-code classification |
| `cli` | `src/cli.rs` | Clap CLI and per-command dispatch (`update`/`uninstall` coordinate lifecycle, synchronously); `ExitCode` taxonomy; authoritative bounded health fetch (`fetch_health_bytes`: per-read timeout plus a 5s total deadline) with detail (`probe_health`) and watchdog (`probe_greggd`) classifications; `dispatch()` is deprecated because it cannot distinguish an explicit `--config` from the implicit default, so the binary boundary uses `dispatch_with_config_intent` |
| `run` | `src/run.rs` | Supervision loop; `RunOutcome`, public `run_with_shutdown()`, pub(crate) `run_with_shutdown_on_ready()` callback seam |
| `config` | `src/config.rs` | TOML config, structured violations, atomic writes |
| `control` | `src/control.rs` | Unix-only control socket for `greggd stop` (`STOP\n` → `OK\n`) |
| `net` | `src/net.rs` | Wildcard-to-local-IP resolution for `configprint` (transient UDP `connect()`, no packets) |
| `sampler` | `src/sampler.rs` | Cadence + readiness lifecycle (`Warming` → `Ready`/`Failed`), identity-safe snapshot publication; `SyntheticClock` |
| `server` | `src/server/` | EggServe 0.4 direct H1 service; one coherent published generation per response, with shared immutable status bytes and explicit bounded runtime policy |
| `startup/*` | `src/startup/*.rs` | Startup install/teardown/instructions/restart split by ownership (façade `src/startup.rs` re-exports `crate::startup::X`): method identity/paths/detection, bounded child execution, systemd unit/install/restart/uninstall, launchd plist/install/restart/uninstall, shell quoting + cron block/install/uninstall, `StartupState` detection, errors/atomic writes/privilege/install dispatch/instructions/restart coordination |
| `status` | `src/status.rs` | Read-only `status` model: `StatusReport`, injected `gather_status`, stable `render_status`, `status_is_present` (valid endpoint = ready/warming/failed, same running definition as `croncheck`) |
| `update` | `src/update.rs` | Exact-executable-aware lifecycle coordinator over the shared `gregg-update` mechanism (binds identity, prepares via `prepare_candidate`, observes `UpdateLifecycle` after preparation — Unix ownership + selected health, Windows `query_registration()` revalidated before quiescence with owned-to-foreign failing pre-replacement — quiesces only owned Windows running, replaces, restarts only `ManagedRunning`/`DirectRunning` via `restart_daemon()`, `UpdatedButRestartFailed`); transport/staging/replacement live in `gregg-update` |
| `uninstall` | `src/uninstall.rs` | Component-safe daemon uninstall: independent discovery, explicit `ArtifactOwnership` from exact systemd/launchd/cron/SCM executable targets, pure plan shared by `--dry-run`/execution, preflight before teardown, startup-owner teardown + SCM `unregister`, direct control-stop with uncertain-stop blocking, default config preservation with opt-in `--purge`; Unix Cargo-owned lifecycle completes before Cargo removal and post-success purge |
| `service` | `src/service/` | Windows-only `ServiceManager` (`start`/`stop`/`restart`/`is_active`/`unregister` plus bounded state/registration query); the native query parses only an unambiguous absolute image from SCM `lpBinaryPathName` and fails closed on ambiguous commands; native dispatcher entry; fake `ScmAdapter` tests run on every platform |

## Runtime ownership

- The binary dispatches synchronously **before** creating any Tokio runtime.
- Foreground `run` creates exactly one current-thread runtime at the binary
  boundary.
- Windows `service` first enters `service_dispatcher::start`; the generated
  `ServiceMain` worker owns exactly one current-thread runtime.
- SCM reports `RUNNING` only after the shared daemon core binds its listener
  (the `on_ready` seam fires post-bind).
- Configured maintenance jobs add one scheduler subsystem only when the job
  list is non-empty. It reads cached sampler load from a watch channel without
  waking on each sample; it executes direct argv as the existing OS principal,
   retains one pending occurrence per job and one global child, and terminates
  only its direct child during shutdown. Cron eligibility reads local civil
  time while retry/max-wait/child bounds stay monotonic; the sleep is capped
  at `min(semantic deadline, now + 60s)` so wall-clock jumps reconcile within
  about a minute (forward jumps coalesce, backward jumps never launch early).
  Never add an implicit shell, secret
  environment map, missed-job replay, remote execution route, or sandbox
  weakening. Unix root requires the explicit `allow_privileged_jobs` opt-in;
  Windows rejects load-gated jobs. Configuration validation rejects a schedule
  that no Gregorian date can satisfy (for example `0 0 31 2 *`) before the
  listener binds; a later schedule-arithmetic failure propagates to the
  scheduler's fatal task boundary instead of substituting a fallback date.
- Scheduler observability is **additive, read-only, and never merged into
  `/v2/status`**. `GET/HEAD /v2/scheduler` and `GET/HEAD /v2/scheduler/history`
  are the only scheduler routes; other methods are 405 and anything else under a
  scheduler path is 404. There is no job control plane, ever. Keep the scheduler
  publication in its own cell rather than the high-frequency metrics
  `PublishedState`, publish only on externally visible state change (so the
  one-minute civil-clock reconciliation wake stays silent), and keep handlers to
  serving already-serialized bytes — no serialization, no awaiting mutation, no
  scheduler lock held across an await, no config/telemetry/process access.
  History is memory-only (`scheduler_history_limit`, default 5, hard max 10,
  `0` = no retention) with no file, database, or replay. `spawn_failed` and
  `load_expired` are terminal records, not omissions.
- **The direct child's wait result is the terminal execution event.** Pipe
  stdout/stderr only when history is enabled and take both handles right after
  the spawn, but treat output as bounded *after* the child exits: freeze
  `finished_unix_ms`/duration at the wait result, then spend at most one fixed
  non-configurable post-exit settle (250 ms) on bytes still in flight before
  closing the read handles, finalizing the record, and freeing the one global
  child slot. A descendant that merely inherited a descriptor must never retain
  the slot, and greggd never kills it or claims its output. Inside that settle
  budget stdout and stderr progress **independently** — select one pending
  `drain_step` per unfinished stream plus the frozen deadline, with an
  alternating preferred branch — so an idle open writer on one stream cannot
  withhold bytes already available on the other, and neither stream can starve
  the other. Fold only the bytes a completed read returns; the losing read is
  cancelled, never partially applied. While the child is alive, both streams
  drain **concurrently with the wait** by borrowing the
  streams (never `wait_with_output`, never a spawned drain task, never a line
  reader that can grow unboundedly, and never one stream drained to EOF before
  the other) so neither a cancelled select nor a flooding child can deadlock the
  slot or delay the two-second shutdown bound. The frozen exit and its settle
  deadline live on the `RunningChild`, not in the completion future, so a
  rebuilt future never re-stamps or restarts them. Retain fixed-size tails only
  (1024 raw bytes per stream, 512 JSON-escaped published bytes, independent
  `truncated` flags). Attribute every terminal record through the index the
  child was launched with — never a job-name search, never an index-zero
  fallback. Never publish `argv` or `working_dir`; do state in documentation
  that the listener is unauthenticated so anyone who can reach it can read job
  output.
- SIGTERM/SIGINT, SCM Stop/Shutdown, and a successful `STOP\n` on the control
  socket all feed the same nonblocking one-shot shutdown signal into
  `run_with_shutdown()` (10s graceful deadline).

The daemon's v2 live telemetry is additive and best effort. CPU frequency is
the current OS-reported value, not a base/max claim; macOS may omit it. Disk
capacity is separate from disk I/O, and `R/s`/`W/s` plus `Rx/s`/`Tx/s` are byte
rates derived from native cumulative counters using monotonic elapsed time.
Network utilization is directional/full-duplex-safe, and loopback is detail
only rather than aggregate capacity. Reset, restart, hotplug, disappearance,
or unsupported optional sources re-baseline or omit that family without
blocking core readiness. Older v1 and pre-feature v2 peers remain supported;
daemon-version transport is deferred.

`Sampler` publishes existing typed `Arc` snapshots through internal
`ServerState` handoff helpers. The server prepares compact v1/v2 status JSON
once per successful publication and stores it as shared `Bytes`; public typed
snapshot/health accessors remain source-compatible. Fresh status requests make
the staleness/failure decision under the publication lock before serving the
cached bytes, while health responses are reconstructed on demand. A failed
cache preparation falls back to request-time serialization without panic.

## CLI subcommands

| Command | Contract |
|---------|----------|
| `run` | Foreground daemon; on Unix also binds the local control socket |
| `stop` | Unix: single tiny control socket targeting only the local instance matching the resolved config identity. Windows: delegates to SCM. Idempotent when already stopped |
| `croncheck` | Watchdog for non-systemd supervisors: bounded raw HTTP `/v2/healthz` probe on the configured **local** bind (wildcards normalized to loopback). The fetch has a per-read timeout *and* a 5s total deadline, so a slow-loris peer cannot hold it open. Valid Gregg Ready/Warming/Failed means running; refusal alone permits detached `<current_exe> run`; unrelated, malformed, silent, or ambiguous peers return nonzero without spawning |
| `configprint` | Read-only print of the canonical bind `host:port`; wildcards resolve to the primary local IP. No probe, no bind, no config mutation, no service management |
| `status` | Read-only local diagnostics: version, config path, canonical bind `host:port`, bounded `/v2/healthz` classification (`ready`/`warming`/`failed`/`unreachable`/`not-gregg`, same probe authority as `croncheck`), detected startup-manager state. Exit 0 only when a valid Gregg endpoint answered; never starts/stops/restarts/installs, never infers process ownership, never invokes `sudo` |
| `startup install` | Install and enable automatic startup (`auto` default; `--method systemd|launchd|cron`). Systemd uses `/usr/local/bin/greggd` + the **selected `--config`** (default `/etc/gregg/greggd.toml`, rendered into `ExecStart`) + `greggd` user/group + `/etc/systemd/system/greggd.service` (atomic, `daemon-reload`/`enable`/`start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist` with the selected `--config` rendered into `ProgramArguments`; cron uses idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab). Auto picks Windows→SCM, macOS→launchd, Linux systemd→systemd else cron. Identified systemd/launchd never silently falls back to cron; prints exact `sudo <exe> startup install --method <...>` and returns `PermissionDenied` without internal `sudo` |
| `startup instructions` | Read-only: prints exact commands/paths for the detected or specified method without mutating state |
| `restart` | Exact-executable-aware manager restart: only owned systemd/launchd/SCM registrations receive manager mutation; foreign same-config or unknown Unix ownership fails closed, a foreign different-known-config registration permits only the selected config's direct path, and Windows has no direct fallback for missing/foreign/unknown SCM. Otherwise Unix uses control `stop` + definitive endpoint-absence check + detached `run`; success requires a bounded valid Gregg health response, not merely process creation. Permission failures print exact platform-correct elevated guidance and return `PermissionDenied` without competing fallback |
| `host` / `port` | Atomic persisted mutation; applies on next start |
| `version` | Compile-time version string |
| `start` / `service` | Windows SCM only (`start` is lifecycle manager; `service` is the internal SCM entry point) |
| `uninstall` | Removes only the exact invoked executable and startup artifacts whose parsed target matches it; foreign/ambiguous systemd, launchd, cron, and SCM artifacts are preserved, unknown SCM state blocks mutation, and config is purged only after successful Unix Cargo removal when `--purge` is requested |

## Unix control socket invariants

- Control identity is an FNV-1a digest of the normalized config path:
  existing files use filesystem canonicalization so relative/absolute/symlink
  spellings converge; a missing implicit default uses a lexical absolute
  fallback. Identity is never derived from the parent directory alone — two
  configs in one directory cannot cross-stop. The canonicalization happens
  **once per `run`/`stop`** and the identity is threaded through the primary
  and fallback derivations, so a file appearing between calls cannot flip the
  canonical/lexical branch and a differing `cwd` cannot produce two `<id>`
  values for one spelling.
- Sockets are bound at their final path with the kernel's exclusive `bind`,
  then set to `0600` and verified; a concurrent creator can make the candidate
  fail but cannot be displaced by a rename. A failed `chmod` discards it.
- Stale socket cleanup unlinks only when metadata confirms a socket **and**
  the connect error is `ConnectionRefused` or `NotFound`.
  `PermissionDenied` and unexpected errors never authorize unlinking.
- Cleanup runs on every exit path, including signals and runtime errors. The
  `ControlSocketGuard` is owned by `run_with_control_path` for the daemon's
  whole lifetime, **not** by the control task: on the signal-driven path that
  task is still parked in `accept()` when the runtime is torn down, so it can
  never reach its own cleanup.
- `greggd stop` never loads or validates the config; identity is path-only, so
  a corrupt TOML cannot block stopping a running daemon.
- Client reads and responses have a one-second deadline; malformed/partial clients are dropped, transient accept errors back off, and control-task failure cannot request daemon shutdown.
- `stop` treats missing/refused candidates as idempotent "not running";
  unexpected I/O conditions yield `StopOutcome::Uncertain` (exit `3` at
  the binary boundary), never a silent not-running success.

## Exit codes

`0` success · `1` configuration · `2` service management · `3` runtime ·
`4` permission denied

Reusable library/runtime code returns typed errors without printing or calling
`std::process::exit()`; the binary boundary owns logging and exit codes.
Failures while awaiting the non-Unix Ctrl-C shutdown source follow the same
runtime error path rather than panicking.

Sampler identity failures follow the ordinary warming/failed lifecycle and
preserve any previous valid snapshot; they never publish a fabricated blank
identity. Daemon display names are non-empty, at most 128 bytes, and may not
contain control characters.
Backward wall-clock movement does not make a future-dated cached snapshot stale;
age-based staleness applies only to a non-negative elapsed age.
If the clock is before the Unix epoch, the sampler does not publish timestamp
`0`; with age-based staleness enabled, the server treats cached data as stale
until the clock is corrected.
Configuration metadata errors are propagated instead of treated as a missing
default file. Atomic writes restrict newly created parent directories to
`0700` while preserving permissions on existing operator-managed directories.
Daemon config temp files are `0600` during the write, then the final file is
`0644` (no secrets; unprivileged `croncheck`/`status`/`configprint` must work).
Systemd/launchd installs repair older `0600` system configs to `0644`/`0755`;
control sockets stay `0600`.

## Tests

- Inline unit tests in every module; server handler tests in `src/server/tests.rs`
- `MemorySource` (Linux), mock FFI seams (macOS/Windows) — see `platform-collectors`
- Integration: `tests/linux_collector.rs` (live `/proc` smoke),
  `tests/windows_smoke.rs` (binary help + foreground daemon + v2 health),
  `tests/installer_rerun.rs` (same-scope bootstrap replacement rules)
- Windows SCM truth comes from `scripts/smoke-windows.ps1` in CI

## Deep dive

See `architecture/greggd-daemon.md` for the full document.
