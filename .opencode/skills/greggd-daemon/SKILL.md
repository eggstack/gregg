---
name: greggd-daemon
description: Work with the greggd daemon crate (collectors wiring, sampler, HTTP server, control socket, croncheck, SCM service)
---

## What I do

Guide agents through the `greggd` daemon crate: runtime wiring, the sampler and
HTTP server, the Unix control socket behind `greggd stop`, the `croncheck`
watchdog, `configprint`, `startup install`/`instructions` and manager-aware
`restart`, and Windows SCM service management.

## When to use me

Use this when modifying daemon runtime code (`run.rs`, `control.rs`, `net.rs`,
`sampler.rs`, `server/`), CLI subcommands, exit-code classification, or
service lifecycle. For platform metric collection itself, use the
`platform-collectors` skill instead.

## Key modules

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Binary boundary: logging init, diagnostics, exit-code classification |
| `cli` | `src/cli.rs` | Clap CLI and per-command dispatch (`update` is synchronous, binary-first); `ExitCode` taxonomy |
| `run` | `src/run.rs` | Supervision loop; `RunOutcome`, public `run_with_shutdown()`, pub(crate) `run_with_shutdown_on_ready()` callback seam |
| `config` | `src/config.rs` | TOML config, structured violations, atomic writes |
| `control` | `src/control.rs` | Unix-only control socket for `greggd stop` (`STOP\n` → `OK\n`) |
| `net` | `src/net.rs` | Wildcard-to-local-IP resolution for `configprint` (transient UDP `connect()`, no packets) |
| `sampler` | `src/sampler.rs` | Cadence + readiness lifecycle (`Warming` → `Ready`/`Failed`), identity-safe snapshot publication; `SyntheticClock` |
| `server` | `src/server/` | Axum HTTP server; one coherent published generation per response |
| `startup` | `src/startup.rs` | Startup install/instructions/restart: auto systemd/launchd/cron/Windows SCM detection, atomic unit/plist write, bounded manager commands with captured stderr, cron quoting/merging, `startup_state()` for `restart`/`update`, `PermissionDenied` without silent fallback |
| `update` | `src/update.rs` | Binary-first self-update: crates.io `max_stable_version` via `curl`, SemVer compare, exact `vX.Y.Z` asset + `.sha256`, `sha2` verify, candidate `version` check, private `tempfile::TempDir` staging, real Cargo child kill/reap on timeout, Windows SCM stop only after complete preparation, `self-replace` atomic/WINDOWS, Cargo `=X.Y.Z` fallback only on 404, manager-aware restart via `startup_state`, `UpdatedButRestartFailed` |
| `service` | `src/service/` | Windows-only `ServiceManager`; native dispatcher entry |

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

## CLI subcommands

| Command | Contract |
|---------|----------|
| `run` | Foreground daemon; on Unix also binds the local control socket |
| `stop` | Unix: single tiny control socket targeting only the local instance matching the resolved config identity. Windows: delegates to SCM. Idempotent when already stopped |
| `croncheck` | Watchdog for non-systemd supervisors: bounded raw HTTP `/v2/healthz` probe on the configured **local** bind (wildcards normalized to loopback). Valid Gregg Ready/Warming/Failed means running; refusal alone permits detached `<current_exe> run`; unrelated, malformed, silent, or ambiguous peers return nonzero without spawning |
| `configprint` | Read-only print of the canonical bind `host:port`; wildcards resolve to the primary local IP. No probe, no bind, no config mutation, no service management |
| `startup install` | Install and enable automatic startup (`auto` default; `--method systemd|launchd|cron`). Systemd uses `/usr/local/bin/greggd` + `/etc/gregg/greggd.toml` + `greggd` user/group + `/etc/systemd/system/greggd.service` (atomic, `daemon-reload`/`enable`/`start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist`; cron uses idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab). Auto picks Windows→SCM, macOS→launchd, Linux systemd→systemd else cron. Identified systemd/launchd never silently falls back to cron; prints exact `sudo <exe> startup install --method <...>` and returns `PermissionDenied` without internal `sudo` |
| `startup instructions` | Read-only: prints exact commands/paths for the detected or specified method without mutating state |
| `restart` | Manager-aware restart (Windows SCM, systemd `systemctl restart greggd`, launchd `launchctl kickstart -k`, otherwise control `stop` + definitive endpoint-absence check + detached `run`); success requires a bounded valid Gregg health response, not merely process creation. Permission failures print exact elevated command and return `PermissionDenied` without competing fallback; factored for `update` reuse |
| `host` / `port` | Atomic persisted mutation; applies on next start |
| `version` | Compile-time version string |
| `start` / `service` | Windows SCM only (`start` is lifecycle manager; `service` is the internal SCM entry point) |

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

## Tests

- Inline unit tests in every module; server handler tests in `src/server/tests.rs`
- `MemorySource` (Linux), mock FFI seams (macOS/Windows) — see `platform-collectors`
- Integration: `tests/linux_collector.rs` (live `/proc` smoke),
  `tests/windows_smoke.rs` (binary help + foreground daemon + v2 health)
- Windows SCM truth comes from `scripts/smoke-windows.ps1` in CI

## Deep dive

See `architecture/greggd-daemon.md` for the full document.
