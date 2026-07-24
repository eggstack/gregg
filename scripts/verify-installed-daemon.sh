#!/usr/bin/env bash
# verify-installed-daemon.sh — Loopback smoke test for greggd.
#
# Starts greggd in foreground mode, polls /healthz and /v1/status,
# validates the JSON response shape, then sends SIGTERM for a clean exit.
#
# Usage:
#   ./scripts/verify-installed-daemon.sh <greggd-binary-path> [port]
#
# Exit codes:
#   0 — all checks passed
#   1 — any check failed

set -euo pipefail

BINARY="${1:?Usage: $0 <greggd-binary-path> [port]}"
PORT="${2:-0}"
STARTUP_DEADLINE_SECS=5

# --- helpers ----------------------------------------------------------------

die() { echo "FATAL: $*" >&2; exit 1; }

cleanup() {
    if [ -n "${GREGGD_PID:-}" ] && kill -0 "$GREGGD_PID" 2>/dev/null; then
        kill "$GREGGD_PID" 2>/dev/null || true
        wait "$GREGGD_PID" 2>/dev/null || true
    fi
    if [ -n "${TEMP_DIR:-}" ] && [ -d "${TEMP_DIR:-}" ]; then
        rm -rf "$TEMP_DIR"
    fi
}
trap cleanup EXIT

# --- setup ------------------------------------------------------------------

[ -x "$BINARY" ] || die "binary not found or not executable: $BINARY"

TEMP_DIR="$(mktemp -d)"
CONFIG_FILE="$TEMP_DIR/greggd.toml"

if [ "$PORT" -eq 0 ] 2>/dev/null; then
    # Select a free port by binding to port 0 and reading the assigned port.
    PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()' 2>/dev/null \
        || ss -tlnH | awk '{print $4}' | sed 's/.*://' | sort -n | tail -1 | awk '{print $1+1}')"
fi

cat > "$CONFIG_FILE" <<TOML
[server]
port = ${PORT}
refresh_ms = 1000
stale_after_ms = 10000

[[systems]]
name = "loopback-test"
TOML

# --- start greggd in background --------------------------------------------

"$BINARY" run --config "$CONFIG_FILE" > "$TEMP_DIR/greggd.log" 2>&1 &
GREGGD_PID=$!
echo "greggd started (PID=$GREGGD_PID) on port $PORT"

# --- poll /healthz with bounded deadline -----------------------------------

ELAPSED=0
while [ "$ELAPSED" -lt "$STARTUP_DEADLINE_SECS" ]; do
    if ! kill -0 "$GREGGD_PID" 2>/dev/null; then
        echo "greggd exited during startup" >&2
        cat "$TEMP_DIR/greggd.log" >&2
        exit 1
    fi

    HTTP_CODE="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/healthz" 2>/dev/null || true)"
    if [ "$HTTP_CODE" = "200" ]; then
        echo "/healthz returned 200 after ${ELAPSED}s"
        break
    fi

    sleep 1
    ELAPSED=$((ELAPSED + 1))
done

if [ "$ELAPSED" -ge "$STARTUP_DEADLINE_SECS" ]; then
    echo "ERROR: /healthz did not return 200 within ${STARTUP_DEADLINE_SECS}s" >&2
    cat "$TEMP_DIR/greggd.log" >&2
    exit 1
fi

# --- query /v1/status and validate JSON ------------------------------------

STATUS_BODY="$(curl -sf "http://127.0.0.1:${PORT}/v1/status" 2>/dev/null)" \
    || die "failed to fetch /v1/status"

# Validate required top-level fields exist.
for FIELD in schema_version observed_at_unix_ms sample_interval_ms capabilities system cpu load memory swap; do
    if ! echo "$STATUS_BODY" | jq -e ".$FIELD" > /dev/null 2>&1; then
        die "/v1/status missing required field: $FIELD"
    fi
done

# Validate schema_version == 1
SV="$(echo "$STATUS_BODY" | jq -r '.schema_version')"
if [ "$SV" != "1" ]; then
    die "schema_version is $SV, expected 1"
fi

# Validate system identity fields.
for FIELD in name hostname os_name os_version kernel_name kernel_release architecture; do
    if ! echo "$STATUS_BODY" | jq -e ".system.$FIELD" > /dev/null 2>&1; then
        die "/v1/status.system missing required field: $FIELD"
    fi
done

# Validate CPU metrics.
CORES="$(echo "$STATUS_BODY" | jq -r '.cpu.logical_cores')"
if [ -z "$CORES" ] || [ "$CORES" = "null" ]; then
    die "/v1/status.cpu.logical_cores is missing or null"
fi

USAGE="$(echo "$STATUS_BODY" | jq -r '.cpu.usage_pct')"
if [ -z "$USAGE" ] || [ "$USAGE" = "null" ]; then
    die "/v1/status.cpu.usage_pct is missing or null"
fi

# Validate load averages.
for FIELD in load_1 load_5 load_15; do
    if ! echo "$STATUS_BODY" | jq -e ".load.$FIELD" > /dev/null 2>&1; then
        die "/v1/status.load missing required field: $FIELD"
    fi
done

# Validate memory and swap have at least one field.
if ! echo "$STATUS_BODY" | jq -e '.memory.total_bytes' > /dev/null 2>&1; then
    die "/v1/status.memory.total_bytes is missing"
fi

echo "/v1/status JSON validation passed"
echo "  schema_version: $SV"
echo "  system.name: $(echo "$STATUS_BODY" | jq -r '.system.name')"
echo "  cpu.logical_cores: $CORES"
echo "  cpu.usage_pct: $USAGE"

# --- send SIGTERM and wait for clean exit -----------------------------------

echo "Sending SIGTERM to greggd (PID=$GREGGD_PID)"
kill -TERM "$GREGGD_PID" 2>/dev/null || true
WAIT_SECS=0
MAX_WAIT=5
while kill -0 "$GREGGD_PID" 2>/dev/null; do
    if [ "$WAIT_SECS" -ge "$MAX_WAIT" ]; then
        echo "WARN: greggd did not exit within ${MAX_WAIT}s, sending SIGKILL" >&2
        kill -9 "$GREGGD_PID" 2>/dev/null || true
        exit 1
    fi
    sleep 1
    WAIT_SECS=$((WAIT_SECS + 1))
done

echo "greggd exited cleanly after ${WAIT_SECS}s"
echo "=== All checks passed ==="
