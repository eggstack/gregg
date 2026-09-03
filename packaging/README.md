# Gregg Daemon Packaging

This directory contains bootstrap installers for prebuilt release binaries plus
native service assets and developer helpers.

## Structure

```text
packaging/
├── install.sh                 # bootstrap installer (Linux/macOS) — binary-first, Cargo fallback
├── install.ps1                # bootstrap installer (Windows) — binary-first, Cargo fallback
├── systemd/
│   └── greggd.service         # systemd unit file (Linux)
├── launchd/
│   └── com.eggstack.greggd.plist  # launchd plist (macOS)
├── install-linux.sh           # legacy local-build helper (Linux) — uses a prebuilt binary path
├── install-macos.sh           # legacy local-build helper (macOS)
├── install-windows.ps1        # Windows service installer (local binary or bootstrap wrapper)
├── uninstall-windows.ps1      # Windows uninstaller
└── README.md                  # This file
```

## Bootstrap installers (recommended)

The `install.sh` (Unix) and `install.ps1` (Windows) scripts prefer prebuilt
GitHub Release assets and fall back to Cargo only when no matching asset
exists. Linux GNU assets are built with glibc 2.17 for portability across
long-lived Debian/Ubuntu/Armbian SBC images. macOS binaries are unsigned
(Gatekeeper may quarantine; allow via System Settings or
`xattr -d com.apple.quarantine`).

**Asset naming contract (stable within a release, no version in filename):**

```text
gregg-x86_64-unknown-linux-gnu         linux x86_64
greggd-x86_64-unknown-linux-gnu
gregg-aarch64-unknown-linux-gnu        linux aarch64 (64-bit Raspberry Pi, Le Potato, etc.)
greggd-aarch64-unknown-linux-gnu
gregg-x86_64-apple-darwin              macOS Intel
greggd-x86_64-apple-darwin
gregg-aarch64-apple-darwin             macOS Apple Silicon
greggd-aarch64-apple-darwin
gregg-x86_64-pc-windows-msvc.exe       windows x86_64
greggd-x86_64-pc-windows-msvc.exe
<asset>.sha256                         SHA-256 for each executable
install.sh / install.ps1               bootstrap scripts themselves
```

No binary is modified after its SHA-256 is generated; the existing release
profile already uses LTO, stripped symbols, and aborting panics. ARMv7 is
source-build only in this phase; the Unix installer treats `armv7l` as Cargo
fallback, and Windows ARM64 is likewise source-only.

### Unix (Linux/macOS)

Use the bootstrap installer (binary-first, Cargo fallback for source-only hosts):

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | bash -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- greggd
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- greggd
```

Cargo fallback (source-only hosts such as ARMv7, or when no matching asset exists):

```bash
cargo install gregg --locked
cargo install greggd --locked
```

The script:

- maps `uname -s`/`uname -m` to the Rust target above (e.g., `Linux+aarch64` →
  `aarch64-unknown-linux-gnu`, `Darwin+arm64` → `aarch64-apple-darwin`);
- constructs `https://github.com/eggstack/gregg/releases/latest/download/<asset>`
  or `.../download/vX.Y.Z/<asset>` for a pinned version;
- uses `curl -fsSL` to a fresh `mktemp -d`, fetches `<asset>.sha256` via the
  same fixed `eggstack/gregg` prefix, verifies with `sha256sum` (Linux) or
  `shasum -a 256` (macOS) before any execution, requires
  `<candidate> version` to print the expected program name (and exact version
  when pinned), `chmod +x`, then installs;
- installs to `/usr/local/bin` when root, otherwise `$HOME/.local/bin`, warns
  when the destination is not on `PATH` (never edits shell rc files), traps
  temporary cleanup on success/failure, and never silently invokes `sudo`;
- on an unsupported/unknown host (or `armv7l`) skips the 404-prone download
  and tries `cargo install --locked` (with `--version "=X.Y.Z"` when pinned and
  `--root` derived from the destination) if Cargo exists;
- treats a checksum or version mismatch as a hard error (no Cargo fallback);
- after a verified `greggd` install, delegates startup to `greggd startup install` (auto) so systemd/launchd/cron logic lives in the binary, not duplicated in shell: when privileged it runs `sudo greggd startup install` (systemd: `daemon-reload` + `enable` + `start`/`restart`; launchd: `bootstrap` + `kickstart -k`; cron: idempotent `# greggd managed watchdog` block). When unprivileged on a systemd/launchd host it prints the exact elevated `sudo <exe> startup install --method <...>` and does **not** silently fall back to cron; on a cron host it installs the user-local crontab without elevation. The client `gregg` has no startup behavior.

No-argument behaviour: attached to an interactive terminal, a tiny selector is
shown; piped/noninteractive without a component prints concise usage and exits
nonzero.

### Windows (PowerShell)

```powershell
irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
.\packaging\install.ps1 -Component Gregg
.\packaging\install.ps1 -Component Greggd
.\packaging\install.ps1 -Component Both -Version 1.0.11
```

Equivalent mapping (`AMD64` → `x86_64-pc-windows-msvc`), `Invoke-WebRequest`
download to a private temp dir, `Get-FileHash -Algorithm SHA256` verification,
candidate `version` check, install to `%ProgramFiles%\Gregg` when Administrator
(preserving `%ProgramData%\gregg\greggd.toml`, registering the SCM service as
`NT AUTHORITY\LocalService` with `auto` start and failure-restart) or
`%LOCALAPPDATA%\Gregg` otherwise, with Cargo fallback for `ARM64`/unknown
hosts. `install-windows.ps1` remains a compatible local-build wrapper; `install.ps1` is now the single canonical bootstrap PowerShell installer and `startup install` on Windows is state-reporting only (SCM registration stays in the installer).

## Startup integration (Plan 100)

Unix startup registration is owned by `greggd startup install` (and `startup instructions` for manual operators). The binary embeds the canonical systemd unit and launchd plist via `include_str!` so `cargo install` works without a checkout; `packaging/systemd/greggd.service` and `packaging/launchd/com.eggstack.greggd.plist` remain the human-readable source and are kept synchronized by build. `cron` uses `croncheck` as the sole health/start primitive (`@reboot` + `* * * * *`), shell-quoted, idempotent, preserving unrelated crontab entries, never editing `/var/spool/cron` directly. `restart` is manager-aware (`systemctl restart greggd` / `launchctl kickstart -k` / SCM / direct stop+detached run) and factored for `update` reuse. No PID files, no process-name scanning, no public shutdown route, no competing supervisor fallback.

## Legacy local-build helpers (developer / packaging path)

These scripts remain for operator-managed local builds and do not duplicate the
bootstrap download/verify logic. The systemd/launchd assets are the canonical templates for both the helpers and the embedded binary; they are not a second independent copy.



## Quick Install (developer / local build)

### Linux (systemd)

```bash
# Build the release binary
cargo build --release -p greggd

# Install (requires root) — legacy helper
sudo ./packaging/install-linux.sh target/release/greggd

# Or: install binary then use the CLI (preferred, no checkout needed after cargo install)
sudo install -m 755 target/release/greggd /usr/local/bin/greggd
sudo greggd startup install                          # auto: systemd on this host
sudo greggd startup install --method systemd         # explicit
greggd startup instructions --method systemd         # read-only preview

# Check status
sudo systemctl status greggd
```

### macOS (launchd)

```bash
# Build the release binary
cargo build --release -p greggd

# Install (requires root) — legacy helper
sudo ./packaging/install-macos.sh target/release/greggd

# Or: CLI-owned install (preferred after binary is at /usr/local/bin/greggd)
sudo install -m 755 target/release/greggd /usr/local/bin/greggd
sudo greggd startup install --method launchd
greggd startup instructions --method launchd
```

### Windows (PowerShell)

```powershell
# Build the release binary
cargo build --release -p greggd

# Install (requires Administrator)
.\packaging\install-windows.ps1 -SourcePath .\target\release\greggd.exe

# Check status
Get-Service greggd
greggd stop
greggd start
greggd restart
```

For normal fleet deployment, prefer the bootstrap installers above; the
`packaging/install-linux.sh` / `install-macos.sh` / `install-windows.ps1`
paths are for local builds and operator-managed packaging where a checkout is
present.

## Configuration

The default configuration file locations are:

- **Linux:** `/etc/gregg/greggd.toml`
- **macOS:** `/Library/Application Support/gregg/greggd.toml`
- **Windows:** `%ProgramData%\gregg\greggd.toml`

Example configuration:

```toml
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
```

Use `greggd host 127.0.0.1` to restrict to localhost only (recommended for SSH-tunnel-only access).

## Optional Service Management

`greggd run` is the normal foreground daemon command. Unix service lifecycle is
owned by the operator and `greggd startup` commands; the foreground `greggd run`
remains unaware of its supervisor. `greggd croncheck` is a watchdog for cron and other
non-systemd supervisors: it probes the configured local TCP port and, if
nothing is listening, spawns `greggd run` as a detached child.

### Linux (systemd)

```bash
sudo systemctl start greggd
sudo systemctl stop greggd
sudo systemctl restart greggd
sudo systemctl status greggd
journalctl -u greggd -f          # follow logs
```

### macOS (launchd)

```bash
# Start
sudo launchctl kickstart -k system/com.eggstack.greggd

# Stop
sudo launchctl bootout system/com.eggstack.greggd

# Restart
sudo launchctl kickstart -k system/com.eggstack.greggd

# Logs
log show --predicate 'process == "greggd"' --last 5m
```

### Windows

```powershell
greggd start
greggd stop
greggd restart
Get-Service greggd
```

The Windows service runs under `NT AUTHORITY\LocalService` with minimal privileges. It does not automatically create firewall rules. LAN exposure is operator-controlled and the daemon has no TLS or authentication.

## Upgrade

All install scripts are idempotent. Rerunning them will:

1. Stop the existing service.
2. Replace the binary.
3. Preserve the existing configuration file.
4. Reload/restart the service.

On Windows, the install script preserves the existing config at `%ProgramData%\gregg\greggd.toml` unless you explicitly provide a different config path.

## Uninstall

### Linux

```bash
sudo systemctl stop greggd
sudo systemctl disable greggd
sudo rm /etc/systemd/system/greggd.service
sudo systemctl daemon-reload
sudo rm /usr/local/bin/greggd
sudo rm -rf /etc/gregg
```

### macOS

```bash
sudo launchctl bootout system/com.eggstack.greggd
sudo rm /Library/LaunchDaemons/com.eggstack.greggd.plist
sudo rm /usr/local/bin/greggd
sudo rm -rf "/Library/Application Support/gregg"
```

### Windows

```powershell
# Stop and remove service (preserves config by default)
.\packaging\uninstall-windows.ps1

# Stop and remove service AND config
.\packaging\uninstall-windows.ps1 -RemoveConfig
```

## Security Notes

- The default configuration binds to `0.0.0.0`, making metrics visible to all reachable peers. Use `greggd host 127.0.0.1` for SSH-tunnel-only access.
- The systemd unit includes security hardening options. Some options may need adjustment on older distributions or ARM boards.
- The launchd plist runs as a system daemon. Consider creating a dedicated `_greggd` user for production deployments.
- On Windows, the service runs under `NT AUTHORITY\LocalService` with minimal privileges. No firewall rule is created; LAN exposure is operator-controlled. The daemon has no TLS or authentication.

## Privilege Model

System installation and mutation of system config/service state generally require administrator privileges. The binary must not silently invoke `sudo` or prompt unexpectedly inside library code.

- **Installation scripts** require root (`sudo`). They detect missing privileges and print the exact command requiring elevation.
- **`greggd run --config <writable temp path>`** can run unprivileged for development and testing.
- **Windows service lifecycle commands** (`start`, `stop`, `restart`) use native SCM and require appropriate privileges.
- **Config mutation commands** (`host`, `port`) atomically persist the config. On Unix, the new value applies on the next daemon start; they do not invoke a service manager.

The systemd unit runs as the dedicated `greggd` user with
`NoNewPrivileges=true` and comprehensive filesystem/capability restrictions.
The installer creates that user, assigns configuration ownership to it, and
keeps the daemon in the foreground under systemd. The service lifecycle still
requires administrator privileges, while `greggd run --config <path>` remains
usable unprivileged for development and tests.

## Development Mode

For development and testing, run the daemon unprivileged with a temporary config:

```bash
greggd run --config /tmp/test-config.toml
```

This avoids needing root privileges and does not interact with the system service manager.
