# Architecture overview

This document is the bird's-eye view of the `gregg` codebase: what each
discrete component does, how they fit together, and where to go for depth.
It is intentionally brief — each section below summarizes one component and
links to its dedicated deep dive. Read this file first, then follow the links
that match your task.

## How to read this directory

| Order | Document | Read before |
|-------|----------|-------------|
| 1 | [workspace.md](workspace.md) | Changing crate layout, dependencies, MSRV, lints, or release profiles |
| 2 | [gregg-protocol.md](gregg-protocol.md) + [protocol.md](protocol.md) | Touching wire types, schema versions, capabilities, or validation |
| 3 | [greggd-daemon.md](greggd-daemon.md) | Touching the sampler, HTTP server, CLI, control socket, or service management |
| 4 | [collectors.md](collectors.md) + [macos-collector-notes.md](macos-collector-notes.md) | Touching Linux, macOS, or Windows metric collection |
| 5 | [gregg-client.md](gregg-client.md) | Touching polling, state, TUI rendering, CLI, or EggPool |
| 6 | [gregg-update.md](gregg-update.md) | Touching `gregg update` / `greggd update`, targets, staging, or release assets |
| 7 | [scripts-and-packaging.md](scripts-and-packaging.md) | Touching installers, service definitions, scripts, or CI |
| 8 | [error-conventions.md](error-conventions.md) | Adding error types or changing wire-facing diagnostics |

Phase plans under [`../plans/`](../plans/) are the source of truth for
sequencing and acceptance criteria; this directory records the architectural
commitments those plans must respect together.

---

## System at a glance

`gregg` is a private-LAN system monitor: a daemon (`greggd`) on each watched
host exposes cached metrics over HTTP, and a terminal client (`gregg`) polls a
small fleet and renders a live TUI. Two small libraries carry the shared
contract and the shared self-update mechanism.

```
┌─────────────────────────────────────────────────────────┐
│ gregg (client)                                          │
│ CLI + polling + state reducer + TUI (+ optional EggPool)│
└────────────────────────────┬────────────────────────────┘
                             │ HTTP JSON: /v2/status first,
                             │ fallback to v1 only on 404
                             ▼
┌─────────────────────────────────────────────────────────┐
│ greggd (daemon)                                         │
│ native collectors + sampler + axum server + service mgmt│
└────────────────────────────┬────────────────────────────┘
                             │ uses wire types from
                             ▼
┌─────────────────────────────────────────────────────────┐
│ gregg-protocol (library)                                │
│ versioned JSON types + structured validation + health   │
└─────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────┐
│ gregg-update (internal library)                         │
│ binary-first self-update mechanics for gregg + greggd   │
└─────────────────────────────────────────────────────────┘
```

Strict one-way dependencies (enforced by manifests, see
[workspace.md](workspace.md)):

```
gregg-protocol  ◄── greggd
gregg-protocol  ◄── gregg
gregg-update    ◄── greggd
gregg-update    ◄── gregg
```

`greggd` and `gregg` never depend on each other. `gregg-protocol` depends on
no workspace crate. `gregg-update` depends on neither app crate, nor service
managers, TUI, EggPool, or the wire protocol.

| Crate | Path | Kind | Role | Deep dive |
|-------|------|------|------|-----------|
| `gregg-protocol` | `crates/gregg-protocol/` | lib | JSON wire contract (v1/v2, capabilities, validation, health) | [gregg-protocol.md](gregg-protocol.md) |
| `gregg-update` | `crates/gregg-update/` | lib (internal) | Shared binary-first self-update mechanics | [gregg-update.md](gregg-update.md) |
| `greggd` | `crates/greggd/` | bin+lib | Metrics daemon: collect, sample, serve, manage lifecycle | [greggd-daemon.md](greggd-daemon.md) |
| `gregg` | `crates/gregg/` | bin (+ `lock_helper` test helper) | Fleet client: manage endpoints, poll, reduce state, render TUI | [gregg-client.md](gregg-client.md) |

---

## Components

### gregg-protocol — the wire contract

Pure data types plus structured validation. No I/O, no runtime/HTTP/terminal/
platform dependencies (`serde`, `serde_json`, `thiserror` only;
`#![forbid(unsafe_code)]`).

- Schema v1: Linux/macOS shape with required load/swap (`snapshot.rs`,
  `validate.rs` — 9 violation kinds).
- Schema v2: cross-platform shape with capability flags (`load_average`,
  `swap`, `memory_commit`, `cpu_iowait`), optional drives (≤32 entries),
  and additive live telemetry (CPU Hz, disk R/s/W/s, net Rx/s/Tx/s)
  (`v2.rs`, `validate_v2.rs` — 16 base kinds + telemetry bounds).
- Health: `Ready` / `Warming` / `Failed` with coarse wire-safe categories
  (`health.rs`); validation returns `Vec<Violation>`, never serde errors.
- Test fixtures: `test_support` builders + `tests/fixtures/` JSON payloads.

**Deep dives:** [gregg-protocol.md](gregg-protocol.md) (crate),
[protocol.md](protocol.md) (wire spec, compat policy, polling contract).

### gregg-update — the shared updater

One authoritative implementation of binary-first self-update (Plan 104).
Caller-parameterized by `UpdateSpec { crate_name, program_name,
current_version }`; knows nothing about service managers, TUI, or protocol.

- `version` / `target`: stable SemVer compare; 5-target table drift-tested
  against `scripts/release-targets.txt`.
- `exec` / `verify` / `stage`: bounded `curl`/Cargo execution, SHA-256 +
  candidate-`version` checks, owner-private `TempDir` staging, `self-replace`.
- Policy: crates.io `max_stable_version` is authoritative, exact `vX.Y.Z`
  asset `<program>-<target>[.exe]` + `.sha256`, Cargo fallback only on
  HTTP 404. `gregg` runs `run_simple_update`; `greggd` prepares fully before
  quiescing and restarts only if running/managed.

**Deep dive:** [gregg-update.md](gregg-update.md) (mechanics);
[scripts-and-packaging.md](scripts-and-packaging.md) (asset/installer/release
contract).

### greggd — the daemon

Runs on each monitored host: collects via native OS interfaces only
(`/proc`, Mach/sysctl/IOKit, Win32 — never external commands), samples on a
clock, serves cached immutable snapshots, and owns its OS lifecycle.
Bin+lib split: reusable code returns errors (exit codes `0`/`1`/`2`/`3`/`4`
are a binary-boundary concern).

- `collector/{linux,macos,windows}/` + shared `rate.rs` (monotonic
  counter baselines), `drives.rs` (dedup/sort/truncate), `error.rs`
  (6 `CollectErrorKind`s). First sample is `Warming`; gaps re-baseline or
  omit — never fabricate zeroes.
- `sampler.rs` (cadence, readiness, `Warming→Ready/Failed`), `server/`
  (axum, `/`, `/v1/status`, `/v2/status`, `/healthz`, `/v2/healthz`,
  staleness policy), `run.rs` (supervision, 10s graceful shutdown).
- `cli.rs` (`run`, `stop`, `croncheck`, `configprint`, `status`, `host`,
  `port`, `startup install/instructions`, `restart`, `update`), `control.rs`
  (Unix `STOP\n→OK\n` socket, FNV-1a config identity, `0600`), `startup/`
  (systemd/launchd/cron + Windows SCM), `status.rs`/`net.rs` (read-only
  diagnostics, wildcard→local-IP resolution).

**Deep dives:** [greggd-daemon.md](greggd-daemon.md) (runtime/server/CLI),
[collectors.md](collectors.md) (per-OS collection),
[macos-collector-notes.md](macos-collector-notes.md) (Activity Monitor/top
differences).

### gregg — the client

Watches many daemons from one terminal: endpoint CLI, concurrent polling,
pure state reducer, Ratatui TUI, plus an isolated optional EggPool pane.

- Polling: `scheduler.rs` (generations, semaphore bound, panic→`Cancelled`,
  offline retried every cadence, never pruned) + `poller.rs` (v2-first,
  64 KiB cap, no redirects, `PollOutcome` incl. stable `OfflineReason`) +
  `normalized.rs` (one UI type for v1/v2, checked drive aggregation).
- State/UI: `state.rs`/`action.rs` (reducer, online-first order, first-batch
  selection snap, transient 10s highlight), `ui/system_block.rs`
  (authoritative fleet-wide `[`/`]` layout), `ui/condensed.rs`, `ui/bar.rs`,
  `ui/text.rs`, `event.rs`/`input.rs`/`terminal.rs` (keys, thread, lifecycle).
- Config/CLI: `config/{model,store,validation,lock}` (atomic writes,
  `flock`/`LockFileEx`), `endpoint.rs` (explicit port required for `add`;
  HTTPS never accepted), `cli.rs` (`add/list/remove/refresh/edit/update`,
  `eggpool add/list/remove`).
- EggPool: separate `eggpool.rs` client/worker (60s passive cadence,
  Bearer from env-var name only) + `ui/eggpool.rs`; never shares greggd
  polling paths.

**Deep dive:** [gregg-client.md](gregg-client.md).

---

## Data flow

Primary (polling):

```
collector (native) → sampler (clock) → cached v1+v2 → HTTP server (axum)
                                                          │ JSON
                                                          ▼
scheduler (timer) → PollBatch (generation) → AppState (reducer) → TUI (read-only)
```

1. Collector reads kernel interfaces; sampler stamps `observed_at` and
   caches both wire shapes. The server never triggers collection.
2. The client scheduler polls each endpoint per cadence; batches carry a
   generation so stale results are rejected; the reducer updates
   reachability/selection; the TUI renders projections without I/O.

Optional EggPool path (`eggpool.rs` worker → `AppState.eggpool` pane) has its
own client, auth, cadence, and rendering — see
[gregg-client.md](gregg-client.md).

---

## Capabilities at a glance

| Area | Guarantee |
|------|-----------|
| Metrics | CPU %, mem, load (Linux/macOS), swap (Linux/macOS) vs commit (Windows), drives (`null` = unavailable, `[]` = none), freq Hz, disk/net byte rates; best-effort live telemetry, omitted when unsupported |
| Honesty | No fabricated zeroes; macOS `iowait` null, Windows load/swap/iowait null; `R/s`/`W/s`/`Rx/s`/`Tx/s` are byte rates; net util is max(Rx,Tx); loopback detail-only |
| Compatibility | `/v2/status` universal; `/v1/status` Linux/macOS only (Windows 503); client falls back v1-only on HTTP 404; additive fields ignored by old peers |
| Client UX | Explicit-port `add`; offline retried every cadence with stable reason; `Ctrl-R` is the only reload boundary; `d`/`n` independent expansions; per-system normal NET rows with fleet-wide horizontal geometry; width-degrading headers |
| Daemon ops | Foreground `run`; Unix control-socket `stop`, Windows SCM; `croncheck` watchdog spawns only on refusal; `configprint`/`status` read-only; `startup install` auto (systemd/launchd/cron/SCM); manager-aware `restart`; binary-first `update` |

---

## Tools at a glance

| Tool | Location | Purpose | Deep dive |
|------|----------|---------|-----------|
| Local check | `scripts/check-local.sh` / `.ps1` | Routine fmt + tests; `--release` adds clippy/docs/version/smoke/protocol dry-run | [scripts-and-packaging.md](scripts-and-packaging.md) |
| Release policy | `scripts/release-targets.txt`, `release-preflight.sh`, `release-check-assets.sh`, `release-install-zig.sh` | Single 5-target table + version/tag/registry preflight + asset validation | [scripts-and-packaging.md](scripts-and-packaging.md) |
| Loopback/SOAK smokes | `scripts/verify-installed-daemon.sh`, `smoke-windows.ps1` (SCM), `run-mixed-fleet-sustained.py` + `scripts/tests/` | Bounded daemon health smoke, Windows lifecycle proof, ignored sustained-workload driver | [scripts-and-packaging.md](scripts-and-packaging.md) |
| Installers | `packaging/install.sh` / `install.ps1` (bootstrap, binary-first) + legacy `install-linux.sh` / `install-macos.sh` / `install-windows.ps1`, `systemd/` unit, `launchd/` plist | Default install path; Cargo fallback for `armv7l`/unknown only | [scripts-and-packaging.md](scripts-and-packaging.md) |
| CI / release workflows | `.github/workflows/ci.yml`, `release-binaries.yml` | Linux fmt/clippy/tests + native macOS/Windows + MSRV 1.75; tag-only 5-target draft release (glibc 2.17) | [scripts-and-packaging.md](scripts-and-packaging.md) |
| User docs | `docs/{installation,daemon,client,display,api,development}.md` | Behavior-facing manuals (install, daemon, client, rendering, API) | — |
| Skills | `.opencode/skills/` (`rust-workspace`, `greggd-daemon`, `gregg-client`, `protocol-wire`, `platform-collectors`, `release-process`, `eggpool`, `architecture-docs`, `plans-workflow`) | Task-scoped agent guidance shadowing the architecture docs | matching deep dive |
| Plans | `plans/` (index: `plans/README.md`) | Sequencing + acceptance criteria; completion rule lives there | [plans-workflow skill](../.opencode/skills/plans-workflow/SKILL.md) |

---

## Cross-cutting rules (brief)

- **Workspace** (`architecture/workspace.md`): MSRV 1.75, one shared version,
  clippy pedantic as warn, `unsafe` allowlist only with safety comments,
  crates.io-only deps, load-bearing upper bounds (re-audit before removing).
- **Config**: TOML, `deny_unknown_fields`, structured violations, atomic
  temp→flush→rename→validate writes; daemon `name` → `system.name`,
  native call → `system.hostname`.
- **Errors** (`architecture/error-conventions.md`): crate-local `thiserror`
  boundaries; wire carries only category + short message; collector errors
  never leak to HTTP.
- **Testing**: unit fixtures + mock seams (`FileSource`, `MacNativeQueries`,
  `WindowsSource`), protocol JSON fixtures, TUI buffer tests, `test_support`
  builders, `lock_helper` behind `test-helper`, platform-native collector
  gates (`collector::linux` / `macos` / `windows`).

---

## Index of architecture documents

### Overview and deep dives

| Document | Scope |
|----------|-------|
| [overview.md](overview.md) | This file — bird's-eye view and component index |
| [gregg-protocol.md](gregg-protocol.md) | Protocol crate: wire types, schema versions, validation, test support |
| [gregg-update.md](gregg-update.md) | Updater crate: version/target policy, download/verify/stage/replace mechanics |
| [greggd-daemon.md](greggd-daemon.md) | Daemon crate: collectors wiring, sampler, HTTP server, CLI, service management |
| [gregg-client.md](gregg-client.md) | Client crate: CLI, polling, state engine, TUI, EggPool |
| [collectors.md](collectors.md) | Platform collectors: Linux, macOS, Windows native collection |
| [scripts-and-packaging.md](scripts-and-packaging.md) | Scripts, installers, service definitions, CI, release workflows |

### Cross-cutting decisions

| Document | Scope |
|----------|-------|
| [workspace.md](workspace.md) | Cargo workspace layout, crate boundaries, dependency direction, MSRV/lints |
| [protocol.md](protocol.md) | Wire format specification, capabilities, validation, compatibility policy |
| [error-conventions.md](error-conventions.md) | Error boundary design, wire response constraints |
| [macos-collector-notes.md](macos-collector-notes.md) | Expected macOS differences vs Activity Monitor / `top` / `vm_stat` |

### Supporting files

| Document | Scope |
|----------|-------|
| [README.md](README.md) | Directory index and purpose |
| [`../plans/`](../plans/) | Phase plans — sequencing and acceptance criteria |
| [`../AGENTS.md`](../AGENTS.md) | Compact agent instructions for this repository |
