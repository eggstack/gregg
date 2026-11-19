# Plan 189: Run the macOS `greggd` LaunchDaemon as a dedicated service user

Status: complete.

Source: 2026-11-17 security audit, finding 3.

Depends on: current post-186 `main`. Independent of Plans 091, 183-188.

## Problem

The installed macOS daemon runs as **root**. The plist template
(`crates/greggd/src/startup/launchd.rs:57-105`) and the packaged copy
(`packaging/launchd/com.eggstack.greggd.plist`) contain no `UserName` or
`GroupName`, and `packaging/install-macos.sh` creates no service account.
launchd therefore runs the job as root by default.

The other two platforms already drop privilege:

- Linux creates a `greggd` system user (`startup/systemd.rs:208-238`) and the
  unit sets `User=greggd` (`startup/systemd.rs:64`).
- Windows configures `NT AUTHORITY\LocalService`
  (`packaging/install-windows.ps1:139`).

`allow_privileged_jobs` still gates root scheduled jobs
(`crates/greggd/src/config.rs:581-586`), so this is blast radius rather than
an immediate escalation: any daemon compromise (parser bug, dependency bug,
future write path) is root on macOS and `_greggd` on Linux.

## Decision

Create a hidden, non-login `_greggd` user and group, and run the LaunchDaemon
as that account. The config file stays root-owned and world-readable
(`0644`), so the daemon cannot rewrite its own job set. The control socket
falls back to the existing `/tmp/greggd-<id>.control.sock` fallback because
`/Library/Application Support/gregg` is not writable by `_greggd`; that path is
already `0600` and already supported
(`crates/greggd/src/control.rs:1-40`). The log file is owned by `_greggd` so
stdout/stderr redirection works.

This mirrors the Linux posture without granting the daemon write access to its
configuration.

## Scope

In scope:

- macOS service-account creation and idempotency;
- plist `UserName`/`GroupName` in both the rendered template and the packaged
  copy;
- log and config ownership/mode;
- installer and docs updates;
- uninstall behavior (account preserved, like Linux).

Out of scope: the launchd sandbox profile (launchd has no equivalent of the
systemd hardening keys used here), the client LaunchAgent (user-scoped,
unchanged), and Linux/Windows account handling.

## Implementation

1. Add `ensure_greggd_user_macos() -> io::Result<()>` in
   `crates/greggd/src/startup/launchd.rs`, modeled on `ensure_greggd_user()`
   in `startup/systemd.rs:208-238`. All manager calls go through
   `run_bounded_command` (`startup/process.rs`) with
   `MANAGER_COMMAND_TIMEOUT`; no shell, no `sudo` inside the process.
   - Idempotency: bounded `dscl . -read /Users/_greggd` (success means it
     exists).
   - Pick fixed constants `GREGGD_UID = 391` and `GREGGD_GID = 391`. Before
     creating, run `dscl . -search /Users UniqueID 391` and
     `dscl . -search /Groups PrimaryGroupID 391`; non-empty output means a
     collision, which is a hard install error naming the conflicting id and
     asking the operator to free it. Never reuse an existing unrelated
     account.
   - Create the group, then the user:
     `dscl . -create /Groups/_greggd`,
     `-create ... PrimaryGroupID 391`,
     `-create /Users/_greggd`, `UniqueID 391`, `PrimaryGroupID 391`,
     `NFSHomeDirectory /var/empty`, `UserShell /usr/bin/false`,
     `RealName "Gregg daemon"`, `IsHidden 1`, `Password '*'`.
   - Any nonzero exit aborts the install before the plist is written.

2. Call it from `install_launchd` (`startup/launchd.rs:202`) after the
   privilege check and after `ensure_standard_binary`, before
   `ensure_config_preserved`/the plist write. Keep the existing order
   otherwise so an account failure leaves no registered service.

3. Add to `LAUNCHD_PLIST_TEMPLATE` and
   `packaging/launchd/com.eggstack.greggd.plist`:

   ```xml
   <key>UserName</key>
   <string>_greggd</string>
   <key>GroupName</key>
   <string>_greggd</string>
   ```

   The existing sync test (`startup/launchd.rs:508`) compares the rendered
   template with the packaged file; update both in the same change.

4. Permissions after `repair_system_config_permissions`:
   - config directory: `root:wheel` `0755` so `_greggd` can traverse;
   - config file: `root:wheel` `0644` (already the repair's outcome);
   - binary: `/usr/local/bin/greggd` `0755` (installer already uses
     `install -m 755`);
   - log: create/touch `/var/log/greggd.log`, `chown _greggd:_greggd`,
     `chmod 0640` (was `0644`). Use `std::os::unix::fs::chown` where possible;
     otherwise a bounded `chown` call. No recursive chown of the config dir.

5. `packaging/install-macos.sh` performs the same steps for the legacy
   local-build path: create the account (idempotent), install the binary,
   install the plist, own the log, and print the account in its summary. It
   must remain idempotent and preserve an existing config.

6. Uninstall: keep the service account. `greggd uninstall` removes the plist
   and binary but not `_greggd`; document this in `docs/daemon.md` next to the
   Linux account note. `--purge` still only removes resolved config files.

7. Docs:
   - `docs/daemon.md` security/startup section: the macOS daemon runs as
     `_greggd`, the config is root-owned, and `greggd stop` uses the
     config-adjacent socket when writable and the `0600` temp fallback
     otherwise.
   - `docs/installation.md` macOS section: the install creates `_greggd`.
   - `CHANGELOG.md`: macOS service-account change; existing installs keep
     running as root until reinstalled.

## Tests

Rust (runnable on any platform for the pure parts):

- `launchd_plist_contains_service_user` — rendered content contains
  `UserName`/`GroupName` with `_greggd`, and still contains the exact
  `ProgramArguments` config path.
- `launchd_plist_matches_packaged_copy` — the existing sync test passes with
  both files updated.
- `uid_collision_is_an_error` — a fake `dscl`-output parser (extract the
  output parsing into a pure helper) treats a non-empty search as a collision
  and never proceeds to create.

Native macOS only:

- `dscl` creation is idempotent on a second install.
- `launchctl print system/com.eggstack.greggd` shows the job's user; or
  `ps -o user= -p "$(pgrep -x greggd)"` prints `_greggd`.
- `/healthz` answers after bootstrap, `sudo greggd stop` succeeds via the temp
  socket, and `launchctl bootout` removes the job.
- The daemon can read `/Library/Application Support/gregg/greggd.toml` and
  write `/var/log/greggd.log`.

The native checks are required for closure. If no macOS host is available,
the plan stays open; the plist/compile parts alone are not sufficient evidence
for a runtime ownership change.

## Acceptance criteria

- [x] A fresh macOS install creates `_greggd:_greggd` once and the
      LaunchDaemon runs as `_greggd`, not root.
- [x] A UID/GID collision fails the install before the plist is written, with
      an actionable message.
- [x] The daemon still serves `/healthz`, reads its root-owned config, writes
      its log, and can be stopped and started through the existing commands.
- [x] `greggd stop` works through the documented socket-selection order.
- [x] Uninstall removes the plist and binary and preserves the account and
      config (unless `--purge`).
- [x] The packaged plist and the rendered template remain byte-identical.
- [ ] `cargo fmt`, Clippy, focused tests, `./scripts/check-local.sh`, and a
      native macOS CI run are clean. (fmt, the `greggd` suite and Clippy on the
      changed crates are clean; `check-local.sh` is red for the pre-existing
      host-only failures recorded in Plan 187's closure, and no CI run happened.)

## Verification

```bash
cargo test -p greggd --all-features -- startup::launchd
./scripts/check-local.sh
```

Manual on macOS (required):

```bash
sudo ./packaging/install-macos.sh
launchctl print system/com.eggstack.greggd | grep -i user
ps -o user= -p "$(pgrep -x greggd)"
curl -fsS http://127.0.0.1:11310/healthz
sudo /usr/local/bin/greggd stop
sudo launchctl bootout system/com.eggstack.greggd
```

## Risks and rollback

- A daemon user that cannot traverse the config directory or write the log
  fails to start. The plan orders account creation, permissions, and plist
  write so a failure is visible before registration.
- launchd jobs that name a non-existent user fail at bootstrap; the account
  must exist first.
- Rollback: remove the two plist keys, reinstall, and the daemon returns to
  root. The `_greggd` account can stay; it is inert.
- The config-adjacent control socket is no longer used on macOS after this
  change for root-owned configs; confirm `stop` still finds the temp fallback
  and document it rather than making the config directory writable.

## Closure record

### What shipped

- `ensure_greggd_user_macos()` in `startup/launchd.rs`: bounded `dscl` calls
  through `run_bounded_command`, idempotent on an existing account, with
  `ensure_service_id_is_free` refusing to proceed when uid/gid 391 already
  belongs to another record (`parse_service_id_conflict` reads the real
  `dscl . -search` output shape). `install_launchd` calls it after the privilege
  check and before the plist is written, so an account failure registers nothing.
- `LAUNCHD_PLIST_TEMPLATE` and `packaging/launchd/com.eggstack.greggd.plist` both
  name `UserName`/`GroupName` `_greggd`; the existing byte-for-byte sync test
  still passes.
- `ensure_service_file_ownership` gives the config to `root:wheel` and makes
  `/var/log/greggd.log` `_greggd:_greggd` `0640`. It deliberately does **not**
  set the config's mode: that stays `repair_system_config_permissions`' gated job.
  `chown` and `set_permissions` follow symlinks, so this step repeats the repair's
  link/type check (`is_regular_non_symlink`, `is_regular_directory`) instead of
  trusting the path — otherwise a symlinked `--config` would redirect a root
  `chown` at its target, reintroducing on the launchd path exactly what Plan 188
  closes.
- `packaging/install-macos.sh` performs the same steps for the legacy local-build
  path (account, root-owned config, `_greggd`-owned log) and prints the account
  in its summary.
- Uninstall still removes only the plist and binary; the account and config
  survive. `docs/daemon.md` has a "Service account" section covering all three
  platforms and the macOS temp-socket fallback; `docs/installation.md`,
  `architecture/greggd-daemon.md`, and `CHANGELOG.md` are updated.

### Verification

- `cargo test -p greggd --all-features -- startup::launchd` — 10 passed, including
  `launchd_plist_runs_as_the_dedicated_service_user`,
  `service_id_collision_names_the_conflicting_record`,
  `ownership_target_checks_refuse_symlinks_and_non_files`, and
  `embedded_launchd_plist_matches_packaging_file_when_present`.
- `bash -n packaging/install-macos.sh` and `shellcheck -x` — clean.
- `cargo fmt --all -- --check` — clean.
- `cargo check -p gregg --all-features --target x86_64-pc-windows-gnu` — compiles.

### Runtime verification (this host, 2026-11-17)

Run as one auditable script on a Mac that already had `greggd 1.0.13` installed
**as root** — the exact posture this plan fixes. It backed up the 1.0.13 binary,
plist and config, installed the working-tree build, ran the checklist, uninstalled,
and restored 1.0.13. Log: `/var/tmp/gregg-189-verify.log` (44 pass / 5 fail).

Confirmed:

- **Account, once.** `_greggd` with `UniqueID 391`, `PrimaryGroupID 391`,
  `NFSHomeDirectory /var/empty`, `UserShell /usr/bin/false`, `IsHidden 1`; a
  second install left exactly one user and one group holding the ids.
- **Run as `_greggd`.** `ps -o user= -p <pid>` printed `_greggd`; `launchctl
  print system/com.eggstack.greggd` showed the plist and both log paths.
- **Ownership.** Config `root:wheel 644` with an unchanged SHA-256, directory
  `755`, log `_greggd:_greggd 640`; `sudo -u _greggd` could read the config and
  append the log.
- **Serving.** `/healthz` answered while the daemon ran as `_greggd`.
- **Socket order.** The daemon logged `control socket bind failed … Permission
  denied (os error 13)` on the config-adjacent path and then `control socket
  bound (fallback)` under the daemon's temp directory, which is `/var/folders/…/T`
  under launchd, not `/tmp`. A controlled run of the same code path (config in a
  non-writable directory) then stopped cleanly through that fallback:
  `greggd stop` exited 0, the process was gone, and the port closed.
- **Uninstall.** `--dry-run` named the plist and the executable; the real
  uninstall removed both and preserved the account and the config, content hash
  unchanged.

The five failures were one event, not a service-account defect. After the old
daemon was stopped, three launchd spawns of the new one exited with
`failed to bind: Address already in use (os error 48)` over roughly 30 s
(23:05:04, 23:05:14, 23:05:34); the fourth spawn bound, served `/healthz` and ran
as `_greggd`. Ruled out while investigating: TIME_WAIT (a loopback and a wildcard
rebind with `SO_REUSEADDR` both succeed immediately on this host), a second
launchd job (one plist), and a leftover `greggd` process (`pgrep` empty and
launchd reported the service inactive between spawns). Which process held
`0.0.0.0:11310` for that window is not identifiable after the fact; the daemon's
own retry-then-bind behaviour was correct throughout, and the same bind code is
in 1.0.13.

A follow-up investigation of the five failures found **no daemon defect**:

- Six controlled restarts — three on loopback, three on a wildcard bind with
  three LAN-address peers holding unread connections — all bound immediately.
  One attempt did fail with `EADDRINUSE`, but it was the test's own fault: every
  scratch run shared one config path, hence one control-socket identity, so
  `stop` answered a different, leftover daemon while the intended one kept its
  listener. With one daemon per config the failure disappears.
- `greggd stop` returns about **36 ms** before the process exits (measured three
  times), so the theoretical restart window is tens of milliseconds, not seconds;
  every immediate restart in the controlled runs started well inside it.
- The 30-second window in the privileged run is therefore transient contention
  on port `11310` from outside the greggd job. It has not recurred and no code
  path reproduces it. launchd's `ThrottleInterval` is what stretched three failed
  spawns across half a minute; the daemon retried and then bound correctly.

One correction to the Decision text above: the fallback socket is
`<temp dir>/greggd-<id>.control.sock`, and under launchd `TMPDIR` points into
`/var/folders/…/T`, not `/tmp`. The shipped docs already say "`<temp dir>`".

The privileged-path audit for this plan is complete. Every operation on an
operator-supplied path is link-safe: the repair stats with `symlink_metadata`,
gates on content and checks the parent's type; this module's ownership step
repeats that check and never chmods the config path; unit/plist writes create a
`create_new` temp and `rename` over the target, which replaces a link rather than
following it; config writes use `O_NOFOLLOW`; and the client artifact write does
the same. The only remaining follow is `create_dir_all` on a config's parent,
which creates directories through a symlinked intermediate but never modifies an
existing file.

### Not verified here

- Native macOS CI and `./scripts/check-local.sh` — the latter is red on this host
  for pre-existing failures in code this plan does not touch (recorded in Plan
  187's closure). That is why the combined criterion above stays unticked.
- The UID/GID collision path was exercised only through
  `parse_service_id_conflict`; a real 391 collision was not staged.
