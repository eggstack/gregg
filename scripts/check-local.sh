#!/usr/bin/env bash
# check-local.sh — local validation entry point for gregg.
#
# Modes:
#   (default)   Fast developer check: fmt and workspace tests.
#   --release   Release preflight: adds lint/docs, clean-tree, package/install smoke, dry-run.
#
# Usage:
#   ./scripts/check-local.sh [--release] [--help]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

MODE="default"

usage() {
    cat <<'EOF'
Usage: scripts/check-local.sh [OPTIONS]

Options:
  --release    Run the release preflight tier (adds clean-tree, version
               consistency, package lists, installed-binary smoke, and the
               protocol publish dry-run).
  --help       Show this help message and exit.

Modes:
  (default)    fmt and workspace tests.
  --release    Default checks plus clippy, docs, clean-tree, version/package checks,
               source installation, v2 loopback smoke, and protocol dry-run.

Examples:
  ./scripts/check-local.sh                  # fast developer check
  ./scripts/check-local.sh --release        # release preflight
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            MODE="release"
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

# ── Tier 1 (default): fast developer check ──────────────────────────────────

step "cargo fmt --all -- --check"
run_or_fail cargo fmt --all -- --check

step "cargo test --workspace"
run_or_fail cargo test --workspace

# ── Release preflight ───────────────────────────────────────────────────────

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
    step "cargo clippy --workspace --all-targets --all-features -- -D warnings"
    run_or_fail cargo clippy --workspace --all-targets --all-features -- -D warnings

    step "cargo doc --workspace --no-deps"
    run_or_fail cargo doc --workspace --no-deps

    step "clean-tree check"
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "error: working tree is not clean" >&2
        git status --short >&2
        exit 1
    fi

    step "version consistency check"
    run_or_fail check_version_consistency

    step "cargo package --list (gregg-protocol)"
    run_or_fail cargo package --list -p gregg-protocol

    step "cargo package --list (greggd)"
    run_or_fail cargo package --list -p greggd

    step "cargo package --list (gregg)"
    run_or_fail cargo package --list -p gregg

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

    step "cargo publish -p gregg-protocol --dry-run --locked"
    run_or_fail cargo publish -p gregg-protocol --dry-run --locked
fi

echo ""
echo "=== all checks passed (mode: ${MODE}) ==="
