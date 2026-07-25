#!/usr/bin/env bash
# Verify a package archive and the executable installed from it.
set -euo pipefail

MANIFEST=""
PACKAGE=""
ARCHIVE=""
BINARY=""
VERSION=""
CANDIDATE_SHA=""
OUTPUT=""
ALLOW_PLATFORM_BINARY=0

usage() {
    echo "usage: $0 --manifest PATH --package NAME --archive PATH --binary PATH --version VERSION --candidate-sha SHA [--allow-platform-binary] [--output PATH]" >&2
    exit 2
}

while (($# > 0)); do
    case "$1" in
        --manifest|--package|--archive|--binary|--version|--candidate-sha|--output)
            [[ $# -ge 2 ]] || usage
            case "$1" in
                --manifest) MANIFEST="$2" ;; --package) PACKAGE="$2" ;;
                --archive) ARCHIVE="$2" ;; --binary) BINARY="$2" ;;
                --version) VERSION="$2" ;; --candidate-sha) CANDIDATE_SHA="$2" ;; --output) OUTPUT="$2" ;;
            esac
            shift 2 ;;
        --allow-platform-binary)
            ALLOW_PLATFORM_BINARY=1
            shift ;;
        *) usage ;;
    esac
done

[[ -f "$MANIFEST" && -f "$ARCHIVE" && -x "$BINARY" ]] || {
    echo "FATAL: manifest, archive, and executable binary are required" >&2
    exit 1
}
[[ -n "$PACKAGE" && -n "$VERSION" ]] || usage
[[ "$CANDIDATE_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "FATAL: candidate SHA must be a lowercase full 40-character SHA" >&2; exit 1; }

file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}
file_size() {
    if stat -c '%s' "$1" >/dev/null 2>&1; then stat -c '%s' "$1"; else stat -f '%z' "$1"; fi
}

expected_sha="$(jq -er --arg package "$PACKAGE" '.packages[$package].sha256' "$MANIFEST")"
expected_size="$(jq -er --arg package "$PACKAGE" '.packages[$package].size_bytes' "$MANIFEST")"
manifest_sha="$(jq -er '.candidate_sha' "$MANIFEST")"
manifest_version="$(jq -er '.release_version' "$MANIFEST")"
[[ "$manifest_sha" == "$CANDIDATE_SHA" ]] || { echo "FATAL: package provenance candidate SHA mismatch" >&2; exit 1; }
[[ "$manifest_version" == "$VERSION" ]] || { echo "FATAL: package provenance release version mismatch" >&2; exit 1; }
actual_sha="$(file_sha256 "$ARCHIVE")"
actual_size="$(file_size "$ARCHIVE")"
[[ "$actual_sha" == "$expected_sha" ]] || { echo "FATAL: archive checksum mismatch" >&2; exit 1; }
[[ "$actual_size" == "$expected_size" ]] || { echo "FATAL: archive size mismatch" >&2; exit 1; }

binary_version="$("$BINARY" --version 2>&1)"
grep -Eq "(^|[[:space:]])${PACKAGE}([[:space:]]|-)${VERSION}([[:space:]]|$)" <<<"$binary_version" || {
    echo "FATAL: binary version does not report ${PACKAGE} ${VERSION}: ${binary_version}" >&2
    exit 1
}
binary_sha="$(file_sha256 "$BINARY")"
binary_size="$(file_size "$BINARY")"
manifest_binary_sha="$(jq -er --arg package "$PACKAGE" '.packages[$package].installed_binary_sha256 // empty' "$MANIFEST")"
binary_verification="checksum-and-version"
if ((ALLOW_PLATFORM_BINARY == 1)); then
    binary_verification="version-and-archive"
elif [[ -n "$manifest_binary_sha" ]]; then
    [[ "$binary_sha" == "$manifest_binary_sha" ]] || { echo "FATAL: installed binary checksum mismatch" >&2; exit 1; }
else
    echo "FATAL: provenance has no installed binary checksum; use --allow-platform-binary only for a native package install" >&2
    exit 1
fi

if [[ -n "$OUTPUT" ]]; then
    jq -n --arg package "$PACKAGE" --arg archive "$ARCHIVE" --arg binary "$BINARY" \
        --arg archive_sha "$actual_sha" --arg binary_sha "$binary_sha" \
        --arg binary_verification "$binary_verification" \
        --argjson archive_size "$actual_size" --argjson binary_size "$binary_size" \
        '{package:$package,archive:$archive,archive_sha256:$archive_sha,archive_size_bytes:$archive_size,
          binary:$binary,binary_sha256:$binary_sha,binary_size_bytes:$binary_size,
          binary_verification:$binary_verification}' >"$OUTPUT"
else
    printf 'package=%s\narchive_sha256=%s\nbinary_sha256=%s\nbinary_verification=%s\n' "$PACKAGE" "$actual_sha" "$binary_sha" "$binary_verification"
fi
