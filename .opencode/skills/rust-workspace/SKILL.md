---
name: rust-workspace
description: Build, test, and verify the gregg Rust workspace
---

## What I do

Guide agents through building, testing, and verifying changes to the gregg workspace.

## When to use me

Use this when making code changes, adding features, or fixing bugs in any of the three crates.

## Build and verify

Run before every commit:

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

This runs in order: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-targets --all-features`, `cargo doc --workspace --no-deps`, then platform-native collector tests.

**Platform-native collector tests (run separately):**

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
- **Dependency upper bounds** are used intentionally when fresh resolution exceeds MSRV. Check `Cargo.toml` comments before changing dependency versions.

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
