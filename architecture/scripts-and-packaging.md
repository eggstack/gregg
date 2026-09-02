# Scripts and packaging deep dive

This document covers the build/test scripts, installer scripts, service
definitions, and packaging infrastructure.

## Scripts

**Source:** `scripts/`

### check-local.sh / check-local.ps1

Primary local validation scripts. Two modes:

**Default mode (short routine loop):**
1. `cargo fmt --all -- --check`
2. `cargo test --workspace`

**`--release` mode** (adds):
1. Full workspace Clippy and documentation
2. Clean-tree check (no uncommitted changes)
3. Version consistency (workspace version matches all crates)
4. `cargo package --list` for each crate
5. Installed-binary loopback smoke (installs greggd, runs health/status check)
6. `cargo publish --dry-run` for gregg-protocol

### verify-installed-daemon.sh

Bounded loopback smoke test for greggd:

1. Allocates isolated port (Python `socket.bind`)
2. Writes temp TOML config
3. Starts greggd
4. Polls `/v2/healthz` until `ready` (configurable deadline)
5. Validates `/v2/status` JSON against jq schema
6. Sends SIGTERM and verifies clean shutdown

Handles port collisions with retry logic. Used by both `check-local.sh` and
`check-local.ps1` release modes.

### test-verify-installed-daemon.sh

Unit tests for the verifier script. Exercises against a fake daemon
(`scripts/tests/fake-greggd.py`) to confirm correct failure on:
- Invalid binary path
- Startup failure
- Health timeout
- Malformed status JSON
- Nonzero shutdown

### smoke-windows.ps1

Full Windows service lifecycle smoke test, run by the existing Windows CI job
on `windows-2022` with Administrator privileges:
- install → native SCM start → health/status → stop → start → restart;
- config mutation and custom config-path persistence;
- bind failure on an occupied ephemeral loopback port and recovery;
- reinstall with `LocalService` and config preservation;
- configured `system.name` plus nonempty, NUL-free `system.hostname`;
- service, binary, and temporary-config cleanup.

It invokes the installed `greggd.exe` explicitly for lifecycle/configuration
commands, checks every required `sc.exe` result, and fails cleanup assertions
instead of converting them to warnings. Local manual runs require
Administrator privileges.

### run-mixed-fleet-sustained.py

Diagnostic tool for sustained mixed-fleet workloads:
- Builds the `gregg` test binary
- Launches as child process, samples `/proc/<pid>/status`
- Validates written summary JSON against strict contract
- Optional, not part of CI

## Packaging

**Source:** `packaging/`

### Bootstrap installers (binary-first, Plan 099)

The current `v1.0.11` release is source-only, so Cargo remains the working
installation path until the first binary-bearing release publishes these
assets. The `latest/download` and pinned-release installer URLs below are the
forward-looking contract for those binary-bearing releases, not commands that
claim to work against today's release.

| Script | Platform | Mode |
|--------|----------|------|
| `install.sh` | Linux, macOS | bootstrap: download prebuilt `gregg-<target>`/`greggd-<target>` + `.sha256`, verify, install to `/usr/local/bin` (root) or `$HOME/.local/bin` (user); Cargo fallback for `armv7l`/unknown |
| `install.ps1` | Windows | bootstrap: `Invoke-WebRequest` + `Get-FileHash` + candidate `version` check, `ProgramFiles\Gregg` (admin) vs `LOCALAPPDATA\Gregg` (user), SCM registration preserved, Cargo fallback for ARM64/unknown |

**`install.sh` contract (Unix):**

- `install.sh gregg` / `install.sh greggd` / `install.sh both` plus optional `--version X.Y.Z`;
- no-arg on a TTY shows a tiny selector, piped noninteractive without component prints usage and exits nonzero;
- maps `uname -s`/`uname -m` to `x86_64-unknown-linux-gnu` (Linux x86_64/amd64), `aarch64-unknown-linux-gnu` (Linux aarch64/arm64), `x86_64-apple-darwin` (Darwin x86_64), `aarch64-apple-darwin` (Darwin arm64), `armv7l` → `armv7-unknown-linux-gnueabihf` source-only;
- constructs `https://github.com/eggstack/gregg/releases/latest/download/<asset>` or `.../download/vX.Y.Z/<asset>` for pinned; requires fixed `eggstack/gregg` prefix and `curl -fsSL`;
- downloads into a fresh `mktemp -d` with `trap` cleanup, fetches `<asset>.sha256`, verifies via `sha256sum` (Linux) or `shasum -a 256` (macOS) before any `chmod +x` or execution, runs `<candidate> version` and requires the expected program name and exact version when pinned, never installs a partial download, never falls back to Cargo on checksum/version mismatch;
- destination `/usr/local/bin` when `EUID=0` else `$HOME/.local/bin`, warns when the dest is not on `PATH`, never edits shell rc files, never silently invokes `sudo`;
- unsupported hosts and ARMv7 go to Cargo fallback: `cargo install --locked` with `--version "=X.Y.Z"` when pinned and `--root` derived from the destination;
- after a verified `greggd` install, delegates startup to `greggd startup install` (auto) so systemd/launchd/cron logic lives in the binary: privileged runs `daemon-reload`/`enable`/`start`/`restart` or `bootstrap`/`kickstart -k` or idempotent crontab; unprivileged on systemd/launchd prints exact `sudo <exe> startup install --method <...>` without silent cron fallback; on cron hosts installs user-local crontab without elevation.

**`install.ps1` contract (Windows):**

- `-Component Gregg|Greggd|Both` plus optional `-Version X.Y.Z`;
- detects `PROCESSOR_ARCHITECTURE`/`Is64BitOperatingSystem` (`AMD64` → `x86_64-pc-windows-msvc`; `ARM64`/unknown → source-only fallback);
- constructs the same `latest/download` / `download/vX.Y.Z` URLs for `gregg-<target>.exe` / `greggd-<target>.exe` and `.sha256`;
- `Invoke-WebRequest` to a private temp dir, `Get-FileHash -Algorithm SHA256` verification, candidate `version` check;
- installs `gregg` user-local where appropriate and `greggd` to `%ProgramFiles%\Gregg` when Administrator (preserving `%ProgramData%\gregg\greggd.toml`), with SCM registration (`LocalService`, `auto` start, failure restart) owned by the installer; `startup install` on Windows is state-reporting only (`startup instructions` prints SCM commands) and there is a single canonical SCM implementation.

Raw executables are published, not per-target tarballs/zip files; Windows `.exe` is already directly executable.

### Self-update (`gregg update` / `greggd update`, Plans 101-102)

Both binaries share the same binary-first contract (no fourth public crate; small duplication is intentional):

- `env!("CARGO_PKG_VERSION")` is the local version; crates.io `max_stable_version` (via `curl -fsSL --max-time 15 -H "User-Agent: gregg/<version> ..." https://crates.io/api/v1/crates/<crate>`) is the authority; GitHub `latest` is never authoritative.
- SemVer-safe `MAJOR.MINOR.PATCH` compare; equal version exits 0 without file mutation.
- Host mapping `detect_target()` -> `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc[.exe]` (ARMv7/unknown -> Cargo fallback).
- Exact URLs `https://github.com/eggstack/gregg/releases/download/vX.Y.Z/<program>-<target>[.exe]` and `.sha256`; only HTTP 404 permits `cargo install --locked --version "=X.Y.Z" --root <temp>` staged then verified; 5xx/transport/checksum/version mismatch are hard errors.
- Bounded `curl -fsSL --max-time 90` download to an exclusive owner-private `tempfile::TempDir`, SHA-256 via `sha2` crate (not platform tools) before any `chmod +x` or execution, then `candidate version` must equal `"<program> X.Y.Z"` with exit 0.
- Staged before touching current exe; `current_exe()` derived destination (never assumed prefix); symlink policy: if `current_exe()` is a symlink, replace the resolved target and preserve the symlink; Unix same-filesystem atomic rename via `self-replace` 1.5.0 (Rust 1.63, small `tempfile`/`windows-sys` footprint, preserves `0600` -> `0755` etc.), Windows running-image via `self-replace`.
- Cargo fallback owns its child process and kills/reaps it when the bounded build deadline expires. It does not leave a compiler running in the background or copy a verified candidate to a predictable shared-temp pathname.
- Permission probe before any `greggd` shutdown; on `PermissionDenied` prints `sudo <exe> update` and returns 4 without stopping daemon; no internal `sudo`.
- `greggd` fully prepares and verifies the candidate before stopping a running Windows SCM service; stop failure is a hard pre-replacement error. It preserves config/registration and restarts only when running/managed: `SystemdActive`, `LaunchdLoaded`, `WindowsServiceRunning`, or `UnmanagedOrCron` that is actually running (bounded `GET /v2/healthz` probe); `SystemdInstalledStopped`, `LaunchdInstalledUnloaded`, `WindowsServiceStopped`, and not-running remain stopped; successful replacement + failed restart is `UpdatedButRestartFailed` with installed version and exact `systemctl restart greggd` / `launchctl kickstart -k` / `greggd restart` command and nonzero exit. Direct/cron restart proves endpoint absence before spawning and waits for valid Gregg health readiness before success.
- Installer and updater share the `eggstack/gregg` prefix, target suffixes, tag `vX.Y.Z`, and `.sha256` contract; `gregg-protocol` remains free of updater concerns; no background checks, TUI notifications, or package-manager integration.

### Legacy local-build helpers (developer path)

| Script | Platform | Requirements |
|--------|----------|-------------|
| `install-linux.sh` | Linux | root, systemd (local `target/release/greggd` path) |
| `install-macos.sh` | macOS | root, launchd |
| `install-windows.ps1` | Windows | Administrator, SCM |

These helpers remain for operator-managed local builds and do not duplicate the bootstrap download/verify logic. The embedded systemd unit (`startup::systemd_unit_content` via `include_str!`) and launchd plist are the canonical templates for both helpers and the binary; the packaging assets are kept synchronized and are not a second independent copy.

All install scripts (bootstrap and legacy):
- Are idempotent (preserve existing config)
- Create platform-specific default config if absent

**Linux** (`install-linux.sh`):
- Creates `greggd` system user
- Installs binary to `/usr/local/bin`
- Creates `/etc/gregg/greggd.toml` (default if absent)
- Installs systemd unit file, reloads systemd

**macOS** (`install-macos.sh`):
- Installs binary to `/usr/local/bin`
- Creates `/Library/Application Support/gregg/greggd.toml`
- Installs launchd plist
- Creates log file at `/var/log/greggd.log`
- Does NOT auto-bootstrap (user must run `launchctl bootstrap` manually)

**Windows** (`install-windows.ps1`):
- Copies binary to `%ProgramFiles%\Gregg\greggd.exe`
- Creates `%ProgramData%\gregg\greggd.toml`
- Uses the resolved `-ConfigPath` (explicit or default) in the SCM image path
- Registers service via `sc.exe create`
- `LocalService` account, `auto` start
- Failure recovery: 3 restarts with 60s delays

### Uninstall scripts

| Script | Platform |
|--------|----------|
| `uninstall-windows.ps1` | Windows |

Stops and removes the service. Config preserved by default; `-RemoveConfig`
flag removes config directory.

### Service definitions

**systemd** (`packaging/systemd/greggd.service`):
- Runs as `greggd` user/group
- Security hardening: `NoNewPrivileges`, `ProtectSystem=strict`,
  `ProtectHome`, `ReadOnlyPaths=/proc /sys`, `PrivateTmp`,
  `ProtectKernelTunables/Modules/ControlGroups`, `RestrictNamespaces`,
  `MemoryDenyWriteExecute`, `SystemCallFilter=@system-service`,
  `SystemCallArchitectures=native`, empty `CapabilityBoundingSet`
- Restart on failure with 5s delay, burst limit 5 in 60s

**launchd** (`packaging/launchd/com.eggstack.greggd.plist`):
- Runs `greggd run --config <path>`
- `RunAtLoad=true`
- `KeepAlive` on crash and non-clean-exit
- `ThrottleInterval=10`
- 1024 file descriptor limit

### Packaging docs

`packaging/README.md` covers:
- Bootstrap install (`install.sh`/`install.ps1`) asset contract, glibc floor, unsigned macOS note, and Cargo fallback
- Native service assets vs legacy local-build helpers
- Install/uninstall for all platforms
- Config file locations
- Service management commands
- Upgrade behavior (idempotent)
- Privilege model and security notes
- Development mode

### Release configuration

- `Cargo.toml` release profile already uses fat LTO, one codegen unit, stripped symbols, and aborting panics; no UPX/packer.
- Linux GNU assets keep the ordinary `x86_64-unknown-linux-gnu`/`aarch64-unknown-linux-gnu` suffix without a `.2.17` qualifier; the `.2.17` is the `cargo zigbuild --target <target>.2.17` input qualifier for the glibc floor, not the public name.

## CI

GitHub Actions CI (`.github/workflows/ci.yml`) runs on push to `main` and
pull requests:

- **Linux**: fmt, clippy, and full workspace tests
- **macOS**: native workspace check + native macOS collector smoke (arm64 + Intel matrix)
- **Windows** (`windows-2022`): all-target, all-feature workspace tests, a
  release `greggd` build, and the bounded SCM lifecycle smoke
- **MSRV**: compilation check with Rust 1.75

Release-only workflow (`.github/workflows/release-binaries.yml`) runs only on
`v*` tags and manual dispatch:

- mandatory preflight: workspace/tag version equality, tag points at HEAD, clean
  checkout, crates.io visibility for `gregg`/`greggd`;
- five jobs (Linux x86_64/aarch64 with glibc 2.17 via `cargo-zigbuild` + Zig,
  macOS Intel/ARM64 native, Windows x86_64 native) each build both binaries,
  run `version`/`--help`, a foreground `greggd` loopback smoke
  (`/v2/healthz` + `/v2/status` schema 2), hash after verification, and upload
  artifacts;
- assemble job validates the ten executables + ten `.sha256` stable names, checks
  `install.sh` syntax, and creates/updates a **draft** GitHub Release via `gh`
  (`--clobber` on rerun, hard failure if already published), never calling
  `cargo publish`, `git tag`, or auto-publishing.

Local default verification is the short routine loop. Documentation and full
Clippy are release-preflight work, not ordinary CI work. The Windows SCM smoke
is the authoritative operational proof for the native dispatcher and service
lifecycle. Ordinary CI remains read-only, nonpublishing, and artifact-free; the
tagged release workflow is the one narrow exception that may create a draft
release from prebuilt binaries.

## Build configuration

**`Cargo.toml`** (workspace root):
- Three members: `gregg-protocol`, `greggd`, `gregg`
- One shared version from `[workspace.package]` (currently `1.0.11`),
  edition 2021, MSRV 1.75
- Release profile: fat LTO, 1 codegen unit, stripped symbols, aborting panics

**`deny.toml`** (cargo-deny):
- Advisory checking, license auditing, dependency bans
- Allowed licenses: MIT, Apache-2.0, Unicode-3.0, BSD-2/3, ISC, Zlib, CDLA
- Sources: only crates.io

**`rust-toolchain.toml`**:
- Pinned to `stable` channel with `rustfmt` and `clippy` components
