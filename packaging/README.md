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

## Configuration

The default configuration file locations are:

- **Linux:** `/etc/gregg/greggd.toml`
- **macOS:** `/Library/Application Support/gregg/greggd.toml`

Example configuration:

```toml
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
```

Use `greggd host 127.0.0.1` to restrict to localhost only (recommended for SSH-tunnel-only access).

## Service Management

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

## Upgrade

Both install scripts are idempotent. Rerunning them will:

1. Stop the existing service.
2. Replace the binary.
3. Preserve the existing configuration file.
4. Reload/restart the service.

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

## Security Notes

- The default configuration binds to `0.0.0.0`, making metrics visible to all reachable peers. Use `greggd host 127.0.0.1` for SSH-tunnel-only access.
- The systemd unit includes security hardening options. Some options may need adjustment on older distributions or ARM boards.
- The launchd plist runs as a system daemon. Consider creating a dedicated `_greggd` user for production deployments.

## Privilege Model

System installation and mutation of system config/service state generally require administrator privileges. The binary must not silently invoke `sudo` or prompt unexpectedly inside library code.

- **Installation scripts** require root (`sudo`). They detect missing privileges and print the exact command requiring elevation.
- **`greggd run --config <writable temp path>`** can run unprivileged for development and testing.
- **Service lifecycle commands** (`start`, `stop`, `restart`, `croncheck`) delegate to the native service manager and require appropriate privileges.
- **Config mutation commands** (`host`, `port`) atomically persist the config and restart the service, requiring write access to the config directory and service manager privileges.

The systemd unit runs as root with `NoNewPrivileges=true` and comprehensive filesystem/capability restrictions. This is intentional: the daemon needs read access to `/proc` and `/sys` for metrics collection, and bind access to the configured port. Running as a dedicated system user is possible but adds installation complexity; the current model relies on systemd's security hardening to limit the blast radius.

## Development Mode

For development and testing, run the daemon unprivileged with a temporary config:

```bash
greggd run --config /tmp/test-config.toml
```

This avoids needing root privileges and does not interact with the system service manager.
