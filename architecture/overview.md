# Architecture overview

This document provides a bird's-eye view of the entire `gregg` codebase: what
each piece does, how they connect, and where to go for details.

## System at a glance

`gregg` is a cross-platform system metrics collection and monitoring tool
composed of three Rust crates in a single Cargo workspace:

```
┌─────────────────────────────────────────────────────────────┐
│                        gregg (client)                       │
│  TUI + CLI + HTTP polling + EggPool summary                 │
│  Platforms: Linux, macOS, Windows                           │
└──────────────────────────┬──────────────────────────────────┘
                           │ HTTP (JSON)
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                       greggd (daemon)                       │
│  Metrics collector + sampler + HTTP server + service mgmt   │
│  Platforms: Linux, macOS, Windows                           │
└──────────────────────────┬──────────────────────────────────┘
                           │ uses wire types from
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                   gregg-protocol (library)                  │
│  Shared wire types, schema versions, validation, health     │
│  No runtime, HTTP, terminal, or platform dependencies       │
└─────────────────────────────────────────────────────────────┘
```

## The three crates

| Crate | Role | Key modules |
|-------|------|-------------|
| [`gregg-protocol`](#gregg-protocol) | Shared wire types and validation | `snapshot`, `v2`, `validate`, `validate_v2`, `health`, `test_support` |
| [`greggd`](#greggd-daemon) | Metrics daemon and service manager | `collector`, `sampler`, `server`, `service`, `config`, `cli` |
| [`gregg`](#gregg-client) | Client TUI and endpoint CLI | `poller`, `scheduler`, `state`, `action`, `ui/`, `config`, `cli`, `eggpool` |

Dependency direction is strictly one-way:

```
gregg-protocol  ◄── greggd
gregg-protocol  ◄── gregg
```

`greggd` and `gregg` never depend on each other.

---

## gregg-protocol

**Purpose:** Defines the JSON wire contract between daemon and client. Pure data
types with serde serialization and structured validation. No I/O, no runtime
dependencies beyond serialization.

**Key concepts:**
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

**Key concepts:**
- **Collector** — platform-specific metric collection (`/proc` on Linux, Mach FFI
  on macOS, Win32 API on Windows). No external commands for metrics.
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
normal and condensed fleet views.

**Key concepts:**
- **Poll scheduler** — generation-based concurrency; v2-first/v1-fallback protocol
- **State reducer** — action/Reducer pattern; all state changes through `Action` enum
- **Normalized snapshots** — v1 and v2 wire formats normalized to a single internal type
- **EggPool** — optional summary pane for EggPool API metrics
- **Cross-process config locking** — `flock(2)` / `LockFileEx` prevents concurrent corruption

**Deep dive:** [gregg-client.md](gregg-client.md)

---

## Cross-cutting concerns

### Platform collectors

Each platform collector implements the `SystemCollector` trait and reads only
native kernel interfaces. CPU percentages require two samples (delta-based).
macOS has no I/O-wait equivalent. Windows cannot produce v1 snapshots (no
load/swap).

**Deep dive:** [collectors.md](collectors.md)

### Wire protocol and validation

The protocol supports two schema versions. V2 is preferred; the client falls
back to v1 on 404. Capability flags control which optional fields must be
present. Validation is structured and separate from deserialization.

**Existing docs:** [protocol.md](protocol.md)

### Error boundaries

Each binary crate has crate-local typed errors via `thiserror`. Wire responses
carry only safe, structured info (category + message). Collector errors never
appear on the wire.

**Existing doc:** [error-conventions.md](error-conventions.md)

### Scripts and packaging

Installer scripts for all three platforms, a local validation script
(`check-local.sh`), loopback smoke tests, and systemd/launchd/SCM service
definitions.

**Deep dive:** [scripts-and-packaging.md](scripts-and-packaging.md)

---

## Data flow

```
┌──────────────────────────────────────────────────────────────────┐
│  greggd on monitored host                                       │
│                                                                  │
│  ┌──────────┐    ┌─────────┐    ┌──────────────┐    ┌────────┐ │
│  │ Collector │───▶│ Sampler │───▶│ Cached Snap. │◀───│ Server │ │
│  │ (native)  │    │ (clock) │    │ (v1 + v2)    │    │ (HTTP) │ │
│  └──────────┘    └─────────┘    └──────────────┘    └───┬────┘ │
│                                                          │      │
└──────────────────────────────────────────────────────────┼──────┘
                                                           │ JSON
                                                           ▼
┌──────────────────────────────────────────────────────────────────┐
│  gregg on user's terminal                                       │
│                                                                  │
│  ┌──────────┐    ┌───────────┐    ┌──────────┐    ┌──────────┐ │
│  │ Scheduler │───▶│ PollBatch │───▶│ AppState │───▶│   TUI    │ │
│  │ (timer)   │    │ channel   │    │ reducer  │    │ (ratatui)│ │
│  └──────────┘    └───────────┘    └──────────┘    └──────────┘ │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
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

---

## Configuration

| Component | Format | Default path (Linux) |
|-----------|--------|---------------------|
| greggd | TOML | `/etc/gregg/greggd.toml` |
| gregg | TOML | `$XDG_CONFIG_HOME/gregg/gregg.toml` |

Both use atomic writes (write-flush-rename-verify) and structured validation.
The daemon config has 5 fields; the client config stores system endpoints,
refresh intervals, and optional EggPool settings.

---

## Testing strategy

- **Unit tests** in every module with deterministic fixtures and mock collectors
- **Integration tests** in `tests/` directories for live smoke tests
- **TUI buffer tests** cover width degradation, mixed fleets, and resize
- **Sustained workload test** (`#[ignore]`) exercises the full polling loop
- **Platform-native collector tests** run only on the target OS

Run all checks with:

```bash
./scripts/check-local.sh
```

---

## Index of architecture documents

| Document | Scope |
|----------|-------|
| [overview.md](overview.md) | This file — bird's-eye view |
| [gregg-protocol.md](gregg-protocol.md) | Deep dive: protocol crate |
| [greggd-daemon.md](greggd-daemon.md) | Deep dive: daemon crate |
| [gregg-client.md](gregg-client.md) | Deep dive: client crate |
| [collectors.md](collectors.md) | Deep dive: platform collectors |
| [scripts-and-packaging.md](scripts-and-packaging.md) | Deep dive: scripts and packaging |
| [protocol.md](protocol.md) | Wire format specification |
| [workspace.md](workspace.md) | Crate boundaries and module structure |
| [error-conventions.md](error-conventions.md) | Error boundary design |
| [macos-collector-notes.md](macos-collector-notes.md) | macOS collector diagnostics |
