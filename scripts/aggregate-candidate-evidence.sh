#!/usr/bin/env bash
# Aggregate candidate.json files downloaded from one release workflow run.
#
# Usage: aggregate-candidate-evidence.sh <evidence-dir> <candidate-sha> <output>

set -euo pipefail

EVIDENCE_DIR="${1:-}"
EXPECTED_SHA="${2:-}"
OUTPUT_PATH="${3:-}"

die() {
    echo "FATAL: $*" >&2
    exit 1
}

[[ -d "${EVIDENCE_DIR}" ]] || die "evidence directory does not exist: ${EVIDENCE_DIR}"
[[ "${EXPECTED_SHA}" =~ ^[0-9a-f]{40}$ ]] || die "candidate SHA must be a full 40-character SHA"
[[ -n "${OUTPUT_PATH}" ]] || die "usage: $0 <evidence-dir> <candidate-sha> <output>"

mapfile -t METADATA_FILES < <(find "${EVIDENCE_DIR}" -type f -name candidate.json -print | sort)
(( ${#METADATA_FILES[@]} > 0 )) || die "no candidate.json files found under ${EVIDENCE_DIR}"

for metadata in "${METADATA_FILES[@]}"; do
    jq -e --arg expected "${EXPECTED_SHA}" \
        '.candidate_sha == $expected and (.version | type == "string") and (.stage | type == "string")' \
        "${metadata}" >/dev/null \
        || die "mixed or malformed candidate metadata: ${metadata}"
done

mkdir -p "$(dirname "${OUTPUT_PATH}")"
jq -n \
    --arg candidate_sha "${EXPECTED_SHA}" \
    --slurpfile jobs <(jq -s '.' "${METADATA_FILES[@]}") \
    '{candidate_sha: $candidate_sha, jobs: $jobs[0]}' \
    >"${OUTPUT_PATH}"

jq -e --arg expected "${EXPECTED_SHA}" \
    '.candidate_sha == $expected and (.jobs | length > 0) and all(.jobs[]; .candidate_sha == $expected)' \
    "${OUTPUT_PATH}" >/dev/null \
    || die "aggregate manifest failed self-validation"

echo "Wrote aggregate candidate manifest: ${OUTPUT_PATH}"
