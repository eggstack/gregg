# AGENTS.md

Compact instructions for AI coding agents. Every line answers: "Would an agent likely miss this without help?"

Deep detail lives in `architecture/` (index: `architecture/overview.md`); sequencing and acceptance criteria live in `plans/` (index: `plans/README.md`). When a change alters user-visible behavior, update `README.md`, the affected crate README, the matching architecture deep dive, the relevant skill, and add a `CHANGELOG.md` entry in the same pass.

## Project structure

Four Rust crates, strict one-way dependencies:

```
gregg-protocol  ◄── greggd      (daemon, metrics, HTTP server)
gregg-protocol  ◄── gregg       (client, TUI, polling)
gregg-update    ◄── greggd      (shared self-update mechanics)
gregg-update    ◄── gregg       (shared self-update mechanics)
```

- `gregg-protocol`: wire types only (`serde`, `serde_json`, `thiserror`). No runtime/HTTP/terminal/platform deps. `#![forbid(unsafe_code)]`
- `gregg-update`: internal binary-first self-update (version/target policy, bounded curl/Cargo, SHA-256, staging, replace). Knows nothing about service managers, TUI, EggPool, or wire protocol. Publishable; order is `gregg-protocol` → `gregg-update` → `greggd` → `gregg`.
- `greggd`: bin+lib daemon. Collectors `src/collector/{linux,macos,windows}/`; shared rate math `src/collector/rate.rs`; startup `src/startup/`; read-only diagnostics `src/status.rs`.
- `gregg`: TUI client (ratatui+crossterm). Event loop `src/main.rs`; UI `src/ui/`; config `src/config/`; offline provenance `src/poller.rs`.
- `greggd` and `gregg` never depend on each other. `gregg-protocol` never depends on another workspace crate. `gregg-update` never depends on app crates, service managers, or the protocol.
→ `architecture/workspace.md`

## Build and verify

Routine loop (fmt + tests only; no clippy/docs/release checks):

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
./scripts/check-local.sh --release  # preflight: +clippy -D warnings, docs, clean-tree, version/package checks, installed-binary smoke, protocol dry-run
```

Single / focused tests (mirror CI flags when touching that area):

```bash
cargo test -p gregg-protocol -- <test_name>
cargo test -p greggd --all-features -- <test_name>
cargo test -p gregg -- <test_name>
cargo test -p greggd --all-features -- collector::linux     # Linux native
cargo test -p greggd --all-features -- collector::macos     # macOS native
cargo test -p greggd --all-targets -- collector::windows    # Windows native
```

CI (`RUSTFLAGS: -D warnings`, so warnings fail there but not locally): Linux runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-targets --all-features`; macOS runs workspace check + `collector::macos::ffi::native_tests` on arm64+Intel; Windows runs workspace tests + release `greggd` build + `scripts/smoke-windows.ps1` SCM smoke; MSRV job runs `cargo check --workspace --all-features` on Rust 1.75.

## Key constraints

### Workspace (`architecture/workspace.md`)

- **MSRV 1.75.** `rust-toolchain.toml` pins stable channel; all crates inherit `rust-version = "1.75"`. Never raise MSRV incidentally.
- **Clippy pedantic is warn, not error.** Don't add new warnings.
- **Unsafe allowlist only, each block needs a safety comment:** `greggd/src/collector/{linux/source.rs (statvfs),macos/ffi.rs (Mach),windows/source.rs}`, `greggd/src/startup/install.rs` (`geteuid`), `gregg/src/` (flock/LockFileEx, `cli.rs` executable probe).
- **No external commands for metrics.** Use `/proc`, Mach APIs, Windows native APIs.
- **Live telemetry (freq, disk/network rates) is best-effort:** native cumulative counters + real monotonic elapsed time; reset/hotplug/unsupported re-baselines or omits that family without failing core readiness. Never fabricate zeroes; `R/s`/`W/s`/`Rx/s`/`Tx/s` are byte rates; freq is current OS-reported Hz (macOS may omit); network util is max(Rx,Tx) direction, loopback never in aggregate capacity.
- **Config writes are atomic:** temp file → flush → rename → validate. Tests never sleep production intervals — inject clocks/short intervals.
- **Dep upper bounds are load-bearing for 1.75** (see `architecture/workspace.md` Plan 105 audit). Re-audit with relax + 1.75 check before removing any bound.

### Client polling/state (`architecture/gregg-client.md`)

- Keep scheduler invariants: one ordered result per endpoint per generation, semaphore bound, panic→`Cancelled`, fixed cadence. Offline endpoints are retried every cadence, never pruned/backed-off (locked by `offline_endpoint_is_retried_and_recovers_on_next_generation` + `offline_endpoint_remains_in_scheduler_across_generations` in `crates/gregg/src/scheduler.rs`).
- `Ctrl-R` is the only config-reload boundary (no watcher): reload `ConfigStore`, reconcile stable IDs, deliver via bounded scheduler channel (full = backpressure, closed = TUI error), poll immediately; invalid reload keeps last-known-good.
- `AppState::apply_batch` snaps selection/viewport to `display_order()[0]` only on the first accepted batch (`last_applied_generation == 0`); later batches and `Ctrl-R` preserve selection. No second scroll state machine.

### TUI rendering (`architecture/gregg-client.md`)

- `crates/gregg/src/ui/system_block.rs` (`build_metric_rows`, `compute_fleet_metric_layout`, `render_metric_row`) is authoritative: one fleet-wide layout per render aligns `[`/`]` across all online systems; rows indent 4 spaces. NET row appears fleet-wide if any online snapshot has it (`—` for legacy systems; all-legacy keeps 4-row height).
- `e` (drives) and `n` (network) are independent expansions; per-drive rates only on exact device match. DISK suffix is `<used> / <total>`; missing rows render `—`, never `0.0%`/fabricated zero. Compact mode drops the whole suffix fleet-wide when longest natural suffix > 1/4 terminal width; header `IO` token is omitted (not placeholder) when iowait unsupported.
- Logical `selected_id` persists; reverse-video highlight is transient (startup `false`, Systems actions arm a resettable 10s event-loop `ClearSelectionHighlight`). No frame ticker. Offline rows are `name@host:port offline` (never duplicate host) + stable category when known (`offline (refused)`, `offline (http) HTTP 503`); pending rows never carry a reason.

### CLI contracts (`architecture/gregg-client.md`, `architecture/greggd-daemon.md`)

- `gregg add` requires an explicit port (`host:port`, `[ipv6]:port`, `http://host:port/`, `nickname@host:port`); reject host-only, portless URLs, `nickname@host`, `nickname@`, inline-nickname+`--name`. HTTPS never accepted/downgraded. `gregg remove` accepts host-only. `default_port` is compat-only. Don't add implicit-port examples anywhere.
- `greggd configprint` (prints canonical bind only), `status` (version+bind+bounded `/v2/healthz` ready/warming/failed/unreachable/not-gregg+manager state, exit 0 only on valid Gregg answer), and `croncheck` (watchdog: only refusal may spawn detached `<current_exe> run`; no shells, `pkill`, PID files, service managers) are read-only/bounded. `stop` uses only the Unix control socket (`STOP\n`→`OK\n`) keyed by FNV-1a of normalized config path (never parent dir), `0600` sockets, narrow stale cleanup; Windows delegates to SCM. `run` never self-daemonizes.
- `startup install` defaults `auto` (Windows→SCM, macOS→launchd, Linux systemd-if-running else cron); systemd/launchd hosts never silently fall back to cron — print exact `sudo <exe> startup install --method …` and return `PermissionDenied`. No internal `sudo`. `restart` is manager-aware (`systemctl`/`launchctl kickstart -k`/SCM else stop+absence-check+detached `run`) and requires a valid health response, not just process spawn.
- `gregg update` / `greggd update` are binary-first, crates.io-authoritative (`curl` max-stable SemVer compare, GitHub `latest` never authoritative), exact `vX.Y.Z` asset `<program>-<target>[.exe]`+`.sha256`, bounded `curl -fsSL --max-time` to owner-private `TempDir`, `sha2`-verified before chmod/exec, candidate `version` must equal `"<program> X.Y.Z"`, staged before touching current exe (`self-replace`), only HTTP 404 falls back to `cargo install --locked --version "=X.Y.Z"`. `greggd` prepares fully before stopping (Windows SCM stop failure blocks replace); restarts only if running/managed. Shared mechanics live in `gregg-update` (`UpdateSpec`-parameterized); `greggd` owns activation/restart.
- `gregg uninstall` / `greggd uninstall` (`--dry-run`, `--purge`) remove only the exact invoked executable (never a directory; sibling survives) plus daemon startup artifacts classified as owned by that exact executable (systemd `ExecStart`, launchd `ProgramArguments`, managed cron command, or Windows SCM image path); foreign/unknown artifacts are preserved, and SCM query uncertainty blocks mutation. Config is preserved by default, `--purge` removes only resolved component files (never an arbitrary explicit parent); preflight before teardown, never internal `sudo`, uncertain direct stop blocks deletion, no `--all`, no receipt. Unix Cargo-owned installs complete owned startup/direct-stop lifecycle, delegate executable removal to `cargo uninstall --root`, and purge only after Cargo succeeds; Windows retains the exact zero-mutation Cargo handoff (never pathname-guessed, never parsed metadata).
- Reusable `greggd` lib code returns errors (no printing/`exit()`); binary maps to exit codes `0` ok · `1` config · `2` service · `3` runtime · `4` permission.

### Daemon runtime / release (`architecture/greggd-daemon.md`, `architecture/scripts-and-packaging.md`)

- Dispatch synchronously before Tokio: Windows SCM `service_dispatcher::start` first, one current-thread runtime per worker, `RUNNING` only after bind; Unix SIGTERM/SIGINT, SCM Stop/Shutdown, and successful `STOP\n` share `run_with_shutdown()`; control-socket cleanup on every exit. Never init a global tracing subscriber from lib code.
- Asset contract: `gregg-<target>` / `greggd-<target>[.exe]`, targets exactly `x86_64-unknown-linux-gnu` + `aarch64-unknown-linux-gnu` (glibc 2.17 via zigbuild), `x86_64-apple-darwin`, `aarch64-apple-darwin` (unsigned), `x86_64-pc-windows-msvc.exe`, each + `.sha256`. `release-binaries.yml` (only `v*` tags/manual) verifies tag==workspace version and tag==HEAD, checks crates.io visibility, smokes `version`/`--help`/loopback, assembles a **draft** (`--clobber` on rerun, fail if published). Installers `packaging/install.sh|ps1` are binary-first, same-scope reruns replace in place with install-vs-update reporting (foreign destinations fail, never overwritten; no `PATH` search, no privilege crossing), Cargo fallback builds staged-only (private temp `--root`, verified copy, staging removed); never edit shell rc, never silent `sudo`, never fallback on checksum/version mismatch. `packaging/uninstall-windows.ps1` is a thin `greggd uninstall` wrapper (no recursive delete).

## Schema protocol (`architecture/protocol.md`, `architecture/gregg-protocol.md`)

- Client tries `/v2/status` first, falls back to v1 only on HTTP 404 from `/v2/status`. `/v2/status` is universal; `/v1/status` is Linux/macOS only (Windows 503).
- Never fabricate: macOS `iowait_pct` null; Windows load/swap/iowait null, commit instead; `drives` null = unavailable/legacy, `[]` = none eligible; optional v2 freq/disk_io/network absent on old daemons stays absent. `system.name` = configured daemon name; `system.hostname` = native hostname. V2 caps required on all four fields; identity fields ≤512 UTF-8 bytes. `validate()` returns structured violations (V1: 9 kinds, V2 base: 16 + telemetry/identity bounds), not serde errors.

## Versions and testing

- All crates inherit workspace version; inter-crate dep versions must equal it. Publish order `gregg-protocol` → `gregg-update` → `greggd` → `gregg`. Ordinary CI never publishes; see `RELEASING.md`.
- Integration: `gregg-protocol/tests/integration.rs`, `greggd/tests/{linux_collector.rs,windows_smoke.rs}`; fixtures `gregg-protocol/tests/fixtures/` + `greggd/src/collector/test_fixtures/` (+ `live-metrics-v2.json` for optional-telemetry compat). `test_support` feature gates mock builders. `gregg` TUI drivers `mixed_fleet_evidence`/`sustained_workload` (`#[cfg(test)]`) run via `scripts/run-mixed-fleet-sustained.py` (pytest in `scripts/tests/`). `lock_helper` bin needs `test-helper` feature — plain `cargo test -p gregg` silently skips that test; use `--all-features` to run it.

## What not to do

- No scope broadening (process monitoring, alerting, dashboards, plugins, TLS, auth); no new deps without MSRV + pattern check; no `cargo publish`/auto-tag/auto-publish in scripts/workflows; no self-daemonization or PID files; no fabricated metrics.

## Plans workflow

Work is plan-driven under `plans/`. Register new plans in `plans/README.md`, close truthfully against acceptance criteria, never rewrite closed history (append corrections). → `plans-workflow` skill + `plans/README.md` completion rule.

## Read before implementing

1. `README.md` — scope and command behavior
2. `architecture/overview.md` — data flow, module map, doc index
3. `plans/README.md` — roadmap status, completion rule
4. Active phase plan in `plans/`
5. `architecture/protocol.md` — wire format

## Architecture index

| Document | Scope |
|----------|-------|
| `architecture/overview.md` | Bird's-eye view, data flow, index |
| `architecture/README.md` | Directory index and purpose |
| `architecture/gregg-protocol.md` | Wire types, validation, test support |
| `architecture/gregg-update.md` | Shared self-update mechanics, targets, staging |
| `architecture/greggd-daemon.md` | Collectors, sampler, server, service mgmt |
| `architecture/gregg-client.md` | CLI, polling, state, TUI, EggPool |
| `architecture/collectors.md` | Linux/macOS/Windows collection |
| `architecture/workspace.md` | Boundaries, MSRV, deps, structure |
| `architecture/protocol.md` | Wire spec, capabilities, compat |
| `architecture/error-conventions.md` | Error boundaries, wire limits |
| `architecture/scripts-and-packaging.md` | Scripts, installers, services |
| `architecture/macos-collector-notes.md` | macOS vs Activity Monitor/top differences |

## OpenCode config

No `opencode.json`/`.cursorrules`. Skills live in `.opencode/skills/` (there is no top-level `.skills/` or `skills/` directory), load via skill tool as needed: `rust-workspace`, `architecture-docs`, `plans-workflow`, `protocol-wire`, `platform-collectors`, `greggd-daemon`, `gregg-client`, `release-process`, `eggpool`.
