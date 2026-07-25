#!/usr/bin/env python3
"""Create package-to-binary provenance from files on disk."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


SHA_RE = re.compile(r"^[0-9a-f]{40}$")


def digest(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--release-version", required=True)
    parser.add_argument("--package", action="append", nargs="+", metavar="PART")
    args = parser.parse_args()

    if not SHA_RE.fullmatch(args.candidate_sha):
        parser.error("--candidate-sha must be a lowercase full 40-character SHA")
    if not args.package:
        parser.error("at least one --package NAME ARCHIVE [BINARY] is required")

    packages = {}
    for item in args.package:
        if len(item) not in (2, 3):
            parser.error("--package takes NAME ARCHIVE [BINARY]")
        name, archive_name = item[:2]
        archive = Path(archive_name)
        if not archive.is_file():
            parser.error(f"archive does not exist: {archive}")
        archive_sha, archive_size = digest(archive)
        record = {
            "archive": archive.name,
            "sha256": archive_sha,
            "size_bytes": archive_size,
        }
        if len(item) == 3:
            binary = Path(item[2])
            if not binary.is_file():
                parser.error(f"binary does not exist: {binary}")
            binary_sha, binary_size = digest(binary)
            record.update(
                {
                    "installed_binary": binary.name,
                    "installed_binary_sha256": binary_sha,
                    "installed_binary_size_bytes": binary_size,
                }
            )
        if name in packages:
            parser.error(f"duplicate package: {name}")
        packages[name] = record

    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(f".{args.output.name}.tmp")
    with temporary.open("w", encoding="utf-8") as handle:
        json.dump(
            {
                "schema_version": 1,
                "candidate_sha": args.candidate_sha,
                "release_version": args.release_version,
                "packages": packages,
            },
            handle,
            indent=2,
            sort_keys=True,
        )
        handle.write("\n")
    temporary.replace(args.output)
    print(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
