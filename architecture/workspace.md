# Workspace and crate boundaries

The repository is a Cargo workspace with three independently publishable
members under `crates/`:

```text
crates/gregg-protocol    library    versioned wire types and compatibility rules
crates/greggd           bin + lib  Linux/macOS metrics daemon + service-management CLI (lib exposes the collector for integration tests)
crates/gregg            binary     endpoint-management CLI + polling/state engine + Ratatui TUI
```

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

The daemon's HTTP server lives under `crates/greggd/src/server/`. It serves three
read-only endpoints:

- `/` — returns the cached `StatusSnapshot` as JSON.
- `/v1/status` — identical to `/`, included for forward-compatible versioning.
- `/healthz` — returns a `HealthResponse` indicating `Ready`, `Warming`, or `Failed`.

The server serves cached immutable snapshots and never triggers metric collection.

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
a `Mutex`-based concurrency guard. Atomic writes follow the same
write-flush-rename-verify pattern as the daemon.

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
  viewport position, and generation tracking. Display order is
  online-first/offline-last while preserving configured relative order.
  Viewport helpers compute visible ranges for mixed 4-row (online) and 1-row
  (offline) entries.
- `action.rs` — `Action` enum for typed state transitions (selection navigation,
  page scrolling, config reload, resize, quit).

The `run_tui` async function in `main.rs` wires the config store, HTTP client,
scheduler, state reducer, terminal lifecycle, crossterm event stream, and
Ratatui rendering. The TUI reads `AppState` projections and renders without
performing network or filesystem I/O.

## Client TUI

The TUI lives in `crates/gregg/src/` and is composed of these modules:

- `terminal.rs` — Terminal lifecycle (raw mode, alternate screen, cursor hiding)
  with panic-hook restoration on all exit paths.
- `input.rs` — Crossterm event stream adapter reading events on a dedicated
  thread and forwarding typed `Event`s through a bounded channel.
- `ui/mod.rs` — Top-level `render()` function delegating to sub-modules.
- `ui/layout.rs` — Viewport computation: which systems are visible and their
  rect positions.
- `ui/system_block.rs` — 4-row online system rendering (header + CPU/MEM/SWP
  bars) and 1-row offline rendering.
- `ui/bar.rs` — Reusable ASCII usage bar renderer with width-safe arithmetic.
- `ui/text.rs` — Text formatting helpers (byte sizes, percentages, load
  averages, priority-aware header composition).
- `ui/diagnostics.rs` — Empty-config and terminal-too-small messages.

Rendering reads `AppState` exclusively; it performs no network or filesystem I/O.
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

A `NoopServiceManager` is provided for testing and development. External command
invocation is acceptable for service management because `systemctl`/`launchctl`
are the native administrative interfaces.

## MSRV

The workspace declares `rust-version = "1.75"` in `[workspace.package]` and
inherits it in every member manifest. Nightly-only language or Cargo features
must not be used. The Rust toolchain pinned in `rust-toolchain.toml` is the
current stable release; CI installs the same channel so formatting and lint
behaviour stay aligned with local development.

## Lints

The workspace enables `clippy::pedantic` as a warning (not an error) so that
contributors see style suggestions without breaking the build on unrelated
changes. The two binary crates and `gregg-protocol` all `#[deny(unsafe_code)]`
through the workspace lint table; the macOS collector FFI module
(`crates/greggd/src/collector/macos/ffi.rs`) is the only exception and
uses `#![allow(unsafe_code)]` with documented safety invariants.

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
