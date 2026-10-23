#!/usr/bin/env bash
# check-gui.sh — local validation entry point for the optional Slint GUI.
#
# The GUI is a SEPARATE Cargo workspace (own lockfile, own rust-toolchain.toml,
# own Rust 1.92 floor), so the root workspace's checks never see it: the root
# `cargo test --workspace` must not pull Slint in, and the GUI's toolchain
# must not raise the root MSRV. That separation means the GUI also needs its
# OWN checks, and this script is them.
#
# The GUI's CI job runs exactly this script, so a green run here is the same
# claim a green CI run makes.
#
# Usage:
#   ./scripts/check-gui.sh [--help]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MANIFEST="${REPO_ROOT}/apps/gregg-gui/Cargo.toml"

usage() {
    cat <<'EOF'
Usage: scripts/check-gui.sh [--help]

Runs the optional GUI workspace's own checks: fmt, clippy, tests and a
release build. The GUI is a separate Cargo workspace, so these are not part
of scripts/check-local.sh.

Options:
  --help       Show this help message and exit.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
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
    echo "gui check failed: ${CURRENT_STEP}" >&2
    exit 1
}

run_or_fail() {
    "$@" || fail
}

if [[ ! -f "${MANIFEST}" ]]; then
    echo "error: ${MANIFEST} not found" >&2
    exit 1
fi

step "cargo fmt --manifest-path apps/gregg-gui/Cargo.toml -- --check"
run_or_fail cargo fmt --manifest-path apps/gregg-gui/Cargo.toml -- --check

step "cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets --all-features -- -D warnings"
run_or_fail cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets --all-features -- -D warnings

step "cargo test --manifest-path apps/gregg-gui/Cargo.toml"
run_or_fail cargo test --manifest-path apps/gregg-gui/Cargo.toml

step "cargo build --manifest-path apps/gregg-gui/Cargo.toml --release"
run_or_fail cargo build --manifest-path apps/gregg-gui/Cargo.toml --release

BINARY="${REPO_ROOT}/apps/gregg-gui/target/release/gregg-gui"
if [[ -f "${BINARY}" ]]; then
    SIZE="$(wc -c < "${BINARY}" | tr -d '[:space:]')"
    echo "    gregg-gui release binary: ${SIZE} bytes"
fi

echo ""
echo "=== gui checks passed ==="
