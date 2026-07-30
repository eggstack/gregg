#!/usr/bin/env bash
# verify-installed-daemon.sh — bounded loopback smoke test for greggd.
#
# The caller owns the package-install boundary. This script verifies the
# supplied executable and never falls back to a workspace build.
#
# Usage:
#   ./scripts/verify-installed-daemon.sh <greggd-binary-path> [port]
#
# Environment overrides are intended for deterministic tests:
#   STARTUP_DEADLINE_SECS, SHUTDOWN_DEADLINE_SECS, POLL_INTERVAL_SECS

set -euo pipefail

BINARY="${1:-}"
REQUESTED_PORT="${2:-0}"
STARTUP_DEADLINE_SECS="${STARTUP_DEADLINE_SECS:-10}"
SHUTDOWN_DEADLINE_SECS="${SHUTDOWN_DEADLINE_SECS:-10}"
POLL_INTERVAL_SECS="${POLL_INTERVAL_SECS:-0.2}"
MAX_PORT_ATTEMPTS="${MAX_PORT_ATTEMPTS:-5}"
ALLOW_PORT_RETRY=0

TEMP_DIR=""
CONFIG_FILE=""
LOG_FILE=""
GREGGD_PID=""
KILLER_PID=""

die() {
    echo "FATAL: $*" >&2
    if [[ -n "${CONFIG_FILE}" && -f "${CONFIG_FILE}" ]]; then
        echo "=== effective greggd config ===" >&2
        cat "${CONFIG_FILE}" >&2
    fi
    if [[ -n "${LOG_FILE}" && -f "${LOG_FILE}" ]]; then
        echo "=== greggd log ===" >&2
        cat "${LOG_FILE}" >&2
    fi
    exit 1
}

cleanup() {
    # Cleanup must never replace the verifier's original exit status.
    set +e
    if [[ -n "${KILLER_PID}" ]]; then
        kill "${KILLER_PID}" 2>/dev/null
        wait "${KILLER_PID}" 2>/dev/null
    fi
    if [[ -n "${GREGGD_PID}" ]]; then
        kill -KILL "${GREGGD_PID}" 2>/dev/null
        wait "${GREGGD_PID}" 2>/dev/null
    fi
    if [[ -n "${TEMP_DIR}" && -d "${TEMP_DIR}" ]]; then
        rm -rf "${TEMP_DIR}"
    fi
    set -e
}
trap cleanup EXIT

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

is_positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]]
}

is_nonnegative_number() {
    [[ "$1" =~ ^[0-9]+([.][0-9]+)?$ ]]
}

allocate_port() {
    python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

write_config() {
    local port="$1"
    cat >"${CONFIG_FILE}" <<TOML
name = "loopback-test"
host = "127.0.0.1"
port = ${port}
sample_interval_ms = 1000
stale_after_ms = 10000
TOML
}

fetch() {
    local url="$1"
    local body_path="$2"
    local result
    local curl_status

    set +e
    result="$(curl --silent --show-error --connect-timeout 1 --max-time 2 \
        --output "${body_path}" --write-out '%{http_code}' "${url}" 2>"${TEMP_DIR}/curl.err")"
    curl_status=$?
    set -e

    if ((curl_status != 0)); then
        FETCH_REASON="connection failure (curl exit ${curl_status}: $(tr '\n' ' ' <"${TEMP_DIR}/curl.err"))"
        FETCH_STATUS="000"
        return 1
    fi

    FETCH_STATUS="${result}"
    FETCH_REASON="HTTP ${result}"
    return 0
}

validate_health() {
    local body_path="$1"
    jq -e '
        (.schema_version == 1 or .schema_version == 2) and
        (.state == "ready" or .state == "warming" or .state == "failed")
    ' "${body_path}" >/dev/null 2>&1 || die "/healthz returned malformed JSON"
}

validate_status_v2() {
    local body_path="$1"
    jq -e '
        .schema_version == 2 and
        (.observed_at_unix_ms | type == "number" and . > 0) and
        (.sample_interval_ms | type == "number" and . > 0) and
        (.capabilities.cpu_iowait | type == "boolean") and
        (.capabilities.load_average | type == "boolean") and
        (.capabilities.swap | type == "boolean") and
        (.capabilities.memory_commit | type == "boolean") and
        (.system.name | type == "string" and length > 0) and
        (.system.hostname | type == "string" and length > 0) and
        (.system.architecture | type == "string" and length > 0) and
        (.cpu.logical_cores | type == "number" and . > 0) and
        (.cpu.usage_pct | type == "number" and isfinite and . >= 0 and . <= 100) and
        ((.capabilities.cpu_iowait and
          (.cpu.iowait_pct | type == "number" and isfinite and . >= 0 and . <= 100)) or
         ((.capabilities.cpu_iowait | not) and (.cpu.iowait_pct == null))) and
        ((.capabilities.load_average and
          (.load.one | type == "number" and isfinite and . >= 0) and
          (.load.five | type == "number" and isfinite and . >= 0) and
          (.load.fifteen | type == "number" and isfinite and . >= 0)) or
         ((.capabilities.load_average | not) and (.load == null))) and
        (.memory.total_bytes | type == "number" and . >= 0) and
        (.memory.used_bytes | type == "number" and . >= 0) and
        (.memory.used_bytes <= .memory.total_bytes) and
        (.memory.usage_pct | type == "number" and isfinite and . >= 0 and . <= 100) and
        ((.capabilities.swap and
          (.swap.total_bytes | type == "number" and . >= 0) and
          (.swap.used_bytes | type == "number" and . >= 0) and
          (.swap.used_bytes <= .swap.total_bytes) and
          (.swap.usage_pct | type == "number" and isfinite and . >= 0 and . <= 100) and
          ((.swap.total_bytes != 0) or (.swap.usage_pct == 0))) or
         ((.capabilities.swap | not) and (.swap == null))) and
        ((.capabilities.memory_commit and
          (.commit.limit_bytes | type == "number" and . >= 0) and
          (.commit.used_bytes | type == "number" and . >= 0) and
          (.commit.used_bytes <= .commit.limit_bytes) and
          (.commit.usage_pct | type == "number" and isfinite and . >= 0 and . <= 100)) or
         ((.capabilities.memory_commit | not) and (.commit == null)))
    ' "${body_path}" >/dev/null 2>&1 || die "/v2/status failed protocol field validation"
}

retry_after_bind_collision() {
    if ((ALLOW_PORT_RETRY == 1)) && grep -Eiq 'address already in use|already allocated|cannot assign requested address' "${LOG_FILE}"; then
        echo "Port collision detected; retrying with a new isolated port" >&2
        set +e
        wait "${GREGGD_PID}" 2>/dev/null
        set -e
        GREGGD_PID=""
        return 0
    fi
    return 1
}

start_and_verify() {
    local port="$1"
    local attempt="$2"
    local health_body="${TEMP_DIR}/health-${attempt}.json"
    local status_body="${TEMP_DIR}/status-${attempt}.json"
    local elapsed=0
    local health_state=""
    local shutdown_status

    LOG_FILE="${TEMP_DIR}/greggd-${attempt}.log"
    write_config "${port}"
    : >"${LOG_FILE}"

    "${BINARY}" run --config "${CONFIG_FILE}" >"${LOG_FILE}" 2>&1 &
    GREGGD_PID=$!
    echo "greggd started (PID=${GREGGD_PID}) on port ${port}"

    while awk "BEGIN { exit !(${elapsed} < ${STARTUP_DEADLINE_SECS}) }"; do
        if ! kill -0 "${GREGGD_PID}" 2>/dev/null; then
            if retry_after_bind_collision; then
                return 75
            fi
            die "greggd exited during startup"
        fi

        if fetch "http://127.0.0.1:${port}/v2/healthz" "${health_body}"; then
            if [[ "${FETCH_STATUS}" == "200" ]]; then
                validate_health "${health_body}"
                health_state="$(jq -r '.state' "${health_body}")"
                if [[ "${health_state}" == "ready" ]]; then
                    echo "/v2/healthz returned 200/ready after ${elapsed}s"
                    break
                fi
                die "/v2/healthz returned 200 with state ${health_state}, expected ready"
            elif [[ "${FETCH_STATUS}" == "503" ]]; then
                validate_health "${health_body}"
                health_state="$(jq -r '.state' "${health_body}")"
                echo "/v2/healthz is still ${health_state} after ${elapsed}s"
            else
                die "/v2/healthz returned unexpected ${FETCH_STATUS}"
            fi
        else
            echo "/v2/healthz ${FETCH_REASON}" >&2
        fi

        sleep "${POLL_INTERVAL_SECS}"
        elapsed="$(awk -v current="${elapsed}" -v interval="${POLL_INTERVAL_SECS}" 'BEGIN { printf "%.3f", current + interval }')"
    done

    if ! [[ "${health_state}" == "ready" ]]; then
        die "/v2/healthz did not become ready within ${STARTUP_DEADLINE_SECS}s"
    fi

    if ! fetch "http://127.0.0.1:${port}/v2/status" "${status_body}"; then
        die "failed to fetch /v2/status: ${FETCH_REASON}"
    fi
    [[ "${FETCH_STATUS}" == "200" ]] || die "/v2/status returned HTTP ${FETCH_STATUS}"
    validate_status_v2 "${status_body}"

    echo "/v2/status JSON validation passed"
    echo "  schema_version: $(jq -r '.schema_version' "${status_body}")"
    echo "  system.name: $(jq -r '.system.name' "${status_body}")"
    echo "  system.hostname: $(jq -r '.system.hostname' "${status_body}")"
    echo "  system.architecture: $(jq -r '.system.architecture' "${status_body}")"
    echo "  cpu.logical_cores: $(jq -r '.cpu.logical_cores' "${status_body}")"
    echo "  cpu.usage_pct: $(jq -r '.cpu.usage_pct' "${status_body}")"

    echo "Sending SIGTERM to greggd (PID=${GREGGD_PID})"
    kill -TERM "${GREGGD_PID}" || die "failed to send SIGTERM to greggd"

    # A separate timer provides a bounded wait while the shell wait builtin
    # reaps the actual child and exposes its true exit status.
    local timeout_marker="${TEMP_DIR}/shutdown-timeout"
    (
        sleep "${SHUTDOWN_DEADLINE_SECS}"
        if kill -0 "${GREGGD_PID}" 2>/dev/null; then
            : >"${timeout_marker}"
            kill -KILL "${GREGGD_PID}" 2>/dev/null
        fi
    ) &
    KILLER_PID=$!

    set +e
    wait "${GREGGD_PID}"
    shutdown_status=$?
    set -e

    set +e
    kill "${KILLER_PID}" 2>/dev/null
    wait "${KILLER_PID}" 2>/dev/null
    set -e
    KILLER_PID=""
    GREGGD_PID=""

    if [[ -e "${timeout_marker}" ]]; then
        die "greggd did not terminate within ${SHUTDOWN_DEADLINE_SECS}s"
    fi
    if ((shutdown_status != 0)); then
        die "greggd exited with status ${shutdown_status} after SIGTERM"
    fi

    echo "greggd exited cleanly with status 0"
}

require_command curl
require_command jq
require_command python3

[[ -n "${BINARY}" ]] || die "usage: $0 <greggd-binary-path> [port]"
[[ -x "${BINARY}" ]] || die "binary not found or not executable: ${BINARY}"
is_nonnegative_number "${STARTUP_DEADLINE_SECS}" || die "invalid STARTUP_DEADLINE_SECS"
is_nonnegative_number "${SHUTDOWN_DEADLINE_SECS}" || die "invalid SHUTDOWN_DEADLINE_SECS"
is_positive_integer "${MAX_PORT_ATTEMPTS}" || die "invalid MAX_PORT_ATTEMPTS"

umask 077
TEMP_DIR="$(mktemp -d)"
CONFIG_FILE="${TEMP_DIR}/greggd.toml"

if [[ "${REQUESTED_PORT}" != "0" ]]; then
    [[ "${REQUESTED_PORT}" =~ ^[0-9]+$ ]] || die "invalid port: ${REQUESTED_PORT}"
    ((REQUESTED_PORT >= 1 && REQUESTED_PORT <= 65535)) || die "port out of range: ${REQUESTED_PORT}"
    start_and_verify "${REQUESTED_PORT}" 1
else
    ALLOW_PORT_RETRY=1
    verified=0
    for attempt in $(seq 1 "${MAX_PORT_ATTEMPTS}"); do
        PORT="$(allocate_port)"
        if start_and_verify "${PORT}" "${attempt}"; then
            verified=1
            break
        else
            status=$?
            if ((status != 75)); then
                exit "${status}"
            fi
        fi
    done
    ((verified == 1)) || die "all ${MAX_PORT_ATTEMPTS} isolated port attempts failed"
fi

echo "=== all installed-daemon checks passed ==="
