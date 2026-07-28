#!/usr/bin/env python3
"""Independently validate complete Phase-34 qualification output."""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


def read_json(path: Path, label: str) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {label}: {error}") from error


def resolve_file(root: Path, relative: Any, label: str) -> Path:
    if not isinstance(relative, str):
        raise ValueError(f"{label} path is missing")
    relpath = Path(relative)
    if relpath.is_absolute() or ".." in relpath.parts:
        raise ValueError(f"{label} path escapes output root")
    path = root / relpath
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise ValueError(f"missing {label}: {relative}: {error}") from error
    if (root != resolved.parent and root not in resolved.parents) or path.is_symlink() or not resolved.is_file():
        raise ValueError(f"{label} is missing or escapes output root: {relative}")
    return resolved


def identity(path: Path) -> tuple[str, int]:
    raw = path.read_bytes()
    return hashlib.sha256(raw).hexdigest(), len(raw)


def validate_file_index(summary: dict[str, Any], root: Path) -> None:
    files = summary.get("files")
    if not isinstance(files, list) or not files:
        raise ValueError("qualification summary is incomplete")
    seen: set[str] = set()
    for item in files:
        if not isinstance(item, dict):
            raise ValueError("qualification file index entry is invalid")
        path = resolve_file(root, item.get("path"), "qualification file")
        relative = path.relative_to(root).as_posix()
        if relative in seen:
            raise ValueError(f"duplicate qualification file: {relative}")
        seen.add(relative)
        if identity(path) != (item.get("sha256"), item.get("size_bytes")):
            raise ValueError(f"qualification file digest/size mismatch: {relative}")


def validate_complete(
    summary: dict[str, Any], root: Path, contract: dict[str, Any],
    requirements: dict[str, Any], dispatch: dict[str, Any],
    hosted_metadata_root: Path | None,
) -> None:
    if contract.get("schema_version") != 1 or contract.get("release_version") != "1.0.1":
        raise ValueError("qualification contract is invalid")
    if summary.get("candidate_sha") is None or summary.get("release_version") != contract["release_version"]:
        raise ValueError("qualification summary release identity is invalid")
    chains = summary.get("chains")
    if not isinstance(chains, dict) or set(contract["required_chains"]) - set(chains):
        raise ValueError("qualification summary omits a required chain")

    stage_sets = summary.get("stage_sets")
    if not isinstance(stage_sets, dict):
        raise ValueError("qualification stage sets are missing")
    expected_sets = {
        "pre_tag": requirements["pre_tag_required_stages"],
        "protocol_publication": requirements["protocol_publication_required_stages"],
        "final": requirements["final_required_stages"],
    }
    for name, expected in expected_sets.items():
        if stage_sets.get(name) != expected:
            raise ValueError(f"qualification {name} stage set does not match production requirements")
    reachable = {
        stage for value in dispatch.get("dispatches", {}).values()
        for stage in value.get("required_stages", [])
    }
    if not set(expected_sets["final"]).issubset(reachable):
        raise ValueError("production requirements contain unreachable stages")

    candidate_manifest = resolve_file(root, chains["candidate_pre_tag"].get("manifest_path"), "candidate manifest")
    final_manifest = resolve_file(root, chains["final"].get("manifest_path"), "final manifest")
    for path, key in ((candidate_manifest, "pre_tag"), (final_manifest, "final")):
        manifest = read_json(path, f"{key} manifest")
        if manifest.get("required_stages") != expected_sets[key] or manifest.get("verdict") != "pass":
            raise ValueError(f"{key} manifest does not contain the exact production stage contract")

    boundary = chains.get("boundary_2")
    if not isinstance(boundary, list):
        raise ValueError("Boundary-2 results are missing")
    by_package = {item.get("package"): item for item in boundary if isinstance(item, dict)}
    if set(by_package) != set(contract["required_boundary2_packages"]):
        raise ValueError("Boundary-2 package set is incomplete")
    for package, item in by_package.items():
        result_path = resolve_file(root, item.get("summary_path"), f"{package} Boundary-2 result")
        result = read_json(result_path, f"{package} Boundary-2 result")
        registry_checksum = result.get("protocol_registry_checksum")
        if (
            not isinstance(registry_checksum, str)
            or len(registry_checksum) != 64
            or result.get("lockfile_protocol_checksum") != registry_checksum
            or result.get("checksum_match") is not True
            or result.get("verification") != "pass"
        ):
            raise ValueError(f"{package} Boundary-2 checksum contract failed")
        index_path = resolve_file(root, item.get("index_path"), f"{package} command index")
        completed = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("registry-reverify.py")),
             "validate-index", "--index", str(index_path), "--package", package],
            text=True, capture_output=True, check=False,
        )
        if completed.returncode != 0:
            raise ValueError(f"{package} command index is invalid: {completed.stderr.strip()}")
        index = read_json(index_path, f"{package} command index")
        if [item["name"] for item in index["commands"]] != contract["required_command_names"]:
            raise ValueError(f"{package} command index command set is invalid")
    protocol_chain = chains["protocol_publication"]
    protocol_index = read_json(
        resolve_file(root, protocol_chain.get("protocol_index_path"), "protocol index evidence"),
        "protocol index evidence",
    )
    if (
        protocol_chain.get("stages") != expected_sets["protocol_publication"]
        or protocol_index.get("checksum_match") is not True
        or protocol_index.get("registry_checksum") != protocol_index.get("archive_sha256")
        or set(protocol_index.get("consumer_resolution", {})) != set(contract["required_boundary2_packages"])
    ):
        raise ValueError("protocol-publication chain is incomplete or inconsistent")

    role_index = read_json(
        resolve_file(root, chains["final"].get("role_index_path"), "singleton role index"),
        "singleton role index",
    )
    role_index_path = resolve_file(
        root, chains["final"].get("role_index_path"), "singleton role index"
    )
    roles = role_index.get("roles", {})
    if set(contract["required_singleton_roles"]) - set(roles):
        raise ValueError("singleton role index omits a required role")
    for role in contract["required_singleton_roles"]:
        record = roles[role]
        for field in (
            "stage", "workflow_run_id", "workflow_run_attempt", "artifact_id",
            "artifact_name", "zip_sha256", "zip_size_bytes", "sha256",
            "size_bytes", "materialized_path",
        ):
            if record.get(field) in (None, ""):
                raise ValueError(f"singleton role {role} lacks {field}")
        materialized = resolve_file(
            role_index_path.parent.resolve(), record["materialized_path"],
            f"materialized singleton role {role}",
        )
        if identity(materialized) != (record["sha256"], record["size_bytes"]):
            raise ValueError(f"materialized singleton role {role} identity mismatch")

    for field in (
        "selection_path", "selection_identity_path", "disposition_decision_path",
        "disposition_identity_path",
    ):
        resolve_file(root, chains["final"].get(field), field)

    negative = summary.get("negative_cases")
    if not isinstance(negative, list):
        raise ValueError("qualification negative cases are missing")
    negative_map = {item.get("case"): item for item in negative if isinstance(item, dict)}
    if set(negative_map) != set(contract["required_negative_cases"]):
        raise ValueError("qualification negative-case set does not match contract")
    if not all(item.get("failed") is True for item in negative_map.values()):
        raise ValueError("one or more qualification negative cases did not fail as required")

    if hosted_metadata_root:
        for name in contract["required_hosted_metadata"]:
            path = hosted_metadata_root / f"{name}.txt"
            if not path.is_file() or not path.read_text(encoding="utf-8").strip():
                raise ValueError(f"hosted metadata missing: {name}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", required=True, type=Path)
    parser.add_argument("--contract", type=Path)
    parser.add_argument("--requirements", type=Path)
    parser.add_argument("--dispatch-contract", type=Path)
    parser.add_argument("--hosted-metadata-root", type=Path)
    args = parser.parse_args()
    try:
        summary = read_json(args.summary, "qualification summary")
        if not isinstance(summary, dict) or summary.get("schema_version") != 1 or summary.get("verdict") != "pass":
            raise ValueError("qualification summary is incomplete")
        root = args.summary.parent.resolve()
        validate_file_index(summary, root)
        supplied = (args.contract, args.requirements, args.dispatch_contract)
        if any(supplied) and not all(supplied):
            raise ValueError("contract, requirements, and dispatch contract must be supplied together")
        if all(supplied):
            validate_complete(
                summary, root,
                read_json(args.contract, "qualification contract"),
                read_json(args.requirements, "release requirements"),
                read_json(args.dispatch_contract, "dispatch contract"),
                args.hosted_metadata_root,
            )
        print(f"validated qualification output: {args.summary}")
        return 0
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"FATAL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
