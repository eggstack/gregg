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
| `cli` | `src/cli.rs` | Clap CLI and per-command dispatch (`update`/`uninstall` coordinate lifecycle, synchronously); `ExitCode` taxonomy; authoritative bounded health fetch (`fetch_health_bytes`) with detail (`probe_health`) and watchdog (`probe_greggd`) classifications |
| `run` | `src/run.rs` | Supervision loop; `RunOutcome`, public `run_with_shutdown()`, pub(crate) `run_with_shutdown_on_ready()` callback seam |
| `config` | `src/config.rs` | TOML config, structured violations, atomic writes |
| `control` | `src/control.rs` | Unix-only control socket for `greggd stop` (`STOP\n` → `OK\n`) |
| `net` | `src/net.rs` | Wildcard-to-local-IP resolution for `configprint` (transient UDP `connect()`, no packets) |
| `sampler` | `src/sampler.rs` | Cadence + readiness lifecycle (`Warming` → `Ready`/`Failed`), identity-safe snapshot publication; `SyntheticClock` |
| `server` | `src/server/` | Axum HTTP server; one coherent published generation per response |
| `startup/*` | `src/startup/*.rs` | Startup install/teardown/instructions/restart split by ownership (façade `src/startup.rs` re-exports `crate::startup::X`): method identity/paths/detection, bounded child execution, systemd unit/install/restart/uninstall, launchd plist/install/restart/uninstall, shell quoting + cron block/install/uninstall, `StartupState` detection, errors/atomic writes/privilege/install dispatch/instructions/restart coordination |
| `status` | `src/status.rs` | Read-only `status` model: `StatusReport`, injected `gather_status`, stable `render_status`, `status_is_present` (valid endpoint = ready/warming/failed, same running definition as `croncheck`) |
| `update` | `src/update.rs` | Thin daemon lifecycle coordinator over the shared `gregg-update` mechanism (binds identity, prepares via `prepare_candidate`, quiesces a running Windows SCM service only after preparation, manager-aware restart via `startup_state`, `UpdatedButRestartFailed`); transport/staging/replacement live in `gregg-update` |
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

## CLI subcommands

| Command | Contract |
|---------|----------|
| `run` | Foreground daemon; on Unix also binds the local control socket |
| `stop` | Unix: single tiny control socket targeting only the local instance matching the resolved config identity. Windows: delegates to SCM. Idempotent when already stopped |
| `croncheck` | Watchdog for non-systemd supervisors: bounded raw HTTP `/v2/healthz` probe on the configured **local** bind (wildcards normalized to loopback). Valid Gregg Ready/Warming/Failed means running; refusal alone permits detached `<current_exe> run`; unrelated, malformed, silent, or ambiguous peers return nonzero without spawning |
| `configprint` | Read-only print of the canonical bind `host:port`; wildcards resolve to the primary local IP. No probe, no bind, no config mutation, no service management |
| `status` | Read-only local diagnostics: version, config path, canonical bind `host:port`, bounded `/v2/healthz` classification (`ready`/`warming`/`failed`/`unreachable`/`not-gregg`, same probe authority as `croncheck`), detected startup-manager state. Exit 0 only when a valid Gregg endpoint answered; never starts/stops/restarts/installs, never infers process ownership, never invokes `sudo` |
| `startup install` | Install and enable automatic startup (`auto` default; `--method systemd|launchd|cron`). Systemd uses `/usr/local/bin/greggd` + `/etc/gregg/greggd.toml` + `greggd` user/group + `/etc/systemd/system/greggd.service` (atomic, `daemon-reload`/`enable`/`start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist`; cron uses idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab). Auto picks Windows→SCM, macOS→launchd, Linux systemd→systemd else cron. Identified systemd/launchd never silently falls back to cron; prints exact `sudo <exe> startup install --method <...>` and returns `PermissionDenied` without internal `sudo` |
| `startup instructions` | Read-only: prints exact commands/paths for the detected or specified method without mutating state |
| `restart` | Manager-aware restart (Windows SCM, systemd `systemctl restart greggd`, launchd `launchctl kickstart -k`, otherwise control `stop` + definitive endpoint-absence check + detached `run`); success requires a bounded valid Gregg health response, not merely process creation. Permission failures print exact elevated command and return `PermissionDenied` without competing fallback; factored for `update` reuse |
| `host` / `port` | Atomic persisted mutation; applies on next start |
| `version` | Compile-time version string |
| `start` / `service` | Windows SCM only (`start` is lifecycle manager; `service` is the internal SCM entry point) |
| `uninstall` | Removes only the exact invoked executable and startup artifacts whose parsed target matches it; foreign/ambiguous systemd, launchd, cron, and SCM artifacts are preserved, unknown SCM state blocks mutation, and config is purged only after successful Unix Cargo removal when `--purge` is requested |

## Unix control socket invariants

- Control identity is an FNV-1a digest of the normalized config path:
  existing files use filesystem canonicalization so relative/absolute/symlink
  spellings converge; a missing implicit default uses a lexical absolute
  fallback. Identity is never derived from the parent directory alone — two
  configs in one directory cannot cross-stop.
- Sockets are bound at their final path with the kernel's exclusive `bind`,
  then set to `0600` and verified; a concurrent creator can make the candidate
  fail but cannot be displaced by a rename. A failed `chmod` discards it.
- Stale socket cleanup unlinks only when metadata confirms a socket **and**
  the connect error is `ConnectionRefused` or `NotFound`.
  `PermissionDenied` and unexpected errors never authorize unlinking.
- Cleanup runs on every exit path, including signals and runtime errors.
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
  `tests/windows_smoke.rs` (binary help + foreground daemon + v2 health)
- Windows SCM truth comes from `scripts/smoke-windows.ps1` in CI

## Deep dive

See `architecture/greggd-daemon.md` for the full document.
