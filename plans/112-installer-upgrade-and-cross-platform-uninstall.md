# Plan 112: installer upgrade semantics and cross-platform uninstall

Status: planned; ready for implementation.

Depends on: Plans 099-105, especially the bootstrap installer contract from Plan 099, startup ownership from Plan 100, self-update behavior from Plans 101-102, and shared `gregg-update` / startup module boundaries from Plans 104-105.

This plan may proceed independently of the remaining Plan 091 soak record. It must preserve the settled `croncheck`, direct-stop, service-manager, update, release, and protocol contracts.

## Objective

Make installation lifecycle behavior explicit and safe without turning Gregg into a package manager:

1. codify and test that rerunning the canonical installer in the same privilege/scope upgrades an existing `gregg` and/or `greggd` installation in place;
2. make installer output distinguish a first install from an in-place replacement where practical;
3. add component-specific `uninstall` CLI commands that remove the exact invoked binary and Gregg-owned startup integration on Linux, macOS, and Windows;
4. preserve configuration and operator data by default, with an explicit `--purge` opt-in;
5. avoid stale Cargo ownership metadata for future bootstrap Cargo fallbacks;
6. retire the current Windows directory-recursive uninstall behavior in favor of the same component-safe lifecycle contract.

This is bounded install/uninstall lifecycle work. It is not a new package manager, package-manager discovery framework, service architecture redesign, or release pipeline redesign.

## Confirmed baseline

The current bootstrap installers already behave as same-scope updaters.

### Unix bootstrap

`packaging/install.sh` selects its destination from privilege:

```text
root      -> /usr/local/bin
non-root  -> $HOME/.local/bin
```

For a prebuilt release asset it verifies SHA-256 and candidate identity/version, then writes the verified candidate to `${DEST_DIR}/gregg` or `${DEST_DIR}/greggd`. It does not reject an existing destination. Rerunning the same installer with the same privilege therefore replaces the existing binary.

For `greggd`, the bootstrap then delegates startup integration to `greggd startup install`. Existing systemd/launchd registrations are updated/restarted by the daemon-owned startup code and the cron path replaces its managed block idempotently.

The important limitation is scope: a non-root rerun does not discover or replace `/usr/local/bin/gregg[d]`, and a root rerun does not search for a user-local copy. That is desirable. The installer should not search arbitrary `PATH` locations or silently cross privilege boundaries.

### Windows bootstrap

`packaging/install.ps1` selects:

```text
Administrator  -> %ProgramFiles%\Gregg
regular user   -> %LOCALAPPDATA%\Gregg
```

It overwrites the selected component with `Copy-Item -Force`. For an Administrator `greggd` installation it stops a running service, preserves `%ProgramData%\gregg\greggd.toml`, replaces the executable, updates or creates the SCM registration, and starts the service again.

As on Unix, the behavior is same-scope replacement rather than global discovery of every Gregg copy on the machine.

### Self-update

`gregg update` and `greggd update` already solve the general installed-location problem more cleanly than a bootstrap script can: the shared `gregg-update` crate resolves `current_exe()`, stages and verifies a candidate, checks write permission, and replaces the exact binary that was invoked. `greggd` separately coordinates daemon activation/restart.

Plan 112 must not duplicate this update machinery in installer scripts or add a second installation locator.

### Existing uninstall surface

The only dedicated uninstaller is `packaging/uninstall-windows.ps1`. It is daemon-oriented, Administrator-only, and recursively removes `%ProgramFiles%\Gregg`. That is unsafe for component ownership because the same directory can contain both `gregg.exe` and `greggd.exe`.

Unix documentation currently describes manual service/file removal but there is no CLI-level uninstall path.

### Existing teardown primitives

Several required primitives already exist and should be reused:

- `gregg-update` owns current-executable resolution, install-directory permission probing, and the existing `self-replace` dependency;
- systemd and launchd modules already own canonical Gregg paths and bounded manager invocation;
- `startup::cron::remove_managed_cron_block` already removes only the `# greggd managed watchdog` block while preserving unrelated crontab content;
- the Windows service module already owns native SCM state/start/stop behavior;
- client and daemon configuration modules already own platform default config paths.

Do not duplicate these policies in new shell/PowerShell uninstall scripts.

## Scope decisions

### 1. Keep installer updates same-scope and explicit

The canonical bootstrap contract is:

> Rerunning the same installer at the same install scope replaces that scope's selected Gregg component with the requested/latest verified version.

Do not add automatic searches of `PATH`, home directories, `/usr/local`, Program Files, or other users' installations. Do not automatically escalate privileges. Do not make an unprivileged install mutate a system-wide installation merely because one exists.

When the canonical destination already contains the expected Gregg component, installer output should identify the operation as an update/replacement and show the existing and candidate versions when both can be obtained safely.

If an existing destination cannot identify itself as the expected component through the stable `version` command, the installer must not casually describe it as a Gregg upgrade. Prefer failing before overwrite with an actionable diagnostic rather than silently clobbering an unrelated executable at a canonical path. Do not add a broad `--force` mode unless implementation review demonstrates a concrete legitimate case that cannot otherwise be handled.

Installer tests must cover first install, same-version rerun, newer-version replacement, and an unexpected existing destination.

### 2. Keep `update` as the preferred installed-binary upgrade command

Documentation should distinguish:

```text
bootstrap installer rerun  -> install/reinstall at the selected user/system scope
gregg update               -> update the exact invoked client binary
greggd update              -> update the exact invoked daemon binary + manager-aware restart
```

Do not route `update` through `install.sh` / `install.ps1`, and do not route bootstrap installation through the self-updater.

### 3. Add component-specific uninstall commands

Add:

```text
gregg uninstall [--dry-run] [--purge]
greggd uninstall [--dry-run] [--purge]
```

Exact Clap struct placement is implementation detail, but the public behavior is fixed by this plan.

`uninstall` always applies to the component whose binary is executing. It must not remove the sibling component merely because both binaries share an installation directory.

There is intentionally no `uninstall --all` in this phase. An operator that installed both components may run both commands explicitly.

`--dry-run` must perform discovery/planning only and print the exact Gregg-owned resources that would be changed or removed. It must not stop a daemon, mutate service state, edit crontab, delete configuration, or schedule self-deletion.

`--purge` removes selected-component configuration/data in addition to the executable and startup integration. Without `--purge`, configuration and operator data are preserved.

Do not add an interactive confirmation prompt. Invoking `uninstall` is the explicit destructive action; scripts and remote administration must remain noninteractive.

### 4. Remove only the exact invoked executable

Binary removal must resolve the exact current executable rather than assume `/usr/local/bin`, `%ProgramFiles%\Gregg`, `$HOME/.local/bin`, or a Cargo path.

Extend the shared install-lifecycle mechanics already adjacent to `gregg-update` rather than copy platform deletion code into both application crates. The shared layer may own only generic executable operations:

- current executable resolution;
- writable-parent preflight;
- caller-specific permission/elevation diagnostics;
- self-deletion of the running executable;
- cross-platform deletion scheduling required by Windows running-image semantics.

It must remain unaware of systemd, launchd, cron, SCM, client config, daemon config, or TUI state.

The existing `self-replace` dependency should be reused for supported self-delete behavior if its API satisfies the required Unix/Windows semantics under the workspace MSRV. Do not add another self-update/delete dependency without a demonstrated gap.

Never recursively remove an executable directory. In particular:

```text
%ProgramFiles%\Gregg
%LOCALAPPDATA%\Gregg
/usr/local/bin
$HOME/.local/bin
```

are containers, not component-owned artifacts.

If an empty Gregg-specific Windows directory remains after deleting one component, removing that now-empty directory is optional cleanup; it must never be required for success and must never remove a sibling binary or unknown file.

### 5. Preflight before daemon teardown

`greggd uninstall` must avoid the failure mode:

```text
stop/disable service -> discover binary cannot be removed -> leave broken partial uninstall
```

Before mutating daemon/service state, resolve the uninstall plan and perform every practical permission/path preflight for:

- the current executable;
- service/unit/plist artifacts that will be removed;
- config/data paths when `--purge` is requested;
- cron mutation availability when a managed cron block exists.

No preflight can eliminate every race, so teardown operations still need precise errors and idempotent retry behavior.

Permission failures must follow the existing project rule: never invoke `sudo` internally. Print the exact elevated command, for example:

```text
sudo /usr/local/bin/greggd uninstall
sudo /usr/local/bin/greggd uninstall --purge
```

Windows errors should instruct the operator to rerun the exact installed executable from an Administrator shell when SCM/system paths require it.

### 6. Teardown startup integration by owned artifact, not only current auto-detection

Do not use only `auto_detect_method()` / `startup_state()` to decide what to remove. Those helpers answer the active/default manager question for restart/update and deliberately collapse cron/unmanaged state.

Uninstall has a different requirement: remove Gregg-owned startup artifacts that actually exist for the selected daemon component.

The implementation should independently inspect the platform-relevant Gregg artifacts and construct a deterministic teardown plan. This also handles legacy/mixed states such as a stale systemd unit plus a managed cron block.

Do not search for generic process names, arbitrary unit files, arbitrary launchd labels, arbitrary crontab commands, or unrelated scheduler entries.

### 7. Linux systemd uninstall

When the canonical Gregg systemd unit exists or the manager reports `greggd` installed/active, the daemon uninstall path must, as applicable:

```text
systemctl stop greggd
systemctl disable greggd
remove /etc/systemd/system/greggd.service
systemctl daemon-reload
```

All manager calls remain bounded using the existing startup process boundary.

Missing/stopped/disabled state is idempotent, not an error by itself. A genuine manager failure or permission denial must be surfaced.

Do not remove another package's unit merely because its service name resembles Gregg. The canonical Gregg unit path/service identity is the ownership boundary.

Do not remove the `greggd` system account in this phase. User/group deletion has broader ownership implications and is unnecessary for a clean executable/service uninstall.

### 8. macOS launchd uninstall

When the canonical Gregg plist exists or the Gregg launchd label is loaded:

- boot out `system/com.eggstack.greggd` when loaded;
- remove `/Library/LaunchDaemons/com.eggstack.greggd.plist`;
- preserve daemon configuration and logs unless `--purge` is set.

Missing/unloaded state is idempotent.

Reuse the existing bounded launchctl execution path and canonical startup constants. Do not introduce a parallel plist/path table in CLI code.

### 9. Cron uninstall

Add a small mutation wrapper around the existing managed-block parser:

```text
crontab -l
-> remove_managed_cron_block(...)
-> install resulting crontab only when changed
```

It must preserve unrelated crontab entries byte-for-byte where the existing helper already guarantees that behavior.

No crontab or no Gregg managed marker is a successful no-op. A missing `crontab` executable is only an uninstall error when the command has positive evidence that Gregg cron integration must be removed; otherwise it should not block removal of an unmanaged binary.

Cron cleanup is scoped to the current account whose crontab is being edited. Do not enumerate other users' crontabs or edit `/var/spool/cron` directly.

### 10. Direct/unmanaged daemon uninstall

If no native startup manager artifact owns the running daemon, use the existing direct control path to stop the daemon when it is running under the selected config identity before deleting the binary.

Preserve the existing stop-safety rules: do not use `pkill`, process-name scanning, PID files, or a public shutdown endpoint.

A daemon that is already absent is a successful state. An uncertain stop outcome must block self-deletion rather than knowingly orphan a running process whose executable has disappeared.

### 11. Windows SCM uninstall

Extend the native Windows service abstraction with a narrowly scoped service unregister/delete operation. Prefer the existing `windows-service` dependency/native SCM API rather than adding a new permanent `sc.exe` shell-out path.

The sequence must be:

1. resolve/preflight uninstall;
2. stop `greggd` when running and wait for the bounded stopped state;
3. delete only the `greggd` SCM registration;
4. preserve `%ProgramData%\gregg\greggd.toml` unless `--purge`;
5. schedule/delete the exact running `greggd.exe` using the shared self-delete primitive;
6. exit promptly so Windows deferred self-delete can complete.

SCM not-installed/stopped states are idempotent. Access denied maps to the existing permission exit taxonomy.

Add deterministic fake-adapter coverage for service deletion before relying on the native CI smoke.

### 12. Configuration/data purge policy

Default uninstall preserves configuration. This is important for reinstall/rollback and prevents a routine binary uninstall from destroying a fleet definition or daemon tuning.

With `--purge`:

#### `gregg`

Remove the resolved client config file:

```text
Linux    $XDG_CONFIG_HOME/gregg/gregg.toml or ~/.config/gregg/gregg.toml
macOS    ~/Library/Application Support/gregg/gregg.toml
Windows  %APPDATA%\gregg\gregg.toml
```

If the default Gregg-specific parent directory becomes empty, it may be removed. If `--config PATH` points somewhere custom, remove only the exact file; never recursively remove its arbitrary parent directory.

#### `greggd`

For standard system installs, `--purge` may remove the standard daemon config file and then the Gregg-specific config directory when empty:

```text
Linux    /etc/gregg/greggd.toml
macOS    /Library/Application Support/gregg/greggd.toml
Windows  %ProgramData%\gregg\greggd.toml
```

For macOS, remove the Gregg daemon log file created by the installer/startup path only under `--purge`.

For an explicit custom config path, remove only the exact config file. Never recursively delete an arbitrary explicit config parent.

Do not remove unrelated files merely because they are inside a directory named `gregg`. Directory removal is allowed only when the directory is one of Gregg's known standard directories and is empty after selected-component files are removed.

### 13. Cargo install ownership and bootstrap fallback

Do not leave future canonical bootstrap installs with Cargo package-tracking metadata that no longer matches the installed binary.

Change bootstrap Cargo fallback behavior to use a private temporary Cargo root as a build/staging mechanism, verify the staged binary exactly as today, then copy/install that binary into the normal bootstrap destination. The temporary Cargo root is deleted afterward. This mirrors the shared updater's staged Cargo fallback and makes the bootstrap installer, not Cargo metadata, the owner of the final destination.

This applies to Unix source-only fallback and Windows source-only fallback. It must not change the final documented install locations.

Direct operator use of:

```text
cargo install gregg
cargo install greggd
```

remains Cargo-owned. Do not parse or mutate Cargo's private tracking files directly.

Where the uninstall implementation can positively identify that the exact running binary is Cargo-managed using Cargo's own supported interface, preserve Cargo ownership semantics rather than self-deleting and leaving stale metadata. The safest accepted behavior is:

- on platforms where invoking `cargo uninstall --root <root> <package>` from the running program is known and tested to work, delegate to Cargo;
- if the running-image/platform semantics make a safe synchronous Cargo removal impossible, fail before any service/config mutation and print the exact `cargo uninstall --root <root> <package>` command for the operator to run after the current process exits.

Do not guess Cargo ownership solely because a path contains `.cargo`, and do not make Cargo a runtime requirement for ordinary bootstrap/manual-binary uninstall.

A direct Cargo install may therefore use a package-manager handoff rather than Gregg bypassing Cargo bookkeeping. That is intentional and must be documented as the safe ownership boundary.

### 14. Do not add a persistent install-receipt subsystem in this phase

A receipt could eventually help distinguish bootstrap, OS package-manager, Cargo, and manual-copy provenance, but the currently supported lifecycle does not require a new persistent registry.

For Plan 112, use:

- exact `current_exe()` identity for binary removal;
- canonical Gregg-owned service/startup artifacts;
- Cargo's supported ownership surface when positively identified;
- explicit `--purge` for data removal.

If future Homebrew/apt/winget/MSI packages require package-manager-aware uninstall, add a separate plan and receipt/metadata contract then. Do not anticipate that ecosystem here.

### 15. Replace/deprecate the standalone Windows uninstaller safely

`packaging/uninstall-windows.ps1` must no longer recursively delete `%ProgramFiles%\Gregg`.

Preferred end state: make it a thin compatibility wrapper around the installed CLI uninstall command, mapping its existing `-RemoveConfig` intent to `--purge`, or deprecate it in documentation if a wrapper cannot be kept truthful without duplicating lifecycle behavior.

Whichever route is chosen:

- uninstalling `greggd` must not delete `gregg.exe`;
- uninstalling `gregg` must not delete `greggd.exe`;
- no PowerShell script should retain a second independent SCM teardown implementation once the Rust CLI owns it.

## Implementation sequence

### Step 1: lock down installer-rerun behavior

Add focused installer tests around existing destination handling before changing implementation.

For Unix and PowerShell bootstrap installers, cover:

- missing destination -> first install;
- valid same-component destination -> replacement/update;
- same version -> safe idempotent replacement or explicit already-current result;
- pinned older/newer versions still honor the requested tag rather than silently forcing latest;
- foreign/unidentifiable destination -> no silent Gregg-upgrade claim and no accidental overwrite;
- root/admin versus user-local scope remains unchanged.

Avoid network-dependent unit tests. Use existing script test patterns with fake release artifacts/commands or extracted pure decision helpers where practical.

### Step 2: make Cargo fallback staging-only

Refactor `install.sh` and `install.ps1` Cargo fallback to an owner-private temporary root, validate the staged candidate, then install/copy only the final executable into the selected bootstrap destination.

Do not change the release-first fallback trigger: only an unsupported/source-only host or exact prebuilt-asset absence should select Cargo. Checksum/version mismatch remains a hard failure.

### Step 3: add shared executable-uninstall primitives

In `gregg-update` or an equivalently small existing lifecycle boundary:

- generalize permission probing so its elevated hint can say `uninstall` rather than hard-coded `update`;
- expose exact current-executable resolution needed by both binaries;
- add tested self-delete scheduling/removal with the already-present dependency;
- keep the crate service-manager-free.

Do not create a fifth workspace crate solely for two or three generic deletion functions unless implementation demonstrates that `gregg-update` becomes materially incoherent.

### Step 4: add `gregg uninstall`

Implement client command parsing, dry-run plan rendering, default config preservation, optional purge, package-manager handoff when positively identified, and exact executable self-delete.

Client uninstall must not initialize the TUI runtime.

### Step 5: add daemon startup teardown primitives

Add the inverse operations beside the existing startup owners:

```text
startup/systemd.rs  -> uninstall/remove systemd integration
startup/launchd.rs  -> uninstall/remove launchd integration
startup/cron.rs     -> uninstall managed cron block
service/windows.rs  -> unregister/delete SCM service
```

Keep command execution bounded and error-returning. Reuse canonical path constants.

### Step 6: add `greggd uninstall`

Build a deterministic uninstall plan first, preflight it, then execute teardown in an order that cannot knowingly leave a running daemon detached from its manager.

The implementation should separate pure plan/discovery data from mutation enough that `--dry-run` and unit tests exercise the same ownership decisions as the real command.

### Step 7: reconcile Windows compatibility script

Turn `packaging/uninstall-windows.ps1` into a thin compatibility entry point or retire it from the preferred documentation surface. Remove recursive shared-directory deletion.

### Step 8: update docs/architecture/skills

Because this introduces user-visible CLI and installer behavior, update the active documentation in the same implementation pass.

At minimum review/update:

```text
README.md
CHANGELOG.md
packaging/README.md
docs/installation.md
docs/client.md
docs/daemon.md
crates/gregg/README.md
crates/greggd/README.md
architecture/gregg-update.md
architecture/scripts-and-packaging.md
architecture/gregg-client.md
architecture/greggd-daemon.md
AGENTS.md
.opencode/skills/gregg-client/SKILL.md
.opencode/skills/greggd-daemon/SKILL.md
.opencode/skills/release-process/SKILL.md
plans/README.md
```

Document explicitly that configuration is preserved by default and `--purge` is destructive.

## Verification

Use the lightest checks that prove the ownership and platform behavior. Do not add a new workflow, matrix, privileged runner, evidence bundle, or package-manager test farm.

### Deterministic tests

At minimum add coverage for:

- current-executable uninstall path resolution;
- permission preflight and exact elevated-command rendering;
- dry-run has zero mutations;
- client default-preserve versus `--purge` behavior;
- explicit custom config purge never deletes its parent recursively;
- sibling binaries in one Windows Gregg directory are not removed;
- systemd teardown command ordering and missing-state idempotence through injected/fake command execution;
- launchd teardown ordering and missing-state idempotence;
- cron managed-block removal with unrelated entries preserved;
- mixed stale startup artifacts are independently discovered rather than hidden by auto manager selection;
- direct/unmanaged uncertain stop blocks binary deletion;
- Windows SCM delete via fake adapter, including access denied and already-not-installed cases;
- Cargo-owned detection never guesses from pathname alone;
- canonical bootstrap Cargo fallback leaves no persistent Cargo install root/metadata outside temporary staging;
- installer rerun replacement semantics on Unix and PowerShell helpers.

### Ubuntu/local smoke

On the available Ubuntu environment, use disposable paths/configuration; do not alter the operator's real Gregg install.

Demonstrate at least:

```text
install staged gregg -> rerun installer path over it -> version changes/validates -> gregg uninstall -> binary absent -> config preserved
```

and a direct/unmanaged daemon lifecycle with a temporary config:

```text
greggd run -> health ready -> dry-run uninstall shows intended actions only -> daemon remains ready -> real uninstall/teardown in disposable install root -> daemon stopped -> binary absent -> config preserved
```

If the host is a real systemd environment and the smoke can be performed safely without touching an operator install, a temporary/unit-scoped systemd proof is useful but is not required to create privileged CI infrastructure. Deterministic systemd tests remain authoritative for command sequencing.

### Windows/macOS truth

Use the existing native CI only.

Extend the existing Windows smoke enough to prove the actual component-safety regression:

```text
install both -> uninstall greggd -> SCM registration gone -> greggd.exe gone -> gregg.exe still runnable
```

Then verify the client uninstall path in an isolated install location where the smoke can safely do so. Configuration-preserve/purge can be primarily deterministic tests if system directories would make the CI smoke brittle.

macOS should compile/test launchd planning and path logic in the existing jobs. Do not require privileged launchd mutation in CI.

### Standard gates

Run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
./scripts/check-local.sh --release
rustup run 1.75 cargo check --workspace --all-features
```

The release preflight is appropriate because bootstrap installer behavior changes in this plan.

Use one ordinary existing remote CI run for native Windows/macOS/MSRV truth after implementation. Record its exact run ID and implementation SHA in the closure record.

## Preserved exclusions

Do not add any of the following under Plan 112:

- global discovery/removal of every Gregg binary on a host;
- an `uninstall --all` fleet/package-manager command;
- internal `sudo`, UAC prompting, or privilege escalation;
- automatic deletion of the `greggd` system user/group;
- process-name scanning, `pkill`, PID files, or public shutdown API;
- recursive deletion of arbitrary install/config directories;
- mutation of other users' crontabs;
- parsing/writing Cargo private metadata directly;
- Homebrew, apt/dpkg, rpm, winget, MSI, or other package-manager integration not already present;
- a persistent install receipt/registry unless a separately demonstrated blocker makes it necessary;
- updater/release asset contract changes;
- protocol, metrics, TUI, EggPool, polling, or collector work;
- a new dependency when the existing self-replace/service dependencies suffice;
- a new CI workflow/job/matrix, privileged runner, artifact/evidence system, or release automation;
- rewriting closed Plans 099-105 to pretend uninstall was part of their original scope.

## Acceptance criteria

Plan 112 is complete only when all of the following are true:

1. [ ] `install.sh` and `install.ps1` have deterministic coverage proving same-scope reruns replace/update the selected existing Gregg component without global install discovery.
2. [ ] Installer output truthfully distinguishes first install from an identified existing Gregg replacement, and an unrelated destination executable is not silently treated as a Gregg upgrade.
3. [ ] Bootstrap Cargo fallback uses temporary staging and no longer leaves Cargo ownership metadata for the final bootstrap destination.
4. [ ] `gregg uninstall` and `greggd uninstall` exist with `--dry-run` and `--purge`.
5. [ ] Both commands remove only the exact invoked component executable; a sibling `gregg`/`greggd` binary sharing a directory survives.
6. [ ] Configuration/data is preserved by default on Linux, macOS, and Windows.
7. [ ] `--purge` removes only the selected component's known/resolved config/data files and never recursively removes an arbitrary explicit config parent.
8. [ ] `greggd uninstall` preflights practical permissions before mutating service/daemon state and never invokes `sudo` internally.
9. [ ] Linux systemd teardown stops/disables only `greggd`, removes the canonical Gregg unit, reloads systemd, and is idempotent for missing/stopped state.
10. [ ] macOS launchd teardown unloads the Gregg label when necessary, removes only the canonical Gregg plist, and is idempotent for missing/unloaded state.
11. [ ] Cron teardown removes only the Gregg managed watchdog block and preserves unrelated crontab content.
12. [ ] Direct/unmanaged daemon uninstall uses the existing safe control identity and refuses binary deletion after an uncertain stop.
13. [ ] Windows SCM teardown is owned by the native service abstraction, stops/waits/deletes only `greggd`, and maps permission failures through the existing exit taxonomy.
14. [ ] Windows running-image self-deletion is handled without recursively deleting `%ProgramFiles%\Gregg` or `%LOCALAPPDATA%\Gregg`.
15. [ ] Direct Cargo-owned installs preserve package-manager bookkeeping: use Cargo when safe/tested or fail before mutation with an exact `cargo uninstall --root ...` handoff rather than silently leaving stale tracking state.
16. [ ] `packaging/uninstall-windows.ps1` no longer owns an independent recursive-directory/SCM uninstall implementation that can delete the sibling component.
17. [ ] Ubuntu disposable install/rerun/uninstall smoke passes for the client, and a disposable direct daemon dry-run/real-uninstall lifecycle proves config preservation and daemon shutdown.
18. [ ] Existing Windows CI smoke demonstrates `install both -> uninstall greggd` leaves `gregg.exe` runnable and removes the daemon service/binary.
19. [ ] Existing macOS/Windows/MSRV CI remains green; no new workflow or matrix is added.
20. [ ] Current README/docs/architecture/skills describe installer rerun, update, uninstall, dry-run, purge, Cargo ownership, and privilege behavior accurately.
21. [ ] `./scripts/check-local.sh`, release preflight, workspace tests/clippy/fmt, and Rust 1.75 workspace check pass on the implementation tree.
22. [ ] The closure record names the implementation SHA, exact CI run used for native-platform truth, local lifecycle smoke results, and any platform limitation encountered without overstating evidence.

## Closure record

Not yet implemented. Do not mark this plan complete until every applicable acceptance criterion above is demonstrated on the implementation tree.