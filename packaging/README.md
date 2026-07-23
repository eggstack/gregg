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
