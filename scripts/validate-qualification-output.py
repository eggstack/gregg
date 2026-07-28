#!/usr/bin/env python3
"""Fail closed when a qualification summary has missing or changed files."""
from __future__ import annotations
import argparse, hashlib, json, sys
from pathlib import Path

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", required=True, type=Path)
    args = parser.parse_args()
    try:
        summary = json.loads(args.summary.read_text(encoding="utf-8"))
        if summary.get("schema_version") != 1 or summary.get("verdict") != "pass" or not summary.get("files"):
            raise ValueError("qualification summary is incomplete")
        root = args.summary.parent.resolve()
        for item in summary["files"]:
            relative = Path(item["path"])
            if relative.is_absolute() or ".." in relative.parts:
                raise ValueError("qualification file path escapes output root")
            path = (root / relative).resolve()
            if root not in path.parents or not path.is_file():
                raise ValueError(f"missing qualification file: {relative}")
            raw = path.read_bytes()
            if hashlib.sha256(raw).hexdigest() != item.get("sha256") or len(raw) != item.get("size_bytes"):
                raise ValueError(f"qualification file digest/size mismatch: {relative}")
        print(f"validated qualification output: {args.summary}")
        return 0
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"FATAL: {error}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    raise SystemExit(main())
