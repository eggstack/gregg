#!/usr/bin/env bash
# Run a bounded daemon and mixed-fleet HTTP soak against supplied binaries.
#
# Usage:
#   soak-test.sh --daemon <path> [--duration-minutes <n>] [--interval-secs <n>]
#                [--port <port>] [--output-dir <dir>] [--fleet-url <url> ...]

set -euo pipefail

DAEMON=""
MODE="smoke"
CANDIDATE_SHA=""
RELEASE_VERSION=""
STAGE=""
RUN_ID="local"
ATTEMPT="1"
JOB_NAME="daemon-soak"
PROVENANCE=""
PACKAGE="greggd"
PORT=0
DURATION_MINUTES=1440
INTERVAL_SECS=5
OUTPUT_DIR=""
TEMP_DIR=""
DAEMON_PID=""
FIXTURE_PID=""
FLEET_URLS=()

die() {
    echo "FATAL: $*" >&2
    if [[ -n "${TEMP_DIR}" && -f "${TEMP_DIR}/greggd.log" ]]; then
        echo "=== greggd log ===" >&2
        cat "${TEMP_DIR}/greggd.log" >&2
    fi
    exit 1
}

file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

file_size() {
    if stat -c '%s' "$1" >/dev/null 2>&1; then
        stat -c '%s' "$1"
    else
        stat -f '%z' "$1"
    fi
}

cleanup() {
    set +e
    if [[ -n "${DAEMON_PID}" ]]; then
        kill -TERM "${DAEMON_PID}" 2>/dev/null
        wait "${DAEMON_PID}" 2>/dev/null
    fi
    if [[ -n "${FIXTURE_PID}" ]]; then
        kill "${FIXTURE_PID}" 2>/dev/null
        wait "${FIXTURE_PID}" 2>/dev/null
    fi
    if [[ -n "${TEMP_DIR}" && -d "${TEMP_DIR}" ]]; then
        rm -rf "${TEMP_DIR}"
    fi
    set -e
}
trap cleanup EXIT

usage() {
    echo "usage: $0 --daemon <path> --candidate-sha SHA --release-version VERSION --stage STAGE [options]" >&2
    exit 2
}

while (($# > 0)); do
    case "$1" in
        --daemon)
            (($# >= 2)) || usage
            DAEMON="$2"
            shift 2
            ;;
        --mode|--candidate-sha|--release-version|--stage|--run-id|--attempt|--job-name|--provenance|--package)
            (($# >= 2)) || usage
            case "$1" in
                --mode) MODE="$2" ;; --candidate-sha) CANDIDATE_SHA="$2" ;;
                --release-version) RELEASE_VERSION="$2" ;; --stage) STAGE="$2" ;;
                --run-id) RUN_ID="$2" ;; --attempt) ATTEMPT="$2" ;; --job-name) JOB_NAME="$2" ;;
                --provenance) PROVENANCE="$2" ;; --package) PACKAGE="$2" ;;
            esac
            shift 2
            ;;
        --duration-minutes)
            (($# >= 2)) || usage
            DURATION_MINUTES="$2"
            shift 2
            ;;
        --interval-secs)
            (($# >= 2)) || usage
            INTERVAL_SECS="$2"
            shift 2
            ;;
        --port)
            (($# >= 2)) || usage
            PORT="$2"
            shift 2
            ;;
        --output-dir)
            (($# >= 2)) || usage
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --fleet-url)
            (($# >= 2)) || usage
            FLEET_URLS+=("$2")
            shift 2
            ;;
        *)
            usage
            ;;
    esac
done

[[ -x "${DAEMON}" ]] || die "daemon is not executable: ${DAEMON}"
[[ "${MODE}" == smoke || "${MODE}" == release ]] || die "mode must be smoke or release"
[[ "${CANDIDATE_SHA}" =~ ^[0-9a-f]{40}$ ]] || die "candidate SHA must be a lowercase full 40-character SHA"
[[ -n "${RELEASE_VERSION}" && -n "${STAGE}" ]] || die "release version and stage are required"
[[ "${PORT}" =~ ^[0-9]+$ ]] && ((PORT <= 65535)) || die "invalid port: ${PORT}"
[[ "${DURATION_MINUTES}" =~ ^[1-9][0-9]*$ ]] || die "invalid duration: ${DURATION_MINUTES}"
[[ "${INTERVAL_SECS}" =~ ^[1-9][0-9]*$ ]] || die "invalid interval: ${INTERVAL_SECS}"
if [[ "${MODE}" == release ]]; then
    ((DURATION_MINUTES >= 1440)) || die "release soaks require at least 1440 minutes"
    [[ -n "${PROVENANCE}" ]] || die "release soaks require --provenance"
fi
command -v curl >/dev/null 2>&1 || die "required command not found: curl"
command -v jq >/dev/null 2>&1 || die "required command not found: jq"
command -v python3 >/dev/null 2>&1 || die "required command not found: python3"

if ((PORT == 0)); then
    PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
fi

umask 077
TEMP_DIR="$(mktemp -d)"
OUTPUT_DIR="${OUTPUT_DIR:-soak-results-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "${OUTPUT_DIR}"
FIXTURE_PORT="$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
python3 "$(dirname "${BASH_SOURCE[0]}")/tests/fleet-fixture.py" --port "${FIXTURE_PORT}" --log "${OUTPUT_DIR}/fleet-fixture.log" >"${TEMP_DIR}/fixture.log" 2>&1 &
FIXTURE_PID=$!
sleep 0.2
kill -0 "${FIXTURE_PID}" 2>/dev/null || die "fleet fixture failed to start"
CONFIG_PATH="${TEMP_DIR}/greggd.toml"
cat >"${CONFIG_PATH}" <<TOML
name = "soak-test"
host = "127.0.0.1"
port = ${PORT}
sample_interval_ms = 1000
stale_after_ms = 10000
TOML

STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
START_MONOTONIC="$(python3 -c 'import time; print(time.monotonic_ns())')"
"${DAEMON}" run --config "${CONFIG_PATH}" >"${TEMP_DIR}/greggd.log" 2>&1 &
DAEMON_PID=$!
ready=0
for _ in $(seq 1 50); do
    kill -0 "${DAEMON_PID}" 2>/dev/null || die "greggd exited before readiness"
    set +e
    code="$(curl --silent --show-error --connect-timeout 1 --max-time 2 \
        --output "${TEMP_DIR}/health.json" --write-out '%{http_code}' \
        "http://127.0.0.1:${PORT}/healthz" 2>"${TEMP_DIR}/curl.err")"
    curl_status=$?
    set -e
    if ((curl_status == 0)) && [[ "${code}" == "200" ]] &&
        jq -e '.schema_version == 1 and .state == "ready"' "${TEMP_DIR}/health.json" >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 0.2
done
((ready == 1)) || die "greggd did not become ready"

CSV_PATH="${OUTPUT_DIR}/soak-samples.csv"
SUMMARY_PATH="${OUTPUT_DIR}/soak-summary.json"
METADATA_PATH="${OUTPUT_DIR}/candidate.json"
printf 'timestamp,elapsed_secs,rss_kb,cpu_pct,threads,fd_count,daemon_status,payload_bytes,latency_ms,fleet_results\n' >"${CSV_PATH}"

MAX_ELAPSED=$((DURATION_MINUTES * 60))
for ((elapsed = 0; elapsed < MAX_ELAPSED; elapsed += INTERVAL_SECS)); do
    kill -0 "${DAEMON_PID}" 2>/dev/null || die "greggd exited at ${elapsed}s"
    response_file="${TEMP_DIR}/status-${elapsed}.json"
    set +e
    result="$(curl --silent --show-error --connect-timeout 1 --max-time 2 \
        --output "${response_file}" --write-out '%{http_code},%{size_download},%{time_total}' \
        "http://127.0.0.1:${PORT}/v1/status" 2>"${TEMP_DIR}/curl.err")"
    curl_status=$?
    set -e
    ((curl_status == 0)) || die "daemon status request failed at ${elapsed}s"
    IFS=, read -r daemon_status payload_bytes latency_secs <<<"${result}"
    [[ "${daemon_status}" == "200" ]] || die "daemon status returned HTTP ${daemon_status} at ${elapsed}s"
    jq -e '.schema_version == 1' "${response_file}" >/dev/null ||
        die "daemon status JSON was malformed at ${elapsed}s"

    rss="$(ps -o rss= -p "${DAEMON_PID}" | tr -d ' ')"
    cpu="$(ps -o %cpu= -p "${DAEMON_PID}" | tr -d ' ')"
    threads="$(ps -o nlwp= -p "${DAEMON_PID}" 2>/dev/null | tr -d ' ' || true)"
    if [[ -z "${threads}" ]]; then
        threads="$(ps -o thcount= -p "${DAEMON_PID}" 2>/dev/null | tr -d ' ' || true)"
    fi
    if [[ -d "/proc/${DAEMON_PID}/fd" ]]; then
        fd_count="$(find "/proc/${DAEMON_PID}/fd" -mindepth 1 -maxdepth 1 -type l | wc -l | tr -d ' ')"
    else
        fd_count="$(lsof -p "${DAEMON_PID}" 2>/dev/null | tail -n +2 | wc -l | tr -d ' ' || true)"
    fi

    fleet_results=""
    fleet_targets=(
        "http://127.0.0.1:${FIXTURE_PORT}/healthy"
        "http://127.0.0.1:9/v1/status"
        "http://127.0.0.1:${FIXTURE_PORT}/offline"
        "http://127.0.0.1:${FIXTURE_PORT}/slow"
        "http://127.0.0.1:${FIXTURE_PORT}/timeout"
        "http://127.0.0.1:${FIXTURE_PORT}/malformed"
        "http://127.0.0.1:${FIXTURE_PORT}/error"
        "http://127.0.0.1:${FIXTURE_PORT}/stale"
        "http://127.0.0.1:${FIXTURE_PORT}/recover"
        "${FLEET_URLS[@]}"
    )
    for endpoint in "${fleet_targets[@]}"; do
        set +e
        fleet_result="$(curl --silent --show-error --connect-timeout 1 --max-time 2 \
            --output /dev/null --write-out '%{http_code},%{time_total}' \
            "${endpoint}" 2>/dev/null)"
        fleet_curl_status=$?
        set -e
        if ((fleet_curl_status != 0)); then
            fleet_result="000,timeout"
        fi
        if [[ -n "${fleet_results}" ]]; then
            fleet_results+=";"
        fi
        fleet_results+="${endpoint}:${fleet_result}"
    done

    latency_ms="$(awk -v seconds="${latency_secs}" 'BEGIN { printf "%.3f", seconds * 1000 }')"
    printf '%s,%d,%s,%s,%s,%s,%s,%s,%s,"%s"\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${elapsed}" "${rss}" "${cpu}" \
        "${threads:-NA}" "${fd_count:-NA}" "${daemon_status}" "${payload_bytes}" \
        "${latency_ms}" "${fleet_results}" >>"${CSV_PATH}"

    if ((elapsed + INTERVAL_SECS < MAX_ELAPSED)); then
        sleep "${INTERVAL_SECS}"
    elif [[ "${MODE}" == "release" ]]; then
        sleep "$((MAX_ELAPSED - elapsed))"
    fi
done

COMPLETED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
END_MONOTONIC="$(python3 -c 'import time; print(time.monotonic_ns())')"
OBSERVED_SECS="$(python3 - "${START_MONOTONIC}" "${END_MONOTONIC}" <<'PY'
import sys

print(int((int(sys.argv[2]) - int(sys.argv[1])) / 1_000_000_000))
PY
)"
EXPECTED_SAMPLES=$(( (MAX_ELAPSED + INTERVAL_SECS - 1) / INTERVAL_SECS ))

set +e
kill -TERM "${DAEMON_PID}"
kill_status=$?
wait "${DAEMON_PID}"
daemon_exit=$?
set -e
DAEMON_PID=""
((kill_status == 0)) || die "failed to send SIGTERM to greggd"
((daemon_exit == 0)) || die "greggd exited with status ${daemon_exit} after soak"

python3 - "${CSV_PATH}" "${SUMMARY_PATH}" "${MODE}" "${MAX_ELAPSED}" "${OBSERVED_SECS}" "${EXPECTED_SAMPLES}" "${STARTED_AT}" "${COMPLETED_AT}" "${START_MONOTONIC}" "${END_MONOTONIC}" <<'PY'
import csv
import json
import sys

csv_path, summary_path, mode, configured, observed, expected, started_at, completed_at, monotonic_start, monotonic_end = sys.argv[1:]
with open(csv_path, newline="", encoding="utf-8") as handle:
    rows = list(csv.DictReader(handle))

def numbers(name):
    return [float(row[name]) for row in rows if row[name] != "NA"]

summary = {
    "samples": len(rows),
    "configured_duration_secs": int(configured),
    "observed_duration_secs": int(observed),
    "started_at": started_at,
    "completed_at": completed_at,
    "monotonic_start_ns": int(monotonic_start),
    "monotonic_end_ns": int(monotonic_end),
    "minimum_expected_samples": int(expected),
    "rss_kb_max": max(numbers("rss_kb"), default=None),
    "cpu_pct_max": max(numbers("cpu_pct"), default=None),
    "payload_bytes_max": max((int(row["payload_bytes"]) for row in rows), default=None),
    "latency_ms_max": max(numbers("latency_ms"), default=None),
    "fleet_samples_recorded": sum(1 for row in rows if row["fleet_results"]),
    "fixture_states": ["healthy", "offline", "slow", "timeout", "malformed", "error", "stale", "recover"],
}
summary["qualification"] = {
    "duration": mode != "release" or summary["observed_duration_secs"] >= summary["configured_duration_secs"],
    "samples": summary["samples"] >= summary["minimum_expected_samples"],
    "fleet_fixture": summary["fleet_samples_recorded"] == summary["samples"],
}
summary["result"] = "pass" if all(summary["qualification"].values()) else "fail"
with open(summary_path, "w", encoding="utf-8") as handle:
    json.dump(summary, handle, indent=2, sort_keys=True)
    handle.write("\n")
print(json.dumps(summary, indent=2, sort_keys=True))
if mode == "release" and summary["result"] != "pass":
    raise SystemExit("soak qualification failed")
PY

jq -n --arg summary "$(basename "${SUMMARY_PATH}")" --arg samples "$(basename "${CSV_PATH}")" --arg fixture fleet-fixture.log '[{name:$summary},{name:$samples},{name:$fixture}]' >"${TEMP_DIR}/artifacts.json"
metadata_args=(
    write-candidate --output "${METADATA_PATH}" --candidate-sha "${CANDIDATE_SHA}" --release-version "${RELEASE_VERSION}"
    --stage "${STAGE}" --workflow-run-id "${RUN_ID}" --workflow-run-attempt "${ATTEMPT}"
    --job-name "${JOB_NAME}" --runner-os "$(uname -s)" --runner-architecture "$(uname -m)"
    --started-at "${STARTED_AT}" --completed-at "${COMPLETED_AT}" --artifacts-json "${TEMP_DIR}/artifacts.json"
    --executable "${PACKAGE}" "$(file_sha256 "${DAEMON}")" "$(file_size "${DAEMON}")"
    --note "mode=${MODE}"
)
if [[ -n "${PROVENANCE}" ]]; then
    metadata_args+=(--note "provenance=${PROVENANCE}")
fi
python3 "$(dirname "${BASH_SOURCE[0]}")/validate-release-evidence.py" "${metadata_args[@]}"

echo "Soak test complete: ${OUTPUT_DIR}"
