#!/usr/bin/env python3
"""Validate and index singleton evidence files by their declared role."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path


def digest(path: Path) -> tuple[str, int]:
    h = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            h.update(chunk)
            size += len(chunk)
    return h.hexdigest(), size


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-list", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        entries = json.loads(args.artifact_list.read_text(encoding="utf-8"))
        if not isinstance(entries, list):
            raise ValueError("artifact list must be an array")
        roles: dict[str, dict[str, object]] = {}
        root = args.root.resolve()
        for entry in entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("name"), str) or not isinstance(entry.get("role"), str):
                raise ValueError("every artifact entry requires name and role")
            role = entry["role"]
            if not role or role in roles:
                raise ValueError(f"duplicate or empty artifact role: {role!r}")
            path = (root / entry["name"]).resolve()
            if root not in path.parents or not path.is_file():
                raise ValueError(f"declared evidence file is missing or escapes root: {entry['name']}")
            sha, size = digest(path)
            roles[role] = {"name": entry["name"], "path": str(path.relative_to(root)), "sha256": sha, "size_bytes": size}
        if not roles:
            raise ValueError("artifact list is empty")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        temporary = args.output.with_name(f".{args.output.name}.tmp")
        temporary.write_text(json.dumps({"schema_version": 1, "roles": roles}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        temporary.replace(args.output)
        return 0
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"FATAL: cannot materialize role-indexed evidence: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
