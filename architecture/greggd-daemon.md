# greggd daemon deep dive

The daemon crate is the metrics collection agent that runs on each monitored
host. It collects system metrics, samples them on a timer, serves them over
HTTP, and manages its own OS service lifecycle.

**Source:** `crates/greggd/`

## Purpose

- Collect CPU, memory, swap, load, and drive metrics using native OS interfaces
- Sample metrics at a configurable interval with delta-based CPU computation
- Serve cached snapshots over HTTP (v1 and v2 endpoints)
- Manage OS service lifecycle (systemd, launchd, Windows SCM)
- Expose CLI for config editing, service control, and runtime mutations

## Module map

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs:1-45` | Entry point, platform collector dispatch |
| `lib` | `src/lib.rs:1-12` | Library root, re-exports all modules |
| `cli` | `src/cli.rs:1-863` | Clap CLI: `run`, `start`, `stop`, `restart`, `croncheck`, `host`, `port` |
| `run` | `src/run.rs:1-738` | Foreground daemon: wiring + supervision loop |
| `config` | `src/config.rs:1-841` | TOML config, validation, atomic writes |
| `sampler` | `src/sampler.rs:1-867` | Periodic sampling loop, readiness lifecycle |
| `server/mod` | `src/server/mod.rs:1-425` | Axum HTTP server, endpoints, staleness |
| `server/error` | `src/server/error.rs:1-54` | Server error types |
| `collector/mod` | `src/collector/mod.rs:1-260` | `SystemCollector` trait, `CollectedMetrics` |
| `collector/error` | `src/collector/error.rs:1-95` | `CollectErrorKind` taxonomy |
| `collector/drives` | `src/collector/drives.rs:1-84` | Shared drive normalization |
| `service/mod` | `src/service/mod.rs:1-272` | `ServiceManager` trait |
| `service/systemd` | `src/service/systemd.rs:1-142` | Linux: systemctl wrapper |
| `service/launchd` | `src/service/launchd.rs:1-929` | macOS: launchctl wrapper |
| `service/windows` | `src/service/windows.rs:1-741` | Windows: SCM integration |

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

### HTTP endpoints

| Route | Handler | Response |
|-------|---------|----------|
| `GET /` | `status_handler` | v1 snapshot (200) or health (503) |
| `GET /v1/status` | `status_handler` | Same as `/` |
| `GET /v2/status` | `status_handler_v2` | v2 payload (200) or v2 health (503) |
| `GET /healthz` | `health_handler` | v1 health (200 if ready, 503 otherwise) |
| `GET /v2/healthz` | `health_handler_v2` | v2 health |
| Other | `fallback_handler` | 404 |

**Published state:** Snapshots, health bodies, observation time, and failure
count are published under one state lock. Each handler takes one coherent
generation, so its HTTP status and JSON body cannot describe different
publications. Windows publishes v2 metrics and returns a v1 `not_serving`
health response with `503` because v1 is structurally unavailable.

**Staleness policy:** If `max_consecutive_failures > 0` and failures exceed the
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

Validation produces structured `ConfigViolation` values. Atomic writes use
write-flush-rename-verify.

Platform defaults:
- Linux: `/etc/gregg/greggd.toml`
- macOS: `/Library/Application Support/gregg/greggd.toml`
- Windows: `%ProgramData%\gregg\greggd.toml`

### CLI subcommands

| Command | Purpose |
|---------|---------|
| `run` | Start foreground daemon |
| `start` | Start OS service |
| `stop` | Stop OS service |
| `restart` | Restart OS service |
| `croncheck` | Check service status (scripting) |
| `host` | Mutate bind host and restart |
| `port` | Mutate port and restart |

### Service management

The `ServiceManager` trait provides `start`, `stop`, `restart`, `is_active`.
Platform adapters:

- **systemd** (`systemd.rs`) — wraps `systemctl` with fixed argument arrays
- **launchd** (`launchd.rs`) — state machine: NotLoaded → bootstrap, Loaded →
  kickstart, Running → no-op
- **Windows SCM** (`windows.rs`) — wraps `windows-service` crate with
  `ScmAdapter` trait for testability

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
| `service/launchd.rs` | 517 | State machine via FakeRunner |
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
- `FakeRunner` (macOS launchd) — scripted launchctl responses
- `MockScmAdapter` (Windows SCM) — injectable service state
