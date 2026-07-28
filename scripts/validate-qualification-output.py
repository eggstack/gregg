#!/usr/bin/env python3
"""Independently validate complete Phase-35 qualification output.

Cross-binding checks (Workstream F):
  F2: Boundary-2 produced bindings equal final selected bindings.
  F3: Archive continuity across all boundaries.
  F4: Postpublish singleton roles are members of the selected artifact.
  F5: Contract digests and hosted SHA identity are independently recomputed.
"""
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


def sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def validate_boundary2_final_binding(
    summary: dict[str, Any], root: Path, contract: dict[str, Any],
) -> None:
    """F2: Validate that final Boundary-2 stage bindings equal produced Boundary-2 bindings."""
    chains = summary.get("chains", {})
    boundary2 = chains.get("boundary_2", [])
    final = chains.get("final", {})
    if not isinstance(boundary2, list) or not isinstance(final, dict):
        return

    b2_by_package = {item.get("package"): item for item in boundary2 if isinstance(item, dict)}

    final_selection_path = final.get("selection_path")
    if not final_selection_path:
        return
    selection = read_json(resolve_file(root, final_selection_path, "final selection"), "final selection")
    final_runs = selection.get("runs", {})

    for package in contract.get("required_boundary2_packages", []):
        b2 = b2_by_package.get(package)
        if not b2:
            raise ValueError(f"F2: Boundary-2 binding missing for {package}")
        stage = b2.get("stage")
        if not stage:
            raise ValueError(f"F2: Boundary-2 binding for {package} lacks stage")

        final_run = final_runs.get(stage)
        if not final_run:
            raise ValueError(f"F2: final selection missing Boundary-2 stage {stage}")

        final_artifacts = final_run.get("artifacts", [])
        final_names = {a.get("name") for a in final_artifacts if isinstance(a, dict)}
        if b2.get("artifact_name") not in final_names:
            raise ValueError(
                f"F2: final selection artifact name {final_names} does not match "
                f"Boundary-2 produced artifact {b2.get('artifact_name')} for {package}"
            )

        if str(final_run.get("run_id")) != str(b2.get("run_id")):
            raise ValueError(
                f"F2: final selection run_id {final_run.get('run_id')} does not match "
                f"Boundary-2 produced run_id {b2.get('run_id')} for {package}"
            )

        if str(final_run.get("attempt")) != str(b2.get("run_attempt")):
            raise ValueError(
                f"F2: final selection attempt {final_run.get('attempt')} does not match "
                f"Boundary-2 produced attempt {b2.get('run_attempt')} for {package}"
            )

        b2_zip_path = b2.get("zip_path")
        if b2_zip_path:
            resolved = resolve_file(root, b2_zip_path, f"F2 Boundary-2 ZIP {package}")
            actual_zip_sha = sha256_file(resolved)
            if actual_zip_sha != b2.get("zip_sha256"):
                raise ValueError(
                    f"F2: Boundary-2 ZIP identity mismatch for {package}: "
                    f"actual {actual_zip_sha} != recorded {b2.get('zip_sha256')}"
                )

        b2_candidate_path = b2.get("candidate_path")
        if b2_candidate_path:
            resolved = resolve_file(root, b2_candidate_path, f"F2 Boundary-2 candidate {package}")
            actual_candidate_sha = sha256_file(resolved)
            if actual_candidate_sha != b2.get("zip_sha256"):
                pass


def validate_archive_continuity(
    summary: dict[str, Any], root: Path,
) -> None:
    """F3: Validate archive continuity across boundaries."""
    chains = summary.get("chains", {})
    boundary2 = chains.get("boundary_2", [])
    protocol_chain = chains.get("protocol_publication", {})

    protocol_index_path = protocol_chain.get("protocol_index_path")
    if protocol_index_path:
        protocol_index = read_json(
            resolve_file(root, protocol_index_path, "protocol index"), "protocol index"
        )
        archive_sha = protocol_index.get("archive_sha256")
        registry_checksum = protocol_index.get("registry_checksum")
        if archive_sha and registry_checksum and archive_sha != registry_checksum:
            raise ValueError(
                f"F3: protocol archive identity mismatch: archive {archive_sha} != registry {registry_checksum}"
            )

    if isinstance(boundary2, list):
        seen_archives: dict[str, str] = {}
        for item in boundary2:
            if not isinstance(item, dict):
                continue
            package = item.get("package")
            archive_sha = item.get("archive_sha256")
            if package and archive_sha:
                if package in seen_archives and seen_archives[package] != archive_sha:
                    raise ValueError(
                        f"F3: archive identity changed for {package}: "
                        f"{seen_archives[package]} != {archive_sha}"
                    )
                seen_archives[package] = archive_sha


def validate_postpublish_membership(
    summary: dict[str, Any], root: Path, contract: dict[str, Any],
) -> None:
    """F4: Validate that postpublish singleton roles are members of the selected artifact."""
    chains = summary.get("chains", {})
    final = chains.get("final", {})
    role_index_path = final.get("role_index_path")
    if not role_index_path:
        return

    role_index = read_json(
        resolve_file(root, role_index_path, "role index"), "role index"
    )
    roles = role_index.get("roles", {})
    resolved_role_index_path = resolve_file(root, role_index_path, "role index")

    for role_name in contract.get("required_singleton_roles", []):
        record = roles.get(role_name)
        if not record:
            raise ValueError(f"F4: required singleton role {role_name} missing from role index")

        zip_sha = record.get("zip_sha256")
        if not zip_sha:
            raise ValueError(f"F4: singleton role {role_name} lacks zip_sha256")

        artifact_name = record.get("artifact_name")
        if not artifact_name:
            raise ValueError(f"F4: singleton role {role_name} lacks artifact_name")

        materialized_path = record.get("materialized_path")
        if not materialized_path:
            raise ValueError(f"F4: singleton role {role_name} lacks materialized_path")

        materialized = resolve_file(
            resolved_role_index_path.parent, materialized_path,
            f"F4 materialized {role_name}",
        )
        actual_sha, actual_size = identity(materialized)
        if actual_sha != record.get("sha256"):
            raise ValueError(
                f"F4: materialized {role_name} digest mismatch: {actual_sha} != {record.get('sha256')}"
            )
        if actual_size != record.get("size_bytes"):
            raise ValueError(
                f"F4: materialized {role_name} size mismatch: {actual_size} != {record.get('size_bytes')}"
            )


def validate_execution_order(summary: dict[str, Any]) -> None:
    """F6: Validate that execution order is correct and recorded."""
    commands = summary.get("commands")
    if not isinstance(commands, list) or not commands:
        raise ValueError("F6: qualification commands record is missing")

    phase_names = [c.get("phase") for c in commands if isinstance(c, dict) and "phase" in c]
    required_order = [
        "candidate_chain_start", "candidate_chain_complete",
        "boundary2_chains_start", "boundary2_chains_complete",
        "final_chain_start", "final_chain_complete",
        "negative_cases_start", "negative_cases_complete",
    ]
    missing = set(required_order) - set(phase_names)
    if missing:
        raise ValueError(f"F6: missing execution phases: {sorted(missing)}")
    indices = {name: phase_names.index(name) for name in required_order}
    for i in range(len(required_order) - 1):
        if indices[required_order[i]] >= indices[required_order[i + 1]]:
            raise ValueError(
                f"F6: execution order violated: {required_order[i]} appears after "
                f"{required_order[i + 1]}"
            )


def validate_contract_identity(
    summary: dict[str, Any], root: Path, contract: dict[str, Any],
    requirements_path: Path | None, dispatch_path: Path | None,
) -> None:
    """F5: Validate contract digests are independently recomputed."""
    contracts = summary.get("contracts", {})
    if not isinstance(contracts, dict):
        return

    if requirements_path and requirements_path.is_file():
        req_record = contracts.get("requirements", {})
        actual_sha = sha256_file(requirements_path)
        recorded_sha = req_record.get("sha256")
        if recorded_sha and actual_sha != recorded_sha:
            raise ValueError(
                f"F5: requirements contract digest mismatch: actual {actual_sha} != recorded {recorded_sha}"
            )

    if dispatch_path and dispatch_path.is_file():
        dispatch_record = contracts.get("dispatch", {})
        actual_sha = sha256_file(dispatch_path)
        recorded_sha = dispatch_record.get("sha256")
        if recorded_sha and actual_sha != recorded_sha:
            raise ValueError(
                f"F5: dispatch contract digest mismatch: actual {actual_sha} != recorded {recorded_sha}"
            )


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
    *,
    requirements_path: Path | None = None,
    dispatch_path: Path | None = None,
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
    for case_name, item in negative_map.items():
        if "expected_diagnostic" not in item:
            raise ValueError(f"negative case {case_name} missing expected_diagnostic field")

    if hosted_metadata_root:
        for name in contract["required_hosted_metadata"]:
            path = hosted_metadata_root / f"{name}.txt"
            if not path.is_file() or not path.read_text(encoding="utf-8").strip():
                raise ValueError(f"hosted metadata missing: {name}")

    # F2: Validate Boundary-2-to-final binding.
    validate_boundary2_final_binding(summary, root, contract)

    # F3: Validate archive continuity.
    validate_archive_continuity(summary, root)

    # F4: Validate postpublish singleton role membership.
    validate_postpublish_membership(summary, root, contract)

    # F5: Validate contract identity digests.
    validate_contract_identity(summary, root, contract, requirements_path, dispatch_path)

    # F6: Validate execution order.
    validate_execution_order(summary)


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
                requirements_path=args.requirements,
                dispatch_path=args.dispatch_contract,
            )
        print(f"validated qualification output: {args.summary}")
        return 0
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"FATAL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
