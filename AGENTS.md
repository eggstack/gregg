# AGENTS.md

Compact instructions for AI coding agents working in this repository.
Every line answers: "Would an agent likely miss this without help?"

Deep design detail lives in `architecture/` (index: `architecture/overview.md`);
phase sequencing and acceptance criteria live in `plans/` (index:
`plans/README.md`). This file stays compact: constraints plus pointers. When a
change alters user-visible behavior, update `README.md`, the affected crate
README, the matching architecture deep dive, the relevant skill, and add a
`CHANGELOG.md` entry in the same pass.

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
→ Details and boundary rules: `architecture/workspace.md`

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
cargo test -p gregg -- <test_name>
```

**CI note:** GitHub Actions sets `RUSTFLAGS: -D warnings`, making all warnings
errors. Local clippy pedantic is a warning only. If CI fails on a warning that
passes locally, the distinction is the cause.

## Key constraints

### Workspace-wide (`architecture/workspace.md`)

- **MSRV: Rust 1.75.** Toolchain pinned in `rust-toolchain.toml` (stable channel). All member crates inherit `rust-version = "1.75"` from workspace.
- **Clippy pedantic** is a warning, not an error. Don't suppress new warnings unless fixing pre-existing ones.
- **Unsafe is heavily restricted.** Only allowed in: `crates/greggd/src/collector/linux/source.rs` (statvfs), `crates/greggd/src/collector/macos/ffi.rs` (Mach FFI), `crates/gregg/src/` (Unix flock + Windows file lock), `crates/greggd/src/collector/windows/source.rs`. Every unsafe block must have a safety comment.
- **No external command execution** for metrics collection. Use kernel interfaces (`/proc`), Mach APIs, or Windows native APIs.
- **Config writes must be atomic:** serialize to temp file, flush, rename, validate. Never leave partial writes.
- **Tests must not sleep** for production refresh intervals. Inject clocks or short intervals.
- **Dependency upper bounds** are used intentionally when fresh resolution exceeds MSRV. Check `Cargo.toml` comments before changing dependency versions.

### Client polling and state (`architecture/gregg-client.md`)

- Client polling is intentionally bounded and isolated: preserve one ordered
  result per endpoint per generation, the semaphore limit, panic-to-`Cancelled`
  conversion, fixed periodic cadence, and cancellation behavior. EggPool commands remain on a separate bounded channel with generation checks; do not replace either state machine merely to reduce line count without a smaller behaviorally equivalent design.
- Offline endpoints continue to be polled on every configured cadence
  (no backoff/retry queue); they are never pruned or suppressed by reachability.
  The `offline_endpoint_is_retried_and_recovers_on_next_generation` and
  `offline_endpoint_remains_in_scheduler_across_generations` tests in
  `crates/gregg/src/scheduler.rs` lock that invariant in.
- Systems-pane `Ctrl-R` is the explicit config reload boundary: reload the
  resolved client `ConfigStore`, reconcile stable system IDs, reliably deliver
  the replacement through the bounded scheduler command channel, and poll
  immediately. A full channel applies backpressure; a closed receiver returns
  through the TUI error boundary; invalid reloads preserve last-known-good
  state. There is no filesystem watcher. EggPool refresh remains pane-local.
- `AppState::apply_batch` snaps `selected_id` and `viewport_top_id` to
  `display_order()[0]` only on the **first** accepted poll batch
  (`last_applied_generation == 0` before applying). Later batches preserve
  ordinary selection/viewport semantics; `Ctrl-R` does not re-snap. Do not add
  a second scroll state machine for this.

### TUI rendering (`architecture/gregg-client.md`)

- The shared normal-view metric-row geometry in
  `crates/gregg/src/ui/system_block.rs` (`MetricRow`, `build_metric_rows`,
  `compute_fleet_metric_layout`, `resolve_system_suffixes`, `render_metric_row`)
  is authoritative for the four CPU/MEM/SWP-or-COMMIT/DISK rows. One fleet-wide
  layout per render keeps `[`/`]` columns aligned across every online system,
  including while scrolling; rows are indented exactly four spaces.
- The DISK aggregate suffix is `<used bytes> / <total bytes>` so the slash
  denominator matches the percentage; explicit caller-available capacity
  (`available_bytes`) is preserved in the normalized model and surfaced only
  through expanded drive detail rows. Unavailable rows render `—`, never a
  fabricated `0.0%`.
- Compact-mode policy (per render, no persisted state): when the longest natural
  suffix across the online fleet exceeds one quarter of terminal width, the whole
  suffix region disappears fleet-wide (`MetricFleetLayout::show_suffix = false`);
  the normal-header `IO` token is omitted entirely when I/O-wait is unsupported
  or has no real value — never a placeholder.
- Logical vs visual selection are separate: `selected_id` persists and drives
  `e` and viewport behavior; `selection_highlight_active` is transient reverse
  video. Startup leaves the highlight `false`; selection-changing Systems
  actions arm a resettable ten-second event-loop deadline that dispatches
  `Action::ClearSelectionHighlight` without touching `selected_id`. Do not add
  a periodic frame ticker or per-keypress background task.
- Offline rows render `name@host:port offline` or `host:port offline`; the host
  is never duplicated when a name is set.

### CLI contracts (`architecture/gregg-client.md`, `architecture/greggd-daemon.md`)

- `gregg add` requires an explicit port on every accepted form. Accepted:
  `host:port`, `[ipv6]:port`, `http://host:port/`, and `nickname@host:port`.
  Rejected: host-only (`host`, `192.168.182.146`, `::1`), HTTP URL without a
  port, `nickname@host` without a port, `nickname@`, and the ambiguous
  combination of inline `nickname@` with `--name`. HTTPS is never accepted and
  is not downgraded to HTTP. `gregg remove` still accepts host-only input.
  Persisted fields remain normalized `host` and `port`; the inline `nickname@`
  form just populates the existing `SystemEntry.name` field. `default_port`
  remains in the configuration schema for compatibility but is not used by
  `gregg add`. Do not introduce implicit-port `gregg add` examples anywhere in
  the repo.
- `greggd configprint` is read-only and prints only the configured canonical
  bind `host:port`; it must not probe, bind, mutate config, or manage services.
- `greggd croncheck` is a watchdog for non-systemd supervisors: it probes the
  configured local `/v2/healthz` endpoint with bounded raw HTTP (wildcards become
  loopback). Valid Gregg Ready/Warming/Failed responses mean running; refusal
  alone permits spawning detached `<current_exe> run`. Unrelated, malformed,
  silent, or ambiguous peers return nonzero without spawning. It must not invoke
  service managers, shells, `pkill`/`killall`, or PID-file management; `host`/`port`
  subcommands only persist config.
- `greggd stop` (Linux/macOS) targets only the local instance matching the
  resolved config identity via one tiny Unix-domain control socket
  (`STOP\n` → `OK\n`). Identity is an FNV-1a digest of the normalized config
  path (canonicalized for existing files, lexical absolute fallback for a
  missing implicit default) — never the parent directory alone, so two configs
  in one directory cannot cross-stop. Sockets are created `0600`; stale-socket
  cleanup unlinks only after metadata confirms a socket and connect fails with
  `ConnectionRefused` or `NotFound`. No service managers, shells, process-name
  scanning, or PID files; the HTTP API stays read-only. Windows delegates to
  SCM.
- `greggd startup install` (`auto` default; `--method systemd|launchd|cron` explicit) installs automatic startup: systemd uses `/usr/local/bin/greggd`, `/etc/gregg/greggd.toml`, `greggd` user/group, `/etc/systemd/system/greggd.service` (atomic, `daemon-reload` + `enable` + `start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist`; cron uses an idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab, never edits `/var/spool/cron` directly, prints manual lines if `crontab` missing). Auto picks Windows→SCM, macOS→launchd, Linux with running systemd→systemd, else cron. An identified systemd/launchd host never silently falls back to cron on permission failure; prints exact `sudo <exe> startup install --method <...>` and returns `PermissionDenied`. No internal `sudo`.
- `greggd startup instructions` (`--method` optional) is read-only and prints exact commands/paths for the selected method without mutating state.
- `greggd restart` is manager-aware and reusable by `update`: Windows via SCM, systemd via `systemctl restart greggd`, launchd via `launchctl kickstart -k`, otherwise via control `stop` + detached `run` (same primitive as `croncheck`); privilege failures print exact elevated `systemctl`/`launchctl` command and return `PermissionDenied` without competing fallback.
- Reusable `greggd` library/runtime code returns errors without printing or
  calling `std::process::exit()`; the binary boundary owns logging, one-time
  diagnostics, and exit-code classification (`0` success · `1` configuration ·
  `2` service management · `3` runtime · `4` permission denied).

### Daemon runtime ownership (`architecture/greggd-daemon.md`)

- `greggd` dispatches synchronously before entering Tokio: Windows SCM
  `service` enters `service_dispatcher::start` first; the generated
  `ServiceMain` worker owns exactly one current-thread runtime and publishes
  `RUNNING` only after the shared daemon binds its listener. SCM Stop/Shutdown
  send a nonblocking one-shot signal into the shared `run_with_shutdown()`
  core, which is also reused for Unix SIGTERM/SIGINT and a successful `STOP\n`;
  control-socket cleanup runs on every exit path.

### Release binaries and bootstrap installers (`architecture/scripts-and-packaging.md`, `plans/099-*`)

- Release binaries use a single public asset contract: `gregg-<target>` /
  `greggd-<target>[.exe]` where `<target>` is exactly one of
  `x86_64-unknown-linux-gnu` (glibc 2.17 via cargo-zigbuild), `aarch64-unknown-linux-gnu` (2.17,
  covers 64-bit Raspberry Pi/Le Potato), `x86_64-apple-darwin`,
  `aarch64-apple-darwin` (unsigned), `x86_64-pc-windows-msvc.exe`; every
  executable has a `<asset>.sha256`. Installers must use the same suffixes.
- The release-only workflow `.github/workflows/release-binaries.yml` triggers
  only on `v*` tags and manual dispatch, verifies tag `vX.Y.Z` == workspace
  version and tag points at HEAD, checks crates.io visibility for `gregg`/`greggd`,
  builds the five targets (Linux with Zig 2.17 floor, macOS native, Windows
  native), runs `version`/`--help` plus a loopback `greggd` smoke before
  hashing, and assembles a **draft** GitHub Release via `gh` (`--clobber` on
  rerun, hard failure if already published). It never calls `cargo publish`,
  `git tag`, or pushes commits.
- Bootstrap installers `packaging/install.sh` (Unix) and `packaging/install.ps1`
  (Windows) are binary-first, Cargo second: map `uname -s`/`uname -m` (and
  Windows `PROCESSOR_ARCHITECTURE`/Is64Bit) to the contract target, construct
  `https://github.com/eggstack/gregg/releases/latest/download/<asset>` or
  `.../download/vX.Y.Z/<asset>` for `--version X.Y.Z`, `curl -fsSL` to a fresh
  `mktemp -d`, fetch `<asset>.sha256`, verify (`sha256sum`/`shasum -a 256` or
  `Get-FileHash`), `chmod +x` and `<candidate> version` must equal the expected
  program/version, trap cleanup, install to `/usr/local/bin` (root) or
  `$HOME/.local/bin` (`%ProgramFiles%\Gregg` vs `%LOCALAPPDATA%\Gregg` on
  Windows, preserving `%ProgramData%\gregg\greggd.toml`), warn when the dest
  is not on `PATH`, never edit shell rc files, never silently invoke `sudo`,
  never fallback on checksum/version mismatch, and only `armv7l`/unknown hosts
  fall back to `cargo install --locked` (with `="X.Y.Z"` when pinned).

## Schema protocol

Wire types live in `gregg-protocol`. Full contract: `architecture/protocol.md`
and `architecture/gregg-protocol.md`.

- Schema version is explicit (`SCHEMA_VERSION_V1 = 1`, `SCHEMA_VERSION_V2 = 2`).
  The client requests v2 first, accepts only the schema matching each endpoint,
  and falls back to v1 only on an HTTP 404 from `/v2/status`. `/v2/status` is
  the universal cross-platform endpoint; `/v1/status` is Linux/macOS only
  (Windows returns 503).
- Platform truth rules (never fabricate values):
  - macOS: `iowait_pct` is `null` (unsupported).
  - Windows: load average, swap, iowait are `null`/unsupported; commit is reported instead.
  - Identity: `system.name` is the validated configured daemon name;
    `system.hostname` remains the native platform hostname (no NUL padding from
    `GetComputerNameExW` on Windows).
  - Drives: `null` = unavailable/legacy, empty list = no eligible filesystems;
    v2 `available_bytes` is optional caller-available capacity and may not
    complement used bytes because of reservations or quotas.
- Validation uses `validate()` methods returning structured violations, not
  serde failures. V1 has 9 violation kinds; V2 has 16 (9 from V1 + 7 additional).
  V2 capability objects require all four explicit capability fields, and every
  system identity field is limited to 512 UTF-8 bytes.

## Crate versions and publishing

All crates inherit version from `[workspace.package]` in root `Cargo.toml`. Inter-crate dependency versions must match workspace version exactly. Publication order is mandatory: `gregg-protocol` → `greggd` → `gregg`. Ordinary CI never publishes; the tagged `release-binaries` workflow may create/update a **draft** GitHub Release from prebuilt binaries after manual `cargo publish` + tag, but never publishes crates or auto-publishes the release. See `RELEASING.md`.

## Testing patterns

- **Integration tests:** `crates/gregg-protocol/tests/integration.rs`, `crates/greggd/tests/linux_collector.rs`, `crates/greggd/tests/windows_smoke.rs`
- **Fixtures:** JSON fixtures in `crates/gregg-protocol/tests/fixtures/` for v1/v2 cross-platform payloads; ~46 text fixtures under `crates/greggd/src/collector/test_fixtures/`
- **TUI tests:** `gregg` crate has `#[cfg(test)]` modules `mixed_fleet_evidence` and `sustained_workload` declared in `src/lib.rs` (separate files `src/mixed_fleet_evidence.rs` and `src/sustained_workload.rs`). `src/main.rs` has its own inline `#[cfg(test)]` module.
- **Test support feature:** `gregg-protocol` exposes `test_support` feature for mock builders in integration tests
- **Sustained workload tests:** the `mixed_fleet_evidence` and `sustained_workload` modules are `#[cfg(test)]`-only product-validation drivers invoked by the external runner `scripts/run-mixed-fleet-sustained.py`; that runner has its own pytest suite in `scripts/tests/`
- **`lock_helper` second bin:** `gregg` also builds `src/bin/lock_helper.rs`, but only with the `test-helper` feature (`required-features = ["test-helper"]`). The cross-process config-lock test in `src/config.rs` silently skips when the binary is absent — plain `cargo test -p gregg` skips it; `--all-features` builds and runs it
- **`probe_top` dev bin:** `gregg` always builds `src/bin/probe_top.rs` (auto-discovered from `src/bin/`, no required features). It is a standalone TCP-connectivity probe driven by `PROBE_HOST`/`PROBE_PORT` env vars, not part of the product CLI — don't mistake it for shipped functionality

## CI

GitHub Actions CI runs on push to `main` and pull requests (`.github/workflows/ci.yml`):

- **Linux**: fmt, clippy, and full workspace tests
- **macOS**: native workspace check + native macOS collector smoke (arm64 + Intel matrix)
- **Windows** (`windows-2022`): workspace tests, a release `greggd` build, and
  the bounded Administrator SCM lifecycle smoke in
  `scripts/smoke-windows.ps1`
- **MSRV**: compilation check with Rust 1.75

The release-only workflow `.github/workflows/release-binaries.yml` runs only
on `v*` tags / manual dispatch and builds the five release targets
(Linux x86_64/AArch64 glibc 2.17, macOS Intel/ARM64, Windows x86_64) into a
draft release. See above and `architecture/scripts-and-packaging.md`.

Local verification via the default `check-local.sh` is the source of truth for
the routine loop; release preflight is manual and nonpublishing. Ordinary CI
keeps one read-only workflow with generic Linux checks, native macOS/Windows
coverage, and one Rust 1.75 compile check. The Windows SCM smoke is the
authoritative operational proof for dispatcher startup, post-bind readiness,
service lifecycle, custom configuration paths, bind-failure recovery, and
cleanup. CI does not build documentation, publish crates, or upload evidence
beyond the release draft assets.
→ Details: `architecture/scripts-and-packaging.md`

## What not to do

- Don't broaden scope (no process monitoring, alerting, web dashboards, plugins, TLS, auth)
- Don't add dependencies without checking existing patterns and MSRV compatibility
- Don't add `cargo publish` to any script or workflow
- Don't add automated tagging, GitHub Release creation, or publication to CI
  (the release workflow's draft creation from prebuilt binaries is the one
  narrow exception and must never publish crates, auto-publish the draft, or
  push tags/commits)
- Don't add self-daemonization or PID-file management to the daemon
- Don't initialize a global tracing subscriber from reusable daemon runtime code;
  the binary boundary uses fallible initialization.
- Don't fabricate metric values for unsupported platform capabilities

## Plans workflow

Implementation work is plan-driven under `plans/`. Register new plans in
`plans/README.md`, close them truthfully against their acceptance criteria,
and never rewrite a closed plan's history (append corrections instead).
→ See the `plans-workflow` skill and `plans/README.md` (completion rule,
verification model, per-plan status table).

## Files to read before implementing

1. `README.md` — public scope and command behavior
2. `architecture/overview.md` — bird's-eye view, data flow, module map, and index of all architecture documents
3. `plans/README.md` — plan index, roadmap status, completion rule
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
| `plans-workflow` | Create, register, and close phase plans under `plans/` |
| `protocol-wire` | Wire types, schema versions, validation |
| `platform-collectors` | Platform-specific metric collectors |
| `greggd-daemon` | Daemon crate: runtime wiring, control socket, croncheck/configprint/stop, SCM service |
| `gregg-client` | Client crate: TUI, polling, state engine, CLI |
| `release-process` | Manual release procedure |
| `eggpool` | EggPool summary pane implementation |

Use the skill tool to load a skill when a task matches its description.
