#!/usr/bin/env bash
# Verify the immutable source identity used by a release gate.
set -euo pipefail

MODE=""
INPUT=""
EXPECTED_SHA=""
OUTPUT=""

usage() {
    echo "usage: $0 --mode pre-tag|tag --input SHA-or-v1.0.1 --candidate-sha SHA [--output PATH]" >&2
    exit 2
}

die() {
    echo "FATAL: $*" >&2
    exit 1
}

while (($# > 0)); do
    case "$1" in
        --mode) [[ $# -ge 2 ]] || usage; MODE="$2"; shift 2 ;;
        --input) [[ $# -ge 2 ]] || usage; INPUT="$2"; shift 2 ;;
        --candidate-sha) [[ $# -ge 2 ]] || usage; EXPECTED_SHA="$2"; shift 2 ;;
        --output) [[ $# -ge 2 ]] || usage; OUTPUT="$2"; shift 2 ;;
        *) usage ;;
    esac
done

[[ "$EXPECTED_SHA" =~ ^[0-9a-f]{40}$ ]] || die "candidate SHA must be a lowercase full 40-character SHA"
git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"

TAG_OBJECT_SHA=""
PEELED_SHA=""
TAGGER_NAME=""
TAGGER_EMAIL=""
TAGGER_TIMESTAMP=""
TAG_OBJECT_CONTENT_SHA=""
if [[ "$MODE" == "pre-tag" ]]; then
    [[ "$INPUT" =~ ^[0-9a-f]{40}$ ]] || die "pre-tag input must be a lowercase full 40-character commit SHA"
    [[ "$(git cat-file -t "$INPUT" 2>/dev/null)" == commit ]] || die "candidate input is not a commit object"
    PEELED_SHA="$(git rev-parse "$INPUT^{commit}")"
    [[ "$PEELED_SHA" == "$EXPECTED_SHA" ]] || die "candidate input does not match expected SHA"
elif [[ "$MODE" == "tag" ]]; then
    [[ "$INPUT" == v1.0.1 ]] || die "final tag input must be exactly v1.0.1"
    [[ "$(git cat-file -t "$INPUT" 2>/dev/null)" == tag ]] || die "v1.0.1 is not an annotated tag"
    TAG_OBJECT_SHA="$(git rev-parse "$INPUT^{tag}")"
    PEELED_SHA="$(git rev-parse "$INPUT^{commit}")"
    [[ "$PEELED_SHA" == "$EXPECTED_SHA" ]] || die "tag peeled commit does not match expected SHA"
    TAGGER_NAME="$(git for-each-ref --format='%(taggername)' "refs/tags/${INPUT}")"
    TAGGER_EMAIL="$(git for-each-ref --format='%(taggeremail)' "refs/tags/${INPUT}")"
    TAGGER_TIMESTAMP="$(git for-each-ref --format='%(taggerdate:iso8601-strict)' "refs/tags/${INPUT}")"
    if command -v sha256sum >/dev/null 2>&1; then
        TAG_OBJECT_CONTENT_SHA="$(git cat-file tag "$INPUT" | sha256sum | awk '{print $1}')"
    else
        TAG_OBJECT_CONTENT_SHA="$(git cat-file tag "$INPUT" | shasum -a 256 | awk '{print $1}')"
    fi
else
    usage
fi

HEAD_SHA="$(git rev-parse HEAD)"
[[ "$HEAD_SHA" == "$EXPECTED_SHA" ]] || die "checked-out HEAD does not match candidate SHA"

if [[ -n "$OUTPUT" ]]; then
    jq -n \
        --arg mode "$MODE" --arg ref_input "$INPUT" --arg candidate_sha "$EXPECTED_SHA" \
        --arg tag_object_sha "${TAG_OBJECT_SHA:-}" --arg peeled_commit_sha "$PEELED_SHA" \
        --arg tagger_name "$TAGGER_NAME" --arg tagger_email "$TAGGER_EMAIL" \
        --arg tagger_timestamp "$TAGGER_TIMESTAMP" --arg tag_object_content_sha "$TAG_OBJECT_CONTENT_SHA" \
        --arg head_sha "$HEAD_SHA" \
        '{mode:$mode,ref_input:$ref_input,candidate_sha:$candidate_sha,
          tag_object_sha:(if $tag_object_sha == "" then null else $tag_object_sha end),
          peeled_commit_sha:$peeled_commit_sha,head_sha:$head_sha,
          tagger_name:(if $tagger_name == "" then null else $tagger_name end),
          tagger_email:(if $tagger_email == "" then null else $tagger_email end),
          tagger_timestamp:(if $tagger_timestamp == "" then null else $tagger_timestamp end),
          tag_object_content_sha256:(if $tag_object_content_sha == "" then null else $tag_object_content_sha end)}' >"$OUTPUT"
else
    printf 'candidate_sha=%s\n' "$EXPECTED_SHA"
    printf 'peeled_commit_sha=%s\n' "$PEELED_SHA"
    if [[ -n "$TAG_OBJECT_SHA" ]]; then
        printf 'tag_object_sha=%s\n' "$TAG_OBJECT_SHA"
        printf 'tagger_name=%s\ntagger_email=%s\ntagger_timestamp=%s\ntag_object_content_sha256=%s\n' \
            "$TAGGER_NAME" "$TAGGER_EMAIL" "$TAGGER_TIMESTAMP" "$TAG_OBJECT_CONTENT_SHA"
    fi
fi
