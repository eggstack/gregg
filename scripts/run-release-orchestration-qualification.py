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
        if self._thread:
            self._thread.join(timeout=5)


# ---------------------------------------------------------------------------
# Chain: Candidate/pre-tag
# ---------------------------------------------------------------------------

def _run_candidate_chain(*, sha: str, version: str, evidence_dir: Path, mock_api: MockGitHubAPI) -> dict:
    """D2: Build a realistic synthetic pre-tag selection and run retrieval + aggregation.

    Uses separate per-binary artifacts to match production topology: one shared
    artifact for common stages, and separate package-specific artifacts for
    ``binary-prepublish-greggd`` and ``binary-prepublish-gregg``.
    """
    candidate_dir = evidence_dir / "candidate"
    candidate_dir.mkdir(parents=True, exist_ok=True)

    shared_stages = ["source-ci", "mixed-fleet-functional", "mixed-fleet-sustained"]
    run_id = 2001

    shared_artifact_id = 5001
    shared_zip = candidate_dir / "shared-artifact.zip"
    with zipfile.ZipFile(shared_zip, "w") as zf:
        for stage in shared_stages:
            zf.writestr(f"{stage}/candidate.json", json.dumps(_candidate(stage, sha, run_id=str(run_id))))
    mock_api.add_run(run_id, stages=shared_stages, artifact_id=shared_artifact_id,
                     artifact_name="shared-artifact", zip_path=shared_zip)

    greggd_artifact_id = 5002
    greggd_zip = candidate_dir / "greggd-artifact.zip"
    with zipfile.ZipFile(greggd_zip, "w") as zf:
        zf.writestr("binary-prepublish-greggd/candidate.json",
                     json.dumps(_candidate("binary-prepublish-greggd", sha, run_id=str(run_id))))
    mock_api.add_run(run_id, stages=["binary-prepublish-greggd"], artifact_id=greggd_artifact_id,
                     artifact_name="binary-prepublish-greggd", zip_path=greggd_zip)

    gregg_artifact_id = 5003
    gregg_zip = candidate_dir / "gregg-artifact.zip"
    with zipfile.ZipFile(gregg_zip, "w") as zf:
        zf.writestr("binary-prepublish-gregg/candidate.json",
                     json.dumps(_candidate("binary-prepublish-gregg", sha, run_id=str(run_id))))
    mock_api.add_run(run_id, stages=["binary-prepublish-gregg"], artifact_id=gregg_artifact_id,
                     artifact_name="binary-prepublish-gregg", zip_path=gregg_zip)

    all_stages = shared_stages + ["binary-prepublish-greggd", "binary-prepublish-gregg"]
    selection = {
        "candidate_sha": sha, "release_version": version, "mode": "pre-tag",
        "runs": {stage: {"run_id": run_id, "attempt": 1, "workflow_name": "release-candidate",
                         "artifacts": [{"name": "shared-artifact"}]}
                 for stage in shared_stages},
    }
    selection["runs"]["binary-prepublish-greggd"] = {
        "run_id": run_id, "attempt": 1, "workflow_name": "release-candidate",
        "artifacts": [{"name": "binary-prepublish-greggd"}],
    }
    selection["runs"]["binary-prepublish-gregg"] = {
        "run_id": run_id, "attempt": 1, "workflow_name": "release-candidate",
        "artifacts": [{"name": "binary-prepublish-gregg"}],
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

    # Write candidate.json files into evidence dir for aggregation
    for stage in all_stages:
        stage_dir = candidate_dir / stage
        stage_dir.mkdir(parents=True, exist_ok=True)
        (stage_dir / "candidate.json").write_text(json.dumps(_candidate(stage, sha, run_id=str(run_id))), encoding="utf-8")

    # Aggregate in pre-tag mode
    manifest_path = candidate_dir / "v1.0.1-release-manifest.json"
    req_args = []
    for stage in all_stages:
        req_args.extend(["--required-stage", stage])
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "aggregate",
          "--evidence-dir", str(candidate_dir),
          "--expected-sha", sha, "--release-version", version,
          "--output", str(manifest_path),
          *req_args,
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

    return {"candidate_dir": str(candidate_dir), "manifest_sha256": _sha256(manifest_bytes), "manifest_size": len(manifest_bytes)}


# ---------------------------------------------------------------------------
# Chain: Boundary-2 (synthetic local registry)
# ---------------------------------------------------------------------------

class MockSparseRegistry:
    """Minimal sparse registry server that serves one crate."""

    def __init__(self, crate_path: Path, crate_sha: str, crate_version: str) -> None:
        self.crate_path = crate_path
        self.crate_sha = crate_sha
        self.crate_version = crate_version
        self._server: http.server.HTTPServer | None = None
        self._thread: threading.Thread | None = None

    def start(self) -> int:
        api = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                path = self.path.lstrip("/")
                # Sparse index: /{1}/{2}/{crate_name} (no /index suffix)
                # Sparse download: /{crate_name}/{version}/download
                # Config: /config.json
                if path in ("gr/eg/gregg-protocol", "gr/eg/gregg_protocol"):
                    entry = json.dumps({
                        "name": "gregg-protocol",
                        "vers": api.crate_version,
                        "deps": [],
                        "cksum": api.crate_sha,
                        "features": {},
                        "yanked": False,
                        "links": None,
                        "rust_version": None,
                    })
                    body = (entry + "\n").encode()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                elif "/download" in path and "gregg-protocol" in path:
                    data = api.crate_path.read_bytes()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                elif path == "config.json":
                    body = json.dumps({"dl": f"http://127.0.0.1:{api._port}/", "api": f"http://127.0.0.1:{api._port}/"}).encode()
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                else:
                    self.send_response(404)
                    self.end_headers()

            def log_message(self, *a: object) -> None:
                pass

        self._server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        self._port = self._server.server_address[1]
        port = self._server.server_address[1]
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return port

    def shutdown(self) -> None:
        if self._server:
            self._server.shutdown()
        if self._thread:
            self._thread.join(timeout=5)


def _build_local_registry_fixture(tmpdir: Path, *, sha: str, version: str) -> tuple[Path, dict[str, str]]:
    """D3: Build a minimal local Cargo registry with gregg-protocol 1.0.1.

    Creates a directory-style source replacement containing an unpacked
    gregg-protocol crate that Cargo can resolve from, and packages the
    protocol crate for Boundary-2 archive verification.
    """
    registry_root = tmpdir / "registry"
    registry_root.mkdir(parents=True, exist_ok=True)

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

    # Build a directory-style source: unpacked crate directories
    # format: registry_root/<crate_name>-<version>/ containing Cargo.toml + src/
    crate_dir = registry_root / f"gregg-protocol-{version}"
    crate_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(protocol_dir / "Cargo.toml", crate_dir / "Cargo.toml")
    src_dir = crate_dir / "src"
    src_dir.mkdir(exist_ok=True)
    shutil.copy2(protocol_dir / "src" / "lib.rs", src_dir / "lib.rs")
    # directory sources require .cargo-checksum.json with real SHA-256 hashes
    checksums = {}
    for f in sorted(crate_dir.rglob("*")):
        if f.is_file() and f.name != ".cargo-checksum.json":
            rel = str(f.relative_to(crate_dir))
            checksums[rel] = _sha256(f.read_bytes())
    (crate_dir / ".cargo-checksum.json").write_text(json.dumps({"files": checksums}), encoding="utf-8")

    return crate_path, {"sha256": crate_sha, "size_bytes": crate_size}


def _run_boundary2_chain(*, package: str, sha: str, version: str, evidence_dir: Path,
                         archive_path: Path, archive_sha: str, protocol_checksum: str,
                         registry_record: dict, protocol_crate_meta: dict) -> dict:
    """D3: Run a single Boundary-2 verification chain against a real crate archive.

    Creates a tar.gz archive containing a Cargo.toml that depends on
    gregg-protocol 1.0.1, and pre-generates a Cargo.lock using source
    replacement against a local registry fixture so that the dependency
    resolves. The registry-reverify.py script then validates the archive,
    generates a fresh lockfile (using the same source replacement), and
    runs build/test/install/help/version.
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
        f'[dependencies]\ngregg-protocol = {{ version = "1.0.1" }}\n'
    )

    # Create the archive with a pre-generated Cargo.lock that contains the
    # correct checksum for gregg-protocol.  Cargo lockfile v4 omits checksums
    # for source-replaced packages, so we construct the lockfile programmatically
    # using the protocol crate's .crate SHA-256 as the checksum value.
    protocol_crate_sha = protocol_crate_meta["sha256"]
    fixture_lockfile = pkg_dir / "Cargo.lock"
    fixture_lockfile.write_text(
        f'# This file is automatically @generated by Cargo.\n'
        f'# It is not intended for manual editing.\n'
        f'version = 3\n\n'
        f'[[package]]\n'
        f'name = "gregg-protocol"\n'
        f'version = "{version}"\n'
        f'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
        f'checksum = "{protocol_crate_sha}"\n\n'
        f'[[package]]\n'
        f'name = "{package}"\n'
        f'version = "{version}"\n'
        f'dependencies = [\n'
        f' "gregg-protocol",\n'
        f']\n',
        encoding="utf-8",
    )

    with tempfile.TemporaryDirectory() as tar_tmp:
        crate_root = Path(tar_tmp) / package_dir_name
        crate_root.mkdir()
        (crate_root / "Cargo.toml").write_text(cargo_toml, encoding="utf-8")
        (crate_root / "src").mkdir()
        (crate_root / "src" / "lib.rs").write_text(f"pub fn version() -> &'static str {{ \"{version}\" }}\n", encoding="utf-8")
        shutil.copy2(fixture_lockfile, crate_root / "Cargo.lock")
        _run(["tar", "czf", str(archive_tmp), "-C", str(tar_tmp), package_dir_name])

    archive_sha_actual = _sha256(archive_tmp.read_bytes())
    archive_size = archive_tmp.stat().st_size

    # Set up CARGO_HOME so cargo can find config if needed
    verify_env = {**os.environ}

    _run([sys.executable, str(SCRIPTS / "registry-reverify.py"),
          "--archive", str(archive_tmp),
          "--package", package,
          "--expected-sha256", archive_sha_actual,
          "--protocol-checksum", protocol_checksum,
          "--registry-record", str(registry_record_path),
          "--registry-source", "registry+https://github.com/rust-lang/crates.io-index",
          "--fixture-lockfile", str(fixture_lockfile),
          "--evidence-dir", str(command_evidence_dir),
          "--output", str(summary_path)],
         env=verify_env)

    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    assert summary["verification"] == "pass", f"Boundary-2 verification failed for {package}"

    return {"summary_path": str(summary_path), "package": package}


# ---------------------------------------------------------------------------
# Chain: Final cross-run
# ---------------------------------------------------------------------------

def _run_final_chain(*, sha: str, version: str, evidence_dir: Path, mock_api: MockGitHubAPI,
                     candidate_manifest_path: Path, boundary2_summaries: list[dict],
                     crate_paths: dict[str, Path]) -> dict:
    """D4: Run the final synthetic cross-run chain."""
    final_dir = evidence_dir / "final"
    final_dir.mkdir(parents=True, exist_ok=True)

    stages = ["source-ci", "binary-prepublish-greggd", "binary-prepublish-gregg",
              "mixed-fleet-functional", "mixed-fleet-sustained",
              "registry-reverify-greggd", "registry-reverify-gregg",
              "protocol-index-check", "postpublish-verify"]

    run_id = 3001
    artifact_id = 6001
    zip_path = final_dir / "final-artifact.zip"
    with zipfile.ZipFile(zip_path, "w") as zf:
        for stage in stages:
            zf.writestr(f"{stage}/candidate.json", json.dumps(_candidate(stage, sha, run_id=str(run_id))))

    mock_api.add_run(run_id, stages=stages, artifact_id=artifact_id, artifact_name="final-artifact", zip_path=zip_path)

    # Final selection
    selection = {
        "candidate_sha": sha, "release_version": version, "mode": "final",
        "runs": {stage: {"run_id": run_id, "attempt": 1, "workflow_name": "release-candidate",
                         "artifacts": [{"name": "final-artifact"}]}
                 for stage in stages},
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
        (stage_dir / "candidate.json").write_text(json.dumps(_candidate(stage, sha, run_id=str(run_id))), encoding="utf-8")

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
    _write_json(final_dir / "1.0.0-disposition.json", disposition)

    # Registry summary
    registry_summary = [
        {"crate": "gregg-protocol", "version": version, "yanked": False, "checksum": "a" * 64, "published_at": "2026-07-24T00:00:00Z"},
        {"crate": "greggd", "version": version, "yanked": False, "checksum": "b" * 64, "published_at": "2026-07-24T00:00:00Z"},
        {"crate": "gregg", "version": version, "yanked": False, "checksum": "c" * 64, "published_at": "2026-07-24T00:00:00Z"},
    ]
    _write_json(final_dir / "registry-summary.json", registry_summary)

    # Aggregate in final mode
    manifest_path = final_dir / "v1.0.1-release-manifest.json"
    req_args = []
    for stage in stages:
        req_args.extend(["--required-stage", stage])
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "aggregate",
          "--evidence-dir", str(final_dir),
          "--expected-sha", sha, "--release-version", version,
          "--output", str(manifest_path),
          *req_args,
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
          "--registry-summary", str(final_dir / "registry-summary.json"),
          "--disposition", str(final_dir / "1.0.0-disposition.json"),
          "--final"])

    # Validate final manifest
    _run([sys.executable, str(SCRIPTS / "validate-release-evidence.py"), "validate-manifest",
          str(manifest_path), "--expected-sha", sha, "--expected-version", version, "--mode", "final"])

    manifest_bytes = manifest_path.read_bytes()
    (final_dir / "manifest.sha256").write_text(_sha256(manifest_bytes), encoding="utf-8")

    # D4: Materialize role-indexed evidence for the final chain
    artifact_list = []
    for fpath in sorted(final_dir.rglob("*")):
        if fpath.is_file() and fpath.name != "role-index.json":
            rel = str(fpath.relative_to(final_dir))
            # Use slash-separated relative path as role to ensure uniqueness
            role = rel.replace("/", "-").replace("\\", "-")
            artifact_list.append({"name": rel, "role": role})
    if artifact_list:
        artifact_list_path = final_dir / "artifact-list.json"
        _write_json(artifact_list_path, artifact_list)
        role_index_path = final_dir / "role-index.json"
        _run([sys.executable, str(SCRIPTS / "materialize-release-evidence.py"),
              "--artifact-list", str(artifact_list_path),
              "--root", str(final_dir),
              "--output", str(role_index_path)])

    return {"final_dir": str(final_dir), "manifest_sha256": _sha256(manifest_bytes), "manifest_size": len(manifest_bytes)}


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

    return results


# ---------------------------------------------------------------------------
# Main driver
# ---------------------------------------------------------------------------

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--release-version", required=True)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    args = parser.parse_args()

    if args.release_version != "1.0.1" or len(args.candidate_sha) != 40 or args.candidate_sha != args.candidate_sha.lower():
        parser.error("qualification requires a lowercase 40-character 1.0.1 candidate SHA")
    args.evidence_dir.mkdir(parents=True, exist_ok=True)

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
        candidate_result = _run_candidate_chain(sha=sha, version=version, evidence_dir=args.evidence_dir, mock_api=mock_api)
    finally:
        mock_api.shutdown()

    # Phase 2: Boundary-2 chains
    boundary2_results = []
    with tempfile.TemporaryDirectory(prefix="gregg-qual-b2-") as b2_tmp:
        b2_root = Path(b2_tmp)

        # Build local registry fixture with gregg-protocol 1.0.1
        _crate_path, _crate_meta = _build_local_registry_fixture(b2_root, sha=sha, version=version)

        protocol_checksum = _crate_meta["sha256"]
        registry_record = {"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": protocol_checksum, "created_at": "2026-01-01T00:00:00Z"}}

        for pkg in ("greggd", "gregg"):
            result = _run_boundary2_chain(
                package=pkg, sha=sha, version=version,
                evidence_dir=args.evidence_dir, archive_path=b2_root / f"{pkg}-1.0.1.crate",
                archive_sha="unused", protocol_checksum=protocol_checksum,
                registry_record=registry_record, protocol_crate_meta=_crate_meta)
            boundary2_results.append(result)

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
            boundary2_summaries=boundary2_results, crate_paths=crate_paths)
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
        if path.is_file() and path.name not in ("qualification-summary.json", "qualification-commands.json"):
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
        "chains": {
            "candidate_pre_tag": candidate_result,
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
