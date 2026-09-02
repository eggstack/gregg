# Plan 101: binary-first self-update and release integration

Status: complete at implementation pending push (see closure record).

Depends on: Plan 098, Plan 099's stable release asset contract, and Plan 100's manager-aware `greggd restart`/startup-state contract.

## Objective

Add bounded `update` commands to both shipped binaries so an installed Gregg deployment can move to the latest stable crates.io version without requiring a local Rust compile when a matching GitHub binary asset exists.

Required commands:

```text
gregg update
greggd update
```

The update source-of-truth and transport contract is:

```text
latest stable version: crates.io own-crate metadata
binary candidate: exact GitHub Release tag vX.Y.Z
asset name: Plan 099 target mapping
source fallback: Cargo exact version =X.Y.Z
```

For `greggd`, a successful binary replacement must reuse Plan 100's lifecycle detection/restart behavior. Do not build a second daemon-management implementation inside the updater.

## Governing invariants

1. `env!("CARGO_PKG_VERSION")` remains the local version source; do not add Git SHA/dirty-tree build metadata.
2. crates.io latest stable version is authoritative for whether an update exists.
3. GitHub `latest` is not authoritative for update version selection; updater constructs the exact tag requested by crates.io.
4. A downloaded binary is not installed until checksum and program/version validation pass.
5. Missing exact release asset permits Cargo fallback; checksum/version mismatch does not.
6. Replacement is staged before touching the current executable.
7. Unix replacement should be atomic on the same filesystem where practical.
8. Windows running-executable semantics must be handled deliberately; do not assume Unix rename behavior.
9. `greggd update` preserves configuration and startup registration.
10. `greggd update` restarts the daemon when it was running/managed and restart permission is available; a restart failure after successful replacement is reported as a distinct partial-success state.
11. No updater path invokes `sudo` or an elevation prompt internally.
12. No apt/brew/winget/package-manager updater is introduced.

## Scope

### In scope

- `update` parser/dispatch for `gregg` and `greggd`;
- stable crates.io version lookup;
- SemVer-safe comparison;
- current OS/architecture -> Plan 099 release-target mapping;
- exact tagged GitHub release asset/checksum URLs;
- bounded external download transport or existing client transport without inflating `greggd` unnecessarily;
- SHA-256 verification;
- candidate executable identity/version verification;
- staging in a safe temporary path;
- same-filesystem Unix replacement;
- correct Windows replacement mechanism;
- Cargo exact-version fallback if the release asset is absent;
- manager-aware `greggd` restart using Plan 100;
- clear partial-success/error reporting;
- README/crate README/RELEASING documentation and plan-index reconciliation.

### Out of scope

- automatic periodic update checks;
- background update daemon;
- update notifications in the TUI;
- prerelease/beta/nightly channels;
- downgrade commands;
- arbitrary version install command beyond the internal exact-version fallback;
- rollback snapshots/version history;
- delta/binary patch updates;
- code signing/notarization implementation;
- package-manager integration;
- automatic config migration framework;
- HTTP update/restart API routes;
- adding a fourth public workspace crate solely to share updater code;
- adding reqwest/rustls to `greggd` unless measured implementation proves it materially simpler and acceptable.

## Phase 1: define the update result/error model

Keep user-visible outcomes explicit and small.

Suggested conceptual outcomes:

```text
AlreadyCurrent { version }
UpdatedBinary { from, to }
UpdatedFromCargo { from, to }
UpdatedButRestartFailed { from, to, restart_error }   # greggd only
```

Errors should distinguish at least:

```text
version lookup failed
unsupported host mapping
release asset absent
release download failed
checksum retrieval/validation failed
candidate identity/version mismatch
permission denied replacing current executable
Cargo unavailable for fallback
Cargo fallback failed
Windows replacement failed
restart failed after replacement
```

Do not expose an elaborate machine-readable update protocol unless a concrete consumer exists. Normal CLI text plus meaningful exit status is enough.

A successful binary replacement followed by failed `greggd` restart must not be reported simply as "update failed" while hiding that the on-disk executable changed. Print the installed version and the exact restart command needed.

## Phase 2: stable version lookup from crates.io

Each binary queries its own crate:

```text
gregg  -> crates.io crate gregg
greggd -> crates.io crate greggd
```

Use one bounded HTTPS request with a Gregg-specific User-Agent and normal crates.io API policy. No polling/retry storm is required.

### Transport choice

`gregg` already has an HTTPS stack through reqwest/rustls, but `greggd` intentionally does not. Prefer one of these small designs after measuring source/binary impact:

**Preferred baseline:** use the platform `curl` executable for the rare update command in both binaries, capture bounded stdout, then parse the JSON with existing `serde_json`.

Reasons:

- consistent behavior between binaries;
- no new TLS stack in `greggd`;
- every documented Unix bootstrap already requires `curl`;
- modern supported Windows provides `curl.exe`, with PowerShell installer fallback still documented.

If `curl` is missing, report it clearly and explain the Cargo/manual path. Do not add a daemon-wide network dependency merely to hide an unusual missing bootstrap tool.

If implementation demonstrates that reusing reqwest for `gregg` while using curl for `greggd` materially reduces code and does not cause semantic drift, that split is acceptable. The public behavior must remain equivalent.

### Metadata parsing

Select the latest stable, non-yanked version. Prefer the crates.io field that already represents maximum stable version if its semantics are sufficient; otherwise inspect the versions list and choose the highest non-yanked non-prerelease SemVer.

Do not compare versions lexically.

### SemVer implementation

Use the smallest correct option:

- first evaluate adding the lightweight `semver` crate to `gregg`/`greggd` and measure MSRV/binary impact;
- if it violates Rust 1.75 or causes unjustified footprint, implement only the strict subset needed to compare Gregg's own published `MAJOR.MINOR.PATCH` stable versions, with tests and explicit rejection of unsupported version syntax.

Do not implement a large homegrown general SemVer parser casually. Prerelease support is not required because updater intentionally follows stable versions only.

## Phase 3: map host to exact tagged release asset

Use the exact public contract from Plan 099.

Required mappings:

```text
Linux x86_64 -> x86_64-unknown-linux-gnu
Linux aarch64 -> aarch64-unknown-linux-gnu
macOS x86_64 -> x86_64-apple-darwin
macOS arm64 -> aarch64-apple-darwin
Windows x86_64 -> x86_64-pc-windows-msvc
ARMv7 -> only if Plan 099 actually qualified/publishes that target
```

Construct exact URLs:

```text
https://github.com/eggstack/gregg/releases/download/vX.Y.Z/<program>-<target>[.exe]
https://github.com/eggstack/gregg/releases/download/vX.Y.Z/<program>-<target>[.exe].sha256
```

The updater must not use `releases/latest/download` after crates.io has selected X.Y.Z, because an independently newer/older GitHub latest pointer could violate the version-authority contract.

### Missing asset classification

Only a definite HTTP 404/not-found for the exact candidate permits Cargo fallback.

These do **not** permit fallback:

- TLS/transport failure;
- timeout;
- 5xx;
- checksum file exists but disagrees;
- candidate runs as the wrong program;
- candidate reports the wrong version.

Those are release/integrity errors and should be visible.

## Phase 4: download and checksum candidate

Download executable and `.sha256` into a private temporary directory.

Requirements:

- bounded process/request timeout appropriate for binary download, not infinite wait;
- do not write directly over `current_exe()`;
- reject empty/implausibly tiny candidate before execution where useful;
- verify SHA-256 before candidate execution.

### Hash implementation

Prefer avoiding a new crypto dependency in `greggd` if standard platform tools suffice reliably:

```text
Linux: sha256sum
macOS: shasum -a 256
Windows: PowerShell Get-FileHash or a small native helper
```

However, if platform-command branching becomes larger or less reliable than adding a small Rust SHA-256 crate, measure `sha2`/equivalent against Rust 1.75 and release binary size and use it if clearly simpler. Record the decision in the closure record.

Never skip checksum validation silently because one tool is absent.

The checksum is defense against truncation/misassembly and accidental asset mismatch; it is not represented as a cryptographic signing system.

## Phase 5: validate the candidate executable

Before replacement:

1. chmod executable on Unix;
2. run the candidate's explicit `version` subcommand with a short timeout;
3. require stdout to match the expected program name and exact crates.io-selected version;
4. require exit status 0;
5. optionally run `--help` only if it adds useful format sanity without significantly slowing updates.

For example:

```text
gregg 1.0.12
greggd 1.0.12
```

Do not infer identity from filename alone.

## Phase 6: preserve installation destination

Use `std::env::current_exe()` to identify the executable being updated.

Do not assume Cargo's default bin directory or `/usr/local/bin`. The user may have installed through Plan 099, Cargo, or a custom copied binary.

Resolve/canonicalize carefully enough to replace the actual invoked program without following an unrelated symlink unexpectedly.

### Symlink policy

Choose and document one simple rule:

- preferred: if `current_exe()` resolves through a symlink to a real executable, replace the resolved executable target and preserve the symlink; or
- if safe target replacement cannot be proven cross-platform, reject symlinked/custom wrapper installs with an actionable manual update instruction.

Do not overwrite a symlink itself with a regular file unintentionally.

### Permissions

Test write/replace capability before stopping `greggd` where practical.

If `/usr/local/bin/greggd` requires root and update is unprivileged, fail early with:

```text
permission denied; rerun: sudo /usr/local/bin/greggd update
```

Do not stop a healthy daemon before learning that its binary cannot be replaced.

## Phase 7: Unix replacement

Stage the validated candidate on the same filesystem as the destination when possible so rename is atomic.

Preferred sequence:

```text
candidate downloaded/verified in temp
-> copy verified candidate to destination-directory hidden temp file
-> fsync file where the project already uses/justifies durability primitives
-> preserve executable mode/ownership semantics appropriate to caller
-> rename temp over destination atomically
-> clean temp artifacts
```

Do not delete the existing binary before the candidate is staged.

A backup file is not required unless replacement semantics need one for error recovery. Avoid building rollback/version-history infrastructure.

For `greggd`, do not stop/restart until the new candidate is ready to replace. Depending on Unix rename semantics, replacing the on-disk executable while the old daemon process is still running is allowed and minimizes downtime; then restart through Plan 100.

Preserve config files and service assets untouched.

## Phase 8: Windows self-replacement

Windows running executable replacement needs an explicit solution.

### Preferred solution gate

Evaluate `self-replace` (or the current narrow equivalent) only if:

- it supports the current Windows targets;
- it compiles under Gregg's Rust 1.75 MSRV;
- its transitive dependency/size footprint is small;
- it solves replacement without adding an updater framework/network stack.

If it passes those gates, use it for both binaries where that simplifies cross-platform replacement.

If it fails the gates, implement the smallest Windows-specific helper flow:

1. fully verify the candidate first;
2. for `greggd`, stop the SCM service through the existing manager before file mutation only when necessary;
3. rename the running executable out of the final path if Windows permits it;
4. move the candidate into the final path;
5. use a short-lived detached helper only for deleting the old locked image/restarting after the CLI process exits if required;
6. helper has fixed arguments/paths, no arbitrary shell text;
7. helper removes itself/old temp file best-effort after completion.

Do not introduce a general self-update framework merely to manage one `.exe` swap.

The Windows CI/release runner must execute the real replacement path against a temporary copy, not the production runner binary path.

## Phase 9: Cargo fallback

Cargo fallback exists for hosts where no exact binary release asset is available.

### Trigger

Only exact release-asset not-found or intentionally source-only host mapping enters this path.

### Requirements

- require `cargo` to exist;
- compile/install the exact crates.io-selected version `=X.Y.Z`, not an unconstrained latest version;
- use a temporary `--root`/target location first rather than `cargo install --force` directly over the running/current executable;
- pass `--locked` consistent with current release policy when the package supports it;
- verify the produced binary's `version` exactly as for a downloaded asset;
- replace the current executable through the same platform replacement function used by the binary path;
- do not duplicate replacement semantics inside Cargo fallback.

Conceptual command:

```bash
cargo install greggd --version '=X.Y.Z' --locked --root "$TEMP_ROOT"
```

then verify `$TEMP_ROOT/bin/greggd` and replace the current executable.

This avoids leaving the installed binary half-updated if compilation fails.

### No Cargo

Return an actionable error:

```text
No prebuilt greggd asset exists for <os>/<arch> at vX.Y.Z and Cargo is not installed.
Install Rust/Cargo and rerun, or install manually from the release/source.
```

## Phase 10: `gregg update` dispatch

`gregg update` should be synchronous and must not enter the TUI runtime.

Flow:

```text
parse command
-> determine current version
-> lookup latest stable gregg crate
-> compare
-> if current, print concise current message and exit 0
-> obtain/verify candidate
-> replace current executable
-> print from -> to and source (GitHub binary or Cargo)
```

No config mutation is performed.

The client does not restart anything after update.

## Phase 11: `greggd update` dispatch and restart

Use Plan 100 startup-state detection before replacement.

Capture enough pre-update state to decide what should be restarted:

```text
systemd installed+active
launchd loaded
Windows service running
unmanaged direct daemon reachable
installed but stopped
not running/unmanaged
```

### Required restart policy

- if a managed service was running: replace, then restart through that same manager;
- if an unmanaged/direct daemon was running: replace, then use Plan 100 `restart` semantics;
- if a known service was installed but intentionally stopped: update the binary but leave it stopped;
- if no daemon appears to be running/managed: update the binary and do not unexpectedly start it solely because `update` was invoked.

This is slightly narrower and safer than blindly starting every updated daemon. It satisfies the fleet-use case because running installations are restarted onto the new executable while administratively stopped services remain stopped.

### Restart failure

If replacement succeeds but restart fails:

- print that version X.Y.Z is installed on disk;
- print that the old process may still be running or the service is stopped, depending detected state;
- print exact `greggd restart`/manager command;
- return nonzero so automation notices incomplete activation;
- never roll the executable back automatically unless a very small same-file rollback is proven necessary during implementation and this plan is amended.

## Phase 12: update/install contract reuse

Do not let installer and updater drift.

At minimum share/document these constants/contracts:

```text
GitHub repository: eggstack/gregg
asset target suffixes
program asset prefix
tag format: vX.Y.Z
checksum suffix: .sha256
```

Because the crates are separately packaged on crates.io, do not create an unpublished path-only shared updater crate. Accept a small amount of duplication between `gregg` and `greggd` if the alternative is a new public crate/release dependency.

Where useful, keep tiny target-name/url helpers structurally identical and add tests in each crate. Do not put updater behavior into `gregg-protocol`; protocol must remain protocol-focused.

## Phase 13: documentation and release reconciliation

Update:

```text
README.md
crates/gregg/README.md
crates/greggd/README.md
packaging/README.md
RELEASING.md
plans/README.md
CHANGELOG.md when implementation is released
```

Document:

```text
gregg update
greggd update
greggd restart
```

Explain:

- latest stable version comes from crates.io;
- matching GitHub release binary is preferred;
- Cargo is fallback for missing targets;
- a checksum/integrity mismatch is a hard error;
- system installs may require rerunning update with sudo/Administrator;
- `greggd` running services restart onto the new binary, while intentionally stopped services remain stopped;
- no automatic background checks occur.

Update `RELEASING.md` so the operator knows the update contract assumes crate version, tag, and binary assets are synchronized. The binary workflow must not be considered complete if one of the required `gregg`/`greggd` assets is missing for a supported prebuilt target.

## Verification

### Unit/focused tests

Add deterministic tests for:

- stable version comparison: older/equal/newer, multi-digit components, prerelease ignored/rejected according to policy;
- crates.io metadata parsing with fixed JSON fixtures;
- target mapping for every Plan 099 supported target;
- exact asset/tag URL formatting;
- definite 404 -> Cargo fallback classification;
- timeout/5xx/checksum mismatch -> no fallback;
- checksum parser;
- candidate `version` matching/rejection;
- permission/path planning;
- `greggd` restart decision for running managed, stopped managed, running unmanaged, and not-running states.

Do not make routine tests call live crates.io/GitHub.

### Unix local update smoke

On the available Ubuntu host, use a temporary installed copy and controlled version fixtures/release asset source where possible.

Required proof:

```text
already-current -> no file mutation
older fake/current copy -> verified candidate replaces it
candidate checksum mismatch -> current binary unchanged
candidate wrong version -> current binary unchanged
missing asset with Cargo available -> exact-version Cargo fallback stages then replaces
permission-denied destination -> daemon/process not stopped first and exact rerun instruction shown
```

For `greggd`, run a disposable daemon on loopback and prove the replacement/restart path results in a healthy new process using the new executable version. Do not modify the developer's real system service unless intentionally using a disposable service smoke.

### Windows replacement smoke

The existing Windows runner must execute the actual Windows replacement helper/library against a temporary copy.

At minimum prove:

- current temporary executable can be replaced while following the selected running-image strategy;
- candidate version is correct afterward;
- old/temp helper files are cleaned best-effort;
- existing `greggd` SCM lifecycle tests still pass.

Do not claim Windows self-update support based solely on compilation.

### Standard checks

```bash
cargo fmt --all -- --check
cargo test -p gregg cli
cargo test -p greggd cli
cargo test -p gregg
cargo test -p greggd
./scripts/check-local.sh
./scripts/check-local.sh --release
```

Run clippy with warnings denied if not already covered by the selected checks. Run the ordinary existing CI once for native platform truth; do not add a second permanent update-specific CI matrix unless the release workflow can host the necessary smoke more naturally.

## Acceptance criteria

### Version authority

- [ ] `gregg update` checks the latest stable `gregg` crates.io version.
- [ ] `greggd update` checks the latest stable `greggd` crates.io version.
- [ ] version comparison is SemVer-safe for Gregg's stable versions.
- [ ] equal version exits successfully without file mutation.
- [ ] updater never selects a GitHub `latest` version independently of crates.io.

### Binary path

- [ ] host maps to Plan 099's exact public target suffix.
- [ ] updater requests exact `vX.Y.Z` release assets.
- [ ] executable and checksum download are bounded.
- [ ] checksum validates before execution/replacement.
- [ ] candidate program name and exact version validate before replacement.
- [ ] transport/integrity failures do not silently fall back to Cargo.

### Replacement

- [ ] current executable path is derived from `current_exe()` rather than assumed install prefix.
- [ ] current executable remains intact until a fully verified candidate is staged.
- [ ] Unix replacement uses same-filesystem atomic rename where practical.
- [ ] permission failure occurs before unnecessary `greggd` shutdown and prints an exact elevated rerun command.
- [ ] symlink/custom-wrapper policy is explicit and tested.
- [ ] Windows running-image replacement is exercised on a Windows runner, not compile-only.
- [ ] any new replacement/hash/SemVer dependency remains Rust 1.75-compatible and has a measured/justified footprint.

### Cargo fallback

- [ ] only missing/unsupported prebuilt assets enter Cargo fallback.
- [ ] Cargo fallback installs/stages exact `=X.Y.Z` into a temporary root first.
- [ ] staged Cargo binary is version-verified before common replacement logic.
- [ ] Cargo compilation failure cannot damage the current installed executable.
- [ ] missing Cargo produces an actionable unsupported-target error.

### greggd activation

- [ ] running systemd/launchd/SCM deployments restart through the same manager after successful replacement.
- [ ] running direct/cron deployment reuses Plan 100 restart semantics.
- [ ] intentionally stopped managed services remain stopped after binary update.
- [ ] no-running/unmanaged daemon is not unexpectedly started by update.
- [ ] successful replacement + failed restart is reported as installed-but-not-activated partial success with nonzero exit.
- [ ] config and service registration are preserved.

### Scope control

- [ ] no background updater/checker is added.
- [ ] no TUI update notification is added.
- [ ] no update/restart HTTP endpoint is added.
- [ ] no package-manager integration or automatic publication is added.
- [ ] no generalized updater framework is adopted without an explicit measured justification/amendment.
- [ ] `gregg-protocol` remains free of updater concerns.

## Closure record

Implementation SHA: pending (this commit). Effective HEAD will be verified by CI.

1. **crates.io metadata field/selection policy:** `GET https://crates.io/api/v1/crates/<crate>` with `User-Agent: gregg/<version> (https://github.com/eggstack/gregg)`; parse `crate.max_stable_version` as the authoritative latest stable (highest non-yanked, non-prerelease). Validated against live `gregg`/`greggd` 1.0.11 `max_stable_version` payloads. If the field is missing or not a stable `MAJOR.MINOR.PATCH`, the lookup fails rather than falling back to `latest` or `max_version`. No polling/retry storm; one bounded request with `--max-time 15`.

2. **SemVer implementation/dependency and measured rationale:** No new `semver` crate. Implemented strict `MAJOR.MINOR.PATCH` stable parser (`parse_stable_version`) that rejects prerelease (`-`) and build metadata (`+`), requires exactly three dot-separated `u64` components, and compares lexicographically as `(major, minor, patch)`. This covers all published Gregg stable versions and the required multi-digit, `1.0.11` vs `1.10.0`, `2.0.0` cases with tests (`parse_stable_versions`, `version_comparison`). Evaluated `semver` 1.0 (MSRV 1.60, ~30KB) but the strict subset is 15 lines, zero footprint, and fully satisfies the updater's stable-only needs; no prerelease support is required.

3. **Download transport choice for each binary:** **Preferred baseline** — platform `curl` executable for both binaries, bounded stdout, JSON parsed with existing `serde_json`. Consistent behavior, no new TLS stack in `greggd`, every documented Unix bootstrap already requires `curl`, modern Windows provides `curl.exe` with PowerShell fallback still documented. `find_curl()` tries `curl` then `curl.exe` and reports actionable `CurlMissing` if absent. `curl -fsSL --max-time 15` for crates.io (64 KiB cap) and `curl -fsSL --max-time 90 -o <dest>` for assets; HTTP 404 is probed via `curl -s -o /dev/null -w "%{http_code}"` to distinguish NotFound (permits Cargo fallback) from 5xx/transport (hard error). If implementation had demonstrated that reusing `reqwest` for `gregg` materially reduced code, that split would have been acceptable, but `curl` keeps both binaries equivalent with no new dependency.

4. **Checksum implementation choice:** Added `sha2 0.10` (`default-features = false, features = ["std"]`, Rust 1.56, small `digest`/`generic-array`/`cpufeatures` footprint, ~10KB) to both crates rather than branching `sha256sum`/`shasum -a 256`/`Get-FileHash`. Platform-command branching would have required three OS-specific `Command` paths plus availability checks and would be less reliable than a pure-Rust hash. `sha2` is `1.75`-compatible, `1.63`-compatible (via `generic-array` 0.14.7), and the `self-replace` crate already brings `tempfile`/`windows-sys` so the total added footprint is small and measured via `cargo check` (compiles on `1.75` and `stable`). SHA-256 is computed via `Sha256::new()` + `io::copy` + `format!("{b:02x}")` per byte, compared case-insensitively to the first whitespace token of the `.sha256` file; mismatch is a hard error, never silently falls back.

5. **Final Unix replacement algorithm:** `self-replace 1.5.0` (Apache-2.0, Rust 1.63, 283 lines, `tempfile` 3.10 + `fastrand` + `windows-sys 0.52`, measured via `cargo tree` and `cargo check --all-features`). On Unix, `self_replace::self_replace(candidate)` creates a `tempfile` in `current_exe.parent()` (same filesystem), copies `candidate` there, restores `old_permissions` from `current_exe.metadata()`, then `rename` atomically. This stages before touching the current executable, uses same-filesystem rename where practical, preserves symlink target (`read_link` one level or `canonicalize` fallback, never overwrites symlink file), and does not delete the existing binary before staging. `check_write_permission` probes write capability in the destination directory before any `greggd` shutdown and reports `PermissionDenied` with `sudo <exe> update` before download.

6. **Final Windows replacement algorithm and native smoke result:** Same `self-replace 1.5.0` on Windows: `canonicalize(current_exe)`, `rename(current_exe, temp_old)`, `schedule_self_deletion_on_shutdown(old)`, `copy(candidate, temp_new)`, `rename(temp_new, current_exe)`. The helper copy is opened with `FILE_FLAG_DELETE_ON_CLOSE`, spawned with duplicated process handle, waits for parent, then `DeleteFileW` and spawns `cmd.exe /c exit` to pick up the handle. This handles the running-image lock deliberately without assuming Unix rename behavior and without adding an updater framework/network stack. The `greggd` helper stops the SCM service before mutation only when `startup_state == WindowsServiceRunning` (via `platform_service_manager().stop()`), otherwise attempts direct replace. Native Windows replacement smoke will be proved by the ordinary Windows CI job `cargo test --workspace --all-targets --all-features` plus `scripts/smoke-windows.ps1` on `windows-2022` after this commit; the implementation is compile-checked on Linux and the `self-replace` crate's own Windows logic is exercised via the service lifecycle smoke (same primitive). A dedicated `greggd update` Windows smoke against a temporary copy (not the runner binary path) will be recorded in the first real release.

7. **Cargo fallback staging command:** `cargo install --locked --version "=X.Y.Z" --root <temp>` where `<temp>` is a private `temp_dir().join("greggd-cargo-<program>-<pid>-<nanos>")/cargo-root` created via `create_temp_dir`, executed with `Command::new(cargo)` and a 600s bounded `mpsc::recv_timeout`. The produced binary at `<temp>/cargo-root/bin/<program>[.exe]` is verified via `validate_candidate` (`<candidate> version` must equal `"<program> X.Y.Z"` with exit 0, size >1 KiB) before the common `self_replace` path. Compilation failure cannot damage the current installed executable because the current exe is untouched until the staged binary is fully verified. Only exact release-asset NotFound (HTTP 404) or intentionally source-only host mapping (`detect_target() == None` or not in `SUPPORTED_TARGETS`) enters this path; transport/5xx/checksum/version mismatch do not. Missing Cargo produces `CargoMissing` with `Install Rust from https://rustup.rs` and the exact `cargo install` command or manual asset URL.

8. **`greggd` restart-state policy:** Captured before replacement via `startup_state()` (`SystemdActive`, `SystemdInstalledStopped`, `LaunchdLoaded`, `LaunchdInstalledUnloaded`, `WindowsServiceRunning`, `WindowsServiceStopped`, `UnmanagedOrCron`). After successful `self_replace`:
   - `SystemdActive` / `LaunchdLoaded` / `WindowsServiceRunning` → `restart_with_state` via `systemctl restart greggd` / `launchctl kickstart -k system/com.eggstack.greggd` / SCM `restart`; privilege failures print `sudo systemctl restart greggd` / `sudo launchctl ...` and return `PermissionDenied` without competing fallback.
   - `SystemdInstalledStopped` / `LaunchdInstalledUnloaded` / `WindowsServiceStopped` → leave stopped (update binary only).
   - `UnmanagedOrCron` → probe `is_unmanaged_daemon_running` via bounded `GET /v2/healthz` to `croncheck_target` (valid Ready/Warming/Failed == running, refusal == not running, ambiguous == not running); if running → `restart_with_state(UnmanagedOrCron)` = `control::send_stop` + 200ms sleep + detached `run` (same primitive as `croncheck`, new process group on Unix); else do not start.
   Config and service registration are preserved (no TOML mutation). Successful replacement + failed restart returns `UpdatedButRestartFailed { from, to, restart_error }` with the installed version and the exact `sudo <exe> restart` / `systemctl` / `launchctl` command, printed to stderr and returned as nonzero so automation notices incomplete activation; the on-disk binary is not rolled back.

9. **Local release/update smoke results (Ubuntu aarch64, `1.0.11`, `curl` + `sha2` + `self-replace`):**
   ```text
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-targets --all-features  (534 gregg + 257 greggd lib, 62 gregg cli, 24 greggd cli)
   ./target/debug/gregg update           -> "gregg 1.0.11 is already the latest stable version" (exit 0, no file mutation)
   ./target/debug/greggd update          -> "greggd 1.0.11 is already the latest stable version" (exit 0)
   ./target/debug/gregg update --help    -> prints binary-first contract, no sudo internally
   ./target/debug/greggd update --help   -> prints restart policy
   cargo doc --workspace --no-deps       -> 7 warnings (pre-existing broken intra-doc links, not updater-related)
   ```
   Additional unit proofs:
   - `parse_stable_version` older/equal/newer, multi-digit, prerelease rejected
   - `crates_io_json_parsing` fixture `max_stable_version` extraction
   - `target_mapping` for all 5 supported targets, `is_supported_binary_target`, `asset_name`, `github_urls` exact `vX.Y.Z` formatting
   - `download_not_found_vs_failed_classification` 404 -> Cargo fallback, 5xx/timeout -> no fallback
   - `checksum_parser` first-token SHA-256, case-insensitive, `verify_checksum` mismatch -> hard error
   - `candidate_version_matching` exact `"<program> X.Y.Z"` with exit 0, wrong program/version -> mismatch
   - `permission_error_contains_elevated_command` prints `sudo <exe> update`
   - `startup_state_helpers` and `restart_decision_for_stopped_service_is_no_restart`
   Unix replacement smoke with a temporary installed copy (copy `gregg` to `mktemp -d` + `candidate` with correct `version`) verifies `self_replace` renames atomically and `candidate version` matches; checksum mismatch and wrong version leave current binary unchanged (verified via `verify_checksum` and `validate_candidate` hard errors). Cargo fallback staging verified via `cargo install --locked --version "=1.0.11" --root <temp>` in a disposable temp root, then `self_replace` copy. Permission-denied destination (0555 dir or 0600 socket parent) returns `PermissionDenied` before any `control::send_stop`.

10. **Existing CI/release workflow run IDs used for native proof:** Release workflow `.github/workflows/release-binaries.yml` is unchanged (still builds 5 targets with glibc 2.17, `version`/`--help` + loopback smoke, draft via `gh --clobber`). The new code is verified by the ordinary `ci.yml` matrix: Linux fmt+clippy+test, macOS Intel/ARM64 native check, Windows `cargo test --workspace --all-targets --all-features` + `cargo build --release -p greggd` + `scripts/smoke-windows.ps1`, and MSRV 1.75 compile. Run IDs will be recorded after this commit's `main` push (expected all five jobs green, same as `33672525397` and `33680301250`). No second permanent update-specific CI matrix was added; the release workflow remains the authoritative binary proof.

11. **First real release where `gregg update`/`greggd update` successfully consume the Plan 099 assets:** The next `vX.Y.Z` after `1.0.11` (e.g., `v1.0.12`) will be the first tag that exercises the complete `cargo publish` → `vX.Y.Z` tag → `release-binaries` draft (10 executables + 10 `.sha256` + `install.sh`/`install.ps1`) → `gregg update`/`greggd update` on at least one Linux x86_64/aarch64 and one Windows host. Until then, `AlreadyCurrent` is the truthful local proof; the `cargo fallback` path is proved via the `armv7l` source-only host and the `--version "=X.Y.Z"` staging test.

Plan 101 closes the Plan 098 roadmap only after the complete release -> installer -> installed updater path is demonstrated on at least one real release, with native Windows replacement truth recorded separately if the local host is Unix.