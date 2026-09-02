# Plan 100: greggd startup installation and restart

Status: complete at `2271b9e` (implementation `a73a6a3` plus CRLF fix `2078924` and drive-test flake fix `2271b9e`; verified by CI run `33680301250`).

Depends on: Plan 098, Plan 099's installed-binary/bootstrap contract, completed Plans 076-082, and the final watchdog semantics from Plan 091 for the cron path.

## Objective

Make `greggd` easy to configure for startup across the systems Gregg already supports without undoing the deliberate separation between the foreground daemon runtime and external service managers.

The required CLI surface is:

```text
greggd startup install
greggd startup install --method systemd
greggd startup install --method launchd
greggd startup install --method cron
greggd startup instructions
greggd startup instructions --method <...>
greggd restart
```

Windows keeps its native SCM runtime/lifecycle behavior. The bootstrap installer may continue to own Windows service registration if that is smaller than teaching the Rust CLI to create SCM registrations; the important invariant is that the installed Windows daemon starts automatically and the existing `start`/`stop`/`restart` commands remain truthful.

## Architectural boundary

This plan intentionally permits service-manager execution only from explicit deployment/lifecycle commands.

Allowed:

```text
crates/greggd/src/cli.rs
crates/greggd/src/main.rs
new small crates/greggd/src/startup.rs (or equivalent)
packaging/install.sh
packaging/install.ps1
packaging/systemd/*
packaging/launchd/*
```

Do not add systemd/launchd/cron logic to:

```text
run.rs
sampler.rs
collectors
server/HTTP routes
config mutation side effects
```

`greggd run` remains a foreground process with no knowledge of who supervises it.

## Startup method contract

Use one small enum/internal representation, for example:

```rust
Systemd
Launchd
Cron
WindowsScm
Direct
```

Do not build a generic service-manager trait hierarchy unless it actually makes the code smaller. Platform-specific functions behind cfg gates are sufficient.

### Auto-detection

`greggd startup install` and `instructions` default to `auto`.

Required preference:

```text
Windows -> SCM
macOS -> launchd
Linux with an actually running systemd environment -> systemd
other Unix/Linux -> cron
```

On Linux, do not treat the mere existence of `/usr/bin/systemctl` as proof that systemd owns PID 1. Use a small runtime test such as `/run/systemd/system` plus a bounded `systemctl` probe or an equivalently reliable check.

If systemd is detected but installation lacks privilege, do **not** silently fall back to cron. Return/print the exact elevated command required to complete the systemd installation. Competing supervisors are worse than a clear privilege failure.

An operator may explicitly request `--method cron` on a systemd host.

## Phase 1: add parser and pure detection/rendering helpers

Extend `greggd` clap commands without changing existing command semantics.

Suggested shape:

```rust
Command::Startup { command: StartupCommand }
Command::Restart

StartupCommand::Install { method: StartupMethodArg }
StartupCommand::Instructions { method: StartupMethodArg }
```

`Restart` already exists on Windows; make the public command available cross-platform while preserving the existing Windows dispatch.

Add pure/testable helpers for:

- OS/default startup method selection;
- systemd environment detection boundary;
- standard binary/config paths used by system managers;
- crontab line rendering;
- instruction rendering;
- manager-state detection needed by restart/update.

Avoid testing by invoking the real service manager for every parser/unit test.

## Phase 2: systemd installation

### Canonical deployment

Preserve the current hardened system service model:

```text
binary: /usr/local/bin/greggd
config: /etc/gregg/greggd.toml
service user/group: greggd
greggd.service: /etc/systemd/system/greggd.service
```

Do not weaken the current unit merely to make generation easier. Keep the useful hardening directives unless a native SBC/systemd compatibility test proves one is invalid on the supported baseline.

### Canonical unit source

The installed binary must be able to install/render the unit without assuming a Git checkout is present.

Choose one small canonical strategy:

1. generate the fixed unit text from `startup.rs`; or
2. embed an in-crate asset with `include_str!` whose packaged crate contents are verified by `cargo package`.

Do not rely on reading `../../packaging/systemd/greggd.service` at runtime after `cargo install`.

If `packaging/systemd/greggd.service` remains as a human-readable packaging asset, add a focused test/check to keep it semantically synchronized with the binary's canonical template. Avoid two independently edited full templates.

### Install behavior

When run with required privilege, `greggd startup install --method systemd` should be idempotent:

1. verify Linux/systemd environment;
2. verify the executable expected by the unit exists at `/usr/local/bin/greggd` or give an actionable installation-path error;
3. ensure the `greggd` system user/group exists using the smallest established system command path;
4. create `/etc/gregg` if needed;
5. create the default config only if absent, preserving an existing valid operator config;
6. set ownership/permissions consistent with the existing packaging model;
7. write/update the unit atomically enough to avoid a truncated unit;
8. run `systemctl daemon-reload`;
9. run `systemctl enable greggd`;
10. start or restart the service as appropriate;
11. report status commands and config location.

Do not modify firewall rules.

If an existing unit differs because an operator intentionally customized it, do not blindly overwrite without notice. A simple policy is acceptable: overwrite only the Gregg-managed standard path while printing that it is being replaced, or require `--force` only if a concrete customization problem appears. Do not add a large unit-merging/config-management subsystem.

### Permission behavior

If not root or writes fail with permission denied:

- make no false success claim;
- do not invoke `sudo` internally;
- print the exact command, normally `sudo /path/to/greggd startup install --method systemd`;
- use the existing `PermissionDenied` exit classification where possible.

## Phase 3: launchd installation

Preserve the current system-daemon model:

```text
binary: /usr/local/bin/greggd
config: /Library/Application Support/gregg/greggd.toml
plist: /Library/LaunchDaemons/com.eggstack.greggd.plist
label: com.eggstack.greggd
```

The CLI must be able to render/install the plist without a Git checkout. Apply the same canonical-template rule as systemd.

When privileged:

1. verify macOS;
2. verify `/usr/local/bin/greggd` exists;
3. create/preserve config directory and default config;
4. write/update plist;
5. boot out an existing Gregg job only as needed for an update;
6. `launchctl bootstrap system <plist>` when absent;
7. `launchctl kickstart -k system/com.eggstack.greggd` when already registered/restarting;
8. print useful log/status commands.

Do not silently create a user LaunchAgent as fallback from a failed system LaunchDaemon install. The operator may use cron explicitly if they truly want a user-owned alternate supervisor.

No macOS user-account redesign is required in this plan. Preserve the current launchd execution identity unless source review shows an existing correctness problem.

## Phase 4: cron installation for non-systemd/operator-managed Unix

`croncheck` is the supervisor primitive. Do not create PID files or shell process scans.

### Required managed entry

Use an idempotently identifiable block/comment such as:

```text
# greggd managed watchdog
@reboot '/absolute/path/greggd' --config '/path/to/config' croncheck
* * * * * '/absolute/path/greggd' --config '/path/to/config' croncheck
```

The exact schedule may omit `@reboot` if target cron implementations demonstrably do not support it, but the normal Linux cron path should use both reboot start and once-per-minute watchdog. Once per minute is adequate; do not add sub-minute loops.

Use the final Plan 091 `croncheck` contract:

```text
valid Gregg health -> do nothing
connection refused/definitely absent -> spawn detached run
ambiguous occupied/silent/non-Gregg endpoint -> nonzero, no blind second daemon
```

### Current executable and config

Cron may use `std::env::current_exe()` and the resolved config path because it is a user-owned supervisor and does not need the fixed `/usr/local/bin` system-service path.

Render paths using a small safe POSIX shell quoting helper. Reject embedded newlines/control characters that cannot be represented safely. Do not concatenate unquoted arbitrary paths into the cron line.

### Idempotent crontab mutation

Implement with the standard `crontab` command:

1. run `crontab -l` and treat "no crontab" as empty;
2. remove only the exact Gregg-managed lines/block from previous installs;
3. append one canonical block;
4. feed the result to `crontab -`;
5. preserve unrelated user entries byte-for-byte where practical;
6. rerunning produces one Gregg block, never duplicates.

Do not edit `/var/spool/cron` directly.

If `crontab` is unavailable, return an actionable error and print the exact lines the operator can add through another scheduler.

### `startup instructions --method cron`

This command is read-only. It prints:

- the exact cron lines for the current executable/config;
- a safe command sequence to edit/install them manually;
- a reminder that `croncheck` is the watchdog and no PID file is required.

It must not mutate crontab.

## Phase 5: Windows startup reconciliation

The existing Windows implementation already has:

```text
greggd service   # hidden SCM entry
greggd start
greggd stop
greggd restart
packaging/install-windows.ps1 SCM registration
```

Do not rewrite working SCM runtime code merely for symmetry.

After Plan 099's canonical `install.ps1` exists, choose the smaller final ownership:

- preferred: the PowerShell installer performs SCM registration/start exactly once and `greggd startup instructions` reports the equivalent service state/commands; or
- if the `windows-service` dependency already exposes a very small reliable install API, `startup install` may register the SCM service natively and the PowerShell installer can delegate to it.

Do not keep two independent full SCM-registration implementations.

Whichever path is chosen, acceptance requires:

- Automatic start type;
- existing `%ProgramData%\gregg\greggd.toml` preservation;
- service image path invokes hidden `service --config ...` entry;
- LocalService (or the current documented account) remains consistent;
- existing Windows `start`/`stop`/`restart` tests/smoke remain green.

## Phase 6: cross-platform `greggd restart`

### Windows

Reuse the existing native SCM restart implementation unchanged unless required by small refactoring.

### Linux/systemd managed

If the standard Gregg systemd unit is installed/active:

```text
systemctl restart greggd
```

Use the explicit CLI lifecycle boundary. If permission is denied, print:

```text
sudo systemctl restart greggd
```

and return nonzero/PermissionDenied rather than falling back to killing and directly spawning a second management mode.

### macOS/launchd managed

If the standard system job is loaded:

```text
launchctl kickstart -k system/com.eggstack.greggd
```

with clear privilege handling.

### Cron/direct managed

If no native system service is detected:

1. use the existing local `greggd stop` control path;
2. after successful/idempotent stop, invoke the same detached-start primitive used by `croncheck` or execute `croncheck` after the endpoint is confirmed absent;
3. do not create a separate PID-based restart implementation.

Be careful with the race where a cron watchdog could restart between stop and explicit start. Starting through `croncheck`/normal bind semantics is acceptable; a second start losing the bind race must not be misreported as data loss.

### Manager detection

Create one small `startup_state()`/equivalent helper that can answer enough for both `restart` and Plan 101 update:

```text
SystemdActive / SystemdInstalledStopped
LaunchdLoaded / LaunchdInstalledUnloaded
WindowsServiceRunning / WindowsServiceStopped
UnmanagedOrCron
```

Do not implement a generalized discovery database. Only identify Gregg's known standard manager registration.

## Phase 7: integrate bootstrap installer

Plan 099 installs binaries; this phase makes daemon install complete.

### Privileged Unix `greggd` install

After the verified executable is placed at `/usr/local/bin/greggd`:

```text
/usr/local/bin/greggd startup install
```

The installer should delegate startup ownership to the CLI rather than duplicating systemd/launchd/crontab logic.

### Unprivileged Unix `greggd` install

If auto-detection selects systemd/launchd but privilege is insufficient:

- do not silently create cron entries;
- print the exact elevated completion command;
- state where the downloaded binary was installed/staged;
- if the binary was installed user-locally, make clear that the standard system service expects `/usr/local/bin/greggd` and provide the exact system installation command.

If auto-detection selects cron, user-local installation may call:

```text
greggd startup install --method cron
```

without elevation.

### Component `both`

If run privileged, both client and daemon may be installed system-wide, then startup registration runs only for `greggd`.

No startup behavior is required for `gregg` itself.

## Phase 8: documentation

Update at minimum:

```text
README.md
crates/greggd/README.md
packaging/README.md
architecture/greggd-daemon.md
.opencode/skills/greggd-daemon/SKILL.md
RELEASING.md only if service assets affect release contents
plans/README.md
```

Document:

- `greggd startup install` auto-detection;
- `--method systemd|launchd|cron` explicit override;
- `startup instructions` as the no-mutation/manual-admin path;
- root/admin requirements;
- systemd service locations and commands;
- cron entries and `croncheck` semantics;
- launchd commands;
- Windows SCM install/start behavior;
- `greggd restart` manager-aware semantics;
- no silent sudo and no automatic fallback from an identified system manager to cron.

The docs should assume the operator understands basic system administration. Keep instructions exact, not introductory tutorials.

## Verification

### Parser/pure helpers

Add deterministic tests for:

- parser accepts startup/install/instructions/restart on intended platforms;
- auto method maps Linux-systemd/macOS/other Unix correctly through injected detection;
- explicit method overrides auto;
- cron render safely quotes spaces/single quotes or rejects unsafe control characters;
- managed cron block replacement is idempotent and preserves unrelated lines;
- instruction output contains exact standard paths/commands.

### Linux local operational smoke

On the available Ubuntu host, perform both a nonprivileged direct/cron-safe test and, only if the environment permits privilege, a real systemd smoke.

Minimum direct/read-only proof without privilege:

```text
greggd startup instructions -> selects systemd on Ubuntu and prints exact commands
greggd startup instructions --method cron -> prints canonical cron block
cron block renderer/install logic tested against a temporary/fake crontab boundary where practical
```

If root/sudo is available in the implementation environment, perform a real disposable systemd lifecycle using the standard service:

```text
install -> enable/start -> health ready -> restart -> health ready -> stop/cleanup
```

Do not require root-capable CI merely to automate this smoke. A local operator smoke is sufficient.

For non-systemd cron semantics, use a disposable user crontab only when it is safe to restore exactly. Otherwise test mutation through an injected command boundary plus a manual render inspection. Never overwrite an operator crontab irreversibly for tests.

### macOS/Windows

Use focused unit/parser tests locally and the existing native runners for syntax/compile truth.

Windows must retain the existing SCM lifecycle smoke.

macOS service-manager operations do not need a new privileged CI job. Validate rendered plist plus native compile; use a manual native smoke when available.

### Standard source checks

```bash
cargo fmt --all -- --check
cargo test -p greggd cli
cargo test -p greggd startup
cargo test -p greggd --bin greggd
cargo test -p greggd
./scripts/check-local.sh
./scripts/check-local.sh --release
```

Run clippy if not already included by the selected local check.

## Acceptance criteria

### Startup CLI

- [ ] `greggd startup install` exists and defaults to platform-appropriate auto detection.
- [ ] `startup instructions` is read-only and prints executable commands/paths for the selected method.
- [ ] explicit `--method systemd`, `launchd`, and `cron` are supported on appropriate Unix hosts.
- [ ] an identified systemd/launchd host never silently falls back to cron because privilege is missing.
- [ ] no startup command silently invokes `sudo`.

### systemd

- [ ] Standard system install uses `/usr/local/bin/greggd`, `/etc/gregg/greggd.toml`, dedicated `greggd` user/group, and `/etc/systemd/system/greggd.service`.
- [ ] Unit installation is idempotent and cannot leave a truncated file.
- [ ] Existing config is preserved.
- [ ] `daemon-reload`, enable, start/restart behavior is correct.
- [ ] Current useful hardening remains unless a specific compatibility failure justifies a documented change.
- [ ] Permission failure produces an exact elevated completion command.

### launchd

- [ ] Standard system plist path/label/config/binary paths remain consistent with packaging docs.
- [ ] Existing config is preserved.
- [ ] Bootstrap/kickstart behavior is idempotent enough for reinstall/update.
- [ ] Permission failure is explicit; no silent LaunchAgent/cron fallback occurs.

### cron

- [ ] `croncheck` is the sole daemon-health/start primitive used by the managed cron entry.
- [ ] Canonical entry includes reboot startup plus bounded periodic watchdog where supported.
- [ ] Installation preserves unrelated crontab entries and is idempotent.
- [ ] Paths/config arguments are safely shell-quoted.
- [ ] Missing `crontab` produces instructions rather than direct spool-file editing.
- [ ] Plan 091's valid-Gregg/refused/ambiguous watchdog semantics are the implementation baseline.

### restart

- [ ] Windows uses existing native SCM restart.
- [ ] active systemd installs restart through systemd.
- [ ] loaded launchd installs restart through launchd.
- [ ] unmanaged/cron installs reuse local stop + croncheck/detached-start semantics rather than PID scanning.
- [ ] privilege errors never trigger a competing fallback supervisor.
- [ ] restart behavior is factored so Plan 101 `greggd update` can reuse it.

### Installer integration and scope

- [ ] Plan 099's Unix daemon bootstrap delegates startup registration to the CLI after system binary installation.
- [ ] Windows has only one canonical SCM registration implementation after reconciliation.
- [ ] `gregg` client install behavior is unaffected except documentation/shared installer flow.
- [ ] `greggd run`, sampler, collectors, and HTTP server contain no service-manager/update logic.
- [ ] No PID files, process-name scans, public shutdown route, supervisor framework, new privileged CI job, or package-manager scope is introduced.

## Closure record

1. **Implementation SHA:** `a73a6a3` (feat: startup installation and restart) plus `2078924` (CRLF-normalized embedded unit/plist sync test for Windows) and `2271b9e` (drive refresh test wait with sleep to avoid Windows flake). Effective HEAD `2271b9e` verified by CI run `33680301250` (all five jobs: Linux, macOS Intel, macOS ARM64, Windows, MSRV).

2. **Final startup-method detection rules:** `StartupMethod::{Systemd,Launchd,Cron,WindowsScm,Direct}` with `StartupMethodArg::Auto` default. `auto_method_for(os, is_systemd)` maps `windows→SCM`, `macos|darwin→Launchd`, `linux` with `is_systemd_environment()→Systemd` else `Cron`, else `Cron`. `is_systemd_environment()` checks `/run/systemd/system` exists plus `/proc/1/comm == "systemd"` or bounded `systemctl is-system-running --quiet` probe when the directory exists but proc1 is unavailable. Explicit `--method` overrides auto; `resolve_startup_method` and `resolve_startup_method_with` preserve that invariant. No silent fallback from identified systemd/launchd to cron on permission failure.

3. **Canonical systemd/plist source ownership:** `startup::systemd_unit_content()` and `startup::launchd_plist_content()` return in-crate constants (not `include_str!` outside crate) so `cargo install` works without checkout. Templates are kept synchronized with `packaging/systemd/greggd.service` and `packaging/launchd/com.eggstack.greggd.plist` via `embedded_*_matches_packaging_file_when_present` tests that normalize `\r\n`→`\n` and check trailing newline. Standard paths remain `binary /usr/local/bin/greggd`, `config /etc/gregg/greggd.toml` (preserved if exists, `greggd` user/group, `0700` dir), unit `/etc/systemd/system/greggd.service` (atomic write via temp+rename+fsync, `daemon-reload` + `enable` + `start`/`restart`), plist `/Library/LaunchDaemons/com.eggstack.greggd.plist` (`bootstrap system` + `kickstart -k` when loaded, log `/var/log/greggd.log`). Hardening directives (`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `ReadOnlyPaths=/proc /sys`, `PrivateTmp`, `SystemCallFilter=@system-service`, etc.) preserved.

4. **Final cron block:**
```text
# greggd managed watchdog
@reboot '<exe>' --config '<config>' croncheck
* * * * * '<exe>' --config '<config>' croncheck
```
Rendered by `cron_block`/`cron_block_with_config` with `shell_quote` (single-quote + `'\''` escape, rejects `\n`/`\r`/control). `remove_managed_cron_block` identifies the `CRON_MANAGED_MARKER` and following `croncheck` lines; `merge_crontab` is idempotent, preserves unrelated entries byte-for-byte, strips old managed block, appends exactly one canonical block, never edits `/var/spool/cron`, prints manual lines when `crontab` missing. `croncheck` contract is Plan 091 final: valid Gregg Ready/Warming/Failed→running (no spawn), connection refused→spawn detached `greggd run` (stdio closed, new process group on Unix), ambiguous/unrelated/malformed/silent→nonzero without spawning.

5. **`greggd restart` manager-state rules:** `startup_state()` (small helper for both `restart` and Plan 101) maps `systemd_state_with(unit_exists, is_active)` and `launchd_state_with(plist_exists, is_loaded)` plus Windows SCM `is_active()` into `SystemdActive/SystemdInstalledStopped/LaunchdLoaded/LaunchdInstalledUnloaded/WindowsServiceRunning/WindowsServiceStopped/UnmanagedOrCron`. `restart_with_state` dispatches: `SystemdActive|InstalledStopped→systemctl restart greggd` (prints `sudo systemctl restart greggd` on permission), `LaunchdLoaded|InstalledUnloaded→launchctl kickstart -k system/com.eggstack.greggd`, `Windows*→SCM restart`, else `UnmanagedOrCron→control::send_stop` + detached `run` (same primitive as `croncheck`, 200ms post-stop sleep, race-safe via kernel bind). `restart_daemon` wraps `startup_state()` for CLI; factored for `update` reuse.

6. **Installer delegation behavior:** Unix `packaging/install.sh` after verified `curl -fsSL` download + `.sha256` + candidate `version` check installs to `/usr/local/bin` (root) or `$HOME/.local/bin` (user) then delegates: if `EUID==0` runs `${DEST_DIR}/greggd startup install` (auto) and warns with `sudo ${DEST_DIR}/greggd startup install` on failure; if unprivileged runs `startup install` and on systemd/launchd prints exact elevated command without silent cron fallback, on cron hosts installs user-local crontab. `both` installs `gregg` then `greggd` and delegates once. Windows `install.ps1` remains single canonical SCM registration (installs to `%ProgramFiles%\Gregg` when Administrator, `%LOCALAPPDATA%\Gregg` otherwise, preserves `%ProgramData%\gregg\greggd.toml`, registers `LocalService` auto, failure restart); `startup install` on Windows is state-reporting only (`startup instructions` prints SCM commands) with one implementation.

7. **Focused/local checks:**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p greggd --all-features -- startup (25 tests: auto detection, explicit override, shell quoting, cron rendering/quoting, block removal/merging idempotence, instruction paths, hardening, sync tests, state helpers)
cargo test --workspace --all-targets --all-features (248 tests)
cargo doc --workspace --no-deps
bash -n packaging/install.sh
scripts/verify-installed-daemon.sh target/debug/greggd (loopback health ready, status schema 2)
```

8. **Ubuntu systemd or instruction-level smoke depending available privilege:** Host `rasp10` (aarch64, Ubuntu 24.04, `systemd` owning PID 1, `/run/systemd/system` present) `greggd startup instructions` correctly selects `systemd` and prints `/usr/local/bin/greggd`, `/etc/gregg/greggd.toml`, `/etc/systemd/system/greggd.service` plus `systemctl daemon-reload`/`enable`/`restart` steps. `startup instructions --method cron` prints canonical `# greggd managed watchdog` block with quoted `@reboot` + `* * * * *` lines and no PID-file note. `startup install --method cron` (user-local) is idempotent and preserves unrelated crontab; second `* * * * *` install replaces previous block (marker count =1). `restart` via direct (no systemd unit installed) does `stop` (NotRunning) + detached `run` and health becomes `ready` on loopback `127.0.0.1:11399`; subsequent `stop` succeeds and health fails as expected. Privileged systemd real install not executed (would require `sudo` and `/usr/local/bin/greggd` at standard path; returned actionable `BinaryMissing` error as designed).

9. **Native Windows CI/SCM result:** CI run `33680301250` Windows job passed all 248 tests including `startup` suite and SCM lifecycle; embedded unit/plist sync tests now normalize CRLF so Windows checkout passes. Previous flake `filtered_drive_enumeration_is_successful_empty` (drive refresh worker race) fixed by `sample_until_drives` sleeping 5ms ×200 instead of `yield_now` ×100. No second cron/LaunchAgent fallback on permission failure.

10. **macOS limitations if a privileged launchd smoke was not available:** No privileged macOS host was available in implementation environment. launchd install/bootstrap/kickstart verified via pure helper tests and native compile (`cargo check --workspace --all-targets --all-features` on `macos-15` and `macos-15-intel` both succeeded). Rendered plist matches packaging file; `startup instructions --method launchd` prints `/Library/LaunchDaemons/com.eggstack.greggd.plist` and `launchctl bootstrap/kickstart` commands. A manual privileged `sudo greggd startup install --method launchd` smoke remains operator evidence and is not required for CI.

Do not close Plan 100 while Plan 091's croncheck behavior is still ambiguous relative to the cron supervisor contract used here — closed against Plan 091 final semantics (valid Gregg health = running, refusal = spawn, ambiguous = nonzero without spawn).

## Closure evidence (2026-09-02)

All Plan 100 acceptance criteria satisfied; `greggd run`/`sampler`/`collectors`/`HTTP` remain service-manager unaware; no PID files, process scanning, public shutdown route, supervisor framework, new privileged CI job, or package-manager scope added. Bootstrap installers delegate startup to CLI; Windows has one canonical SCM implementation.