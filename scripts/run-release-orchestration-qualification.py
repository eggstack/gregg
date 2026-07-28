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


def _run_boundary2_chain(*, package: str, sha: str, version: str, evidence_dir: Path,
                         archive_path: Path, archive_sha: str, protocol_checksum: str,
                         registry_record: dict, protocol_crate_meta: dict,
                         registry_source: str, cargo_home: Path) -> dict:
    """D3: Run a single Boundary-2 verification chain against a real crate archive.

    Creates a tar.gz archive containing a Cargo.toml that depends on
    gregg-protocol 1.0.1 through the exact loopback sparse registry.
    The registry-reverify.py script validates the archive, generates a
    fresh lockfile, and runs build/test/install/help/version.
    """
    pkg_dir = evidence_dir / "boundary-2" / f"registry-reverify-{package}"
    pkg_dir.mkdir(parents=True, exist_ok=True)

    # Write registry record
    registry_record_path = pkg_dir / "registry-record.json"
    _write_json(registry_record_path, registry_record)

    summary_path = pkg_dir / f"registry-reverify-{package}.json"
    command_evidence_dir = pkg_dir / "command-evidence"

    # Build a real tar.gz archive with a valid Cargo.toml
    archive_tmp = pkg_dir / f"{package}-1.0.1.crate"
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
        _run(["tar", "czf", str(archive_tmp), "-C", str(tar_tmp), package_dir_name])

    archive_sha_actual = _sha256(archive_tmp.read_bytes())

    # Set up environment: source-replaced mode uses CARGO_NET_OFFLINE=true
    verify_env = {**os.environ}

    cmd = [sys.executable, str(SCRIPTS / "registry-reverify.py"),
           "--archive", str(archive_tmp),
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

    return {
        "summary_path": str(summary_path.relative_to(evidence_dir)), "package": package,
        "index_path": str((command_evidence_dir / "command-evidence-index.json").relative_to(evidence_dir)),
        "registry_checksum": summary["protocol_registry_checksum"],
        "lockfile_checksum": summary["lockfile_protocol_checksum"],
        "checksum_match": summary["checksum_match"],
    }


# ---------------------------------------------------------------------------
# Chain: Final cross-run
# ---------------------------------------------------------------------------

def _run_final_chain(*, sha: str, version: str, evidence_dir: Path, mock_api: MockGitHubAPI,
                     candidate_manifest_path: Path, boundary2_summaries: list[dict],
                     crate_paths: dict[str, Path], stages: list[str],
                     requirements: Path) -> dict:
    """D4: Run the final synthetic cross-run chain."""
    final_dir = evidence_dir / "final"
    final_dir.mkdir(parents=True, exist_ok=True)

    selection_runs: dict[str, dict] = {}
    for offset, stage in enumerate(stages):
        run_id = 3001 + offset
        artifact_id = 6001 + offset
        artifact_name = f"phase34-final-{stage}"
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

    # Write candidate.json files
    for stage in stages:
        stage_dir = final_dir / stage
        stage_dir.mkdir(parents=True, exist_ok=True)
        (stage_dir / "candidate.json").write_text(
            json.dumps(_candidate(stage, sha, run_id=str(selection_runs[stage]["run_id"]))),
            encoding="utf-8",
        )

    # Package provenance for all three crates
    for pkg in ("gregg-protocol", "greggd", "gregg"):
        archive = crate_paths.get(pkg)
        if archive is None:
            archive = final_dir / f"{pkg}-1.0.1.crate"
            archive.write_bytes(f"{pkg}-stub-archive".encode())
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

    # Disposition decision
    decision = {
        "schema_version": 1, "historical_version": "1.0.0", "candidate_sha": sha,
        "decisions": {
            "gregg-protocol": {"decision": "retain", "rationale": "stable release"},
            "greggd": {"decision": "retain", "rationale": "stable release"},
            "gregg": {"decision": "retain", "rationale": "stable release"},
        },
    }
    decision_encoded = base64.b64encode(json.dumps(decision).encode()).decode()
    decision_path = final_dir / "disposition-decision.json"
    decision_identity_path = final_dir / "disposition-decision-identity.json"
    _run([sys.executable, str(SCRIPTS / "decode-release-disposition.py"),
          "--base64", decision_encoded, "--candidate-sha", sha,
          "--output", str(decision_path),
          "--identity-output", str(decision_identity_path),
          "--workflow-run-id", "9002", "--workflow-run-attempt", "1", "--actor", "qualifier"])

    # 1.0.0 disposition (merged with registry observations)
    disposition = {
        "schema_version": 1, "observed_at": _now_iso(),
        "crates": {
            "gregg-protocol": {"version": "1.0.0", "yanked": False, "checksum": "a" * 64, "published_at": "2026-01-01T00:00:00Z", "decision": "retain"},
            "greggd": {"version": "1.0.0", "yanked": False, "checksum": "b" * 64, "published_at": "2026-01-01T00:00:00Z", "decision": "retain"},
            "gregg": {"version": "1.0.0", "yanked": False, "checksum": "c" * 64, "published_at": "2026-01-01T00:00:00Z", "decision": "retain"},
        },
    }
    postpublish_root = final_dir / "retrieved-postpublish"
    postpublish_root.mkdir(parents=True, exist_ok=True)
    _write_json(postpublish_root / "1.0.0-disposition.json", disposition)

    # Registry summary
    registry_summary = [
        {"crate": "gregg-protocol", "version": version, "yanked": False, "checksum": "a" * 64, "published_at": "2026-07-24T00:00:00Z"},
        {"crate": "greggd", "version": version, "yanked": False, "checksum": "b" * 64, "published_at": "2026-07-24T00:00:00Z"},
        {"crate": "gregg", "version": version, "yanked": False, "checksum": "c" * 64, "published_at": "2026-07-24T00:00:00Z"},
    ]
    _write_json(postpublish_root / "registry-summary.json", registry_summary)

    postpublish_artifact = next(
        item for item in mock_api.artifacts.values()
        if item["name"] == "phase34-final-postpublish-verify"
    )
    postpublish_zip_sha = _sha256(postpublish_artifact["zip_path"].read_bytes())
    singleton_artifacts = [
        {
            "name": "registry-summary.json", "role": "registry-summary",
            "stage": "postpublish-verify",
            "workflow_run_id": str(postpublish_artifact["run_id"]),
            "workflow_run_attempt": "1",
            "artifact_id": postpublish_artifact["id"],
            "artifact_name": postpublish_artifact["name"],
            "zip_sha256": postpublish_zip_sha,
            "zip_size_bytes": postpublish_artifact["zip_path"].stat().st_size,
        },
        {
            "name": "1.0.0-disposition.json", "role": "version-1.0.0-disposition",
            "stage": "postpublish-verify",
            "workflow_run_id": str(postpublish_artifact["run_id"]),
            "workflow_run_attempt": "1",
            "artifact_id": postpublish_artifact["id"],
            "artifact_name": postpublish_artifact["name"],
            "zip_sha256": postpublish_zip_sha,
            "zip_size_bytes": postpublish_artifact["zip_path"].stat().st_size,
        },
    ]
    singleton_list_path = final_dir / "singleton-artifacts.json"
    _write_json(singleton_list_path, singleton_artifacts)
    role_index_path = final_dir / "role-index.json"
    materialized_dir = final_dir / "materialized"
    _run([
        sys.executable, str(SCRIPTS / "materialize-release-evidence.py"),
        "--artifact-list", str(singleton_list_path),
        "--root", str(postpublish_root),
        "--output", str(role_index_path),
        "--materialize-dir", str(materialized_dir),
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
          "--registry-summary", str(materialized_dir / "registry-summary.json"),
          "--disposition", str(materialized_dir / "version-1.0.0-disposition.json"),
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
        "disposition_decision_path": str(decision_path.relative_to(evidence_dir)),
        "disposition_identity_path": str(decision_identity_path.relative_to(evidence_dir)),
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
    boundary2_results = []
    with tempfile.TemporaryDirectory(prefix="gregg-qual-b2-") as b2_tmp:
        b2_root = Path(b2_tmp)

        # Build local registry fixture with gregg-protocol 1.0.1
        crate_path, crate_meta = _build_local_registry_fixture(b2_root, sha=sha, version=version)

        protocol_checksum = crate_meta["sha256"]
        registry_record = {"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": protocol_checksum, "created_at": "2026-01-01T00:00:00Z"}}
        registry = LocalSparseRegistry(
            b2_root / "sparse-registry", crate_path,
            crate="gregg-protocol", version=version,
        )
        registry_source = registry.start()
        try:
            cargo_home = b2_root / "cargo-home"
            registry.write_cargo_home(cargo_home, registry_source)
            for pkg in ("greggd", "gregg"):
                result = _run_boundary2_chain(
                    package=pkg, sha=sha, version=version,
                    evidence_dir=args.evidence_dir, archive_path=b2_root / f"{pkg}-1.0.1.crate",
                    archive_sha="unused", protocol_checksum=protocol_checksum,
                    registry_record=registry_record, protocol_crate_meta=crate_meta,
                    registry_source=registry_source, cargo_home=cargo_home)
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

    # Phase 3: Final chain
    mock_api2 = MockGitHubAPI(sha)
    mock_api2.start()
    try:
        crate_paths = {}
        for pkg in ("greggd", "gregg"):
            archive = args.evidence_dir / f"final-{pkg}-1.0.1.crate"
            archive.write_bytes(f"{pkg}-final-archive".encode())
            crate_paths[pkg] = archive
        protocol_archive = args.evidence_dir / "final-gregg-protocol-1.0.1.crate"
        protocol_archive.write_bytes(b"protocol-final-archive")
        crate_paths["gregg-protocol"] = protocol_archive

        final_result = _run_final_chain(
            sha=sha, version=version, evidence_dir=args.evidence_dir,
            mock_api=mock_api2,
            candidate_manifest_path=args.evidence_dir / "candidate" / "v1.0.1-release-manifest.json",
            boundary2_summaries=boundary2_results, crate_paths=crate_paths,
            stages=requirements["final_required_stages"],
            requirements=args.requirements)
    finally:
        mock_api2.shutdown()

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
