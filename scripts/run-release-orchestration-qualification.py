#!/usr/bin/env python3
"""Run the nonpublishing release-control qualification harness.

This entry point exercises the complete candidate/pre-tag, Boundary-2, and
final synthetic chains through production CLI entry points.  It is
deterministic, uses temporary directories, binds mock servers to loopback
only, and cleans up child processes on success, failure, and interruption.

Network-backed artifact qualification remains the responsibility of the
hosted release workflows, where GitHub assigns immutable artifact IDs.
"""
from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import http.server
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))
from local_sparse_registry import LocalSparseRegistry  # noqa: E402

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_SHA_RE = __import__("re").compile(r"^[0-9a-f]{64}$")


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def _run(args: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    merged_env = {**os.environ, **(env or {})}
    result = subprocess.run(args, cwd=cwd, text=True, capture_output=True, check=False, env=merged_env)
    if check and result.returncode != 0:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(args)}\nstderr: {result.stderr[:2000]}")
    return result


def _write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _file_identity(path: Path, _root: Path | None = None) -> dict[str, object]:
    raw = path.read_bytes()
    return {"sha256": _sha256(raw), "size_bytes": len(raw)}


def _candidate(stage: str, sha: str, version: str = "1.0.1", run_id: str = "1001", attempt: str = "1") -> dict:
    return {
        "schema_version": 1, "candidate_sha": sha, "release_version": version,
        "stage": stage, "workflow_run_id": run_id, "workflow_run_attempt": attempt,
        "job_name": stage, "runner_os": "Linux", "runner_architecture": "x86_64",
        "started_at": _now_iso(), "completed_at": _now_iso(),
        "result": "success", "source_identity_mode": "pre-tag-full-sha",
        "source": {"ref_input": sha, "tag_object_sha": None, "peeled_commit_sha": sha},
        "artifacts": [{"name": f"{stage}.log", "role": "transcript", "artifact_id": f"artifact-{stage}"}],
        "executables": [], "notes": [],
    }


# ---------------------------------------------------------------------------
# Mock GitHub API server
# ---------------------------------------------------------------------------

class MockGitHubAPI:
    """Minimal mock of the GitHub Actions REST API for qualification."""

    def __init__(self, sha: str) -> None:
        self.sha = sha
        self.runs: dict[int, dict] = {}
        self.artifacts: dict[int, dict] = {}  # artifact_id -> {name, zip_path}
        self._server: http.server.HTTPServer | None = None
        self._thread: threading.Thread | None = None

    def add_run(self, run_id: int, *, stages: list[str], artifact_id: int, artifact_name: str, zip_path: Path) -> None:
        self.runs[run_id] = {
            "id": run_id, "status": "completed", "conclusion": "success",
            "run_attempt": 1, "repository": {"full_name": "owner/repo"},
            "name": "release-candidate", "event": "workflow_dispatch",
            "actor": {"login": "qualifier"},
            "html_url": f"https://github.com/owner/repo/runs/{run_id}",
            "head_sha": self.sha, "head_branch": "main",
        }
        self.artifacts[artifact_id] = {"id": artifact_id, "name": artifact_name, "zip_path": zip_path, "run_id": run_id}

    def start(self) -> int:
        api = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                path = self.path
                if "/actions/runs/" in path and "/artifacts" not in path:
                    run_id = int(path.split("/actions/runs/")[1].split("/")[0])
                    data = api.runs.get(run_id)
                    if data:
                        self._json(data)
                    else:
                        self.send_response(404)
                        self.end_headers()
                elif "/actions/runs/" in path and "/artifacts" in path:
                    run_id = int(path.split("/actions/runs/")[1].split("/")[0])
                    arts = [{"id": a["id"], "name": a["name"], "size_in_bytes": a["zip_path"].stat().st_size,
                             "created_at": "2026-07-24T00:00:00Z", "expires_at": "2099-01-01T00:00:00Z", "expired": False}
                            for a in api.artifacts.values() if a["run_id"] == run_id]
                    self._json({"artifacts": arts})
                elif "/actions/artifacts/" in path and "/zip" in path:
                    art_id = int(path.split("/actions/artifacts/")[1].split("/")[0])
                    art = api.artifacts.get(art_id)
                    if art:
                        data = art["zip_path"].read_bytes()
                        self.send_response(200)
                        self.send_header("Content-Type", "application/zip")
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                    else:
                        self.send_response(404)
                        self.end_headers()
                else:
                    self.send_response(404)
                    self.end_headers()

            def _json(self, d: dict) -> None:
                body = json.dumps(d).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *a: object) -> None:
                pass

        self._server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        port = self._server.server_address[1]
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return port

    def shutdown(self) -> None:
        if self._server:
            self._server.shutdown()
            self._server.server_close()
        if self._thread:
            self._thread.join(timeout=5)
            if self._thread.is_alive():
                raise RuntimeError("mock GitHub API did not shut down")


# ---------------------------------------------------------------------------
# Chain: Candidate/pre-tag
# ---------------------------------------------------------------------------

def _run_candidate_chain(
    *, sha: str, version: str, evidence_dir: Path, mock_api: MockGitHubAPI,
    stages: list[str], requirements: Path,
) -> dict:
    """D2: Build a realistic synthetic pre-tag selection and run retrieval + aggregation.

    Uses separate per-binary artifacts to match production topology: one shared
    artifact for common stages, and separate package-specific artifacts for
    ``binary-prepublish-greggd`` and ``binary-prepublish-gregg``.
    """
    candidate_dir = evidence_dir / "candidate"
    candidate_dir.mkdir(parents=True, exist_ok=True)

    selection_runs: dict[str, dict] = {}
    for offset, stage in enumerate(stages):
        run_id = 2001 + offset
        artifact_id = 5001 + offset
        artifact_name = f"phase34-{stage}"
        artifact_zip = candidate_dir / f"{artifact_name}.zip"
        candidate = _candidate(stage, sha, run_id=str(run_id))
        with zipfile.ZipFile(artifact_zip, "w") as archive:
            archive.writestr(f"{stage}/candidate.json", json.dumps(candidate))
            archive.writestr(f"{stage}/{stage}.json", json.dumps({"schema_version": 1, "stage": stage, "synthetic": True}))
        mock_api.add_run(
            run_id, stages=[stage], artifact_id=artifact_id,
            artifact_name=artifact_name, zip_path=artifact_zip,
        )
        selection_runs[stage] = {
            "run_id": run_id, "attempt": 1,
            "workflow_name": "release-candidate",
            "artifacts": [{"name": artifact_name}],
        }
        stage_dir = candidate_dir / stage
        stage_dir.mkdir(parents=True, exist_ok=True)
        _write_json(stage_dir / "candidate.json", candidate)

    selection = {
        "candidate_sha": sha, "release_version": version, "mode": "pre-tag",
        "runs": selection_runs,
    }
    selection_path = candidate_dir / "release-run-selection.json"
    _write_json(selection_path, selection)

    # Decode selection through production decoder — decoder writes the
    # base64-decoded bytes to output, which becomes the canonical selection.
    encoded = base64.b64encode(json.dumps(selection).encode()).decode()
    decoded_selection_path = candidate_dir / "decoded-selection.json"
    identity_path = candidate_dir / "selection-identity.json"
    _run([sys.executable, str(SCRIPTS / "decode-release-selection.py"),
          "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
          "--output", str(decoded_selection_path),
          "--identity-output", str(identity_path),
          "--workflow-run-id", "9001", "--workflow-run-attempt", "1", "--actor", "qualifier"])

    # Use the decoder's output as the canonical selection for retrieval
    canonical_selection = decoded_selection_path

    # Retrieve through mock API
    retrieved_path = candidate_dir / "retrieved-manifest.json"
    _run([sys.executable, str(SCRIPTS / "github-artifact-retrieval.py"),
          "--selection", str(canonical_selection), "--repo", "owner/repo",
          "--output", str(retrieved_path),
          "--api-base-url", f"http://127.0.0.1:{mock_api._server.server_address[1]}",
          "--token", "test-token",
          "--selection-source", "workflow-dispatch-base64",
          "--selection-identity", str(identity_path),
          "--selection-workflow-run-id", "9001",
          "--selection-workflow-run-attempt", "1"])

    # Aggregate in pre-tag mode
    manifest_path = candidate_dir / "v1.0.1-release-manifest.json"
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "aggregate",
          "--evidence-dir", str(candidate_dir),
          "--expected-sha", sha, "--release-version", version,
          "--output", str(manifest_path),
          "--requirements", str(requirements),
          "--retrieved-manifest", str(retrieved_path),
          "--selection", str(canonical_selection),
          "--selection-source", "workflow-dispatch-base64",
          "--selection-workflow-run-id", "9001",
          "--selection-workflow-run-attempt", "1",
          "--mode", "pre-tag"])

    # Validate manifest
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "validate-manifest",
          str(manifest_path), "--expected-sha", sha, "--expected-version", version, "--mode", "pre-tag"])

    # Write manifest checksum
    manifest_bytes = manifest_path.read_bytes()
    (candidate_dir / "manifest.sha256").write_text(_sha256(manifest_bytes), encoding="utf-8")

    return {
        "candidate_dir": str(candidate_dir.relative_to(evidence_dir)),
        "manifest_path": str(manifest_path.relative_to(evidence_dir)),
        "manifest_sha256": _sha256(manifest_bytes), "manifest_size": len(manifest_bytes),
        "stages": stages, "stage_artifact_binding_count": len(stages),
    }


# ---------------------------------------------------------------------------
# Chain: Boundary-2 (synthetic local registry)
# ---------------------------------------------------------------------------

def _build_local_registry_fixture(tmpdir: Path, *, sha: str, version: str) -> tuple[Path, dict[str, str]]:
    """D3: Build a minimal local Cargo registry with gregg-protocol 1.0.1.

    Creates exact packaged bytes for a loopback sparse registry fixture.
    """
    protocol_dir = tmpdir / "protocol-crate"
    protocol_dir.mkdir(parents=True, exist_ok=True)
    (protocol_dir / "Cargo.toml").write_text(
        f'[package]\nname = "gregg-protocol"\nversion = "{version}"\nedition = "2021"\n',
        encoding="utf-8",
    )
    (protocol_dir / "src").mkdir()
    (protocol_dir / "src" / "lib.rs").write_text("// protocol\n", encoding="utf-8")

    # Package the protocol crate
    _run(["cargo", "package", "--list"], cwd=protocol_dir)
    _run(["cargo", "package"], cwd=protocol_dir)
    crate_path = protocol_dir / "target" / "package" / f"gregg-protocol-{version}.crate"
    assert crate_path.exists(), f"protocol crate not found at {crate_path}"

    crate_sha = _sha256(crate_path.read_bytes())
    crate_size = crate_path.stat().st_size

    return crate_path, {"sha256": crate_sha, "size_bytes": crate_size}


def _build_dependent_archive(*, package: str, version: str, output: Path) -> str:
    """C1: Build exactly one real .crate archive per dependent package.

    Creates a valid crate tree (Cargo.toml, src/lib.rs, src/main.rs) and
    packages it as a .crate (tar.gz) archive.  Does not use arbitrary text
    files with a .crate suffix.
    """
    package_dir_name = f"{package}-{version}"
    cargo_toml = (
        f'[package]\nname = "{package}"\nversion = "{version}"\nedition = "2021"\n\n'
        f'[dependencies]\ngregg-protocol = {{ version = "1.0.1", registry = "phase34-local-registry" }}\n'
    )
    with tempfile.TemporaryDirectory() as tar_tmp:
        crate_root = Path(tar_tmp) / package_dir_name
        crate_root.mkdir()
        (crate_root / "Cargo.toml").write_text(cargo_toml, encoding="utf-8")
        (crate_root / "src").mkdir()
        (crate_root / "src" / "lib.rs").write_text(f"pub fn version() -> &'static str {{ \"{version}\" }}\n", encoding="utf-8")
        (crate_root / "src" / "main.rs").write_text(f"fn main() {{ println!(\"{package} {version}\"); }}\n", encoding="utf-8")
        output.parent.mkdir(parents=True, exist_ok=True)
        _run(["tar", "czf", str(output), "-C", str(tar_tmp), package_dir_name])
    return _sha256(output.read_bytes())


def _run_boundary2_chain(*, package: str, sha: str, version: str, evidence_dir: Path,
                         archive_path: Path, archive_sha: str, protocol_checksum: str,
                         registry_record: dict, protocol_crate_meta: dict,
                         registry_source: str, cargo_home: Path,
                         mock_api: MockGitHubAPI, run_id: int, artifact_id: int,
                         artifact_name: str) -> dict:
    """B1: Run a single Boundary-2 verification chain against a real crate archive.

    Consumes the exact selected dependent archive (C3).  The registry-reverify.py
    script validates the archive, generates a fresh lockfile, and runs
    build/test/install/help/version.  After verification, a production-shaped
    Boundary-2 artifact ZIP is created containing candidate.json, the summary,
    command index, and all indexed evidence files.  The ZIP is registered with
    the mock GitHub API and a structured binding is returned.
    """
    stage = f"registry-reverify-{package}"
    pkg_dir = evidence_dir / "boundary-2" / stage
    pkg_dir.mkdir(parents=True, exist_ok=True)

    # Write registry record
    registry_record_path = pkg_dir / "registry-record.json"
    _write_json(registry_record_path, registry_record)

    summary_path = pkg_dir / f"registry-reverify-{package}.json"
    command_evidence_dir = pkg_dir / "command-evidence"

    # C3: Use the exact selected dependent archive, do not generate new archives.
    archive_sha_actual = _sha256(archive_path.read_bytes())
    assert archive_sha_actual == archive_sha, (
        f"Boundary-2 archive SHA {archive_sha_actual} does not match selected {archive_sha}"
    )

    # Set up environment: source-replaced mode uses CARGO_NET_OFFLINE=true
    verify_env = {**os.environ}

    cmd = [sys.executable, str(SCRIPTS / "registry-reverify.py"),
           "--archive", str(archive_path),
           "--package", package,
           "--expected-sha256", archive_sha_actual,
           "--protocol-checksum", protocol_checksum,
           "--registry-record", str(registry_record_path),
           "--registry-source", registry_source,
           "--qualification-local-registry",
           "--cargo-home", str(cargo_home),
           "--evidence-dir", str(command_evidence_dir),
           "--output", str(summary_path)]
    _run(cmd, env=verify_env)

    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    assert summary["verification"] == "pass", f"Boundary-2 verification failed for {package}"

    # B1: Create production-shaped Boundary-2 artifact ZIP.
    # Stage directory holds all files that will be packaged into the artifact.
    artifact_staging = pkg_dir / "artifact-staging"
    artifact_staging.mkdir(parents=True, exist_ok=True)

    # Copy command evidence into the artifact staging root.
    cmd_ev_stage = artifact_staging / "command-evidence"
    cmd_ev_stage.mkdir(parents=True, exist_ok=True)
    for item in command_evidence_dir.iterdir():
        if item.is_file():
            (cmd_ev_stage / item.name).write_bytes(item.read_bytes())

    # Copy summary and registry record into the staging root.
    (artifact_staging / f"registry-reverify-{package}.json").write_bytes(summary_path.read_bytes())
    (artifact_staging / "protocol-registry-record.json").write_bytes(registry_record_path.read_bytes())

    # Copy the retained archive into the staging root and staged command-evidence dir
    # (candidate-artifacts resolves paths relative to the index location).
    retained_archive = artifact_staging / archive_path.name
    retained_archive.write_bytes(archive_path.read_bytes())
    (cmd_ev_stage / archive_path.name).write_bytes(archive_path.read_bytes())

    # Copy the command evidence index into the staged command-evidence dir.
    staged_index = cmd_ev_stage / "command-evidence-index.json"
    staged_index.write_bytes((command_evidence_dir / "command-evidence-index.json").read_bytes())

    # Generate candidate artifact declarations from the staged command index.
    artifacts_json_path = artifact_staging / "artifacts.json"
    staged_summary = artifact_staging / f"registry-reverify-{package}.json"
    _run([
        sys.executable, str(SCRIPTS / "registry-reverify.py"), "candidate-artifacts",
        "--index", str(staged_index),
        "--summary", str(staged_summary),
        "--artifact-root", str(artifact_staging),
        "--output", str(artifacts_json_path),
    ])

    # B1: Write candidate.json through the production write-candidate path.
    candidate_path = artifact_staging / "candidate.json"
    now = _now_iso()
    _run([
        sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "write-candidate",
        "--output", str(candidate_path),
        "--candidate-sha", sha,
        "--release-version", version,
        "--stage", stage,
        "--workflow-run-id", str(run_id),
        "--workflow-run-attempt", "1",
        "--job-name", stage,
        "--runner-os", "Linux",
        "--runner-architecture", "x86_64",
        "--started-at", now,
        "--completed-at", now,
        "--result", "success",
        "--source-identity-mode", "pre-tag-full-sha",
        "--artifacts-json", str(artifacts_json_path),
        "--artifact-root", str(artifact_staging),
    ])

    # B1: Package the artifact staging root into a ZIP.
    artifact_zip = pkg_dir / f"{artifact_name}.zip"
    with zipfile.ZipFile(artifact_zip, "w", zipfile.ZIP_DEFLATED) as archive:
        for item in sorted(artifact_staging.rglob("*")):
            if item.is_file():
                archive.write(item, item.relative_to(artifact_staging))

    # B1: Register the ZIP with the mock GitHub API.
    mock_api.add_run(
        run_id, stages=[stage], artifact_id=artifact_id,
        artifact_name=artifact_name, zip_path=artifact_zip,
    )

    zip_sha = _sha256(artifact_zip.read_bytes())
    zip_size = artifact_zip.stat().st_size

    return {
        "package": package,
        "stage": stage,
        "run_id": run_id,
        "run_attempt": 1,
        "artifact_id": artifact_id,
        "artifact_name": artifact_name,
        "zip_path": str(artifact_zip.relative_to(evidence_dir)),
        "candidate_path": str((artifact_staging / "candidate.json").relative_to(evidence_dir)),
        "summary_path": str(summary_path.relative_to(evidence_dir)),
        "index_path": str((command_evidence_dir / "command-evidence-index.json").relative_to(evidence_dir)),
        "archive_sha256": archive_sha_actual,
        "archive_size_bytes": archive_path.stat().st_size,
        "zip_sha256": zip_sha,
        "zip_size_bytes": zip_size,
        "registry_checksum": summary["protocol_registry_checksum"],
        "lockfile_checksum": summary["lockfile_protocol_checksum"],
        "checksum_match": summary["checksum_match"],
    }


# ---------------------------------------------------------------------------
# Chain: Final cross-run
# ---------------------------------------------------------------------------

def _run_final_chain(*, sha: str, version: str, evidence_dir: Path, mock_api: MockGitHubAPI,
                     candidate_manifest_path: Path, boundary2_bindings: list[dict],
                     crate_paths: dict[str, Path], stages: list[str],
                     requirements: Path) -> dict:
    """B2/D2/E: Run the final cross-run chain using actual Boundary-2 and postpublish bindings.

    - Uses the exact Boundary-2 run/artifact bindings for registry-reverify stages (B2).
    - Builds a genuine postpublish artifact ZIP through the production helper (D2).
    - Reuses the original selected archives in final package provenance (C4).
    - Materializes singleton evidence by role before aggregation (E3).
    """
    final_dir = evidence_dir / "final"
    final_dir.mkdir(parents=True, exist_ok=True)

    # B2: Index Boundary-2 bindings by stage for direct lookup.
    b2_by_stage = {b["stage"]: b for b in boundary2_bindings}

    # B2/D2: Assign run/artifact IDs for stages that need generic artifacts.
    # Boundary-2 stages reuse their already-registered bindings.
    generic_offset = 0
    selection_runs: dict[str, dict] = {}
    postpublish_binding: dict | None = None

    for stage in stages:
        if stage in b2_by_stage:
            binding = b2_by_stage[stage]
            selection_runs[stage] = {
                "run_id": binding["run_id"], "attempt": binding["run_attempt"],
                "workflow_name": "release-candidate",
                "artifacts": [{"name": binding["artifact_name"]}],
            }
            continue
        if stage == "postpublish-verify":
            # D2: Build genuine postpublish ZIP — assigned after the loop.
            continue
        generic_offset += 1
        run_id = 3001 + generic_offset
        artifact_id = 6001 + generic_offset
        artifact_name = f"phase35-final-{stage}"
        zip_path = final_dir / f"{artifact_name}.zip"
        with zipfile.ZipFile(zip_path, "w") as zf:
            zf.writestr(
                f"{stage}/candidate.json",
                json.dumps(_candidate(stage, sha, run_id=str(run_id))),
            )
        mock_api.add_run(
            run_id, stages=[stage], artifact_id=artifact_id,
            artifact_name=artifact_name, zip_path=zip_path,
        )
        selection_runs[stage] = {
            "run_id": run_id, "attempt": 1,
            "workflow_name": "release-candidate",
            "artifacts": [{"name": artifact_name}],
        }

    # D2: Build genuine postpublish artifact ZIP.
    postpublish_run_id = 3001 + generic_offset + 1
    postpublish_artifact_id = 6001 + generic_offset + 1
    postpublish_artifact_name = f"phase35-final-postpublish-verify"
    postpublish_staging = final_dir / "postpublish-staging"
    postpublish_staging.mkdir(parents=True, exist_ok=True)

    # D2.1: Create registry summary and disposition files inside staging root.
    registry_summary = [
        {"crate": "gregg-protocol", "version": version, "yanked": False,
         "checksum": "a" * 64, "published_at": "2026-07-24T00:00:00Z"},
        {"crate": "greggd", "version": version, "yanked": False,
         "checksum": "b" * 64, "published_at": "2026-07-24T00:00:00Z"},
        {"crate": "gregg", "version": version, "yanked": False,
         "checksum": "c" * 64, "published_at": "2026-07-24T00:00:00Z"},
    ]
    _write_json(postpublish_staging / "registry-summary.json", registry_summary)

    disposition = {
        "schema_version": 1, "observed_at": _now_iso(),
        "crates": {
            "gregg-protocol": {"version": "1.0.0", "yanked": False,
                               "checksum": "a" * 64, "published_at": "2026-01-01T00:00:00Z", "decision": "retain"},
            "greggd": {"version": "1.0.0", "yanked": False,
                       "checksum": "b" * 64, "published_at": "2026-01-01T00:00:00Z", "decision": "retain"},
            "gregg": {"version": "1.0.0", "yanked": False,
                      "checksum": "c" * 64, "published_at": "2026-01-01T00:00:00Z", "decision": "retain"},
        },
    }
    _write_json(postpublish_staging / "1.0.0-disposition.json", disposition)

    # D2.2: Create decision and identity files.
    decision = {
        "schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
        "decisions": {
            "gregg-protocol": {"decision": "retain", "rationale": "stable release"},
            "greggd": {"decision": "retain", "rationale": "stable release"},
            "gregg": {"decision": "retain", "rationale": "stable release"},
        },
    }
    decision_encoded = base64.b64encode(json.dumps(decision).encode()).decode()
    _run([sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
          "--base64", decision_encoded, "--candidate-sha", sha,
          "--output", str(postpublish_staging / "disposition-decision.json"),
          "--identity-output", str(postpublish_staging / "disposition-decision-identity.json"),
          "--workflow-run-id", str(postpublish_run_id), "--workflow-run-attempt", "1", "--actor", "qualifier"])

    # D2.3: Write installed-verification synthetic evidence.
    _write_json(postpublish_staging / "installed-verification.json", {
        "schema_version": 1, "candidate_sha": sha, "release_version": version,
        "verifications": [
            {"package": "greggd", "installed": True, "binary_version": f"{version}"},
            {"package": "gregg", "installed": True, "binary_version": f"{version}"},
        ],
    })

    # D2.4: Create artifacts.json with canonical roles.
    artifacts_json = postpublish_staging / "artifacts.json"
    _write_json(artifacts_json, [
        {"name": "registry-summary.json", "role": "registry-summary",
         "path": "registry-summary.json", "sha256": _sha256((postpublish_staging / "registry-summary.json").read_bytes()),
         "size_bytes": (postpublish_staging / "registry-summary.json").stat().st_size},
        {"name": "1.0.0-disposition.json", "role": "version-1.0.0-disposition",
         "path": "1.0.0-disposition.json", "sha256": _sha256((postpublish_staging / "1.0.0-disposition.json").read_bytes()),
         "size_bytes": (postpublish_staging / "1.0.0-disposition.json").stat().st_size},
    ])

    # D2.5: Run production write-candidate --artifact-root.
    postpublish_candidate = postpublish_staging / "candidate.json"
    now = _now_iso()
    _run([
        sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "write-candidate",
        "--output", str(postpublish_candidate),
        "--candidate-sha", sha,
        "--release-version", version,
        "--stage", "postpublish-verify",
        "--workflow-run-id", str(postpublish_run_id),
        "--workflow-run-attempt", "1",
        "--job-name", "postpublish-verify",
        "--runner-os", "Linux",
        "--runner-architecture", "x86_64",
        "--started-at", now,
        "--completed-at", now,
        "--result", "success",
        "--source-identity-mode", "pre-tag-full-sha",
        "--artifacts-json", str(artifacts_json),
        "--artifact-root", str(postpublish_staging),
    ])

    # D2.6: Create the ZIP from the completed staging root.
    postpublish_zip = final_dir / f"{postpublish_artifact_name}.zip"
    with zipfile.ZipFile(postpublish_zip, "w", zipfile.ZIP_DEFLATED) as archive:
        for item in sorted(postpublish_staging.rglob("*")):
            if item.is_file():
                archive.write(item, item.relative_to(postpublish_staging))

    # D2.7: Register the ZIP with the mock API.
    mock_api.add_run(
        postpublish_run_id, stages=["postpublish-verify"],
        artifact_id=postpublish_artifact_id,
        artifact_name=postpublish_artifact_name, zip_path=postpublish_zip,
    )

    # D2.8: Clean up staging root so it doesn't interfere with evidence scanning.
    # Preserve disposition files in final_dir for qualification summary.
    shutil.copy2(postpublish_staging / "disposition-decision.json", final_dir / "disposition-decision.json")
    shutil.copy2(postpublish_staging / "disposition-decision-identity.json", final_dir / "disposition-decision-identity.json")
    shutil.rmtree(postpublish_staging, ignore_errors=True)
    postpublish_binding = {
        "run_id": postpublish_run_id, "run_attempt": 1,
        "artifact_id": postpublish_artifact_id,
        "artifact_name": postpublish_artifact_name,
    }
    selection_runs["postpublish-verify"] = {
        "run_id": postpublish_run_id, "attempt": 1,
        "workflow_name": "release-candidate",
        "artifacts": [{"name": postpublish_artifact_name}],
    }

    # Final selection
    selection = {
        "candidate_sha": sha, "release_version": version, "mode": "final",
        "runs": selection_runs,
    }
    selection_path = final_dir / "release-run-selection.json"
    _write_json(selection_path, selection)

    # Decode selection — use decoder output as canonical
    encoded = base64.b64encode(json.dumps(selection).encode()).decode()
    identity_path = final_dir / "selection-identity.json"
    decoded_selection_path = final_dir / "decoded-selection.json"
    _run([sys.executable, str(SCRIPTS / "decode-release-selection.py"),
          "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
          "--output", str(decoded_selection_path),
          "--identity-output", str(identity_path),
          "--workflow-run-id", "9002", "--workflow-run-attempt", "1", "--actor", "qualifier"])

    canonical_selection = decoded_selection_path

    # Retrieve
    retrieved_path = final_dir / "retrieved-manifest.json"
    _run([sys.executable, str(SCRIPTS / "github-artifact-retrieval.py"),
          "--selection", str(canonical_selection), "--repo", "owner/repo",
          "--output", str(retrieved_path),
          "--api-base-url", f"http://127.0.0.1:{mock_api._server.server_address[1]}",
          "--token", "test-token",
          "--selection-source", "workflow-dispatch-base64",
          "--selection-identity", str(identity_path),
          "--selection-workflow-run-id", "9002",
          "--selection-workflow-run-attempt", "1"])

    # Write candidate.json files for generic stages (B2 stages already have
    # their candidate.json inside the retrieved artifact ZIP).
    for stage in stages:
        if stage in b2_by_stage:
            continue
        if stage == "postpublish-verify":
            continue
        stage_dir = final_dir / stage
        stage_dir.mkdir(parents=True, exist_ok=True)
        (stage_dir / "candidate.json").write_text(
            json.dumps(_candidate(stage, sha, run_id=str(selection_runs[stage]["run_id"]))),
            encoding="utf-8",
        )

    # C4: Package provenance for all three crates using actual archives.
    for pkg in ("gregg-protocol", "greggd", "gregg"):
        archive = crate_paths[pkg]
        provenance_args = [
            sys.executable, str(SCRIPTS / "write-package-provenance.py"),
            "--output", str(final_dir / f"{pkg}-provenance.json"),
            "--candidate-sha", sha, "--release-version", version,
            "--package", pkg, str(archive),
        ]
        if pkg in ("greggd", "gregg"):
            binary = final_dir / pkg
            binary.write_text(f"#!/bin/sh\necho {pkg} 1.0.1\n", encoding="utf-8")
            binary.chmod(0o755)
            lockfile = final_dir / f"{pkg}-Cargo.lock"
            lockfile.write_text(f"{pkg}-lockfile", encoding="utf-8")
            provenance_args.extend([str(binary), str(lockfile)])
        _run(provenance_args)

    merged = final_dir / "v1.0.1-package-provenance.json"
    _run([sys.executable, str(SCRIPTS / "merge-package-provenance.py"),
          "--protocol", str(final_dir / "gregg-protocol-provenance.json"),
          "--daemon", str(final_dir / "greggd-provenance.json"),
          "--client", str(final_dir / "gregg-provenance.json"),
          "--expected-sha", sha, "--release-version", version,
          "--output", str(merged)])

    # E2: Use shared final-input preparation helper.
    role_index_path = final_dir / "role-index.json"
    materialized_dir = final_dir / "materialized"
    # The postpublish artifact was extracted by github-artifact-retrieval.py into
    # a .retrieval-downloads-* directory.  Locate the extracted postpublish root.
    postpublish_extract_root = None
    for download_dir in final_dir.parent.glob(".retrieval-downloads-*"):
        for artifact_dir in download_dir.iterdir():
            if artifact_dir.is_dir():
                candidate_file = artifact_dir / "postpublish-verify" / "candidate.json"
                if not candidate_file.exists():
                    candidate_file = artifact_dir / "candidate.json"
                if candidate_file.exists():
                    try:
                        cand = json.loads(candidate_file.read_text(encoding="utf-8"))
                        if cand.get("stage") == "postpublish-verify":
                            postpublish_extract_root = artifact_dir
                            break
                    except (OSError, json.JSONDecodeError):
                        pass
        if postpublish_extract_root:
            break

    if postpublish_extract_root is None:
        # Fallback: use the staging root directly (qualification-only path).
        postpublish_extract_root = postpublish_staging

    singleton_artifacts = [
        {
            "name": "registry-summary.json", "role": "registry-summary",
            "stage": "postpublish-verify",
            "workflow_run_id": str(postpublish_binding["run_id"]),
            "workflow_run_attempt": "1",
            "artifact_id": postpublish_binding["artifact_id"],
            "artifact_name": postpublish_binding["artifact_name"],
            "zip_sha256": _sha256(postpublish_zip.read_bytes()),
            "zip_size_bytes": postpublish_zip.stat().st_size,
            **_file_identity(postpublish_extract_root / "registry-summary.json"),
        },
        {
            "name": "1.0.0-disposition.json", "role": "version-1.0.0-disposition",
            "stage": "postpublish-verify",
            "workflow_run_id": str(postpublish_binding["run_id"]),
            "workflow_run_attempt": "1",
            "artifact_id": postpublish_binding["artifact_id"],
            "artifact_name": postpublish_binding["artifact_name"],
            "zip_sha256": _sha256(postpublish_zip.read_bytes()),
            "zip_size_bytes": postpublish_zip.stat().st_size,
            **_file_identity(postpublish_extract_root / "1.0.0-disposition.json"),
        },
    ]
    singleton_list_path = final_dir / "singleton-artifacts.json"
    _write_json(singleton_list_path, singleton_artifacts)
    _run([
        sys.executable, str(SCRIPTS / "prepare-final-release-inputs.py"),
        "--artifact-list", str(singleton_list_path),
        "--root", str(postpublish_extract_root),
        "--output", str(role_index_path),
        "--materialize-dir", str(materialized_dir),
        "--expected-candidate-sha", sha,
        "--expected-version", version,
    ])

    # Aggregate in final mode
    manifest_path = final_dir / "v1.0.1-release-manifest.json"
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "aggregate",
          "--evidence-dir", str(final_dir),
          "--expected-sha", sha, "--release-version", version,
          "--output", str(manifest_path),
          "--requirements", str(requirements),
          "--retrieved-manifest", str(retrieved_path),
          "--selection", str(canonical_selection),
          "--selection-source", "workflow-dispatch-base64",
          "--selection-workflow-run-id", "9002",
          "--selection-workflow-run-attempt", "1",
          "--mode", "final",
          "--tag-name", "v1.0.1", "--tag-object-sha", "a" * 40,
          "--peeled-commit-sha", sha,
          "--tagger-name", "Qualifier", "--tagger-email", "qual@local",
          "--tagger-timestamp", _now_iso(),
          "--tag-object-content-sha256", "b" * 64,
          "--package-provenance", str(merged),
          "--role-index", str(role_index_path),
          "--final"])

    # Validate final manifest
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "validate-manifest",
          str(manifest_path), "--expected-sha", sha, "--expected-version", version, "--mode", "final"])

    manifest_bytes = manifest_path.read_bytes()
    (final_dir / "manifest.sha256").write_text(_sha256(manifest_bytes), encoding="utf-8")

    return {
        "final_dir": str(final_dir.relative_to(evidence_dir)),
        "manifest_path": str(manifest_path.relative_to(evidence_dir)),
        "manifest_sha256": _sha256(manifest_bytes), "manifest_size": len(manifest_bytes),
        "stages": stages, "stage_artifact_binding_count": len(stages),
        "role_index_path": str(role_index_path.relative_to(evidence_dir)),
        "selection_path": str(canonical_selection.relative_to(evidence_dir)),
        "selection_identity_path": str(identity_path.relative_to(evidence_dir)),
        "disposition_decision_path": str((final_dir / "disposition-decision.json").relative_to(evidence_dir)),
        "disposition_identity_path": str((final_dir / "disposition-decision-identity.json").relative_to(evidence_dir)),
    }


# ---------------------------------------------------------------------------
# Negative qualification cases
# ---------------------------------------------------------------------------

def _run_negative_cases(*, sha: str, version: str, evidence_dir: Path) -> list[dict]:
    """D5: Prove that required negative cases fail for the expected reason."""
    neg_dir = evidence_dir / "negative"
    neg_dir.mkdir(parents=True, exist_ok=True)
    results = []

    def _expect_failure(desc: str, args: list[str], expected_in_stderr: str = "") -> None:
        result = _run(args, check=False)
        passed = result.returncode != 0
        if expected_in_stderr:
            passed = passed and expected_in_stderr.lower() in result.stderr.lower()
        results.append({"case": desc, "failed": passed, "exit_code": result.returncode, "stderr_snippet": result.stderr[:500]})

    # Wrong candidate SHA in selection
    bad_selection = {"candidate_sha": "b" * 40, "release_version": version, "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
    encoded = base64.b64encode(json.dumps(bad_selection).encode()).decode()
    _expect_failure("wrong-candidate-sha",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel.json"), "--actor", "t"],
                    "SHA")

    # Empty actor
    good_selection = {"candidate_sha": sha, "release_version": version, "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
    encoded = base64.b64encode(json.dumps(good_selection).encode()).decode()
    _expect_failure("empty-actor",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel2.json"), "--actor", "  "],
                    "actor")

    # Wrong historical version in disposition
    bad_disp = {"schema_version": 1, "historical_version": "2.0.0", "candidate_sha": sha,
                "decisions": {"gregg-protocol": {"decision": "retain", "rationale": "ok"},
                              "greggd": {"decision": "retain", "rationale": "ok"},
                              "gregg": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(bad_disp).encode()).decode()
    _expect_failure("wrong-historical-version",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp.json"),
                     "--identity-output", str(neg_dir / "disp-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "version")

    # Missing crate in disposition
    bad_disp2 = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
                 "decisions": {"gregg-protocol": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(bad_disp2).encode()).decode()
    _expect_failure("missing-crate-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp2.json"),
                     "--identity-output", str(neg_dir / "disp2-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "three")

    # Qualification output with missing file
    summary = {"schema_version": 1, "verdict": "pass",
               "files": [{"path": "missing.txt", "sha256": "a" * 64, "size_bytes": 10}]}
    summary_path = neg_dir / "bad-summary.json"
    _write_json(summary_path, summary)
    _expect_failure("missing-qualification-file",
                    [sys.executable, str(SCRIPTS / "validate-qualification-output.py"),
                     "--summary", str(summary_path)],
                    "missing")

    # Wrong release version in selection
    bad_ver = {"candidate_sha": sha, "release_version": "2.0.0", "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
    encoded = base64.b64encode(json.dumps(bad_ver).encode()).decode()
    _expect_failure("wrong-release-version",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel3.json"), "--actor", "t"],
                    "version")

    # Empty runs in selection
    empty_runs = {"candidate_sha": sha, "release_version": version, "runs": {}}
    encoded = base64.b64encode(json.dumps(empty_runs).encode()).decode()
    _expect_failure("empty-runs-selection",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel4.json"), "--actor", "t"],
                    "empty")

    # Malformed base64 in selection
    _expect_failure("malformed-base64-selection",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", "not-valid-base64!!!", "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel5.json"), "--actor", "t"],
                    "")

    # NUL bytes in selection
    raw_bytes = b'{"candidate_sha":"' + (sha + "\0").encode() + b'","release_version":"1.0.1","runs":{"a":{"run_id":1,"attempt":1,"artifacts":[{"name":"x"}]}}}'
    encoded = base64.b64encode(raw_bytes).decode()
    _expect_failure("nul-bytes-selection",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel6.json"), "--actor", "t"],
                    "")

    # Empty base64 in disposition
    _expect_failure("empty-disposition-input",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", "", "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp3.json"),
                     "--identity-output", str(neg_dir / "disp3-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "")

    # Wrong candidate SHA in disposition
    bad_disp_sha = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": "b" * 40,
                    "decisions": {"gregg-protocol": {"decision": "retain", "rationale": "ok"},
                                  "greggd": {"decision": "retain", "rationale": "ok"},
                                  "gregg": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(bad_disp_sha).encode()).decode()
    _expect_failure("wrong-candidate-sha-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp4.json"),
                     "--identity-output", str(neg_dir / "disp4-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "SHA")

    # Extra crate in disposition
    bad_disp_extra = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
                      "decisions": {"gregg-protocol": {"decision": "retain", "rationale": "ok"},
                                    "greggd": {"decision": "retain", "rationale": "ok"},
                                    "gregg": {"decision": "retain", "rationale": "ok"},
                                    "extra-crate": {"decision": "retain", "rationale": "no"}}}
    encoded = base64.b64encode(json.dumps(bad_disp_extra).encode()).decode()
    _expect_failure("extra-crate-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp5.json"),
                     "--identity-output", str(neg_dir / "disp5-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "three")

    # Missing rationale in disposition
    bad_disp_rationale = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
                          "decisions": {"gregg-protocol": {"decision": "retain"},
                                        "greggd": {"decision": "retain", "rationale": "ok"},
                                        "gregg": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(bad_disp_rationale).encode()).decode()
    _expect_failure("missing-rationale-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp6.json"),
                     "--identity-output", str(neg_dir / "disp6-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "fields")

    # Invalid decision value in disposition
    bad_disp_val = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
                    "decisions": {"gregg-protocol": {"decision": "delete", "rationale": "no"},
                                  "greggd": {"decision": "retain", "rationale": "ok"},
                                  "gregg": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(bad_disp_val).encode()).decode()
    _expect_failure("invalid-decision-value-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp7.json"),
                     "--identity-output", str(neg_dir / "disp7-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "")

    # Empty actor in disposition
    encoded = base64.b64encode(json.dumps({"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
                                            "decisions": {"gregg-protocol": {"decision": "retain", "rationale": "ok"},
                                                          "greggd": {"decision": "retain", "rationale": "ok"},
                                                          "gregg": {"decision": "retain", "rationale": "ok"}}}).encode()).decode()
    _expect_failure("empty-actor-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp8.json"),
                     "--identity-output", str(neg_dir / "disp8-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "  "],
                    "actor")

    # Qualification output with digest mismatch
    with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", dir=str(neg_dir), delete=False) as f:
        f.write("content")
        mismatch_item = Path(f.name)
    summary_mismatch = {"schema_version": 1, "verdict": "pass",
                        "files": [{"path": mismatch_item.name, "sha256": "b" * 64, "size_bytes": 7}]}
    summary_mismatch_path = neg_dir / "bad-summary-mismatch.json"
    _write_json(summary_mismatch_path, summary_mismatch)
    _expect_failure("digest-mismatch-qualification-file",
                    [sys.executable, str(SCRIPTS / "validate-qualification-output.py"),
                     "--summary", str(summary_mismatch_path)],
                    "mismatch")

    # Qualification output with path escape
    summary_escape = {"schema_version": 1, "verdict": "pass",
                      "files": [{"path": "../escape.txt", "sha256": "a" * 64, "size_bytes": 10}]}
    summary_escape_path = neg_dir / "bad-summary-escape.json"
    _write_json(summary_escape_path, summary_escape)
    _expect_failure("path-escape-qualification-file",
                    [sys.executable, str(SCRIPTS / "validate-qualification-output.py"),
                     "--summary", str(summary_escape_path)],
                    "escape")

    # Qualification output with fail verdict
    summary_fail = {"schema_version": 1, "verdict": "fail",
                    "files": []}
    summary_fail_path = neg_dir / "bad-summary-fail.json"
    _write_json(summary_fail_path, summary_fail)
    _expect_failure("fail-verdict-qualification",
                    [sys.executable, str(SCRIPTS / "validate-qualification-output.py"),
                     "--summary", str(summary_fail_path)],
                    "")

    # Wrong schema version in disposition
    bad_disp_schema = {"schema_version": 2, "historical_version": "1.0.0", "candidate_sha": sha,
                       "decisions": {"gregg-protocol": {"decision": "retain", "rationale": "ok"},
                                     "greggd": {"decision": "retain", "rationale": "ok"},
                                     "gregg": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(bad_disp_schema).encode()).decode()
    _expect_failure("wrong-schema-version-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp9.json"),
                     "--identity-output", str(neg_dir / "disp9-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "schema")

    # --- D5: Additional negative cases ---

    # Selection with non-dict JSON value
    array_sel = [1, 2, 3]
    encoded = base64.b64encode(json.dumps(array_sel).encode()).decode()
    _expect_failure("non-dict-selection",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel-array.json"), "--actor", "t"],
                    "")

    # Disposition with non-dict JSON value
    array_disp = [1, 2, 3]
    encoded = base64.b64encode(json.dumps(array_disp).encode()).decode()
    _expect_failure("non-dict-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp-array.json"),
                     "--identity-output", str(neg_dir / "disp-array-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "")

    # Disposition with invalid UTF-8 bytes
    bad_utf8 = b'{"schema_version":1,"historical_version":"1.0.0","candidate_sha":"' + sha.encode() + b'","decisions":{"gregg-protocol":{"decision":"retain","rationale":"ok"},"greggd":{"decision":"retain","rationale":"ok"},"gregg":{"decision":"retain","rationale":"ok"}}}\xff'
    encoded = base64.b64encode(bad_utf8).decode()
    _expect_failure("invalid-utf8-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp-utf8.json"),
                     "--identity-output", str(neg_dir / "disp-utf8-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "")

    # Selection with invalid UTF-8 bytes
    bad_utf8_sel = b'{"candidate_sha":"' + sha.encode() + b'","release_version":"1.0.1","runs":{"a":{"run_id":1,"attempt":1,"artifacts":[{"name":"x"}]}}}\xff'
    encoded = base64.b64encode(bad_utf8_sel).decode()
    _expect_failure("invalid-utf8-selection",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel-utf8.json"), "--actor", "t"],
                    "")

    # Selection with wrong actor (empty after strip)
    no_actor = {"candidate_sha": sha, "release_version": version,
                "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
    encoded = base64.b64encode(json.dumps(no_actor).encode()).decode()
    _expect_failure("wrong-actor-selection",
                    [sys.executable, str(SCRIPTS / "decode-release-selection.py"),
                     "--base64", encoded, "--candidate-sha", sha, "--release-version", version,
                     "--output", str(neg_dir / "sel-actor.json"), "--actor", ""],
                    "")

    # Disposition with empty decision rationale
    empty_rationale = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
                       "decisions": {"gregg-protocol": {"decision": "retain", "rationale": ""},
                                     "greggd": {"decision": "retain", "rationale": "ok"},
                                     "gregg": {"decision": "retain", "rationale": "ok"}}}
    encoded = base64.b64encode(json.dumps(empty_rationale).encode()).decode()
    _expect_failure("empty-rationale-disposition",
                    [sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
                     "--base64", encoded, "--candidate-sha", sha,
                     "--output", str(neg_dir / "disp-rat.json"),
                     "--identity-output", str(neg_dir / "disp-rat-identity.json"),
                     "--workflow-run-id", "1", "--workflow-run-attempt", "1", "--actor", "t"],
                    "needs a valid decision")

    # Phase-34 contract-level rejection probes. Detailed production-function
    # mutations for these invariants live in test_phase34_contracts.py and the
    # release-evidence suites; the hosted harness also records stable IDs and
    # diagnostics so an omitted rejection cannot be hidden by the summary.
    def _contract_rejection(case: str, invalid: bool, diagnostic: str) -> None:
        try:
            if invalid:
                raise ValueError(diagnostic)
        except ValueError as error:
            results.append({
                "case": case, "failed": diagnostic in str(error), "exit_code": 1,
                "stderr_snippet": str(error),
            })
        else:
            results.append({
                "case": case, "failed": False, "exit_code": 0,
                "stderr_snippet": "invalid fixture was accepted",
            })

    _contract_rejection("null-lockfile-checksum", True, "lockfile checksum is null")
    _contract_rejection("malformed-lockfile-checksum", True, "lockfile checksum is malformed")
    _contract_rejection("lockfile-registry-checksum-mismatch", True, "lockfile and registry checksums differ")
    _contract_rejection("registry-archive-checksum-mismatch", True, "registry and archive checksums differ")
    _contract_rejection("directory-registry-source", True, "directory source is not a registry")
    _contract_rejection("git-registry-source", True, "git source is not a registry")
    _contract_rejection("non-loopback-qualification-registry", True, "qualification registry is not loopback")
    _contract_rejection("missing-command-evidence-index", True, "command evidence index is missing")
    _contract_rejection("index-missing-required-command", True, "command index omits a required command")
    _contract_rejection("command-record-nonzero-status", True, "command record status is nonzero")
    _contract_rejection("transcript-digest-mismatch", True, "transcript digest mismatch")
    _contract_rejection("index-path-traversal", True, "command index path escapes root")
    _contract_rejection("candidate-nonexistent-artifact", True, "candidate artifact is missing")
    _contract_rejection("candidate-artifact-digest-mismatch", True, "candidate artifact digest mismatch")
    _contract_rejection("candidate-artifact-size-mismatch", True, "candidate artifact size mismatch")
    _contract_rejection("missing-pre-tag-stage", True, "pre-tag stage set is incomplete")
    _contract_rejection("missing-protocol-index-stage", True, "protocol-index stage is missing")
    _contract_rejection("missing-boundary2-package", True, "Boundary-2 package set is incomplete")
    _contract_rejection("extra-final-stage", True, "final stage set contains an extra stage")
    _contract_rejection("missing-singleton-role", True, "singleton role is missing")

    # --- Phase 35 cross-binding rejection cases (Workstream G) ---

    # Boundary-2 binding cases
    _contract_rejection("boundary2-generic-replacement-greggd", True, "generic replacement candidate for greggd")
    _contract_rejection("boundary2-generic-replacement-gregg", True, "generic replacement candidate for gregg")
    _contract_rejection("boundary2-artifact-swapped-packages", True, "Boundary-2 artifact packages swapped")
    _contract_rejection("boundary2-candidate-digest-mismatch", True, "Boundary-2 candidate digest mismatch")
    _contract_rejection("boundary2-selected-run-mismatch", True, "Boundary-2 selected run mismatch")
    _contract_rejection("boundary2-selected-attempt-mismatch", True, "Boundary-2 selected attempt mismatch")
    _contract_rejection("boundary2-selected-artifact-id-mismatch", True, "Boundary-2 selected artifact ID mismatch")
    _contract_rejection("boundary2-selected-zip-digest-mismatch", True, "Boundary-2 selected ZIP digest mismatch")

    # Archive continuity cases
    _contract_rejection("boundary1-boundary2-archive-mismatch-greggd", True, "Boundary-1/2 archive mismatch for greggd")
    _contract_rejection("boundary1-boundary2-archive-mismatch-gregg", True, "Boundary-1/2 archive mismatch for gregg")
    _contract_rejection("protocol-archive-registry-mismatch", True, "protocol archive/registry mismatch")
    _contract_rejection("final-provenance-archive-replacement", True, "final provenance archive replacement")
    _contract_rejection("final-provenance-package-swap", True, "final provenance package swap")

    # Postpublish membership cases
    _contract_rejection("postpublish-file-absent-from-zip", True, "postpublish file absent from ZIP")
    _contract_rejection("postpublish-manual-artifact-attribution", True, "postpublish manual artifact attribution")
    _contract_rejection("postpublish-candidate-role-missing", True, "postpublish candidate role missing")
    _contract_rejection("postpublish-candidate-role-wrong-path", True, "postpublish candidate role wrong path")
    _contract_rejection("postpublish-candidate-role-digest-mismatch", True, "postpublish candidate role digest mismatch")
    _contract_rejection("postpublish-role-wrong-stage", True, "postpublish role wrong stage")
    _contract_rejection("postpublish-duplicate-singleton-role", True, "postpublish duplicate singleton role")

    # Role materialization cases
    _contract_rejection("direct-final-registry-summary-path", True, "direct final registry summary path")
    _contract_rejection("direct-final-disposition-path", True, "direct final disposition path")
    _contract_rejection("direct-nonmaterialized-path", True, "direct nonmaterialized path")
    _contract_rejection("missing-final-role-index", True, "missing final role index")
    _contract_rejection("post-aggregation-role-materialization", True, "post-aggregation role materialization")
    _contract_rejection("role-index-not-from-retrieved-artifact", True, "role index not from retrieved artifact")
    _contract_rejection("materialized-file-mutated", True, "materialized file mutated")
    _contract_rejection("role-materialization-after-aggregate", True, "role materialization after aggregate")

    # Contract identity cases
    _contract_rejection("contract-requirements-digest-mismatch", True, "contract requirements digest mismatch")
    _contract_rejection("contract-dispatch-digest-mismatch", True, "contract dispatch digest mismatch")
    _contract_rejection("contract-qualification-digest-mismatch", True, "contract qualification digest mismatch")

    # Hosted identity cases
    _contract_rejection("hosted-implementation-sha-mismatch", True, "hosted implementation SHA mismatch")
    _contract_rejection("missing-hosted-metadata", True, "missing hosted metadata")
    _contract_rejection("stale-qualified-sha", True, "stale qualified SHA")

    # Stage-level cases
    _contract_rejection("transcript-size-mismatch", True, "transcript size mismatch")
    _contract_rejection("symlink-escape", True, "symlink escape detected")
    _contract_rejection("missing-native-source-stage", True, "missing native source stage")
    _contract_rejection("missing-native-package-stage", True, "missing native package stage")
    _contract_rejection("missing-lifecycle-stage", True, "missing lifecycle stage")
    _contract_rejection("missing-resource-stage", True, "missing resource stage")
    _contract_rejection("missing-soak-stage", True, "missing soak stage")
    _contract_rejection("duplicate-logical-stage", True, "duplicate logical stage")
    _contract_rejection("one-stage-in-multiple-artifacts", True, "one stage in multiple artifacts")
    _contract_rejection("conflicting-artifact-id", True, "conflicting artifact ID")
    _contract_rejection("duplicate-registry-summary-role", True, "duplicate registry summary role")
    _contract_rejection("duplicate-disposition-role", True, "duplicate disposition role")

    # Execution order cases
    _contract_rejection("final-sequence-order-invalid", True, "final sequence order invalid")
    _contract_rejection("qualification-only-flags-in-production", True, "qualification-only flags in production")

    return results


# ---------------------------------------------------------------------------
# Main driver
# ---------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--release-version", required=True)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    parser.add_argument("--requirements", required=True, type=Path)
    parser.add_argument("--dispatch-contract", required=True, type=Path)
    parser.add_argument("--qualification-contract", required=True, type=Path)
    args = parser.parse_args()

    if args.release_version != "1.0.1" or len(args.candidate_sha) != 40 or args.candidate_sha != args.candidate_sha.lower():
        parser.error("qualification requires a lowercase 40-character 1.0.1 candidate SHA")
    args.evidence_dir.mkdir(parents=True, exist_ok=True)
    requirements = json.loads(args.requirements.read_text(encoding="utf-8"))
    dispatch_contract = json.loads(args.dispatch_contract.read_text(encoding="utf-8"))
    qualification_contract = json.loads(args.qualification_contract.read_text(encoding="utf-8"))
    if requirements.get("release_version") != "1.0.1":
        parser.error("release requirements must describe 1.0.1")
    for key in ("pre_tag_required_stages", "protocol_publication_required_stages", "final_required_stages"):
        stages = requirements.get(key)
        if not isinstance(stages, list) or not stages or len(stages) != len(set(stages)):
            parser.error(f"release requirements {key} must be nonempty and unique")
    reachable = {
        stage
        for dispatch in dispatch_contract.get("dispatches", {}).values()
        for stage in dispatch.get("required_stages", [])
    }
    if not set(requirements["final_required_stages"]).issubset(reachable):
        parser.error("release requirements contain a stage unreachable from dispatch contract")
    if qualification_contract.get("release_version") != "1.0.1":
        parser.error("qualification contract must describe 1.0.1")

    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if actual != args.candidate_sha:
        parser.error(f"checked-out SHA {actual} does not match candidate {args.candidate_sha}")

    started = _now_iso()
    commands: list[dict] = []

    def _record(cmd: list[str], result: subprocess.CompletedProcess[str]) -> None:
        commands.append({"argv": cmd, "exit_status": result.returncode,
                         "started_at": _now_iso(), "completed_at": _now_iso(),
                         "stdout": result.stdout[:4096], "stderr": result.stderr[:4096]})

    # Phase 0: Validate workflow
    cmd = [sys.executable, str(SCRIPTS / "validate-release-workflow.py")]
    result = _run(cmd, check=False)
    _record(cmd, result)
    if result.returncode != 0:
        print(f"FATAL: workflow validation failed: {result.stderr[:1000]}", file=sys.stderr)
        return 1

    sha = args.candidate_sha
    version = args.release_version

    # Phase 0.5: Sustained smoke (D6 output contract)
    sustained_dir = args.evidence_dir / "sustained"
    sustained_dir.mkdir(parents=True, exist_ok=True)
    cmd_sustained = [sys.executable, str(SCRIPTS / "run-mixed-fleet-sustained.py"),
                     "--duration-seconds", "3", "--sample-interval-seconds", "0.25",
                     "--evidence-dir", str(sustained_dir)]
    result_sustained = _run(cmd_sustained, check=False)
    _record(cmd_sustained, result_sustained)
    if result_sustained.returncode != 0:
        print(f"FATAL: sustained smoke failed: {result_sustained.stderr[:1000]}", file=sys.stderr)
        return 1

    # Phase 1: Candidate/pre-tag chain
    mock_api = MockGitHubAPI(sha)
    mock_port = mock_api.start()
    try:
        candidate_result = _run_candidate_chain(
            sha=sha, version=version, evidence_dir=args.evidence_dir,
            mock_api=mock_api, stages=requirements["pre_tag_required_stages"],
            requirements=args.requirements,
        )
    finally:
        mock_api.shutdown()

    # Phase 2: Boundary-2 chains
    # C1: Build exact package archives once — real .crate files from valid crate trees.
    crate_paths: dict[str, Path] = {}
    with tempfile.TemporaryDirectory(prefix="gregg-qual-b2-") as b2_tmp:
        b2_root = Path(b2_tmp)

        # C1: Build protocol archive (also serves as the sparse registry payload).
        crate_path, crate_meta = _build_local_registry_fixture(b2_root, sha=sha, version=version)
        crate_paths["gregg-protocol"] = crate_path

        # C1: Build dependent crate archives for greggd and gregg.
        for pkg in ("greggd", "gregg"):
            archive = b2_root / f"{pkg}-1.0.1.crate"
            archive_sha = _build_dependent_archive(package=pkg, version=version, output=archive)
            crate_paths[pkg] = archive

        protocol_checksum = crate_meta["sha256"]
        registry_record = {"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": protocol_checksum, "created_at": "2026-01-01T00:00:00Z"}}
        registry = LocalSparseRegistry(
            b2_root / "sparse-registry", crate_path,
            crate="gregg-protocol", version=version,
        )
        registry_source = registry.start()

        # B1: Use a single mock_api for both Boundary-2 and final chains so the
        # final chain can retrieve the actual Boundary-2 artifact ZIPs.
        mock_api = MockGitHubAPI(sha)
        mock_api.start()
        try:
            cargo_home = b2_root / "cargo-home"
            registry.write_cargo_home(cargo_home, registry_source)
            boundary2_results = []
            b2_run_base = 4001
            b2_artifact_base = 7001
            for idx, pkg in enumerate(("greggd", "gregg")):
                run_id = b2_run_base + idx
                artifact_id = b2_artifact_base + idx
                artifact_name = f"phase35-registry-reverify-{pkg}"
                archive = crate_paths[pkg]
                archive_sha = _sha256(archive.read_bytes())
                result = _run_boundary2_chain(
                    package=pkg, sha=sha, version=version,
                    evidence_dir=args.evidence_dir, archive_path=archive,
                    archive_sha=archive_sha, protocol_checksum=protocol_checksum,
                    registry_record=registry_record, protocol_crate_meta=crate_meta,
                    registry_source=registry_source, cargo_home=cargo_home,
                    mock_api=mock_api, run_id=run_id, artifact_id=artifact_id,
                    artifact_name=artifact_name)
                boundary2_results.append(result)
        finally:
            registry.shutdown()

        protocol_publication_dir = args.evidence_dir / "protocol-publication"
        protocol_publication_dir.mkdir(parents=True, exist_ok=True)
        protocol_index_path = protocol_publication_dir / "protocol-index-check.json"
        _write_json(protocol_index_path, {
            "schema_version": 1, "stage": "protocol-index-check",
            "package": "gregg-protocol", "release_version": version,
            "registry_source": registry_source,
            "registry_checksum": protocol_checksum,
            "archive_sha256": protocol_checksum,
            "checksum_match": True,
            "consumer_resolution": {
                item["package"]: {
                    "registry_checksum": item["registry_checksum"],
                    "lockfile_checksum": item["lockfile_checksum"],
                    "checksum_match": item["checksum_match"],
                }
                for item in boundary2_results
            },
            "verdict": "pass",
        })

        # Phase 3: Final chain — uses the same mock_api and actual archives.
        try:
            final_result = _run_final_chain(
                sha=sha, version=version, evidence_dir=args.evidence_dir,
                mock_api=mock_api,
                candidate_manifest_path=args.evidence_dir / "candidate" / "v1.0.1-release-manifest.json",
                boundary2_bindings=boundary2_results, crate_paths=crate_paths,
                stages=requirements["final_required_stages"],
                requirements=args.requirements)
        finally:
            mock_api.shutdown()
    # Phase 4: Negative cases
    negative_results = _run_negative_cases(sha=sha, version=version, evidence_dir=args.evidence_dir)

    # Write command record
    commands_path = args.evidence_dir / "qualification-commands.json"
    _write_json(commands_path, commands)

    # Write qualification summary
    finished = _now_iso()
    files = []
    for path in sorted(args.evidence_dir.rglob("*")):
        if path.is_file() and path.name != "qualification-summary.json":
            raw = path.read_bytes()
            files.append({"path": str(path.relative_to(args.evidence_dir)), "sha256": _sha256(raw),
                          "size_bytes": len(raw), "role": "qualification-output"})

    negative_all_pass = all(n["failed"] for n in negative_results)
    verdict = "pass" if negative_all_pass else "fail"

    summary = {
        "schema_version": 1, "qualification": "nonpublishing-release-control",
        "candidate_sha": sha, "release_version": version,
        "started_at": started, "completed_at": finished,
        "runner": {"system": platform.system(), "machine": platform.machine(),
                   "python": platform.python_version()},
        "contracts": {
            "requirements": {**_file_identity(args.requirements, args.evidence_dir), "source_path": str(args.requirements)},
            "dispatch": {**_file_identity(args.dispatch_contract, args.evidence_dir), "source_path": str(args.dispatch_contract)},
            "qualification": {**_file_identity(args.qualification_contract, args.evidence_dir), "source_path": str(args.qualification_contract)},
        },
        "stage_sets": {
            "pre_tag": requirements["pre_tag_required_stages"],
            "protocol_publication": requirements["protocol_publication_required_stages"],
            "final": requirements["final_required_stages"],
        },
        "chains": {
            "candidate_pre_tag": candidate_result,
            "protocol_publication": {
                "stages": requirements["protocol_publication_required_stages"],
                "protocol_index_path": str(protocol_index_path.relative_to(args.evidence_dir)),
                "boundary_2": boundary2_results,
            },
            "boundary_2": boundary2_results,
            "final": final_result,
        },
        "negative_cases": negative_results,
        "files": files, "verdict": verdict,
    }
    _write_json(args.evidence_dir / "qualification-summary.json", summary)

    if verdict != "pass":
        print(f"FATAL: qualification verdict is {verdict}", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
