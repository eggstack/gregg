#!/usr/bin/env bash
# install-linux.sh — Install greggd on Linux with systemd (legacy local-build helper).
# For prebuilt release binaries, prefer: curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
# This helper remains for operator-managed local builds where a checkout is present.
#
# Usage: sudo ./install-linux.sh [BINARY_PATH]
#
# This script:
# 1. Validates the binary exists and is executable.
# 2. Creates the configuration directory and default config.
# 3. Installs the binary to /usr/local/bin.
# 4. Installs the systemd unit file.
# 5. Reloads systemd.
# 6. Optionally enables and starts the service.
#
# Idempotent: safe to rerun. Preserves existing configuration.

set -euo pipefail

BINARY_PATH="${1:-}"
CONFIG_DIR="/etc/gregg"
CONFIG_FILE="${CONFIG_DIR}/greggd.toml"
BIN_DIR="/usr/local/bin"
SERVICE_FILE="/etc/systemd/system/greggd.service"
UNIT_SOURCE="$(dirname "$0")/systemd/greggd.service"

# --- Functions ---

usage() {
    echo "Usage: sudo $0 [BINARY_PATH]"
    echo ""
    echo "Install greggd as a systemd service on Linux."
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

# Create dedicated service user if it doesn't exist.
if ! id -u greggd >/dev/null 2>&1; then
    useradd --system --no-create-home --shell /usr/sbin/nologin greggd
    echo "  Created system user: greggd"
fi

# Find the binary.
if [[ -z "$BINARY_PATH" ]]; then
    BINARY_PATH="$(cd "$(dirname "$0")/.." && pwd)/target/release/greggd"
fi

[[ -f "$BINARY_PATH" ]] || die "binary not found: $BINARY_PATH"
[[ -x "$BINARY_PATH" ]] || die "binary is not executable: $BINARY_PATH"

# Validate architecture matches host.
HOST_ARCH="$(uname -m)"
FILE_ARCH="$(file "$BINARY_PATH" | grep -oE 'x86_64|aarch64|ARM|80386' | head -1)"
case "$HOST_ARCH" in
    x86_64)  EXPECTED_ARCH="x86_64" ;;
    aarch64) EXPECTED_ARCH="aarch64" ;;
    armv7l)  EXPECTED_ARCH="ARM" ;;
    *)       EXPECTED_ARCH="" ;;
esac
if [[ -n "$EXPECTED_ARCH" && -n "$FILE_ARCH" && "$FILE_ARCH" != "$EXPECTED_ARCH" ]]; then
    die "binary architecture ($FILE_ARCH) does not match host ($HOST_ARCH)"
fi

# --- Installation ---

echo "Installing greggd..."

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

# Set config ownership for the service user.
chown -R greggd:greggd "$CONFIG_DIR"
echo "  Set config ownership: greggd:greggd"

# Install systemd unit.
if [[ -f "$UNIT_SOURCE" ]]; then
    install -m 644 "$UNIT_SOURCE" "$SERVICE_FILE"
    echo "  Installed systemd unit: $SERVICE_FILE"
else
    die "systemd unit not found: $UNIT_SOURCE"
fi

# Reload systemd.
systemctl daemon-reload
echo "  Reloaded systemd"

echo ""
echo "Installation complete."
echo ""
echo "To enable and start the service:"
echo "  sudo systemctl enable --now greggd"
echo ""
echo "To check status:"
echo "  sudo systemctl status greggd"
echo ""
echo "Configuration: $CONFIG_FILE"
echo "Logs:          journalctl -u greggd -f"
