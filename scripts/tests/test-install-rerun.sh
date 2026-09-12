#!/usr/bin/env bash
# test-install-rerun.sh — deterministic installer-rerun coverage for install.sh (Plan 112).
#
# Drives the real `packaging/install.sh` with fake `curl`/`cargo` commands and
# an isolated HOME, proving same-scope rerun semantics without network:
#
#   - missing destination -> first install;
#   - valid same-component destination -> replacement/update with versions;
#   - same version -> safe idempotent replacement;
#   - pinned older/newer versions honor the requested tag;
#   - foreign/unidentifiable destination -> no overwrite, actionable error;
#   - Cargo fallback stages privately and leaves no Cargo metadata behind.
#
# Usage: bash scripts/tests/test-install-rerun.sh
# Exit nonzero on the first failure; prints TAP-ish progress to stdout.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INSTALL_SH="${REPO_ROOT}/packaging/install.sh"

PASS=0
FAIL=0

ok() {
  PASS=$((PASS + 1))
  echo "ok - $1"
}

fail() {
  FAIL=$((FAIL + 1))
  echo "FAIL - $1" >&2
}

# --- isolated environment -----------------------------------------------------

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

export HOME="${SANDBOX}/home"
mkdir -p "$HOME"
DEST_DIR="${HOME}/.local/bin"

FAKEBIN="${SANDBOX}/fakebin"
mkdir -p "$FAKEBIN"

# Requested fake asset version for the next install.sh invocation.
FAKE_VERSION="9.9.9"
# When set to 404, the fake curl fails asset downloads with HTTP 404.
FAKE_CURL_MODE="ok"
# Log of fake-cargo invocations.
CARGO_LOG="${SANDBOX}/cargo.log"
: > "$CARGO_LOG"

# Fake release-asset executable content: prints `<program> <version>`.
make_fake_asset() {
  local program="$1"
  local version="$2"
  local out="$3"
  cat > "$out" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "version" ]]; then
  echo "${program} ${version}"
  exit 0
fi
if [[ "\${1:-}" == "startup" ]]; then
  exit 0
fi
echo "fake ${program}" >&2
exit 0
EOF
  chmod +x "$out"
}

# Fake curl: serves the fake asset + checksum, or HTTP 404 when asked.
cat > "${FAKEBIN}/curl" <<'EOF'
#!/usr/bin/env bash
# Intercept: curl -fsSL -o <file> <url>   and   curl -s -o /dev/null -w ... <url>
outfile=""
url=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "-o" ]]; then
    outfile="$arg"
  fi
  prev="$arg"
  case "$arg" in
    http*) url="$arg" ;;
  esac
done
if [[ "${FAKE_CURL_MODE:-ok}" == "404" ]]; then
  if [[ " $* " == *" %{http_code}"* ]]; then
    printf '404'
    exit 0
  fi
  echo "curl: (22) the requested URL returned error: 404" >&2
  exit 22
fi
if [[ " $* " == *" %{http_code}"* ]]; then
  printf '200'
  exit 0
fi
if [[ "$outfile" == *.sha256 ]]; then
  asset="${outfile%.sha256}"
  ( sha256sum "$asset" 2>/dev/null || shasum -a 256 "$asset" ) | awk '{print $1 "  fake"}' > "$outfile"
  exit 0
fi
# Asset download: derive program from the URL tail (<program>-<target>).
base="$(basename "$url")"
program="${base%%-*}"
make_fake_asset_inline() {
  cat > "$outfile" <<INNEREOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "version" ]]; then
  echo "${program} ${FAKE_VERSION:-9.9.9}"
  exit 0
fi
if [[ "\${1:-}" == "startup" ]]; then
  exit 0
fi
exit 0
INNEREOF
  chmod +x "$outfile"
}
make_fake_asset_inline
exit 0
EOF
chmod +x "${FAKEBIN}/curl"

# Fake cargo: implements `cargo install --locked [--version X] --root R <program>`
# by writing a fake executable, and logs its arguments.
cat > "${FAKEBIN}/cargo" <<'EOF'
#!/usr/bin/env bash
echo "cargo $*" >> "${CARGO_LOG:?}"
if [[ " $* " == *" install "* ]]; then
  root=""
  prev=""
  program=""
  for arg in "$@"; do
    if [[ "$prev" == "--root" ]]; then
      root="$arg"
    fi
    prev="$arg"
    case "$arg" in
      --*|--*=*|install|--locked) ;;
      *) program="$arg" ;;
    esac
  done
  # Last non-flag token that is not the --version value: find --version value to skip.
  skip_next=0
  program=""
  prev=""
  for arg in "$@"; do
    if [[ $skip_next -eq 1 ]]; then
      skip_next=0
      prev="$arg"
      continue
    fi
    if [[ "$prev" == "--version" ]]; then
      prev="$arg"
      continue
    fi
    case "$arg" in
      --version) skip_next=0 ;;
      --root) skip_next=1 ;;
      --*|install|--locked) ;;
      *) program="$arg" ;;
    esac
    prev="$arg"
  done
  mkdir -p "${root}/bin"
  cat > "${root}/bin/${program}" <<INNEREOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "version" ]]; then
  echo "${program} ${FAKE_VERSION:-9.9.9}"
  exit 0
fi
if [[ "\${1:-}" == "startup" ]]; then
  exit 0
fi
exit 0
INNEREOF
  chmod +x "${root}/bin/${program}"
  # Simulate Cargo bookkeeping that must NOT leak to the real destination.
  echo '{"fake":true}' > "${root}/.crates.toml"
  exit 0
fi
if [[ " $* " == *" --version "* ]]; then
  echo "cargo 1.75.0"
  exit 0
fi
echo "cargo 1.75.0"
exit 0
EOF
chmod +x "${FAKEBIN}/cargo"

export PATH="${FAKEBIN}:${PATH}"
export FAKE_VERSION FAKE_CURL_MODE CARGO_LOG

run_install() {
  # run_install <args...> — runs install.sh, captures output+status.
  set +e
  OUT="$(bash "$INSTALL_SH" "$@" 2>&1)"
  STATUS=$?
  set -e
}

expect_contains() {
  # expect_contains <haystack> <needle> <label>
  if [[ "$1" == *"$2"* ]]; then
    ok "$3"
  else
    fail "$3 (missing $(printf '%q' "$2") in: $1)"
  fi
}

# --- 1. first install ----------------------------------------------------------

run_install gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "first install exits 0 and installs the binary"
else
  fail "first install (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "Installed gregg to" "first install reports an install (not an update)"
if [[ "$("${DEST_DIR}/gregg" version)" == "gregg 9.9.9" ]]; then
  ok "installed binary validates as the candidate version"
else
  fail "installed binary version check"
fi

# --- 2. same-version rerun is a safe in-place replacement ----------------------

run_install gregg
if [[ $STATUS -eq 0 ]]; then
  ok "same-version rerun exits 0"
else
  fail "same-version rerun (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "replace" "same-version rerun identifies replacement scope"
expect_contains "$OUT" "gregg 9.9.9" "same-version rerun shows the existing version"

# --- 3. older-version replacement shows the upgrade ----------------------------

make_fake_asset gregg 1.0.0 "${DEST_DIR}/gregg"
FAKE_VERSION="9.9.9"
export FAKE_VERSION
run_install gregg
if [[ $STATUS -eq 0 ]]; then
  ok "older-version replacement exits 0"
else
  fail "older-version replacement (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "gregg 1.0.0" "replacement output names the existing version"
expect_contains "$OUT" "gregg 9.9.9" "replacement output names the candidate version"
if [[ "$("${DEST_DIR}/gregg" version)" == "gregg 9.9.9" ]]; then
  ok "destination now holds the candidate version"
else
  fail "destination version after replacement"
fi

# --- 4. pinned version is honored ------------------------------------------------

make_fake_asset gregg 1.0.0 "${DEST_DIR}/gregg"
FAKE_VERSION="1.0.11"
export FAKE_VERSION
run_install --version 1.0.11 gregg
if [[ $STATUS -eq 0 && "$("${DEST_DIR}/gregg" version)" == "gregg 1.0.11" ]]; then
  ok "pinned version installs the requested tag"
else
  fail "pinned install (status=$STATUS, out=$OUT)"
fi

# A candidate that disagrees with the pin is a hard error, not a silent install.
FAKE_VERSION="9.9.9"
export FAKE_VERSION
run_install --version 1.0.11 gregg
if [[ $STATUS -ne 0 ]]; then
  ok "version mismatch against the pin fails instead of installing"
else
  fail "pin mismatch should fail (out=$OUT)"
fi
if [[ "$("${DEST_DIR}/gregg" version)" == "gregg 1.0.11" ]]; then
  ok "failed pin leaves the previous binary untouched"
else
  fail "failed pin mutated the destination"
fi

# --- 5. foreign destination is never overwritten ----------------------------------

printf '#!/usr/bin/env bash\necho "something-else 1.2.3"\n' > "${DEST_DIR}/gregg"
chmod +x "${DEST_DIR}/gregg"
run_install gregg
if [[ $STATUS -ne 0 ]]; then
  ok "foreign destination fails instead of overwriting"
else
  fail "foreign destination should fail (out=$OUT)"
fi
expect_contains "$OUT" "Refusing to overwrite" "foreign destination prints an actionable diagnostic"
if grep -q "something-else" "${DEST_DIR}/gregg"; then
  ok "foreign executable is preserved byte-for-byte"
else
  fail "foreign executable was clobbered"
fi

# Unidentifiable (non-executable) destination is also protected.
rm -f "${DEST_DIR}/gregg"
echo "not an executable" > "${DEST_DIR}/gregg"
chmod 644 "${DEST_DIR}/gregg"
run_install gregg
if [[ $STATUS -ne 0 && "$(cat "${DEST_DIR}/gregg")" == "not an executable" ]]; then
  ok "unidentifiable destination is preserved"
else
  fail "unidentifiable destination (status=$STATUS, out=$OUT)"
fi
rm -f "${DEST_DIR}/gregg"

# --- 6. greggd rerun replaces in place --------------------------------------------

FAKE_VERSION="9.9.9"
export FAKE_VERSION
make_fake_asset greggd 1.0.0 "${DEST_DIR}/greggd"
run_install greggd
if [[ $STATUS -eq 0 ]]; then
  ok "greggd rerun exits 0"
else
  fail "greggd rerun (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "greggd 1.0.0" "greggd replacement names the existing version"

# --- 7. Cargo fallback stages privately --------------------------------------------

FAKE_CURL_MODE="404"
export FAKE_CURL_MODE
: > "$CARGO_LOG"
rm -f "${DEST_DIR}/gregg"
FAKE_VERSION="9.9.9"
export FAKE_VERSION
run_install gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "Cargo fallback installs the binary to the bootstrap destination"
else
  fail "Cargo fallback (status=$STATUS, out=$OUT)"
fi
if grep -q "\-\-root" "$CARGO_LOG"; then
  STAGING_ROOT="$(grep -o "\-\-root [^ ]*" "$CARGO_LOG" | head -1 | cut -d' ' -f2)"
  if [[ "$STAGING_ROOT" == "${SANDBOX}"* || "$STAGING_ROOT" == /tmp/* ]]; then
    ok "Cargo fallback builds under a temporary staging root ($STAGING_ROOT)"
  else
    fail "Cargo root is not staging-private: $STAGING_ROOT"
  fi
  if [[ -e "$STAGING_ROOT" ]]; then
    fail "staging root was not cleaned up: $STAGING_ROOT"
  else
    ok "staging root is removed afterwards"
  fi
else
  fail "fake cargo saw no --root invocation"
fi
if [[ -e "${HOME}/.local/.crates.toml" || -e "${DEST_DIR}/.crates.toml" || -e "${HOME}/.cargo" ]]; then
  fail "Cargo ownership metadata leaked outside staging"
else
  ok "no Cargo metadata persists outside temporary staging"
fi
if [[ "$("${DEST_DIR}/gregg" version)" == "gregg 9.9.9" ]]; then
  ok "staged Cargo binary validates at the destination"
else
  fail "staged binary version check"
fi

# --- summary -----------------------------------------------------------------------

echo ""
echo "pass=$PASS fail=$FAIL"
if [[ $FAIL -ne 0 ]]; then
  exit 1
fi
