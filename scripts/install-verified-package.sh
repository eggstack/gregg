#!/usr/bin/env bash
# Verify a .crate, unpack it, and install that exact package with --locked.
set -euo pipefail

MANIFEST=""
PACKAGE=""
ARCHIVE=""
ROOT=""
VERSION=""
CANDIDATE_SHA=""
WORK_DIR=""

usage() {
    echo "usage: $0 --manifest PATH --package NAME --archive PATH --version VERSION --candidate-sha SHA --root PATH" >&2
    exit 2
}

while (($# > 0)); do
    case "$1" in
        --manifest|--package|--archive|--version|--candidate-sha|--root)
            [[ $# -ge 2 ]] || usage
            case "$1" in
                --manifest) MANIFEST="$2" ;; --package) PACKAGE="$2" ;;
                --archive) ARCHIVE="$2" ;; --version) VERSION="$2" ;;
                --candidate-sha) CANDIDATE_SHA="$2" ;; --root) ROOT="$2" ;;
            esac
            shift 2 ;;
        *) usage ;;
    esac
done

[[ -f "$MANIFEST" && -f "$ARCHIVE" && -n "$PACKAGE" && -n "$VERSION" && -n "$CANDIDATE_SHA" && -n "$ROOT" ]] || usage
[[ "$CANDIDATE_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "FATAL: candidate SHA must be a lowercase full 40-character SHA" >&2; exit 1; }

manifest_sha="$(jq -er '.candidate_sha' "$MANIFEST")"
manifest_version="$(jq -er '.release_version' "$MANIFEST")"
[[ "$manifest_sha" == "$CANDIDATE_SHA" ]] || { echo "FATAL: package provenance candidate SHA mismatch" >&2; exit 1; }
[[ "$manifest_version" == "$VERSION" ]] || { echo "FATAL: package provenance release version mismatch" >&2; exit 1; }

file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}
file_size() {
    if stat -c '%s' "$1" >/dev/null 2>&1; then stat -c '%s' "$1"; else stat -f '%z' "$1"; fi
}

expected_sha="$(jq -er --arg package "$PACKAGE" '.packages[$package].sha256' "$MANIFEST")"
expected_size="$(jq -er --arg package "$PACKAGE" '.packages[$package].size_bytes' "$MANIFEST")"
if command -v sha256sum >/dev/null 2>&1; then actual_sha="$(sha256sum "$ARCHIVE" | awk '{print $1}')"; else actual_sha="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"; fi
if stat -c '%s' "$ARCHIVE" >/dev/null 2>&1; then actual_size="$(stat -c '%s' "$ARCHIVE")"; else actual_size="$(stat -f '%z' "$ARCHIVE")"; fi
[[ "$actual_sha" == "$expected_sha" ]] || { echo "FATAL: archive checksum mismatch" >&2; exit 1; }
[[ "$actual_size" == "$expected_size" ]] || { echo "FATAL: archive size mismatch" >&2; exit 1; }

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
tar -xzf "$ARCHIVE" -C "$WORK_DIR"
PACKAGE_DIR="$WORK_DIR/${PACKAGE}-${VERSION}"
[[ -f "$PACKAGE_DIR/Cargo.toml" ]] || { echo "FATAL: unpacked package directory is invalid" >&2; exit 1; }
rm -rf "$ROOT"
mkdir -p "$ROOT"
# Cargo packages do not necessarily carry the workspace lockfile.  Generate
# the lockfile in the unpacked package before the locked install so the exact
# dependency resolution used for verification is preserved in the evidence.
if [[ ! -f "$PACKAGE_DIR/Cargo.lock" ]]; then
    cargo generate-lockfile --manifest-path "$PACKAGE_DIR/Cargo.toml"
fi
cargo install --path "$PACKAGE_DIR" --locked --root "$ROOT" \
    | tee "$WORK_DIR/cargo-install.log"
BINARY="$ROOT/bin/$PACKAGE"
[[ -x "$BINARY" ]] || { echo "FATAL: package install did not produce $BINARY" >&2; exit 1; }
version_output="$("$BINARY" --version 2>&1)"
grep -Eq "(^|[[:space:]])${PACKAGE}([[:space:]]|-)${VERSION}([[:space:]]|$)" <<<"$version_output" || {
    echo "FATAL: installed binary reports unexpected version: $version_output" >&2
    exit 1
}
file_sha256 "$BINARY"
file_size "$BINARY"
printf '%s\n' "$BINARY"
