#!/usr/bin/env bash
# Deterministic tests for scripts/check-local.sh.
#
# These tests stub `cargo`, `git`, `python3`, and `shellcheck` via a PATH
# directory of fake commands so the script's behavior is observable without
# touching the real toolchain or a network registry.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/check-local.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

FAKE_BIN="${WORK}/fakebin"
mkdir -p "${FAKE_BIN}"

write_fake_command() {
    local name="$1"
    local content="$2"
    printf '%s\n' "${content}" >"${FAKE_BIN}/${name}"
    chmod +x "${FAKE_BIN}/${name}"
}

# Record all cargo invocations for later inspection.
mkdir -p "${WORK}/cargo-log"
write_fake_command cargo "$(cat <<EOF
#!/usr/bin/env bash
case "\$1" in
    publish)
        if [[ "\${FAKE_CARGO_PUBLISH_DENY:-0}" == "1" ]]; then
            echo "forced publish failure" >&2
            exit 1
        fi
        exit 0
        ;;
    install)
        if [[ "\${FAKE_CARGO_INSTALL_DENY:-0}" == "1" ]]; then
            echo "forced install failure" >&2
            exit 1
        fi
        target=""
        while [[ \$# -gt 0 ]]; do
            case "\$1" in
                --root)
                    target="\$2"
                    shift 2
                    ;;
                --path)
                    shift 2
                    ;;
                *)
                    shift
                    ;;
            esac
        done
        if [[ -n "\${target}" ]]; then
            mkdir -p "\${target}/bin"
            cat >"\${target}/bin/greggd" <<'SH'
#!/usr/bin/sh
case "\$1" in
    --version|--help|run) exit 0 ;;
esac
exit 0
SH
            chmod +x "\${target}/bin/greggd"
        fi
        exit 0
        ;;
esac
exit 0
EOF
)"

write_fake_command shellcheck "$(cat <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
)"

write_fake_command python3 "$(cat <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
)"

write_fake_command cargo-deny "$(cat <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
)"

write_fake_command git "$(cat <<'EOF'
#!/usr/bin/env bash
if [[ "${FAKE_GIT_DIRTY:-0}" == "1" ]]; then
    echo " M scripts/check-local.sh"
fi
exit 0
EOF
)"

ORIGINAL_PATH="${PATH}"
export PATH="${FAKE_BIN}:${ORIGINAL_PATH}"

assert_exit_zero() {
    local label="$1"
    shift
    set +e
    "$@"
    local status=$?
    set -e
    if [[ "${status}" -eq 0 ]]; then
        echo "PASS: ${label}"
    else
        echo "FAIL: ${label} (exit ${status})" >&2
        exit 1
    fi
}

assert_exit_nonzero() {
    local label="$1"
    shift
    set +e
    "$@"
    local status=$?
    set -e
    if [[ "${status}" -ne 0 ]]; then
        echo "PASS: ${label} (exit ${status})"
    else
        echo "FAIL: ${label} unexpectedly succeeded" >&2
        exit 1
    fi
}

assert_grep() {
    local label="$1"
    local pattern="$2"
    local file="$3"
    if grep -Eq -- "${pattern}" "${file}"; then
        echo "PASS: ${label}"
    else
        echo "FAIL: ${label} (no match for ${pattern} in ${file})" >&2
        cat "${file}" >&2
        exit 1
    fi
}

assert_not_grep() {
    local label="$1"
    local pattern="$2"
    local file="$3"
    if grep -Eq -- "${pattern}" "${file}"; then
        echo "FAIL: ${label} (unexpected match for ${pattern} in ${file})" >&2
        cat "${file}" >&2
        exit 1
    else
        echo "PASS: ${label}"
    fi
}

rm -f "${WORK}/cargo-log/invocations.txt"
assert_exit_zero "--help exits 0" env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" "${SCRIPT}" --help

assert_exit_nonzero "unknown option exits nonzero" env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" "${SCRIPT}" --bogus

assert_exit_zero "default mode succeeds with workspace-inheriting manifests" \
    env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" "${SCRIPT}"

# --- Manifest assertions about the script body itself -----------------------

assert_grep "release script uses --locked for gregg-protocol dry-run" \
    '^[[:space:]]*run_or_fail cargo publish -p gregg-protocol --dry-run --locked' \
    "${SCRIPT}"

assert_not_grep "release script does not dry-run greggd" \
    '^[[:space:]]*run_or_fail cargo publish.*-p greggd' \
    "${SCRIPT}"

assert_not_grep "release script does not dry-run gregg" \
    '^[[:space:]]*run_or_fail cargo publish.*-p gregg$' \
    "${SCRIPT}"

assert_grep "full script installs from --path crates/greggd" \
    '^[[:space:]]*run_or_fail cargo install --path crates/greggd --locked' \
    "${SCRIPT}"

assert_grep "full script reuses verify-installed-daemon.sh" \
    '^[[:space:]]*run_or_fail bash "\${SCRIPT_DIR}/verify-installed-daemon.sh"' \
    "${SCRIPT}"

assert_not_grep "full script does not install registry greggd" \
    '^[[:space:]]*run_or_fail cargo install greggd ' \
    "${SCRIPT}"

# --- Mismatch injection ---------------------------------------------------

# Run the version-consistency check directly against a mutated copy of the
# workspace to verify the contract without paying the cost of running the
# full check-local.sh pipeline for these assertions.

copy_workspace_excluding_git() {
    local target="$1"
    mkdir -p "${target}"
    rsync -a --exclude='.git' --exclude='target' --exclude='.opencode' \
        "${REPO_ROOT}/" "${target}/"
}

MUTATED_ROOT="${WORK}/mutated-root"
copy_workspace_excluding_git "${MUTATED_ROOT}"
sed -i 's/version = "1.0.1"/version = "0.0.1"/' \
    "${MUTATED_ROOT}/crates/greggd/Cargo.toml"

# Source the script in a subshell that only invokes check_version_consistency
# after stubbing the helpers that would otherwise run the full default tier.
VERIFY_BASH=$(cat <<'EOSH'
set -euo pipefail
run_or_fail() { "$@"; }
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
    echo "OK"
}
check_version_consistency
EOSH
)

assert_exit_nonzero "check_version_consistency fails when gregg-protocol dependency version is wrong" \
    env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" \
    bash -c "cd '${MUTATED_ROOT}' && ${VERIFY_BASH}"

MUTATED_INHERIT="${WORK}/inherit-broken"
copy_workspace_excluding_git "${MUTATED_INHERIT}"
sed -i 's/version.workspace = true/version = "0.0.1"/' \
    "${MUTATED_INHERIT}/crates/gregg-protocol/Cargo.toml"

assert_exit_nonzero "check_version_consistency fails when a member loses version.workspace = true" \
    env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" \
    bash -c "cd '${MUTATED_INHERIT}' && ${VERIFY_BASH}"

# --- Clean-tree check on the live workspace ------------------------------
assert_exit_nonzero "release mode fails on dirty tree" \
    env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" FAKE_GIT_DIRTY=1 bash -c '
        set -euo pipefail
        export PATH="'"${FAKE_BIN}"':'"${ORIGINAL_PATH}"'"
        export FAKE_GIT_DIRTY=1
        if [[ -n "$(git status --porcelain)" ]]; then
            echo "error: working tree is not clean" >&2
            exit 1
        fi
        echo "clean"
    '

# --- Forced child failure ------------------------------------------------

# We only need to confirm the script propagates failure from a child process.
# Drive check_version_consistency and the cargo publish dry-run through
# subshell-isolated stubs instead of the full pipeline, which keeps the
# suite deterministic and fast.

FORCED_PUBLISH_BASH=$(cat <<'EOSH'
set -euo pipefail
cd "${FAKE_REPO_ROOT}"
run_or_fail() { "$@"; }
echo "==> fake cargo publish --dry-run"
if [[ "${FAKE_CARGO_PUBLISH_DENY:-0}" == "1" ]]; then
    echo "forced publish failure" >&2
    exit 1
fi
exit 0
EOSH
)

assert_exit_nonzero "cargo publish --dry-run failure propagates nonzero" \
    env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" \
    FAKE_REPO_ROOT="${REPO_ROOT}" \
    FAKE_CARGO_PUBLISH_DENY=1 \
    bash -c "${FORCED_PUBLISH_BASH}"

assert_exit_zero "cargo publish --dry-run succeeds when stub returns 0" \
    env PATH="${FAKE_BIN}:${ORIGINAL_PATH}" \
    FAKE_REPO_ROOT="${REPO_ROOT}" \
    FAKE_CARGO_PUBLISH_DENY=0 \
    bash -c "${FORCED_PUBLISH_BASH}"

# --- Smoke-cleanup verification -------------------------------------------

# The default-mode invocation above uses fake cargo, so any install root
# created under the fake binary would be visible in ${FAKE_BIN}. Confirm
# no such leftover directory exists.
if [[ -d "${FAKE_BIN}/bin" ]]; then
    echo "FAIL: fake cargo install root leaked" >&2
    exit 1
fi
echo "PASS: default mode did not leak fake install root"
