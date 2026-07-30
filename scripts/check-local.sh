#!/usr/bin/env bash
# check-local.sh — local validation entry point for gregg.
#
# Tiers:
#   (default)   Fast developer check: fmt, clippy, test, doc, deny.
#   --full      Full local check: adds shellcheck, python tests, package checks.
#   --release   Release preflight: adds clean-tree, version consistency, package list.
#
# Usage:
#   ./scripts/check-local.sh [--full] [--release] [--skip-deny] [--help]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

MODE="default"
SKIP_DENY=0

usage() {
    cat <<'EOF'
Usage: scripts/check-local.sh [OPTIONS]

Options:
  --full       Run the full local check tier (adds shellcheck, python tests,
               package checks, and installed-binary smoke).
  --release    Run the release preflight tier (adds clean-tree, version
               consistency, cargo package list, and publish dry-run).
  --skip-deny  Skip the cargo-deny dependency check.
  --help       Show this help message and exit.

Tiers:
  (default)    fmt, clippy, test, doc, cargo deny, platform-native tests.
  --full       Everything in default plus shellcheck, python tests, package
               content checks, and installed-binary loopback smoke.
  --release    Everything in full plus clean-tree, version consistency,
               cargo package --list, and cargo publish --dry-run.

Examples:
  ./scripts/check-local.sh                  # fast developer check
  ./scripts/check-local.sh --full           # pre-merge full check
  ./scripts/check-local.sh --full --skip-deny
  ./scripts/check-local.sh --release        # release preflight
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --full)
            MODE="full"
            shift
            ;;
        --release)
            MODE="release"
            shift
            ;;
        --skip-deny)
            SKIP_DENY=1
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

cd "${REPO_ROOT}"

FAILED=""
CURRENT_STEP=""

step() {
    CURRENT_STEP="$1"
    echo "==> $1"
}

fail() {
    echo "local check failed: ${CURRENT_STEP}" >&2
    exit 1
}

run_or_fail() {
    "$@" || fail
}

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "macos" ;;
        MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
        *)       echo "unknown" ;;
    esac
}

OS="$(detect_os)"

# ── Tier 1 (default): fast developer check ──────────────────────────────────

step "cargo fmt --all -- --check"
run_or_fail cargo fmt --all -- --check

step "cargo clippy --workspace --all-targets --all-features -- -D warnings"
run_or_fail cargo clippy --workspace --all-targets --all-features -- -D warnings

step "cargo test --workspace --all-targets --all-features"
run_or_fail cargo test --workspace --all-targets --all-features

step "cargo doc --workspace --no-deps"
run_or_fail cargo doc --workspace --no-deps

if [[ "${SKIP_DENY}" -eq 0 ]]; then
    if command -v cargo-deny >/dev/null 2>&1; then
        step "cargo deny check"
        run_or_fail cargo deny check
    else
        echo "cargo-deny not installed, skipping (install with: cargo install cargo-deny)"
    fi
fi

# Platform-native collector tests
case "${OS}" in
    linux)
        step "native Linux collector tests"
        run_or_fail cargo test -p greggd --all-features -- collector::linux
        ;;
    macos)
        step "native macOS collector tests"
        run_or_fail cargo test -p greggd --all-features -- collector::macos::ffi::native_tests
        ;;
    windows)
        step "native Windows collector tests"
        run_or_fail cargo test -p greggd --all-features -- collector::windows
        ;;
    *)
        echo "  skipping platform-native collector tests on ${OS}"
        ;;
esac

# ── Tier 2 (--full): full local check ───────────────────────────────────────

if [[ "${MODE}" == "full" || "${MODE}" == "release" ]]; then

    # Shell syntax checks
    if command -v shellcheck >/dev/null 2>&1; then
        step "shellcheck packaging/install scripts"
        run_or_fail shellcheck packaging/install-linux.sh packaging/install-macos.sh
    else
        echo "shellcheck not installed, skipping"
    fi

    # Python tests
    if command -v python3 >/dev/null 2>&1; then
        step "python3 tests for scripts"
        run_or_fail python3 -m pytest scripts/tests/ -v --tb=short
    else
        echo "python3 not installed, skipping python tests"
    fi

    # Package content check (no publish)
    step "cargo package --list (gregg-protocol)"
    run_or_fail cargo package --list -p gregg-protocol

    step "cargo package --list (greggd)"
    run_or_fail cargo package --list -p greggd

    step "cargo package --list (gregg)"
    run_or_fail cargo package --list -p gregg

    # Install binary loopback smoke
    step "installed-binary loopback smoke"
    TEMP_INSTALL_DIR="$(mktemp -d)"
    cleanup_install() { rm -rf "${TEMP_INSTALL_DIR}"; }
    trap cleanup_install EXIT

    run_or_fail cargo install --path crates/greggd --locked --root "${TEMP_INSTALL_DIR}" --debug
    run_or_fail "${TEMP_INSTALL_DIR}/bin/greggd" --version
    run_or_fail "${TEMP_INSTALL_DIR}/bin/greggd" --help
    run_or_fail bash "${SCRIPT_DIR}/verify-installed-daemon.sh" "${TEMP_INSTALL_DIR}/bin/greggd"

    trap - EXIT
    cleanup_install
fi

# ── Tier 3 (--release): release preflight ────────────────────────────────────

check_version_consistency() {
    local workspace_version_value
    workspace_version_value="$(awk '
        /^\[workspace\.package\][[:space:]]*$/ { in_package=1; next }
        in_package && /^\[/ { exit }
        in_package && /^[[:space:]]*version[[:space:]]*=/ {
            if (match($0, /"[^"]+"/)) {
                print substr($0, RSTART + 1, RLENGTH - 2)
                exit
            }
        }
    ' Cargo.toml)"

    if [[ -z "${workspace_version_value}" ]]; then
        echo "error: Cargo.toml has no version in [workspace.package]" >&2
        return 1
    fi

    local crate
    local manifest
    for crate in crates/gregg-protocol crates/greggd crates/gregg; do
        manifest="${crate}/Cargo.toml"
        if ! grep -Eq '^[[:space:]]*version\.workspace[[:space:]]*=[[:space:]]*true[[:space:]]*$' "${manifest}"; then
            echo "error: ${manifest} is missing version.workspace = true" >&2
            return 1
        fi
    done

    local dependency_lines
    local line
    local dependency_version
    for crate in crates/greggd crates/gregg; do
        manifest="${crate}/Cargo.toml"
        dependency_lines="$(grep -E '^[[:space:]]*gregg-protocol[[:space:]]*=' "${manifest}" || true)"
        if [[ -z "${dependency_lines}" ]]; then
            echo "error: ${manifest} has no gregg-protocol dependency declaration" >&2
            return 1
        fi
        while IFS= read -r line; do
            [[ -z "${line}" ]] && continue
            if [[ "${line}" =~ version[[:space:]]*=[[:space:]]*\"([^\"]+)\" ]]; then
                dependency_version="${BASH_REMATCH[1]}"
            else
                echo "error: ${manifest} gregg-protocol dependency has no registry version" >&2
                return 1
            fi
            if [[ "${dependency_version}" != "${workspace_version_value}" ]]; then
                echo "error: ${manifest} gregg-protocol dependency version ${dependency_version} != workspace ${workspace_version_value}" >&2
                return 1
            fi
        done <<< "${dependency_lines}"
    done

    echo "  workspace version ${workspace_version_value}; all members inherit it and gregg-protocol constraints match"
}

if [[ "${MODE}" == "release" ]]; then

    step "clean-tree check"
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "error: working tree is not clean" >&2
        git status --short >&2
        exit 1
    fi

    step "version consistency check"
    run_or_fail check_version_consistency

    step "cargo publish -p gregg-protocol --dry-run --locked"
    run_or_fail cargo publish -p gregg-protocol --dry-run --locked
fi

echo ""
echo "=== all checks passed (tier: ${MODE}) ==="
