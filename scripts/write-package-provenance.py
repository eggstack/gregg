#!/usr/bin/env python3
"""Create package-to-binary provenance from files on disk.

Each package record carries the archive checksum/size, optional installed binary
checksum/size, optional verification lockfile identity, and optional extended
metadata fields (normalized manifest, package list, transcripts, etc.).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

# Optional metadata fields that may be attached to a package record.
OPTIONAL_FIELDS = {
    "normalized_manifest_sha256",
    "package_list_sha256",
    "test_transcript_sha256",
    "msrv_transcript_sha256",
    "install_transcript_sha256",
    "version_output_sha256",
    "smoke_transcript_sha256",
}


def digest(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def declared_path(path: Path) -> str:
    """Return a portable relative evidence path; never emit an absolute path."""
    try:
        return str(path.resolve().relative_to(Path.cwd().resolve()))
    except ValueError:
        return path.name


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--release-version", required=True)
    parser.add_argument("--package", action="append", nargs="+", metavar="PART")
    parser.add_argument("--metadata", type=Path, help="JSON file with optional metadata fields per package")
    args = parser.parse_args()

    if not SHA_RE.fullmatch(args.candidate_sha):
        parser.error("--candidate-sha must be a lowercase full 40-character SHA")
    if not args.package:
        parser.error("at least one --package NAME ARCHIVE [BINARY] [LOCKFILE] is required")

    metadata: dict[str, dict[str, str]] = {}
    if args.metadata:
        if not args.metadata.is_file():
            parser.error(f"metadata file does not exist: {args.metadata}")
        metadata = json.loads(args.metadata.read_text(encoding="utf-8"))
        if not isinstance(metadata, dict):
            parser.error("metadata must be a JSON object keyed by package name")

    packages = {}
    for item in args.package:
        if len(item) not in (2, 3, 4):
            parser.error("--package takes NAME ARCHIVE [BINARY] [LOCKFILE]")
        name, archive_name = item[:2]
        archive = Path(archive_name)
        if not archive.is_file():
            parser.error(f"archive does not exist: {archive}")
        archive_sha, archive_size = digest(archive)
        record: dict[str, object] = {
            "archive": archive.name,
            "archive_path": declared_path(archive),
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
                    "installed_binary_path": declared_path(binary),
                    "installed_binary_sha256": binary_sha,
                    "installed_binary_size_bytes": binary_size,
                }
            )
        if len(item) == 4:
            lockfile = Path(item[3])
            if not lockfile.is_file():
                parser.error(f"lockfile does not exist: {lockfile}")
            lockfile_sha, lockfile_size = digest(lockfile)
            record.update(
                {
                    "verification_lockfile": lockfile.name,
                    "verification_lockfile_path": declared_path(lockfile),
                    "verification_lockfile_sha256": lockfile_sha,
                    "verification_lockfile_size_bytes": lockfile_size,
                }
            )
        # Attach optional metadata fields for this package.
        pkg_meta = metadata.get(name, {})
        if not isinstance(pkg_meta, dict):
            parser.error(f"metadata for {name} must be a JSON object")
        for field, value in pkg_meta.items():
            if field not in OPTIONAL_FIELDS:
                parser.error(f"unknown metadata field: {field}")
            if not isinstance(value, str) or not SHA256_RE.fullmatch(value):
                parser.error(f"metadata field {field} must be a lowercase 64-character SHA-256")
            record[field] = value
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
