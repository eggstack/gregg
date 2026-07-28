#!/usr/bin/env python3
"""Prepare final release inputs: validate, derive role records, materialize.

This shared helper is invoked by both the production finalizer
(``release-finalize.yml``) and the nonpublishing qualification harness.
It enforces that singleton role records are derived from the retrieved
postpublish artifact candidate declarations — never from manually copied
local files or direct paths.

Responsibilities (E2):
  1. Validate selection/retrieved-manifest identity.
  2. Locate the exact selected postpublish stage artifact.
  3. Read and validate its candidate.
  4. Derive singleton role records from candidate declarations.
  5. Verify extracted file path, digest, size, and containment.
  6. Verify complete artifact identity.
  7. Reject duplicate/missing roles.
  8. Materialize each required singleton.
  9. Verify copied identity.
  10. Write a role index with relative materialized paths.
  11. Print canonical aggregation input paths.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sys
from pathlib import Path
from typing import Any


SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class PreparationError(ValueError):
    """A final-input preparation step failed."""


def fail(message: str) -> None:
    raise PreparationError(message)


def digest(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def read_json(path: Path) -> Any:
    try:
        with path.open(encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read JSON {path}: {error}")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    with temporary.open("w", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, sort_keys=True)
        handle.write("\n")
        handle.flush()
    temporary.replace(path)


def validate_candidate(value: Any, *, expected_sha: str, expected_version: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail("candidate must be a JSON object")
    if value.get("schema_version") != 1:
        fail(f"unsupported candidate schema version: {value.get('schema_version')!r}")
    candidate_sha = value.get("candidate_sha")
    if not isinstance(candidate_sha, str) or not SHA_RE.fullmatch(candidate_sha):
        fail("candidate_sha must be a lowercase 40-character SHA")
    if candidate_sha != expected_sha:
        fail(f"candidate SHA {candidate_sha} does not match expected {expected_sha}")
    version = value.get("release_version")
    if not isinstance(version, str) or version != expected_version:
        fail(f"candidate release_version {version!r} does not match expected {expected_version}")
    stage = value.get("stage")
    if not isinstance(stage, str) or not stage:
        fail("candidate stage must be a nonempty string")
    artifacts = value.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        fail("candidate must declare at least one artifact")
    for index, artifact in enumerate(artifacts):
        if not isinstance(artifact, dict):
            fail(f"candidate artifacts[{index}] must be an object")
        if not isinstance(artifact.get("name"), str) or not artifact["name"]:
            fail(f"candidate artifacts[{index}] requires a nonempty name")
        if not isinstance(artifact.get("role"), str) or not artifact["role"]:
            fail(f"candidate artifacts[{index}] requires a nonempty role")
        if not isinstance(artifact.get("sha256"), str) or not SHA256_RE.fullmatch(artifact["sha256"]):
            fail(f"candidate artifacts[{index}] sha256 must be a lowercase 64-character SHA-256")
        if not isinstance(artifact.get("size_bytes"), int) or artifact["size_bytes"] <= 0:
            fail(f"candidate artifacts[{index}] size_bytes must be a positive integer")
        relative = artifact.get("path", artifact.get("name"))
        if not isinstance(relative, str) or not relative:
            fail(f"candidate artifacts[{index}] requires a path")
        relpath = Path(relative)
        if relpath.is_absolute() or ".." in relpath.parts:
            fail(f"candidate artifacts[{index}] path escapes artifact root")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-list", required=True, type=Path,
                        help="JSON array of singleton role declarations")
    parser.add_argument("--root", required=True, type=Path,
                        help="Extracted postpublish artifact root")
    parser.add_argument("--output", required=True, type=Path,
                        help="Output role index path")
    parser.add_argument("--materialize-dir", required=True, type=Path,
                        help="Directory for materialized singleton copies")
    parser.add_argument("--expected-candidate-sha", required=True,
                        help="Expected candidate SHA (lowercase 40-char)")
    parser.add_argument("--expected-version", required=True,
                        help="Expected release version")
    parser.add_argument("--required-role", action="append", default=[],
                        help="Required singleton role (may be repeated)")
    args = parser.parse_args()

    try:
        if not SHA_RE.fullmatch(args.expected_candidate_sha):
            fail("--expected-candidate-sha must be a lowercase 40-character SHA")

        root = args.root.resolve()
        if not root.is_dir():
            fail(f"postpublish extraction root is not a directory: {root}")

        entries = read_json(args.artifact_list)
        if not isinstance(entries, list):
            fail("artifact list must be an array")

        # E2.3: Derive singleton role records from candidate declarations.
        # The artifact list entries must carry role, name/path, sha256, size_bytes,
        # and complete artifact identity (stage, run, attempt, artifact_id, etc.).
        roles: dict[str, dict[str, Any]] = {}
        for entry in entries:
            if not isinstance(entry, dict):
                fail("every artifact entry must be an object")
            role = entry.get("role")
            if not isinstance(role, str) or not role:
                fail("every artifact entry requires a nonempty role")
            if role in roles:
                fail(f"duplicate singleton role: {role}")
            name = entry.get("name")
            if not isinstance(name, str) or not name:
                fail(f"artifact entry for role {role} requires a name")

            # E2.5: Verify extracted file path, digest, size, and containment.
            relpath = Path(name)
            if relpath.is_absolute() or ".." in relpath.parts:
                fail(f"role {role} path escapes artifact root: {name}")
            path = (root / relpath).resolve()
            if root != path.parent and root not in path.parents:
                fail(f"role {role} path escapes artifact root: {name}")
            if path.is_symlink() or not path.is_file():
                fail(f"role {role} file is missing or not a regular file: {name}")

            actual_sha, actual_size = digest(path)
            if entry.get("sha256") != actual_sha:
                fail(f"role {role} digest mismatch: expected {entry.get('sha256')}, got {actual_sha}")
            if entry.get("size_bytes") != actual_size:
                fail(f"role {role} size mismatch: expected {entry.get('size_bytes')}, got {actual_size}")

            # E2.6: Verify complete artifact identity.
            for field in ("stage", "workflow_run_id", "workflow_run_attempt",
                          "artifact_id", "artifact_name", "zip_sha256", "zip_size_bytes"):
                if entry.get(field) in (None, ""):
                    fail(f"role {role} is missing artifact identity field: {field}")

            record: dict[str, Any] = {
                "name": name,
                "path": relpath.as_posix(),
                "sha256": actual_sha,
                "size_bytes": actual_size,
                "stage": entry["stage"],
                "workflow_run_id": entry["workflow_run_id"],
                "workflow_run_attempt": entry["workflow_run_attempt"],
                "artifact_id": entry["artifact_id"],
                "artifact_name": entry["artifact_name"],
                "zip_sha256": entry["zip_sha256"],
                "zip_size_bytes": entry["zip_size_bytes"],
            }

            # E2.8: Materialize each required singleton.
            materialize_dir = args.materialize_dir.resolve()
            materialize_dir.mkdir(parents=True, exist_ok=True)
            destination = materialize_dir / f"{role}{path.suffix}"
            shutil.copyfile(path, destination)

            # E2.9: Verify copied identity.
            copied_sha, copied_size = digest(destination)
            if (copied_sha, copied_size) != (actual_sha, actual_size):
                fail(f"materialized role {role} changed identity: {copied_sha} != {actual_sha}")

            # E2.10: Write relative materialized paths.
            try:
                record["materialized_path"] = destination.relative_to(
                    args.output.parent.resolve()
                ).as_posix()
            except ValueError:
                record["materialized_path"] = str(destination)

            roles[role] = record

        # E2.7: Reject missing roles.
        if args.required_role:
            required = set(args.required_role)
            present = set(roles)
            missing = required - present
            if missing:
                fail(f"missing required singleton roles: {sorted(missing)}")

        if not roles:
            fail("artifact list is empty")

        index = {"schema_version": 1, "roles": roles}
        write_json(args.output, index)

        # E2.11: Print canonical aggregation input paths.
        print(f"role-index: {args.output}")
        print(f"materialize-dir: {args.materialize_dir}")
        for role, record in sorted(roles.items()):
            print(f"  {role}: {record['materialized_path']}")

        return 0
    except (OSError, json.JSONDecodeError, PreparationError) as error:
        print(f"FATAL: cannot prepare final release inputs: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
