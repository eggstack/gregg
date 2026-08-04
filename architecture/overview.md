# Architecture overview

This document is the bird's-eye view of the entire `gregg` codebase: what each
piece does, who owns it, how they connect, and where to go for details. It also
serves as an index to the deep-dive documents in this directory.

## System at a glance

`gregg` is a cross-platform system metrics collection and monitoring tool
composed of three Rust crates in a single Cargo workspace.

```
┌─────────────────────────────────────────────────────────────────────┐
│                         gregg (client)                              │
│  CLI endpoint management + HTTP polling + state reducer + TUI       │
│  Optional EggPool summary pane                                      │
│  Platforms: Linux, macOS, Windows                                   │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ HTTP (JSON)
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        greggd (daemon)                              │
│  Native metric collection + sampler + HTTP server + OS service mgmt │
│  Platforms: Linux, macOS, Windows                                   │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ uses wire types from
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│                   gregg-protocol (library)                          │
│  Shared wire types, schema versions, validation, health responses  │
│  No runtime, HTTP, terminal, or platform dependencies               │
└─────────────────────────────────────────────────────────────────────┘
```

**Dependency direction is strictly one-way:**

```
gregg-protocol  ◄── greggd
gregg-protocol  ◄── gregg
```

`greggd` and `gregg` never depend on each other. `gregg-protocol` never
depends on either application crate.

---

## Crate ownership

| Crate | Path | Type | Role |
|-------|------|------|------|
| `gregg-protocol` | `crates/gregg-protocol/` | Library | Wire contract between daemon and client |
| `greggd` | `crates/greggd/` | Bin + lib | Metrics daemon, collector, HTTP server, service manager |
| `gregg` | `crates/gregg/` | Binary | Client TUI, endpoint CLI, polling, EggPool |

---

## gregg-protocol

**Purpose:** Defines the JSON wire contract. Pure data types with serde
serialization and structured validation. No I/O, no runtime dependencies
beyond serialization.

### Modules

| Module | File | Purpose |
|--------|------|---------|
| `lib` | `src/lib.rs` | Root, re-exports, schema version constants |
| `snapshot` | `src/snapshot.rs` | V1 wire types: `StatusSnapshot`, `CpuMetrics`, `LoadAverage`, `MemoryMetrics`, `SwapMetrics`, `SystemIdentity`, `MetricCapabilities` |
| `v2` | `src/v2.rs` | V2 wire types: `StatusSnapshotV2`, `StatusPayloadV2`, `MetricCapabilitiesV2`, `DriveMetrics`, `CommitMetrics`, `HealthResponseV2` |
| `validate` | `src/validate.rs` | V1 validation: 6 violation kinds |
| `validate_v2` | `src/validate_v2.rs` | V2 validation: 11 violation kinds, capability/value consistency |
| `health` | `src/health.rs` | V1 health types: `HealthResponse`, `ReadinessState`, `HealthCategory` |
| `test_support` | `src/test_support.rs` | Feature-gated builder fixtures for tests |

### Key concepts

- **Schema v1** — original Linux/macOS format with required load/swap
- **Schema v2** — extended with capability flags for load, swap, commit; drives array
- **Capability flags** — each platform declares what metrics it supports; the
  client uses these to decide what to render
- **Validation** — separate from serde; `validate()` returns structured violation lists

**Deep dive:** [gregg-protocol.md](gregg-protocol.md)

---

## greggd (daemon)

**Purpose:** Runs on the monitored host. Collects system metrics using native OS
interfaces, samples them at a configurable interval, serves them over HTTP, and
manages its own OS service lifecycle.

### Modules

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Entry point, platform collector dispatch |
| `lib` | `src/lib.rs` | Library root, re-exports all modules |
| `run` | `src/run.rs` | Supervision loop: wires collector, sampler, server, signals |
| `cli` | `src/cli.rs` | Clap CLI: `run`, `start`, `stop`, `restart`, `croncheck`, `host`, `port` |
| `config` | `src/config.rs` | TOML config, validation, atomic writes |
| `sampler` | `src/sampler.rs` | Periodic sampling loop, readiness lifecycle, clock abstraction |
| `server/mod` | `src/server/mod.rs` | Axum HTTP server, five endpoints, staleness detection |
| `server/error` | `src/server/error.rs` | Server error types |
| `collector/mod` | `src/collector/mod.rs` | `SystemCollector` trait, `CollectedMetrics` normalization |
| `collector/error` | `src/collector/error.rs` | `CollectErrorKind` taxonomy (6 kinds) |
| `collector/drives` | `src/collector/drives.rs` | Shared drive normalization (dedup, sort, truncate) |
| `collector/linux/` | `src/collector/linux/` | Linux collector: `/proc/stat`, `/proc/meminfo`, `/proc/self/mountinfo`, `statvfs` |
| `collector/macos/` | `src/collector/macos/` | macOS collector: Mach FFI, sysctl, `getloadavg`, `getmntinfo` |
| `collector/windows/` | `src/collector/windows/` | Windows collector: `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo` |
| `service/mod` | `src/service/mod.rs` | `ServiceManager` trait: `start`, `stop`, `restart`, `is_active` |
| `service/systemd` | `src/service/systemd.rs` | Linux: `systemctl` wrapper |
| `service/launchd` | `src/service/launchd.rs` | macOS: `launchctl` wrapper with state machine |
| `service/windows` | `src/service/windows.rs` | Windows: SCM integration via `windows-service` crate |

### Key concepts

- **Collector** — platform-specific metric collection. No external commands for metrics.
- **Sampler** — owns the clock and cadence; calls the collector periodically,
  stamps timestamps, produces immutable cached snapshots
- **HTTP server** — serves cached snapshots (never triggers collection); staleness
  detection; v1 + v2 endpoints
- **Service manager** — wraps systemd/launchd/Windows SCM for start/stop/restart

**Deep dive:** [greggd-daemon.md](greggd-daemon.md)

---

## gregg (client)

**Purpose:** Monitors one or more `greggd` instances from a terminal UI. Manages
endpoints via CLI, polls them over HTTP, and renders a Ratatui-based TUI with
normal and condensed fleet views. Optionally displays EggPool summary data.

### Modules

#### Core

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Entry point, event loop, TUI wiring |
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `eggpool` |
| `config` | `src/config.rs` | Config model, validation, atomic I/O, cross-process locking |
| `state` | `src/state.rs` | `AppState` reducer, viewport logic |
| `action` | `src/action.rs` | `Action` enum (14 state transition triggers) |

#### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, `PollOutcome` classification (12 failure modes) |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, generation-based concurrency |
| `endpoint` | `src/endpoint.rs` | Endpoint parsing: IPv4, IPv6, DNS/mDNS |
| `clock` | `src/clock.rs` | Clock trait for deterministic testing |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI consumption |

#### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs` | Key-to-action translation (Vim-style) |
| `input` | `src/input.rs` | Crossterm event stream adapter |
| `terminal` | `src/terminal.rs` | Terminal lifecycle (raw mode, alt screen, panic hook) |

#### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs` | Render dispatcher |
| `ui/layout` | `src/ui/layout.rs` | Viewport computation (which systems are visible) |
| `ui/system_block` | `src/ui/system_block.rs` | Normal-view system rendering (5-row blocks) |
| `ui/condensed` | `src/ui/condensed.rs` | Condensed one-row fleet view |
| `ui/bar` | `src/ui/bar.rs` | Reusable ASCII usage bar widget |
| `ui/text` | `src/ui/text.rs` | Text formatting (bytes, percentages, load averages) |
| `ui/diagnostics` | `src/ui/diagnostics.rs` | Empty-config, terminal-too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering |

#### EggPool

| Module | File | Purpose |
|--------|------|---------|
| `eggpool` | `src/eggpool.rs` | EggPool summary client and background worker |
| `eggpool_endpoint` | `src/eggpool_endpoint.rs` | EggPool-specific endpoint parsing |

#### Test modules

| Module | File | Purpose |
|--------|------|---------|
| `mixed_fleet_evidence` | `src/mixed_fleet_evidence.rs` | Integration test with Python fixture servers |
| `sustained_workload` | `src/sustained_workload.rs` | Long-running regression test (`#[ignore]`) |

### Key concepts

- **Poll scheduler** — generation-based concurrency; v2-first/v1-fallback protocol
- **State reducer** — action/Reducer pattern; all state changes through `Action` enum
- **Normalized snapshots** — v1 and v2 wire formats normalized to a single internal type
- **EggPool** — optional summary pane for EggPool API metrics (separate from greggd polling)
- **Cross-process config locking** — `flock(2)` / `LockFileEx` prevents concurrent corruption

**Deep dive:** [gregg-client.md](gregg-client.md)

---

## Data flow

### Primary: greggd → gregg polling

```
┌──────────────────────────────────────────────────────────────────────┐
│  greggd on monitored host                                           │
│                                                                      │
│  ┌──────────┐    ┌─────────┐    ┌──────────────┐    ┌────────────┐ │
│  │ Collector │───▶│ Sampler │───▶│ Cached Snap. │◀───│ HTTP Server│ │
│  │ (native)  │    │ (clock) │    │ (v1 + v2)    │    │ (axum)     │ │
│  └──────────┘    └─────────┘    └──────────────┘    └─────┬──────┘ │
│                                                            │        │
└────────────────────────────────────────────────────────────┼────────┘
                                                             │ JSON
                                                             ▼
┌──────────────────────────────────────────────────────────────────────┐
│  gregg on user's terminal                                           │
│                                                                      │
│  ┌──────────┐    ┌───────────┐    ┌──────────┐    ┌──────────────┐ │
│  │ Scheduler │───▶│ PollBatch │───▶│ AppState │───▶│     TUI      │ │
│  │ (timer)   │    │ channel   │    │ reducer  │    │  (ratatui)   │ │
│  └──────────┘    └───────────┘    └──────────┘    └──────────────┘ │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

1. The **collector** reads native OS interfaces (procfs, Mach, Win32 API)
2. The **sampler** calls the collector on a timer, stamps timestamps, produces
   immutable v1 and v2 status snapshots
3. The **HTTP server** caches snapshots and serves them on request
4. The **client scheduler** polls each endpoint on a configurable interval
5. **PollBatches** arrive on a channel with a generation counter
6. The **state reducer** applies batches, rejects stale generations, updates
   reachability and selection
7. The **TUI** reads `AppState` projections and renders without I/O

### Optional: EggPool

```
┌──────────────────┐    HTTP (JSON)    ┌──────────────────┐
│  EggPool worker  │──────────────────▶│  EggPool API     │
│  (gregg client)  │                   │  (external)      │
└────────┬─────────┘                   └──────────────────┘
         │ apply result
         ▼
┌──────────────────┐
│    AppState      │
│    (eggpool pane)│
└──────────────────┘
```

The EggPool path is deliberately separate from greggd polling. It has its own
client, worker, authentication (Bearer token from env var), and rendering.

---

## Cross-cutting concerns

### Platform collectors

Each platform collector implements the `SystemCollector` trait and reads only
native kernel interfaces. CPU percentages require two samples (delta-based).
macOS has no I/O-wait equivalent. Windows cannot produce v1 snapshots (no
load/swap). No external commands are executed for metric collection.

| Platform | Source | Key interfaces |
|----------|--------|----------------|
| Linux | `collector/linux/` | `/proc/stat`, `/proc/meminfo`, `/proc/self/mountinfo`, `statvfs` |
| macOS | `collector/macos/` | Mach `host_statistics`, `sysctl`, `getloadavg`, `getmntinfo` |
| Windows | `collector/windows/` | `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo` |

**Deep dive:** [collectors.md](collectors.md)

### Wire protocol and validation

The protocol supports two schema versions. V2 is preferred; the client falls
back to v1 on 404. Capability flags control which optional fields must be
present. Validation is structured and separate from deserialization.

**Deep dive:** [protocol.md](protocol.md)

### Error boundaries

Each binary crate has crate-local typed errors via `thiserror`. Wire responses
carry only safe, structured info (category + message). Collector errors never
appear on the wire.

**Deep dive:** [error-conventions.md](error-conventions.md)

### Scripts and packaging

Installer scripts for all three platforms, a local validation script
(`check-local.sh`), loopback smoke tests, and systemd/launchd/SCM service
definitions.

**Deep dive:** [scripts-and-packaging.md](scripts-and-packaging.md)

---

## Configuration

| Component | Format | Default path (Linux) | Default path (macOS) | Default path (Windows) |
|-----------|--------|---------------------|---------------------|----------------------|
| greggd | TOML | `/etc/gregg/greggd.toml` | `/Library/Application Support/gregg/greggd.toml` | `%ProgramData%\gregg\greggd.toml` |
| gregg | TOML | `$XDG_CONFIG_HOME/gregg/gregg.toml` | `~/Library/Application Support/gregg/gregg.toml` | `%APPDATA%\gregg\gregg.toml` |

Both use atomic writes (write-flush-rename-verify) and structured validation.
The daemon config has 5 fields; the client config stores system endpoints,
refresh intervals, and optional EggPool settings.

### Cross-process config locking

- Unix: `flock(2)` advisory lock on `<config>.lock`
- Windows: `LockFileEx` exclusive lock on `<config>.lock`
- Other platforms: in-process `Mutex` only

---

## Testing strategy

- **Unit tests** in every module with deterministic fixtures and mock collectors
- **Integration tests** in `tests/` directories for live smoke tests
- **TUI buffer tests** cover width degradation, mixed fleets, and resize
- **Sustained workload test** (`#[ignore]`) exercises the full polling loop
- **Platform-native collector tests** run only on the target OS
- **40+ JSON/text fixture files** in `src/collector/test_fixtures/`
- **Mock seams:** `MemorySource` (Linux), `MockNativeQueries` (macOS), `MockWindowsSource` (Windows)

Run all checks with:

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

---

## Index of architecture documents

### Overview and deep dives

| Document | Scope |
|----------|-------|
| [overview.md](overview.md) | This file — bird's-eye view and component index |
| [gregg-protocol.md](gregg-protocol.md) | Protocol crate: wire types, schema versions, validation, test support |
| [greggd-daemon.md](greggd-daemon.md) | Daemon crate: collectors, sampler, HTTP server, service management |
| [gregg-client.md](gregg-client.md) | Client crate: CLI, polling, state engine, TUI, EggPool |
| [collectors.md](collectors.md) | Platform collectors: Linux, macOS, Windows native metric collection |
| [scripts-and-packaging.md](scripts-and-packaging.md) | Scripts, installers, service definitions, CI |

### Cross-cutting decisions

| Document | Scope |
|----------|-------|
| [workspace.md](workspace.md) | Cargo workspace layout, crate boundaries, dependency direction, module structure |
| [protocol.md](protocol.md) | Wire format specification, schema versions, capabilities, validation, compatibility |
| [error-conventions.md](error-conventions.md) | Error boundary design, wire response constraints |
| [macos-collector-notes.md](macos-collector-notes.md) | Expected differences between macOS collector and Activity Monitor / `top` / `vm_stat` |

### Supporting files

| Document | Scope |
|----------|-------|
| [README.md](README.md) | Directory index and purpose |
| [`../plans/`](../plans/) | Phase plans — source of truth for sequencing and acceptance criteria |
| [`../AGENTS.md`](../AGENTS.md) | Compact agent instructions for this repository |
