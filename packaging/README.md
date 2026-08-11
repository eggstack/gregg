# Gregg Daemon Packaging

This directory contains installation assets for deploying `greggd` as a native system service.

## Structure

```text
packaging/
├── systemd/
│   └── greggd.service          # systemd unit file (Linux)
├── launchd/
│   └── com.eggstack.greggd.plist  # launchd plist (macOS)
├── install-linux.sh            # Linux installer script
├── install-macos.sh            # macOS installer script
├── install-windows.ps1         # Windows installer script
├── uninstall-windows.ps1       # Windows uninstaller script
└── README.md                   # This file
```

## Quick Install

### Linux (systemd)

```bash
# Build the release binary
cargo build --release -p greggd

# Install (requires root)
sudo ./packaging/install-linux.sh target/release/greggd

# Enable and start
sudo systemctl enable --now greggd

# Check status
sudo systemctl status greggd
```

### macOS (launchd)

```bash
# Build the release binary
cargo build --release -p greggd

# Install (requires root)
sudo ./packaging/install-macos.sh target/release/greggd
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
owned by the operator and these packaging assets; the binary does not invoke
systemd or launchd. `greggd croncheck` is a read-only HTTP probe of `/v2/healthz`.

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
