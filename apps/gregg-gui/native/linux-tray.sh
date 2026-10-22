#!/usr/bin/env bash
#
# The Linux half of the GUI's native system-tray exercise.
#
# Slint implements the Linux system tray with the `ksni` crate, which is a
# StatusNotifierItem *client*. A client is useless without a StatusNotifierItem
# *host*, and a headless CI runner has none: without a host, Slint silently has
# no tray at all, and a green build would prove nothing about the tray.
#
# This script therefore does three things:
#
#   1. starts the repository's own counted fleet fixture and writes a Gregg
#      configuration that points at it, so the tray has real fleet state to
#      carry;
#   2. runs the `sni-host` helper in `--mode with-host`, which owns
#      `org.kde.StatusNotifierWatcher`, starts the GUI, and asserts that the
#      GUI registered an item whose tooltip carries the live fleet state,
#      whose menu is the context menu, and whose pulldown was placed inside
#      the work area it reported;
#   3. runs the same helper in `--mode no-host`, in a *fresh* session bus with
#      no tray host at all, and asserts that the GUI reports
#      `tray=unavailable` and keeps running instead of dying.
#
# Each pass runs under its own `dbus-run-session`, so the second pass is
# guaranteed to find no watcher name: a fresh session bus has no owner for
# `org.kde.StatusNotifierWatcher`.
#
# The order matters: the host has to exist BEFORE the GUI starts, because
# `ksni` registers once at startup and gives up if no watcher answers then.
#
# Nothing here is skipped. If there is no display, no session bus, or no
# fixture, the script fails loudly.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../../.." && pwd)"
readonly REPO_ROOT

GUI=""
GREGG=""
HELPER=""
PYTHON="python3"
FIXTURE="${REPO_ROOT}/scripts/tests/fleet-fixture.py"
# The name the fixture host is configured under. The tray's tooltip names the
# hosts it shows, so this is what proves the tray carries the *fleet* and
# not a placeholder.
readonly HOST_NAME="gui-fixture"
# What the tray's tooltip is expected to say about a healthy single-host
# fleet. The wording comes from the GUI's own `tray_status()`.
readonly EXPECT_STATUS="1 online"

WORK_DIR=""
FIXTURE_PID=""

usage() {
    cat >&2 <<'EOF'
usage: linux-tray.sh --gui <path> --gregg <path> --helper <path>
                     [--python <path>] [--fixture <path>]

  --gui     the gregg-gui executable under test
  --gregg   the gregg client executable the GUI launches its daemon with
  --helper  the sni-host helper executable
  --python  the python interpreter used for the fleet fixture (default python3)
  --fixture the fleet fixture script (default scripts/tests/fleet-fixture.py)
EOF
}

log() {
    printf 'linux-tray: %s\n' "$*"
}

fail() {
    printf 'linux-tray: FAIL %s\n' "$*" >&2
    exit 1
}

need_value() {
    # $1 is the flag, $2 the number of remaining arguments.
    if [ "$2" -lt 2 ]; then
        fail "$1 needs a value"
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --gui) need_value "$1" "$#"; GUI="$2"; shift 2 ;;
        --gregg) need_value "$1" "$#"; GREGG="$2"; shift 2 ;;
        --helper) need_value "$1" "$#"; HELPER="$2"; shift 2 ;;
        --python) need_value "$1" "$#"; PYTHON="$2"; shift 2 ;;
        --fixture) need_value "$1" "$#"; FIXTURE="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage; fail "unrecognized argument: $1" ;;
    esac
done

[ -n "${GUI}" ] || fail "--gui is required"
[ -n "${GREGG}" ] || fail "--gregg is required"
[ -n "${HELPER}" ] || fail "--helper is required"

[ -x "${GUI}" ] || fail "the GUI executable ${GUI} is missing or not executable"
[ -x "${GREGG}" ] || fail "the gregg executable ${GREGG} is missing or not executable"
[ -x "${HELPER}" ] || fail "the helper executable ${HELPER} is missing or not executable"
[ -f "${FIXTURE}" ] || fail "the fleet fixture ${FIXTURE} is missing"

command -v "${PYTHON}" >/dev/null 2>&1 || fail "no ${PYTHON} on PATH for the fleet fixture"
command -v dbus-run-session >/dev/null 2>&1 || fail "dbus-run-session is missing (install dbus-x11)"

# The GUI creates a real window, so it needs a display. Failing here is much
# clearer than a timeout deep inside the helper.
if [ -z "${DISPLAY:-}" ]; then
    fail "DISPLAY is not set: the GUI needs an X display (run this under xvfb-run)"
fi

cleanup() {
    local status=$?
    if [ -n "${FIXTURE_PID}" ]; then
        kill "${FIXTURE_PID}" >/dev/null 2>&1 || true
        wait "${FIXTURE_PID}" >/dev/null 2>&1 || true
    fi
    if [ -n "${GREGG}" ] && [ -n "${WORK_DIR}" ] && [ -f "${WORK_DIR}/gregg.toml" ]; then
        # The GUI launches a per-config client daemon; it is detached, so it
        # outlives the GUI and has to be stopped explicitly.
        "${GREGG}" --config "${WORK_DIR}/gregg.toml" daemon stop >/dev/null 2>&1 || true
    fi
    if [ -n "${WORK_DIR}" ]; then
        rm -rf "${WORK_DIR}"
    fi
    exit "${status}"
}
trap cleanup EXIT

WORK_DIR="$(mktemp -d)"
log "work directory ${WORK_DIR}"

# ---------------------------------------------------------------- the fixture
log "starting the fleet fixture"
"${PYTHON}" "${FIXTURE}" \
    --port 0 \
    --log "${WORK_DIR}/fixture.log" \
    --default-mode healthy \
    >"${WORK_DIR}/fixture.port" 2>"${WORK_DIR}/fixture.err" &
FIXTURE_PID=$!

PORT=""
for _ in $(seq 1 100); do
    if ! kill -0 "${FIXTURE_PID}" >/dev/null 2>&1; then
        cat "${WORK_DIR}/fixture.err" >&2 || true
        fail "the fleet fixture exited before it reported a port"
    fi
    if grep -q '^PORT=' "${WORK_DIR}/fixture.port" 2>/dev/null; then
        PORT="$(sed -n 's/^PORT=//p' "${WORK_DIR}/fixture.port" | head -n 1)"
        break
    fi
    sleep 0.1
done
[ -n "${PORT}" ] || fail "the fleet fixture never reported a port"
log "the fleet fixture is listening on 127.0.0.1:${PORT}"

# ------------------------------------------------------------- the config
SYSTEM_ID="$("${PYTHON}" -c 'import uuid; print(uuid.uuid4())')"
cat >"${WORK_DIR}/gregg.toml" <<EOF
config_version = 1
refresh_seconds = 5
request_timeout_ms = 1500
max_concurrent_requests = 16
default_port = 8123

[[systems]]
id = "${SYSTEM_ID}"
host = "127.0.0.1"
port = ${PORT}
name = "${HOST_NAME}"
EOF
log "wrote ${WORK_DIR}/gregg.toml"

# ------------------------------------------------------------- pass one
log "pass 1: a StatusNotifierItem host is present"
dbus-run-session -- "${HELPER}" \
    --mode with-host \
    --gui "${GUI}" \
    --gregg "${GREGG}" \
    --config "${WORK_DIR}/gregg.toml" \
    --expect-status "${EXPECT_STATUS}" \
    --expect-host-name "${HOST_NAME}" \
    --timeout-secs 90

# ------------------------------------------------------------- pass two
# A fresh session bus: nothing owns org.kde.StatusNotifierWatcher on it.
log "pass 2: no StatusNotifierItem host is present"
dbus-run-session -- "${HELPER}" \
    --mode no-host \
    --gui "${GUI}" \
    --gregg "${GREGG}" \
    --config "${WORK_DIR}/gregg.toml" \
    --expect-status "${EXPECT_STATUS}" \
    --expect-host-name "${HOST_NAME}" \
    --timeout-secs 90

log "PASS both Linux tray passes"
