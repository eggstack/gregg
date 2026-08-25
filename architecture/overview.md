# Architecture overview

This document is the bird's-eye view of the `gregg` codebase: what each piece
does, who owns it, how the pieces connect, and where to go for details. It is
also the index for the deep-dive documents in this directory — every component
section below ends with a link to its deep dive.

## Reading this document

If you are new to the codebase, read this overview first, then follow the deep
dive links in the order that matches your task:

1. **[workspace.md](workspace.md)** — crate boundaries, dependency rules,
   MSRV, lint policy, and release profiles. Read before changing any crate
   structure.
2. **[gregg-protocol.md](gregg-protocol.md)** — the wire contract. Read before
   touching shared types or adding fields.
3. **[greggd-daemon.md](greggd-daemon.md)** — daemon internals. Read before
   modifying collectors, the sampler, HTTP server, or service management.
4. **[gregg-client.md](gregg-client.md)** — client internals. Read before
   modifying the TUI, polling, state engine, or EggPool.
5. **[collectors.md](collectors.md)** — platform-specific metric collection.
   Read before modifying Linux, macOS, or Windows collector code.
6. **[protocol.md](protocol.md)** — wire format specification, schema
   versions, capabilities, and compatibility policy. Read before changing
   validation rules or adding schema versions.
7. **[error-conventions.md](error-conventions.md)** — error boundary design
   and wire response constraints. Read before adding new error types.
8. **[scripts-and-packaging.md](scripts-and-packaging.md)** — scripts,
   installers, service definitions, CI. Read before touching packaging.
9. **[macos-collector-notes.md](macos-collector-notes.md)** — expected
   differences between the macOS collector and Activity Monitor / `top`.

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
│  Shared wire types, schema versions, validation, health responses   │
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
| `gregg` | `crates/gregg/` | Bin + lib | Client TUI, endpoint CLI, polling, EggPool | [gregg-client.md](gregg-client.md) |

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
| `lib` | `src/lib.rs` | Root, re-exports, schema version constant (`SCHEMA_VERSION_V1 = 1`), `MAX_SAMPLE_INTERVAL_MS` |
| `snapshot` | `src/snapshot.rs` | V1 wire types: `StatusSnapshot`, `CpuMetrics`, `LoadAverage`, `MemoryMetrics`, `SwapMetrics`, `SystemIdentity`, `MetricCapabilities`; `validate()` |
| `v2` | `src/v2.rs` | V2 wire types: `StatusSnapshotV2`, `StatusPayloadV2`, `MetricCapabilitiesV2`, `DriveMetrics`, `CommitMetrics`, `HealthResponseV2`; constants `SCHEMA_VERSION_V2 = 2`, `MAX_DRIVE_ENTRIES = 32`, `MAX_DRIVE_NAME_BYTES = 512` |
| `validate` | `src/validate.rs` | V1 validation: returns `Result<(), Vec<ValidationViolation>>` with 9 violation kinds |
| `validate_v2` | `src/validate_v2.rs` | V2 validation: `validate_v2()` and `validate_payload_v2()` with 16 violation kinds, capability/value consistency |
| `health` | `src/health.rs` | V1 health types: `HealthResponse`, `ReadinessState`, `HealthCategory` |
| `test_support` | `src/test_support.rs` | Feature-gated (`test_support`) builder fixtures: `LinuxSnapshotBuilder`, `MacosSnapshotBuilder`, `LinuxSnapshotV2Builder`, `WindowsSnapshotV2Builder`, `IdentityFixture` |

### Key concepts

- **Schema v1** — original Linux/macOS format with required load/swap.
  Windows cannot produce it (`/v1/status` returns 503).
- **Schema v2** — extended with capability flags for load, swap, commit; an
  optional drives array with caller-available capacity. `/v2/status` is the
  universal cross-platform endpoint.
- **Capability flags** — each platform declares what metrics it supports; the
  client uses these to decide what to render.
- **Validation** — separate from serde; `validate()` methods return structured
  violation lists, not serde failures.
- **Health responses** — three states (`Ready`, `Warming`, `Failed`) with coarse
  categories for the wire; internal error chains never leak.

**Deep dive:** [gregg-protocol.md](gregg-protocol.md)

---

## greggd (daemon)

**Purpose:** Runs on the monitored host. Collects system metrics using native OS
interfaces, samples them at a configurable interval, serves them over HTTP, and
manages its own OS service lifecycle on Windows. Both a binary (`src/main.rs`)
and a library (`src/lib.rs`) target; the lib surface exposes the collector for
integration tests.

### Modules

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Binary boundary: CLI parsing, logging, error reporting, exit-code classification, platform collector dispatch |
| `lib` | `src/lib.rs` | Library root re-exporting all modules below |
| `run` | `src/run.rs` | Supervision loop wiring collector, sampler, server, signals, and the local control socket; `RunOutcome`, `run_with_shutdown()` core with 10s graceful deadline |
| `cli` | `src/cli.rs` | Clap CLI: `run`, `stop`, `croncheck` (TCP-connect watchdog that spawns `run` if nothing listens), `configprint` (read-only bind address), `host`, `port`, `version`; Windows adds SCM `start`/`restart`/`service`; `ExitCode` taxonomy |
| `config` | `src/config.rs` | TOML config, validation, atomic writes; `ConfigError`, `ConfigViolation`, `AtomicWriteError` |
| `control` | `src/control.rs` | Unix-only control socket for `greggd stop` (`STOP\n` → `OK\n`); config identity via FNV-1a digest of canonicalized path; restrictive permissions, conservative stale-socket cleanup; `ControlSocketGuard` cleanup on every exit path |
| `net` | `src/net.rs` | Local-network address resolution for `configprint`: resolves a wildcard bind host to the primary local IP via a transient UDP `connect()` (no packets sent) |
| `sampler` | `src/sampler.rs` | Periodic sampling loop, readiness lifecycle (`Warming` → `Ready`/`Failed`); `SamplerError`, clock abstraction with `RealClock`/synthetic variants |
| `server/mod` | `src/server/mod.rs` | Axum HTTP server, five routes (`/`, `/v1/status`, `/v2/status`, `/healthz`, `/v2/healthz`), staleness detection; `ServerState`, `PublishedState` |
| `server/error` | `src/server/error.rs` | Server error types |
| `server/tests` | `src/server/tests.rs` | In-module HTTP handler tests |
| `collector/mod` | `src/collector/mod.rs` | `SystemCollector` trait (`identity()`, `sample()`, `capabilities()`, `capabilities_v2()`, `supports_v1_snapshot()`); `CollectedMetrics` normalization to v1/v2 wire formats |
| `collector/error` | `src/collector/error.rs` | `CollectErrorKind` taxonomy (6 kinds) |
| `collector/drives` | `src/collector/drives.rs` | Shared drive normalization: candidates, dedup, sort, truncate to `MAX_DRIVE_ENTRIES` |
| `collector/linux/` | `src/collector/linux/` | Linux collector: cpu, memory, drives, identity; `FileSource` trait (`ProcSource` prod reads `/proc`, `MemorySource` test) plus statvfs FFI in `source.rs` |
| `collector/macos/` | `src/collector/macos/` | macOS collector: cpu, memory, swap, identity, normalize; Mach/sysctl FFI seam in `ffi.rs` (`MacNativeQueries` trait, `FfiNativeQueries` prod, mock for tests) |
| `collector/windows/` | `src/collector/windows/` | Windows collector: cpu, memory, commit, identity; `WindowsSource` trait (`NativeWindowsSource` prod, mock for tests) |
| `service/mod` | `src/service/mod.rs` | `ServiceManager` trait (Windows-only) |
| `service/windows` | `src/service/windows.rs` | Windows SCM integration via `windows-service`; native dispatcher entry owned by the binary, one current-thread Tokio runtime per service worker |

### Key concepts

- **Collector** — platform-specific metric collection. No external commands;
  kernel interfaces only (`/proc`, Mach APIs, Win32 APIs). The first sample
  after construction is `Warming` because CPU percentages need two readings.
- **Sampler** — owns the clock and cadence; calls the collector periodically
  and produces immutable cached snapshots. The server never triggers collection.
- **HTTP server** — read-only. Five routes; serves cached snapshots with
  staleness detection; `/v1/status` is unavailable where unsupported.
- **Supervision** — `tokio::select!` over shutdown signal, server task, and
  sampler task; graceful shutdown with a 10-second deadline. SIGTERM/SIGINT,
  SCM Stop/Shutdown, and `STOP\n` on the local control socket all feed the
  same shutdown path.
- **Service manager** — Windows SCM only (dispatcher started synchronously
  before any Tokio runtime exists). Unix supervisors stay external packaging.
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

### Binaries

| Binary | File | Purpose |
|--------|------|---------|
| `gregg` | `src/main.rs` | The client itself: CLI dispatch plus the async TUI event loop |
| `lock_helper` | `src/bin/lock_helper.rs` | Cross-process config-lock test helper; only built behind the `test-helper` feature |
| `probe_top` | `src/bin/probe_top.rs` | Standalone TCP-connectivity probe helper (Tokio + std connect checks against `PROBE_HOST`/`PROBE_PORT`); auto-discovered by Cargo from `src/bin/` and always built |

### Modules

#### Core

| Module | File | Purpose |
|--------|------|---------|
| `main` | `src/main.rs` | Entry point, biased `tokio::select!` event loop, TUI lifecycle, subcommand dispatch |
| `cli` | `src/cli.rs` | Clap CLI: `version`, `add`, `list`, `remove`, `refresh`, `edit`, `eggpool add/list/remove`; strict port-required endpoint parsing for `add`; `ExitCode` taxonomy |
| `config` | `src/config.rs` | Config model, validation, atomic I/O, cross-process locking; `ConfigStore` with `load_or_default`, `load_existing`, `write`, `mutate`; editor resolution for `edit` |
| `state` | `src/state.rs` | `AppState` reducer, viewport logic, display order, pane/view-mode state, selection-highlight deadline |
| `action` | `src/action.rs` | `Action` enum (14 variants: `MoveDown`, `MoveUp`, `PageDown`, `PageUp`, `SelectFirst`, `SelectLast`, `PreviousPane`, `NextPane`, `ToggleSystemView`, `ToggleDrives`, `RefreshNow`, `Resize`, `ClearSelectionHighlight`, `Quit`) |

#### Polling

| Module | File | Purpose |
|--------|------|---------|
| `poller` | `src/poller.rs` | HTTP client, v2-first/v1-fallback (fallback only on HTTP 404), `PollBatch` with generation counter, 64 KiB body cap |
| `scheduler` | `src/scheduler.rs` | Periodic poll scheduler; `SchedulerCommand` (`Refresh`, `ReplaceEndpoints`); semaphore-bounded per-endpoint tasks; one ordered result per endpoint per generation; offline endpoints keep polling every cadence |
| `endpoint` | `src/endpoint.rs` | Endpoint parsing: `host:port`, `[ipv6]:port`, HTTP URL convenience form, `nickname@host:port`; explicit port always required for `add`; HTTPS never accepted/downgraded |
| `clock` | `src/clock.rs` | Clock trait for deterministic testing; real and fake implementations |
| `normalized` | `src/normalized.rs` | Normalized v1/v2 snapshot for UI consumption with capability flags; drive aggregation with checked arithmetic |

#### Input

| Module | File | Purpose |
|--------|------|---------|
| `event` | `src/event.rs` | Input event model: key events, signals (hangup/window-change/terminate), poll batches, config-change notifications; Vim-style key-to-action translation |
| `input` | `src/input.rs` | Crossterm event-stream adapter on a dedicated task feeding the event loop |
| `terminal` | `src/terminal.rs` | Terminal lifecycle (raw mode, alt screen, cursor hiding, panic hook) |

#### UI

| Module | File | Purpose |
|--------|------|---------|
| `ui/mod` | `src/ui/mod.rs` | Render entry point; dispatches on active pane and view mode; guards empty-config and too-small terminals |
| `ui/layout` | `src/ui/layout.rs` | Viewport computation (which systems are visible, rect positions) |
| `ui/system_block` | `src/ui/system_block.rs` | Normal-view system rendering; authoritative fleet-wide metric-row geometry (`MetricRow`, `build_metric_rows`, `compute_fleet_metric_layout`) so `[`/`]` columns align across the fleet |
| `ui/condensed` | `src/ui/condensed.rs` | Condensed one-row fleet view (Wide ≥ 64, Medium 48–63, Narrow 30–47, Minimal < 30 cols) |
| `ui/bar` | `src/ui/bar.rs` | Reusable ASCII usage bar widget with width-safe arithmetic |
| `ui/text` | `src/ui/text.rs` | Text formatting (bytes, percentages, load averages), priority-aware header composition, drive detail rows/tables |
| `ui/diagnostics` | `src/ui/diagnostics.rs` | Empty-config and terminal-too-small messages |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering across pending/success/stale/error states |

#### EggPool

| Module | File | Purpose |
|--------|------|---------|
| `eggpool` | `src/eggpool.rs` | EggPool summary client and background worker; separate bounded command channel with generation checks; 60-second passive refresh when the pane is active; Hour/Day/Week/Month period cycling |
| `eggpool_endpoint` | `src/eggpool_endpoint.rs` | EggPool-specific endpoint parsing; defaults to HTTP port 11300 |

#### Test modules

| Module | File | Purpose |
|--------|------|---------|
| `mixed_fleet_evidence` | `src/mixed_fleet_evidence.rs` | `#[cfg(test)]` integration driver with Python fixture servers; fixture modes + refused endpoint |
| `sustained_workload` | `src/sustained_workload.rs` | `#[cfg(test)]` long-running regression driver (`#[ignore]`); validates generation invariants and bounded concurrency; invoked by `scripts/run-mixed-fleet-sustained.py` |

### Key concepts

- **Poll scheduler** — generation-based concurrency; v2-first/v1-fallback
  protocol. One isolated poll task per endpoint; a semaphore bounds active
  polls; task panic converts to `Cancelled`. One ordered result per endpoint
  per generation, every cadence — offline endpoints are retried without
  backoff.
- **State reducer** — action/reducer pattern; all state changes flow through
  the `Action` enum. `AppState::apply_action()` and `apply_batch()` are pure
  and deterministic; the first accepted batch snaps selection to the first
  system, later batches preserve user selection.
- **Selection model** — logical selection (`selected_id`) is persistent;
  the reverse-video highlight is transient with a ten-second reset deadline
  owned by the event loop.
- **Config reload** — `Ctrl-R` reloads the resolved `ConfigStore`, reconciles
  stable system IDs, and delivers replacement endpoints through the bounded
  scheduler channel; invalid reloads preserve last-known-good state.
- **Normalized snapshots** — v1 and v2 wire formats normalize to one internal
  type, eliminating version-branching in the UI.
- **EggPool** — optional summary pane, deliberately separate from greggd
  polling: its own client, worker, authentication (API key name stored; key
  value stays in the named env var), and rendering.
- **Cross-process config locking** — `flock(2)` / `LockFileEx` prevents
  concurrent corruption of the TOML config file.
- **Width degradation** — header line drops lower-priority segments as width
  decreases (< 32: no load, < 50: no OS, < 80: no arch); compact-mode metric
  suffixes are suppressed fleet-wide when they exceed a quarter of terminal
  width.

**Deep dive:** [gregg-client.md](gregg-client.md)

---

## Data flow

### Primary: greggd → gregg polling

```
┌──────────────────────────────────────────────────────────────────────┐
│  greggd on monitored host                                            │
│                                                                      │
│  ┌───────────┐    ┌─────────┐    ┌────────────┐    ┌─────────────┐  │
│  │ Collector │───▶│ Sampler │───▶│ Cached     │◀───│ HTTP Server │  │
│  │ (native)  │    │ (clock) │    │ Snap v1+v2 │    │ (axum)      │  │
│  └───────────┘    └─────────┘    └────────────┘    └─────────────┘  │
│                                          ▲               │          │
└──────────────────────────────────────────┼───────────────┼──────────┘
                                           │ cached        │ JSON
                                           ▼               ▼
┌──────────────────────────────────────────────────────────────────────┐
│  gregg on user's terminal                                            │
│                                                                      │
│  ┌───────────┐    ┌───────────┐    ┌──────────┐    ┌─────────────┐  │
│  │ Scheduler │───▶│ PollBatch │───▶│ AppState │───▶│ TUI         │  │
│  │ (timer)   │    │ channel   │    │ reducer  │    │ (ratatui)   │  │
│  └───────────┘    └───────────┘    └──────────┘    └─────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
```

1. The **collector** reads native OS interfaces (procfs, Mach, Win32 API)
2. The **sampler** calls the collector on a timer, stamps timestamps, produces
   immutable v1 and v2 status snapshots
3. The **HTTP server** serves the cached snapshots on request
4. The **client scheduler** polls each endpoint on the configured interval
5. **PollBatches** arrive on a channel tagged with a generation counter
6. The **state reducer** applies batches, rejects stale generations, updates
   reachability and selection
7. The **TUI** renders `AppState` projections without doing I/O

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
│ (eggpool pane)   │
└──────────────────┘
```

The EggPool path is deliberately separate from greggd polling. It has its own
client, worker, authentication, and rendering. The worker runs a 60-second
passive refresh cadence while the pane is active, uses generation-based
staleness, aborts in-flight requests on superseding commands, and keeps
period cycling (Hour/Day/Week/Month) pane-local.

---

## Cross-cutting concerns

### Platform collectors

Each platform collector implements the `SystemCollector` trait and reads only
native kernel interfaces. CPU percentages require two samples (delta-based).
No external commands are executed for metric collection.

| Platform | Source | Key interfaces | Test seam |
|----------|--------|----------------|-----------|
| Linux | `collector/linux/` | `/proc/stat`, `/proc/meminfo`, `/proc/self/mountinfo`, `statvfs` | `FileSource` trait (`ProcSource` prod, in-memory test source) |
| macOS | `collector/macos/` | Mach `host_statistics`, `sysctl`, `getloadavg`, `getmntinfo` | `MacNativeQueries` trait (`FfiNativeQueries` prod, mock in tests) |
| Windows | `collector/windows/` | `GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo` | `WindowsSource` trait (`NativeWindowsSource` prod, mock in tests) |

Platform gaps are reported honestly: macOS has no I/O-wait equivalent
(`iowait_pct` is `null`); Windows cannot produce load average, swap, or
I/O-wait and reports `commit` instead, so it serves no v1 snapshot. Values are
never fabricated.

**Deep dives:** [collectors.md](collectors.md),
[macos-collector-notes.md](macos-collector-notes.md)

### Wire protocol and validation

Two schema versions. V2 is preferred; the client falls back to v1 only on an
HTTP 404 from `/v2/status`. Capability flags control which optional fields
must be present. Validation is structured and separate from deserialization.

| Concept | Details |
|---------|---------|
| Schema v1 | Original Linux/macOS format; required load/swap; 9 validation violation kinds |
| Schema v2 | Capability flags; optional load/swap/commit; drives array; 16 validation violation kinds |
| Validation | Structured violation lists (`Vec<ValidationViolation>`), not serde errors |
| Compatibility | Additive within a schema version; breaking changes require a new major version |
| Identity | `system.name` is the validated configured daemon name; `system.hostname` is the native platform hostname |
| Health responses | Three states (`Ready`, `Warming`, `Failed`) with coarse categories |

**Deep dive:** [protocol.md](protocol.md)

### Error boundaries

Each application crate uses crate-local typed errors via `thiserror`. Wire
responses carry only safe, structured info (category + message). Collector
errors never appear on the wire.

| Boundary | Pattern |
|----------|---------|
| Daemon runtime | Typed errors; binary boundary formats diagnostics; exit codes 0=success, 1=config, 2=service, 3=runtime, 4=permission |
| Wire responses | `HealthCategory` + short message; no paths or error chains |
| Collector | 6 `CollectErrorKind` variants (`Warming`, `SourceUnavailable`, `Parse`, `CounterReset`, `Numeric`, `IdentityFallback`); crate-local, never on wire |
| Client polling | `PollOutcome`: 12 classifications (2 success: `Online`/`OnlineV2`, 10 failure incl. `Cancelled`) |

**Deep dive:** [error-conventions.md](error-conventions.md)

### Scripts and packaging

Installer scripts for all three platforms, the routine validation script,
loopback smoke tests, and systemd/launchd/SCM service definitions.

| Artifact | Purpose |
|----------|---------|
| `scripts/check-local.sh` / `.ps1` | Primary local validation: fmt check + workspace tests; `--release` adds Clippy, docs, smoke, protocol dry-run |
| `scripts/verify-installed-daemon.sh` | Bounded loopback smoke: isolated port, temp config, health poll, SIGTERM |
| `scripts/test-verify-installed-daemon.sh` | Self-test wrapper for the verify script |
| `scripts/smoke-windows.ps1` | Bounded Administrator SCM lifecycle smoke: install → start → health → stop → restart → cleanup |
| `packaging/install-linux.sh` | Systemd service, dedicated user, hardened unit |
| `packaging/install-macos.sh` | Launchd plist installation |
| `packaging/install-windows.ps1` / `uninstall-windows.ps1` | SCM service install/remove |
| `packaging/systemd/greggd.service` | Hardened unit (NoNewPrivileges, ProtectSystem, …) |
| `packaging/launchd/com.eggstack.greggd.plist` | KeepAlive on crash, RunAtLoad, fd limit |

CI (GitHub Actions) runs fmt/clippy/tests on Linux, native macOS and Windows
jobs including the SCM smoke, and a Rust 1.75 MSRV compile check. CI never
publishes or uploads evidence; releases are manual per `RELEASING.md`.

**Deep dive:** [scripts-and-packaging.md](scripts-and-packaging.md)

### Workspace rules

Three crates, strict one-way dependency direction, shared version from
`[workspace.package]`, MSRV Rust 1.75 pinned via `rust-toolchain.toml`,
clippy pedantic warnings, unsafe restricted to named FFI files with mandatory
safety comments, and publication order `gregg-protocol` → `greggd` → `gregg`.

**Deep dive:** [workspace.md](workspace.md)

---

## Configuration

| Component | Format | Default path (Linux) | Default path (macOS) | Default path (Windows) |
|-----------|--------|----------------------|----------------------|------------------------|
| greggd | TOML | `/etc/gregg/greggd.toml` | `/Library/Application Support/gregg/greggd.toml` | `%ProgramData%\gregg\greggd.toml` |
| gregg | TOML | `$XDG_CONFIG_HOME/gregg/gregg.toml` | `~/Library/Application Support/gregg/gregg.toml` | `%APPDATA%\gregg\gregg.toml` |

Both use atomic writes (temp file → flush → rename → validate) and structured
validation returning typed violations. The daemon's configured `name` is
published as `system.name` on the wire; each native collector supplies the
separate `system.hostname`.

### Cross-process config locking

- Unix: `flock(2)` advisory lock on `<config>.lock`
- Windows: `LockFileEx` exclusive lock on `<config>.lock`
- Other platforms: in-process `Mutex` only

---

## Testing strategy

- **Unit tests** in every module with deterministic fixtures and mock
  collector sources
- **Integration tests:** `crates/gregg-protocol/tests/integration.rs`,
  `crates/greggd/tests/linux_collector.rs`,
  `crates/greggd/tests/windows_smoke.rs`
- **JSON fixtures:** `crates/gregg-protocol/tests/fixtures/` for v1/v2
  cross-platform payloads; ~46 text fixtures under
  `crates/greggd/src/collector/test_fixtures/` for `/proc` and OS files
- **TUI buffer tests** cover width degradation, mixed fleets, and resize
- **Sustained workload driver** (`#[ignore]`) exercises the full polling loop
  via `scripts/run-mixed-fleet-sustained.py` with its pytest suite in
  `scripts/tests/`
- **Platform-native collector tests** run only on their target OS
- **Mock seams:** in-memory `FileSource` (Linux), `MockNativeQueries` (macOS),
  mock `WindowsSource` (Windows)
- **Protocol builders:** `test_support` feature exposes snapshot builders that
  validate on build
- **Lock contention:** `lock_helper` binary behind the `test-helper` feature;
  the cross-process lock test silently skips when the binary is absent

Routine verification:

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

The manual `--release` preflight adds Clippy, documentation, package/version
checks, installation smoke, and the protocol dry-run.

Platform-native collector tests:

```bash
cargo test -p greggd --all-features -- collector::linux     # Linux
cargo test -p greggd --all-features -- collector::macos     # macOS
cargo test -p greggd --all-targets -- collector::windows    # Windows
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
