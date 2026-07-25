#!/usr/bin/env python3
"""Retrieve immutable GitHub workflow run and artifact evidence for final aggregation.

Queries the GitHub Actions API for selected workflow runs and attempts, resolves
actual numeric artifact identities, downloads artifact ZIP archives, calculates
their SHA-256 and byte size, safely extracts them, and validates all contained
candidate metadata before consuming any other file.

Designed for deterministic testing: the GitHub API base URL and token are
configurable so mocked or recorded fixtures can be used in tests.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
import tempfile
import urllib.error
import urllib.request
import zipfile
from pathlib import Path
from typing import Any


SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")

GITHUB_API_DEFAULT = "https://api.github.com"

SUPPORTED_WORKFLOWS = {"release-candidate"}

MAX_ZIP_ENTRIES = 1000
MAX_EXTRACTED_BYTES = 512 * 1024 * 1024  # 512 MiB

PRE_TAG_REQUIRED_STAGE_GROUPS = [
    "source-ci",
    "protocol-prepublish",
    "protocol-index-check",
    "binary-prepublish",
    "binary-msrv",
    "native-source",
    "native-package",
    "mixed-fleet",
    "systemd-lifecycle",
    "launchd-lifecycle",
    "resource",
    "soak",
]

FINAL_EXTRA_STAGE_GROUPS = [
    "postpublish-verify",
]


class RetrievalError(ValueError):
    """A retrieval or validation step failed."""


def fail(message: str) -> None:
    raise RetrievalError(message)


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


def sha256_of_file(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def api_request(base_url: str, token: str, path: str) -> dict[str, Any]:
    """Perform a GET request to the GitHub API and return parsed JSON."""
    url = f"{base_url.rstrip('/')}{path}"
    headers = {"Accept": "application/vnd.github+json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read().decode("utf-8")
            return json.loads(body)
    except urllib.error.HTTPError as error:
        fail(f"GitHub API {path} returned HTTP {error.code}: {error.read().decode('utf-8', errors='replace')}")
    except urllib.error.URLError as error:
        fail(f"GitHub API {path} unreachable: {error}")
    return {}


def api_request_raw(base_url: str, token: str, path: str) -> bytes:
    """Perform a GET request and return raw bytes (for artifact downloads)."""
    url = f"{base_url.rstrip('/')}{path}"
    headers = {"Accept": "application/vnd.github+json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.read()
    except urllib.error.HTTPError as error:
        fail(f"GitHub API download {path} returned HTTP {error.code}")
    except urllib.error.URLError as error:
        fail(f"GitHub API download {path} unreachable: {error}")
    return b""


def validate_selection(selection: dict[str, Any]) -> None:
    """Validate a selection file before any network access."""
    if not isinstance(selection, dict):
        fail("selection must be an object")

    candidate_sha = selection.get("candidate_sha")
    if not isinstance(candidate_sha, str) or not SHA_RE.fullmatch(candidate_sha):
        fail("selection.candidate_sha must be a lowercase 40-character SHA")

    release_version = selection.get("release_version")
    if not isinstance(release_version, str) or not VERSION_RE.fullmatch(release_version):
        fail("selection.release_version must be a semver string")
    if release_version != "1.0.1":
        fail(f"release_version must be 1.0.1, got {release_version}")

    tooling_sha = selection.get("tooling_sha")
    if tooling_sha is not None:
        if not isinstance(tooling_sha, str) or not SHA_RE.fullmatch(tooling_sha):
            fail("selection.tooling_sha must be a lowercase 40-character SHA when present")

    runs = selection.get("runs")
    if not isinstance(runs, dict) or not runs:
        fail("selection.runs must be a nonempty object")

    for stage_name, run_info in runs.items():
        if not isinstance(run_info, dict):
            fail(f"selection.runs.{stage_name} must be an object")

        run_id = run_info.get("run_id")
        attempt = run_info.get("attempt")
        if not isinstance(run_id, (str, int)) or not str(run_id).isdigit():
            fail(f"selection.runs.{stage_name}.run_id must be numeric")
        if not isinstance(attempt, (str, int)) or not str(attempt).isdigit():
            fail(f"selection.runs.{stage_name}.attempt must be numeric")
        if int(str(run_id)) <= 0:
            fail(f"selection.runs.{stage_name}.run_id must be positive")
        if int(str(attempt)) <= 0:
            fail(f"selection.runs.{stage_name}.attempt must be positive")

        workflow_name = run_info.get("workflow_name", "release-candidate")
        if workflow_name not in SUPPORTED_WORKFLOWS:
            fail(f"selection.runs.{stage_name}.workflow_name {workflow_name} is not supported")

        artifacts = run_info.get("artifacts")
        if not isinstance(artifacts, list) or not artifacts:
            fail(f"selection.runs.{stage_name}.artifacts must be a nonempty array")

        run_artifact_names: set[str] = set()
        for i, artifact in enumerate(artifacts):
            if not isinstance(artifact, dict):
                fail(f"selection.runs.{stage_name}.artifacts[{i}] must be an object")
            name = artifact.get("name")
            if not isinstance(name, str) or not name.strip():
                fail(f"selection.runs.{stage_name}.artifacts[{i}].name must be a nonempty string")
            if name in run_artifact_names:
                fail(f"selection.runs.{stage_name} has duplicate artifact name: {name}")
            run_artifact_names.add(name)

            artifact_id = artifact.get("artifact_id")
            if artifact_id is not None:
                if not isinstance(artifact_id, (str, int)) or int(str(artifact_id)) <= 0:
                    fail(f"selection.runs.{stage_name}.artifacts[{i}].artifact_id must be a positive integer")

    mode = selection.get("mode", "pre-tag")
    if mode not in ("pre-tag", "final"):
        fail(f"selection.mode must be pre-tag or final, got {mode}")


def validate_run_metadata(
    run: dict[str, Any],
    *,
    expected_repo: str,
    expected_workflow: str,
    expected_attempt: str,
) -> dict[str, Any]:
    """Validate a workflow run record from the GitHub API."""
    if not isinstance(run, dict):
        fail("workflow run metadata must be an object")
    run_id = str(run.get("id", ""))
    if not run_id:
        fail("workflow run has no id")
    if run.get("status") != "completed":
        fail(f"workflow run {run_id} is not completed (status={run.get('status')})")
    if run.get("conclusion") != "success":
        fail(f"workflow run {run_id} did not conclude successfully (conclusion={run.get('conclusion')})")
    attempt = str(run.get("run_attempt", ""))
    if attempt != expected_attempt:
        fail(f"workflow run {run_id} attempt {attempt} does not match selected {expected_attempt}")
    repo_full = run.get("repository", {}).get("full_name", "")
    if repo_full != expected_repo:
        fail(f"workflow run {run_id} repository {repo_full} does not match {expected_repo}")
    workflow_name = run.get("name", "")
    if workflow_name != expected_workflow:
        fail(f"workflow run {run_id} name {workflow_name} does not match {expected_workflow}")
    return {
        "repository": repo_full,
        "workflow_id": str(run.get("workflow_id", "")),
        "workflow_name": workflow_name,
        "run_id": run_id,
        "run_attempt": attempt,
        "event_type": run.get("event", ""),
        "status": run.get("status", ""),
        "conclusion": run.get("conclusion", ""),
        "created_at": run.get("created_at", ""),
        "started_at": run.get("started_at", ""),
        "updated_at": run.get("updated_at", ""),
        "closed_at": run.get("closed_at", ""),
        "actor": run.get("actor", {}).get("login", ""),
        "run_url": run.get("html_url", ""),
        "head_sha": run.get("head_sha", ""),
        "source_branch": run.get("head_branch", ""),
    }


def resolve_artifacts(
    base_url: str,
    token: str,
    repo: str,
    run_id: str,
    expected_names: list[str],
    explicit_ids: dict[str, int] | None = None,
) -> list[dict[str, Any]]:
    """List artifacts for a run and filter by expected names."""
    data = api_request(base_url, token, f"/repos/{repo}/actions/runs/{run_id}/artifacts")
    artifacts = data.get("artifacts", [])
    if not isinstance(artifacts, list):
        fail(f"artifact listing for run {run_id} is not a list")

    explicit_ids = explicit_ids or {}
    matched: list[dict[str, Any]] = []
    for name in expected_names:
        candidates = [a for a in artifacts if a.get("name") == name]
        if not candidates:
            fail(f"run {run_id} has no artifact named {name}")
        if len(candidates) > 1 and name not in explicit_ids:
            fail(f"run {run_id} has multiple artifacts named {name}; use explicit artifact ID selection")
        if name in explicit_ids:
            candidates = [a for a in candidates if int(a.get("id", 0)) == explicit_ids[name]]
            if not candidates:
                fail(f"run {run_id} explicit artifact ID {explicit_ids[name]} does not match {name}")
        artifact = candidates[0]
        if artifact.get("expired", False):
            fail(f"run {run_id} artifact {name} is expired")
        matched.append({
            "github_artifact_id": int(artifact["id"]),
            "github_artifact_name": artifact["name"],
            "github_reported_size_bytes": int(artifact.get("size_in_bytes", 0)),
            "creation_time": artifact.get("created_at", ""),
            "expiration_time": artifact.get("expires_at", ""),
            "workflow_run_id": run_id,
        })
    return matched


def _safe_extract_zip(zip_path: Path, extract_dir: Path) -> None:
    """Extract a ZIP archive, rejecting unsafe entries."""
    try:
        with zipfile.ZipFile(zip_path) as zf:
            entries = zf.infolist()
            if not entries:
                fail(f"ZIP {zip_path} is empty")
            if len(entries) > MAX_ZIP_ENTRIES:
                fail(f"ZIP {zip_path} has {len(entries)} entries, exceeding limit of {MAX_ZIP_ENTRIES}")

            total_extracted = 0
            seen_normalized: set[str] = set()
            for entry in entries:
                name = entry.filename
                if name.startswith("/") or ":" in name:
                    fail(f"ZIP entry {name!r} has an absolute path")
                normalized = os.path.normpath(name)
                if normalized.startswith("..") or "/../" in normalized:
                    fail(f"ZIP entry {name!r} contains path traversal")
                resolved = (extract_dir / normalized).resolve()
                if not str(resolved).startswith(str(extract_dir.resolve())):
                    fail(f"ZIP entry {name!r} resolves outside extraction root")
                if entry.is_dir():
                    continue
                if normalized in seen_normalized:
                    fail(f"ZIP entry {name!r} collides after normalization")
                seen_normalized.add(normalized)
                total_extracted += entry.file_size
                if total_extracted > MAX_EXTRACTED_BYTES:
                    fail(f"ZIP extracted size exceeds {MAX_EXTRACTED_BYTES} bytes")

            zf.extractall(extract_dir)
    except zipfile.BadZipFile as error:
        fail(f"ZIP {zip_path} is not valid: {error}")


def download_artifact(
    base_url: str,
    token: str,
    repo: str,
    artifact: dict[str, Any],
    expected_sha: str,
    expected_version: str,
    output_dir: Path,
) -> dict[str, Any]:
    """Download an artifact ZIP, calculate its checksum, safely extract, and validate all candidates."""
    artifact_id = artifact["github_artifact_id"]
    artifact_name = artifact["github_artifact_name"]
    download_url = f"/repos/{repo}/actions/artifacts/{artifact_id}/zip"
    zip_bytes = api_request_raw(base_url, token, download_url)
    if not zip_bytes:
        fail(f"artifact {artifact_id} ({artifact_name}) download returned empty bytes")

    zip_path = output_dir / f"artifact-{artifact_id}.zip"
    zip_path.write_bytes(zip_bytes)
    downloaded_zip_sha, downloaded_zip_size = sha256_of_file(zip_path)

    extract_dir = output_dir / f"artifact-{artifact_id}"
    extract_dir.mkdir(parents=True, exist_ok=True)
    _safe_extract_zip(zip_path, extract_dir)

    candidate_files = sorted(extract_dir.rglob("candidate.json"))
    if not candidate_files:
        fail(f"artifact {artifact_id} ({artifact_name}) contains no candidate.json")

    validated_candidates: list[dict[str, Any]] = []
    candidate_paths: list[dict[str, Any]] = []
    for candidate_path in candidate_files:
        candidate = read_json(candidate_path)
        if not isinstance(candidate, dict):
            fail(f"artifact {artifact_id} candidate {candidate_path} is not an object")
        if candidate.get("candidate_sha") != expected_sha:
            fail(f"artifact {artifact_id} candidate {candidate_path} SHA {candidate.get('candidate_sha')} does not match expected {expected_sha}")
        version = candidate.get("release_version")
        if version != expected_version:
            fail(f"artifact {artifact_id} candidate {candidate_path} version {version} does not match expected {expected_version}")
        if candidate.get("workflow_run_id") and str(candidate["workflow_run_id"]) != str(artifact.get("workflow_run_id", "")):
            fail(f"artifact {artifact_id} candidate run_id {candidate.get('workflow_run_id')} does not match artifact run_id {artifact.get('workflow_run_id')}")
        candidate_sha256, candidate_size = sha256_of_file(candidate_path)
        candidate_paths.append({
            "path": str(candidate_path.relative_to(extract_dir)),
            "sha256": candidate_sha256,
            "size_bytes": candidate_size,
            "stage": candidate.get("stage", ""),
        })
        validated_candidates.append(candidate)

    stage_names = sorted(set(c.get("stage", "") for c in validated_candidates))

    artifact["downloaded_zip_sha256"] = downloaded_zip_sha
    artifact["downloaded_zip_size_bytes"] = downloaded_zip_size
    artifact["extraction_path"] = str(extract_dir)
    artifact["candidate_metadata"] = candidate_paths
    artifact["logical_stages"] = stage_names
    return artifact


def build_provenance_index(artifact_cache: dict[str, dict[str, Any]]) -> dict[str, Any]:
    """Scan extracted artifacts for provenance, lockfile, and archive files.

    Returns a deterministic index mapping logical stage names to file paths
    so downstream consumers do not need filesystem heuristics.
    """
    index: dict[str, Any] = {"packages": {}}
    for artifact_key, artifact in sorted(artifact_cache.items()):
        extract_dir = Path(artifact.get("extraction_path", ""))
        if not extract_dir.is_dir():
            continue
        for stage in artifact.get("logical_stages", []):
            stage_entry: dict[str, Any] = {"artifact_key": artifact_key}
            # Find provenance file.
            provenance_files = sorted(extract_dir.rglob("package-provenance.json"))
            if provenance_files:
                stage_entry["provenance_path"] = str(provenance_files[0])
            # Find lockfile.
            lockfile_files = sorted(extract_dir.rglob("Cargo.lock"))
            if lockfile_files:
                stage_entry["lockfile_path"] = str(lockfile_files[0])
            # Find .crate archives.
            archive_files = sorted(extract_dir.rglob("*.crate"))
            if archive_files:
                stage_entry["archive_path"] = str(archive_files[0])
            # Map stage to package name for provenance lookup.
            for package in ("gregg-protocol", "greggd", "gregg"):
                if package in stage or f"binary-{package}" in stage:
                    index["packages"].setdefault(package, []).append({
                        "stage": stage,
                        **stage_entry,
                    })
                    break
    return index


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", required=True, type=Path, help="Run selection JSON file")
    parser.add_argument("--repo", required=True, help="Repository full name (owner/repo)")
    parser.add_argument("--output", required=True, type=Path, help="Output retrieved manifest path")
    parser.add_argument("--api-base-url", default=GITHUB_API_DEFAULT, help="GitHub API base URL (for testing)")
    parser.add_argument("--token", default="", help="GitHub token; defaults to GITHUB_TOKEN env var")
    args = parser.parse_args()

    token = args.token or os.environ.get("GITHUB_TOKEN", "")
    if not token:
        fail("no GitHub token supplied: pass --token or set GITHUB_TOKEN")

    selection = read_json(args.selection)
    validate_selection(selection)

    expected_sha = selection["candidate_sha"]
    expected_version = selection["release_version"]
    runs = selection["runs"]
    tooling_sha = selection.get("tooling_sha", expected_sha)

    # Collect unique (run_id, attempt, workflow_name) tuples and their artifacts.
    unique_runs: dict[tuple[str, str, str], dict[str, Any]] = {}
    stage_artifact_map: dict[str, list[tuple[str, str, dict[str, Any]]]] = {}
    for stage_name, run_info in runs.items():
        run_id = str(run_info["run_id"])
        attempt = str(run_info["attempt"])
        workflow_name = run_info.get("workflow_name", "release-candidate")
        key = (run_id, attempt, workflow_name)
        if key not in unique_runs:
            unique_runs[key] = {
                "run_id": run_id,
                "attempt": attempt,
                "workflow_name": workflow_name,
                "artifact_specs": [],
                "stage_names": [],
            }
        entry = unique_runs[key]
        entry["stage_names"].append(stage_name)
        for a in run_info["artifacts"]:
            name = a["name"]
            if name not in {x["name"] for x in entry["artifact_specs"]}:
                entry["artifact_specs"].append(a)
        stage_artifact_map.setdefault(stage_name, [])
        for a in run_info["artifacts"]:
            stage_artifact_map[stage_name].append((run_id, attempt, a))

    # Validate and download each unique run once.
    run_cache: dict[tuple[str, str], dict[str, Any]] = {}
    artifact_cache: dict[str, dict[str, Any]] = {}
    stages: list[dict[str, Any]] = []
    for (run_id, attempt, workflow_name), info in unique_runs.items():
        run_meta = api_request(args.api_base_url, token, f"/repos/{args.repo}/actions/runs/{run_id}")
        validated_run = validate_run_metadata(
            run_meta,
            expected_repo=args.repo,
            expected_workflow=workflow_name,
            expected_attempt=attempt,
        )
        run_cache[(run_id, attempt)] = validated_run

        expected_names = [a["name"] for a in info["artifact_specs"]]
        explicit_ids = {}
        for a in info["artifact_specs"]:
            if "artifact_id" in a:
                explicit_ids[a["name"]] = int(a["artifact_id"])

        artifacts = resolve_artifacts(
            args.api_base_url, token, args.repo, run_id,
            expected_names, explicit_ids,
        )

        download_dir = args.output.parent.parent / f".retrieval-downloads-{run_id}"
        download_dir.mkdir(parents=True, exist_ok=True)

        for artifact in artifacts:
            artifact_key = f"{run_id}:{artifact['github_artifact_name']}"
            if artifact_key not in artifact_cache:
                download_artifact(
                    args.api_base_url, token, args.repo,
                    artifact, expected_sha, expected_version, download_dir,
                )
                artifact_cache[artifact_key] = artifact
            else:
                # Reuse cached artifact metadata.
                artifact.update(artifact_cache[artifact_key])

    # Build per-stage manifest entries from the caches.
    # Validate that every selection stage resolves to an artifact containing that stage.
    all_stages_in_artifacts: dict[str, list[str]] = {}
    for artifact_key, artifact in artifact_cache.items():
        for ls in artifact.get("logical_stages", []):
            all_stages_in_artifacts.setdefault(ls, []).append(artifact_key)

    seen_stages: set[str] = set()
    for (run_id, attempt, workflow_name), info in unique_runs.items():
        validated_run = run_cache[(run_id, attempt)]
        for stage_name in info["stage_names"]:
            if stage_name in seen_stages:
                continue
            seen_stages.add(stage_name)
            stage_artifacts = []
            for a in info["artifact_specs"]:
                artifact_key = f"{run_id}:{a['name']}"
                if artifact_key in artifact_cache:
                    stage_artifacts.append(artifact_cache[artifact_key])
            # Validate that at least one artifact for this stage contains the stage in its candidate metadata.
            found_in = [k for k in stage_artifacts if stage_name in k.get("logical_stages", [])]
            if not found_in:
                fail(f"stage {stage_name} not found in any candidate metadata for run {run_id}; "
                     f"found stages: {[a.get('logical_stages', []) for a in stage_artifacts]}")
            stages.append({
                "stage": stage_name,
                "run": validated_run,
                "artifacts": stage_artifacts,
            })

    import datetime as dt
    retrieved_at = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")

    provenance_index = build_provenance_index(artifact_cache)

    manifest = {
        "manifest_schema_version": 1,
        "release_version": expected_version,
        "candidate_sha": expected_sha,
        "tooling_sha": tooling_sha,
        "retrieved_at": retrieved_at,
        "stages": stages,
        "provenance_index": provenance_index,
        "verdict": "pass",
    }
    write_json(args.output, manifest)

    # Write provenance index as a separate file for downstream consumers.
    provenance_index_path = args.output.parent / "provenance-index.json"
    write_json(provenance_index_path, provenance_index)

    # Copy validated candidate.json files into the evidence directory under stage names
    # so aggregation can discover them by scanning.
    evidence_dir = args.output.parent
    for stage_entry in stages:
        stage_name = stage_entry["stage"]
        for art in stage_entry["artifacts"]:
            extract_dir = Path(art.get("extraction_path", ""))
            if not extract_dir.is_dir():
                continue
            for candidate_path in extract_dir.rglob("candidate.json"):
                candidate = read_json(candidate_path)
                if candidate.get("stage") == stage_name:
                    dest = evidence_dir / stage_name / "candidate.json"
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_bytes(candidate_path.read_bytes())

    print(f"retrieved {len(stages)} stages with {sum(len(s['artifacts']) for s in stages)} artifacts; wrote {args.output}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RetrievalError as error:
        print(f"FATAL: {error}", file=sys.stderr)
        raise SystemExit(1)
