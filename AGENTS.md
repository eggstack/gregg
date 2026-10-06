# AGENTS.md

Compact instructions for AI coding agents. Every line answers: "Would an agent likely miss this without help?"

Deep detail lives in `architecture/` (index: `architecture/overview.md`); sequencing and acceptance criteria live in `plans/` (index: `plans/README.md`). When a change alters user-visible behavior, update `README.md`, the affected crate README, the matching architecture deep dive, the relevant skill, and add a `CHANGELOG.md` entry in the same pass.

## Project structure

Five Rust crates, strict one-way dependencies:

```
gregg-protocol  ◄── greggd      (daemon, metrics, HTTP server)
gregg-protocol  ◄── gregg       (client, TUI, polling)
gregg-host      ◄── greggd      (native host telemetry)
gregg-update    ◄── greggd      (shared self-update mechanics)
gregg-update    ◄── gregg       (shared self-update mechanics)
```

- `gregg-protocol`: wire types only (`serde`, `serde_json`, `thiserror`). No runtime/HTTP/terminal/platform deps. `#![forbid(unsafe_code)]`
- `gregg-host`: native host telemetry (`linux/macos/windows/freebsd/`, shared rate math `src/rate.rs`, `DriveRefreshCache` slow probe). Runtime-neutral and protocol-neutral; re-exported via `greggd::collector`.
- `gregg-update`: internal binary-first self-update (version/target policy, bounded curl/Cargo, SHA-256, staging, replace). Knows nothing about service managers, TUI, EggPool, or wire protocol. Publish order is `gregg-protocol` → `gregg-update` → `gregg-host` → `greggd` → `gregg`.
- `greggd`: bin+lib daemon. Platform adapters `crates/greggd/src/collector/{linux,macos,windows}/` over `gregg-host` (no FreeBSD adapter — FreeBSD is `gregg-host`-only on FreeBSD CI; full daemon stays Linux/macOS/Windows → `architecture/collectors.md`); startup `crates/greggd/src/startup/`; read-only diagnostics `crates/greggd/src/status.rs`.
- `gregg`: TUI client (ratatui+crossterm). Event loop `crates/gregg/src/main.rs`; UI `crates/gregg/src/ui/`; config `crates/gregg/src/config/`; offline provenance `crates/gregg/src/poller.rs`.
- `greggd` and `gregg` never depend on each other. `gregg-protocol` never depends on another workspace crate. `gregg-host`/`gregg-update` never depend on app crates, service managers, or the protocol.
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
cargo test -p gregg-protocol --all-targets --all-features -- <test_name>
cargo test -p gregg-host --all-targets --all-features -- <test_name>
cargo test -p greggd --all-targets --all-features -- <test_name>
cargo test -p gregg --all-targets --all-features -- <test_name>
cargo test -p gregg-host --all-features -- linux     # Linux native
cargo test -p greggd --all-features -- collector::linux     # Linux adapter
cargo test -p gregg-host --all-features -- macos     # macOS native
cargo test -p greggd --all-features -- collector::macos     # macOS adapter
cargo test -p greggd --all-targets --all-features -- collector::windows    # Windows native
```

CI (`RUSTFLAGS: -D warnings`, so warnings fail there but not locally): Linux runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-targets --all-features`; macOS runs workspace check + `gregg-host` native + `collector::macos::ffi::native_tests` on arm64+Intel; Windows runs the same full-workspace Clippy gate *and* workspace tests (Windows-only Rust is linted natively, not cross-checked from Linux — `#[cfg(windows)]` code nobody compiles is how Plan 168 shipped a client daemon that did not build), plus release `greggd` and `gregg` builds and `scripts/smoke-windows.ps1` SCM smoke; FreeBSD runs `gregg-host` native; MSRV job runs `cargo test --workspace --all-targets --all-features` on Rust 1.89.

Cross-checking a `--target x86_64-pc-windows-gnu` Clippy run locally is a fast development aid, but **only a native Windows CI run is authority** for Windows lint/test status: MSVC needs the MSVC toolchain and named pipes need a real Windows host. A green local cross-run is not closure evidence.

## Key constraints

### Workspace (`architecture/workspace.md`)

- **MSRV 1.89.** `rust-toolchain.toml` pins stable channel; all crates inherit `rust-version = "1.89"`. Never change MSRV incidentally.
- **Clippy pedantic is warn, not error.** Don't add new warnings.
- **Unsafe allowlist only, each block needs a safety comment:** `gregg-host/src/{linux/source.rs (statvfs),macos/ffi.rs (Mach),windows/source.rs,freebsd/source.rs}`, `greggd/src/{startup/install.rs (geteuid),cli.rs (flock guard)}`, `gregg/src/{config/lock.rs,config/store.rs,bin/lock_helper.rs (flock/LockFileEx),clientd/ipc.rs (named pipe/SDDL, plus one `unsafe impl Send` for the accept wait's thread handle),cli.rs (executable probe)}`.
- **No external commands for metrics.** Use `/proc`, Mach APIs, Windows native APIs.
- **Live telemetry (freq, disk/network rates) is best-effort:** native cumulative counters + real monotonic elapsed time; reset/hotplug/unsupported re-baselines or omits that family without failing core readiness. Never fabricate zeroes; `R/s`/`W/s`/`Rx/s`/`Tx/s` are byte rates; freq is current OS-reported Hz (macOS may omit); network util is max(Rx,Tx) direction, loopback never in aggregate capacity.
- **Config writes are atomic:** temp file → flush → rename → validate. Tests never sleep production intervals — inject clocks/short intervals.
- **Deps are ordinary semver; crates.io only.** Don't re-add transitive guard pins (removed by Plan 117). `gregg` client HTTP is feature-minimal `eggfetch-core 0.2` (`standard-http1` + `tls-rustls`, no redirect/retry/Basic/proxy features) — never re-add `reqwest`/`axum`. `greggd` HTTP is EggServe 0.4 direct H1 (`eggserve-server 0.4` + `eggserve-primitives 0.2.2`, no core/static/TLS/H2/H3 compat crates).

### Client polling/state (`architecture/gregg-client.md`)

- **`gregg` is a frontend; a per-config client daemon owns remote polling.** `gregg daemon run` is a foreground process per config (identity = FNV-1a of the normalized config path, `0600` socket beside it, Windows named pipe with an owner-only SDDL). `FleetState` holds every reducer that consumes network results; `AppState` in a TUI has no such entry point and its only fleet writer is `AppState::adopt_snapshot`. There is **no direct-polling fallback** — a frontend that cannot reach a compatible daemon reports the reason and exits, because a silent fallback would double the request budget exactly when the daemon is unhealthy and would hide the failure.
- **Client-daemon fan-out is a `watch` slot, not a queue,** and each document is serialized **once** for all frontends (same discipline as greggd's scheduler cell). A slow frontend skips to the newest generation, which is safe because documents are complete. Control acks are the only non-latest-state message and ride a small per-connection channel. Frontend connection ids are stable, so a reconnecting frontend cannot leave a stale `EggPool` intent behind. **The `Hello` is always the first frame on a connection:** never publish a document before the handshake is answered, or a frontend cannot identify its daemon and refuses a healthy one.
- **Selection placement keys on first *polled reachability*, not first document.** The daemon publishes once at bind with everything `pending`, so a frontend uses `FrontendSnapshot::poll_initialized`, and the daemon publishes that one-time transition even when it changed nothing visible — otherwise the placement would fire on an unrelated later change and undo the operator's selection.
- **`EggPool` state converges, it does not race.** The worker is daemon-owned; each frontend publishes its whole `(active, period)` intent and the daemon reduces it order-independently (any-active, shortest period). Fleet state carries the **converged** period, never one frontend's request: the reducer accepts a result only on a period match, so a per-frontend value would reject everything the worker fetched and strand both panes in `Refreshing`. The last pane leaving always converges the worker to inactive. `j`/`k` records a *request* in `AppState::eggpool_period_request`; the pane only ever shows a period the daemon actually fetched.
- **`Ctrl-R` stays the only config-reload boundary** (no watcher) and is now performed by the daemon, which republishes; an invalid file keeps the last-known-good fleet and publishes a diagnostic. `gregg add`/`remove`/`refresh` nudge a running daemon best-effort and silently — absence is the common case and never fails a successful mutation. `gregg daemon status`/`stop` identify their target through the local protocol handshake, never a process name or PID file. `gregg daemon run` never forks or self-daemonizes.
- **Client-daemon lazy activation is lock-based, never PID-file-based.** The launch lock is an advisory OS lock held only across probe/spawn/readiness, its file is never unlinked or read as a signal, and the wait runs on `spawn_blocking` (a blocking sleep loop would starve the current-thread runtime a TUI runs on). **Only plain absence authorizes a spawn, and the classification is redone under the lock** — a foreign service can bind the endpoint while a launcher waits. Rotation is directional: an *older* owned daemon is replaced, a *newer* one is reported with upgrade guidance and left running.
- **Client-daemon startup is always user-scoped.** `systemctl --user`, `~/Library/LaunchAgents`, a current-user Startup-folder entry, or a managed user crontab watchdog — never a system unit, never `LocalService` SCM, never `sudo`. A root/Administrator install registers nothing. All manager execution goes through one allowlist (`startup_support`) with no shell and a bounded wait, so a config path is a path, not a command. Artifact ownership is proven by parsing what would be written and re-reading it after; `Unknown` (unreadable) is preserved exactly like `Foreign`. Artifact names embed the config identity; no global daemon registry exists.
- **Update is prepare-then-quiesce; uninstall is ownership-first.** `gregg update` identifies the daemon, prepares and verifies the candidate, and only then stops, replaces, and relaunches — a failed relaunch is reported as partial success with the retry command. `gregg uninstall --dry-run` names the startup entry and running daemon; execution stops only an identified owned daemon, removes only a provably owned entry, and **blocks executable deletion when it cannot confidently stop a running daemon**. No idle shutdown on last-TUI disconnect, ever.

- **Cron display is truthful and bounded: a load row states the relation it actually means (`load15m 9.24 > 8.00` only when load-delayed, `start ... <=` for a running job's admission gate, `last gate ...` for an idle one, `unavailable` — never `0.00` and never a comparison — when there is no reading); elapsed states read `for 3m`/`pending 17m`/`queued 2m` and countdowns read `next 11h`/`retry 20s`, never `… ago`; record clocks are labelled `Z` because they are UTC instants beside a remote-local schedule, and reconstructing scheduler-local time is a protocol change, not a renderer one. `crates/gregg/src/ui/cron.rs` builds every row once (`block_rows`) so the requested height and the emitted height cannot drift, reserves the selected job's header and newest record ahead of the job table, windows the table *around the selection* (clamped at both ends, hidden jobs counted), and marks what it could not draw with `… more cron rows not shown` — distinct from remote `stdout+` truncation. There is still one scroll model.

Remote text is inert before it reaches a cell.** `sanitize` (crate root) renders controls in `cat -v` caret notation instead of stripping them, so `ESC [ 2 J` shows as `^[[2J` rather than clearing the screen. It is applied at `AppState::adopt_snapshot` (one chokepoint covering every renderer path) **and** locally in `ui/cron.rs`. Escaping there is unbounded; viewport truncation is a render-time concern and must never shorten stored data. A missing load observation is `—`, never `0.00`; a non-child outcome is a real terminal record with `ran —`, never a fabricated `0ms`; remote truncation and viewport truncation are reported separately.
- **The scheduler plane is the client daemon's, and cron state is never reachability.** Summary on a 30s cadence (no command output); history only on first support discovery and when `history_revision` changes. Deduplicate by `(epoch, sequence)` and gate on `(epoch, revision)` — never `sequence` alone, because a restarted `greggd` reissues sequences and resets the revision. Apply history only when its `(epoch, revision)` matches the summary it was fetched with: a straddle is never merged, never advances the gate, and is reported as a scheduler-scoped `Incoherent` diagnostic with the summary kept. Republish a frontend document on **operator-visible** change (capability, job rows, `(epoch, revision)`, error), never on `history_revision` alone and never because a local attempt timestamp moved. Cron observations and gate entries are **target-bound**: carry the polled host/port, drop an observation whose target is no longer configured, clear a repointed id's state, and key the gate by `(id, normalized host, port)`. Run **one** cron round at startup (`interval_at`, not `interval`, whose first tick is already due) and observe the fleet with a fixed **four reads in flight**, never sequentially and never unbounded. An accepted reload and cancellation are selected **inside** an active round, not only between rounds: on reload the round drops its in-flight requests, the worker re-snapshots endpoints, prunes absent gate keys, and starts a fresh round immediately instead of burning through `ceil(fleet/4)` timeout waves — "reload wakes the worker immediately" means active-round preemption, and rounds never overlap. A coherent history pair advances the gate **only after** its observation has been handed to the engine (`settle` returns a pending commit; "the gate says fetched" implies the document reached a cache), never during settlement where a dropped document could suppress every later fetch. The cache is memory-only and bounded by per-job depth, a **global constant** record ceiling (evicted oldest-first, ties by `(system, job)`), and the remote output cap. Two-tier local publication over the existing socket: summary always, history only for cron-intent systems; the intent governs **transmission and never fetching**. `Unsupported` (404) is a healthy older daemon, not an error.
- Keep scheduler invariants: one ordered result per endpoint per generation, semaphore bound, panic→`Cancelled`, fixed cadence. Offline endpoints are retried every cadence, never pruned/backed-off (locked by `offline_endpoint_is_retried_and_recovers_on_next_generation` + `offline_endpoint_remains_in_scheduler_across_generations` in `crates/gregg/src/scheduler.rs`).
- `Ctrl-R` is the only config-reload boundary (no watcher): reload `ConfigStore`, reconcile stable IDs, deliver via bounded scheduler channel (full = backpressure, closed = TUI error), poll immediately; invalid reload keeps last-known-good.
- `AppState::apply_batch` snaps selection/viewport to `display_order()[0]` only on the first accepted batch (`last_applied_generation == 0`); later batches and `Ctrl-R` preserve selection. No second scroll state machine.
- Production event loop consumes owned `PollBatch`; borrowed `apply_batch(&PollBatch)` and borrowed normalization constructors are compat paths. Ordered results use positional matching with stable-ID fallback; endpoint host/port validation gates every mutation.
- EggPool control is one nonblocking retained latest desired state (`EggpoolDesiredState` active/period/generation published through `EggpoolControl::publish`). Never reintroduce a lossy bounded command queue, `try_send` drop semantics, a queued `Shutdown`, or `EggpoolStatus::Busy`; leaving the pane must always converge the worker inactive, and a closed control channel is `EggpoolWorkerState::WorkerUnavailable`. Local worker state is never EggPool proxy/provider health.
- The EggPool pane has two independent read-only planes: `/api/stats/summary` (four periodized metrics, 16 KiB cap) and EggPool schema-v1 `/api/status` (current proxy/provider health, 1 MiB per-request cap, separate reducer freshness). One worker result carries both outcomes — never collapse them, never render a transport failure as EggPool-reported health, and never add a second credential field, health cadence, or provider/account/model drill-down.

### TUI rendering (`architecture/gregg-client.md`)

- `crates/gregg/src/ui/system_block.rs` (`build_metric_rows`, `compute_fleet_metric_layout`, `render_metric_row`) is authoritative: one fleet-wide layout per render aligns `[`/`]` across all online systems' active rows; rows indent 4 spaces. NET is present only when that system's current snapshot has network telemetry, while horizontal geometry remains fleet-wide.
- `d` (drives) and `n` (network) are independent expansions; per-drive rates only on exact device match. DISK suffix is `<used> / <total>`; missing rows render `—`, never `0.0%`/fabricated zero. Compact mode drops the whole suffix fleet-wide when longest natural suffix > 1/4 terminal width; header `IO` token is omitted (not placeholder) when iowait unsupported.
- Logical `selected_id` persists; reverse-video highlight is transient (startup `false`, Systems actions arm a resettable 10s event-loop `ClearSelectionHighlight`). No frame ticker. Offline rows are `name@host:port offline` (never duplicate host) + stable category when known (`offline (refused)`, `offline (http) HTTP 503`); pending rows never carry a reason.
- Event loop redraws only after render-visible transitions; unmapped input and no-op channel wakeups do not draw. Normal metric memoization is renderer-internal, keyed by stable system ID plus a compact render key.

### CLI contracts (`architecture/gregg-client.md`, `architecture/greggd-daemon.md`)

- `gregg add` requires an explicit port (`host:port`, `[ipv6]:port`, `http://host:port/`, `nickname@host:port`); reject host-only, portless URLs, `nickname@host`, `nickname@`, inline-nickname+`--name`. HTTPS never accepted/downgraded. `gregg remove` accepts host-only. `default_port` is compat-only. Don't add implicit-port examples anywhere.
- `greggd configprint` (prints canonical bind only), `status` (version+bind+bounded `/v2/healthz` ready/warming/failed/unreachable/not-gregg+manager state, exit 0 only on valid Gregg answer), and `croncheck` (watchdog: only refusal may spawn detached `<current_exe> run`; no shells, `pkill`, PID files, service managers) are read-only/bounded. `stop` uses only the Unix control socket (`STOP\n`→`OK\n`) keyed by FNV-1a of normalized config path (never parent dir), `0600` sockets, narrow stale cleanup; Windows delegates to SCM. `run` never self-daemonizes.
- `startup install` defaults `auto` (Windows→SCM, macOS→launchd, Linux systemd-if-running else cron); systemd/launchd hosts never silently fall back to cron — print exact `sudo <exe> startup install --method …` and return `PermissionDenied`. No internal `sudo`. `restart` is exact-executable-aware: only an owned systemd/launchd/SCM registration may receive manager mutation; foreign same-config or unknown Unix ownership fails closed, foreign different-known-config registrations permit only the selected config's direct path, and Windows has no direct fallback for missing/foreign/unknown SCM. Otherwise Unix uses config-specific stop+absence-check+detached `run` and requires a valid health response, not just process spawn.
- `gregg update` / `greggd update` are binary-first, crates.io-authoritative (`curl` max-stable SemVer compare, GitHub `latest` never authoritative), exact `vX.Y.Z` asset `<program>-<target>[.exe]`+`.sha256`, bounded `curl -fsSL --max-time` to owner-private `TempDir`, `sha2`-verified before chmod/exec, candidate `version` must equal `"<program> X.Y.Z"`, staged before touching current exe (`self-replace`), only HTTP 404 falls back to `cargo install --locked --version "=X.Y.Z"`. External `curl` stays (Plan 126 measured an eggfetch in-process candidate and closed RETAIN CURL: +105% stripped `greggd`). `greggd` prepares fully before observing exact-executable lifecycle and before any stop; restarts only `ManagedRunning`/`DirectRunning` via ownership-aware `restart_daemon()`, stopped/foreign remain stopped/preserved without fabricated restart claims. Permission rerun guidance is platform-correct: Unix `sudo`, Windows names an Administrator terminal/PowerShell and never emits `sudo`. Shared mechanics live in `gregg-update` (`UpdateSpec`-parameterized); `greggd` owns activation/restart.
- `gregg uninstall` / `greggd uninstall` (`--dry-run`, `--purge`) remove only the exact invoked executable (never a directory; sibling survives) plus daemon startup artifacts classified as owned by that exact executable (systemd `ExecStart`, launchd `ProgramArguments`, managed cron command, or Windows SCM image path); foreign/unknown artifacts are preserved, and SCM query uncertainty blocks mutation. Config is preserved by default, `--purge` removes only resolved component files (never an arbitrary explicit parent); preflight before teardown, never internal `sudo`, uncertain direct stop blocks deletion, no `--all`, no receipt. Unix Cargo-owned installs complete owned startup/direct-stop lifecycle, delegate executable removal to `cargo uninstall --root`, and purge only after Cargo succeeds; Windows retains the exact zero-mutation Cargo handoff.
- Reusable `greggd` lib code returns errors (no printing/`exit()`); binary maps to exit codes `0` ok · `1` config · `2` service · `3` runtime · `4` permission.
- `greggd` publishes typed `Arc` snapshots and cached compact v1/v2 status bytes coherently under one `PublishedState` write. Status handlers evaluate current staleness/failure policy before serving cached bytes; health getters still reconstruct the source-compatible typed envelopes on demand.
- `greggd` HTTP transport sets explicit runtime limits including a 300-second total connection lifetime and 1000-request per-connection cap, and serves cached `Bytes` through a known-length stream without payload copies. Request bodies are bounded at 64 KiB; GET/HEAD route behavior is retained.
- Optional maintenance jobs run as the existing greggd principal with direct argv, never an implicit shell or injected environment; Unix euid 0 requires `allow_privileged_jobs = true`. Reuse only the sampler's cached load through a non-waking watch read, retain one pending occurrence per job and one global child slot, and keep the two-second direct-child shutdown bound inside shared cleanup. Do not replay downtime or weaken systemd/launchd/SCM sandboxing. Windows rejects load gates. Config validation must reject a schedule no Gregorian date can satisfy (DOM/DOW OR semantics respected, clock- and timezone-independent) before the listener binds; never fabricate a fallback retry date — propagate to the scheduler's fatal task boundary.
- Scheduler observability is additive and read-only on `GET/HEAD /v2/scheduler` and `GET/HEAD /v2/scheduler/history`, never merged into `/v2/status` (metrics must never carry command output). The scheduler owns a **separate** publication cell from the high-frequency metrics `PublishedState`; handlers serve already-serialized bytes and never await mutation or hold a scheduler lock. Publication happens only when externally visible state changes, so the one-minute civil-clock reconciliation wake stays silent. History is memory-only (`scheduler_history_limit`, default 5, hard max 10, `0` = no retention), non-child outcomes (`spawn_failed`, `load_expired`) are terminal records, and stdout/stderr are piped only when history is enabled and drained concurrently with the child wait by borrowing the streams (never `wait_with_output`, never spawned drain tasks, never one stream drained to EOF before the other). **The direct child's wait result is the terminal execution event**: `finished_unix_ms`/duration are frozen there, output is drained for at most one fixed non-configurable post-exit settle bound (250 ms) and then bounded with the read handles closed, so an inherited descendant writer can never retain the one global child slot — and greggd never kills descendants or claims their output. Inside that one budget stdout and stderr progress **independently**: select one pending `drain_step` per unfinished stream plus the frozen deadline, with an alternating preferred branch, so an idle open inherited writer on one stream cannot withhold bytes already available on the other and neither stream can starve the other; only bytes from a completed read are folded in, and the deadline is fixed at the exit and never restarted by a wake or per-stream progress. Terminal records are attributed through the carried configuration index, never a job-name search or index-zero fallback. Published text is bounded in **JSON-escaped** bytes so the body maximum is closed by construction. No `argv`/`working_dir` is published, but the listener is unauthenticated: document that any principal that can reach it can read job output.

### Daemon runtime / release (`architecture/greggd-daemon.md`, `architecture/scripts-and-packaging.md`)

- Dispatch synchronously before Tokio: Windows SCM `service_dispatcher::start` first, one current-thread runtime per worker, `RUNNING` only after bind; Unix SIGTERM/SIGINT, SCM Stop/Shutdown, and successful `STOP\n` share `run_with_shutdown()`; control-socket cleanup on every exit. Never init a global tracing subscriber from lib code.
- Unix non-root same-scope `greggd` bootstrap replacement captures valid default-config health before overwriting; only a previously running user-local daemon is transitioned with the new binary's config-specific `stop` + `croncheck`. First installs and stopped daemons remain stopped; prebuilt and staged-Cargo candidates share the path, and post-replacement activation failure is nonzero with an exact retry command.
- Asset contract: `gregg-<target>` / `greggd-<target>[.exe]`, targets exactly `x86_64-unknown-linux-gnu` + `aarch64-unknown-linux-gnu` (glibc 2.17 via zigbuild), `x86_64-apple-darwin`, `aarch64-apple-darwin` (unsigned), `x86_64-pc-windows-msvc.exe`, each + `.sha256`. `release-binaries.yml` (only `v*` tags/manual) verifies tag==workspace version and tag==HEAD, checks crates.io visibility, smokes `version`/`--help`/loopback, assembles a **draft** (`--clobber` on rerun, fail if published). Installers `packaging/install.sh|ps1` are binary-first, same-scope reruns replace in place with install-vs-update reporting (foreign destinations fail, never overwritten; no `PATH` search, no privilege crossing), Cargo fallback builds staged-only (private temp `--root`, verified copy, staging removed); only non-root user-local PATH persistence may make a bounded user-profile edit (system installs and unrelated shell config untouched), never silent `sudo`, never fallback on checksum/version mismatch. `packaging/uninstall-windows.ps1` is a thin `greggd uninstall` wrapper (no recursive delete).

## Schema protocol (`architecture/protocol.md`, `architecture/gregg-protocol.md`)

- Client tries `/v2/status` first, falls back to v1 only on HTTP 404 from `/v2/status`. `/v2/status` is universal; `/v1/status` is Linux/macOS only (Windows 503).
- Never fabricate: macOS `iowait_pct` null; Windows load/swap/iowait null, commit instead; `drives` null = unavailable/legacy, `[]` = none eligible; optional v2 freq/disk_io/network absent on old daemons stays absent. `system.name` = configured daemon name; `system.hostname` = native hostname. V2 caps required on all four fields; identity fields ≤512 UTF-8 bytes. `validate()` returns structured violations (V1: 9 kinds, V2: 36 kinds), not serde errors.

## Versions and testing

- All crates inherit workspace version; inter-crate dep versions must equal it. Publish order `gregg-protocol` → `gregg-update` → `gregg-host` → `greggd` → `gregg`. Ordinary CI never publishes; see `RELEASING.md`.
- Integration: `gregg-protocol/tests/integration.rs`, `greggd/tests/{linux_collector.rs,windows_smoke.rs,installer_rerun.rs}`; fixtures `gregg-protocol/tests/fixtures/` + `greggd/src/collector/test_fixtures/` (+ `live-metrics-v2.json` for optional-telemetry compat). `test_support` feature gates mock builders. `gregg` TUI drivers `mixed_fleet_evidence`/`sustained_workload` (`#[cfg(test)]`) run via `scripts/run-mixed-fleet-sustained.py` (pytest in `scripts/tests/`). `lock_helper` bin needs `test-helper` feature — plain `cargo test -p gregg` silently skips that test; use `--all-features` to run it.

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
| `architecture/collectors.md` | Linux/macOS/Windows/FreeBSD native telemetry + `greggd` adapter facade |
| `architecture/workspace.md` | Boundaries, MSRV, deps, structure |
| `architecture/protocol.md` | Wire spec, capabilities, compat |
| `architecture/error-conventions.md` | Error boundaries, wire limits |
| `architecture/scripts-and-packaging.md` | Scripts, installers, services |
| `architecture/macos-collector-notes.md` | macOS vs Activity Monitor/top differences |

## OpenCode config

No `opencode.json`/`.cursorrules`. Skills live in `.opencode/skills/` (there is no top-level `.skills/` or `skills/` directory), load via skill tool as needed: `rust-workspace`, `architecture-docs`, `plans-workflow`, `protocol-wire`, `platform-collectors`, `greggd-daemon`, `gregg-client`, `release-process`, `eggpool`.
