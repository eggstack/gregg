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
