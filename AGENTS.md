# AGENTS.md

Compact instructions for AI coding agents working in this repository.
Every line answers: "Would an agent likely miss this without help?"

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
- `gregg` `Ctrl-R` on the Systems pane is the explicit config reload boundary:
  reload the already-resolved client `ConfigStore`, reconcile stable system IDs,
  reliably deliver the replacement through the existing bounded scheduler
  command channel, and poll immediately. A full channel applies backpressure;
  a closed scheduler receiver is returned through the TUI error boundary rather
  than silently diverging state from the scheduler. Invalid reloads preserve
  the last-known-good state; there is no filesystem watcher. EggPool refresh
  behavior remains pane-local.
- `gregg add` requires an explicit port on every accepted form. Accepted:
  `host:port`, `[ipv6]:port`, `http://host:port/`, and `nickname@host:port`.
  Rejected: host-only (`host`, `192.168.182.146`, `::1`), HTTP URL
  without a port, `nickname@host` without a port, `nickname@`, and the
  ambiguous combination of inline `nickname@` with `--name`. HTTPS is
  never accepted and is not downgraded to HTTP. `gregg remove` still
  accepts host-only input. Persisted fields remain normalized `host`
  and `port`; the inline `nickname@` form just populates the existing
  `SystemEntry.name` field. `default_port` remains in the configuration
  schema for compatibility but is not used by `gregg add`. Do not introduce
  implicit-port `gregg add` examples anywhere in the repo.
- The shared normal-view metric-row geometry in `crates/gregg/src/ui/system_block.rs`
  (via `MetricRow`, `build_metric_rows`, `compute_metric_group_layout`,
  and `render_metric_row`) is the authoritative path for the four
  CPU/MEM/SWP-or-COMMIT/DISK rows. All four rows share one `bar_width`
  so the opening `[` and closing `]` columns align at every supported
  terminal width. Metric rows are indented by exactly four spaces.
  Unavailable rows render `—` instead of fabricating a `0.0%`.
  Offline endpoints render as `name@host:port offline` or
  `host:port offline`; the host is never duplicated when a name is set.
- `AppState::apply_batch` snaps `selected_id` and `viewport_top_id` to
  `display_order()[0]` only on the **first** accepted poll batch (when
  `last_applied_generation == 0` before applying). Subsequent batches
  preserve ordinary selection/viewport semantics. `Ctrl-R` does not
  re-snap. Do not add a new scroll state machine for this.
- Offline endpoints continue to be polled on every configured cadence
  (no backoff/retry queue). The scheduler always returns one ordered
  result per endpoint per generation; the new
  `offline_endpoint_is_retried_and_recovers_on_next_generation` and
  `offline_endpoint_remains_in_scheduler_across_generations` tests in
  `crates/gregg/src/scheduler.rs` lock that invariant in.
- `greggd configprint` is read-only and prints only the configured canonical
  bind `host:port`; it must not probe, bind, mutate config, or manage services.
- Reusable `greggd` library/runtime code must return errors without printing or
  calling `std::process::exit()`; the binary boundary owns logging, one-time
  diagnostics, and exit-code classification.
- `greggd croncheck` is a watchdog for non-systemd supervisors (cron,
  Task Scheduler, etc.). It opens a bounded TCP connect to the configured
  local bind address (`127.0.0.1:port`, with wildcards normalized to
  loopback). If a listener accepts the connection, it exits silently with
  status `0`. If nothing is listening, it spawns `<current_exe> run`
  (passing `--config PATH` when an explicit path was given) as a detached
  child with stdio closed to `/dev/null` and, on Unix, in a new process
  group so signals sent to croncheck's group do not reach the daemon. It
  must not invoke `systemctl`, `launchctl`, `pkill`, `killall`, shell
  commands, or PID-file management. The HTTP API is not consulted.
  `host` and `port` only persist config.
- `greggd stop` on Linux/macOS targets only the local foreground `greggd`
  instance associated with the resolved config identity via a single tiny
  Unix-domain control socket (`STOP\n` -> `OK\n`). The control identity is
  derived from a deterministic FNV-1a digest of the normalized config path:
  existing files use filesystem canonicalization so relative, absolute, and
  symlink spellings converge; a missing implicit default uses a lexical
  absolute fallback. It is never derived from the config parent directory
  alone, so two configs in the same directory cannot cross-stop. The control socket is created with
  restrictive `0600` permissions; a failed `chmod` discards the candidate
  and tries the next legitimate one. Stale socket cleanup unlinks only
  after metadata confirms a socket and the connect result classifies as
  `ConnectionRefused` or `NotFound`; `PermissionDenied` and unexpected
  connect errors never authorize unlinking. It must not invoke
  `systemctl`, `launchctl`, `pkill`, `killall`, shell commands, PID-file
  management, or process-name scanning. The HTTP API remains read-only.
  Windows `greggd stop` continues to delegate to the native SCM manager.
- `greggd` dispatches synchronously before entering Tokio: foreground `run` and
  Windows SCM `service` first enters `service_dispatcher::start`; the generated
  `ServiceMain` worker then owns exactly one current-thread runtime. Its
  selected config path comes from one process-local launch context, and it
  publishes `RUNNING` only after the shared daemon has bound its listener.
  SCM Stop and Shutdown callbacks only send a nonblocking one-shot signal into
  the shared `run_with_shutdown()` core. The same shutdown path is reused on
  Unix for SIGTERM/SIGINT and a successful `STOP\n` over the local control
  socket; the control socket is cleaned up on every exit path including
  SIGINT/SIGTERM and the runtime-error cleanup.
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
2. `architecture/overview.md` — bird's-eye view, data flow, module map, and index of all architecture documents
3. `plans/000-roadmap-v1.md` — sequencing and release gates
4. Active phase plan in `plans/` for current requirements
5. `architecture/protocol.md` — wire format details

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

## OpenCode config

No `opencode.json` or `.cursorrules` exists. Skills live in `.opencode/skills/`
and are loaded via the skill tool as needed.

## Skills

Reusable agent instructions live in `.opencode/skills/`:

| Skill | Purpose |
|-------|---------|
| `rust-workspace` | Build, test, verify the workspace |
| `architecture-docs` | Read and update architecture documentation |
| `protocol-wire` | Wire types, schema versions, validation |
| `platform-collectors` | Platform-specific metric collectors |
| `gregg-client` | Client crate: TUI, polling, state engine, CLI |
| `release-process` | Manual release procedure |
| `eggpool` | EggPool summary pane implementation |

Use the skill tool to load a skill when a task matches its description.
