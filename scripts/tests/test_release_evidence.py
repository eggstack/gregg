#!/usr/bin/env python3
"""Deterministic tests for Phase 16 release tooling."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VALIDATOR = ROOT / "scripts" / "validate-release-evidence.py"
SHA = "0123456789abcdef0123456789abcdef01234567"


def run(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, cwd=ROOT, text=True, capture_output=True, check=check)


def candidate(stage: str, sha: str = SHA, version: str = "1.0.1") -> dict[str, object]:
    return {
        "schema_version": 1,
        "candidate_sha": sha,
        "release_version": version,
        "stage": stage,
        "workflow_run_id": "100",
        "workflow_run_attempt": "1",
        "job_name": stage,
        "runner_os": "Linux",
        "runner_architecture": "x86_64",
        "started_at": "2026-07-24T00:00:00Z",
        "completed_at": "2026-07-24T00:01:00Z",
        "result": "success",
        "source": {"ref_input": sha, "tag_object_sha": None, "peeled_commit_sha": sha},
        "artifacts": [{"name": f"{stage}.log", "artifact_id": f"artifact-{stage}"}],
        "executables": [],
        "notes": [],
    }


class EvidenceTests(unittest.TestCase):
    def write(self, directory: Path, name: str, value: object) -> Path:
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_rejects_short_unknown_mixed_and_wrong_version(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            for value, needle in (
                ({**candidate("stage"), "candidate_sha": "unknown"}, "candidate_sha"),
                ({**candidate("stage"), "candidate_sha": SHA[:8]}, "candidate_sha"),
                ({**candidate("stage"), "release_version": "1.0.0"}, "release version"),
                ({**candidate("stage"), "stage": ""}, "stage"),
            ):
                path = self.write(directory, f"{len(needle)}.json", value)
                result = run("python3", str(VALIDATOR), "validate-candidate", str(path), "--expected-version", "1.0.1", check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(needle, result.stderr)

    def test_aggregate_rejects_missing_and_duplicate_stages(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            self.write(directory, "one/candidate.json", candidate("one"))
            missing = run(
                "python3", str(VALIDATOR), "aggregate", "--evidence-dir", str(directory),
                "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(directory / "out.json"),
                "--required-stage", "one", "--required-stage", "two", check=False,
            )
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn("missing required stages", missing.stderr)

            self.write(directory, "two/a/candidate.json", candidate("two"))
            duplicate = candidate("two")
            duplicate["workflow_run_id"] = "101"
            self.write(directory, "two/b/candidate.json", duplicate)
            result = run(
                "python3", str(VALIDATOR), "aggregate", "--evidence-dir", str(directory),
                "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(directory / "out.json"),
                "--required-stage", "one", "--required-stage", "two", check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("multiple successful", result.stderr)

    def test_complete_manifest_accepts_explicit_selection(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            self.write(directory, "one/candidate.json", candidate("one"))
            self.write(directory, "two/candidate.json", candidate("two"))
            selection = {stage: {"workflow_run_id": "100", "workflow_run_attempt": "1", "artifact_ids": [f"id-{stage}"]} for stage in ("one", "two")}
            selection_path = self.write(directory, "selection.json", selection)
            output = directory / "manifest.json"
            result = run(
                "python3", str(VALIDATOR), "aggregate", "--evidence-dir", str(directory),
                "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(output),
                "--required-stage", "one", "--required-stage", "two", "--selection", str(selection_path),
                "--tag-name", "v1.0.1", "--tag-object-sha", "fedcba9876543210fedcba9876543210fedcba98",
                "--peeled-commit-sha", SHA,
            )
            self.assertEqual(result.returncode, 0)
            self.assertEqual(run("python3", str(VALIDATOR), "validate-manifest", str(output)).returncode, 0)

    def test_release_soak_rejects_short_duration_and_missing_identity(self) -> None:
        result = run(
            "bash", str(ROOT / "scripts" / "soak-test.sh"), "--daemon", "/bin/true",
            "--candidate-sha", SHA, "--release-version", "1.0.1", "--stage", "soak-linux-24h",
            "--mode", "release", "--duration-minutes", "1", check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("1440", result.stderr)
        missing = run("bash", str(ROOT / "scripts" / "soak-test.sh"), "--daemon", "/bin/true", check=False)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("candidate SHA", missing.stderr)

    def test_candidate_identity_rejects_branch_and_accepts_full_head(self) -> None:
        head = run("git", "rev-parse", "HEAD").stdout.strip()
        accepted = run("bash", str(ROOT / "scripts" / "verify-candidate-identity.sh"), "--mode", "pre-tag", "--input", head, "--candidate-sha", head)
        self.assertEqual(accepted.returncode, 0)
        rejected = run("bash", str(ROOT / "scripts" / "verify-candidate-identity.sh"), "--mode", "pre-tag", "--input", "HEAD", "--candidate-sha", head, check=False)
        self.assertNotEqual(rejected.returncode, 0)

    def test_binary_provenance_mismatch_fails(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            archive = directory / "greggd-1.0.1.crate"
            archive.write_bytes(b"archive")
            binary = directory / "greggd"
            binary.write_text("#!/bin/sh\necho greggd 1.0.1\n", encoding="utf-8")
            binary.chmod(0o755)
            manifest = directory / "provenance.json"
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(manifest), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "greggd", str(archive), str(binary))
            candidate_mismatch = run("bash", str(ROOT / "scripts" / "verify-package-provenance.sh"), "--manifest", str(manifest), "--package", "greggd", "--archive", str(archive), "--binary", str(binary), "--version", "1.0.1", "--candidate-sha", "fedcba9876543210fedcba9876543210fedcba98", check=False)
            self.assertNotEqual(candidate_mismatch.returncode, 0)
            self.assertIn("candidate SHA", candidate_mismatch.stderr)
            archive.write_bytes(b"changed")
            result = run("bash", str(ROOT / "scripts" / "verify-package-provenance.sh"), "--manifest", str(manifest), "--package", "greggd", "--archive", str(archive), "--binary", str(binary), "--version", "1.0.1", "--candidate-sha", SHA, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("checksum mismatch", result.stderr)


if __name__ == "__main__":
    unittest.main()
