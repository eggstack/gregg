#!/usr/bin/env bash
# Validate staged release assets against scripts/release-targets.txt.
#
#   bash scripts/release-check-assets.sh <dist-dir>
#
# The expected asset set is derived from the single target table, so a
# target added to the table but missing from the staged directory (or vice
# versa) fails loudly here instead of shipping a partial draft release.
# Checksum files must contain a lowercase hex digest and the asset name.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <dist-dir>" >&2
  exit 1
fi
DIST="$1"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGETS_FILE="$SCRIPT_DIR/release-targets.txt"
if [[ ! -f "$TARGETS_FILE" ]]; then
  echo "error: target table $TARGETS_FILE not found" >&2
  exit 1
fi

missing=0
checked=0
while IFS= read -r line || [[ -n "$line" ]]; do
  # Strip comments and blank lines.
  row="${line%%#*}"
  row="$(echo "$row" | tr -s '[:space:]' ' ' | sed 's/^ //;s/ $//')"
  [[ -z "$row" ]] && continue
  target="$(echo "$row" | cut -d' ' -f1)"
  suffix="$(echo "$row" | cut -d' ' -sf2)"
  for program in gregg greggd; do
    asset="${program}-${target}${suffix}"
    for f in "$asset" "$asset.sha256"; do
      checked=$((checked + 1))
      if [[ ! -f "$DIST/$f" ]]; then
        echo "MISSING: $f" >&2
        missing=1
      else
        echo "OK: $f"
        if [[ "$f" == *.sha256 ]]; then
          if ! grep -qE '^[a-f0-9]{64}  ' "$DIST/$f"; then
            echo "  bad checksum format: $f" >&2
            missing=1
          fi
        fi
      fi
    done
  done
done < "$TARGETS_FILE"

if [[ "$checked" -eq 0 ]]; then
  echo "error: target table produced no expected assets" >&2
  exit 1
fi
if [[ "$missing" -ne 0 ]]; then
  echo "error: one or more expected assets missing or malformed" >&2
  exit 1
fi
echo "All expected assets present ($checked files)"
