---
name: rust-workspace
description: Build, test, and verify the gregg Rust workspace
---

## What I do

Guide agents through building, testing, and verifying changes to the gregg workspace.

## When to use me

Use this when making code changes, adding features, or fixing bugs in any of the three crates.

## Build and verify

Routine local check:

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

This runs exactly `cargo fmt --all -- --check` and `cargo test --workspace`.
It is intentionally fast and does not repeat native collector tests.

Run the comprehensive manual preflight before a release:

The release preflight adds full Clippy, documentation, package/version checks,
installation smoke, and the protocol-only publish dry-run. It is manual and
nonpublishing.

**Platform-native collector tests (run separately when focused coverage is needed):**

```bash
cargo test -p greggd --all-features -- collector::linux     # Linux
cargo test -p greggd --all-features -- collector::macos     # macOS
cargo test -p greggd --all-targets -- collector::windows    # Windows
```

**Release preflight (non-publishing):**

```bash
./scripts/check-local.sh --release
```

Adds: clean-tree check, version consistency, package lists, installed-binary v2 loopback smoke, protocol dry-run.

**Running a single test:**

```bash
cargo test -p gregg-protocol -- <test_name>
cargo test -p greggd --all-features -- <test_name>
```

## Key constraints

- **MSRV: Rust 1.75.** Toolchain pinned in `rust-toolchain.toml` (stable channel). All member crates inherit `rust-version = "1.75"` from workspace.
- **Clippy pedantic** is a warning, not an error. Don't suppress new warnings unless fixing pre-existing ones.
- **Unsafe is heavily restricted.** Only allowed in: `crates/greggd/src/collector/linux/source.rs` (statvfs), `crates/greggd/src/collector/macos/ffi.rs` (Mach FFI), `crates/gregg/src/` (Unix flock + Windows file lock), `crates/greggd/src/collector/windows/source.rs`. Every unsafe block must have a safety comment.
- **No external command execution** for metrics collection. Use kernel interfaces (`/proc`), Mach APIs, or Windows native APIs.
- **Config writes must be atomic:** serialize to temp file, flush, rename, validate. Never leave partial writes.
- **Tests must not sleep** for production refresh intervals. Inject clocks or short intervals.
- The client scheduler intentionally uses isolated per-endpoint tasks plus a
  semaphore: retain ordered one-result-per-endpoint batches, panic isolation,
  fixed cadence, and bounded cancellation unless a replacement is materially
  smaller without weakening those guarantees.
- The optional EggPool worker intentionally uses bounded command/result
  channels and generation checks. Evaluate latest-state channels only when
  they reduce both production concepts and focused test coverage.
- **Dependency upper bounds** are used intentionally when fresh resolution exceeds MSRV. Check `Cargo.toml` comments before changing dependency versions.
- Daemon runtime and CLI library paths return errors; only the `greggd` binary
  boundary prints diagnostics, initializes tracing with `try_init()`, and maps
  errors to exit codes.
- The binary selects command mode before creating Tokio. Foreground `run` and
  Windows SCM `service` each own one current-thread runtime; SCM callbacks must
  signal shutdown without blocking or entering a runtime.

## Workspace structure

Three Rust crates in a workspace, strict one-way dependency direction:

```
gregg-protocol  ◄── greggd      (daemon, metrics collection, HTTP server)
gregg-protocol  ◄── gregg       (client, TUI, polling)
```

- `gregg-protocol`: shared wire types (serde, serde_json, thiserror only). `#![forbid(unsafe_code)]`
- `greggd`: metrics daemon. Exposes both `bin` and `lib` targets.
- `gregg`: client TUI (ratatui + crossterm). Event loop in `src/main.rs`.

`greggd` and `gregg` must never depend on each other. `gregg-protocol` must never depend on either application crate.

## What not to do

- Don't broaden scope (no process monitoring, alerting, web dashboards, plugins, TLS, auth)
- Don't add dependencies without checking existing patterns and MSRV compatibility
- Don't add `cargo publish` to any script or workflow
- Don't add automated tagging, GitHub Release creation, or publication to CI
- Don't add self-daemonization or PID-file management to the daemon
- Don't fabricate metric values for unsupported platform capabilities
