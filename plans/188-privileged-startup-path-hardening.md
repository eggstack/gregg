# Plan 188: Harden privileged startup paths and the client's user-scoped registration

Status: proposed (not started).

Source: 2026-11-17 security audit, findings 2, 7, and 8.

Depends on: current post-186 `main`. Independent of Plans 091 and 183-187.

## Problem

Three related boundary defects sit in the startup/registration path.

### A. The privileged config-permission repair follows symlinks

`repair_system_config_permissions` (`crates/greggd/src/startup/install.rs:417-439`)
uses `fs::metadata` and `fs::set_permissions`, both of which follow symlinks,
and ORs in `0o755` on the parent and `0o644` on the file. It runs from the
systemd install (`startup/systemd.rs:345`) and the launchd install
(`startup/launchd.rs:232`), i.e. as root.

An install invoked with a `--config` that is a symlink to `/etc/shadow` (or
any root-owned `0600` file) makes that target world-readable. A symlinked
parent directory gets world-traverse added. The precondition is an
administrator passing an attacker-influenced path, so this is local and
admin-assisted, but the repair has no reason to follow links or to touch a
file that is not a Gregg config.

### B. Privileged rerun hints interpolate the executable path unquoted

`crates/greggd/src/startup/systemd.rs:541-546` and
`crates/greggd/src/startup/launchd.rs:449-451, 467-471` build copy-pasteable
`sudo <path> uninstall` hints with `exe.display()` unquoted. A path containing
spaces, `$`, quotes, or `;` changes meaning when the operator pastes the hint.
The codebase already has the correct quoting helper for this
(`gregg_update::stage::unix_shell_quote`, used by `elevated_command` at
`startup/install.rs:378-388`), and AGENTS.md states every printed rerun
command quotes its executable through that helper.

### C. The client registers user-scoped startup without a privilege guard and writes artifacts through symlinks

`dispatch_daemon_startup` (`crates/gregg/src/cli.rs:571-584`) calls
`startup::install_with` with no effective-privilege check, contradicting the
contract that a root/Administrator install registers nothing.
`write_artifact` (`crates/gregg/src/clientd/startup.rs:727-737`) uses
`fs::write`, which follows an existing symlink, so a privileged run with a
hostile `HOME` gets a write-through primitive.

## Scope

In scope: the permission-repair policy, the two hint renderers, the client
install guard, and the client artifact write. Out of scope: the macOS launchd
service account (Plan 189), the update transport (Plan 190), and any change to
systemd unit/launchd plist ownership.

## A. Make the permission repair symlink- and content-safe

Policy, chosen to keep unprivileged diagnostics working without letting the
repair touch arbitrary files:

1. Inspect with `fs::symlink_metadata`, never `fs::metadata`.
2. Skip (do not chmod) when the final path component is a symlink, and skip
   the parent-directory step when the parent is a symlink or not a directory.
3. Only act on regular files and regular directories.
4. Only relax a file when either:
   - it is the platform default config path (`Config::default_path()`), or
   - it parses and validates as a `Config` (`Config::load(path).is_ok()`).

   The content gate is what makes a `/etc/shadow` target a no-op: it is not a
   Gregg config. The default-path branch keeps the repair working for the
   normal packaged install even if the file temporarily fails to parse.
5. Keep the existing "add only the missing bits" behavior for files that pass
   the gate, so operator-managed `0640`/`0750` modes are preserved.

`install.rs` may use `tracing::warn!` for a skipped symlink so the operator
learns why unprivileged `status`/`croncheck` may fail; the function still
returns `Ok(())` (this remains a best-effort repair).

Tests:

- symlinked config to a `0600` regular file: target mode unchanged, symlink
  unchanged, function returns `Ok`.
- symlinked parent directory: parent target mode unchanged, file still
  relaxed.
- non-Gregg file in a `0700` directory: unchanged.
- existing valid-config tests (`repair_system_config_permissions_relaxes_old_0600`
  and the group-only/relaxed-mode cases) still pass; update any fixture whose
  content is not a valid `Config` to a minimal valid one.

## B. Quote every privileged rerun hint

Add one helper next to `elevated_command`:

```rust
pub(crate) fn sudo_uninstall_hint(exe: &Path) -> String
```

which quotes through `gregg_update::stage::unix_shell_quote` and returns
`sudo <quoted> uninstall`. Replace the three hint sites with it. Leave
diagnostic text that merely names the executable in parentheses (for example
`launchd.rs:361, 376`) alone: it is not a command.

Tests: paths with a space, `$HOME`, a single quote, a semicolon, and a
newline; assert the rendered hint re-parses as `sudo`, one path argument, and
`uninstall`. Follow the existing `elevated_command` test style
(`startup/install.rs:816-838`).

## C. Refuse user-scoped registration when privileged

1. Add `crates/gregg/src/privilege.rs` with `pub fn is_privileged() -> bool`.
   - Unix: `unsafe { libc::geteuid() == 0 }` with a SAFETY comment.
   - Windows: `OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, ...)` plus
     `GetTokenInformation(..., TokenElevation, ...)`; treat any error as
     privileged (fail closed) so an unreadable token never silently registers.
     The required `windows-sys` features (`Win32_Security`,
     `Win32_System_Threading`, `Win32_Foundation`) are already enabled for the
     named-pipe transport.
   - Other platforms: `false`.
2. Add the new file to the AGENTS.md unsafe allowlist
   (`crates/gregg/src/privilege.rs (geteuid)`).
3. In `dispatch_daemon_startup`, refuse `DaemonStartupCommand::Install` when
   `is_privileged()` and return a clear error:
   "refusing to register user-scoped startup as root/Administrator; run gregg
   as the user who should own the daemon". `Uninstall`, `Instructions`, and
   `Status` stay allowed so a root operator can still inspect and clean up
   legacy artifacts.
4. Make the guard testable without changing process credentials: an inner
   `dispatch_daemon_startup_with(privileged: bool, ...)` carries the decision;
   the public wrapper passes `is_privileged()`. Tests cover both values and
   assert no manager call or file write happens on refusal.
5. Harden `write_artifact`:
   - reject an existing symlink at the exact artifact path with a typed
     `StartupError::UnsafeArtifact { path, reason }`;
   - on Unix, open with
     `OpenOptions::new().write(true).create(true).truncate(true).custom_flags(libc::O_NOFOLLOW)`;
   - on Windows, keep the pre-check and document that creating a symlink needs
     privilege; the write is still confined to the user's own profile.
   - Parent directories may still be symlinked (dotfile-managed `~/.config`);
     only the final component is rejected.

Tests (Unix): pre-created symlink at the systemd unit / LaunchAgent plist /
Startup-folder path makes install fail and leaves the target unchanged; a
normal install still overwrites an existing regular artifact; the missing-file
case still creates it.

## Acceptance criteria

- [x] A symlinked `--config` can no longer cause `repair_system_config_permissions`
      to chmod the link target; a non-Gregg file is never chmodded.
- [x] A valid system config still becomes readable/traversable exactly as
      before, and operator-set mode bits are preserved.
- [x] Every printed `sudo ... uninstall` hint quotes the executable with
      `unix_shell_quote`, and metacharacter paths re-parse as one argument.
- [x] `gregg daemon startup install` as root/Administrator is refused with the
      stated message and makes no file/manager change; as a normal user it is
      unchanged.
- [x] A symlink planted at a client startup artifact path is rejected; the
      target file is unchanged.
- [x] `cargo fmt`, workspace Clippy, focused tests, and
      `./scripts/check-local.sh` are clean.

## Verification

```bash
cargo test -p greggd --all-features -- startup::install
cargo test -p greggd --all-features -- startup::systemd
cargo test -p greggd --all-features -- startup::launchd
cargo test -p gregg --all-features -- clientd::startup
cargo test -p gregg --all-features -- privilege
./scripts/check-local.sh
```

Native macOS/Windows checks are required only for the platform-gated guards:
the Windows token check must compile and run on the native Windows CI job, and
the launchd hint tests run on the macOS jobs.

## Risks

- The content gate means a hand-written system config that fails validation
  (for example root jobs without `allow_privileged_jobs = true`) will no longer
  be relaxed. That config is already unusable by the daemon; the warning makes
  the consequence visible.
- Refusing root installs is a behavior change. It is the documented contract,
  and uninstall/instructions remain available.

## Closure record

### What shipped

- `repair_system_config_permissions` stats with `symlink_metadata`, refuses a
  symlinked or non-regular final component, and evaluates
  `permission_repair_target_is_trusted` — the default config path, or a path that
  parses as a `Config` — **before** touching the parent. A skipped target warns
  and returns `Ok`, because this stays a best-effort repair.
- `sudo_uninstall_hint(exe)` renders `sudo '<quoted>' uninstall` through
  `gregg_update::stage::unix_shell_quote`; it replaced the three unquoted sites
  (`systemd.rs` `systemd_manager_error_for`, `launchd.rs` bootout and
  remove-plist) and the fourth site that already quoted inline, so one helper now
  owns the invariant.
- `crates/gregg/src/privilege.rs` is a new `geteuid()` / `TokenElevation` probe
  (Windows fails closed on an unreadable token) and is on the `AGENTS.md` unsafe
  allowlist. `privileged_install_refusal` is a pure decision that
  `dispatch_daemon_startup_with` applies; `install` is the only refused verb.
- `write_artifact` rejects an existing symlink with
  `StartupError::UnsafeArtifact` and opens with `O_NOFOLLOW` on Unix, closing the
  check-then-open race. Symlinked parents remain legal.

### Verification

- `cargo test -p greggd --all-features -- startup::install` — 19 passed,
  including `repair_refuses_a_symlinked_config_and_leaves_its_target_alone`,
  `repair_skips_a_symlinked_parent_and_still_relaxes_the_file`,
  `repair_never_relaxes_a_file_that_is_not_a_gregg_config`,
  `repair_skips_a_non_regular_config_path`, and
  `the_uninstall_hint_quotes_a_metacharacter_path` (which re-parses the hint in
  `/bin/sh` for a space, `$HOME`, a single quote, a semicolon, and a newline).
- `cargo test -p gregg --all-features -- clientd::startup` — 31 passed,
  including `write_artifact_refuses_a_planted_symlink`,
  `write_artifact_refuses_a_dangling_symlink`, and
  `write_artifact_creates_and_replaces_regular_files`.
- `cargo test -p gregg --all-features -- privilege` — 5 passed, including
  `privileged_install_is_refused_before_any_artifact_is_written` and
  `only_install_is_refused_when_privileged`.
- `cargo check -p gregg --all-features --target x86_64-pc-windows-gnu` —
  compiles, so the Windows token branch is at least type-checked here.

### Not verified here

- The native Windows Clippy/test job is the authority for the token branch; a
  `--target x86_64-pc-windows-gnu` check is a cross-compile aid, not a run.
- The workspace Clippy gate is red on this host only because
  `crates/gregg-host/src/macos/ffi.rs` (untouched by this plan) trips
  `clippy::borrow_as_ptr` under the installed 1.100.0 toolchain. Every crate
  changed here is Clippy-clean, and the one finding this plan did introduce
  (`explicit_auto_deref` in the Plan 193 scan test) was fixed.
