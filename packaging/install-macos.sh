#!/usr/bin/env bash
# install-macos.sh — Install greggd on macOS with launchd.
#
# Usage: sudo ./install-macos.sh [BINARY_PATH]
#
# This script:
# 1. Validates the binary exists and is executable.
# 2. Creates the configuration directory and default config.
# 3. Installs the binary to /usr/local/bin.
# 4. Installs the launchd plist.
# 5. Bootstraps the service.
#
# Idempotent: safe to rerun. Preserves existing configuration.

set -euo pipefail

BINARY_PATH="${1:-}"
CONFIG_DIR="/Library/Application Support/gregg"
CONFIG_FILE="${CONFIG_DIR}/greggd.toml"
BIN_DIR="/usr/local/bin"
PLIST_NAME="com.eggstack.greggd"
PLIST_DIR="/Library/LaunchDaemons"
PLIST_FILE="${PLIST_DIR}/${PLIST_NAME}.plist"
PLIST_SOURCE="$(dirname "$0")/launchd/${PLIST_NAME}.plist"
LOG_FILE="/var/log/greggd.log"

# --- Functions ---

usage() {
    echo "Usage: sudo $0 [BINARY_PATH]"
    echo ""
    echo "Install greggd as a launchd system daemon on macOS."
    echo ""
    echo "Arguments:"
    echo "  BINARY_PATH  Path to the greggd binary (default: built from source)"
    exit 1
}

die() {
    echo "error: $*" >&2
    exit 1
}

# --- Pre-flight checks ---

[[ $EUID -eq 0 ]] || die "this script must be run as root (use sudo)"

# Find the binary.
if [[ -z "$BINARY_PATH" ]]; then
    BINARY_PATH="$(cd "$(dirname "$0")/../.." && pwd)/target/release/greggd"
fi

[[ -f "$BINARY_PATH" ]] || die "binary not found: $BINARY_PATH"
[[ -x "$BINARY_PATH" ]] || die "binary is not executable: $BINARY_PATH"

# --- Installation ---

echo "Installing greggd..."

# Stop existing service if running.
if launchctl list | grep -q "$PLIST_NAME" 2>/dev/null; then
    echo "  Stopping existing service..."
    launchctl bootout "system/${PLIST_NAME}" 2>/dev/null || true
    sleep 1
fi

# Create configuration directory.
mkdir -p "$CONFIG_DIR"
echo "  Created $CONFIG_DIR"

# Install default config if it doesn't exist.
if [[ ! -f "$CONFIG_FILE" ]]; then
    cat > "$CONFIG_FILE" <<'EOF'
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
EOF
    echo "  Created default config: $CONFIG_FILE"
else
    echo "  Preserved existing config: $CONFIG_FILE"
fi

# Install binary.
install -m 755 "$BINARY_PATH" "${BIN_DIR}/greggd"
echo "  Installed binary: ${BIN_DIR}/greggd"

# Install launchd plist.
if [[ -f "$PLIST_SOURCE" ]]; then
    mkdir -p "$PLIST_DIR"
    install -m 644 "$PLIST_SOURCE" "$PLIST_FILE"
    echo "  Installed plist: $PLIST_FILE"
else
    die "launchd plist not found: $PLIST_SOURCE"
fi

# Create log file.
touch "$LOG_FILE"
chmod 644 "$LOG_FILE"
echo "  Created log file: $LOG_FILE"

# Bootstrap the service.
launchctl bootstrap "system" "$PLIST_FILE"
echo "  Bootstrapped service"

echo ""
echo "Installation complete."
echo ""
echo "To manage the service:"
echo "  sudo launchctl kickstart -k system/${PLIST_NAME}"
echo "  sudo launchctl bootout system/${PLIST_NAME}"
echo ""
echo "Configuration: $CONFIG_FILE"
echo "Logs:          log show --predicate 'process == \"greggd\"' --last 5m"
