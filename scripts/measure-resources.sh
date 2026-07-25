#!/usr/bin/env bash
# Measure a supplied greggd release binary against a valid isolated config.
#
# Usage:
#   measure-resources.sh --daemon <path> [--port <port>] [--duration-secs <n>]
#                        [--output-dir <dir>]

set -euo pipefail

DAEMON=""
MODE="smoke"
CANDIDATE_SHA=""
RELEASE_VERSION=""
STAGE=""
RUN_ID="local"
ATTEMPT="1"
JOB_NAME="resource-measurement"
PROVENANCE=""
PACKAGE="greggd"
PORT=0
DURATION_SECS=30
INTERVAL_SECS=5
OUTPUT_DIR=""
TEMP_DIR=""
DAEMON_PID=""

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
        --port)
            (($# >= 2)) || usage
            PORT="$2"
            shift 2
            ;;
        --duration-secs)
            (($# >= 2)) || usage
            DURATION_SECS="$2"
            shift 2
            ;;
        --output-dir)
            (($# >= 2)) || usage
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --interval-secs)
            (($# >= 2)) || usage
            INTERVAL_SECS="$2"
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
[[ "${DURATION_SECS}" =~ ^[1-9][0-9]*$ ]] || die "invalid duration: ${DURATION_SECS}"
[[ "${INTERVAL_SECS}" =~ ^[1-9][0-9]*$ ]] || die "invalid interval: ${INTERVAL_SECS}"
if [[ "${MODE}" == release ]]; then
    ((DURATION_SECS >= 1800)) || die "release resource measurements require at least 1800 seconds"
    [[ -n "${PROVENANCE}" ]] || die "release resource measurements require --provenance"
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
OUTPUT_DIR="${OUTPUT_DIR:-resource-measurement-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "${OUTPUT_DIR}"
CONFIG_PATH="${TEMP_DIR}/greggd.toml"
cat >"${CONFIG_PATH}" <<TOML
name = "resource-measurement"
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
    if ! kill -0 "${DAEMON_PID}" 2>/dev/null; then
        die "greggd exited before readiness"
    fi
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

CSV_PATH="${OUTPUT_DIR}/resource-samples.csv"
SUMMARY_PATH="${OUTPUT_DIR}/resource-summary.json"
METADATA_PATH="${OUTPUT_DIR}/candidate.json"
printf 'timestamp,elapsed_secs,rss_kb,cpu_pct,threads,fd_count,payload_bytes,latency_ms\n' >"${CSV_PATH}"

for ((elapsed = 0; elapsed < DURATION_SECS; elapsed += INTERVAL_SECS)); do
    kill -0 "${DAEMON_PID}" 2>/dev/null || die "greggd exited during measurement"
    response_file="${TEMP_DIR}/status-${elapsed}.json"
    set +e
    latency="$(curl --silent --show-error --connect-timeout 1 --max-time 2 \
        --output "${response_file}" --write-out '%{time_total},%{http_code},%{size_download}' \
        "http://127.0.0.1:${PORT}/v1/status" 2>"${TEMP_DIR}/curl.err")"
    curl_status=$?
    set -e
    ((curl_status == 0)) || die "status request failed at ${elapsed}s"
    IFS=, read -r latency_secs status_code payload_bytes <<<"${latency}"
    [[ "${status_code}" == "200" ]] || die "status request returned HTTP ${status_code} at ${elapsed}s"
    jq -e '.schema_version == 1' "${response_file}" >/dev/null ||
        die "status response was malformed at ${elapsed}s"

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
    latency_ms="$(awk -v seconds="${latency_secs}" 'BEGIN { printf "%.3f", seconds * 1000 }')"
    printf '%s,%d,%s,%s,%s,%s,%s,%s\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${elapsed}" "${rss}" "${cpu}" \
        "${threads:-NA}" "${fd_count:-NA}" "${payload_bytes}" "${latency_ms}" >>"${CSV_PATH}"

    if ((elapsed + INTERVAL_SECS < DURATION_SECS)); then
        sleep "${INTERVAL_SECS}"
    elif [[ "${MODE}" == "release" ]]; then
        sleep "$((DURATION_SECS - elapsed))"
    fi
done

COMPLETED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
END_MONOTONIC="$(python3 -c 'import time; print(time.monotonic_ns())')"
OBSERVED_SECS="$(python3 - "${START_MONOTONIC}" "${END_MONOTONIC}" <<'PY'
import sys

print(int((int(sys.argv[2]) - int(sys.argv[1])) / 1_000_000_000))
PY
)"

python3 - "${CSV_PATH}" "${SUMMARY_PATH}" "${MODE}" "${DURATION_SECS}" "${OBSERVED_SECS}" "${INTERVAL_SECS}" "${STARTED_AT}" "${COMPLETED_AT}" "${START_MONOTONIC}" "${END_MONOTONIC}" <<'PY'
import csv
import json
import statistics
import sys

csv_path, summary_path, mode, configured, observed, interval, started_at, completed_at, monotonic_start, monotonic_end = sys.argv[1:]
with open(csv_path, newline="", encoding="utf-8") as handle:
    rows = list(csv.DictReader(handle))

def values(name):
    return [float(row[name]) for row in rows if row[name] != "NA"]

def percentile(items, fraction):
    if not items:
        return None
    ordered = sorted(items)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * fraction)))
    return ordered[index]

summary = {
    "samples": len(rows),
    "configured_duration_secs": int(configured),
    "observed_duration_secs": int(observed),
    "started_at": started_at,
    "completed_at": completed_at,
    "monotonic_start_ns": int(monotonic_start),
    "monotonic_end_ns": int(monotonic_end),
    "rss_kb_max": max(values("rss_kb"), default=None),
    "cpu_pct_avg": statistics.fmean(values("cpu_pct")) if values("cpu_pct") else None,
    "latency_ms_p50": percentile(values("latency_ms"), 0.50),
    "latency_ms_p95": percentile(values("latency_ms"), 0.95),
    "latency_ms_p99": percentile(values("latency_ms"), 0.99),
    "payload_bytes_max": max((int(row["payload_bytes"]) for row in rows), default=None),
}
summary["minimum_expected_samples"] = (summary["configured_duration_secs"] + int(interval) - 1) // int(interval)
summary["thresholds"] = {"rss_kb_max": 16384, "cpu_pct_avg": 0.2, "payload_bytes_max": 2048, "latency_ms_p95": 10.0}
summary["qualification"] = {
    "duration": summary["observed_duration_secs"] >= summary["configured_duration_secs"],
    "samples": summary["samples"] >= summary["minimum_expected_samples"],
    "rss": summary["rss_kb_max"] is not None and summary["rss_kb_max"] <= summary["thresholds"]["rss_kb_max"],
    "cpu": summary["cpu_pct_avg"] is not None and summary["cpu_pct_avg"] <= summary["thresholds"]["cpu_pct_avg"],
    "payload": summary["payload_bytes_max"] is not None and summary["payload_bytes_max"] <= summary["thresholds"]["payload_bytes_max"],
    "latency": summary["latency_ms_p95"] is not None and summary["latency_ms_p95"] <= summary["thresholds"]["latency_ms_p95"],
}
summary["result"] = "pass" if all(summary["qualification"].values()) else "fail"
with open(summary_path, "w", encoding="utf-8") as handle:
    json.dump(summary, handle, indent=2, sort_keys=True)
    handle.write("\n")
print(json.dumps(summary, indent=2, sort_keys=True))
if mode == "release" and summary["result"] != "pass":
    raise SystemExit("resource qualification failed")
PY

jq -n --arg summary "$(basename "${SUMMARY_PATH}")" --arg samples "$(basename "${CSV_PATH}")" '[{name:$summary},{name:$samples}]' >"${TEMP_DIR}/artifacts.json"
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

echo "Resource measurement complete: ${OUTPUT_DIR}"
