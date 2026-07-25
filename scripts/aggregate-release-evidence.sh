#!/usr/bin/env bash
# Canonical cross-run evidence aggregation wrapper.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "${SCRIPT_DIR}/validate-release-evidence.py" aggregate "$@"
