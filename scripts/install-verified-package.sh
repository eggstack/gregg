#!/usr/bin/env bash
# Verify a .crate, unpack it, and install that exact package with --locked.
set -euo pipefail

MANIFEST=""
PACKAGE=""
ARCHIVE=""
ROOT=""
VERSION=""
CANDIDATE_SHA=""
LOCKFILE=""
WORK_DIR=""

usage() {
    echo "usage: $0 --manifest PATH --package NAME --archive PATH --version VERSION --candidate-sha SHA --root PATH --lockfile PATH" >&2
    exit 2
}

while (($# > 0)); do
    case "$1" in
        --manifest|--package|--archive|--version|--candidate-sha|--root|--lockfile)
            [[ $# -ge 2 ]] || usage
            case "$1" in
                --manifest) MANIFEST="$2" ;; --package) PACKAGE="$2" ;;
                --archive) ARCHIVE="$2" ;; --version) VERSION="$2" ;;
                --candidate-sha) CANDIDATE_SHA="$2" ;; --root) ROOT="$2" ;; --lockfile) LOCKFILE="$2" ;;
            esac
            shift 2 ;;
        *) usage ;;
    esac
done

[[ -f "$MANIFEST" && -f "$ARCHIVE" && -n "$PACKAGE" && -n "$VERSION" && -n "$CANDIDATE_SHA" && -n "$ROOT" && -f "$LOCKFILE" ]] || usage
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

# Verify the supplied lockfile checksum and size against provenance.
lockfile_field="$(jq -r --arg package "$PACKAGE" '.packages[$package].verification_lockfile // empty' "$MANIFEST" 2>/dev/null || true)"
if [[ -z "$lockfile_field" ]]; then
    echo "FATAL: package provenance has no verification_lockfile for ${PACKAGE}" >&2
    exit 1
fi
expected_lockfile_sha="$(jq -er --arg package "$PACKAGE" '.packages[$package].verification_lockfile_sha256' "$MANIFEST")"
expected_lockfile_size="$(jq -er --arg package "$PACKAGE" '.packages[$package].verification_lockfile_size_bytes' "$MANIFEST")"
actual_lockfile_sha="$(file_sha256 "$LOCKFILE")"
actual_lockfile_size="$(file_size "$LOCKFILE")"
[[ "$actual_lockfile_sha" == "$expected_lockfile_sha" ]] || { echo "FATAL: lockfile checksum mismatch" >&2; exit 1; }
[[ "$actual_lockfile_size" == "$expected_lockfile_size" ]] || { echo "FATAL: lockfile size mismatch" >&2; exit 1; }

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
tar -xzf "$ARCHIVE" -C "$WORK_DIR"
PACKAGE_DIR="$WORK_DIR/${PACKAGE}-${VERSION}"
[[ -f "$PACKAGE_DIR/Cargo.toml" ]] || { echo "FATAL: unpacked package directory is invalid" >&2; exit 1; }

# Reject an unexpected existing Cargo.lock unless it is byte-identical.
if [[ -f "$PACKAGE_DIR/Cargo.lock" ]]; then
    existing_sha="$(file_sha256 "$PACKAGE_DIR/Cargo.lock")"
    if [[ "$existing_sha" != "$actual_lockfile_sha" ]]; then
        echo "FATAL: unpacked package has a divergent Cargo.lock; refusing to generate a replacement lockfile" >&2
        exit 1
    fi
fi

# Copy the verified lockfile into the unpacked package root.
cp "$LOCKFILE" "$PACKAGE_DIR/Cargo.lock"

# Re-verify the copied lockfile checksum.
copied_sha="$(file_sha256 "$PACKAGE_DIR/Cargo.lock")"
[[ "$copied_sha" == "$actual_lockfile_sha" ]] || { echo "FATAL: copied lockfile checksum mismatch" >&2; exit 1; }

rm -rf "$ROOT"
mkdir -p "$ROOT"
cargo install --path "$PACKAGE_DIR" --locked --root "$ROOT" \
    | tee "$WORK_DIR/cargo-install.log"
BINARY="$ROOT/bin/$PACKAGE"
[[ -x "$BINARY" ]] || { echo "FATAL: package install did not produce $BINARY" >&2; exit 1; }
version_output="$("$BINARY" --version 2>&1)"
grep -Eq "(^|[[:space:]])${PACKAGE}([[:space:]]|-)${VERSION}([[:space:]]|$)" <<<"$version_output" || {
    echo "FATAL: installed binary reports unexpected version: $version_output" >&2;
    exit 1
}
binary_sha="$(file_sha256 "$BINARY")"
binary_size="$(file_size "$BINARY")"

# Output a machine-readable installation record containing archive, lockfile, and binary identities.
INSTALL_RECORD="$WORK_DIR/install-record.json"
jq -n \
    --arg package "$PACKAGE" --arg archive "$ARCHIVE" --arg binary "$BINARY" \
    --arg lockfile "$LOCKFILE" \
    --arg archive_sha "$actual_sha" --arg binary_sha "$binary_sha" --arg lockfile_sha "$actual_lockfile_sha" \
    --argjson archive_size "$actual_size" --argjson binary_size "$binary_size" --argjson lockfile_size "$actual_lockfile_size" \
    '{package:$package,archive:$archive,archive_sha256:$archive_sha,archive_size_bytes:$archive_size,
      lockfile:$lockfile,lockfile_sha256:$lockfile_sha,lockfile_size_bytes:$lockfile_size,
      binary:$binary,binary_sha256:$binary_sha,binary_size_bytes:$binary_size}' >"$INSTALL_RECORD"

# Retain the install transcript and record outside the temporary directory.
INSTALL_RECORD_OUT="$(dirname "$ROOT")/$(basename "$ROOT")-install-record.json"
cp "$INSTALL_RECORD" "$INSTALL_RECORD_OUT"
cat "$WORK_DIR/cargo-install.log" >&2

printf '%s\n' "$BINARY"
printf '%s\n' "$INSTALL_RECORD_OUT"
