# gregg-update deep dive

The updater crate is the single authoritative implementation of binary-first
self-update mechanics shared by `gregg update` and `greggd update`
(Plan 104). It is workspace-internal infrastructure, not a user-facing
product.

**Source:** `crates/gregg-update/`

## Purpose

- Own cross-program update mechanics once: stable-version parsing/comparison,
  supported-target mapping, asset naming/URL construction, `curl`/Cargo
  discovery and bounded execution, SHA-256 verification, staged candidate
  validation, staging lifetime, executable replacement, shared error/outcome
  primitives.
- Stay caller-parameterized: each program passes
  `UpdateSpec { crate_name, program_name, current_version }`; the mechanism
  never learns which program it serves beyond names and versions.

Non-goals (stay in the callers): service-manager activation/restart policy
(`greggd`), CLI outcome presentation (each app crate), TUI/EggPool state, and
the wire protocol (`gregg-protocol` is deliberately not involved).

## Module map

| Module | File | Purpose |
|--------|------|---------|
| `lib` | `src/lib.rs` | `UpdateSpec` identity, `UpdatePlan`, `resolve_plan`, `prepare_candidate`, `cargo_fallback`, `run_simple_update`, `preflight_exe_writable`, shared `UpdateOutcome` |
| `error` | `src/error.rs` | Shared `UpdateError` taxonomy (`RestartFailed` is constructed only by `greggd` coordination) |
| `version` | `src/version.rs` | Stable `MAJOR.MINOR.PATCH` parsing/comparison (`parse_stable_version`, `compare_versions`, `is_update_available`) |
| `target` | `src/target.rs` | `SUPPORTED_TARGETS`, host mapping, asset naming, GitHub URLs; drift test against `scripts/release-targets.txt` |
| `exec` | `src/exec.rs` | `curl`/Cargo discovery, bounded child execution with kill/reap, crates.io `max_stable_version` lookup, downloads (404-only fallback signal) |
| `verify` | `src/verify.rs` | SHA-256 checksum + staged candidate `version` identity verification (`"<program> X.Y.Z"`) |
| `stage` | `src/stage.rs` | Owner-private `TempDir` staging, current-exe resolution, permission probe (`check_write_permission_for` names the caller operation), `self-replace` replacement |
| `uninstall` | `src/uninstall.rs` | Generic executable-uninstall primitives: exact current-exe resolution, shared path equivalence, writable-parent preflight with `uninstall` elevation hints, `self-replace` self-deletion, Cargo-ownership detection via `cargo install --list` confirmation (never pathname guessing), Cargo handoff/delegation |

## Contract

Both binaries share the same binary-first policy (see also
[scripts-and-packaging.md](scripts-and-packaging.md)):

1. Local version is `env!("CARGO_PKG_VERSION")`; crates.io
   `max_stable_version` (bounded `curl -fsSL --max-time`) is the authority.
   GitHub `latest` is never authoritative. Equal or newer local versions
   (`current >= latest`) exit `0` as `AlreadyCurrent` without mutating files.
2. Host mapping resolves to one of five prebuilt targets
   (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
   `x86_64-apple-darwin`, `aarch64-apple-darwin`,
   `x86_64-pc-windows-msvc`); `armv7l`/unknown go straight to Cargo fallback.
3. Exact URLs `.../releases/download/vX.Y.Z/<program>-<target>[.exe]` +
   `.sha256`; only an exact asset-URL HTTP 404 falls back to
   `cargo install --locked --version "=X.Y.Z"` staged under a private root.
   Checksum-URL 404, transport/5xx, and checksum/`version` mismatches are hard errors.
4. Download to an exclusive owner-private temp dir, verify SHA-256 (via the
   `sha2` crate) before any `chmod +x` or execution, then require the staged
   candidate's `version` output to equal `"<program> X.Y.Z"`.
5. Stage fully before touching the current exe (preflight resolves via
   `current_exe_path()`; replacement is `self_replace(candidate)` against the
   implicit current exe — Unix same-filesystem atomic rename where practical,
   Windows running-image semantics — preserving symlink targets, never
   overwriting the symlink file itself). Unix uses same-filesystem atomic rename via `self-replace`; Windows uses
   the same helper for running-image semantics. Never elevate internally;
   permission failures surface exit `4` with an exact platform-correct rerun
   hint: `sudo <exe> update` on Unix, or the exact executable/operation from
   an Administrator terminal/PowerShell on Windows.

## Caller split

- `crates/gregg/src/update.rs` — thin CLI adapter: binds the client identity
  and delegates the full flow to `run_simple_update`, preserving exact outcome
  strings (`AlreadyCurrent` / `UpdatedBinary` / `UpdatedFromCargo`).
- `crates/greggd/src/update.rs` — lifecycle coordinator: binds the daemon
  identity, permission-probes before download, prepares via
  `prepare_candidate`, observes exact-executable `UpdateLifecycle` only after
  full preparation (Unix manager ownership + selected health; Windows SCM
  `query_registration()` revalidated immediately before quiescence, owned
  running/start-pending only may stop, owned stop-pending waits stopped
  without restart, foreign/unknown/not-installed do zero SCM mutation),
  replaces, then restarts only `ManagedRunning`/`DirectRunning` through
  exact-executable-aware `restart_daemon()`. Stopped/foreign stay
  stopped/preserved without fabricated restart claims. Successful replacement with
  failed restart is `UpdatedButRestartFailed` with the exact restart command
  and nonzero exit.

## Target table

`scripts/release-targets.txt` is the single machine-readable target source.
`target.rs::SUPPORTED_TARGETS` is checked against it by unit test, and
`scripts/release-check-assets.sh`, `packaging/install.sh|ps1`, and
`.github/workflows/release-binaries.yml` derive from the same table. Add a
target in all consumers at once, never in one place alone.

## Key constraints

- No dependency on app crates, service managers, TUI, EggPool, or protocol.
- External `curl` remains the update transport (Plan 126 experiment closed
  RETAIN CURL: the parity-complete eggfetch 0.2 candidate required the broad
  `http1` alias plus a first TLS stack in the daemon and grew stripped
  `greggd` 2,432,408 → 4,989,488 bytes, ~20× the adoption gate, so it was
  reverted; `exec::tests::curl_baseline` now locks the curl
  redirect/404/hard-failure/capture contract against local fixtures).
- Uninstall stays in the same boundary: the shared crate owns only generic
  executable operations (resolution, path-equivalence, preflight, self-delete,
  bin-layout candidate + `cargo install --list` Cargo confirmation — pathnames
  only select the candidate root, never ownership alone). Startup teardown
  (systemd `ExecStart`, launchd `ProgramArguments`, cron, SCM image-path
  parsing) and config/data removal live beside their existing owners in each
  application crate. No install receipt is kept.
- Bounded execution everywhere: crates.io 15s / 256 KiB, download 90s
  (100s wall) / 64 MiB, capture/probe/candidate 20s/20s/5s, Cargo 600s,
  `cargo --list` 30s, owner-private `TempDir 0700`, partial-file removal,
  kill/reap (no orphaned compilers), no predictable shared-temp pathnames.
  `exit 4` mapping is caller-owned; the shared crate returns
  `PermissionDenied` with a platform-correct hint.
- Publish order: `gregg-protocol` → `gregg-update` → `greggd` → `gregg`.
