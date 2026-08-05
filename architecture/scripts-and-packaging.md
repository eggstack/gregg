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

Full Windows service lifecycle smoke test (manual, not CI):
- Install → start → health check → stop → restart → bind failure →
  reinstall → uninstall

Requires Administrator.

### run-mixed-fleet-sustained.py

Diagnostic tool for sustained mixed-fleet workloads:
- Builds the `gregg` test binary
- Launches as child process, samples `/proc/<pid>/status`
- Validates written summary JSON against strict contract
- Optional, not part of CI

## Packaging

**Source:** `packaging/`

### Install scripts

| Script | Platform | Requirements |
|--------|----------|-------------|
| `install-linux.sh` | Linux | root, systemd |
| `install-macos.sh` | macOS | root, launchd |
| `install-windows.ps1` | Windows | Administrator, SCM |

All install scripts:
- Validate binary architecture matches host
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
- Install/uninstall for all platforms
- Config file locations
- Service management commands
- Upgrade behavior (idempotent)
- Privilege model and security notes
- Development mode

## CI

GitHub Actions CI (`.github/workflows/ci.yml`) runs on push to `main` and
pull requests:

- **Linux**: fmt, clippy, and full workspace tests
- **macOS**: native workspace check + native macOS collector smoke (arm64 + Intel matrix)
- **Windows**: one all-target, all-feature workspace test
- **MSRV**: compilation check with Rust 1.75

Local default verification is the short routine loop. Documentation and full
Clippy are release-preflight work, not ordinary CI work. CI remains read-only,
nonpublishing, and artifact-free.

## Build configuration

**`Cargo.toml`** (workspace root):
- Three members: `gregg-protocol`, `greggd`, `gregg`
- Version: `1.0.2`, edition 2021, MSRV 1.75
- Release profile: thin LTO, 1 codegen unit, strip symbols

**`deny.toml`** (cargo-deny):
- Advisory checking, license auditing, dependency bans
- Allowed licenses: MIT, Apache-2.0, Unicode-3.0, BSD-2/3, ISC, Zlib, CDLA
- Sources: only crates.io

**`rust-toolchain.toml`**:
- Pinned to `stable` channel with `rustfmt` and `clippy` components
