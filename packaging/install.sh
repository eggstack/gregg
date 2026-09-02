#!/usr/bin/env bash
# install.sh — bootstrap installer for prebuilt gregg/greggd binaries.
#
# Binary-first, Cargo fallback second. Downloads the matching GitHub Release
# asset for the current OS/architecture, verifies SHA-256 and candidate
# version, then installs to /usr/local/bin (when root) or $HOME/.local/bin.
# If no matching asset exists (HTTP 404 or intentionally source-only host such
# as ARMv7) and Cargo is available, falls back to `cargo install`.
#
# Usage:
#   curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
#   curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
#   curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
#   ./packaging/install.sh [--version X.Y.Z] gregg|greggd|both
#
# Component selection is required. When invoked without arguments through a
# pipe/noninteractive shell, usage is printed and the script exits nonzero
# rather than guessing. An interactive TTY may present a tiny selector.
# No installer code silently invokes sudo.
#
# Linux glibc floor for published GNU assets is 2.17 (cargo-zigbuild + Zig).

set -euo pipefail

REPO="eggstack/gregg"
BASE_URL="https://github.com/${REPO}/releases"

VERSION=""
COMPONENT=""

usage() {
  cat <<'EOF'
usage: install.sh [--version X.Y.Z] gregg|greggd|both

Install prebuilt gregg and/or greggd binaries for the current OS/architecture.
Downloads the matching GitHub Release asset (latest or pinned version), verifies
SHA-256 and candidate version, and installs to /usr/local/bin (root) or
$HOME/.local/bin (non-root). If no asset exists for the host and Cargo is
available, falls back to `cargo install --locked`.

arguments:
  gregg          install only the client
  greggd         install only the daemon
  both           install both

options:
  --version X.Y.Z  install pinned release vX.Y.Z instead of latest
  --help, -h       show this help

examples:
  curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
  curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
  curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
  ./packaging/install.sh --version 1.0.11 gregg

artifact naming (stable, no version in filename):
  gregg-x86_64-unknown-linux-gnu        linux x86_64
  greggd-x86_64-unknown-linux-gnu
  gregg-aarch64-unknown-linux-gnu       linux aarch64 (64-bit Raspberry Pi, Le Potato, etc.)
  greggd-aarch64-unknown-linux-gnu
  gregg-x86_64-apple-darwin             macOS Intel
  greggd-x86_64-apple-darwin
  gregg-aarch64-apple-darwin            macOS Apple Silicon
  greggd-aarch64-apple-darwin

Linux ARMv7 (armv7l) is source-build only and uses Cargo fallback when available.
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

# --- argument parsing ------------------------------------------------------

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      if [[ $# -lt 2 || -z "${2:-}" ]]; then
        die "--version requires an argument (X.Y.Z)"
      fi
      VERSION="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    gregg|greggd|both)
      if [[ -n "$COMPONENT" ]]; then
        die "component already specified: $COMPONENT"
      fi
      COMPONENT="$1"
      shift
      ;;
    *)
      die "unknown argument: $1 (expected gregg|greggd|both or --version X.Y.Z)"
      ;;
  esac
done

if [[ -z "$COMPONENT" ]]; then
  if [[ -t 0 && -t 1 ]]; then
    echo "Select component to install:" >&2
    echo "  1) gregg (client)" >&2
    echo "  2) greggd (daemon)" >&2
    echo "  3) both" >&2
    read -rp "Enter choice [1-3]: " choice
    case "$choice" in
      1) COMPONENT="gregg" ;;
      2) COMPONENT="greggd" ;;
      3) COMPONENT="both" ;;
      *) die "invalid choice: $choice" ;;
    esac
  else
    usage >&2
    exit 1
  fi
fi

# Validate version format if pinned
STRIPPED_VERSION=""
TAG=""
if [[ -n "$VERSION" ]]; then
  STRIPPED_VERSION="${VERSION#v}"
  if [[ ! "$STRIPPED_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
    die "--version must be X.Y.Z (got '$VERSION')"
  fi
  TAG="v${STRIPPED_VERSION}"
fi

# --- host mapping ----------------------------------------------------------

OS="$(uname -s 2>/dev/null || echo "unknown")"
ARCH="$(uname -m 2>/dev/null || echo "unknown")"

TARGET=""
case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      armv7l) TARGET="armv7-unknown-linux-gnueabihf" ;;
      *) TARGET="" ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      *) TARGET="" ;;
    esac
    ;;
  *) TARGET="" ;;
esac

SUPPORTED_BINARY=false
case "$TARGET" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin)
    SUPPORTED_BINARY=true
    ;;
  *)
    SUPPORTED_BINARY=false
    ;;
esac

# armv7 is intentionally source-only in this phase (not published)
if [[ "$TARGET" == "armv7-unknown-linux-gnueabihf" ]]; then
  SUPPORTED_BINARY=false
fi

echo "Detected: OS=$OS ARCH=$ARCH TARGET=${TARGET:-unknown} component=$COMPONENT ${TAG:-latest}" >&2

# --- destination and privilege behaviour -----------------------------------

if [[ $EUID -eq 0 ]]; then
  DEST_DIR="/usr/local/bin"
else
  DEST_DIR="${HOME}/.local/bin"
fi

# --- requirement checks ----------------------------------------------------

if ! command -v curl >/dev/null 2>&1; then
  die "curl is required but not found; install curl and rerun"
fi

# --- helpers ---------------------------------------------------------------

check_path_advice() {
  case ":${PATH:-}:" in
    *":${DEST_DIR}:"*) ;;
    *)
      echo "note: ${DEST_DIR} is not in PATH; add it to your shell profile:" >&2
      echo "  export PATH=\"\$HOME/.local/bin:\$PATH\"" >&2
      ;;
  esac
}

verify_checksum() {
  local file="$1"
  local sha_file="$2"
  local expected actual
  expected="$(awk '{print $1}' "$sha_file" 2>/dev/null || true)"
  if [[ -z "$expected" ]]; then
    die "checksum file is empty or unreadable: $sha_file"
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    die "no sha256 tool found (need sha256sum or shasum)"
  fi
  if [[ "$expected" != "$actual" ]]; then
    echo "expected: $expected" >&2
    echo "actual:   $actual" >&2
    die "SHA-256 mismatch for $(basename "$file")"
  fi
}

verify_candidate_version() {
  local candidate="$1"
  local program="$2"
  local output version_part
  if ! output="$("$candidate" version 2>&1)"; then
    die "candidate $program failed 'version' check: $output"
  fi
  # Expected form: "gregg X.Y.Z" or "greggd X.Y.Z"
  if [[ "$output" != "${program} "* ]]; then
    die "candidate version output does not start with '${program} ': $output"
  fi
  version_part="$(echo "$output" | awk '{print $2}')"
  if [[ -z "$version_part" ]]; then
    die "candidate version output missing version: $output"
  fi
  if [[ -n "$STRIPPED_VERSION" ]]; then
    if [[ "$version_part" != "$STRIPPED_VERSION" ]]; then
      die "candidate version $version_part != requested $STRIPPED_VERSION (output: $output)"
    fi
  fi
  echo "Verified candidate: $output" >&2
}

cargo_fallback() {
  local program="$1"
  if ! command -v cargo >/dev/null 2>&1; then
    echo "No prebuilt $program asset for ${TARGET:-unknown} (${OS}/${ARCH})" >&2
    if [[ "$TARGET" == "armv7-unknown-linux-gnueabihf" ]]; then
      echo "ARMv7 is source-build only in this phase; install Rust to build from source." >&2
    fi
    echo "Install Rust from https://rustup.rs, then rerun:" >&2
    if [[ -n "$STRIPPED_VERSION" ]]; then
      echo "  cargo install $program --version \"=${STRIPPED_VERSION}\" --locked" >&2
    else
      echo "  cargo install $program --locked" >&2
    fi
    echo "Or download a matching asset manually from https://github.com/${REPO}/releases" >&2
    exit 1
  fi

  local cargo_root
  if [[ "$DEST_DIR" == "/usr/local/bin" ]]; then
    cargo_root="/usr/local"
  else
    cargo_root="${HOME}/.local"
  fi

  local -a args=(install --locked)
  if [[ -n "$STRIPPED_VERSION" ]]; then
    args+=(--version "=${STRIPPED_VERSION}")
  fi
  args+=(--root "$cargo_root" "$program")

  echo "No prebuilt $program asset for ${TARGET:-unknown}; building from source:" >&2
  echo "  cargo ${args[*]}" >&2

  # cargo install may fail if run as root without cargo in root's PATH; surface clearly
  if ! cargo "${args[@]}"; then
    die "Cargo fallback failed for $program"
  fi

  local installed="${cargo_root}/bin/${program}"
  if [[ ! -x "$installed" ]]; then
    die "cargo install succeeded but $installed not found or not executable"
  fi
  verify_candidate_version "$installed" "$program"
  echo "$program installed via Cargo to $installed" >&2
  if [[ "$DEST_DIR" != "${cargo_root}/bin" ]]; then
    echo "note: Cargo installed to ${cargo_root}/bin; DEST_DIR is $DEST_DIR" >&2
  fi
  check_path_advice
}

install_program() {
  local program="$1"
  local asset url sha_url
  local tmpdir

  # Source-only hosts go directly to Cargo fallback
  if [[ -z "$TARGET" || "$SUPPORTED_BINARY" != "true" ]]; then
    echo "Host ${OS}/${ARCH} (${TARGET:-unknown}) has no prebuilt $program asset; trying Cargo fallback..." >&2
    cargo_fallback "$program"
    return 0
  fi

  asset="${program}-${TARGET}"

  if [[ -n "$TAG" ]]; then
    url="${BASE_URL}/download/${TAG}/${asset}"
  else
    url="${BASE_URL}/latest/download/${asset}"
  fi
  sha_url="${url}.sha256"

  # Require fixed repo host (defense against accidental URL mutation)
  if [[ "$url" != https://github.com/${REPO}/releases/* ]]; then
    die "constructed URL is not under expected repo: $url"
  fi

  tmpdir="$(mktemp -d)"
  # cleanup on success or failure; expand now so trap sees value
  # shellcheck disable=SC2016
  trap 'rm -rf "$tmpdir"' EXIT

  echo "Downloading $program from $url ..." >&2

  # Attempt to download executable; classify 404 as Cargo-fallback trigger, other errors as hard failure
  set +e
  curl -fsSL -o "${tmpdir}/${asset}" "$url"
  local curl_status=$?
  set -e

  if [[ $curl_status -ne 0 ]]; then
    local http_code
    http_code="$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || echo "000")"
    if [[ "$http_code" == "404" ]]; then
      echo "No prebuilt $program asset at $url (HTTP 404); trying Cargo fallback..." >&2
      rm -rf "$tmpdir"
      trap - EXIT
      cargo_fallback "$program"
      return 0
    else
      echo "curl exit $curl_status, HTTP $http_code for $url" >&2
      die "failed to download $asset (HTTP $http_code)"
    fi
  fi

  echo "Downloading checksum ${sha_url} ..." >&2
  if ! curl -fsSL -o "${tmpdir}/${asset}.sha256" "$sha_url"; then
    die "failed to download checksum for $asset from $sha_url"
  fi

  verify_checksum "${tmpdir}/${asset}" "${tmpdir}/${asset}.sha256"

  chmod +x "${tmpdir}/${asset}"

  verify_candidate_version "${tmpdir}/${asset}" "$program"

  # Do not install an unverified partial download — verification already passed

  mkdir -p "$DEST_DIR"
  # Use install(1) when available for atomic mode setting; fall back to cp
  if command -v install >/dev/null 2>&1; then
    install -m 755 "${tmpdir}/${asset}" "${DEST_DIR}/${program}"
  else
    cp "${tmpdir}/${asset}" "${DEST_DIR}/${program}"
    chmod 755 "${DEST_DIR}/${program}"
  fi

  echo "Installed ${program} to ${DEST_DIR}/${program}" >&2

  rm -rf "$tmpdir"
  trap - EXIT

  # Destination advice
  check_path_advice

  # Daemon privilege note (Plan 099/100 boundary): user-local daemon install on a systemd/launchd
  # machine cannot produce the final system deployment; print exact privileged completion instead
  # of silently registering an alternate supervisor.
  if [[ "$program" == "greggd" && $EUID -ne 0 ]]; then
    local has_systemd=false
    local has_launchd=false
    if [[ -d /run/systemd/system ]]; then
      has_systemd=true
    fi
    if [[ "$OS" == "Darwin" ]]; then
      has_launchd=true
    fi
    if [[ "$has_systemd" == "true" || "$has_launchd" == "true" ]]; then
      echo "note: greggd installed to ${DEST_DIR}/greggd (user-local)." >&2
      echo "For a system service, rerun with privilege:" >&2
      if [[ -n "$TAG" ]]; then
        echo "  curl -fsSL https://github.com/${REPO}/releases/download/${TAG}/install.sh | sudo sh -s -- greggd" >&2
      else
        echo "  curl -fsSL https://github.com/${REPO}/releases/latest/download/install.sh | sudo sh -s -- greggd" >&2
      fi
      echo "No service was registered; Plan 100 will refine startup registration." >&2
    fi
  fi
}

# --- main ------------------------------------------------------------------

# Ensure non-root daemon install does not silently create a cron-managed duplicate
# simply because systemd registration needs root — that check is above.

case "$COMPONENT" in
  gregg) install_program "gregg" ;;
  greggd) install_program "greggd" ;;
  both)
    install_program "gregg"
    install_program "greggd"
    ;;
  *) die "internal: unknown component $COMPONENT" ;;
esac

echo "Done. Installed $COMPONENT to ${DEST_DIR}" >&2
if [[ "$COMPONENT" == "both" ]]; then
  echo "  ${DEST_DIR}/gregg version  => $( "${DEST_DIR}/gregg" version 2>&1 || echo "not runnable")" >&2
  echo "  ${DEST_DIR}/greggd version => $( "${DEST_DIR}/greggd" version 2>&1 || echo "not runnable")" >&2
elif [[ "$COMPONENT" == "gregg" ]]; then
  echo "  ${DEST_DIR}/gregg version => $( "${DEST_DIR}/gregg" version 2>&1 || echo "not runnable")" >&2
elif [[ "$COMPONENT" == "greggd" ]]; then
  echo "  ${DEST_DIR}/greggd version => $( "${DEST_DIR}/greggd" version 2>&1 || echo "not runnable")" >&2
fi
