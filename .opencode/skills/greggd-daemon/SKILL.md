---
name: greggd-daemon
description: Work with the greggd daemon crate (collectors wiring, sampler, HTTP server, control socket, croncheck, SCM service)
---

## What I do

Guide agents through the `greggd` daemon crate: runtime wiring, the sampler and
HTTP server, the Unix control socket behind `greggd stop`, the `croncheck`
watchdog, `configprint`, and Windows SCM service management.

## When to use me

Use this when modifying daemon runtime code (`run.rs`, `control.rs`, `net.rs`,
`sampler.rs`, `server/`), CLI subcommands, exit-code classification, or
service lifecycle. For platform metric collection itself, use the
`platform-collectors` skill instead.

## Key modules

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Binary boundary: logging init, diagnostics, exit-code classification |
| `cli` | `src/cli.rs` | Clap CLI and per-command dispatch; `ExitCode` taxonomy |
| `run` | `src/run.rs` | Supervision loop; `RunOutcome`, public `run_with_shutdown()`, pub(crate) `run_with_shutdown_on_ready()` callback seam |
| `config` | `src/config.rs` | TOML config, structured violations, atomic writes |
| `control` | `src/control.rs` | Unix-only control socket for `greggd stop` (`STOP\n` → `OK\n`) |
| `net` | `src/net.rs` | Wildcard-to-local-IP resolution for `configprint` (transient UDP `connect()`, no packets) |
| `sampler` | `src/sampler.rs` | Cadence + readiness lifecycle (`Warming` → `Ready`/`Failed`), identity-safe snapshot publication; `SyntheticClock` |
| `server` | `src/server/` | Axum HTTP server; one coherent published generation per response |
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
| `host` / `port` | Atomic persisted mutation; applies on next start |
| `version` | Compile-time version string |
| `start` / `restart` / `service` | Windows SCM only (`start`/`restart` are lifecycle managers; `service` is the internal SCM entry point) |

## Unix control socket invariants

- Control identity is an FNV-1a digest of the normalized config path:
  existing files use filesystem canonicalization so relative/absolute/symlink
  spellings converge; a missing implicit default uses a lexical absolute
  fallback. Identity is never derived from the parent directory alone — two
  configs in one directory cannot cross-stop.
- Sockets are created with `0600`; a failed `chmod` discards the candidate.
  The inode is bound inside a process-private `0700` staging directory in
  the same parent and renamed into place only after verification, so it is
  never publicly reachable with umask-derived permissions.
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
