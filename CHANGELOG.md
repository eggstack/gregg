# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and
this project adheres to [Semantic Versioning](https://semver.org/).

## [1.0.1] - 2026-07-23

### Fixed

- **launchd stop idempotency** (`greggd`): `greggd stop` now returns success
  when the service is already unloaded, instead of failing with a launchd
  not-found error. This makes stop safe to call unconditionally in scripts
  and automation.
- **Client config permissions** (`gregg`): All atomic configuration writes now
  enforce `0600` (owner read/write only) permissions on the final config file,
  preventing other users from reading endpoint credentials or host lists.
- **Lock-file truncation** (`gregg`): The advisory lock file is no longer
  truncated during acquisition. Previous behavior could silently drop lock
  content on concurrent access; the fix preserves existing file content.
- **Release-candidate workflow** (`.github/workflows/release-candidate.yml`):
  Candidate refs are now resolved to immutable commit SHAs before comparison,
  so tag and branch inputs work correctly. Package verification is staged
  (`gregg-protocol` first, then `greggd` and `gregg`) so binary crates can
  resolve the indexed protocol dependency.
- **Installed daemon loopback verification** (`scripts/verify-installed-daemon.sh`):
  New script that installs the daemon from a clean root, starts it, verifies
  the HTTP `/healthz` endpoint responds on loopback, and stops the service.
  Replaces the previous masked `|| true` smoke test.

- **macOS FFI** (`greggd`): `mach_host_self()` and `mach_task_self()` are now
  declared as foreign functions instead of being assumed to exist. `HostPort::current()`
  returns `Result` and rejects `MACH_PORT_NULL`. `Drop` releases via
  `mach_task_self()` rather than `MACH_PORT_NULL`. Swap-info length is validated
  before field access.
- **macOS collector** (`greggd`): Added `complete_production_collector_smoke`
  native test verifying CPU iowait is reported as unsupported/null and memory/
  swap metrics are sane.
- **CI** (`.github/workflows/ci.yml`): Explicit `macos-15` (arm64) and
  `macos-15-intel` (x86_64) matrix entries with architecture verification.
- **Resolved port storage** (`gregg`): `cmd_add` now stores the resolved port
  from `EndpointSpec` instead of the parser default, fixing the case where a
  non-default port from a previous config entry was overwritten.
- **Cross-process locking** (`gregg`): Replaced in-process `AdvisoryLock` with
  OS-level `flock` (`FileLockGuard`) so concurrent CLI invocations across
  processes cannot corrupt the config file. Lock timeout is configurable.
- **`port_was_explicit` removal** (`gregg`): Removed `port_was_explicit` from
  `SystemEntry` (the persistence struct). `EndpointSpec` retains it for CLI
  disambiguation.
- **Scheduler** (`gregg`): One trigger (manual refresh or periodic tick) now
  produces exactly one generation — the old fall-through caused double polls on
  Ctrl-R. Closing the refresh channel no longer causes a busy loop. Timer uses
  `tokio::time::interval` with `MissedTickBehavior::Skip` for fixed cadence;
  manual refresh does not reset the periodic schedule.
- **Response size cap** (`gregg`): The body-size check now happens before
  `extend_from_slice`, preventing a single oversized chunk from allocating
  beyond `MAX_RESPONSE_BYTES` (64 KiB).
- **Daemon supervision** (`greggd`): Unexpected clean exit from the HTTP server
  or sampler (without a shutdown signal) is now treated as a failure. State
  updates from the sampler callback are awaited inline (no detached spawns).
  After a shutdown signal, both tasks are joined with a 10-second timeout.
- **launchd state semantics** (`greggd`): `start()` now bootstraps if the
  service is not loaded, kickstarts if loaded but not running, and is a no-op
  if already running. `is_active()` returns true only when the service is
  actually running (not just loaded). Added `ServiceState` enum and `state()`
  method using `launchctl print`.
- **Installer root resolution** (`packaging/`): `install-linux.sh` and
  `install-macos.sh` now resolve the default binary path one level up
  (`$(dirname "$0")/..`) instead of two, matching the `packaging/` directory
  layout.
- **systemd non-root identity** (`packaging/`): The unit file now runs as a
  dedicated `greggd` user/group with `RuntimeDirectory=gregg`. The Linux
  installer creates the system user and sets config ownership.
- **CI package checks** (`.github/workflows/ci.yml`): Removed `--allow-dirty
  --no-verify` from all `cargo package` invocations so packages are verified
  and the working tree must be clean. Added shellcheck step for installer
  scripts.

## [1.0.0] - 2026-07-23

### Added

- `gregg-protocol` crate: versioned JSON wire types, metric capabilities,
  identity structures, and snapshot validation for schema version 1.
- `greggd` crate: lightweight Linux and macOS metrics daemon with read-only
  HTTP API (`/`, `/v1/status`, `/healthz`), periodic sampling, graceful
  shutdown, TOML configuration, and native service integration (systemd,
  launchd).
- `gregg` crate: compact keyboard-first terminal monitor with endpoint
  management (`add`, `list`, `remove`, `refresh`, `edit`), bounded concurrent
  polling, application state engine, and Ratatui-based four-row-per-system TUI.
- Native Linux metrics collection from `/proc` (CPU, memory, swap, load,
  identity).
- macOS metrics collection from Mach host statistics and sysctl (CPU, memory,
  swap, load, identity).
- Protocol compatibility fixtures for Linux, macOS, and health responses.
- Supply-chain policy via `cargo-deny`.
- CI pipeline: formatting, clippy, tests, docs, and package validation on
  Linux and macOS.

### Known limitations

- macOS does not expose a Linux-equivalent aggregate CPU I/O-wait state.
  This is reported as unsupported (`iowait_pct: null`) rather than
  fabricated as zero.
- The daemon is designed for private-network use only. It does not provide
  TLS, authentication, rate limiting, or other public-internet hardening.
- Per-process inspection, historical telemetry, alerting, and web dashboards
  are explicitly out of scope for version 1.
