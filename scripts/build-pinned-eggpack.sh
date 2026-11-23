#!/usr/bin/env bash
# Build eggpack-cli from the single reviewed immutable revision in packaging/eggpack/revision.txt.
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: bash scripts/build-pinned-eggpack.sh <output-binary-path>" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
REVISION_FILE="$REPO_ROOT/packaging/eggpack/revision.txt"
if [[ ! -f "$REVISION_FILE" ]]; then
  echo "error: pinned Eggpack revision file not found: $REVISION_FILE" >&2
  exit 1
fi
REVISION="$(cat "$REVISION_FILE")"
case "$REVISION" in
  *[!0-9a-f]*|'') echo "error: Eggpack revision must be 40 lowercase hexadecimal characters" >&2; exit 1 ;;
esac
if [[ ${#REVISION} -ne 40 ]]; then
  echo "error: Eggpack revision must be 40 lowercase hexadecimal characters" >&2
  exit 1
fi

command -v git >/dev/null 2>&1 || { echo "error: git is required to fetch pinned Eggpack" >&2; exit 1; }
command -v rustup >/dev/null 2>&1 || { echo "error: rustup is required to build pinned Eggpack" >&2; exit 1; }
if ! rustup run 1.89.0 rustc --version >/dev/null 2>&1; then
  echo "error: Rust 1.89.0 is required (install it with rustup toolchain install 1.89.0)" >&2
  exit 1
fi

OUTPUT="$1"
case "$OUTPUT" in
  /*) ;;
  *) OUTPUT="$PWD/$OUTPUT" ;;
esac
OUTPUT_DIR="$(dirname "$OUTPUT")"
mkdir -p "$OUTPUT_DIR"

BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/eggpack-build.XXXXXX")"
trap 'rm -rf "$BUILD_ROOT"' EXIT
SOURCE="$BUILD_ROOT/source"
mkdir -p "$SOURCE"
git -C "$SOURCE" init -q
git -C "$SOURCE" remote add origin https://github.com/eggstack/eggpack.git
git -C "$SOURCE" fetch --quiet --depth=1 --no-tags origin "$REVISION"
git -C "$SOURCE" checkout --quiet --detach FETCH_HEAD
FETCHED_REVISION="$(git -C "$SOURCE" rev-parse HEAD)"
if [[ "$FETCHED_REVISION" != "$REVISION" ]]; then
  echo "error: fetched Eggpack revision $FETCHED_REVISION does not match pin $REVISION" >&2
  exit 1
fi

CARGO_TARGET_DIR="$BUILD_ROOT/cargo-target" rustup run 1.89.0 cargo build \
  --locked --release --manifest-path "$SOURCE/Cargo.toml" -p eggpack-cli
BUILT_CLI="$BUILD_ROOT/cargo-target/release/eggpack"
if [[ ! -x "$BUILT_CLI" ]]; then
  echo "error: pinned Eggpack CLI build did not produce an executable" >&2
  exit 1
fi
cp "$BUILT_CLI" "$OUTPUT"
chmod +x "$OUTPUT"
"$OUTPUT" --version
