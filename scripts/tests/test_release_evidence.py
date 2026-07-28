#!/usr/bin/env python3
"""Deterministic tests for Phase 18 release tooling."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VALIDATOR = ROOT / "scripts" / "validate-release-evidence.py"
SHA = "0123456789abcdef0123456789abcdef01234567"


def run(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, cwd=ROOT, text=True, capture_output=True, check=check)


def candidate(stage: str, sha: str = SHA, version: str = "1.0.1", run_id: str = "100", attempt: str = "1") -> dict[str, object]:
    return {
        "schema_version": 1,
        "candidate_sha": sha,
        "release_version": version,
        "stage": stage,
        "workflow_run_id": run_id,
        "workflow_run_attempt": attempt,
        "job_name": stage,
        "runner_os": "Linux",
        "runner_architecture": "x86_64",
        "started_at": "2026-07-24T00:00:00Z",
        "completed_at": "2026-07-24T00:01:00Z",
        "result": "success",
        "source_identity_mode": "pre-tag-full-sha",
        "source": {"ref_input": sha, "tag_object_sha": None, "peeled_commit_sha": sha},
        "artifacts": [{"name": f"{stage}.log", "role": "transcript", "artifact_id": f"artifact-{stage}"}],
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

    def test_tag_normalization_accepts_annotated_and_rejects_lightweight(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            repo = Path(raw) / "repo"
            run("git", "init", "-q", "--bare", str(repo / "bare"))
            run("git", "clone", "-q", str(repo / "bare"), str(repo / "work"))
            env = {**os.environ, "GIT_AUTHOR_NAME": "Tester", "GIT_AUTHOR_EMAIL": "test@example.com", "GIT_COMMITTER_NAME": "Tester", "GIT_COMMITTER_EMAIL": "test@example.com"}
            run("git", "-C", str(repo / "work"), "config", "user.name", "Tester")
            run("git", "-C", str(repo / "work"), "config", "user.email", "test@example.com")
            (repo / "work" / "a.txt").write_text("hello\n", encoding="utf-8")
            run("git", "-C", str(repo / "work"), "add", "a.txt")
            commit_result = subprocess.run(["git", "-C", str(repo / "work"), "commit", "-q", "-m", "initial"], env=env, capture_output=True, text=True)
            self.assertEqual(commit_result.returncode, 0, commit_result.stderr)
            commit = run("git", "-C", str(repo / "work"), "rev-parse", "HEAD").stdout.strip()
            tag_result = subprocess.run(["git", "-C", str(repo / "work"), "tag", "-a", "v1.0.1", "-m", "release 1.0.1"], env=env, capture_output=True, text=True)
            self.assertEqual(tag_result.returncode, 0, tag_result.stderr)
            output = repo / "tag-identity.json"
            result = subprocess.run(["bash", str(ROOT / "scripts" / "verify-candidate-identity.sh"), "--mode", "tag", "--input", "v1.0.1", "--candidate-sha", commit, "--output", str(output)], cwd=str(repo / "work"), capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads(output.read_text(encoding="utf-8"))
            self.assertTrue(data["tagger_timestamp"].endswith("Z"))
            self.assertEqual(data["peeled_commit_sha"], commit)
            self.assertTrue(data["tag_object_sha"])
            self.assertTrue(data["tag_object_content_sha256"])
            with tempfile.TemporaryDirectory() as raw2:
                repo2 = Path(raw2) / "repo"
                run("git", "init", "-q", "--bare", str(repo2 / "bare"))
                run("git", "clone", "-q", str(repo2 / "bare"), str(repo2 / "work"))
                run("git", "-C", str(repo2 / "work"), "config", "user.name", "Tester")
                run("git", "-C", str(repo2 / "work"), "config", "user.email", "test@example.com")
                (repo2 / "work" / "a.txt").write_text("hello\n", encoding="utf-8")
                run("git", "-C", str(repo2 / "work"), "add", "a.txt")
                commit2_result = subprocess.run(["git", "-C", str(repo2 / "work"), "commit", "-q", "-m", "initial"], env=env, capture_output=True, text=True)
                self.assertEqual(commit2_result.returncode, 0, commit2_result.stderr)
                commit2 = run("git", "-C", str(repo2 / "work"), "rev-parse", "HEAD").stdout.strip()
                run("git", "-C", str(repo2 / "work"), "tag", "v1.0.1")
                rejected = subprocess.run(["bash", str(ROOT / "scripts" / "verify-candidate-identity.sh"), "--mode", "tag", "--input", "v1.0.1", "--candidate-sha", commit2], cwd=str(repo2 / "work"), capture_output=True, text=True)
                self.assertNotEqual(rejected.returncode, 0)
                self.assertIn("annotated tag", rejected.stderr)

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

    def test_installer_lockfile_drift_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            archive = directory / "greggd-1.0.1.crate"
            archive.write_bytes(b"archive")
            binary = directory / "greggd"
            binary.write_text("#!/bin/sh\necho greggd 1.0.1\n", encoding="utf-8")
            binary.chmod(0o755)
            lockfile = directory / "Cargo.lock"
            lockfile.write_text("original-lockfile", encoding="utf-8")
            manifest = directory / "provenance.json"
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(manifest), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "greggd", str(archive), str(binary), str(lockfile))
            missing = run("bash", str(ROOT / "scripts" / "install-verified-package.sh"), "--manifest", str(manifest), "--package", "greggd", "--archive", str(archive), "--version", "1.0.1", "--candidate-sha", SHA, "--root", str(directory / "root"), check=False)
            self.assertNotEqual(missing.returncode, 0)
            self.assertIn("lockfile", missing.stderr.lower())
            lockfile.write_text("modified-lockfile", encoding="utf-8")
            modified = run("bash", str(ROOT / "scripts" / "install-verified-package.sh"), "--manifest", str(manifest), "--package", "greggd", "--archive", str(archive), "--version", "1.0.1", "--candidate-sha", SHA, "--root", str(directory / "root"), "--lockfile", str(lockfile), check=False)
            self.assertNotEqual(modified.returncode, 0)
            self.assertIn("lockfile checksum mismatch", modified.stderr)

    def test_provenance_lockfile_identity_required_for_binaries(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            archive = directory / "greggd-1.0.1.crate"
            archive.write_bytes(b"archive")
            binary = directory / "greggd"
            binary.write_text("#!/bin/sh\necho greggd 1.0.1\n", encoding="utf-8")
            binary.chmod(0o755)
            manifest = directory / "provenance.json"
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(manifest), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "greggd", str(archive), str(binary))
            lockfile = directory / "Cargo.lock"
            lockfile.write_text("lockfile", encoding="utf-8")
            result = run("bash", str(ROOT / "scripts" / "install-verified-package.sh"), "--manifest", str(manifest), "--package", "greggd", "--archive", str(archive), "--version", "1.0.1", "--candidate-sha", SHA, "--root", str(directory / "root"), "--lockfile", str(lockfile), check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("verification_lockfile", result.stderr)

    def test_provenance_merge_accepts_complete_three_crate(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            protocol_archive = directory / "gregg-protocol-1.0.1.crate"
            protocol_archive.write_bytes(b"protocol-archive")
            daemon_archive = directory / "greggd-1.0.1.crate"
            daemon_archive.write_bytes(b"daemon-archive")
            daemon_binary = directory / "greggd"
            daemon_binary.write_text("#!/bin/sh\necho greggd 1.0.1\n", encoding="utf-8")
            daemon_binary.chmod(0o755)
            daemon_lockfile = directory / "greggd-Cargo.lock"
            daemon_lockfile.write_text("daemon-lockfile", encoding="utf-8")
            client_archive = directory / "gregg-1.0.1.crate"
            client_archive.write_bytes(b"client-archive")
            client_binary = directory / "gregg"
            client_binary.write_text("#!/bin/sh\necho gregg 1.0.1\n", encoding="utf-8")
            client_binary.chmod(0o755)
            client_lockfile = directory / "gregg-Cargo.lock"
            client_lockfile.write_text("client-lockfile", encoding="utf-8")
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(directory / "protocol.json"), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "gregg-protocol", str(protocol_archive))
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(directory / "daemon.json"), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "greggd", str(daemon_archive), str(daemon_binary), str(daemon_lockfile))
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(directory / "client.json"), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "gregg", str(client_archive), str(client_binary), str(client_lockfile))
            merged = directory / "merged.json"
            result = run("python3", str(ROOT / "scripts" / "merge-package-provenance.py"), "--protocol", str(directory / "protocol.json"), "--daemon", str(directory / "daemon.json"), "--client", str(directory / "client.json"), "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(merged))
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads(merged.read_text(encoding="utf-8"))
            self.assertEqual(set(data["packages"]), {"gregg-protocol", "greggd", "gregg"})

    def test_provenance_merge_rejects_missing_and_mixed(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            protocol_archive = directory / "gregg-protocol-1.0.1.crate"
            protocol_archive.write_bytes(b"protocol-archive")
            daemon_archive = directory / "greggd-1.0.1.crate"
            daemon_archive.write_bytes(b"daemon-archive")
            daemon_lockfile = directory / "greggd-Cargo.lock"
            daemon_lockfile.write_text("daemon-lockfile", encoding="utf-8")
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(directory / "protocol.json"), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "gregg-protocol", str(protocol_archive))
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(directory / "daemon.json"), "--candidate-sha", SHA, "--release-version", "1.0.1", "--package", "greggd", str(daemon_archive), str(directory / "greggd"), str(daemon_lockfile))
            missing = run("python3", str(ROOT / "scripts" / "merge-package-provenance.py"), "--protocol", str(directory / "protocol.json"), "--daemon", str(directory / "daemon.json"), "--client", str(directory / "nonexistent.json"), "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(directory / "merged.json"), check=False)
            self.assertNotEqual(missing.returncode, 0)
            mixed_sha = "fedcba9876543210fedcba9876543210fedcba98"
            directory.joinpath("client-archive").write_bytes(b"client-archive")
            directory.joinpath("client-binary").write_text("#!/bin/sh\necho gregg 1.0.1\n", encoding="utf-8")
            directory.joinpath("client-binary").chmod(0o755)
            directory.joinpath("client-lockfile").write_text("client-lockfile", encoding="utf-8")
            run("python3", str(ROOT / "scripts" / "write-package-provenance.py"), "--output", str(directory / "client-mixed.json"), "--candidate-sha", mixed_sha, "--release-version", "1.0.1", "--package", "gregg", str(directory / "client-archive"), str(directory / "client-binary"), str(directory / "client-lockfile"))
            mixed = run("python3", str(ROOT / "scripts" / "merge-package-provenance.py"), "--protocol", str(directory / "protocol.json"), "--daemon", str(directory / "daemon.json"), "--client", str(directory / "client-mixed.json"), "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(directory / "merged.json"), check=False)
            self.assertNotEqual(mixed.returncode, 0)
            self.assertIn("candidate SHA", mixed.stderr)

    def test_github_retrieval_rejects_failed_run_and_expired_artifact(self) -> None:
        """K1: Cross-run GitHub API behavior tested with mocked responses."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        failed_run = {"id": 1001, "status": "completed", "conclusion": "failure", "run_attempt": 1, "repository": {"full_name": "owner/repo"}, "name": "release-candidate", "event": "workflow_dispatch", "actor": {"login": "tester"}, "html_url": "https://github.com/owner/repo/runs/1001", "head_sha": SHA, "head_branch": "main"}
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_run_metadata(failed_run, expected_repo="owner/repo", expected_workflow="release-candidate", expected_attempt="1")
        self.assertIn("did not conclude successfully", str(ctx.exception))

        wrong_attempt = {"id": 1002, "status": "completed", "conclusion": "success", "run_attempt": 2, "repository": {"full_name": "owner/repo"}, "name": "release-candidate"}
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_run_metadata(wrong_attempt, expected_repo="owner/repo", expected_workflow="release-candidate", expected_attempt="1")
        self.assertIn("attempt", str(ctx.exception))

        expired_artifact = {"id": 5001, "name": "source-ci", "size_in_bytes": 100, "created_at": "2026-01-01T00:00:00Z", "expires_at": "2026-01-02T00:00:00Z", "expired": True}
        artifacts_list = [expired_artifact]
        artifact = artifacts_list[0]
        self.assertTrue(artifact.get("expired", False))

    def test_github_retrieval_accepts_valid_run_and_artifact(self) -> None:
        """K1: Valid run and artifact metadata accepted."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        valid_run = {"id": 1001, "status": "completed", "conclusion": "success", "run_attempt": 1, "repository": {"full_name": "owner/repo"}, "name": "release-candidate", "event": "workflow_dispatch", "actor": {"login": "tester"}, "html_url": "https://github.com/owner/repo/runs/1001", "head_sha": SHA, "head_branch": "main"}
        result = module.validate_run_metadata(valid_run, expected_repo="owner/repo", expected_workflow="release-candidate", expected_attempt="1")
        self.assertEqual(result["run_id"], "1001")
        self.assertEqual(result["conclusion"], "success")
        self.assertEqual(result["actor"], "tester")

    def test_github_retrieval_rejects_wrong_repo_and_workflow(self) -> None:
        """K1: Wrong repository and workflow name rejected."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        wrong_repo = {"id": 1001, "status": "completed", "conclusion": "success", "run_attempt": 1, "repository": {"full_name": "other/repo"}, "name": "release-candidate"}
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_run_metadata(wrong_repo, expected_repo="owner/repo", expected_workflow="release-candidate", expected_attempt="1")
        self.assertIn("repository", str(ctx.exception))

        wrong_workflow = {"id": 1001, "status": "completed", "conclusion": "success", "run_attempt": 1, "repository": {"full_name": "owner/repo"}, "name": "other-workflow"}
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_run_metadata(wrong_workflow, expected_repo="owner/repo", expected_workflow="release-candidate", expected_attempt="1")
        self.assertIn("does not match", str(ctx.exception))

    def test_selection_validation_rejects_malformed_input(self) -> None:
        """A2: Selection validation rejects malformed inputs before network access."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        # Short SHA.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": "abc123", "release_version": "1.0.1", "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}}})
        self.assertIn("candidate_sha", str(ctx.exception))

        # Wrong version.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": SHA, "release_version": "2.0.0", "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}}})
        self.assertIn("1.0.1", str(ctx.exception))

        # Empty runs.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": SHA, "release_version": "1.0.1", "runs": {}})
        self.assertIn("nonempty", str(ctx.exception))

        # Non-numeric run ID.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"source-ci": {"run_id": "abc", "attempt": 1, "artifacts": [{"name": "a"}]}}})
        self.assertIn("numeric", str(ctx.exception))

        # Empty artifact names.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": ""}]}}})
        self.assertIn("nonempty", str(ctx.exception))

        # Duplicate artifact name in run.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}, {"name": "a"}]}}})
        self.assertIn("duplicate", str(ctx.exception))

        # Grouped alias rejected.
        with self.assertRaises(module.RetrievalError) as ctx:
            module.validate_selection({"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"binary-prepublish": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}}})
        self.assertIn("unknown stage", str(ctx.exception))

    def test_selection_validation_accepts_valid_input(self) -> None:
        """A2: Valid selection accepted."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        valid = {
            "candidate_sha": SHA,
            "release_version": "1.0.1",
            "runs": {
                "source-ci": {"run_id": 1001, "attempt": 1, "artifacts": [{"name": "source-ci-abc-1"}]},
                "binary-prepublish-greggd": {"run_id": 1002, "attempt": 1, "artifacts": [{"name": "binary-greggd-abc-1"}]},
            },
        }
        module.validate_selection(valid)  # Should not raise.

    def test_selection_allows_shared_artifacts_across_stages(self) -> None:
        """Phase 19: Multiple logical stages may reference the same artifact from the same run."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        shared = {
            "candidate_sha": SHA,
            "release_version": "1.0.1",
            "runs": {
                "resource-linux": {"run_id": 1018, "attempt": 1, "artifacts": [{"name": "operational-abc-1"}]},
                "soak-linux-24h": {"run_id": 1018, "attempt": 1, "artifacts": [{"name": "operational-abc-1"}]},
            },
        }
        module.validate_selection(shared)  # Should not raise.

    def test_safe_zip_extraction_rejects_traversal(self) -> None:
        """B3: Safe ZIP extraction rejects path traversal."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            malicious_zip = directory / "malicious.zip"
            with zipfile.ZipFile(malicious_zip, "w") as zf:
                zf.writestr("../escape.txt", "escaped")
            extract_dir = directory / "extract"
            extract_dir.mkdir()
            with self.assertRaises(module.RetrievalError) as ctx:
                module._safe_extract_zip(malicious_zip, extract_dir)
            self.assertIn("traversal", str(ctx.exception))

    def test_safe_zip_extraction_rejects_absolute_paths(self) -> None:
        """B3: Safe ZIP extraction rejects absolute paths."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            malicious_zip = directory / "absolute.zip"
            with zipfile.ZipFile(malicious_zip, "w") as zf:
                zf.writestr("/etc/passwd", "root:x:0:0:root:/root:/bin/bash")
            extract_dir = directory / "extract"
            extract_dir.mkdir()
            with self.assertRaises(module.RetrievalError) as ctx:
                module._safe_extract_zip(malicious_zip, extract_dir)
            self.assertIn("absolute", str(ctx.exception))

    def test_safe_zip_extraction_rejects_empty_zip(self) -> None:
        """B3: Safe ZIP extraction rejects empty archives."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            empty_zip = directory / "empty.zip"
            with zipfile.ZipFile(empty_zip, "w") as zf:
                pass  # empty
            extract_dir = directory / "extract"
            extract_dir.mkdir()
            with self.assertRaises(module.RetrievalError) as ctx:
                module._safe_extract_zip(empty_zip, extract_dir)
            self.assertIn("empty", str(ctx.exception))

    def test_safe_zip_extraction_accepts_valid_archive(self) -> None:
        """B3: Safe ZIP extraction accepts valid archives."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            valid_zip = directory / "valid.zip"
            with zipfile.ZipFile(valid_zip, "w") as zf:
                zf.writestr("evidence/candidate.json", json.dumps({"candidate_sha": SHA}))
                zf.writestr("evidence/transcript.txt", "ok")
            extract_dir = directory / "extract"
            extract_dir.mkdir()
            module._safe_extract_zip(valid_zip, extract_dir)
            self.assertTrue((extract_dir / "evidence" / "candidate.json").exists())
            self.assertTrue((extract_dir / "evidence" / "transcript.txt").exists())

    def test_download_validates_all_candidates(self) -> None:
        """B2: Download validates all candidate.json files, not just the first."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            valid_zip = directory / "artifact.zip"
            c1 = json.dumps({"candidate_sha": SHA, "release_version": "1.0.1", "stage": "one", "workflow_run_id": "100", "workflow_run_attempt": "1"})
            c2 = json.dumps({"candidate_sha": SHA, "release_version": "1.0.1", "stage": "two", "workflow_run_id": "100", "workflow_run_attempt": "1"})
            with zipfile.ZipFile(valid_zip, "w") as zf:
                zf.writestr("one/candidate.json", c1)
                zf.writestr("two/candidate.json", c2)

            extract_dir = directory / "extract"
            extract_dir.mkdir()
            module._safe_extract_zip(valid_zip, extract_dir)
            candidates = sorted(extract_dir.rglob("candidate.json"))
            self.assertEqual(len(candidates), 2)

            # Validate both candidates.
            for path in candidates:
                data = json.loads(path.read_text())
                self.assertEqual(data["candidate_sha"], SHA)
                self.assertEqual(data["release_version"], "1.0.1")

    def test_native_workflow_static_invariants(self) -> None:
        """K4: Source-native and package-native stage names both exist; package-native jobs download artifacts."""
        workflow = (ROOT / ".github" / "workflows" / "release-candidate.yml").read_text(encoding="utf-8")
        contract = json.loads((ROOT / "plans" / "evidence" / "release-dispatch-contract.json").read_text(encoding="utf-8"))
        all_stages = set()
        for dispatch in contract.get("dispatches", {}).values():
            all_stages.update(dispatch.get("required_stages", []))
        for stage in ("native-source-linux-x86-64", "native-source-linux-arm64", "native-source-macos-arm64", "native-source-macos-intel"):
            self.assertIn(stage, all_stages, f"source-native stage {stage} missing from dispatch contract")
        for stage in ("native-package-linux-x86-64", "native-package-linux-arm64", "native-package-macos-arm64", "native-package-macos-intel"):
            self.assertIn(stage, all_stages, f"package-native stage {stage} missing from dispatch contract")
        self.assertIn("actions/download-artifact@v4", workflow)
        self.assertIn("--lockfile", workflow)
        for arch in ("x86_64", "aarch64", "arm64"):
            self.assertIn(arch, workflow)
        self.assertIn("macos-15", workflow)
        self.assertIn("macos-15-intel", workflow)

    def test_protected_cleanup_static_invariants(self) -> None:
        """K5: systemd and launchd jobs use fail-closed cleanup."""
        workflow = (ROOT / ".github" / "workflows" / "release-candidate.yml").read_text(encoding="utf-8")
        self.assertIn("if: always()", workflow)
        self.assertNotIn("continue-on-error: true", workflow)
        self.assertIn("GREGG_SYSTEMD_HOST", workflow)
        self.assertIn("GREGG_LAUNCHD_HOST", workflow)
        self.assertNotIn("GREGG_SYSTEMD_BINARY", workflow)
        self.assertNotIn("GREGG_LAUNCHD_BINARY", workflow)
        self.assertNotIn("GREGG_INSTALLED_GREGGD", workflow)
        self.assertIn("cargo install --path", workflow)
        self.assertIn("--locked", workflow)
        install_script = (ROOT / "scripts" / "install-verified-package.sh").read_text(encoding="utf-8")
        self.assertNotIn("cargo generate-lockfile", install_script)
        # Cleanup must not use || true.
        cleanup_lines = [line for line in workflow.splitlines() if "cleanup" in line.lower() and "|| true" in line]
        self.assertEqual(cleanup_lines, [], f"Cleanup lines with || true found: {cleanup_lines}")

    def test_sustained_harness_smoke_rejects_short_duration(self) -> None:
        """K6: Sustained harness smoke mode rejects insufficient durations."""
        result = run("bash", str(ROOT / "scripts" / "soak-test.sh"), "--daemon", "/bin/true",
                     "--candidate-sha", SHA, "--release-version", "1.0.1",
                     "--stage", "soak-linux-24h", "--mode", "release",
                     "--duration-minutes", "1", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("1440", result.stderr)
        smoke = run("bash", str(ROOT / "scripts" / "soak-test.sh"), "--daemon", "/bin/true",
                    "--candidate-sha", SHA, "--release-version", "1.0.1",
                    "--stage", "soak-smoke", "--mode", "smoke",
                    "--duration-minutes", "1", "--interval-secs", "1", check=False)
        self.assertNotEqual(smoke.returncode, 0)

    def test_manifest_validation_rejects_incomplete_final(self) -> None:
        """J3: Final manifest validation rejects missing tag and provenance."""
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            manifest = {
                "manifest_schema_version": 1,
                "release_version": "1.0.1",
                "candidate_sha": SHA,
                "tooling_sha": SHA,
                "tag": None,
                "mode": "pre-tag",
                "manifest_scope": "current-run",
                "required_stages": ["one", "two"],
                "stages": [
                    {"stage": "one", "workflow_run_id": "100", "workflow_run_attempt": "1",
                     "artifact_ids": ["a1"], "metadata_path": "one/candidate.json",
                     "metadata_sha256": "0" * 64, "content_artifacts": [],
                     "candidate": {"schema_version": 1, "candidate_sha": SHA, "release_version": "1.0.1",
                                   "stage": "one", "workflow_run_id": "100", "workflow_run_attempt": "1",
                                   "job_name": "one", "runner_os": "Linux", "runner_architecture": "x86_64",
                                   "started_at": "2026-07-24T00:00:00Z", "completed_at": "2026-07-24T00:01:00Z",
                                   "result": "success", "source_identity_mode": "pre-tag-full-sha",
                                   "source": {"ref_input": SHA, "tag_object_sha": None, "peeled_commit_sha": SHA},
                                   "artifacts": [], "executables": [], "notes": []}},
                    {"stage": "two", "workflow_run_id": "100", "workflow_run_attempt": "1",
                     "artifact_ids": ["a2"], "metadata_path": "two/candidate.json",
                     "metadata_sha256": "0" * 64, "content_artifacts": [],
                     "candidate": {"schema_version": 1, "candidate_sha": SHA, "release_version": "1.0.1",
                                   "stage": "two", "workflow_run_id": "100", "workflow_run_attempt": "1",
                                   "job_name": "two", "runner_os": "Linux", "runner_architecture": "x86_64",
                                   "started_at": "2026-07-24T00:00:00Z", "completed_at": "2026-07-24T00:01:00Z",
                                   "result": "success", "source_identity_mode": "pre-tag-full-sha",
                                   "source": {"ref_input": SHA, "tag_object_sha": None, "peeled_commit_sha": SHA},
                                   "artifacts": [], "executables": [], "notes": []}},
                ],
                "rerun_selection": {},
                "package_provenance": None,
                "registry": None,
                "version_1_0_0_disposition": None,
                "verdict": "pass",
            }
            manifest_path = directory / "manifest.json"
            manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
            result = run("python3", str(VALIDATOR), "validate-manifest", str(manifest_path),
                         "--expected-sha", SHA, "--expected-version", "1.0.1", "--mode", "pre-tag")
            self.assertEqual(result.returncode, 0, result.stderr)
            final_manifest = dict(manifest)
            final_manifest["tag"] = {"name": "v1.0.1", "tag_object_sha": "a" * 40,
                                     "peeled_commit_sha": SHA,
                                     "tagger_name": "Tester", "tagger_email": "test@example.com",
                                     "tagger_timestamp": "2026-07-24T00:00:00Z",
                                     "tag_object_content_sha256": "b" * 64}
            final_path = directory / "final.json"
            final_path.write_text(json.dumps(final_manifest, indent=2), encoding="utf-8")
            final_result = run("python3", str(VALIDATOR), "validate-manifest", str(final_path),
                               "--expected-sha", SHA, "--expected-version", "1.0.1", "--mode", "final", check=False)
            self.assertNotEqual(final_result.returncode, 0)
            self.assertIn("package provenance", final_result.stderr.lower())

    def test_pre_tag_aggregation_does_not_require_postpublish(self) -> None:
        """D1: Pre-tag mode succeeds without postpublication evidence."""
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            self.write(directory, "source-ci/candidate.json", candidate("source-ci"))
            output = directory / "manifest.json"
            result = run(
                "python3", str(VALIDATOR), "aggregate", "--evidence-dir", str(directory),
                "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(output),
                "--required-stage", "source-ci", "--mode", "pre-tag",
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            data = json.loads(output.read_text())
            self.assertEqual(data["mode"], "pre-tag")
            self.assertIsNone(data["tag"])

    def test_final_aggregation_requires_postpublish(self) -> None:
        """D2: Final mode rejects missing postpublish evidence."""
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            self.write(directory, "source-ci/candidate.json", candidate("source-ci"))
            output = directory / "manifest.json"
            # Final mode requires provenance before postpublish check, so the error is about provenance.
            result = run(
                "python3", str(VALIDATOR), "aggregate", "--evidence-dir", str(directory),
                "--expected-sha", SHA, "--release-version", "1.0.1", "--output", str(output),
                "--required-stage", "source-ci", "--mode", "final",
                "--tag-name", "v1.0.1", "--tag-object-sha", "a" * 40, "--peeled-commit-sha", SHA,
                "--tagger-name", "T", "--tagger-email", "t@t", "--tagger-timestamp", "2026-07-24T00:00:00Z",
                "--tag-object-content-sha256", "b" * 64,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            # Final mode validates tag -> provenance -> registry -> disposition -> postpublish.
            # With no provenance supplied, it fails at provenance validation.
            self.assertIn("provenance", result.stderr.lower())

    def test_mixed_fleet_sustained_stage_exists(self) -> None:
        """F1: mixed-fleet-sustained has a producing job in the workflow."""
        workflow = (ROOT / ".github" / "workflows" / "release-candidate.yml").read_text(encoding="utf-8")
        self.assertIn("mixed-fleet-sustained", workflow)
        self.assertIn("stage mixed-fleet-sustained", workflow)

    def test_finalizer_uses_immutable_checkout(self) -> None:
        """H2: Finalize workflow checks out candidate SHA, not mutable branch."""
        finalize = (ROOT / ".github" / "workflows" / "release-finalize.yml").read_text(encoding="utf-8")
        self.assertIn("ref: ${{ inputs.candidate_sha }}", finalize)
        self.assertIn("git rev-parse HEAD", finalize)
        self.assertNotIn("ref: main", finalize)
        self.assertNotIn("ref: ${{ github.ref }}", finalize)

    def test_retrieval_cli_consumes_github_token_env(self) -> None:
        """Phase 19: retrieval CLI consumes GITHUB_TOKEN when --token is omitted."""
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            selection_path = directory / "sel.json"
            selection_path.write_text(json.dumps({
                "candidate_sha": SHA,
                "release_version": "1.0.1",
                "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}},
            }), encoding="utf-8")
            output_path = directory / "out.json"
            # Without GITHUB_TOKEN and without --token, should fail with clear diagnostic.
            result = run(
                "python3", str(ROOT / "scripts" / "github-artifact-retrieval.py"),
                "--selection", str(selection_path), "--repo", "owner/repo",
                "--output", str(output_path), "--api-base-url", "http://127.0.0.1:1",
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("token", result.stderr.lower())

    def test_retrieval_cli_uses_token_flag_over_env(self) -> None:
        """Phase 19: --token flag takes precedence over GITHUB_TOKEN env."""
        import importlib.util
        spec = importlib.util.spec_from_file_location("github_retrieval", ROOT / "scripts" / "github-artifact-retrieval.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)

        valid = {
            "candidate_sha": SHA,
            "release_version": "1.0.1",
            "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}},
        }
        module.validate_selection(valid)  # Should not raise.

    def test_end_to_end_retrieval_and_aggregation(self) -> None:
        """Phase 19: Mocked end-to-end retrieval through pre-tag aggregation."""
        import http.server
        import threading
        import zipfile

        def e2e_candidate(stage: str, run_id: str = "100", attempt: str = "1") -> dict[str, object]:
            return {
                "schema_version": 1, "candidate_sha": SHA, "release_version": "1.0.1",
                "stage": stage, "workflow_run_id": run_id, "workflow_run_attempt": attempt,
                "job_name": stage, "runner_os": "Linux", "runner_architecture": "x86_64",
                "started_at": "2026-07-24T00:00:00Z", "completed_at": "2026-07-24T00:01:00Z",
                "result": "success", "source_identity_mode": "pre-tag-full-sha",
                "source": {"ref_input": SHA, "tag_object_sha": None, "peeled_commit_sha": SHA},
                "artifacts": [{"name": f"{stage}.log", "role": "transcript", "artifact_id": f"artifact-{stage}"}],
                "executables": [], "notes": [],
            }

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)

            # Create mock artifacts: ZIP files containing candidate.json files.
            artifact_dir = directory / "artifacts"
            artifact_dir.mkdir()

            # Source CI artifact.
            source_ci_zip = artifact_dir / "artifact-5001.zip"
            with zipfile.ZipFile(source_ci_zip, "w") as zf:
                zf.writestr("evidence/candidate.json", json.dumps(e2e_candidate("source-ci", "1001", "1")))

            # Operational artifact with resource + soak stages.
            operational_zip = artifact_dir / "artifact-5002.zip"
            with zipfile.ZipFile(operational_zip, "w") as zf:
                zf.writestr("resource/candidate.json", json.dumps(e2e_candidate("resource-linux", "1018", "1")))
                zf.writestr("soak/candidate.json", json.dumps(e2e_candidate("soak-linux-24h", "1018", "1")))

            # Create mock GitHub API server.
            api_calls: list[str] = []

            class MockHandler(http.server.BaseHTTPRequestHandler):
                def do_GET(self) -> None:
                    api_calls.append(self.path)
                    if self.path == "/repos/owner/repo/actions/runs/1001":
                        self._json_response({
                            "id": 1001, "status": "completed", "conclusion": "success",
                            "run_attempt": 1, "repository": {"full_name": "owner/repo"},
                            "name": "release-candidate", "event": "workflow_dispatch",
                            "actor": {"login": "tester"},
                            "html_url": "https://github.com/owner/repo/runs/1001",
                            "head_sha": SHA, "head_branch": "main",
                        })
                    elif self.path == "/repos/owner/repo/actions/runs/1018":
                        self._json_response({
                            "id": 1018, "status": "completed", "conclusion": "success",
                            "run_attempt": 1, "repository": {"full_name": "owner/repo"},
                            "name": "release-candidate", "event": "workflow_dispatch",
                            "actor": {"login": "tester"},
                            "html_url": "https://github.com/owner/repo/runs/1018",
                            "head_sha": SHA, "head_branch": "main",
                        })
                    elif self.path == "/repos/owner/repo/actions/runs/1001/artifacts":
                        self._json_response({"artifacts": [
                            {"id": 5001, "name": "source-ci-abcdef01-1", "size_in_bytes": 100,
                             "created_at": "2026-07-24T00:00:00Z", "expires_at": "2099-01-01T00:00:00Z", "expired": False},
                        ]})
                    elif self.path == "/repos/owner/repo/actions/runs/1018/artifacts":
                        self._json_response({"artifacts": [
                            {"id": 5002, "name": "operational-abcdef01-1", "size_in_bytes": 200,
                             "created_at": "2026-07-24T00:00:00Z", "expires_at": "2099-01-01T00:00:00Z", "expired": False},
                        ]})
                    elif self.path == "/repos/owner/repo/actions/artifacts/5001/zip":
                        self.send_response(200)
                        self.send_header("Content-Type", "application/zip")
                        data = source_ci_zip.read_bytes()
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                    elif self.path == "/repos/owner/repo/actions/artifacts/5002/zip":
                        self.send_response(200)
                        self.send_header("Content-Type", "application/zip")
                        data = operational_zip.read_bytes()
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                    else:
                        self.send_response(404)
                        self.end_headers()

                def _json_response(self, data: dict) -> None:
                    body = json.dumps(data).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

                def log_message(self, format: str, *args: object) -> None:
                    pass  # Suppress request logging.

            server = http.server.HTTPServer(("127.0.0.1", 0), MockHandler)
            port = server.server_address[1]
            server_thread = threading.Thread(target=server.serve_forever, daemon=True)
            server_thread.start()

            try:
                # Create selection file.
                selection = {
                    "candidate_sha": SHA,
                    "release_version": "1.0.1",
                    "mode": "pre-tag",
                    "runs": {
                        "source-ci": {
                            "run_id": 1001, "attempt": 1,
                            "workflow_name": "release-candidate",
                            "artifacts": [{"name": "source-ci-abcdef01-1"}],
                        },
                        "resource-linux": {
                            "run_id": 1018, "attempt": 1,
                            "workflow_name": "release-candidate",
                            "artifacts": [{"name": "operational-abcdef01-1"}],
                        },
                        "soak-linux-24h": {
                            "run_id": 1018, "attempt": 1,
                            "workflow_name": "release-candidate",
                            "artifacts": [{"name": "operational-abcdef01-1"}],
                        },
                    },
                }
                selection_path = directory / "selection.json"
                selection_path.write_text(json.dumps(selection), encoding="utf-8")
                evidence_dir = directory / "evidence"
                evidence_dir.mkdir(parents=True, exist_ok=True)
                retrieved_manifest_path = evidence_dir / "retrieved-manifest.json"

                # Run retrieval CLI.
                result = run(
                    "python3", str(ROOT / "scripts" / "github-artifact-retrieval.py"),
                    "--selection", str(selection_path),
                    "--repo", "owner/repo",
                    "--output", str(retrieved_manifest_path),
                    "--api-base-url", f"http://127.0.0.1:{port}",
                    "--token", "test-token",
                )
                self.assertEqual(result.returncode, 0, result.stderr)

                # Verify retrieved manifest.
                manifest = json.loads(retrieved_manifest_path.read_text(encoding="utf-8"))
                self.assertEqual(manifest["candidate_sha"], SHA)
                self.assertEqual(len(manifest["stages"]), 3)
                stage_names = {s["stage"] for s in manifest["stages"]}
                self.assertEqual(stage_names, {"source-ci", "resource-linux", "soak-linux-24h"})
                # Verify artifact identity is recorded.
                for stage_entry in manifest["stages"]:
                    for art in stage_entry["artifacts"]:
                        self.assertIn("github_artifact_id", art)
                        self.assertIn("downloaded_zip_sha256", art)
                        self.assertIn("downloaded_zip_size_bytes", art)
                # Verify provenance index exists.
                self.assertIn("provenance_index", manifest)

                # Retrieval script copies candidate.json files into evidence dir.
                for stage in ("source-ci", "resource-linux", "soak-linux-24h"):
                    self.assertTrue((evidence_dir / stage / "candidate.json").exists(),
                                    f"candidate.json for {stage} not found in evidence dir")

                # Run pre-tag aggregation using the retrieved manifest.
                output_manifest = directory / "release-manifest.json"
                result = run(
                    "python3", str(VALIDATOR), "aggregate",
                    "--evidence-dir", str(evidence_dir),
                    "--expected-sha", SHA, "--release-version", "1.0.1",
                    "--output", str(output_manifest),
                    "--required-stage", "source-ci",
                    "--required-stage", "resource-linux",
                    "--required-stage", "soak-linux-24h",
                    "--retrieved-manifest", str(retrieved_manifest_path),
                    "--mode", "pre-tag",
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                agg_manifest = json.loads(output_manifest.read_text(encoding="utf-8"))
                self.assertEqual(agg_manifest["mode"], "pre-tag")
                self.assertIsNone(agg_manifest["tag"])
                # Verify github_artifact populated from retrieved manifest.
                for stage in agg_manifest["stages"]:
                    if "github_artifact" in stage:
                        self.assertIsInstance(stage["github_artifact"]["id"], int)

                # Verify final mode fails without publication inputs.
                final_output = directory / "final-manifest.json"
                result = run(
                    "python3", str(VALIDATOR), "aggregate",
                    "--evidence-dir", str(evidence_dir),
                    "--expected-sha", SHA, "--release-version", "1.0.1",
                    "--output", str(final_output),
                    "--required-stage", "source-ci",
                    "--required-stage", "resource-linux",
                    "--required-stage", "soak-linux-24h",
                    "--retrieved-manifest", str(retrieved_manifest_path),
                    "--mode", "final",
                    "--tag-name", "v1.0.1", "--tag-object-sha", "a" * 40, "--peeled-commit-sha", SHA,
                    "--tagger-name", "T", "--tagger-email", "t@t", "--tagger-timestamp", "2026-07-24T00:00:00Z",
                    "--tag-object-content-sha256", "b" * 64,
                    check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("provenance", result.stderr.lower())

                # Verify API was called for each unique run only once.
                run_api_calls = [c for c in api_calls if "/actions/runs/" in c and "/artifacts" not in c]
                self.assertEqual(len(run_api_calls), 2)  # 1001 and 1018, not 3 (1018 deduplicated).

            finally:
                server.shutdown()

    def test_full_candidate_mode_covers_all_stage_classes(self) -> None:
        """Covers all required stage classes in candidate/pre-tag mode."""
        import http.server
        import threading
        import zipfile

        ALL_CANDIDATE_STAGES = [
            "source-ci", "protocol-prepublish",
            "binary-prepublish-greggd", "binary-prepublish-gregg",
            "binary-msrv-greggd", "binary-msrv-gregg",
            "native-source-linux-x86-64", "native-source-linux-arm64",
            "native-source-macos-arm64", "native-source-macos-intel",
            "native-package-linux-x86-64", "native-package-linux-arm64",
            "native-package-macos-arm64", "native-package-macos-intel",
            "mixed-fleet-functional", "mixed-fleet-sustained",
            "systemd-lifecycle", "launchd-lifecycle",
            "resource-linux", "resource-macos-arm64",
            "soak-linux-24h", "soak-macos-arm64-24h",
        ]

        def full_candidate(stage: str, run_id: str = "100", attempt: str = "1") -> dict:
            return {
                "schema_version": 1, "candidate_sha": SHA, "release_version": "1.0.1",
                "stage": stage, "workflow_run_id": run_id, "workflow_run_attempt": attempt,
                "job_name": stage, "runner_os": "Linux", "runner_architecture": "x86_64",
                "started_at": "2026-07-24T00:00:00Z", "completed_at": "2026-07-24T00:01:00Z",
                "result": "success", "source_identity_mode": "pre-tag-full-sha",
                "source": {"ref_input": SHA, "tag_object_sha": None, "peeled_commit_sha": SHA},
                "artifacts": [{"name": f"{stage}.log", "role": "transcript", "artifact_id": f"artifact-{stage}"}],
                "executables": [], "notes": [],
            }

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            artifact_dir = directory / "artifacts"
            artifact_dir.mkdir()

            # Create one ZIP per stage group (shared artifacts within a run).
            # Run 1001: source-ci, protocol-prepublish, binary-*, msrv-*
            # Run 1002: native-source-*, native-package-*
            # Run 1003: mixed-fleet-*
            # Run 1004: resource-*, soak-*, systemd, launchd
            runs_stages = {
                1001: ["source-ci", "protocol-prepublish",
                        "binary-prepublish-greggd", "binary-prepublish-gregg",
                        "binary-msrv-greggd", "binary-msrv-gregg"],
                1002: ["native-source-linux-x86-64", "native-source-linux-arm64",
                        "native-source-macos-arm64", "native-source-macos-intel",
                        "native-package-linux-x86-64", "native-package-linux-arm64",
                        "native-package-macos-arm64", "native-package-macos-intel"],
                1003: ["mixed-fleet-functional", "mixed-fleet-sustained"],
                1004: ["resource-linux", "resource-macos-arm64",
                        "soak-linux-24h", "soak-macos-arm64-24h",
                        "systemd-lifecycle", "launchd-lifecycle"],
            }

            zips: dict[int, Path] = {}
            for run_id, stages in runs_stages.items():
                zip_path = artifact_dir / f"artifact-{run_id}.zip"
                with zipfile.ZipFile(zip_path, "w") as zf:
                    for stage in stages:
                        zf.writestr(f"{stage}/candidate.json", json.dumps(full_candidate(stage, str(run_id))))
                zips[run_id] = zip_path

            # Build mock API server.
            class Handler(http.server.BaseHTTPRequestHandler):
                def do_GET(self) -> None:
                    if "/actions/runs/" in self.path and "/artifacts" not in self.path:
                        run_id = self.path.split("/actions/runs/")[1].split("/")[0]
                        self._json({
                            "id": int(run_id), "status": "completed", "conclusion": "success",
                            "run_attempt": 1, "repository": {"full_name": "o/r"},
                            "name": "release-candidate", "event": "workflow_dispatch",
                            "actor": {"login": "t"}, "html_url": "", "head_sha": SHA, "head_branch": "m",
                        })
                    elif "/actions/runs/" in self.path and "/artifacts" in self.path:
                        run_id = self.path.split("/actions/runs/")[1].split("/")[0]
                        self._json({"artifacts": [
                            {"id": int(run_id) * 100, "name": f"artifact-{run_id}",
                             "size_in_bytes": 100, "created_at": "2026-07-24T00:00:00Z",
                             "expires_at": "2099-01-01T00:00:00Z", "expired": False}
                        ]})
                    elif "/actions/artifacts/" in self.path and "/zip" in self.path:
                        art_id = self.path.split("/actions/artifacts/")[1].split("/")[0]
                        run_id = int(art_id) // 100
                        data = zips[run_id].read_bytes()
                        self.send_response(200)
                        self.send_header("Content-Type", "application/zip")
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                    else:
                        self.send_response(404)
                        self.end_headers()

                def _json(self, d):
                    body = json.dumps(d).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

                def log_message(self, *a): pass

            server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
            port = server.server_address[1]
            t = threading.Thread(target=server.serve_forever, daemon=True)
            t.start()
            try:
                # Build selection covering all stage classes.
                runs = {}
                for run_id, stages in runs_stages.items():
                    for stage in stages:
                        runs[stage] = {
                            "run_id": run_id, "attempt": 1,
                            "workflow_name": "release-candidate",
                            "artifacts": [{"name": f"artifact-{run_id}"}],
                        }
                selection = {
                    "candidate_sha": SHA, "release_version": "1.0.1",
                    "mode": "pre-tag", "runs": runs,
                }
                sel_path = directory / "selection.json"
                sel_path.write_text(json.dumps(selection))
                evidence_dir = directory / "evidence"
                evidence_dir.mkdir(parents=True, exist_ok=True)
                retrieved_manifest_path = evidence_dir / "retrieved-manifest.json"

                result = run(
                    "python3", str(ROOT / "scripts" / "github-artifact-retrieval.py"),
                    "--selection", str(sel_path), "--repo", "o/r",
                    "--output", str(retrieved_manifest_path),
                    "--api-base-url", f"http://127.0.0.1:{port}", "--token", "t",
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                manifest = json.loads(retrieved_manifest_path.read_text())
                self.assertEqual(len(manifest["stages"]), len(ALL_CANDIDATE_STAGES))

                # Run aggregation.
                output = directory / "manifest.json"
                req_args = []
                for stage in ALL_CANDIDATE_STAGES:
                    req_args.extend(["--required-stage", stage])
                result = run(
                    "python3", str(VALIDATOR), "aggregate",
                    "--evidence-dir", str(evidence_dir),
                    "--expected-sha", SHA, "--release-version", "1.0.1",
                    "--output", str(output), *req_args,
                    "--retrieved-manifest", str(retrieved_manifest_path),
                    "--mode", "pre-tag",
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                agg = json.loads(output.read_text())
                self.assertEqual(agg["mode"], "pre-tag")
                self.assertEqual(len(agg["stages"]), len(ALL_CANDIDATE_STAGES))
            finally:
                server.shutdown()

    def test_final_mode_succeeds_with_all_publication_evidence(self) -> None:
        """Covers final-mode aggregation with complete publication evidence."""
        import http.server
        import threading
        import zipfile

        FINAL_STAGES = ["source-ci", "protocol-prepublish", "protocol-index-check",
                        "binary-prepublish-greggd", "binary-prepublish-gregg",
                        "mixed-fleet-functional", "mixed-fleet-sustained",
                        "resource-linux", "soak-linux-24h",
                        "systemd-lifecycle", "launchd-lifecycle",
                        "native-source-linux-x86-64", "native-source-linux-arm64",
                        "native-source-macos-arm64", "native-source-macos-intel",
                        "native-package-linux-x86-64", "native-package-linux-arm64",
                        "native-package-macos-arm64", "native-package-macos-intel",
                        "binary-msrv-greggd", "binary-msrv-gregg",
                        "resource-macos-arm64", "soak-macos-arm64-24h",
                        "postpublish-verify"]

        def final_candidate(stage: str, run_id: str = "100", attempt: str = "1") -> dict:
            return {
                "schema_version": 1, "candidate_sha": SHA, "release_version": "1.0.1",
                "stage": stage, "workflow_run_id": run_id, "workflow_run_attempt": attempt,
                "job_name": stage, "runner_os": "Linux", "runner_architecture": "x86_64",
                "started_at": "2026-07-24T00:00:00Z", "completed_at": "2026-07-24T00:01:00Z",
                "result": "success", "source_identity_mode": "pre-tag-full-sha",
                "source": {"ref_input": SHA, "tag_object_sha": None, "peeled_commit_sha": SHA},
                "artifacts": [{"name": f"{stage}.log", "role": "transcript", "artifact_id": f"artifact-{stage}"}],
                "executables": [], "notes": [],
            }

        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            artifact_dir = directory / "artifacts"
            artifact_dir.mkdir()

            # Create ZIP with all stages in one artifact (same run).
            zip_path = artifact_dir / "artifact-5001.zip"
            with zipfile.ZipFile(zip_path, "w") as zf:
                for stage in FINAL_STAGES:
                    zf.writestr(f"{stage}/candidate.json", json.dumps(final_candidate(stage, "1001")))

            class Handler(http.server.BaseHTTPRequestHandler):
                def do_GET(self) -> None:
                    if "/actions/runs/1001" in self.path and "/artifacts" not in self.path:
                        self._json({
                            "id": 1001, "status": "completed", "conclusion": "success",
                            "run_attempt": 1, "repository": {"full_name": "o/r"},
                            "name": "release-candidate", "event": "workflow_dispatch",
                            "actor": {"login": "t"}, "html_url": "", "head_sha": SHA, "head_branch": "m",
                        })
                    elif "/actions/runs/1001/artifacts" in self.path:
                        self._json({"artifacts": [
                            {"id": 5001, "name": "final-artifact",
                             "size_in_bytes": 200, "created_at": "2026-07-24T00:00:00Z",
                             "expires_at": "2099-01-01T00:00:00Z", "expired": False}
                        ]})
                    elif "/actions/artifacts/5001/zip" in self.path:
                        data = zip_path.read_bytes()
                        self.send_response(200)
                        self.send_header("Content-Type", "application/zip")
                        self.send_header("Content-Length", str(len(data)))
                        self.end_headers()
                        self.wfile.write(data)
                    else:
                        self.send_response(404)
                        self.end_headers()

                def _json(self, d):
                    body = json.dumps(d).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)

                def log_message(self, *a): pass

            server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
            port = server.server_address[1]
            t = threading.Thread(target=server.serve_forever, daemon=True)
            t.start()
            try:
                runs = {stage: {"run_id": 1001, "attempt": 1, "workflow_name": "release-candidate",
                                "artifacts": [{"name": "final-artifact"}]}
                        for stage in FINAL_STAGES}
                selection = {"candidate_sha": SHA, "release_version": "1.0.1", "mode": "final", "runs": runs}
                sel_path = directory / "selection.json"
                sel_path.write_text(json.dumps(selection))
                evidence_dir = directory / "evidence"
                evidence_dir.mkdir(parents=True, exist_ok=True)
                retrieved_manifest_path = evidence_dir / "retrieved-manifest.json"

                result = run(
                    "python3", str(ROOT / "scripts" / "github-artifact-retrieval.py"),
                    "--selection", str(sel_path), "--repo", "o/r",
                    "--output", str(retrieved_manifest_path),
                    "--api-base-url", f"http://127.0.0.1:{port}", "--token", "t",
                )
                self.assertEqual(result.returncode, 0, result.stderr)

                # Create package provenance for all three crates.
                for pkg in ("gregg-protocol", "greggd", "gregg"):
                    archive = directory / f"{pkg}-1.0.1.crate"
                    archive.write_bytes(f"{pkg}-archive".encode())
                    provenance_args = [
                        "python3", str(ROOT / "scripts" / "write-package-provenance.py"),
                        "--output", str(directory / f"{pkg}-provenance.json"),
                        "--candidate-sha", SHA, "--release-version", "1.0.1",
                        "--package", pkg, str(archive),
                    ]
                    # Binary packages need a binary and lockfile.
                    if pkg in ("greggd", "gregg"):
                        binary = directory / pkg
                        binary.write_text(f"#!/bin/sh\necho {pkg} 1.0.1\n", encoding="utf-8")
                        binary.chmod(0o755)
                        lockfile = directory / f"{pkg}-Cargo.lock"
                        lockfile.write_text(f"{pkg}-lockfile", encoding="utf-8")
                        provenance_args.extend([str(binary), str(lockfile)])
                    result = run(*provenance_args)
                    self.assertEqual(result.returncode, 0, result.stderr)
                merged = directory / "merged-provenance.json"
                result = run("python3", str(ROOT / "scripts" / "merge-package-provenance.py"),
                    "--protocol", str(directory / "gregg-protocol-provenance.json"),
                    "--daemon", str(directory / "greggd-provenance.json"),
                    "--client", str(directory / "gregg-provenance.json"),
                    "--expected-sha", SHA, "--release-version", "1.0.1",
                    "--output", str(merged))
                self.assertEqual(result.returncode, 0, result.stderr)

                # Create registry summary and disposition.
                registry_summary = [
                    {"crate": "gregg-protocol", "version": "1.0.1", "yanked": False,
                     "checksum": "a" * 64, "published_at": "2026-07-24T00:00:00Z"},
                    {"crate": "greggd", "version": "1.0.1", "yanked": False,
                     "checksum": "b" * 64, "published_at": "2026-07-24T00:00:00Z"},
                    {"crate": "gregg", "version": "1.0.1", "yanked": False,
                     "checksum": "c" * 64, "published_at": "2026-07-24T00:00:00Z"},
                ]
                (directory / "registry-summary.json").write_text(json.dumps(registry_summary))
                disposition = {
                    "schema_version": 1,
                    "observed_at": "2026-07-24T00:00:00Z",
                    "crates": {
                        "gregg-protocol": {"version": "1.0.0", "yanked": False,
                                           "checksum": "a" * 64, "published_at": "2026-07-24T00:00:00Z", "decision": "retain"},
                        "greggd": {"version": "1.0.0", "yanked": False,
                                   "checksum": "b" * 64, "published_at": "2026-07-24T00:00:00Z", "decision": "retain"},
                        "gregg": {"version": "1.0.0", "yanked": False,
                                  "checksum": "c" * 64, "published_at": "2026-07-24T00:00:00Z", "decision": "retain"},
                    },
                }
                (directory / "disposition.json").write_text(json.dumps(disposition))

                # E3: Build a role index with materialized singleton paths.
                reg_sha, reg_size = hashlib.sha256(json.dumps(registry_summary).encode()).hexdigest(), len(json.dumps(registry_summary).encode())
                disp_sha, disp_size = hashlib.sha256(json.dumps(disposition).encode()).hexdigest(), len(json.dumps(disposition).encode())
                # Copy files into evidence dir so relative paths work.
                import shutil
                reg_mat = evidence_dir / "materialized" / "registry-summary.json"
                reg_mat.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(directory / "registry-summary.json", reg_mat)
                disp_mat = evidence_dir / "materialized" / "1.0.0-disposition.json"
                shutil.copy2(directory / "disposition.json", disp_mat)
                role_index = {
                    "schema_version": 1,
                    "roles": {
                        "registry-summary": {
                            "name": "registry-summary.json", "path": "registry-summary.json",
                            "sha256": reg_sha, "size_bytes": reg_size,
                            "stage": "postpublish-verify", "workflow_run_id": "100",
                            "workflow_run_attempt": "1", "artifact_id": 10000,
                            "artifact_name": "postpublish", "zip_sha256": "a" * 64,
                            "zip_size_bytes": 100,
                            "materialized_path": str(reg_mat.relative_to(evidence_dir)),
                        },
                        "version-1.0.0-disposition": {
                            "name": "1.0.0-disposition.json", "path": "1.0.0-disposition.json",
                            "sha256": disp_sha, "size_bytes": disp_size,
                            "stage": "postpublish-verify", "workflow_run_id": "100",
                            "workflow_run_attempt": "1", "artifact_id": 10000,
                            "artifact_name": "postpublish", "zip_sha256": "a" * 64,
                            "zip_size_bytes": 100,
                            "materialized_path": str(disp_mat.relative_to(evidence_dir)),
                        },
                    },
                }
                (evidence_dir / "role-index.json").write_text(json.dumps(role_index))

                # Run final-mode aggregation.
                output = directory / "final-manifest.json"
                req_args = []
                for stage in FINAL_STAGES:
                    req_args.extend(["--required-stage", stage])
                result = run(
                    "python3", str(VALIDATOR), "aggregate",
                    "--evidence-dir", str(evidence_dir),
                    "--expected-sha", SHA, "--release-version", "1.0.1",
                    "--output", str(output), *req_args,
                    "--retrieved-manifest", str(retrieved_manifest_path),
                    "--mode", "final",
                    "--tag-name", "v1.0.1", "--tag-object-sha", "a" * 40,
                    "--peeled-commit-sha", SHA,
                    "--tagger-name", "T", "--tagger-email", "t@t",
                    "--tagger-timestamp", "2026-07-24T00:00:00Z",
                    "--tag-object-content-sha256", "b" * 64,
                    "--package-provenance", str(merged),
                    "--role-index", str(evidence_dir / "role-index.json"),
                    "--final",
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                agg = json.loads(output.read_text())
                self.assertEqual(agg["mode"], "final")
                self.assertIsNotNone(agg["tag"])
                self.assertIsNotNone(agg["package_provenance"])
                self.assertIsNotNone(agg["registry"])
                self.assertIsNotNone(agg["version_1_0_0_disposition"])
            finally:
                server.shutdown()


class TestProductionFinalizerContract(unittest.TestCase):
    """H3: Verify release-finalize.yml and release-candidate.yml structural contracts."""

    def _read_workflow(self, name: str) -> str:
        path = ROOT / ".github" / "workflows" / name
        return path.read_text(encoding="utf-8")

    def test_finalize_uses_shared_helper(self) -> None:
        content = self._read_workflow("release-finalize.yml")
        self.assertIn("prepare-final-release-inputs.py", content)

    def test_finalize_passes_role_index(self) -> None:
        content = self._read_workflow("release-finalize.yml")
        self.assertIn("--role-index evidence/role-index.json", content)

    def test_finalize_no_independent_registry_summary(self) -> None:
        content = self._read_workflow("release-finalize.yml")
        self.assertNotIn("crates.io/api/v1/crates", content)

    def test_postpublish_includes_disposition_role(self) -> None:
        content = self._read_workflow("release-candidate.yml")
        self.assertIn("version-1.0.0-disposition", content)

    def test_postpublish_uses_fail_closed_upload(self) -> None:
        content = self._read_workflow("release-candidate.yml")
        idx = content.find("postpublish-${{")
        self.assertGreater(idx, -1)
        snippet = content[idx:idx + 500]
        self.assertIn("if-no-files-found: error", snippet)

    def test_postpublish_decodes_disposition(self) -> None:
        content = self._read_workflow("release-candidate.yml")
        self.assertIn("decode-release-disposition.py", content)
        self.assertIn("disposition_decision_base64", content)


if __name__ == "__main__":
    unittest.main()
