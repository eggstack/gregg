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
| `cli` | `src/cli.rs` | Clap CLI: `run`, `croncheck`, `configprint`, `host`, `port`, `version`; Windows adds SCM lifecycle commands |
| `run` | `src/run.rs` | Foreground daemon: wiring + supervision loop |
| `config` | `src/config.rs` | TOML config, validation, atomic writes |
| `sampler` | `src/sampler.rs` | Periodic sampling loop, readiness lifecycle |
| `server/mod` | `src/server/mod.rs` | Axum HTTP server, endpoints, staleness |
| `server/error` | `src/server/error.rs` | Server error types |
| `collector/mod` | `src/collector/mod.rs` | `SystemCollector` trait, `CollectedMetrics` |
| `collector/error` | `src/collector/error.rs` | `CollectErrorKind` taxonomy |
| `collector/drives` | `src/collector/drives.rs` | Shared drive normalization with independent total-free and caller-available capacity |
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
returns 503, including for v2-only Windows publication. The snapshot is preserved (not cleared) for stale serving.

### Sampler

The sampler owns the clock and cadence. Key behaviors:

- First `sample()` returns `Warming` — CPU percentages require two readings
- Subsequent samples return `Ok(CollectedMetrics)` with delta-based percentages
- Produces both v1 `StatusSnapshot` and v2 `StatusPayloadV2` from one collection
- Manages readiness lifecycle: `Warming` → `Ready` (on first delta) or `Failed`
  (on collector error)
- `Clock` trait for deterministic testing with `FakeClock`

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
| `croncheck` | Bounded HTTP probe of `/v2/healthz`; fixed 512-byte CRLF-terminated HTTP/1.x status-line read, accepting only HTTP/1.0/1.1 status 200 |
| `configprint` | Read configured bind address and print one canonical `host:port` line; no network, service, or write side effects |
| `host` | Atomically mutate bind host; applies on next start |
| `port` | Atomically mutate port; applies on next start |
| `version` | Print compile-time daemon version |

The binary boundary owns logging initialization and error presentation. The
runtime and CLI library functions return errors and never call
`std::process::exit()`. `main` installs tracing with non-panicking `try_init()`,
prints one diagnostic for failures, and applies the exit-code taxonomy: `0`
success, `1` configuration, `2` service management, `3` runtime, and `4`
permission denied.

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
pub trait SystemCollector {
    fn identity(&self) -> SystemIdentity;
    fn sample(&mut self) -> Result<CollectedMetrics, CollectError>;
    fn capabilities(&self) -> MetricCapabilities;
    fn capabilities_v2(&self) -> MetricCapabilitiesV2;
    fn supports_v1_snapshot(&self) -> bool;
}
```

One call to `sample()` produces `CollectedMetrics` which converts to both v1
and v2 wire formats without duplicate collection.

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

| Module | ~Lines | Coverage |
|--------|--------|----------|
| `cli.rs` | 590 | CLI parsing, config resolution, exit codes, mutations |
| `run.rs` | 290 | Supervision select, task joining, non-cooperative abort |
| `config.rs` | 410 | Validation, atomic writes, TOML round-trips |
| `sampler.rs` | 550 | Interval validation, readiness lifecycle, counter reset |
| `server/tests.rs` | 954 | All handlers, staleness, concurrency (50 parallel) |
| `service/windows.rs` | 316 | MockScmAdapter for all states |
| `collector/linux/tests.rs` | 952 | 40+ tests with fixture-driven invariants |
| `collector/macos/tests.rs` | 586 | Mock-based + native smoke tests |
| `collector/windows/mod.rs` | 286 | Topology guards, structural invariants |

### Integration tests

- `tests/linux_collector.rs` — live `/proc` smoke test
- `tests/windows_smoke.rs` — binary help + foreground daemon + v2 health polling

### Test infrastructure

- 40+ JSON/text fixture files in `src/collector/test_fixtures/`
- `MemorySource` (Linux) — in-memory file map for deterministic tests
- `MockNativeQueries` (macOS) — injectable FFI with auto-increment CPU
- `MockWindowsSource` (Windows) — injectable API with auto-increment CPU
- `MockScmAdapter` (Windows SCM) — injectable service state
