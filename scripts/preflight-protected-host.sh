#!/usr/bin/env bash
# Preflight checks for protected-host lifecycle jobs.
set -euo pipefail

SERVICE_NAME="${GREGG_SERVICE_NAME:-greggd}"
EXPECTED_PORT="${GREGG_EXPECTED_PORT:-11310}"
MARKER_DIR="${GREGG_MARKER_DIR:-/tmp/gregg-release-run}"
EXPECTED_SHA="${GREGG_EXPECTED_SHA:-}"
EXPECTED_VERSION="${GREGG_EXPECTED_VERSION:-1.0.1}"

die() {
    echo "PREFLIGHT_FAIL: $*" >&2
    exit 1
}

echo "=== PREFLIGHT CHECKS ==="
echo "host: $(uname -a)"
echo "expected_sha: ${EXPECTED_SHA}"
echo "expected_version: ${EXPECTED_VERSION}"

# 1. Expected OS and architecture.
case "$(uname -s)" in
    Linux) echo "os: Linux" ;;
    Darwin) echo "os: macOS" ;;
    *) die "unexpected OS: $(uname -s)" ;;
esac
echo "arch: $(uname -m)"

# 2. Runner label/host class.
echo "runner_os: ${RUNNER_OS:-unknown}"
echo "runner_arch: ${RUNNER_ARCH:-unknown}"

# 3. Current service state.
if [[ "$(uname -s)" == "Linux" ]]; then
    if systemctl list-unit-files "${SERVICE_NAME}.service" >/dev/null 2>&1; then
        echo "service_state: $(systemctl is-active "${SERVICE_NAME}" 2>/dev/null || echo 'unknown')"
    else
        echo "service_state: not-installed"
    fi
elif [[ "$(uname -s)" == "Darwin" ]]; then
    if launchctl print "system/${SERVICE_NAME}" >/dev/null 2>&1; then
        echo "service_state: loaded"
    else
        echo "service_state: not-loaded"
    fi
fi

# 4. Existing installed binary path and checksum.
INSTALLED_BINARY=""
if [[ -x "/usr/local/bin/${SERVICE_NAME}" ]]; then
    INSTALLED_BINARY="/usr/local/bin/${SERVICE_NAME}"
    echo "existing_binary: ${INSTALLED_BINARY}"
    if command -v sha256sum >/dev/null 2>&1; then
        echo "existing_binary_sha256: $(sha256sum "${INSTALLED_BINARY}" | awk '{print $1}')"
    else
        echo "existing_binary_sha256: $(shasum -a 256 "${INSTALLED_BINARY}" | awk '{print $1}')"
    fi
fi

# 5. Existing service definition checksum.
if [[ "$(uname -s)" == "Linux" ]]; then
    UNIT_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
    if [[ -f "${UNIT_FILE}" ]]; then
        if command -v sha256sum >/dev/null 2>&1; then
            echo "existing_unit_sha256: $(sha256sum "${UNIT_FILE}" | awk '{print $1}')"
        else
            echo "existing_unit_sha256: $(shasum -a 256 "${UNIT_FILE}" | awk '{print $1}')"
        fi
    fi
elif [[ "$(uname -s)" == "Darwin" ]]; then
    PLIST_FILE="/Library/LaunchDaemons/com.eggstack.${SERVICE_NAME}.plist"
    if [[ -f "${PLIST_FILE}" ]]; then
        if command -v sha256sum >/dev/null 2>&1; then
            echo "existing_plist_sha256: $(sha256sum "${PLIST_FILE}" | awk '{print $1}')"
        else
            echo "existing_plist_sha256: $(shasum -a 256 "${PLIST_FILE}" | awk '{print $1}')"
        fi
    fi
fi

# 6. Port availability.
if command -v ss >/dev/null 2>&1; then
    if ss -tlnp | grep -q ":${EXPECTED_PORT} "; then
        echo "port_${EXPECTED_PORT}: in-use"
    else
        echo "port_${EXPECTED_PORT}: available"
    fi
elif command -v lsof >/dev/null 2>&1; then
    if lsof -i ":${EXPECTED_PORT}" >/dev/null 2>&1; then
        echo "port_${EXPECTED_PORT}: in-use"
    else
        echo "port_${EXPECTED_PORT}: available"
    fi
else
    echo "port_${EXPECTED_PORT}: cannot-check"
fi

# 7. Stale greggd processes.
STALE_PROCESSES="$(pgrep -x "${SERVICE_NAME}" 2>/dev/null | wc -l || echo 0)"
echo "stale_processes: ${STALE_PROCESSES}"

# 8. Stale release-specific temporary roots.
echo "stale_temp_roots: $(ls -d /tmp/greggd-release-* 2>/dev/null | wc -l || echo 0)"

# 9. Available disk space.
echo "disk_available: $(df -m / | tail -1 | awk '{print $4}') MB"

# 10. Required commands.
for cmd in curl jq python3; do
    if command -v "${cmd}" >/dev/null 2>&1; then
        echo "command_${cmd}: available"
    else
        die "required command ${cmd} not found"
    fi
done

# 11. No conflicting test run marker.
if [[ -d "${MARKER_DIR}" ]]; then
    die "conflicting test run marker found at ${MARKER_DIR}"
fi

# 12. Privilege capability.
if [[ "$(uname -s)" == "Linux" ]]; then
    if ! sudo -n true 2>/dev/null; then
        die "sudo capability required for systemd lifecycle"
    fi
elif [[ "$(uname -s)" == "Darwin" ]]; then
    if ! sudo -n true 2>/dev/null; then
        die "sudo capability required for launchd lifecycle"
    fi
fi

echo "=== PREFLIGHT PASSED ==="
