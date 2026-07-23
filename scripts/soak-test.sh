#!/usr/bin/env bash
# soak-test.sh — Run greggd and gregg soak tests.
#
# Usage:
#   ./scripts/soak-test.sh <duration_minutes> [endpoints]
#
# This script starts greggd in the foreground, then polls it with gregg
# (or curl) to measure resource consumption over the specified duration.
# It tracks RSS, CPU, and task/thread counts at regular intervals.

set -euo pipefail

DURATION_MINUTES="${1:-60}"
ENDPOINTS="${2:-1}"
INTERVAL_SECS=5
OUTPUT_DIR="soak-results-$(date +%Y%m%d-%H%M%S)"

mkdir -p "$OUTPUT_DIR"

echo "=== gregg soak test ==="
echo "Duration: ${DURATION_MINUTES} minutes"
echo "Endpoints: ${ENDPOINTS}"
echo "Sample interval: ${INTERVAL_SECS}s"
echo "Output: ${OUTPUT_DIR}/"
echo ""

# Build release binaries if not present.
if [[ ! -target/release/greggd ]] || [[ ! -target/release/gregg ]]; then
    echo "Building release binaries..."
    cargo build --release
fi

# Start greggd in the background.
echo "Starting greggd..."
./target/release/greggd run \
    --config "${CONFIG_PATH:-/tmp/greggd-soak.toml}" \
    > "$OUTPUT_DIR/greggd.log" 2>&1 &
GREggD_PID=$!
echo "greggd PID: $GREggD_PID"

# Wait for greggd to be ready.
sleep 2

# Sample resources at regular intervals.
ELAPSED=0
MAX_ELAPSED=$((DURATION_MINUTES * 60))
SAMPLE=0

echo "Collecting resource samples..."

while [[ $ELAPSED -lt $MAX_ELAPSED ]]; do
    SAMPLE=$((SAMPLE + 1))
    TIMESTAMP=$(date +%Y-%m-%dT%H:%M:%S)

    # Collect RSS and CPU for greggd.
    if kill -0 "$GREggD_PID" 2>/dev/null; then
        RSS=$(ps -o rss= -p "$GREggD_PID" 2>/dev/null || echo "0")
        CPU=$(ps -o %cpu= -p "$GREggD_PID" 2>/dev/null || echo "0")
        THREADS=$(ps -o nlwp= -p "$GREggD_PID" 2>/dev/null || echo "0")
        FDS=$(ls "/proc/$GREggD_PID/fd" 2>/dev/null | wc -l || echo "N/A")
    else
        echo "ERROR: greggd exited unexpectedly"
        break
    fi

    # Poll the status endpoint.
    PAYLOAD_SIZE=$(curl -s -o /dev/null -w '%{size_download}' \
        "http://127.0.0.1:${PORT:-11310}/v1/status" 2>/dev/null || echo "0")
    RESPONSE_TIME=$(curl -s -o /dev/null -w '%{time_total}' \
        "http://127.0.0.1:${PORT:-11310}/v1/status" 2>/dev/null || echo "0")

    # Log the sample.
    printf "%s,%d,%s,%s,%s,%s,%s,%s\n" \
        "$TIMESTAMP" "$SAMPLE" "$RSS" "$CPU" "$THREADS" "$FDS" \
        "$PAYLOAD_SIZE" "$RESPONSE_TIME" \
        >> "$OUTPUT_DIR/resource-samples.csv"

    if [[ $((SAMPLE % 12)) -eq 0 ]]; then
        printf "[%s] sample=%d rss=%sKB cpu=%s%% threads=%s payload=%sB resp_time=%ss\n" \
            "$TIMESTAMP" "$SAMPLE" "$RSS" "$CPU" "$THREADS" "$PAYLOAD_SIZE" "$RESPONSE_TIME"
    fi

    sleep "$INTERVAL_SECS"
    ELAPSED=$((ELAPSED + INTERVAL_SECS))
done

# Stop greggd.
echo ""
echo "Stopping greggd (PID=$GREggD_PID)..."
kill "$GREggD_PID" 2>/dev/null || true
wait "$GREggD_PID" 2>/dev/null || true

# Generate summary.
echo ""
echo "=== Soak Test Summary ==="
echo "Samples collected: $SAMPLE"
echo "Duration: ${ELAPSED}s"
echo ""

if [[ -f "$OUTPUT_DIR/resource-samples.csv" ]]; then
    echo "RSS (KB): min=$(sort -t, -k3 -n "$OUTPUT_DIR/resource-samples.csv" | head -1 | cut -d, -f3) max=$(sort -t, -k3 -n "$OUTPUT_DIR/resource-samples.csv" | tail -1 | cut -d, -f3)"
    echo "CPU (%):  min=$(sort -t, -k4 -n "$OUTPUT_DIR/resource-samples.csv" | head -1 | cut -d, -f4) max=$(sort -t, -k4 -n "$OUTPUT_DIR/resource-samples.csv" | tail -1 | cut -d, -f4)"
fi

echo ""
echo "Results saved to $OUTPUT_DIR/"
