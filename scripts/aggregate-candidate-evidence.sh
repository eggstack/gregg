#!/usr/bin/env bash
# Backward-compatible per-dispatch entry point for canonical aggregation.
set -euo pipefail

EVIDENCE_DIR="${1:-}"
EXPECTED_SHA="${2:-}"
OUTPUT_PATH="${3:-}"
shift $(( $# >= 3 ? 3 : $# ))
RELEASE_VERSION=""
REQUIRED_STAGES=()

while (($# > 0)); do
    case "$1" in
        --release-version) [[ $# -ge 2 ]] || exit 2; RELEASE_VERSION="$2"; shift 2 ;;
        --required-stage) [[ $# -ge 2 ]] || exit 2; REQUIRED_STAGES+=("$2"); shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

[[ -n "$EVIDENCE_DIR" && -n "$EXPECTED_SHA" && -n "$OUTPUT_PATH" && -n "$RELEASE_VERSION" ]] || {
    echo "usage: $0 <evidence-dir> <candidate-sha> <output> --release-version VERSION --required-stage STAGE ..." >&2
    exit 2
}
((${#REQUIRED_STAGES[@]} > 0)) || { echo "at least one --required-stage is required" >&2; exit 2; }

args=(aggregate --evidence-dir "$EVIDENCE_DIR" --expected-sha "$EXPECTED_SHA" --release-version "$RELEASE_VERSION" --output "$OUTPUT_PATH")
for stage in "${REQUIRED_STAGES[@]}"; do args+=(--required-stage "$stage"); done
python3 "$(dirname "${BASH_SOURCE[0]}")/validate-release-evidence.py" "${args[@]}"
