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
ACTIVATION_FAIL=""
FAKE_DAEMON_RUNNING="0"
# Log of fake-cargo invocations.
CARGO_LOG="${SANDBOX}/cargo.log"
: > "$CARGO_LOG"
STARTUP_LOG="${SANDBOX}/startup.log"
: > "$STARTUP_LOG"
DAEMON_LOG="${SANDBOX}/daemon.log"
: > "$DAEMON_LOG"
RUNNING_MARKER="${SANDBOX}/greggd.running"
rm -f "$RUNNING_MARKER"

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
  echo "${program} startup" >> "\${STARTUP_LOG:?}"
  exit 0
fi
if [[ "\${1:-}" == "status" ]]; then
  echo "${program} status" >> "\${DAEMON_LOG:?}"
  [[ "${program}" != "greggd" || "\${FAKE_DAEMON_RUNNING:-0}" == "1" ]]
  exit \$?
fi
if [[ "\${1:-}" == "stop" ]]; then
  echo "${program} stop" >> "\${DAEMON_LOG:?}"
  if [[ "\${ACTIVATION_FAIL:-}" == "stop" ]]; then exit 1; fi
  rm -f "\${RUNNING_MARKER:?}"
  exit 0
fi
if [[ "\${1:-}" == "croncheck" ]]; then
  echo "${program} croncheck" >> "\${DAEMON_LOG:?}"
  if [[ "\${ACTIVATION_FAIL:-}" == "croncheck" ]]; then exit 1; fi
  touch "\${RUNNING_MARKER:?}"
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
  echo "${program} startup" >> "\${STARTUP_LOG:?}"
  exit 0
fi
if [[ "\${1:-}" == "status" ]]; then
  echo "${program} status" >> "\${DAEMON_LOG:?}"
  [[ "${program}" != "greggd" || "\${FAKE_DAEMON_RUNNING:-0}" == "1" ]]
  exit \$?
fi
if [[ "\${1:-}" == "stop" ]]; then
  echo "${program} stop" >> "\${DAEMON_LOG:?}"
  if [[ "\${ACTIVATION_FAIL:-}" == "stop" ]]; then exit 1; fi
  rm -f "\${RUNNING_MARKER:?}"
  exit 0
fi
if [[ "\${1:-}" == "croncheck" ]]; then
  echo "${program} croncheck" >> "\${DAEMON_LOG:?}"
  if [[ "\${ACTIVATION_FAIL:-}" == "croncheck" ]]; then exit 1; fi
  touch "\${RUNNING_MARKER:?}"
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
  echo "${program} startup" >> "\${STARTUP_LOG:?}"
  exit 0
fi
if [[ "\${1:-}" == "status" ]]; then
  echo "${program} status" >> "\${DAEMON_LOG:?}"
  [[ "${program}" != "greggd" || "\${FAKE_DAEMON_RUNNING:-0}" == "1" ]]
  exit \$?
fi
if [[ "\${1:-}" == "stop" ]]; then
  echo "${program} stop" >> "\${DAEMON_LOG:?}"
  if [[ "\${ACTIVATION_FAIL:-}" == "stop" ]]; then exit 1; fi
  rm -f "\${RUNNING_MARKER:?}"
  exit 0
fi
if [[ "\${1:-}" == "croncheck" ]]; then
  echo "${program} croncheck" >> "\${DAEMON_LOG:?}"
  if [[ "\${ACTIVATION_FAIL:-}" == "croncheck" ]]; then exit 1; fi
  touch "\${RUNNING_MARKER:?}"
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
export FAKE_VERSION FAKE_CURL_MODE ACTIVATION_FAIL FAKE_DAEMON_RUNNING CARGO_LOG STARTUP_LOG DAEMON_LOG RUNNING_MARKER

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
rm -f "$RUNNING_MARKER"
make_fake_asset greggd 1.0.0 "${DEST_DIR}/greggd"
run_install greggd
if [[ $STATUS -eq 0 ]]; then
  ok "greggd rerun exits 0"
else
  fail "greggd rerun (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "greggd 1.0.0" "greggd replacement names the existing version"

STOP_COUNT_BEFORE="$(grep -c '^greggd stop$' "$DAEMON_LOG" || true)"
CRONCHECK_COUNT_BEFORE="$(grep -c '^greggd croncheck$' "$DAEMON_LOG" || true)"
rm -f "$RUNNING_MARKER"
run_install greggd
STOP_COUNT_AFTER="$(grep -c '^greggd stop$' "$DAEMON_LOG" || true)"
CRONCHECK_COUNT_AFTER="$(grep -c '^greggd croncheck$' "$DAEMON_LOG" || true)"
if [[ "$STOP_COUNT_AFTER" == "$STOP_COUNT_BEFORE" && "$CRONCHECK_COUNT_AFTER" == "$CRONCHECK_COUNT_BEFORE" ]]; then
  ok "stopped same-scope replacement remains stopped and does not run activation commands"
else
  fail "stopped replacement ran activation commands (stop ${STOP_COUNT_BEFORE}->${STOP_COUNT_AFTER}, croncheck ${CRONCHECK_COUNT_BEFORE}->${CRONCHECK_COUNT_AFTER})"
fi

touch "$RUNNING_MARKER"
FAKE_DAEMON_RUNNING="1"
export FAKE_DAEMON_RUNNING
STOP_COUNT_BEFORE="$STOP_COUNT_AFTER"
CRONCHECK_COUNT_BEFORE="$CRONCHECK_COUNT_AFTER"
run_install greggd
STOP_COUNT_AFTER="$(grep -c '^greggd stop$' "$DAEMON_LOG" || true)"
CRONCHECK_COUNT_AFTER="$(grep -c '^greggd croncheck$' "$DAEMON_LOG" || true)"
if [[ "$STOP_COUNT_AFTER" -eq $((STOP_COUNT_BEFORE + 1)) && "$CRONCHECK_COUNT_AFTER" -eq $((CRONCHECK_COUNT_BEFORE + 1)) && -e "$RUNNING_MARKER" ]]; then
  ok "running prebuilt replacement performs one safe stop/croncheck activation"
else
  fail "running prebuilt replacement activation (status=$STATUS, out=$OUT)"
fi

touch "$RUNNING_MARKER"
FAKE_DAEMON_RUNNING="1"
export FAKE_DAEMON_RUNNING
ACTIVATION_FAIL="croncheck"
export ACTIVATION_FAIL
run_install greggd
unset ACTIVATION_FAIL
if [[ "$STATUS" -ne 0 && "$OUT" == *"binary updated; daemon activation/restart failed"* && "$OUT" == *"Retry:"* ]]; then
  ok "activation failure after replacement is reported nonzero with an exact retry"
else
  fail "activation failure diagnostic (status=$STATUS, out=$OUT)"
fi

rm -f "$DEST_DIR/greggd" "$RUNNING_MARKER"
FAKE_DAEMON_RUNNING="0"
export FAKE_DAEMON_RUNNING
STOP_COUNT_BEFORE="$(grep -c '^greggd stop$' "$DAEMON_LOG" || true)"
CRONCHECK_COUNT_BEFORE="$(grep -c '^greggd croncheck$' "$DAEMON_LOG" || true)"
run_install greggd
STOP_COUNT_AFTER="$(grep -c '^greggd stop$' "$DAEMON_LOG" || true)"
CRONCHECK_COUNT_AFTER="$(grep -c '^greggd croncheck$' "$DAEMON_LOG" || true)"
if [[ "$STATUS" -eq 0 && "$STOP_COUNT_AFTER" == "$STOP_COUNT_BEFORE" && "$CRONCHECK_COUNT_AFTER" == "$CRONCHECK_COUNT_BEFORE" ]]; then
  ok "first daemon install does not run replacement-only activation"
else
  fail "first daemon install activation (status=$STATUS, out=$OUT)"
fi

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

# A staged daemon candidate must use the same post-install startup and
# replacement-only activation path as a downloaded candidate.
touch "$RUNNING_MARKER"
FAKE_DAEMON_RUNNING="1"
export FAKE_DAEMON_RUNNING
STOP_COUNT_BEFORE="$STOP_COUNT_AFTER"
CRONCHECK_COUNT_BEFORE="$CRONCHECK_COUNT_AFTER"
run_install greggd
STOP_COUNT_AFTER="$(grep -c '^greggd stop$' "$DAEMON_LOG" || true)"
CRONCHECK_COUNT_AFTER="$(grep -c '^greggd croncheck$' "$DAEMON_LOG" || true)"
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/greggd" && "$(grep -c '^greggd startup$' "$STARTUP_LOG")" -ge 1 && "$STOP_COUNT_AFTER" -eq $((STOP_COUNT_BEFORE + 1)) && "$CRONCHECK_COUNT_AFTER" -eq $((CRONCHECK_COUNT_BEFORE + 1)) && -e "$RUNNING_MARKER" ]]; then
  ok "staged Cargo daemon reaches shared finalization and activation"
else
  fail "staged Cargo daemon startup finalization (status=$STATUS, out=$OUT)"
fi

# --- 8. Plan 130 user-local PATH activation ---------------------------------------
#
# Bounded post-install shell persistence with isolated HOME/PATH/SHELL.
# Never touches the runner's real dotfiles. Retains the fake curl/Cargo
# strategy; no network access.

fresh_path_home() {
  # fresh_path_home <name> — new isolated HOME under SANDBOX, DEST_DIR synced.
  HOME="${SANDBOX}/home-$1"
  mkdir -p "$HOME"
  export HOME
  DEST_DIR="${HOME}/.local/bin"
  export DEST_DIR
}

path_without_dest() {
  # Minimal deterministic PATH without DEST_DIR, retaining fakebin + tools.
  PATH="${FAKEBIN}:/usr/bin:/bin"
  export PATH
}

path_with_dest() {
  PATH="${FAKEBIN}:${DEST_DIR}:/usr/bin:/bin"
  export PATH
}

count_occurrences() {
  # count_occurrences <haystack> <needle> — prints integer count.
  local haystack="$1"
  local needle="$2"
  printf '%s' "$haystack" | grep -o -F "$needle" | wc -l | tr -d ' '
}

# Reset download fakes altered by section 7; PATH tests use prebuilt assets.
FAKE_CURL_MODE="ok"
FAKE_VERSION="9.9.9"
FAKE_DAEMON_RUNNING="0"
unset ACTIVATION_FAIL || true
unset GREGG_TEST_OS || true
unset GREGG_TEST_FORCE_SYSTEM || true
unset ZDOTDIR || true
export FAKE_CURL_MODE FAKE_VERSION FAKE_DAEMON_RUNNING

# 8.1 zsh: absent PATH -> bounded profile entry added.
fresh_path_home "zsh"
path_without_dest
export SHELL="/bin/zsh"
unset ZDOTDIR || true
rm -f "${HOME}/.zshrc"
run_install gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "zsh PATH integration installs the binary successfully"
else
  fail "zsh PATH integration install (status=$STATUS, out=$OUT)"
fi
if [[ -f "${HOME}/.zshrc" ]] && grep -Fq "added by gregg installer" "${HOME}/.zshrc" && grep -Fq "\$HOME/.local/bin" "${HOME}/.zshrc"; then
  ok "zsh install adds a bounded profile entry with stable HOME expression"
else
  fail "zsh profile entry missing or expanded (home=$HOME)"
fi
expect_contains "$OUT" "for future shells" "zsh install reports future-shell persistence"
expect_contains "$OUT" "For this shell, run:" "zsh install reports current-shell activation separately"
expect_contains "$OUT" "export PATH=\"\$HOME/.local/bin:\$PATH\"" "zsh install prints the exact manual export"
if grep -Fq "${HOME}/.local/bin" "${HOME}/.zshrc" 2>/dev/null && ! grep -Fq "\$HOME/.local/bin" "${HOME}/.zshrc"; then
  fail "zsh profile persisted an expanded home path instead of the stable expression"
else
  ok "zsh profile does not persist an expanded absolute home path"
fi

# 8.2 rerun -> no duplicate entry.
ZSH_COUNT_BEFORE="$(grep -c "added by gregg installer" "${HOME}/.zshrc" || true)"
run_install gregg
ZSH_COUNT_AFTER="$(grep -c "added by gregg installer" "${HOME}/.zshrc" || true)"
if [[ $STATUS -eq 0 && "$ZSH_COUNT_AFTER" == "$ZSH_COUNT_BEFORE" && "$ZSH_COUNT_AFTER" == "1" ]]; then
  ok "zsh rerun is idempotent with no duplicate profile entry"
else
  fail "zsh rerun duplicated the profile entry (before=$ZSH_COUNT_BEFORE after=$ZSH_COUNT_AFTER status=$STATUS out=$OUT)"
fi
if [[ "$OUT" == *"Added "* && "$OUT" == *"for future shells"* ]]; then
  fail "zsh rerun must not report a fresh profile addition (out=$OUT)"
else
  ok "zsh rerun does not claim a fresh profile addition"
fi

# 8.3 existing user-authored entry -> no redundant Gregg entry.
fresh_path_home "zsh-user"
path_without_dest
export SHELL="/bin/zsh"
unset ZDOTDIR || true
printf '%s\n' "export PATH=\"\$HOME/.local/bin:\$PATH\"" > "${HOME}/.zshrc"
run_install gregg
if [[ $STATUS -eq 0 ]]; then
  ok "user-authored entry install exits 0"
else
  fail "user-authored entry install (status=$STATUS, out=$OUT)"
fi
if [[ "$(grep -c -F ".local/bin" "${HOME}/.zshrc" || true)" == "1" ]] && ! grep -Fq "added by gregg installer" "${HOME}/.zshrc"; then
  ok "existing user-authored entry is not redundantly duplicated"
else
  fail "user-authored entry was duplicated"
fi
expect_contains "$OUT" "is not on the current PATH" "user-authored case still reports current-shell state truthfully"

# 8.4 bash on Linux -> ~/.bashrc integration.
fresh_path_home "bash-linux"
path_without_dest
export SHELL="/bin/bash"
unset ZDOTDIR || true
unset GREGG_TEST_OS || true
export GREGG_TEST_OS
rm -f "${HOME}/.bashrc" "${HOME}/.bash_profile" "${HOME}/.bash_login" "${HOME}/.profile"
run_install gregg
if [[ $STATUS -eq 0 && -f "${HOME}/.bashrc" ]] && grep -Fq "added by gregg installer" "${HOME}/.bashrc"; then
  ok "bash Linux install persists to ~/.bashrc"
else
  fail "bash Linux profile target (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" ".bashrc" "bash Linux output names the touched profile"

# 8.5 bash profile content is preserved byte-for-byte except the append.
fresh_path_home "bash-preserve"
path_without_dest
export SHELL="/bin/bash"
unset GREGG_TEST_OS || true
printf '%s\n' '# my config' 'alias ll="ls -l"' > "${HOME}/.bashrc"
cp "${HOME}/.bashrc" "${SANDBOX}/orig-bashrc"
run_install gregg
if [[ $STATUS -eq 0 ]] && head -n 2 "${HOME}/.bashrc" | diff - "${SANDBOX}/orig-bashrc" >/dev/null; then
  ok "bash profile preserves existing content byte-for-byte"
else
  fail "bash profile did not preserve existing content (status=$STATUS, out=$OUT)"
fi

# 8.6 macOS bash selection via deterministic helper input (no real Darwin host).
fresh_path_home "bash-macos-fresh"
path_without_dest
export SHELL="/bin/bash"
export GREGG_TEST_OS="Darwin"
rm -f "${HOME}/.bash_profile" "${HOME}/.bash_login" "${HOME}/.profile" "${HOME}/.bashrc"
run_install gregg
if [[ $STATUS -eq 0 && -f "${HOME}/.bash_profile" ]] && grep -Fq "added by gregg installer" "${HOME}/.bash_profile"; then
  ok "macOS bash fresh install creates ~/.bash_profile"
else
  fail "macOS bash fresh target (status=$STATUS, out=$OUT)"
fi
if [[ -e "${HOME}/.bashrc" ]]; then
  fail "macOS bash must not touch Linux ~/.bashrc"
else
  ok "macOS bash leaves Linux ~/.bashrc untouched"
fi

fresh_path_home "bash-macos-login"
path_without_dest
export SHELL="/bin/bash"
export GREGG_TEST_OS="Darwin"
printf '%s\n' '# login config' > "${HOME}/.bash_login"
rm -f "${HOME}/.bash_profile" "${HOME}/.profile" "${HOME}/.bashrc"
run_install gregg
if [[ $STATUS -eq 0 && -f "${HOME}/.bash_login" ]] && grep -Fq "added by gregg installer" "${HOME}/.bash_login" && [[ ! -e "${HOME}/.bash_profile" ]]; then
  ok "macOS bash honors an existing ~/.bash_login"
else
  fail "macOS bash login selection (status=$STATUS, out=$OUT)"
fi

fresh_path_home "bash-macos-profile"
path_without_dest
export SHELL="/bin/bash"
export GREGG_TEST_OS="Darwin"
printf '%s\n' '# profile config' > "${HOME}/.profile"
rm -f "${HOME}/.bash_profile" "${HOME}/.bash_login" "${HOME}/.bashrc"
run_install gregg
if [[ $STATUS -eq 0 && -f "${HOME}/.profile" ]] && grep -Fq "added by gregg installer" "${HOME}/.profile"; then
  ok "macOS bash honors an existing ~/.profile"
else
  fail "macOS bash profile selection (status=$STATUS, out=$OUT)"
fi
unset GREGG_TEST_OS || true

# 8.7 ZDOTDIR is honored for zsh when safe.
fresh_path_home "zsh-zdotdir"
path_without_dest
export SHELL="/bin/zsh"
mkdir -p "${HOME}/.config/zsh"
export ZDOTDIR="${HOME}/.config/zsh"
rm -f "${ZDOTDIR}/.zshrc" "${HOME}/.zshrc"
run_install gregg
if [[ $STATUS -eq 0 && -f "${ZDOTDIR}/.zshrc" ]] && grep -Fq "added by gregg installer" "${ZDOTDIR}/.zshrc" && [[ ! -e "${HOME}/.zshrc" ]]; then
  ok "zsh honors a safe exported ZDOTDIR"
else
  fail "zsh ZDOTDIR selection (status=$STATUS, out=$OUT)"
fi
unset ZDOTDIR || true

# 8.8 --no-shell-profile -> zero mutation plus exact manual guidance.
fresh_path_home "no-profile"
path_without_dest
export SHELL="/bin/zsh"
unset ZDOTDIR || true
rm -f "${HOME}/.zshrc" "${HOME}/.bashrc"
run_install --no-shell-profile gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "--no-shell-profile still installs the binary successfully"
else
  fail "--no-shell-profile install (status=$STATUS, out=$OUT)"
fi
if [[ ! -e "${HOME}/.zshrc" && ! -e "${HOME}/.bashrc" ]]; then
  ok "--no-shell-profile performs zero profile mutation"
else
  fail "--no-shell-profile mutated a profile"
fi
expect_contains "$OUT" "is not on the current PATH" "--no-shell-profile reports PATH absence"
expect_contains "$OUT" "export PATH=\"\$HOME/.local/bin:\$PATH\"" "--no-shell-profile prints the exact manual export"

# 8.9 unsupported shell -> zero mutation plus exact manual guidance.
fresh_path_home "unsupported"
path_without_dest
export SHELL="/bin/fish"
unset ZDOTDIR || true
rm -f "${HOME}/.zshrc" "${HOME}/.bashrc" "${HOME}/.bash_profile"
run_install gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "unsupported shell still installs the binary successfully"
else
  fail "unsupported shell install (status=$STATUS, out=$OUT)"
fi
if [[ ! -e "${HOME}/.zshrc" && ! -e "${HOME}/.bashrc" && ! -e "${HOME}/.bash_profile" ]]; then
  ok "unsupported shell performs zero profile mutation"
else
  fail "unsupported shell guessed a profile file"
fi
expect_contains "$OUT" "is not on the current PATH" "unsupported shell reports PATH absence truthfully"
expect_contains "$OUT" "export PATH=\"\$HOME/.local/bin:\$PATH\"" "unsupported shell prints the exact manual export"

# 8.10 destination already on PATH -> no unnecessary mutation.
fresh_path_home "on-path"
export SHELL="/bin/zsh"
unset ZDOTDIR || true
rm -f "${HOME}/.zshrc"
path_with_dest
run_install gregg
if [[ $STATUS -eq 0 ]]; then
  ok "on-PATH install exits 0"
else
  fail "on-PATH install (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "is available on the current PATH" "on-PATH install reports current-shell availability"
if [[ ! -e "${HOME}/.zshrc" ]]; then
  ok "on-PATH install performs no unnecessary profile mutation"
else
  fail "on-PATH install mutated a profile unnecessarily"
fi

# 8.11 invalid profile target -> install succeeds, failure reported truthfully.
fresh_path_home "invalid-profile"
path_without_dest
export SHELL="/bin/zsh"
unset ZDOTDIR || true
rm -f "${HOME}/.zshrc"
mkdir -p "${HOME}/.zshrc"
run_install gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "invalid profile target still installs the binary successfully"
else
  fail "invalid profile install (status=$STATUS, out=$OUT)"
fi
expect_contains "$OUT" "is not on the current PATH" "invalid profile reports PATH absence"
expect_contains "$OUT" "Could not update" "invalid profile reports the integration failure separately"
expect_contains "$OUT" "export PATH=\"\$HOME/.local/bin:\$PATH\"" "invalid profile prints the exact manual export"
rm -rf "${HOME}/.zshrc"

# 8.12 system installs never mutate profiles (forced without real root).
fresh_path_home "system"
path_without_dest
export SHELL="/bin/zsh"
unset ZDOTDIR || true
export GREGG_TEST_FORCE_SYSTEM="1"
rm -f "${HOME}/.zshrc" "${HOME}/.bashrc" "${HOME}/.bash_profile"
run_install gregg
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" ]]; then
  ok "system-mode install exits 0"
else
  fail "system-mode install (status=$STATUS, out=$OUT)"
fi
if [[ ! -e "${HOME}/.zshrc" && ! -e "${HOME}/.bashrc" && ! -e "${HOME}/.bash_profile" ]]; then
  ok "system install performs zero user-profile mutation"
else
  fail "system install mutated a user profile"
fi
unset GREGG_TEST_FORCE_SYSTEM || true

# 8.13 `both` performs exactly one PATH integration action.
fresh_path_home "both"
path_without_dest
export SHELL="/bin/zsh"
unset ZDOTDIR || true
unset GREGG_TEST_OS || true
rm -f "${HOME}/.zshrc"
run_install both
if [[ $STATUS -eq 0 && -x "${DEST_DIR}/gregg" && -x "${DEST_DIR}/greggd" ]]; then
  ok "both installs both binaries successfully"
else
  fail "both install (status=$STATUS, out=$OUT)"
fi
if [[ "$(grep -c "added by gregg installer" "${HOME}/.zshrc" || true)" == "1" ]]; then
  ok "both writes exactly one profile entry"
else
  fail "both profile entry count is not one"
fi
FUTURE_COUNT="$(count_occurrences "$OUT" "for future shells")"
if [[ "$FUTURE_COUNT" == "1" ]]; then
  ok "both reports exactly one PATH integration action"
else
  fail "both reported $FUTURE_COUNT PATH integrations (out=$OUT)"
fi

# --- summary -----------------------------------------------------------------------

echo ""
echo "pass=$PASS fail=$FAIL"
if [[ $FAIL -ne 0 ]]; then
  exit 1
fi
