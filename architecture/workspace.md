# Workspace and crate boundaries

The repository is a Cargo workspace with three independently publishable
members under `crates/`:

```text
crates/gregg-protocol    library    versioned wire types and compatibility rules
crates/greggd           bin + lib  Linux/macOS/Windows metrics daemon + service-management CLI (lib exposes the collector for integration tests)
crates/gregg            binary     endpoint-management CLI + polling/state engine + Ratatui TUI
```

The `gregg` client compiles and runs natively on Windows x86-64, Linux, and macOS. The `greggd` daemon compiles and runs on Linux, macOS, and Windows x86-64.

## Dependency direction

```text
gregg-protocol  ◄── greggd
gregg-protocol  ◄── gregg
```

Allowed:

- `gregg-protocol` depends only on narrow serialization and error crates.
- `greggd` and `gregg` may each depend on `gregg-protocol`.

Forbidden:

- `gregg-protocol` depending on either binary crate.
- `greggd` depending on `gregg`, or vice versa.
- Sharing implementation code through `gregg-protocol` to avoid creating a new
  internal module in the consuming crate.

## Internal module boundaries

Within each binary crate, the following are kept separate:

- Native collection is distinct from sampling and HTTP serving.
- Service management is distinct from the foreground daemon process.
- Client polling is distinct from application-state reduction.
- The renderer reads state; it does not perform I/O or mutate polling internals.
- Platform-specific code remains under narrow `cfg(target_os = ...)` modules.

## Collector module boundary

The daemon's collector lives under `crates/greggd/src/collector/`. Platform-specific
collectors are `cfg(target_os = ...)`-gated and share the `SystemCollector` trait
defined in `collector/mod.rs`. Only one platform module is compiled per target.

## HTTP server module

The daemon's HTTP server lives under `crates/greggd/src/server/`. It serves five
read-only endpoints:

- `/` and `/v1/status` — return the cached v1 `StatusSnapshot` as JSON on
  Linux/macOS. On Windows they return `503 Service Unavailable` with a
  v1 health response because a truthful v1 snapshot cannot be produced
  (load, swap, and CPU I/O wait are absent). `/v1/status` is retained
  for compatibility with v1-only clients.
- `/v2/status` — returns the cached flat v2 `StatusPayloadV2` on every
  platform, including Windows. It contains the existing `StatusSnapshotV2`
  fields plus optional bounded drive records. This is the universal
  cross-platform status endpoint.
- `/healthz` and `/v2/healthz` — return readiness/health as compact JSON
  indicating `Ready`, `Warming`, or `Failed`.

The server serves cached immutable snapshots and never triggers metric
collection.

## Sampler module

The sampler lives under `crates/greggd/src/sampler/`. It owns the sampling cadence
and a `Clock` trait for time abstraction. The periodic sampling loop calls the
collector, computes deltas, and stamps `observed_at_unix_ms` and
`sample_interval_ms` on the resulting `StatusSnapshot`. The sampler manages the
readiness lifecycle: `Warming` until the first delta is available, then `Ready`.
On collector error the sampler transitions to `Failed`.

## Daemon entry point

The `run()` entry point in `crates/greggd/src/run.rs` wires together the collector,
sampler, HTTP server, and signal handlers (SIGTERM/SIGINT). It starts the sampler
loop, binds the HTTP listener, and performs graceful shutdown on signal receipt.

## CLI and configuration

The daemon CLI lives in `crates/greggd/src/cli.rs` and uses `clap` derive macros
for structured argument parsing. Subcommands include `run`, `start`, `stop`,
`restart`, `croncheck`, `host`, and `port`. The `run` command loads validated
TOML configuration and enters the foreground daemon loop. Lifecycle commands
delegate to the platform service manager.

Configuration lives in `crates/greggd/src/config.rs`. The `Config` struct is
serialized/deserialized via `serde` and `toml` with `deny_unknown_fields` to
prevent silent typo acceptance. Validation produces structured `ConfigViolation`
values rather than failing through serde. Atomic writes follow the
write-flush-rename-verify pattern.

## Client CLI and configuration

The client CLI lives in `crates/gregg/src/cli.rs` and uses `clap` derive macros.
Subcommands include `add`, `list`, `remove`, `refresh`, and `edit`. Running
`gregg` without a subcommand starts the TUI entry point.

Client configuration lives in `crates/gregg/src/config.rs`. It stores monitored
endpoints as `[[systems]]` entries with stable UUID v4 IDs, host, port, and
optional display name. The `ConfigStore` provides `load_or_default`,
`load_existing`, `write`, `mutate`, and `mutate_with_result` operations with
a `Mutex`-based concurrency guard plus an OS-backed cross-process file lock.
Atomic writes follow the write-flush-rename-verify pattern.

The client also supports one optional `Config::eggpool` entry through nested
`eggpool add`, `eggpool list`, and `eggpool remove` commands. Its dedicated
parser defaults to HTTP port `11300`, supports only `http` and `https`, and
stores an optional API-key environment-variable name rather than the resolved
secret. These commands perform no network or environment lookup.

Platform-specific config paths:
- Linux: `$XDG_CONFIG_HOME/gregg/gregg.toml` or `~/.config/gregg/gregg.toml`
- macOS: `~/Library/Application Support/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

Cross-process locking:
- Unix: `flock(2)` advisory lock on `<config>.lock`
- Windows: `LockFileEx` exclusive lock on `<config>.lock`
- Other platforms: in-process `Mutex` only

The endpoint parser lives in `crates/gregg/src/endpoint.rs`. It supports IPv4,
IPv6 (bracketed and bare), and DNS/mDNS hostnames with optional ports. The parser
rejects URL schemes, paths, credentials, and malformed input. Host-only removal
semantics are supported for the `remove` command.

## Client polling and state engine

The polling engine lives in `crates/gregg/src/` and is composed of five modules:

- `clock.rs` — `Clock` trait for time abstraction (enables deterministic testing
  with `FakeClock`).
- `poller.rs` — `HttpClient` wrapping a long-lived `reqwest::Client` with
  configurable timeout, 64 KiB body cap, redirect rejection, and bounded
  connection pool. `PollOutcome` classifies every failure mode (timeout,
  connection refused, DNS failure, HTTP status, body too large, decode error,
  unsupported schema, invalid snapshot, cancelled). `PollBatch` carries a
  generation counter and completed results.
- `scheduler.rs` — `PollScheduler` produces `PollBatch`es on a configurable
  interval. Concurrency is bounded by a semaphore. Generation numbers increase
  monotonically; the state reducer rejects stale batches.
- `state.rs` — `AppState` owns the system list, selection (by stable `SystemId`),
  viewport position, independent top-level pane and Systems view mode,
  transient expansion/period state, and generation tracking.
  Display order is online-first/offline-last while preserving configured
  relative order. Viewport helpers compute visible ranges for normal
  five-row-base entries and condensed one-row entries, with bounded selected
  drive detail rows in either view. Header rows are included in view-aware
  geometry; all paging, viewport, and layout calculations use the same
  state-aware height function.
- `action.rs` — `Action` enum for typed state transitions (selection navigation,
  page scrolling, transient view/drive expansion, config reload, resize, quit).

The `run_tui` async function in `main.rs` wires the config store, HTTP client,
scheduler, state reducer, terminal lifecycle, crossterm event stream, and
Ratatui rendering. The TUI reads `AppState` projections and renders without
performing network or filesystem I/O.

### EggPool summary client

The optional EggPool path lives in `crates/gregg/src/eggpool.rs` and is
deliberately separate from greggd polling. `EggpoolClient` reuses the client's
long-lived `reqwest` stack, disables redirects, sends only
`/api/stats/summary?period=...`, and caps response bodies at 16 KiB. It accepts
only the four fixed periods (`1h`, `24h`, `7d`, `30d`) and normalizes the
required fields into `EggpoolSummary`, treating a null cache ratio and zero
streamed requests as unavailable values.

Authentication is request-local: a configured environment-variable name is
resolved immediately before constructing a sensitive Bearer header and is never
stored in an outcome, result, or debug value. Stable outcomes classify missing
credentials, HTTP status, bounded-body, decoding, semantic, timeout, and
network failures without retaining response bodies or error strings.

`spawn_worker` is the only EggPool scheduler. It is created only for a
configured entry, owns at most one in-flight request, fetches on activation or
period/manual refresh, suppresses work while inactive, and ticks no faster than
once per 60 seconds while active. Generation numbers and cancellation make
superseded results safe for the reducer. `main.rs` constructs this worker only
for configured EggPool state, activates it when the pane is visible, routes
pane/period/manual-refresh actions to it, applies optional results without
affecting greggd polling, and cancels it during TUI shutdown. Configuration file
watching is not implemented; the existing `ConfigReloaded` reducer remains a
deterministic seam for future reload plumbing rather than an implied live
watcher.

## Client TUI

The TUI lives in `crates/gregg/src/` and is composed of these modules:

- `terminal.rs` — Terminal lifecycle (raw mode, alternate screen, cursor hiding)
  with panic-hook restoration on all exit paths.
- `input.rs` — Crossterm event stream adapter reading events on a dedicated
  thread and forwarding typed `Event`s through a bounded channel.
- `ui/mod.rs` — Top-level `render()` function dispatching directly on the
  active pane and delegating Systems or EggPool to small sub-modules.
- `ui/layout.rs` — Viewport computation: which systems are visible and their
  rect positions.
- `ui/system_block.rs` — five-row-base online system rendering (header +
  CPU/MEM/SWP-or-COMMIT/DISK bars), selected-system drive details, and 1-row
  offline rendering.
- `ui/condensed.rs` — fixed-tier one-row fleet rendering, condensed header and
  separator, offline/pending rows, and selected-system drive details.
- `ui/bar.rs` — Reusable ASCII usage bar renderer with width-safe arithmetic.
- `ui/text.rs` — Text formatting helpers (byte sizes, percentages, load
  averages, priority-aware header composition).
- `ui/diagnostics.rs` — Empty-config and terminal-too-small messages.
- `ui/eggpool.rs` — Pure compact pending/success/stale/error rendering for the
  optional four-value EggPool summary pane.

Rendering reads `AppState` exclusively; it performs no network or filesystem I/O.
`Pane` is independent from `SystemViewMode`; top-level cycling and
context-sensitive vertical movement are reducer actions, not a generic focus
or keymap framework. EggPool state retains only the selected period and the
latest same-period summary.
Width degradation drops lower-priority identity segments before truncating
higher-priority values. The terminal is restored on normal quit, error, signal,
and panic paths.

## Service management

The service abstraction lives in `crates/greggd/src/service/`. A `ServiceManager`
trait provides `start`, `stop`, `restart`, and `is_active` operations. Platform
adapters wrap native tools:

- `service/systemd.rs` — wraps `systemctl` with fixed argument arrays.
- `service/launchd.rs` — wraps `launchctl` with `bootstrap`, `bootout`, and
  `kickstart` flows.
- `service/windows.rs` — wraps the Windows SCM through the `windows-service`
  crate with `start_service`, `stop_service`, and `service_control_handler`.

An `UnsupportedServiceManager` provides `NotAvailable` errors for platforms without
native service integration (e.g. FreeBSD). External command invocation is
acceptable for service management because `systemctl`/`launchctl` are the native
administrative interfaces.

## MSRV

The workspace declares `rust-version = "1.75"` in `[workspace.package]` and
inherits it in every member manifest. Nightly-only language or Cargo features
must not be used. The Rust toolchain pinned in `rust-toolchain.toml` is the
current stable release; CI installs the same channel so formatting and lint
behaviour stay aligned with local development.
Compatibility-only dependency bounds keep fresh workspace and package-source
resolution within that MSRV; the local release preflight and the small MSRV CI
job provide the compatibility checks.

## Lints

The workspace enables `clippy::pedantic` as a warning (not an error) so that
contributors see style suggestions without breaking the build on unrelated
changes. The two binary crates and `gregg-protocol` all `#[deny(unsafe_code)]`
through the workspace lint table. The macOS collector FFI module
(`crates/greggd/src/collector/macos/ffi.rs`), the Windows source module
(`crates/greggd/src/collector/windows/source.rs`), and the client's narrowly scoped
Unix `flock` wrapper and Windows `LockFileEx` adapter are the only
exceptions; each uses `#![allow(unsafe_code)]` with documented safety
invariants. No unsafe pointers or borrowed foreign buffers cross those
boundaries.

## Release profiles

The workspace defines a release profile in `Cargo.toml`:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
strip = "symbols"
```

This optimises for binary size and runtime performance. Thin LTO keeps
incremental build times reasonable; `codegen-units = 1` enables better
cross-crate optimisation; symbol stripping reduces binary size.

## Supply-chain policy

`deny.toml` configures `cargo-deny` for advisory checking, licence auditing,
and dependency bans:

- **Advisories:** unmaintained crates are a workspace-level concern; yanked
  crates produce warnings.
- **Licences:** only MIT, Apache-2.0, Unicode-3.0, Unicode-DFS-2016,
  BSD-2-Clause, BSD-3-Clause, ISC, Zlib, and CDLA-Permissive-2.0 are allowed.
- **Bans:** multiple versions of the same crate produce warnings.
- **Sources:** only crates.io is permitted; unknown registries and git sources
  are denied.

## Testing strategy

The workspace enforces these checks locally and in CI:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

Platform-specific collector tests use deterministic fixtures and mock
collectors (`MockNativeQueries`) so they run on any platform. Native FFI
tests run only on macOS runners. TUI buffer tests cover narrow, medium, wide,
mixed online/offline, and resize cases without sleeping for production refresh
intervals.

The sustained workload driver (`crates/gregg/src/sustained_workload.rs`) is a
`#[cfg(test)]` ignored test that exercises the production `PollScheduler`,
`HttpClient`, and `AppState` reducer against deterministic fixture servers for a
configured duration. It validates generation invariants, online/offline
transitions, and bounded concurrency, then writes a machine-readable summary.
The external runner (`scripts/run-mixed-fleet-sustained.py`) is an optional
diagnostic for short mixed-fleet investigations; it is not part of ordinary CI
or a release-closure evidence system.
