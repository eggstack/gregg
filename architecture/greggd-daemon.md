# greggd daemon deep dive

The daemon crate is the metrics collection agent that runs on each monitored
host. It collects system metrics, samples them on a timer, serves them over
HTTP. On Unix it runs natively in the foreground; Windows SCM support remains
available through the Windows-only service path.

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
| `cli` | `src/cli.rs` | Clap CLI: `run`, `stop`, `croncheck` (TCP-connect watchdog that spawns `run` if nothing is listening), `configprint`, `host`, `port`, `version`; Windows adds SCM `start`/`restart` |
| `run` | `src/run.rs` | Foreground daemon: wiring + supervision loop; entry points `run()`, Unix `run_with_control_path()`, cross-platform `run_with_control_path_or_default()`, all delegating into the shared `run_with_shutdown()` core; `RunOutcome`, 10s graceful shutdown deadline |
| `config` | `src/config.rs` | TOML config, validation, atomic writes; `ConfigViolation`, `AtomicWriteError` |
| `control` | `src/control.rs` | Unix-domain control socket for `greggd stop`; normalized config identity (FNV-1a digest), config-adjacent primary + temp-dir fallback paths; `ControlSocketGuard` for cleanup on SIGTERM/SIGINT |
| `net` | `src/net.rs` | Local-network address resolution for `configprint`: resolves a wildcard bind host to the primary local IP via a transient UDP `connect()` (no packets sent) |
| `sampler` | `src/sampler.rs` | Periodic sampling loop, readiness lifecycle; `SamplerError`, `SyntheticClock` |
| `server/mod` | `src/server/mod.rs` | Axum HTTP server, endpoints, staleness; `ServerState`, `PublishedState`, module-local `Config` (with `ServerConfigError`) |
| `server/error` | `src/server/error.rs` | Server error types |
| `collector/mod` | `src/collector/mod.rs` | `SystemCollector` trait, `CollectedMetrics`, `into_status_payload_v2()` |
| `collector/error` | `src/collector/error.rs` | `CollectErrorKind` taxonomy (6 kinds) |
| `collector/drives` | `src/collector/drives.rs` | Shared drive normalization: `DriveCandidate`, dedup, sort, truncate to `MAX_DRIVE_ENTRIES` |
| `service/mod` | `src/service/mod.rs` | `ServiceManager` trait |
| `service/windows` | `src/service/windows.rs` | Windows: SCM integration |

## Architecture

### Supervision loop

The `run()` function in `run.rs` wires everything together:

```
┌─────────────────────────────────────────────────┐
│  run()                                          │
│                                                  │
│  ┌──────────┐  ┌─────────┐  ┌────────────────┐ │
│  │ Collector │  │ Sampler │  │  HTTP Server   │ │
│  │ (native)  │  │ (timer) │  │  (axum)        │ │
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

### HTTP endpoints

| Route | Handler | Response |
|-------|---------|----------|
| `GET /` | `status_handler` | v1 snapshot (200) or health (503) |
| `GET /v1/status` | `status_handler` | Same as `/` |
| `GET /v2/status` | `status_handler_v2` | v2 payload (200) or v2 health (503) |
| `GET /healthz` | `health_handler` | v1 health (200 if ready, 503 otherwise) |
| `GET /v2/healthz` | `health_handler_v2` | v2 health (200 if ready, 503 otherwise) |
| Other | `fallback_handler` | 404 |

**Published state:** Snapshots, health bodies, observation time, and failure
count are published under one state lock. Each handler takes one coherent
generation, so its HTTP status and JSON body cannot describe different
publications. Windows publishes v2 metrics and returns a v1 `not_serving`
health response with `503` because v1 is structurally unavailable.

**Staleness policy:** If `max_consecutive_failures > 0` and failures reach the
threshold, or if `max_snapshot_age > 0` and the latest published observation is too old, the server
returns 503, including for v2-only Windows publication. The snapshot is preserved (not cleared) for stale serving. A 503 body is always a failed health response: if staleness trips while the stored health state still says `ready`, the handlers substitute a `CollectorFailure` failure ("cached snapshot is stale"), so the body can never contradict the status code.

### Sampler

The sampler owns the clock and cadence. Key behaviors:

- First `sample()` returns `Warming` — CPU percentages require two readings
- Subsequent samples return `Ok(CollectedMetrics)` with delta-based percentages
- Produces both v1 `StatusSnapshot` and v2 `StatusPayloadV2` from one collection
- Manages readiness lifecycle: `Warming` → `Ready` (on first delta) or `Failed`
  (on collector error)
- `Clock` trait for deterministic testing with `SyntheticClock`
- The runtime loop runs each collection cycle on tokio's blocking thread pool
  (`spawn_blocking`), so slow native reads (procfs, one `statvfs()` per mount)
  cannot stall the HTTP server sharing the single current-thread runtime.
  The collector is shared with the blocking task behind a mutex; a panicked
  task poisons it, the panic is logged and reported as a source failure for
  that cycle only, and later ticks recover the lock and resume sampling.

### Configuration

```toml
name = "greggd"           # display name, max 128 chars
host = "0.0.0.0"          # bind address
port = 11310              # TCP port (1-65535)
sample_interval_ms = 1000 # 250-60000
stale_after_ms = 10000    # 0 = disabled, else > sample_interval_ms
```

`name` is the human-readable `system.name` in published snapshots. Foreground
startup and the Windows SCM worker load and validate the config before creating
their native collector, then pass that name as the collector display-name
override. `system.hostname` is collected independently from the native host
interface and is never replaced by the configured name.

Validation produces structured `ConfigViolation` values. Atomic writes use
write-flush-rename-verify.

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
| `croncheck` | Watchdog for cron and other non-systemd supervisors: bounded TCP connect to the configured local bind (wildcards normalized to loopback); exits `0` on a listener, otherwise spawns `<current_exe> run` as a detached child (stdin/stdout/stderr closed, Unix-only new process group); no service manager, shell, or PID-file management |
| `configprint` | Read configured bind address and print one canonical `host:port` line; bind wildcards (`0.0.0.0`, `::`) are resolved to the host's primary local IP so the output is a usable address, and the original wildcard is preserved if the local IP cannot be resolved; no network I/O beyond a local route lookup, no listener bind, no service, no config mutation |
| `host` | Atomically mutate bind host; applies on next start |
| `port` | Atomically mutate port; applies on next start |
| `version` | Print compile-time daemon version |

The binary boundary owns logging initialization and error presentation. The
runtime and CLI library functions return errors and never call
`std::process::exit()`. `main` installs tracing with non-panicking `try_init()`,
prints one diagnostic for failures, and applies the exit-code taxonomy: `0`
success, `1` configuration, `2` service management, `3` runtime, and `4`
permission denied.

### Unix control socket

`greggd run` on Linux/macOS binds a local Unix-domain control socket
alongside the TCP listener. The socket identity is derived from a normalized
config identity path via a deterministic 64-bit FNV-1a hex digest. Existing
config files are filesystem-canonicalized, so relative, absolute, and symlink
spellings of the same file produce the same control paths; an absent implicit
default uses a deterministic lexical absolute path without requiring the TOML
file to exist. Two different config files in the same directory still produce
different control paths (`greggd-<id>.control.sock`). Editing `host` or
`port` inside the same TOML does not change the `<id>`, so the same daemon
continues to advertise `greggd stop` at the same path. The socket file lives at
the canonical config-adjacent path when that directory is writable;
otherwise a deterministic fallback under the standard temp directory is
used. The chosen socket is created with restrictive `0600` permissions;
the inode is bound inside a process-private `0700` staging directory in
the same parent and renamed into its final location only after the mode
is applied and verified, so the socket never exists at a publicly
reachable path with umask-derived permissions. A failed `chmod` causes
the candidate to be discarded before the next
candidate is tried, and if neither candidate yields a secure listener the
foreground entry point returns a clear runtime error rather than silently
losing stop capability. The control socket is removed on orderly shutdown
(SIGTERM/SIGINT or `greggd stop`), on startup failure, and on runtime
errors. The daemon task that owns the listener uses a RAII guard to
ensure socket-file removal even if the runtime is dropped before the
task's cleanup path runs.

`greggd stop` tries the config-adjacent path first, then the temp-dir
fallback. It sends `STOP\n`, reads `OK\n`, and exits 0. Missing or
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

The Windows-only `ServiceManager` trait provides `start`, `stop`, `restart`, and
`is_active`:

- **Windows SCM** (`windows.rs`) — uses the `windows-service` dispatcher and
  generated `ServiceMain` for the daemon entry, a one-shot control signal for
  Stop/Shutdown, and an `ScmAdapter` trait for lifecycle-manager testability

The existing `windows-2022` CI job builds the release daemon and runs
`scripts/smoke-windows.ps1` as the operational SCM proof. The bounded smoke
uses an occupied ephemeral loopback port for bind-failure verification and
checks service creation, `LocalService` configuration, custom config-path
handoff, post-bind readiness, restart/recovery, reinstall, and cleanup.

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
| `IdentityFallback` | Identity field unreadable, fallback used |

These are crate-local typed errors. Wire responses carry only `HealthCategory`.

### Platform collectors

See [collectors.md](collectors.md) for detailed platform-specific analysis.

| Platform | Source | Key interfaces |
|----------|--------|---------------|
| Linux | `collector/linux/` | `/proc/stat`, `/proc/meminfo`, `/proc/self/mountinfo`, `statvfs` |
| macOS | `collector/macos/` | Mach `host_statistics`, `sysctl`, `getloadavg`, `getmntinfo` |
| Windows | `collector/windows/` | `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo` |

## Tests

### Unit tests

Every module has inline `#[cfg(test)]` tests:

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

### Test infrastructure

- 46 JSON/text fixture files in `src/collector/test_fixtures/`
- `MemorySource` (Linux) — in-memory file map for deterministic tests
- `MockNativeQueries` (macOS) — injectable FFI with auto-increment CPU
- `MockWindowsSource` (Windows) — injectable API with auto-increment CPU
- `MockScmAdapter` (Windows SCM) — injectable service state
