#!/usr/bin/env python3
"""Decode and validate immutable workflow-dispatch selection input."""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import sys
from pathlib import Path

MAX_SELECTION_BYTES = 1024 * 1024


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base64", required=True)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--release-version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        raw = base64.b64decode(args.base64.encode("ascii"), validate=True)
        if not raw or len(raw) > MAX_SELECTION_BYTES:
            raise ValueError("selection is empty or exceeds the 1 MiB limit")
        if b"\0" in raw:
            raise ValueError("selection contains NUL bytes")
        text = raw.decode("utf-8")
        value = json.loads(text)
        if not isinstance(value, dict):
            raise ValueError("selection must be a JSON object")
        if value.get("candidate_sha") != args.candidate_sha:
            raise ValueError("selection candidate SHA does not match workflow input")
        if value.get("release_version") != args.release_version:
            raise ValueError("selection release version does not match workflow input")
        if not isinstance(value.get("runs"), dict) or not value["runs"]:
            raise ValueError("selection runs must be a nonempty object")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(f".{args.output.name}.tmp")
        temporary.write_bytes(raw)
        temporary.replace(args.output)
        digest = hashlib.sha256(raw).hexdigest()
        print(json.dumps({"sha256": digest, "size_bytes": len(raw)}, sort_keys=True))
        return 0
    except (binascii.Error, UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        print(f"FATAL: invalid selection input: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
