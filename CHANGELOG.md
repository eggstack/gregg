# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and
this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **Long-running daemon stability** (`greggd`): Unix control-client requests are
  bounded, transient listener errors retry with backoff, and control-channel
  degradation no longer becomes a successful daemon shutdown. Optional drive
  capacity refresh now runs in one bounded collector-owned worker at a slower
  cadence, retaining last-known-good data without blocking core snapshots or
  runtime shutdown. Linux excludes `autofs` and generic FUSE filesystems from
  drive probing.
- **Conservative `croncheck` identity** (`greggd`): the watchdog now validates a
  bounded `/v2/healthz` response, treats Ready/Warming/Failed Gregg health as
  running, starts only after connection refusal, and reports ambiguous occupied
  endpoints without spawning a competing daemon.

### Fixed

- **Remaining 2026-08-27 audit findings** (`gregg-protocol`, `gregg`,
  `greggd`): configuration metadata errors are no longer treated as missing,
  client request timeouts are bounded to 100–60,000 ms, v2 capability objects
  require all four explicit flags, identity fields are bounded to 512 UTF-8
  bytes, failed v1 health responses require a category, DNS classification no
  longer relies on display strings, EggPool deadlines honor its clock seam,
  endpoint deduplication uses ASCII case folding, and existing daemon config
  directory permissions are preserved.

- **Remaining audit findings** (`gregg`, `greggd`): identity failures no longer
  publish blank snapshots, malformed IPv6 zone IDs are rejected consistently,
  failed Systems config reloads remain last-known-good while showing a TUI
  diagnostic, pre-epoch clocks do not falsely stale cached snapshots, large
  byte ratios use widened arithmetic, daemon names reject control characters,
  and CI-blocking clippy diagnostics are resolved.

- **Audit corrections** (`gregg`, `greggd`): macOS byte percentages now use
  the shared collector normalization helper, non-Unix Ctrl-C listener errors
  return through the runtime error boundary instead of panicking, and a
  duplicate EggPool endpoint reports a dedicated configuration conflict.
- **Stale-snapshot 503 bodies no longer claim `ready`** (`greggd`): when a
  cached snapshot ages past `max_snapshot_age` (or the failure threshold is
  met) while the stored health state still says `ready`, the status and
  health handlers substitute a `CollectorFailure` failure ("cached snapshot
  is stale"), so the JSON body always agrees with the 503 status code.
- **One sampling-task panic no longer permanently degrades the daemon**
  (`greggd`): the collector is shared with the blocking pool behind a
  poisoning-tolerant mutex, so a single panic fails only that cycle and
  later ticks recover the lock and resume collection instead of losing all
  metrics until restart.
- **EggPool TLS errors are no longer misreported as DNS failures** (`gregg`):
  the overly broad `"name"` substring was removed from the fetch-error
  classifier, realigning it with the main poller's DNS classification.
- **Truncated mountinfo escapes keep the mount entry** (`greggd`): an octal
  escape cut off at end of input now contributes its raw characters instead
  of silently dropping the whole drive, and a statvfs result with zero block
  size or overflowing capacity logs a diagnostic before the drive is skipped.
- **Pre-epoch clocks are logged loudly** (`greggd`): a system clock behind
  the Unix epoch warns in the sampler and HTTP server instead of silently
  producing zero timestamps that would defeat staleness detection.
- **Client config writes fsync on every platform** (`gregg`): the temp-file
  `sync_all()` durability barrier is no longer Unix-only; builds on targets
  without a cross-process lock implementation (neither unix nor windows)
  now fail to compile rather than silently locking in-process only.
- **Terminal wrapper fixes** (`gregg`): `into_inner()` returns the wrapped
  ratatui terminal instead of fabricating a fresh one, and `restore()`
  flushes buffered frame state before tearing down global terminal mode.
- **Protocol identity docs match validation** (`gregg-protocol`):
  `SystemIdentity` docs now state that empty values are rejected for every
  field (including `name` and `hostname`) instead of claiming they are
  permitted.

- **Windows transient commit over-commit no longer fails the sample**
  (`greggd`): a commit charge momentarily above the commit limit (pagefile
  resize windows, kernel over-commit before expansion) is clamped to the
  limit with `usage_pct` saturating at 100 % instead of aborting the whole
  collection cycle and losing CPU, memory, and drive metrics.
- **EggPool command dispatch never blocks the TUI event loop** (`gregg`):
  pane commands are queued with `try_send`; a momentarily full bounded
  channel drops the command and surfaces a "worker busy" pane state instead
  of stalling key handling and poll batches behind a slow fetch. A closed
  channel still marks the worker unavailable.
- **Zero-total validation reports the root cause** (`gregg-protocol`): v1/v2
  memory, swap, and v2 commit payloads with zero capacity but nonzero used
  bytes now also report `ZeroNotAllowed` for the total/limit field (alongside
  `UsedExceedsTotal`) so consumers matching on violation kinds see that the
  total must be positive; all-zero metrics remain valid.
- **macOS VM counters widen consistently** (`greggd`): `vm_info64` now uses
  `widen_natural()` like `cpu_load_info`, so unsigned 32-bit Mach counters
  can never sign-extend into huge values.
- **Control-socket read errors are logged** (`greggd`): unexpected client
  read failures in the stop listener warn instead of being silently treated
  as EOF.
- **Unknown mountinfo escape sequences are logged** (`greggd`): a mount entry
  containing an octal escape outside `{040, 011, 012, 134}` is still skipped,
  but the skip is now visible in the log.
- **Control-socket startup permission window closed** (`greggd`): the Unix
  control socket is bound inside a process-private `0700` staging directory
  and atomically renamed into its final path only after the `0600` mode is
  verified, so the inode never exists publicly with umask-derived
  permissions.
- **`greggd stop` distinguishes unexpected failures from "not running"**
  (`greggd`): unexpected I/O conditions (for example a daemon that accepts
  `STOP\n` but never replies) now report an uncertain outcome with a
  warning and exit code `3` instead of silently printing "greggd not
  running" with exit code `0`. Missing and refused sockets remain an
  idempotent not-running success; permission errors still map to exit `4`.
- **Sampler no longer blocks the daemon runtime** (`greggd`): each collection
  cycle now runs on tokio's blocking thread pool, so hosts with many mounts or
  slow network filesystems can stretch `statvfs()` without stalling
  `/v1/status`, `/v2/status`, or `/healthz`.
- **Drive validation covers excess entries** (`gregg-protocol`): payloads above
  `MAX_DRIVE_ENTRIES` still report `TooManyDrives`, but individual violations
  in entries beyond the bound are now also reported for diagnostics.
- **Control-socket cleanup race narrowed** (`greggd`): stale-socket removal
  treats a concurrent unlink between the metadata check and the delete as
  success instead of surfacing a spurious error.
- **Config directory permission failures are logged** (`greggd`): a failed
  `0700` chmod on a freshly created config directory warns instead of being
  silently ignored.
- **EggPool deactivation aborts in-flight fetches** (`gregg`): leaving the pane
  promptly releases the pending request task instead of letting it run to a
  result that would be discarded anyway.
- **Dead EggPool generation assignment removed** (`gregg`):
  `apply_eggpool_result` no longer rewrites a generation that the guard just
  proved equal.

### Changed

- **Drive detail rendering allocates less** (`gregg`): expanded drive rows
  are built directly from drive references instead of cloning each
  eligible drive every frame. Rendering behavior is unchanged.
- **Shared percentage normalization** (`greggd`): v1/v2 swap percentages derive
  from one collector helper, preventing future v1/v2 divergence.
- **Client render/poll allocation reductions** (`gregg`): display order is
  computed once per action and render; normal-view metric rows are memoized
  per system and rebuilt only when a snapshot's content or fleet membership
  changes. Rendering behavior is unchanged.
- **Protocol docs** (`gregg-protocol`): documented the capability-flag
  absence-vs-`false` ambiguity, the accepted absence of a `usage_pct`
  vs byte-count cross-check, and the validate-after-decode requirement for
  health-response snapshots.
- **IPv6 zone-ID endpoints** (`gregg`): `status_url`/`v2_status_url` now bracket
  any colon-containing host per RFC 2732, so IPv6 zone IDs such as
  `fe80::1%eth0` produce valid request URLs.
- **Protocol validation hardening** (`gregg-protocol`): correctness pass over
  v1/v2 violation checks, health constructors, and test-support builders from a
  workspace bug audit.
- **Client and daemon hardening** (`gregg`, `greggd`): workspace audit
  corrections across poller outcome handling, endpoint parsing, state/UI text,
  the EggPool client, daemon configuration validation, control-socket stale
  entry handling, sampler accounting, and collector sources.

## [1.0.11] - 2026-08-19

### Changed

- Bumped all crate versions to 1.0.11.

### Added

- **Dynamic compact metric suffix** (`gregg`): when the longest natural metric
  suffix across the entire online fleet exceeds one quarter of the terminal
  width, every normal-view metric row renders bar-only fleet-wide until the
  terminal widens again; resizing wider restores suffixes dynamically
  (Plan 087).
- **Transient selection highlight** (`gregg`): the reverse-video highlight arms
  on Systems navigation and clears roughly ten seconds later via
  `Action::ClearSelectionHighlight`; persistent logical selection (`selected_id`)
  and `e` drive expansion are unaffected (Plan 087).
- **Header I/O-wait omission** (`gregg`): the normal-header `IO` token is
  omitted entirely when CPU I/O-wait is unsupported or has no real value,
  instead of rendering a placeholder (Plan 087).

### Fixed

- **Fleet-wide metric-row geometry** (`gregg`): one fleet-wide layout keeps the
  opening `[` and closing `]` columns aligned across every online system,
  including while scrolling; the DISK aggregate suffix became
  `<used bytes> / <total bytes>` so the slash denominator matches the
  percentage; expanded drive details share one selected-system table layout and
  condensed headings/values share one column layout (Plan 085).
- **Renderer boundary corrections** (`gregg`): condensed offline/pending rows
  keep their configured nickname or endpoint identity; expanded-drive fit math
  shares structural width constants with the renderer and degrades Compact via
  truncated names before Minimal; mixed `SWP`/`COMMIT` fleets budget suffixes
  against the same structural prefix width (Plan 086).

## [1.0.10] - 2026-08-18

### Changed

- Bumped all crate versions to 1.0.10.
- **`greggd croncheck` is now a watchdog** (`greggd`): the subcommand no
  longer performs a read-only HTTP probe of `/v2/healthz`. It opens a
  bounded TCP connect to the configured local bind address (with wildcards
  normalized to loopback). If a listener accepts the connection, it exits
  silently with status `0`. If nothing is listening, it spawns
  `greggd run` as a detached child (stdin/stdout/stderr closed; on Unix,
  in a new process group so signals sent to croncheck's group do not
  reach the daemon) and exits `0`. This restores the intended semantics
  for cron, Task Scheduler, and other supervisors that have no built-in
  readiness monitoring and need `croncheck` to actually start the daemon
  when it is not running.
- **`greggd croncheck --target HOST:PORT` removed** (`greggd`): the new
  watchdog operates only on the configured local bind. There is no remote
  probe mode; existing callers must drop the flag.
- **`greggd configprint` wildcard resolution** (`greggd`): a wildcard bind host
  resolves to the host's primary local IP (transient UDP `connect()` route
  lookup only) so the printed address is dialable; the wildcard is preserved
  verbatim if resolution fails.
- Crate metadata polish and docs.rs build fixes.

### Fixed

- **Compact TUI geometry and endpoint ergonomics** (`gregg`): shared
  normal-view metric-row geometry with aligned brackets, concise disk aggregate
  text, fresh-launch viewport snap on the first accepted poll batch only,
  explicit-port `gregg add` accepting `nickname@host:port` and HTTP URL forms,
  named versus unnamed offline rendering, and regression tests locking in
  offline-endpoint polling across generations (Plan 083), plus corrective
  closure of `--name` validation parity, renderer-level geometry proof,
  Unicode-aware offline padding, and `default_port` documentation (Plan 084).
- Workspace bug-audit findings across protocol validation, collectors, and
  client code.

## [1.0.9] - 2026-08-17

### Added

- **`probe_top` helper binary** (`gregg`): standalone TCP-connectivity probe
  driven by `PROBE_HOST`/`PROBE_PORT` environment variables; a development
  diagnostic, not part of the product CLI.

### Fixed

- Poller live-probe test coverage against a local fixture server.

## [1.0.8] - 2026-08-15

### Changed

- Bumped all crate versions to 1.0.8.

### Fixed

- **Croncheck target** (`greggd`): `greggd croncheck` now accepts a `--host` and
  `--port` flag to target a remote daemon, instead of only probing the local
  instance.

## [1.0.7] - 2026-08-15

### Changed

- Bumped all crate versions to 1.0.7.

## [1.0.6] - 2026-08-14

### Changed

- Bumped all crate versions to 1.0.6.

## [1.0.5] - 2026-08-12

### Fixed

- **Scheduler endpoint replacement** (`gregg`): Ctrl-R now reliably delivers the
  replacement endpoint through the bounded scheduler command channel and polls
  it immediately, instead of silently diverging state.
- **Client endpoint reload** (`gregg`): Reloaded configs reconcile stable system
  IDs and deliver replacements without losing pending state.
- **README** (`greggd`): Corrected `greggd host` description — it sets the bind
  address, not the display name.

## [1.0.3] - 2026-08-11

### Added

- Bounded mounted-local-filesystem capacity metrics in v2 status responses,
  with aggregate disk usage in the normal TUI and selected-system details in
  both normal and condensed views.
- Condensed fleet view with `h`/`l` (and arrow) view cycling plus `e` drive
  expansion while preserving mixed v1/v2 compatibility.

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
- **Installed daemon loopback verification** (`scripts/verify-installed-daemon.sh`):
  The verifier now accepts an explicit executable, writes the flat daemon TOML
  schema, validates bounded health/status responses, and checks the reaped
  child exit status.

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
- **Rust 1.75 dependency resolution** (`gregg`, `greggd`): Added documented
  compatibility-only bounds for the existing CLI, HTTP, URL, UUID, terminal,
  TOML, and crypto dependency graphs where current transitive releases exceed
  the declared MSRV. Fresh package and workspace resolution now stays below
  the edition-2024 and Rust-1.85-only dependency lines while retaining current
  active TLS fixes.

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
