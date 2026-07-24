#!/usr/bin/env bash
# Deterministic failure-path tests for verify-installed-daemon.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERIFY="${SCRIPT_DIR}/verify-installed-daemon.sh"
FAKE="${SCRIPT_DIR}/tests/fake-greggd.py"

expect_failure() {
    local label="$1"
    shift
    local output
    if output="$("$@" 2>&1)"; then
        echo "FAIL: ${label} unexpectedly passed" >&2
        exit 1
    fi
    echo "PASS: ${label}"
    printf '%s\n' "${output}" | tail -n 5
}

expect_failure "invalid binary path" "${VERIFY}" "${SCRIPT_DIR}/does-not-exist"
expect_failure "startup child failure" env FAKE_MODE=startup "${VERIFY}" "${FAKE}"
expect_failure "health timeout" env FAKE_MODE=timeout STARTUP_DEADLINE_SECS=0.4 POLL_INTERVAL_SECS=0.1 "${VERIFY}" "${FAKE}"
expect_failure "malformed status JSON" env FAKE_MODE=malformed "${VERIFY}" "${FAKE}"
expect_failure "nonzero shutdown" env FAKE_MODE=nonzero "${VERIFY}" "${FAKE}"

FAKE_MODE=success "${VERIFY}" "${FAKE}"
echo "PASS: successful fake-daemon path"

if [[ -n "${GREGGD_BINARY:-}" ]]; then
    "${VERIFY}" "${GREGGD_BINARY}"
    echo "PASS: successful real-daemon path"
else
    echo "SKIP: set GREGGD_BINARY to exercise the real-daemon path"
fi
