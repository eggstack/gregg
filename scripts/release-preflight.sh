#!/usr/bin/env bash
# Release preflight: version consistency, tag identity, tree cleanliness,
# and (optionally) crates.io visibility for gregg/greggd.
#
# Used by .github/workflows/release-binaries.yml and runnable locally:
#
#   bash scripts/release-preflight.sh                        # version checks only
#   bash scripts/release-preflight.sh --tag v1.0.12          # + tag/HEAD/tree checks
#   bash scripts/release-preflight.sh --tag v1.0.12 --check-registry
#
# The workflow passes the triggering tag (or manual-dispatch input) via
# --github-ref/--input-tag so tag derivation lives here, not in YAML.
#
# Fails loudly on the first inconsistency; prints actionable diagnostics.
set -euo pipefail

TAG=""
GITHUB_REF=""
INPUT_TAG=""
CHECK_REGISTRY=0
SKIP_GIT_CHECKS=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag) TAG="${2:-}"; shift 2 ;;
    --github-ref) GITHUB_REF="${2:-}"; shift 2 ;;
    --input-tag) INPUT_TAG="${2:-}"; shift 2 ;;
    --check-registry) CHECK_REGISTRY=1; shift ;;
    --skip-git-checks) SKIP_GIT_CHECKS=1; shift ;;
    -h|--help)
      sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "error: unknown argument '$1'" >&2; exit 1 ;;
  esac
done

# Derive the tag: explicit --tag wins, then dispatch input, then the ref.
if [[ -z "$TAG" ]]; then
  if [[ -n "$INPUT_TAG" ]]; then
    TAG="$INPUT_TAG"
  elif [[ -n "$GITHUB_REF" ]]; then
    TAG="${GITHUB_REF#refs/tags/}"
  fi
fi

# Read workspace version from Cargo.toml [workspace.package].
WORKSPACE_VERSION="$(awk '
  /^\[workspace\.package\][[:space:]]*$/ { in_package=1; next }
  in_package && /^\[/ { exit }
  in_package && /^[[:space:]]*version[[:space:]]*=/ {
    if (match($0, /"[^"]+"/)) {
      print substr($0, RSTART + 1, RLENGTH - 2)
      exit
    }
  }
' Cargo.toml)"
if [[ -z "$WORKSPACE_VERSION" ]]; then
  echo "error: could not read [workspace.package].version from Cargo.toml" >&2
  exit 1
fi
echo "Workspace version: $WORKSPACE_VERSION"

VERSION="$WORKSPACE_VERSION"

if [[ -n "$TAG" ]]; then
  if [[ "$TAG" != v* ]]; then
    echo "error: tag must start with 'v' (got '$TAG')" >&2
    exit 1
  fi
  VERSION="${TAG#v}"
  echo "Derived TAG=$TAG VERSION=$VERSION"
  if [[ "$VERSION" != "$WORKSPACE_VERSION" ]]; then
    echo "error: tag version v$VERSION does not match workspace version $WORKSPACE_VERSION" >&2
    exit 1
  fi
fi

# Verify every member manifests inherit the workspace version.
for crate in crates/gregg-protocol crates/gregg-update crates/greggd crates/gregg; do
  manifest="$crate/Cargo.toml"
  if ! grep -Eq '^[[:space:]]*version\.workspace[[:space:]]*=[[:space:]]*true[[:space:]]*$' "$manifest"; then
    echo "error: $manifest missing version.workspace = true" >&2
    exit 1
  fi
done

# Verify inter-crate dependency versions match workspace version.
# Publication order is gregg-protocol -> gregg-update -> greggd -> gregg,
# so greggd/gregg must pin both internal dependencies exactly.
for dep in gregg-protocol gregg-update; do
  for crate in crates/greggd crates/gregg; do
    manifest="$crate/Cargo.toml"
    dep_version="$(grep -E "^[[:space:]]*${dep}[[:space:]]*=" "$manifest" | head -1 | sed -E 's/.*version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')"
    if [[ -z "$dep_version" ]]; then
      echo "error: $manifest has no $dep version" >&2
      exit 1
    fi
    dep_stripped="${dep_version#=}"
    if [[ "$dep_stripped" != "$WORKSPACE_VERSION" ]]; then
      echo "error: $manifest $dep dependency $dep_version != workspace $WORKSPACE_VERSION" >&2
      exit 1
    fi
  done
done
echo "Version consistency OK: $VERSION"

# Export tag/version for the GitHub Actions preflight job outputs when
# running inside a workflow step.
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  echo "tag=${TAG:-}" >> "$GITHUB_OUTPUT"
  echo "version=$VERSION" >> "$GITHUB_OUTPUT"
fi

if [[ -n "$TAG" && "$SKIP_GIT_CHECKS" -eq 0 ]]; then
  HEAD_SHA="$(git rev-parse HEAD)"
  TAG_SHA="$(git rev-list -n 1 "$TAG" 2>/dev/null || echo "")"
  if [[ -z "$TAG_SHA" ]]; then
    echo "error: tag $TAG does not exist locally" >&2
    exit 1
  fi
  if [[ "$HEAD_SHA" != "$TAG_SHA" ]]; then
    echo "error: tag $TAG points at $TAG_SHA but HEAD is $HEAD_SHA" >&2
    echo "The workflow must run on the exact tagged commit." >&2
    exit 1
  fi
  echo "Tag $TAG correctly points at HEAD $HEAD_SHA"
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "error: working tree is not clean after checkout of $TAG" >&2
    git status --short
    exit 1
  fi
  echo "Clean tree verified"
fi

if [[ "$CHECK_REGISTRY" -eq 1 ]]; then
  # The release sequence publishes crates before the tag. A rerun must
  # remain safe. If the registry has not yet indexed the version, fail
  # clearly so the maintainer can rerun after indexing.
  for crate in gregg-protocol gregg-update gregg greggd; do
    echo "Checking crates.io for $crate $VERSION..."
    # crates.io API: https://crates.io/api/v1/crates/<name>/<version>
    HTTP_CODE="$(curl -s -o /tmp/crate.json -w "%{http_code}" \
      -H "User-Agent: gregg-release-ci (eggstack/gregg)" \
      "https://crates.io/api/v1/crates/${crate}/${VERSION}" || true)"
    if [[ "$HTTP_CODE" == "200" ]]; then
      echo "  $crate $VERSION is visible on crates.io"
    elif [[ "$HTTP_CODE" == "404" ]]; then
      echo "error: $crate $VERSION is not yet visible on crates.io (HTTP 404)" >&2
      echo "The release workflow requires the workspace version to be" >&2
      echo "published to crates.io before building GitHub binaries." >&2
      echo "Wait for indexing, then rerun this workflow." >&2
      cat /tmp/crate.json 2>/dev/null || true
      exit 1
    else
      echo "warning: crates.io check for $crate returned HTTP $HTTP_CODE; treating as hard failure" >&2
      cat /tmp/crate.json 2>/dev/null || true
      exit 1
    fi
  done
fi

echo "Release preflight OK"
