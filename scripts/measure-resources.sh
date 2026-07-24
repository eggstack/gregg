#!/usr/bin/env bash
# Measure a supplied greggd release binary against a valid isolated config.
#
# Usage:
#   measure-resources.sh --daemon <path> [--port <port>] [--duration-secs <n>]
#                        [--output-dir <dir>]

set -euo pipefail

DAEMON=""
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
    echo "usage: $0 --daemon <path> [--port <port>] [--duration-secs <n>] [--output-dir <dir>]" >&2
    exit 2
}

while (($# > 0)); do
    case "$1" in
        --daemon)
            (($# >= 2)) || usage
            DAEMON="$2"
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
[[ "${PORT}" =~ ^[0-9]+$ ]] && ((PORT <= 65535)) || die "invalid port: ${PORT}"
[[ "${DURATION_SECS}" =~ ^[1-9][0-9]*$ ]] || die "invalid duration: ${DURATION_SECS}"
[[ "${INTERVAL_SECS}" =~ ^[1-9][0-9]*$ ]] || die "invalid interval: ${INTERVAL_SECS}"
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
SUMMARY_PATH="${OUTPUT_DIR}/summary.json"
METADATA_PATH="${OUTPUT_DIR}/candidate.json"
printf 'timestamp,elapsed_secs,rss_kb,cpu_pct,threads,fd_count,payload_bytes,latency_ms\n' >"${CSV_PATH}"

cat >"${METADATA_PATH}" <<JSON
{
  "candidate_sha": "${CANDIDATE_SHA:-unknown}",
  "started_at": "${STARTED_AT}",
  "host_os": "$(uname -s)",
  "host_architecture": "$(uname -m)",
  "daemon": "${DAEMON}",
  "port": ${PORT},
  "duration_secs": ${DURATION_SECS},
  "interval_secs": ${INTERVAL_SECS}
}
JSON

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
    fi
done

python3 - "${CSV_PATH}" "${SUMMARY_PATH}" <<'PY'
import csv
import json
import statistics
import sys

csv_path, summary_path = sys.argv[1:]
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
    "rss_kb_max": max(values("rss_kb"), default=None),
    "cpu_pct_avg": statistics.fmean(values("cpu_pct")) if values("cpu_pct") else None,
    "latency_ms_p50": percentile(values("latency_ms"), 0.50),
    "latency_ms_p95": percentile(values("latency_ms"), 0.95),
    "latency_ms_p99": percentile(values("latency_ms"), 0.99),
    "payload_bytes_max": max((int(row["payload_bytes"]) for row in rows), default=None),
}
with open(summary_path, "w", encoding="utf-8") as handle:
    json.dump(summary, handle, indent=2, sort_keys=True)
    handle.write("\n")
print(json.dumps(summary, indent=2, sort_keys=True))
PY

echo "Resource measurement complete: ${OUTPUT_DIR}"
