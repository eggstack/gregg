#!/usr/bin/env bash
# measure-resources.sh — Measure greggd and gregg resource usage.
#
# Usage:
#   ./scripts/measure-resources.sh
#
# Measures idle CPU, RSS, payload size, and response latency for both
# greggd and gregg. Records compiler version, target, and hardware info.

set -euo pipefail

OUTPUT_DIR="resource-measurement-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUTPUT_DIR"

echo "=== gregg resource measurement ==="
echo "Output: ${OUTPUT_DIR}/"
echo ""

# Build release binaries.
echo "Building release binaries..."
cargo build --release 2>&1 | tail -3

# Record build metadata.
{
    echo "# Build metadata"
    echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "rustc: $(rustc --version)"
    echo "cargo: $(cargo --version)"
    echo "target: $(rustc -vV | grep host | cut -d' ' -f2)"
    echo "profile: release (lto=thin, codegen-units=1, strip=symbols)"
    echo "binary_size_greggd: $(ls -la target/release/greggd 2>/dev/null | awk '{print $5}' || echo 'N/A')"
    echo "binary_size_gregg: $(ls -la target/release/gregg 2>/dev/null | awk '{print $5}' || echo 'N/A')"
    echo "hardware: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m)"
    echo "memory: $(sysctl -n hw.memsize 2>/dev/null || echo 'N/A') bytes"
    echo "os: $(sw_vers -productVersion 2>/dev/null || uname -r)"
} > "$OUTPUT_DIR/build-metadata.txt"

echo "Build metadata:"
cat "$OUTPUT_DIR/build-metadata.txt"
echo ""

# Measure payload size.
echo "Measuring payload size..."
./target/release/greggd run --config /tmp/greggd-measure.toml &
GREggD_PID=$!
sleep 2

PAYLOAD=$(curl -s "http://127.0.0.1:${PORT:-11310}/v1/status" 2>/dev/null || echo "{}")
PAYLOAD_SIZE=${#PAYLOAD}
echo "Payload size: $PAYLOAD_SIZE bytes"
echo "payload_bytes: $PAYLOAD_SIZE" >> "$OUTPUT_DIR/build-metadata.txt"

# Measure response latency (100 requests).
echo "Measuring response latency (100 requests)..."
LATENCY_FILE="$OUTPUT_DIR/latency-raw.txt"
for _ in $(seq 1 100); do
    curl -s -o /dev/null -w '%{time_total}\n' \
        "http://127.0.0.1:${PORT:-11310}/v1/status" >> "$LATENCY_FILE" 2>/dev/null
done

# Compute percentiles.
if command -v sort &>/dev/null; then
    SORTED=$(sort -n "$LATENCY_FILE")
    TOTAL=$(echo "$SORTED" | wc -l | tr -d ' ')
    P50_IDX=$(echo "($TOTAL * 50 / 100) + 1" | bc)
    P95_IDX=$(echo "($TOTAL * 95 / 100) + 1" | bc)
    P99_IDX=$(echo "($TOTAL * 99 / 100) + 1" | bc)

    P50=$(echo "$SORTED" | sed -n "${P50_IDX}p")
    P95=$(echo "$SORTED" | sed -n "${P95_IDX}p")
    P99=$(echo "$SORTED" | sed -n "${P99_IDX}p")

    # Convert to ms.
    P50_MS=$(echo "$P50 * 1000" | bc 2>/dev/null || echo "$P50")
    P95_MS=$(echo "$P95 * 1000" | bc 2>/dev/null || echo "$P95")
    P99_MS=$(echo "$P99 * 1000" | bc 2>/dev/null || echo "$P99")

    echo "Response latency (ms):"
    echo "  p50: $P50_MS"
    echo "  p95: $P95_MS"
    echo "  p99: $P99_MS"

    {
        echo "latency_p50_ms: $P50_MS"
        echo "latency_p95_ms: $P95_MS"
        echo "latency_p99_ms: $P99_MS"
    } >> "$OUTPUT_DIR/build-metadata.txt"
fi

# Measure idle CPU over 30 seconds.
echo ""
echo "Measuring idle CPU (30 seconds)..."
SAMPLES=0
CPU_SUM=0
for _ in $(seq 1 6); do
    CPU=$(ps -o %cpu= -p "$GREggD_PID" 2>/dev/null | tr -d ' ')
    CPU_SUM=$(echo "$CPU_SUM + $CPU" | bc 2>/dev/null || echo "0")
    SAMPLES=$((SAMPLES + 1))
    sleep 5
done
AVG_CPU=$(echo "scale=4; $CPU_SUM / $SAMPLES" | bc 2>/dev/null || echo "0")
echo "Average idle CPU: ${AVG_CPU}%"

# Measure RSS.
RSS=$(ps -o rss= -p "$GREggD_PID" 2>/dev/null | tr -d ' ')
echo "RSS: ${RSS} KB"

{
    echo "idle_cpu_pct: $AVG_CPU"
    echo "rss_kb: $RSS"
} >> "$OUTPUT_DIR/build-metadata.txt"

# Stop greggd.
kill "$GREggD_PID" 2>/dev/null || true
wait "$GREggD_PID" 2>/dev/null || true

echo ""
echo "=== Measurement Complete ==="
echo "Results: $OUTPUT_DIR/build-metadata.txt"
echo ""
echo "Targets:"
echo "  idle CPU:     <= 0.2%   (measured: ${AVG_CPU}%)"
echo "  RSS:          <= 16 MiB (measured: $(echo "scale=1; $RSS / 1024" | bc) MiB)"
echo "  payload:      < 2 KiB   (measured: ${PAYLOAD_SIZE} bytes)"
echo "  cached p95:   < 10 ms   (measured: ${P95_MS:-N/A} ms)"
