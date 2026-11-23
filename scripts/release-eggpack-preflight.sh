#!/usr/bin/env bash
# Validate Gregg's release identity and contract with the pinned Eggpack CLI.
# Generated plans and the temporary Eggpack checkout are never placed in dist/.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: bash scripts/release-eggpack-preflight.sh <tag> <version> <source-revision>" >&2
  exit 2
fi
RELEASE_TAG="$1"
RELEASE_VERSION="$2"
SOURCE_REVISION="$3"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [[ "$RELEASE_TAG" != "v$RELEASE_VERSION" ]]; then
  echo "error: Eggpack release tag $RELEASE_TAG does not match version $RELEASE_VERSION" >&2
  exit 1
fi
case "$SOURCE_REVISION" in
  *[!0-9a-f]*|'') echo "error: source revision must be 40 lowercase hexadecimal characters" >&2; exit 1 ;;
esac
if [[ ${#SOURCE_REVISION} -ne 40 ]]; then
  echo "error: source revision must be 40 lowercase hexadecimal characters" >&2
  exit 1
fi
WORKSPACE_VERSION="$(awk '
  /^\[workspace\.package\][[:space:]]*$/ { in_package=1; next }
  in_package && /^\[/ { exit }
  in_package && /^[[:space:]]*version[[:space:]]*=/ {
    if (match($0, /"[^"]+"/)) {
      print substr($0, RSTART + 1, RLENGTH - 2)
      exit
    }
  }
' "$REPO_ROOT/Cargo.toml")"
if [[ -z "$WORKSPACE_VERSION" || "$WORKSPACE_VERSION" != "$RELEASE_VERSION" ]]; then
  echo "error: Eggpack version $RELEASE_VERSION does not match workspace version ${WORKSPACE_VERSION:-<missing>}" >&2
  exit 1
fi
HEAD_REVISION="$(git -C "$REPO_ROOT" rev-parse HEAD)"
if [[ "$HEAD_REVISION" != "$SOURCE_REVISION" ]]; then
  echo "error: source revision $SOURCE_REVISION does not match checked-out HEAD $HEAD_REVISION" >&2
  exit 1
fi
TAG_REVISION="$(git -C "$REPO_ROOT" rev-list -n 1 "$RELEASE_TAG" 2>/dev/null || true)"
if [[ -z "$TAG_REVISION" || "$TAG_REVISION" != "$SOURCE_REVISION" ]]; then
  echo "error: release tag $RELEASE_TAG does not resolve to checked-out source revision $SOURCE_REVISION" >&2
  exit 1
fi

TARGETS_FILE="$REPO_ROOT/scripts/release-targets.txt"
if [[ ! -f "$TARGETS_FILE" ]]; then
  echo "error: canonical target table not found: $TARGETS_FILE" >&2
  exit 1
fi
SELECTED_TARGETS="$(awk '
  { sub(/#.*/, "", $0) }
  NF { if (count++) printf ","; printf "%s", $1 }
  END { if (count) printf "\n" }
' "$TARGETS_FILE")"
if [[ -z "$SELECTED_TARGETS" ]]; then
  echo "error: canonical target table is empty" >&2
  exit 1
fi

WORK_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/gregg-eggpack-preflight.XXXXXX")"
trap 'rm -rf "$WORK_ROOT"' EXIT
EGGPACK="$WORK_ROOT/eggpack"
bash "$SCRIPT_DIR/build-pinned-eggpack.sh" "$EGGPACK"
mkdir -p "$WORK_ROOT/plans"
for program in gregg greggd; do
  contract="$REPO_ROOT/packaging/eggpack/distribution-$program.toml"
  build_bindings="$REPO_ROOT/packaging/eggpack/build-bindings-$program.toml"
  qualification_bindings="$REPO_ROOT/packaging/eggpack/qualification-bindings-$program.toml"
  common=(
    --contract "$contract"
    --pack-config "$REPO_ROOT/packaging/eggpack/pack-config.toml"
    --build-bindings "$build_bindings"
    --qualification-bindings "$qualification_bindings"
    --selected "$SELECTED_TARGETS"
    --release-id "$RELEASE_TAG"
    --identity-mode v_prefixed_stable_semver
    --source-revision "$SOURCE_REVISION"
  )
  "$EGGPACK" check "${common[@]}"
  "$EGGPACK" plan "${common[@]}" --output "$WORK_ROOT/plans/$program-release-plan.json"
done

echo "Eggpack release contract/planning preflight OK for $RELEASE_TAG at $SOURCE_REVISION"
