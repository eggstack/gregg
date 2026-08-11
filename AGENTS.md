# AGENTS.md

Compact instructions for AI coding agents working in this repository.

## Project structure

Three Rust crates in a workspace, strict one-way dependency direction:

```
gregg-protocol  ◄── greggd      (daemon, metrics collection, HTTP server)
gregg-protocol  ◄── gregg       (client, TUI, polling)
```

- `gregg-protocol`: shared wire types (serde, serde_json, thiserror only). **No runtime, HTTP, terminal, or platform dependencies.** `#![forbid(unsafe_code)]`
- `greggd`: metrics daemon. Exposes both `bin` and `lib` targets. Platform collectors live under `src/collector/{linux,macos,windows}/`
- `gregg`: client TUI (ratatui + crossterm). Event loop in `src/main.rs`. UI modules under `src/ui/`

`greggd` and `gregg` must never depend on each other. `gregg-protocol` must never depend on either application crate.

## Build and verify

**Fast local check (routine development loop):**

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

This runs exactly `cargo fmt --all -- --check` followed by `cargo test --workspace`.
It is the short routine loop and does not repeat native tests, build docs, or run
release checks.

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

Adds: Clippy, documentation, clean-tree and version consistency, package lists,
installed-binary v2 loopback smoke, and the protocol dry-run.

**Running a single test:**

```bash
cargo test -p gregg-protocol -- <test_name>
cargo test -p greggd --all-features -- <test_name>
```

**CI note:** GitHub Actions sets `RUSTFLAGS: -D warnings`, making all warnings
errors. Local clippy pedantic is a warning only. If CI fails on a warning that
passes locally, the distinction is the cause.

## Key constraints

- **MSRV: Rust 1.75.** Toolchain pinned in `rust-toolchain.toml` (stable channel). All member crates inherit `rust-version = "1.75"` from workspace.
- **Clippy pedantic** is a warning, not an error. Don't suppress new warnings unless fixing pre-existing ones.
- **Unsafe is heavily restricted.** Only allowed in: `crates/greggd/src/collector/linux/source.rs` (statvfs), `crates/greggd/src/collector/macos/ffi.rs` (Mach FFI), `crates/gregg/src/` (Unix flock + Windows file lock), `crates/greggd/src/collector/windows/source.rs`. Every unsafe block must have a safety comment.
- **No external command execution** for metrics collection. Use kernel interfaces (`/proc`), Mach APIs, or Windows native APIs.
- **Config writes must be atomic:** serialize to temp file, flush, rename, validate. Never leave partial writes.
- **Tests must not sleep** for production refresh intervals. Inject clocks or short intervals.
- Client polling is intentionally bounded and isolated: preserve one ordered
  result per endpoint, the semaphore limit, panic-to-`Cancelled` conversion,
  fixed periodic cadence, and cancellation behavior. EggPool commands remain
  on a separate bounded channel with generation checks; do not replace either
  state machine merely to reduce line count without a smaller behaviorally
  equivalent design.
- Reusable `greggd` library/runtime code must return errors without printing or
  calling `std::process::exit()`; the binary boundary owns logging, one-time
  diagnostics, and exit-code classification.
- Unix `greggd croncheck` is a read-only `/v2/healthz` probe with a short
  timeout and a fixed 512-byte CRLF-terminated HTTP/1.0 or HTTP/1.1 status-line
  read; only status 200 is healthy. `host` and `port` only persist config.
- `greggd` dispatches synchronously before entering Tokio: foreground `run` and
  Windows SCM `service` first enters `service_dispatcher::start`; the generated
  `ServiceMain` worker then owns exactly one current-thread runtime. Its
  selected config path comes from one process-local launch context, and it
  publishes `RUNNING` only after the shared daemon has bound its listener.
  SCM Stop and Shutdown callbacks only send a nonblocking one-shot signal into
  the shared `run_with_shutdown()` core.
- **Dependency upper bounds** are used intentionally when fresh resolution exceeds MSRV. Check `Cargo.toml` comments before changing dependency versions.

## Schema protocol

Wire types in `gregg-protocol`. Schema version is explicit (`SCHEMA_VERSION_V1 = 1`, `SCHEMA_VERSION_V2 = 2`). The client requests v2 first, accepts only the schema matching each endpoint, and falls back to v1 only on an HTTP 404 from /v2/status. `/v2/status` is the universal cross-platform endpoint. `/v1/status` is Linux/macOS only (Windows returns 503).

Platform-specific rules:
- macOS: `iowait_pct` is `null` (unsupported). Never fabricate `0.0`.
- Windows: load average, swap, iowait are all `null`/unsupported. Windows reports `commit` instead.
- Identity: `system.name` is the validated configured daemon name, while
  `system.hostname` remains the native platform hostname. Windows hostname
  collection must not retain NUL padding from `GetComputerNameExW`.
- Drives: `null` = unavailable/legacy, empty list = no eligible filesystems;
  v2 `available_bytes` is optional caller-available capacity and may not
  complement used bytes because of reservations or quotas.

Validation uses `validate()` methods returning structured violations, not serde failures.

## Crate versions and publishing

All crates inherit version from `[workspace.package]` in root `Cargo.toml`. Inter-crate dependency versions must match workspace version exactly. Publication order is mandatory: `gregg-protocol` → `greggd` → `gregg`. CI never publishes; publication is manual per `RELEASING.md`.

## Testing patterns

- **Integration tests:** `crates/gregg-protocol/tests/integration.rs`, `crates/greggd/tests/linux_collector.rs`, `crates/greggd/tests/windows_smoke.rs`
- **Fixtures:** JSON fixtures in `crates/gregg-protocol/tests/fixtures/` for v1/v2 cross-platform payloads
- **TUI tests:** `gregg` crate has `#[cfg(test)]` modules `mixed_fleet_evidence` and `sustained_workload` in `src/main.rs`
- **Test support feature:** `gregg-protocol` exposes `test_support` feature for mock builders in integration tests
- **Sustained workload tests:** `mixed_fleet_evidence` and `sustained_workload` modules in `gregg` crate are `#[cfg(test)]`-only product-validation drivers invoked by external runner scripts

## CI

GitHub Actions CI runs on push to `main` and pull requests (`.github/workflows/ci.yml`):

- **Linux**: fmt, clippy, and full workspace tests
- **macOS**: native workspace check + native macOS collector smoke (arm64 + Intel matrix)
- **Windows** (`windows-2022`): workspace tests, a release `greggd` build, and
  the bounded Administrator SCM lifecycle smoke in
  `scripts/smoke-windows.ps1`
- **MSRV**: compilation check with Rust 1.75

Local verification via the default `check-local.sh` is the source of truth for
the routine loop; release preflight is manual and nonpublishing. Ordinary CI
keeps one read-only workflow with generic Linux checks, native macOS/Windows
coverage, and one Rust 1.75 compile check. The Windows SCM smoke is the
authoritative operational proof for dispatcher startup, post-bind readiness,
service lifecycle, custom configuration paths, bind-failure recovery, and
cleanup. CI does not build documentation, publish, or upload evidence.

## What not to do

- Don't broaden scope (no process monitoring, alerting, web dashboards, plugins, TLS, auth)
- Don't add dependencies without checking existing patterns and MSRV compatibility
- Don't add `cargo publish` to any script or workflow
- Don't add automated tagging, GitHub Release creation, or publication to CI
- Don't add self-daemonization or PID-file management to the daemon
- Don't initialize a global tracing subscriber from reusable daemon runtime code;
  the binary boundary uses fallible initialization.
- Don't fabricate metric values for unsupported platform capabilities

## Files to read before implementing

1. `README.md` — public scope and command behavior
2. `plans/000-roadmap-v1.md` — sequencing and release gates
3. Active phase plan in `plans/` for current requirements
4. `architecture/protocol.md` — wire format details

## Architecture index

Deep-dive documents in `architecture/` capture decisions larger than a single crate:

| Document | Scope |
|----------|-------|
| `architecture/overview.md` | Bird's-eye view: data flow, module map, index of all documents |
| `architecture/gregg-protocol.md` | Protocol crate: wire types, schema versions, validation, test support |
| `architecture/greggd-daemon.md` | Daemon crate: collectors, sampler, HTTP server, service management |
| `architecture/gregg-client.md` | Client crate: CLI, polling, state engine, TUI, EggPool |
| `architecture/collectors.md` | Platform collectors: Linux, macOS, Windows native metric collection |
| `architecture/workspace.md` | Crate boundaries, module structure, dependency direction |
| `architecture/protocol.md` | Wire format specification, capabilities, validation, compatibility |
| `architecture/error-conventions.md` | Error boundary design, wire response constraints |
| `architecture/scripts-and-packaging.md` | Scripts, installers, service definitions |
| `architecture/macos-collector-notes.md` | macOS collector differences from Activity Monitor / top |

## Skills

Reusable agent instructions live in `.opencode/skills/`:

| Skill | Purpose |
|-------|---------|
| `rust-workspace` | Build, test, verify the workspace |
| `architecture-docs` | Read and update architecture documentation |
| `protocol-wire` | Wire types, schema versions, validation |
| `platform-collectors` | Platform-specific metric collectors |
| `release-process` | Manual release procedure |
| `eggpool` | EggPool summary pane implementation |
