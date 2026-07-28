#!/usr/bin/env python3
"""Validate canonical Gregg release evidence and aggregate immutable stages."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
RESULTS = {"success", "failure", "skipped"}
SOURCE_MODES = {"pre-tag-full-sha", "annotated-tag"}
SELECTION_SOURCES = {"selection-file", "workflow-dispatch-base64"}


class EvidenceError(ValueError):
    """A candidate or release manifest failed validation."""


def fail(message: str) -> None:
    raise EvidenceError(message)


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


def string(data: dict[str, Any], name: str) -> str:
    value = data.get(name)
    if not isinstance(value, str) or not value.strip():
        fail(f"{name} must be a nonempty string")
    return value


def sha(value: Any, name: str) -> str:
    if not isinstance(value, str) or not SHA_RE.fullmatch(value):
        fail(f"{name} must be a lowercase full 40-character commit SHA")
    return value


def sha256(value: Any, name: str) -> str:
    if not isinstance(value, str) or not SHA256_RE.fullmatch(value):
        fail(f"{name} must be a lowercase 64-character SHA-256")
    return value


def timestamp(value: Any, name: str) -> dt.datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        fail(f"{name} must be an RFC3339 UTC timestamp ending in Z")
    try:
        parsed = dt.datetime.fromisoformat(value[:-1] + "+00:00")
    except ValueError as error:
        fail(f"{name} is not a valid timestamp: {error}")
    if parsed.utcoffset() != dt.timedelta(0):
        fail(f"{name} must be UTC")
    return parsed


def validate_candidate(
    value: Any,
    *,
    expected_sha: str | None = None,
    expected_version: str | None = None,
    expected_architecture: str | None = None,
    expected_source_mode: str | None = None,
    require_success: bool = True,
) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail("candidate metadata must be a JSON object")
    if value.get("schema_version") != SCHEMA_VERSION:
        fail(f"unsupported evidence schema version: {value.get('schema_version')!r}")

    candidate_sha = sha(value.get("candidate_sha"), "candidate_sha")
    if expected_sha is not None and candidate_sha != expected_sha:
        fail(f"candidate SHA {candidate_sha} does not match expected {expected_sha}")

    version = string(value, "release_version")
    if not VERSION_RE.fullmatch(version):
        fail(f"invalid release_version: {version}")
    if expected_version is not None and version != expected_version:
        fail(f"release version {version} does not match expected {expected_version}")

    for name in (
        "stage",
        "workflow_run_id",
        "workflow_run_attempt",
        "job_name",
        "runner_os",
        "runner_architecture",
    ):
        string(value, name)
    if expected_architecture is not None and value["runner_architecture"] != expected_architecture:
        fail(
            f"architecture {value['runner_architecture']} does not match "
            f"{expected_architecture}"
        )

    source_mode = value.get("source_identity_mode")
    if source_mode not in SOURCE_MODES:
        fail(f"source_identity_mode must be one of {sorted(SOURCE_MODES)}")
    if expected_source_mode is not None and source_mode != expected_source_mode:
        fail(f"source_identity_mode {source_mode} does not match expected {expected_source_mode}")

    started = timestamp(value.get("started_at"), "started_at")
    completed = timestamp(value.get("completed_at"), "completed_at")
    if completed < started:
        fail("completed_at precedes started_at")

    if value.get("result") not in RESULTS:
        fail(f"result must be one of {sorted(RESULTS)}")
    if require_success and value["result"] != "success":
        fail(f"qualifying evidence result is {value['result']!r}, not success")

    source = value.get("source")
    if not isinstance(source, dict):
        fail("source must be an object")
    string(source, "ref_input")
    tag_object_sha = source.get("tag_object_sha")
    if tag_object_sha is not None:
        sha(tag_object_sha, "source.tag_object_sha")
    if sha(source.get("peeled_commit_sha"), "source.peeled_commit_sha") != candidate_sha:
        fail("source.peeled_commit_sha must equal candidate_sha")

    if source_mode == "annotated-tag":
        for field in ("tagger_name", "tagger_email", "tagger_timestamp", "tag_object_content_sha256"):
            val = source.get(field)
            if not isinstance(val, str) or not val.strip():
                fail(f"annotated-tag source.{field} must be a nonempty string")
        timestamp(source.get("tagger_timestamp"), "source.tagger_timestamp")
        sha256(source.get("tag_object_content_sha256"), "source.tag_object_content_sha256")
    else:
        for field in ("tagger_name", "tagger_email", "tagger_timestamp", "tag_object_content_sha256"):
            if source.get(field) is not None:
                fail(f"pre-tag-full-sha source.{field} must be null, not present")

    artifacts = value.get("artifacts")
    if not isinstance(artifacts, list):
        fail("artifacts must be an array")
    for index, artifact in enumerate(artifacts):
        if not isinstance(artifact, dict):
            fail(f"artifacts[{index}] must be an object")
        string(artifact, "name")
        string(artifact, "role")
        if "artifact_id" in artifact:
            string(artifact, "artifact_id")
        if "sha256" in artifact and artifact["sha256"] is not None:
            sha256(artifact["sha256"], f"artifacts[{index}].sha256")
        if "size_bytes" in artifact and artifact["size_bytes"] is not None:
            if not isinstance(artifact["size_bytes"], int) or artifact["size_bytes"] < 0:
                fail(f"artifacts[{index}].size_bytes must be a non-negative integer")
        if "media_type" in artifact and artifact["media_type"] is not None:
            string(artifact, "media_type")
        if "path" in artifact and artifact["path"] is not None:
            string(artifact, "path")

    executables = value.get("executables")
    if not isinstance(executables, list):
        fail("executables must be an array")
    for index, executable in enumerate(executables):
        if not isinstance(executable, dict):
            fail(f"executables[{index}] must be an object")
        string(executable, "name")
        sha256(executable.get("sha256"), f"executables[{index}].sha256")
        if not isinstance(executable.get("size_bytes"), int) or executable["size_bytes"] <= 0:
            fail(f"executables[{index}].size_bytes must be a positive integer")

    notes = value.get("notes")
    if not isinstance(notes, list) or not all(isinstance(note, str) for note in notes):
        fail("notes must be an array of strings")
    return value


def files_under(directory: Path) -> list[Path]:
    files = sorted(directory.rglob("candidate.json"))
    if not files:
        fail(f"no candidate.json files found below {directory}")
    return files


def read_selection(path: str | None) -> dict[str, Any]:
    if path is None:
        return {}
    value = read_json(Path(path))
    if not isinstance(value, dict):
        fail("selection must be an object keyed by stage")
    return value


def selected_artifacts(candidate: dict[str, Any], selection: dict[str, Any] | None) -> list[str]:
    ids = selection.get("artifact_ids") if selection is not None else None
    if ids is None:
        ids = [item.get("artifact_id") or item.get("name") for item in candidate["artifacts"]]
    if not isinstance(ids, list) or not ids or not all(isinstance(item, str) and item for item in ids):
        fail(f"stage {candidate['stage']} has no selected immutable artifact IDs")
    return ids


def validate_package_provenance(
    value: Any, *, expected_sha: str, expected_version: str
) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail("package provenance must be an object")
    if sha(value.get("candidate_sha"), "package provenance candidate_sha") != expected_sha:
        fail("package provenance candidate SHA does not match expected SHA")
    if string(value, "release_version") != expected_version:
        fail("package provenance release version does not match expected version")
    packages = value.get("packages")
    if not isinstance(packages, dict):
        fail("package provenance must contain packages")
    for package, record in packages.items():
        if not isinstance(package, str) or not isinstance(record, dict):
            fail("package provenance entries must be named objects")
        string(record, "archive")
        string(record, "archive_path")
        if Path(record["archive_path"]).is_absolute() or ".." in Path(record["archive_path"]).parts:
            fail(f"package provenance {package} archive_path must stay within artifact root")
        sha256(record.get("sha256"), f"package provenance {package} sha256")
        if not isinstance(record.get("size_bytes"), int) or record["size_bytes"] <= 0:
            fail(f"package provenance {package} size_bytes must be positive")
        if "verification_lockfile" in record:
            string(record, "verification_lockfile")
            string(record, "verification_lockfile_path")
            if Path(record["verification_lockfile_path"]).is_absolute() or ".." in Path(record["verification_lockfile_path"]).parts:
                fail(f"package provenance {package} verification_lockfile_path escapes artifact root")
            sha256(record.get("verification_lockfile_sha256"), f"package provenance {package} lockfile sha256")
            if not isinstance(record.get("verification_lockfile_size_bytes"), int) or record["verification_lockfile_size_bytes"] <= 0:
                fail(f"package provenance {package} lockfile size must be positive")
        if "installed_binary_sha256" in record:
            string(record, "installed_binary_path")
            sha256(record["installed_binary_sha256"], f"package provenance {package} installed binary sha256")
            if not isinstance(record.get("installed_binary_size_bytes"), int) or record["installed_binary_size_bytes"] <= 0:
                fail(f"package provenance {package} installed binary size must be positive")
    return value


def validate_registry_summary(
    value: Any, *, expected_version: str
) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        fail("registry summary must be an array")
    records = {item.get("crate"): item for item in value if isinstance(item, dict)}
    if len(value) != 3 or set(records) != {"gregg-protocol", "greggd", "gregg"}:
        fail("registry summary must contain exactly the three release crates")
    for crate, record in records.items():
        if record.get("version") != expected_version:
            fail(f"registry record {crate} has the wrong version")
        if record.get("yanked") is not False:
            fail(f"registry record {crate} is yanked or missing yank state")
        sha256(record.get("checksum"), f"registry record {crate} checksum")
        timestamp(record.get("published_at"), f"registry record {crate} published_at")
    return value


def validate_disposition(value: Any) -> dict[str, Any]:
    """Validate the explicit disposition ledger for the published 1.0.0 crates."""
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        fail("1.0.0 disposition schema_version must be 1")
    timestamp(value.get("observed_at"), "1.0.0 disposition observed_at")
    crates = value.get("crates")
    if not isinstance(crates, dict) or set(crates) != {"gregg-protocol", "greggd", "gregg"}:
        fail("1.0.0 disposition must contain exactly the three crate records")
    for crate, record in crates.items():
        if not isinstance(record, dict):
            fail(f"1.0.0 disposition record {crate} must be an object")
        if record.get("version") != "1.0.0":
            fail(f"1.0.0 disposition record {crate} must describe version 1.0.0")
        if not isinstance(record.get("yanked"), bool):
            fail(f"1.0.0 disposition record {crate}.yanked must be boolean")
        sha256(record.get("checksum"), f"1.0.0 disposition record {crate}.checksum")
        timestamp(record.get("published_at"), f"1.0.0 disposition record {crate}.published_at")
        if record.get("decision") not in {"retain", "yank"}:
            fail(f"1.0.0 disposition record {crate}.decision must be retain or yank")
        if record["decision"] == "yank" and record["yanked"] is not True:
            fail(f"1.0.0 disposition yank decision for {crate} is not observed as executed")
        if record["decision"] == "retain" and record["yanked"] is True and not record.get("ledger_note"):
            fail(f"1.0.0 disposition retain decision for already-yanked {crate} needs ledger_note")
    return value


def aggregate(args: argparse.Namespace) -> None:
    expected_sha = sha(args.expected_sha, "expected SHA")
    mode = getattr(args, "mode", "pre-tag")
    if mode not in ("pre-tag", "final"):
        fail(f"mode must be pre-tag or final, got {mode}")

    candidates = []
    for path in files_under(Path(args.evidence_dir)):
        candidates.append(
            (path, validate_candidate(
                read_json(path),
                expected_sha=expected_sha,
                expected_version=args.release_version,
            ))
        )

    required = args.required_stage
    if args.requirements:
        requirements = read_json(Path(args.requirements))
        if not isinstance(requirements, dict):
            fail("release requirements must be an object")
        if mode == "pre-tag":
            required_from_file = requirements.get("pre_tag_required_stages")
        else:
            required_from_file = requirements.get("final_required_stages")
        if not isinstance(required_from_file, list):
            fail(f"release requirements must contain {mode.replace('-', '_')}_required_stages")
        if required and required != required_from_file:
            fail("explicit required stages do not match the machine-readable requirements")
        required = required_from_file
    if not required or len(set(required)) != len(required):
        fail("required stages must be a nonempty list without duplicates")

    retrieved_manifest = None
    retrieved_by_stage: dict[str, dict[str, Any]] = {}
    if args.retrieved_manifest:
        retrieved_manifest = read_json(Path(args.retrieved_manifest))
        if not isinstance(retrieved_manifest, dict):
            fail("retrieved manifest must be an object")
        if retrieved_manifest.get("candidate_sha") != expected_sha:
            fail("retrieved manifest candidate SHA does not match expected SHA")
        for stage_entry in retrieved_manifest.get("stages", []):
            stage_name = stage_entry.get("stage", "")
            retrieved_by_stage[stage_name] = stage_entry

    selection = read_selection(args.selection)
    selection_record = None
    if args.selection:
        selection_bytes = Path(args.selection).read_bytes()
        if args.selection_source not in SELECTION_SOURCES:
            fail(f"unsupported selection source: {args.selection_source}")
        selection_record = {
            "source": args.selection_source,
            "sha256": hashlib.sha256(selection_bytes).hexdigest(),
            "size_bytes": len(selection_bytes),
            "workflow_run_id": args.selection_workflow_run_id,
            "workflow_run_attempt": args.selection_workflow_run_attempt,
        }
    elif isinstance(retrieved_manifest, dict):
        selection_record = retrieved_manifest.get("selection")
    if retrieved_manifest is not None and selection_record is not None:
        retrieved_selection = retrieved_manifest.get("selection")
        if not isinstance(retrieved_selection, dict):
            fail("retrieved manifest must contain selection identity")
        for field in ("sha256", "size_bytes"):
            if retrieved_selection.get(field) != selection_record.get(field):
                fail(f"retrieved selection {field} does not match aggregation input")
        if args.selection and retrieved_selection.get("source") != args.selection_source:
            fail("retrieved selection source does not match aggregation input")
    by_stage: dict[str, list[tuple[Path, dict[str, Any]]]] = {}
    for path, candidate in candidates:
        by_stage.setdefault(candidate["stage"], []).append((path, candidate))

    missing = [stage for stage in required if stage not in by_stage]
    if missing:
        fail(f"missing required stages: {', '.join(missing)}")

    stage_entries = []
    for stage in required:
        options = by_stage[stage]
        chosen_selection = selection.get(stage)
        if len(options) > 1 and not isinstance(chosen_selection, dict):
            fail(f"stage {stage} has multiple successful candidates; select one explicitly")
        if len(options) > 1:
            # Validate that selection uniquely identifies one candidate.
            run_id = str(chosen_selection.get("workflow_run_id", chosen_selection.get("run_id", "")))
            attempt = str(chosen_selection.get("workflow_run_attempt", chosen_selection.get("attempt", "")))
            matches = [
                item for item in options
                if item[1]["workflow_run_id"] == run_id
                and item[1]["workflow_run_attempt"] == attempt
            ]
            if len(matches) > 1:
                fail(
                    f"stage {stage} has multiple candidates matching run {run_id} "
                    f"attempt {attempt}; each stage must resolve to exactly one candidate"
                )
        chosen_path, chosen = options[0]
        if isinstance(chosen_selection, dict):
            run_id = str(chosen_selection.get("workflow_run_id", chosen_selection.get("run_id", "")))
            attempt = str(chosen_selection.get("workflow_run_attempt", chosen_selection.get("attempt", "")))
            matches = [
                item for item in options
                if item[1]["workflow_run_id"] == run_id
                and item[1]["workflow_run_attempt"] == attempt
            ]
            if not matches:
                fail(f"selection for {stage} does not identify a supplied successful attempt")
            chosen_path, chosen = matches[0]

        github_artifact: dict[str, Any] | None = None
        if stage in retrieved_by_stage:
            retrieved_stage = retrieved_by_stage[stage]
            retrieved_run_id = str(retrieved_stage.get("run", {}).get("run_id", ""))
            retrieved_attempt = str(retrieved_stage.get("run", {}).get("run_attempt", ""))
            if chosen["workflow_run_id"] != retrieved_run_id:
                fail(f"stage {stage} candidate run_id {chosen['workflow_run_id']} does not match retrieved run_id {retrieved_run_id}")
            if chosen["workflow_run_attempt"] != retrieved_attempt:
                fail(f"stage {stage} candidate attempt {chosen['workflow_run_attempt']} does not match retrieved attempt {retrieved_attempt}")
            artifacts = retrieved_stage.get("artifacts", [])
            if not artifacts:
                fail(
                    f"stage {stage} in retrieved manifest has no artifacts; "
                    "immutable artifact identity is mandatory when a retrieved manifest is supplied"
                )
            first = artifacts[0]
            # Require all identity fields.
            if not first.get("github_artifact_id"):
                fail(f"stage {stage} retrieved artifact is missing github_artifact_id")
            if not first.get("github_artifact_name"):
                fail(f"stage {stage} retrieved artifact is missing github_artifact_name")
            if not first.get("downloaded_zip_sha256"):
                fail(f"stage {stage} retrieved artifact is missing downloaded_zip_sha256")
            if not first.get("downloaded_zip_size_bytes"):
                fail(f"stage {stage} retrieved artifact is missing downloaded_zip_size_bytes")
            github_artifact = {
                "id": first.get("github_artifact_id"),
                "name": first.get("github_artifact_name"),
                "zip_sha256": first.get("downloaded_zip_sha256"),
                "zip_size_bytes": first.get("downloaded_zip_size_bytes"),
            }

        content_artifacts = []
        for item in chosen.get("artifacts", []):
            content_artifacts.append({
                "name": item.get("name", ""),
                "role": item.get("role", ""),
                "sha256": item.get("sha256"),
                "size_bytes": item.get("size_bytes"),
            })

        entry: dict[str, Any] = {
            "stage": stage,
            "workflow_run_id": chosen["workflow_run_id"],
            "workflow_run_attempt": chosen["workflow_run_attempt"],
            "artifact_ids": selected_artifacts(chosen, chosen_selection if isinstance(chosen_selection, dict) else None),
            "metadata_path": str(chosen_path),
            "metadata_sha256": hashlib.sha256(chosen_path.read_bytes()).hexdigest(),
            "content_artifacts": content_artifacts,
            "candidate": chosen,
        }
        if github_artifact is not None:
            entry["github_artifact"] = github_artifact
        stage_entries.append(entry)

    tag = None
    if any((args.tag_name, args.tag_object_sha, args.peeled_commit_sha, args.tagger_name, args.tagger_email, args.tagger_timestamp, args.tag_object_content_sha256)):
        if args.tag_name != "v1.0.1":
            fail("final tag name must be exactly v1.0.1")
        tag = {
            "name": args.tag_name,
            "tag_object_sha": sha(args.tag_object_sha, "tag object SHA"),
            "peeled_commit_sha": sha(args.peeled_commit_sha, "peeled commit SHA"),
        }
        if tag["peeled_commit_sha"] != expected_sha:
            fail("tag peeled commit does not equal expected candidate SHA")
        if any((args.tagger_name, args.tagger_email, args.tagger_timestamp, args.tag_object_content_sha256)):
            if not all((args.tagger_name, args.tagger_email, args.tagger_timestamp, args.tag_object_content_sha256)):
                fail("tagger and tag-object content metadata must be supplied together")
            tag.update({
                "tagger_name": args.tagger_name,
                "tagger_email": args.tagger_email,
                "tagger_timestamp": args.tagger_timestamp,
                "tag_object_content_sha256": sha256(args.tag_object_content_sha256, "tag object content SHA"),
            })

    packages = read_json(Path(args.package_provenance)) if args.package_provenance else None
    registry = read_json(Path(args.registry_summary)) if args.registry_summary else None
    disposition = read_json(Path(args.disposition)) if args.disposition else None
    tooling_sha = args.tooling_sha if hasattr(args, "tooling_sha") and args.tooling_sha else expected_sha

    if mode == "final":
        if tag is None:
            fail("final aggregation requires annotated tag identity")
        if not all(tag.get(name) for name in ("tagger_name", "tagger_email", "tagger_timestamp", "tag_object_content_sha256")):
            fail("final aggregation requires tagger and tag-object content metadata")
        packages = validate_package_provenance(
            packages, expected_sha=expected_sha, expected_version=args.release_version
        )
        if set(packages["packages"]) != {"gregg-protocol", "greggd", "gregg"}:
            fail("package provenance must contain all three crates")
        registry = validate_registry_summary(registry, expected_version=args.release_version)
        validate_disposition(disposition)
        if "postpublish-verify" not in by_stage:
            fail("final aggregation requires postpublish-verify evidence")

    manifest = {
        "manifest_schema_version": 1,
        "release_version": args.release_version,
        "candidate_sha": expected_sha,
        "tooling_sha": tooling_sha,
        "tag": tag,
        "mode": mode,
        "manifest_scope": "cross-run" if retrieved_manifest is not None else "current-run",
        "required_stages": required,
        "stages": stage_entries,
        "rerun_selection": selection,
        "selection": selection_record,
        "package_provenance": packages,
        "registry": registry,
        "version_1_0_0_disposition": disposition,
        "verdict": "pass",
    }
    validate_manifest(manifest, expected_sha=expected_sha, expected_version=args.release_version, mode=mode)
    write_json(Path(args.output), manifest)
    print(f"validated {len(stage_entries)} stages ({mode} mode); wrote {args.output}")


def validate_manifest(
    value: Any, *, expected_sha: str | None = None, expected_version: str | None = None, mode: str = "pre-tag"
) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("manifest_schema_version") != 1:
        fail("manifest_schema_version must be 1")
    candidate_sha = sha(value.get("candidate_sha"), "manifest candidate_sha")
    if expected_sha is not None and candidate_sha != expected_sha:
        fail("manifest candidate SHA does not match expected SHA")
    version = string(value, "release_version")
    if expected_version is not None and version != expected_version:
        fail("manifest release version does not match expected version")

    tooling_sha = value.get("tooling_sha")
    if tooling_sha is not None:
        sha(tooling_sha, "manifest tooling_sha")

    required = value.get("required_stages")
    stages = value.get("stages")
    if not isinstance(required, list) or not required or not all(isinstance(item, str) for item in required):
        fail("manifest required_stages must be a nonempty string array")
    if not isinstance(stages, list) or len(stages) != len(required):
        fail("manifest must contain exactly one selected entry for every required stage")

    manifest_mode = value.get("mode", "pre-tag")
    if manifest_mode not in ("pre-tag", "final"):
        fail(f"manifest mode must be pre-tag or final, got {manifest_mode}")

    manifest_scope = value.get("manifest_scope")
    if manifest_scope not in {"current-run", "cross-run"}:
        fail("manifest_scope must be current-run or cross-run")
    if manifest_scope == "cross-run" and value.get("mode") not in {"pre-tag", "final"}:
        fail("cross-run manifest must declare a release mode")
    if manifest_scope == "cross-run":
        selection = value.get("selection")
        if not isinstance(selection, dict):
            fail("cross-run manifest requires selection identity")
        if selection.get("source") not in {"workflow-dispatch-base64", "selection-file"}:
            fail("selection source is unsupported")
        sha256(selection.get("sha256"), "selection.sha256")
        if not isinstance(selection.get("size_bytes"), int) or selection["size_bytes"] <= 0:
            fail("selection.size_bytes must be positive")
        string(selection, "workflow_run_id")
        string(selection, "workflow_run_attempt")
    seen = set()
    artifact_ids: dict[int, tuple[str, str, str]] = {}
    for entry in stages:
        if not isinstance(entry, dict):
            fail("manifest stage entry must be an object")
        stage = string(entry, "stage")
        if stage in seen or stage not in required:
            fail(f"manifest has duplicate or unexpected stage: {stage}")
        seen.add(stage)
        string(entry, "workflow_run_id")
        string(entry, "workflow_run_attempt")
        ids = entry.get("artifact_ids")
        if not isinstance(ids, list) or not ids or not all(isinstance(item, str) and item for item in ids):
            fail(f"manifest stage {stage} has no artifact IDs")
        content_artifacts = entry.get("content_artifacts")
        if content_artifacts is not None:
            if not isinstance(content_artifacts, list):
                fail(f"manifest stage {stage} content_artifacts must be an array")
        github_artifact = entry.get("github_artifact")
        if manifest_scope == "cross-run" and not isinstance(github_artifact, dict):
            fail(f"manifest stage {stage} requires github_artifact identity")
        if github_artifact is not None:
            if not isinstance(github_artifact, dict):
                fail(f"manifest stage {stage} github_artifact must be an object")
            if not isinstance(github_artifact.get("id"), int) or github_artifact["id"] <= 0:
                fail(f"manifest stage {stage} github_artifact.id must be a positive integer")
            if not github_artifact.get("name"):
                fail(f"manifest stage {stage} github_artifact.name must be a nonempty string")
            if not github_artifact.get("zip_sha256") or not SHA256_RE.fullmatch(str(github_artifact["zip_sha256"])):
                fail(f"manifest stage {stage} github_artifact.zip_sha256 must be a lowercase 64-character SHA-256")
            if not isinstance(github_artifact.get("zip_size_bytes"), int) or github_artifact["zip_size_bytes"] <= 0:
                fail(f"manifest stage {stage} github_artifact.zip_size_bytes must be a positive integer")
            identity = (github_artifact["name"], github_artifact["zip_sha256"], str(github_artifact["zip_size_bytes"]))
            previous = artifact_ids.setdefault(github_artifact["id"], identity)
            if previous != identity:
                fail(f"github artifact ID {github_artifact['id']} is reused with conflicting identity")
        validate_candidate(entry.get("candidate"), expected_sha=candidate_sha, expected_version=version)

    if seen != set(required):
        fail("manifest stage set does not match required_stages")
    tag = value.get("tag")
    if tag is not None:
        if not isinstance(tag, dict) or tag.get("name") != "v1.0.1":
            fail("manifest tag must be annotated v1.0.1 metadata")
        sha(tag.get("tag_object_sha"), "manifest tag_object_sha")
        if sha(tag.get("peeled_commit_sha"), "manifest peeled_commit_sha") != candidate_sha:
            fail("manifest tag peeled commit does not match candidate")
        if "tag_object_content_sha256" in tag:
            sha256(tag.get("tag_object_content_sha256"), "manifest tag object content SHA")
            timestamp(tag.get("tagger_timestamp"), "manifest tagger_timestamp")
            string(tag, "tagger_name")
            string(tag, "tagger_email")
    if value.get("package_provenance") is not None:
        validate_package_provenance(
            value["package_provenance"], expected_sha=candidate_sha, expected_version=version
        )
    elif tag is not None and "tag_object_content_sha256" in tag:
        fail("final manifest with complete tag identity must include package provenance")
    if value.get("registry") is not None:
        validate_registry_summary(value["registry"], expected_version=version)
    if value.get("version_1_0_0_disposition") is not None:
        validate_disposition(value["version_1_0_0_disposition"])
    if value.get("verdict") != "pass":
        fail("manifest verdict is not pass")
    return value


def write_candidate(args: argparse.Namespace) -> None:
    now = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    candidate_sha = sha(args.candidate_sha, "candidate SHA")
    artifacts = read_json(Path(args.artifacts_json)) if args.artifacts_json else []
    if not isinstance(artifacts, list):
        fail("artifacts JSON must be an array")
    executables = []
    for item in args.executable:
        if len(item) != 3:
            fail("--executable takes NAME SHA256 SIZE_BYTES")
        if not re.fullmatch(r"[0-9a-f]{64}", item[1]):
            fail("executable SHA-256 must be lowercase 64-character hexadecimal")
        try:
            size_bytes = int(item[2])
        except ValueError:
            fail("executable size must be an integer")
        if size_bytes <= 0:
            fail("executable size must be positive")
        executables.append({"name": item[0], "sha256": item[1], "size_bytes": size_bytes})
    value = {
        "schema_version": SCHEMA_VERSION,
        "candidate_sha": candidate_sha,
        "release_version": args.release_version,
        "stage": args.stage,
        "workflow_run_id": args.workflow_run_id,
        "workflow_run_attempt": args.workflow_run_attempt,
        "job_name": args.job_name,
        "runner_os": args.runner_os,
        "runner_architecture": args.runner_architecture,
        "started_at": args.started_at or now,
        "completed_at": args.completed_at or now,
        "result": args.result,
        "source_identity_mode": args.source_identity_mode,
        "source": {
            "ref_input": args.ref_input or candidate_sha,
            "tag_object_sha": args.tag_object_sha,
            "peeled_commit_sha": args.peeled_commit_sha or candidate_sha,
            "tagger_name": args.tagger_name,
            "tagger_email": args.tagger_email,
            "tagger_timestamp": args.tagger_timestamp,
            "tagger_timestamp_original": args.tagger_timestamp_original,
            "tag_object_content_sha256": args.tag_object_content_sha256,
            "head_sha": args.head_sha,
        },
        "artifacts": artifacts,
        "executables": executables,
        "notes": args.note,
    }
    validate_candidate(value, expected_sha=candidate_sha, expected_version=args.release_version, require_success=False)
    write_json(Path(args.output), value)


def make_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    command = sub.add_parser("validate-candidate")
    command.add_argument("path", type=Path)
    command.add_argument("--expected-sha")
    command.add_argument("--expected-version")
    command.add_argument("--expected-architecture")
    command.add_argument("--expected-source-mode")
    command.add_argument("--allow-failure", action="store_true")

    command = sub.add_parser("write-candidate")
    command.add_argument("--output", required=True, type=Path)
    command.add_argument("--candidate-sha", required=True)
    command.add_argument("--release-version", required=True)
    command.add_argument("--stage", required=True)
    command.add_argument("--workflow-run-id", required=True)
    command.add_argument("--workflow-run-attempt", required=True)
    command.add_argument("--job-name", required=True)
    command.add_argument("--runner-os", required=True)
    command.add_argument("--runner-architecture", required=True)
    command.add_argument("--source-identity-mode", choices=sorted(SOURCE_MODES), default="pre-tag-full-sha")
    command.add_argument("--started-at")
    command.add_argument("--completed-at")
    command.add_argument("--result", choices=sorted(RESULTS), default="success")
    command.add_argument("--ref-input")
    command.add_argument("--tag-object-sha")
    command.add_argument("--peeled-commit-sha")
    command.add_argument("--tagger-name")
    command.add_argument("--tagger-email")
    command.add_argument("--tagger-timestamp")
    command.add_argument("--tagger-timestamp-original")
    command.add_argument("--tag-object-content-sha256")
    command.add_argument("--head-sha")
    command.add_argument("--artifacts-json")
    command.add_argument("--executable", action="append", nargs=3, default=[], metavar=("NAME", "SHA256", "SIZE"))
    command.add_argument("--note", action="append", default=[])

    command = sub.add_parser("aggregate")
    command.add_argument("--evidence-dir", required=True)
    command.add_argument("--expected-sha", required=True)
    command.add_argument("--release-version", required=True)
    command.add_argument("--output", required=True)
    command.add_argument("--required-stage", action="append", default=[])
    command.add_argument("--requirements")
    command.add_argument("--selection")
    command.add_argument("--selection-source", default="selection-file")
    command.add_argument("--selection-workflow-run-id", default="not-recorded")
    command.add_argument("--selection-workflow-run-attempt", default="not-recorded")
    command.add_argument("--retrieved-manifest")
    command.add_argument("--mode", choices=["pre-tag", "final"], default="pre-tag")
    command.add_argument("--tooling-sha")
    command.add_argument("--tag-name")
    command.add_argument("--tag-object-sha")
    command.add_argument("--peeled-commit-sha")
    command.add_argument("--tagger-name")
    command.add_argument("--tagger-email")
    command.add_argument("--tagger-timestamp")
    command.add_argument("--tag-object-content-sha256")
    command.add_argument("--package-provenance")
    command.add_argument("--registry-summary")
    command.add_argument("--disposition")
    command.add_argument("--final", action="store_true")

    command = sub.add_parser("validate-manifest")
    command.add_argument("path", type=Path)
    command.add_argument("--expected-sha")
    command.add_argument("--expected-version")
    command.add_argument("--mode", choices=["pre-tag", "final"], default="pre-tag")
    return parser


def main() -> int:
    args = make_parser().parse_args()
    try:
        if args.command == "validate-candidate":
            validate_candidate(
                read_json(args.path),
                expected_sha=args.expected_sha,
                expected_version=args.expected_version,
                expected_architecture=args.expected_architecture,
                expected_source_mode=args.expected_source_mode,
                require_success=not args.allow_failure,
            )
            print(f"valid candidate metadata: {args.path}")
        elif args.command == "write-candidate":
            write_candidate(args)
        elif args.command == "aggregate":
            aggregate(args)
        elif args.command == "validate-manifest":
            validate_manifest(read_json(args.path), expected_sha=args.expected_sha, expected_version=args.expected_version, mode=args.mode)
            print(f"valid release manifest: {args.path}")
        return 0
    except EvidenceError as error:
        print(f"FATAL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
