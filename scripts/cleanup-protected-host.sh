#!/usr/bin/env bash
# Guaranteed cleanup for protected-host lifecycle jobs.
# Must be called with if: always() in the workflow.
set -euo pipefail

SERVICE_NAME="${GREGG_SERVICE_NAME:-greggd}"
EXPECTED_PORT="${GREGG_EXPECTED_PORT:-11310}"
MARKER_DIR="${GREGG_MARKER_DIR:-/tmp/gregg-release-run}"
CLEANUP_LOG="${GREGG_CLEANUP_LOG:-/tmp/gregg-cleanup.log}"

echo "=== CLEANUP START ===" | tee "${CLEANUP_LOG}"

cleanup_step() {
    echo "cleanup: $*" | tee -a "${CLEANUP_LOG}"
}

cleanup_failed=0

# 1. Stop/boots the service.
cleanup_step "stopping service"
if [[ "$(uname -s)" == "Linux" ]]; then
    sudo systemctl stop "${SERVICE_NAME}" 2>>"${CLEANUP_LOG}" || cleanup_failed=1
elif [[ "$(uname -s)" == "Darwin" ]]; then
    sudo launchctl bootout "system/com.eggstack.${SERVICE_NAME}" 2>>"${CLEANUP_LOG}" || cleanup_failed=1
    sudo /usr/local/bin/"${SERVICE_NAME}" stop 2>>"${CLEANUP_LOG}" || true
fi

# 2. Disable or unload the service.
cleanup_step "disabling service"
if [[ "$(uname -s)" == "Linux" ]]; then
    sudo systemctl disable "${SERVICE_NAME}" 2>>"${CLEANUP_LOG}" || cleanup_failed=1
elif [[ "$(uname -s)" == "Darwin" ]]; then
    sudo launchctl unload "system/com.eggstack.${SERVICE_NAME}" 2>>"${CLEANUP_LOG}" || cleanup_failed=1
fi

# 3. Terminate remaining candidate processes.
cleanup_step "terminating remaining processes"
pkill -x "${SERVICE_NAME}" 2>>"${CLEANUP_LOG}" || true

# 4. Remove installed candidate binary.
cleanup_step "removing installed binary"
sudo rm -f "/usr/local/bin/${SERVICE_NAME}" 2>>"${CLEANUP_LOG}" || cleanup_failed=1

# 5. Restore or remove unit/plist files.
cleanup_step "removing service definition"
if [[ "$(uname -s)" == "Linux" ]]; then
    sudo rm -f "/etc/systemd/system/${SERVICE_NAME}.service" 2>>"${CLEANUP_LOG}" || cleanup_failed=1
    sudo systemctl daemon-reload 2>>"${CLEANUP_LOG}" || true
elif [[ "$(uname -s)" == "Darwin" ]]; then
    sudo rm -f "/Library/LaunchDaemons/com.eggstack.${SERVICE_NAME}.plist" 2>>"${CLEANUP_LOG}" || cleanup_failed=1
fi

# 6. Remove temporary roots and run marker.
cleanup_step "removing temporary roots and marker"
rm -rf /tmp/greggd-release-* 2>>"${CLEANUP_LOG}" || true
rm -rf "${MARKER_DIR}" 2>>"${CLEANUP_LOG}" || true

# 7. Verify port release.
cleanup_step "verifying port release"
if command -v ss >/dev/null 2>&1; then
    if ss -tlnp | grep -q ":${EXPECTED_PORT} "; then
        echo "WARNING: port ${EXPECTED_PORT} still in use" | tee -a "${CLEANUP_LOG}"
        cleanup_failed=1
    else
        echo "port ${EXPECTED_PORT}: released" | tee -a "${CLEANUP_LOG}"
    fi
elif command -v lsof >/dev/null 2>&1; then
    if lsof -i ":${EXPECTED_PORT}" >/dev/null 2>&1; then
        echo "WARNING: port ${EXPECTED_PORT} still in use" | tee -a "${CLEANUP_LOG}"
        cleanup_failed=1
    else
        echo "port ${EXPECTED_PORT}: released" | tee -a "${CLEANUP_LOG}"
    fi
fi

# 8. Record final service state.
cleanup_step "recording final service state"
if [[ "$(uname -s)" == "Linux" ]]; then
    systemctl is-active "${SERVICE_NAME}" 2>/dev/null || echo "service: inactive" | tee -a "${CLEANUP_LOG}"
elif [[ "$(uname -s)" == "Darwin" ]]; then
    launchctl print "system/com.eggstack.${SERVICE_NAME}" 2>/dev/null || echo "service: not-loaded" | tee -a "${CLEANUP_LOG}"
fi

# 9. Record cleanup result.
if [[ ${cleanup_failed} -eq 0 ]]; then
    echo "=== CLEANUP PASSED ===" | tee -a "${CLEANUP_LOG}"
else
    echo "=== CLEANUP FAILED ===" | tee -a "${CLEANUP_LOG}"
    exit 1
fi
