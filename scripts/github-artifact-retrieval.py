#!/usr/bin/env python3
"""Retrieve immutable GitHub workflow run and artifact evidence for final aggregation.

Queries the GitHub Actions API for selected workflow runs and attempts, resolves
actual numeric artifact identities, downloads artifact ZIP archives, calculates
their SHA-256 and byte size, extracts them, and validates contained candidate
metadata before consuming any other file.

Designed for deterministic testing: the GitHub API base URL and token are
configurable so mocked or recorded fixtures can be used in tests.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
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
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.read()
    except urllib.error.HTTPError as error:
        fail(f"GitHub API download {path} returned HTTP {error.code}")
    except urllib.error.URLError as error:
        fail(f"GitHub API download {path} unreachable: {error}")
    return b""


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
    explicit_ids: dict[str, str] | None = None,
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
            candidates = [a for a in candidates if str(a.get("id")) == explicit_ids[name]]
            if not candidates:
                fail(f"run {run_id} explicit artifact ID {explicit_ids[name]} does not match {name}")
        artifact = candidates[0]
        if artifact.get("expired", False):
            fail(f"run {run_id} artifact {name} is expired")
        matched.append({
            "github_artifact_id": artifact["id"],
            "github_artifact_name": artifact["name"],
            "archive_size_bytes": artifact.get("size_in_bytes", 0),
            "creation_time": artifact.get("created_at", ""),
            "expiration_time": artifact.get("expires_at", ""),
            "workflow_run_id": run_id,
        })
    return matched


def download_artifact(
    base_url: str,
    token: str,
    repo: str,
    artifact: dict[str, Any],
    output_dir: Path,
) -> dict[str, Any]:
    """Download an artifact ZIP, calculate its checksum, and extract it."""
    artifact_id = artifact["github_artifact_id"]
    download_url = f"/repos/{repo}/actions/artifacts/{artifact_id}/zip"
    zip_bytes = api_request_raw(base_url, token, download_url)
    if not zip_bytes:
        fail(f"artifact {artifact_id} download returned empty bytes")

    zip_path = output_dir / f"artifact-{artifact_id}.zip"
    zip_path.write_bytes(zip_bytes)
    archive_sha, archive_size = sha256_of_file(zip_path)

    extract_dir = output_dir / f"artifact-{artifact_id}"
    extract_dir.mkdir(parents=True, exist_ok=True)
    try:
        with zipfile.ZipFile(zip_path) as zf:
            zf.extractall(extract_dir)
    except zipfile.BadZipFile as error:
        fail(f"artifact {artifact_id} is not a valid ZIP: {error}")

    # Validate candidate metadata before consuming other files.
    candidate_files = sorted(extract_dir.rglob("candidate.json"))
    if not candidate_files:
        fail(f"artifact {artifact_id} contains no candidate.json")
    candidate = read_json(candidate_files[0])
    if not isinstance(candidate, dict):
        fail(f"artifact {artifact_id} candidate.json is not an object")
    if candidate.get("candidate_sha") != artifact.get("expected_sha"):
        fail(f"artifact {artifact_id} candidate SHA mismatch")

    artifact["archive_sha256"] = archive_sha
    artifact["archive_size_bytes"] = archive_size
    artifact["extract_dir"] = str(extract_dir)
    artifact["candidate_metadata_path"] = str(candidate_files[0])
    artifact["candidate_metadata_sha256"] = sha256_of_file(candidate_files[0])[0]
    return artifact


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", required=True, type=Path, help="Run selection JSON file")
    parser.add_argument("--repo", required=True, help="Repository full name (owner/repo)")
    parser.add_argument("--workflow-name", required=True, help="Expected workflow name")
    parser.add_argument("--output", required=True, type=Path, help="Output final manifest path")
    parser.add_argument("--api-base-url", default=GITHUB_API_DEFAULT, help="GitHub API base URL (for testing)")
    parser.add_argument("--token", default="", help="GitHub token (for testing)")
    parser.add_argument("--expected-artifact-names", action="append", default=[], help="Expected artifact names per stage")
    args = parser.parse_args()

    selection = read_json(args.selection)
    if not isinstance(selection, dict):
        fail("selection must be an object")

    expected_sha = selection.get("candidate_sha")
    if not isinstance(expected_sha, str) or not SHA_RE.fullmatch(expected_sha):
        fail("selection.candidate_sha must be a lowercase 40-character SHA")
    expected_version = selection.get("release_version")
    if not isinstance(expected_version, str) or not VERSION_RE.fullmatch(expected_version):
        fail("selection.release_version must be a semver string")
    runs = selection.get("runs")
    if not isinstance(runs, dict) or not runs:
        fail("selection.runs must be a nonempty object")

    stages: list[dict[str, Any]] = []
    for stage_name, run_info in runs.items():
        if not isinstance(run_info, dict):
            fail(f"selection.runs.{stage_name} must be an object")
        run_id = str(run_info.get("run_id", ""))
        attempt = str(run_info.get("attempt", ""))
        if not run_id or not attempt:
            fail(f"selection.runs.{stage_name} must have run_id and attempt")
        explicit_ids = run_info.get("artifact_ids") or {}

        # Retrieve run metadata.
        run_meta = api_request(args.api_base_url, args.token, f"/repos/{args.repo}/actions/runs/{run_id}")
        validated_run = validate_run_metadata(
            run_meta,
            expected_repo=args.repo,
            expected_workflow=args.workflow_name,
            expected_attempt=attempt,
        )

        # Resolve artifacts.
        expected_names = run_info.get("expected_artifact_names") or args.expected_artifact_names
        if not expected_names:
            expected_names = [stage_name]
        artifacts = resolve_artifacts(
            args.api_base_url, args.token, args.repo, run_id,
            expected_names, explicit_ids,
        )

        # Download and validate each artifact.
        for artifact in artifacts:
            artifact["expected_sha"] = expected_sha
            artifact["stage"] = stage_name
        stages.append({
            "stage": stage_name,
            "run": validated_run,
            "artifacts": artifacts,
        })

    manifest = {
        "manifest_schema_version": 1,
        "release_version": expected_version,
        "candidate_sha": expected_sha,
        "retrieved_at": "",
        "stages": stages,
        "verdict": "pass",
    }
    write_json(args.output, manifest)
    print(f"retrieved {len(stages)} stages; wrote {args.output}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RetrievalError as error:
        print(f"FATAL: {error}", file=sys.stderr)
        raise SystemExit(1)
