# Architecture overview

This document is the bird's-eye view of the entire `gregg` codebase: what each
piece does, who owns it, how they connect, and where to go for details. It also
serves as an index to the deep-dive documents in this directory.

## Reading this document

If you are new to the codebase, read this overview first, then follow the deep
dive links in the order that matches your task:

1. **[workspace.md](workspace.md)** — crate boundaries, dependency rules, MSRV,
   lint policy, and release profiles. Read this before changing any crate
   structure.
2. **[gregg-protocol.md](gregg-protocol.md)** — the wire contract. Read this
   before touching any shared types or adding fields.
3. **[greggd-daemon.md](greggd-daemon.md)** — the daemon internals. Read this
   before modifying collectors, the sampler, HTTP server, or service management.
4. **[gregg-client.md](gregg-client.md)** — the client internals. Read this
   before modifying the TUI, polling, state engine, or EggPool.
5. **[collectors.md](collectors.md)** — platform-specific metric collection.
   Read this before modifying Linux, macOS, or Windows collector code.
6. **[protocol.md](protocol.md)** — wire format specification, schema versions,
   capabilities, and compatibility policy. Read this before changing validation
   rules or adding schema versions.
7. **[error-conventions.md](error-conventions.md)** — error boundary design
   and wire response constraints. Read this before adding new error types or
   changing what appears on the wire.

---

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
depends on either application crate. This constraint is enforced by the
workspace Cargo manifests and must not be violated.

---

## Crate ownership

| Crate | Path | Type | Role | Deep dive |
|-------|------|------|------|-----------|
| `gregg-protocol` | `crates/gregg-protocol/` | Library | Wire contract between daemon and client | [gregg-protocol.md](gregg-protocol.md) |
| `greggd` | `crates/greggd/` | Bin + lib | Metrics daemon, collector, HTTP server, service manager | [greggd-daemon.md](greggd-daemon.md) |
| `gregg` | `crates/gregg/` | Binary | Client TUI, endpoint CLI, polling, EggPool | [gregg-client.md](gregg-client.md) |

---

## gregg-protocol

**Purpose:** Defines the JSON wire contract. Pure data types with serde
serialization and structured validation. No I/O, no runtime dependencies
beyond serialization.

**Dependencies:** `serde`, `serde_json`, `thiserror` only. No HTTP, terminal,
or platform crate enters this boundary. `#![forbid(unsafe_code)]`.

### Modules

| Module | File | Purpose |
|--------|------|---------|
| `lib` | `src/lib.rs` | Root, re-exports, schema version constant (`SCHEMA_VERSION_V1`), `MAX_SAMPLE_INTERVAL_MS` |
| `snapshot` | `src/snapshot.rs` | V1 wire types: `StatusSnapshot`, `CpuMetrics`, `LoadAverage`, `MemoryMetrics`, `SwapMetrics`, `SystemIdentity`, `MetricCapabilities` |
| `v2` | `src/v2.rs` | V2 wire types: `StatusSnapshotV2`, `StatusPayloadV2`, `MetricCapabilitiesV2`, `DriveMetrics`, `CommitMetrics`, `HealthResponseV2`; constants `SCHEMA_VERSION_V2`, `MAX_DRIVE_ENTRIES`, `MAX_DRIVE_NAME_BYTES` |
| `validate` | `src/validate.rs` | V1 validation: `validate()` returns `Result<(), Vec<ValidationViolation>>` with 8 violation kinds |
| `validate_v2` | `src/validate_v2.rs` | V2 validation: `validate_v2()` and `validate_payload_v2()` with 15 violation kinds, capability/value consistency |
| `health` | `src/health.rs` | V1 health types: `HealthResponse`, `ReadinessState`, `HealthCategory` |
| `test_support` | `src/test_support.rs` | Feature-gated builder fixtures: `LinuxSnapshotBuilder`, `MacosSnapshotBuilder`, `LinuxSnapshotV2Builder`, `WindowsSnapshotV2Builder`, `IdentityFixture` |

### Key concepts

- **Schema v1** — original Linux/macOS format with required load/swap
- **Schema v2** — extended with capability flags for load, swap, commit; drives array
- **Capability flags** — each platform declares what metrics it supports; the
  client uses these to decide what to render
- **Validation** — separate from serde; `validate()` returns structured violation lists
- **Health responses** — three states (`Ready`, `Warming`, `Failed`) with coarse
  categories for the wire; internal error chains never leak

**Deep dive:** [gregg-protocol.md](gregg-protocol.md)

---

## greggd (daemon)

**Purpose:** Runs on the monitored host. Collects system metrics using native OS
interfaces, samples them at a configurable interval, serves them over HTTP, and
manages its own OS service lifecycle. Both a binary (`src/main.rs`) and a
library (`src/lib.rs`) target; the lib surface exposes the collector for
integration tests.

### Modules

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Binary boundary: CLI parsing, logging, error reporting, exit-code classification, platform collector dispatch |
| `lib` | `src/lib.rs` | Library root, re-exports all modules: `cli`, `collector`, `config`, `control` (Unix), `run`, `sampler`, `server`, `service` (Windows) |
| `run` | `src/run.rs` | Supervision loop: wires collector, sampler, server, signals, local Unix control socket; `RunOutcome` enum, `run_with_shutdown_on_ready()` callback seam, 10s graceful shutdown deadline |
| `cli` | `src/cli.rs` | Clap CLI: `run`, `stop`, `croncheck` (TCP-connect watchdog that spawns `run` if nothing is listening), `configprint`, `host`, `port`, `version`; Windows adds SCM `start`/`restart`/`service`; `ExitCode` taxonomy (0-4) |
| `config` | `src/config.rs` | TOML config, validation, atomic writes; `ConfigViolation`, `AtomicWriteError` |
| `control` | `src/control.rs` | Unix-domain control socket for `greggd stop`; normalized config identity (FNV-1a digest), config-adjacent primary + temp-dir fallback paths; `ControlSocketGuard` for cleanup on SIGTERM/SIGINT |
| `sampler` | `src/sampler.rs` | Periodic sampling loop, readiness lifecycle (`Warming` → `Ready`/`Failed`), clock abstraction; `SamplerError`, `SyntheticClock` |
| `server/mod` | `src/server/mod.rs` | Axum HTTP server, five endpoints, staleness detection; `ServerState`, `PublishedState`, `ServerConfig` |
| `server/error` | `src/server/error.rs` | Server error types |
| `collector/mod` | `src/collector/mod.rs` | `SystemCollector` trait, `CollectedMetrics` normalization to v1 and v2 wire formats |
| `collector/error` | `src/collector/error.rs` | `CollectErrorKind` taxonomy (6 kinds) |
| `collector/drives` | `src/collector/drives.rs` | Shared drive normalization: `DriveCandidate`, dedup, sort, truncate to `MAX_DRIVE_ENTRIES` |
| `collector/linux/` | `src/collector/linux/` | Linux collector: `LinuxCollector`, `FileSource` trait, `ProcSource` (prod), `MemorySource` (test) |
| `collector/macos/` | `src/collector/macos/` | macOS collector: `MacOsCollector`, `MacNativeQueries` trait, `FfiNativeQueries` (prod), `MockNativeQueries` (test) |
| `collector/windows/` | `src/collector/windows/` | Windows collector: `WindowsCollector`, `WindowsSource` trait, `NativeWindowsSource` (prod), `MockWindowsSource` (test) |
| `service/mod` | `src/service/mod.rs` | `ServiceManager` trait (Windows-only) |
| `service/windows` | `src/service/windows.rs` | Windows SCM integration via `windows-service` crate; `ScmAdapter` trait for testability |

### Key concepts

- **Collector** — platform-specific metric collection. No external commands.
  Implements `SystemCollector` trait: `identity()`, `sample()`, `capabilities()`,
  `capabilities_v2()`, `supports_v1_snapshot()`.
- **Sampler** — owns the clock and cadence; calls the collector periodically,
  stamps timestamps, produces immutable cached snapshots. First sample returns
  `Warming` (CPU percentages require two readings).
- **HTTP server** — serves cached snapshots (never triggers collection); staleness
  detection; v1 + v2 endpoints. Five routes: `/`, `/v1/status`, `/v2/status`,
  `/healthz`, `/v2/healthz`.
- **Supervision** — `tokio::select!` on shutdown signal, server task, and sampler
  task. Graceful shutdown with 10-second deadline.
- **Service manager** — Windows SCM only; Unix supervisors remain external packaging.
- **Exit codes** — `0` success, `1` configuration, `2` service management,
  `3` runtime, `4` permission denied.
- **Binary/library split** — reusable runtime code returns errors without
  printing or calling `std::process::exit()`; the binary boundary owns
  logging, diagnostics, and exit-code classification.

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
| `main` | `src/main.rs` | Entry point, event loop (`tokio::select!` biased), TUI wiring, `run_tui()` async function |
| `cli` | `src/cli.rs` | Clap CLI: `add`, `list`, `remove`, `refresh`, `edit`, `eggpool`; `add` also accepts HTTP URL input; `ExitCode` taxonomy |
| `config` | `src/config.rs` | Config model, validation, atomic I/O, cross-process locking; `ConfigStore` with `load_or_default`, `load_existing`, `write`, `mutate` |
| `state` | `src/state.rs` | `AppState` reducer, viewport logic, display order, pane/view-mode state; 1572 lines |
| `action` | `src/action.rs` | `Action` enum (13 variants: `MoveDown`, `MoveUp`, `PageDown`, `PageUp`, `SelectFirst`, `SelectLast`, `PreviousPane`, `NextPane`, `ToggleSystemView`, `ToggleDrives`, `RefreshNow`, `Resize`, `Quit`) |

#### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback, `PollOutcome` classification (12 variants: 2 success, 10 failure); `PollBatch` with generation counter; 64 KiB body cap |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler, generation-based concurrency; `SchedulerCommand` enum (`Refresh`, `ReplaceEndpoints`); semaphore-bounded per-endpoint tasks |
| `endpoint` | `src/endpoint.rs` | Endpoint parsing: IPv4, IPv6 (bracketed/bare), DNS/mDNS; HTTP URL convenience adapter for `add` |
| `clock` | `src/clock.rs` | Clock trait for deterministic testing; `RealClock` and `FakeClock` implementations |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI consumption; `NormalizedSnapshot` with capability flags; `aggregate_drives()` with checked arithmetic |

#### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs` | Key-to-action translation (Vim-style); 18 test cases |
| `input` | `src/input.rs` | Crossterm event stream adapter; dedicated thread, bounded channel |
| `terminal` | `src/terminal.rs` | Terminal lifecycle (raw mode, alt screen, cursor hiding, panic hook) |

#### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs` | Render dispatcher; 1486 lines; dispatches on `active_pane` and `system_view_mode` |
| `ui/layout` | `src/ui/layout.rs` | Viewport computation (which systems are visible, rect positions) |
| `ui/system_block` | `src/ui/system_block.rs` | Normal-view system rendering (5-row blocks: header + CPU/MEM/SWP-or-COMMIT/DISK bars) |
| `ui/condensed` | `src/ui/condensed.rs` | Condensed one-row fleet view (Wide ≥ 64, Medium 48-63, Narrow 30-47, Minimal < 30 cols) |
| `ui/bar` | `src/ui/bar.rs` | Reusable ASCII usage bar widget with width-safe arithmetic |
| `ui/text` | `src/ui/text.rs` | Text formatting (bytes, percentages, load averages, priority-aware header composition) |
| `ui/diagnostics` | `src/ui/diagnostics.rs` | Empty-config and terminal-too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering; pending/success/stale/error states |

#### EggPool

| Module | File | Purpose |
|--------|------|---------|
| `eggpool` | `src/eggpool.rs` | EggPool summary client (`EggpoolClient`) and background worker (`spawn_worker`); 900 lines; 60s passive refresh cadence |
| `eggpool_endpoint` | `src/eggpool_endpoint.rs` | EggPool-specific endpoint parsing; defaults to HTTP port 11300 |

#### Test modules

| Module | File | Purpose |
|--------|------|---------|
| `mixed_fleet_evidence` | `src/mixed_fleet_evidence.rs` | Integration test with Python fixture servers; 9 fixture modes + refused endpoint |
| `sustained_workload` | `src/sustained_workload.rs` | Long-running regression test (`#[ignore]`); validates generation invariants, bounded concurrency |

### Key concepts

- **Poll scheduler** — generation-based concurrency; v2-first/v1-fallback protocol.
  One isolated poll task per endpoint; semaphore bounds active polls; task panic
  is converted to `Cancelled`.
- **State reducer** — action/Reducer pattern; all state changes through `Action`
  enum. `AppState::apply_action()` and `apply_batch()` are pure, deterministic
  functions.
- **Normalized snapshots** — v1 and v2 wire formats normalized to a single
  internal type; eliminates version-branching in the UI.
- **EggPool** — optional summary pane for EggPool API metrics (separate from
  greggd polling). Own client, worker, authentication (Bearer token from env var),
  and rendering. 60-second passive refresh cadence.
- **Cross-process config locking** — `flock(2)` / `LockFileEx` prevents
  concurrent corruption of the TOML config file.
- **Width degradation** — header line drops lower-priority segments as width
  decreases (< 32: no load, < 50: no OS, < 80: no arch).

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
The worker runs a 60-second passive refresh cadence when the EggPool pane is
active, uses generation-based staleness, and aborts in-flight requests on
superseding commands. Period cycling (Hour/Day/Week/Month) is pane-local.

---

## Cross-cutting concerns

### Platform collectors

Each platform collector implements the `SystemCollector` trait and reads only
native kernel interfaces. CPU percentages require two samples (delta-based).
macOS has no I/O-wait equivalent. Windows cannot produce v1 snapshots (no
load/swap). No external commands are executed for metric collection.

| Platform | Source | Key interfaces | FFI seam |
|----------|--------|----------------|----------|
| Linux | `collector/linux/` | `/proc/stat`, `/proc/meminfo`, `/proc/self/mountinfo`, `statvfs` | `FileSource` trait (`ProcSource` prod, `MemorySource` test) |
| macOS | `collector/macos/` | Mach `host_statistics`, `sysctl`, `getloadavg`, `getmntinfo` | `MacNativeQueries` trait (`FfiNativeQueries` prod, `MockNativeQueries` test) |
| Windows | `collector/windows/` | `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo` | `WindowsSource` trait (`NativeWindowsSource` prod, `MockWindowsSource` test) |

**Deep dive:** [collectors.md](collectors.md)

### Wire protocol and validation

The protocol supports two schema versions. V2 is preferred; the client falls
back to v1 on 404. Capability flags control which optional fields must be
present. Validation is structured and separate from deserialization.

| Concept | Details |
|---------|---------|
| Schema v1 | Original Linux/macOS format; required load/swap; 8 validation violation kinds |
| Schema v2 | Extended with capability flags; optional load/swap/commit; drives array; 15 validation violation kinds |
| Validation | Structured violation lists (`Vec<ValidationViolation>`), not serde errors |
| Compatibility | Additive within schema; breaking changes require new major |
| Health responses | Three states (`Ready`, `Warming`, `Failed`) with coarse categories |

**Deep dive:** [protocol.md](protocol.md)

### Error boundaries

Each binary crate has crate-local typed errors via `thiserror`. Wire responses
carry only safe, structured info (category + message). Collector errors never
appear on the wire.

| Boundary | Pattern |
|----------|---------|
| Daemon runtime | Returns typed errors; binary boundary formats diagnostics; exit codes: 0=success, 1=config, 2=service, 3=runtime, 4=permission |
| Wire responses | `HealthCategory` + short message; no paths or error chains |
| Collector | 6 `CollectErrorKind` variants (`Warming`, `SourceUnavailable`, `Parse`, `CounterReset`, `Numeric`, `IdentityFallback`); crate-local, never on wire |
| Client | `PollOutcome` with 12 outcome classifications (2 success: `Online`/`OnlineV2`, 10 failure) |

**Deep dive:** [error-conventions.md](error-conventions.md)

### Scripts and packaging

Installer scripts for all three platforms, a local validation script
(`check-local.sh`), loopback smoke tests, and systemd/launchd/SCM service
definitions.

| Artifact | Purpose |
|----------|---------|
| `check-local.sh` / `check-local.ps1` | Primary local validation (fmt + test; `--release` adds clippy/docs/smoke) |
| `verify-installed-daemon.sh` | Bounded loopback smoke: isolated port, temp config, health poll, SIGTERM |
| `smoke-windows.ps1` | Full Windows SCM lifecycle: install → start → health → stop → restart → cleanup |
| `install-linux.sh` | Systemd service, `greggd` user, `/usr/local/bin`, `/etc/gregg/` |
| `install-macos.sh` | Launchd plist, `/usr/local/bin`, `/Library/Application Support/gregg/` |
| `install-windows.ps1` | SCM service, `%ProgramFiles%\Gregg\%`, `LocalService` account |
| `systemd/greggd.service` | Hardened systemd unit (NoNewPrivileges, ProtectSystem, etc.) |
| `launchd/com.eggstack.greggd.plist` | KeepAlive on crash, RunAtLoad, 1024 fd limit |

**Deep dive:** [scripts-and-packaging.md](scripts-and-packaging.md)

### macOS collector differences

The macOS collector uses availability-oriented memory accounting (matching
Linux `free` semantics) which reports **less** used memory than Activity
Monitor. I/O-wait is `null` (no aggregate equivalent). Compressed pages
are counted as swap. Detailed comparison with Activity Monitor, `top`, and
`vm_stat` is documented separately.

**Deep dive:** [macos-collector-notes.md](macos-collector-notes.md)

---

## Configuration

| Component | Format | Default path (Linux) | Default path (macOS) | Default path (Windows) |
|-----------|--------|---------------------|---------------------|----------------------|
| greggd | TOML | `/etc/gregg/greggd.toml` | `/Library/Application Support/gregg/greggd.toml` | `%ProgramData%\gregg\greggd.toml` |
| gregg | TOML | `$XDG_CONFIG_HOME/gregg/gregg.toml` | `~/Library/Application Support/gregg/gregg.toml` | `%APPDATA%\gregg\gregg.toml` |

Both use atomic writes (write-flush-rename-verify) and structured validation.
The daemon config has 5 fields; the client config stores system endpoints,
refresh intervals, and optional EggPool settings.

The daemon's configured `name` is published as `system.name`; each native
collector supplies the separate `system.hostname` field.

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
- **Cross-process lock contention** covered by `lock_helper` binary behind `test-helper` feature
- **Protocol test support:** `test_support` feature flag exposes builder fixtures (`LinuxSnapshotBuilder`, `MacosSnapshotBuilder`, `LinuxSnapshotV2Builder`, `WindowsSnapshotV2Builder`) that validate on build

Run the short routine check with:

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

The manual `--release` / `-Release` preflight adds Clippy, documentation,
package/version checks, installation smoke, and the protocol dry-run. Ordinary
CI keeps Linux generic checks, native macOS/Windows coverage, and one
compile-only Rust 1.75 check; it does not build docs, publish, or upload
evidence.

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
| [scripts-and-packaging.md](scripts-and-packaging.md) | Scripts, installers, service definitions, CI |
| [macos-collector-notes.md](macos-collector-notes.md) | Expected differences between macOS collector and Activity Monitor / `top` / `vm_stat` |

### Supporting files

| Document | Scope |
|----------|-------|
| [README.md](README.md) | Directory index and purpose |
| [`../plans/`](../plans/) | Phase plans — source of truth for sequencing and acceptance criteria |
| [`../AGENTS.md`](../AGENTS.md) | Compact agent instructions for this repository |
