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
   GitHub `latest` is never authoritative. Equal version exits `0` without
   mutating files.
2. Host mapping resolves to one of five prebuilt targets
   (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
   `x86_64-apple-darwin`, `aarch64-apple-darwin`,
   `x86_64-pc-windows-msvc`); `armv7l`/unknown go straight to Cargo fallback.
3. Exact URLs `.../releases/download/vX.Y.Z/<program>-<target>[.exe]` +
   `.sha256`; only HTTP 404 falls back to
   `cargo install --locked --version "=X.Y.Z"` staged under a private root.
   Transport/5xx/checksum/`version` mismatches are hard errors.
4. Download to an exclusive owner-private temp dir, verify SHA-256 (via the
   `sha2` crate) before any `chmod +x` or execution, then require the staged
   candidate's `version` output to equal `"<program> X.Y.Z"`.
5. Stage fully before touching the current exe (`current_exe()`-derived
   destination; symlinks replace the resolved target and are preserved).
   Unix uses same-filesystem atomic rename via `self-replace`; Windows uses
   the same helper for running-image semantics. Never `sudo` internally;
   permission failures surface exit `4` with an exact `sudo <exe> update`
   hint.

## Caller split

- `crates/gregg/src/update.rs` — thin CLI adapter: binds the client identity
  and delegates the full flow to `run_simple_update`, preserving exact outcome
  strings (`AlreadyCurrent` / `UpdatedBinary` / `UpdatedFromCargo`).
- `crates/greggd/src/update.rs` — lifecycle coordinator: binds the daemon
  identity, permission-probes before download, prepares via
  `prepare_candidate`, quiesces a running Windows SCM service only after full
  preparation (prepare-before-quiesce rule), replaces, then restarts through
  detected-manager policy (`restart_with_state`). Restarts only when
  running/managed; stopped services stay stopped. Successful replacement with
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
- Uninstall stays in the same boundary: the shared crate owns only generic
  executable operations (resolution, path equivalence, preflight, self-delete,
  Cargo ownership). Startup teardown and config/data removal live beside their
  existing owners in each application crate. No install receipt is kept;
  provenance is exact `current_exe()` identity plus parsed canonical artifact
  targets plus Cargo confirmation.
- Bounded execution everywhere: `curl --max-time`, build deadlines with
  kill/reap (no orphaned compilers), no predictable shared-temp pathnames.
- Publish order: `gregg-protocol` → `gregg-update` → `greggd` → `gregg`.
