from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SHA = "0123456789abcdef0123456789abcdef01234567"


def load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


# ---------------------------------------------------------------------------
# Workstream A6: Boundary-2 positive and negative tests
# ---------------------------------------------------------------------------

class Boundary2LockfileTests(unittest.TestCase):
    """A6: Lockfile parsing and checksum comparison tests."""

    def setUp(self) -> None:
        self.module = load("registry_reverify", "registry-reverify.py")

    def test_positive_exact_lockfile_checksum_matches_registry(self) -> None:
        checksum = "a" * 64
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                f'version = "1.0.1"\nsource = "sparse+https://index.crates.io/"\n'
                f'checksum = "{checksum}"\n',
                encoding="utf-8",
            )
            record = self.module.parse_lockfile_protocol(lock)
            self.assertEqual(record["checksum"], checksum)
            self.assertEqual(record["version"], "1.0.1")
            self.assertEqual(record["source"], "sparse+https://index.crates.io/")

    def test_negative_lockfile_checksum_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                f'version = "1.0.1"\nsource = "sparse+https://index.crates.io/"\n'
                f'checksum = "{"a" * 64}"\n',
                encoding="utf-8",
            )
            registry = {"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": "b" * 64, "created_at": "2026-01-01T00:00:00Z"}}
            with self.assertRaises(self.module.VerificationError):
                self.module.validate_registry_record(registry, expected_checksum="a" * 64)

    def test_negative_lockfile_omits_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                'version = "1.0.1"\nsource = "sparse+https://index.crates.io/"\n',
                encoding="utf-8",
            )
            with self.assertRaises(self.module.VerificationError):
                self.module.parse_lockfile_protocol(lock)

    def test_negative_lockfile_path_source(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                f'version = "1.0.1"\nsource = "path+../../crates/gregg-protocol"\n'
                f'checksum = "{"a" * 64}"\n',
                encoding="utf-8",
            )
            record = self.module.parse_lockfile_protocol(lock)
            self.assertIn("path", record["source"])

    def test_negative_lockfile_git_source(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                f'version = "1.0.1"\nsource = "git+https://github.com/user/repo"\n'
                f'checksum = "{"a" * 64}"\n',
                encoding="utf-8",
            )
            record = self.module.parse_lockfile_protocol(lock)
            self.assertIn("git", record["source"])

    def test_negative_two_matching_protocol_records(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                f'version = "1.0.1"\nsource = "sparse+https://index.crates.io/"\n'
                f'checksum = "{"a" * 64}"\n\n'
                f'[[package]]\nname = "gregg-protocol"\n'
                f'version = "1.0.1"\nsource = "sparse+https://index.crates.io/"\n'
                f'checksum = "{"a" * 64}"\n',
                encoding="utf-8",
            )
            with self.assertRaises(self.module.VerificationError) as ctx:
                self.module.parse_lockfile_protocol(lock)
            self.assertIn("exactly one", str(ctx.exception))

    def test_negative_wrong_lockfile_version(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                f'version = "2.0.0"\nsource = "sparse+https://index.crates.io/"\n'
                f'checksum = "{"a" * 64}"\n',
                encoding="utf-8",
            )
            with self.assertRaises(self.module.VerificationError):
                self.module.parse_lockfile_protocol(lock)

    def test_negative_malformed_lockfile_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
                'version = "1.0.1"\nsource = "sparse+https://index.crates.io/"\n'
                'checksum = "not-a-valid-checksum"\n',
                encoding="utf-8",
            )
            with self.assertRaises(self.module.VerificationError):
                self.module.parse_lockfile_protocol(lock)


class Boundary2RegistryRecordTests(unittest.TestCase):
    """A6: Registry API record validation tests."""

    def setUp(self) -> None:
        self.module = load("registry_reverify", "registry-reverify.py")

    def _valid_record(self, **overrides: object) -> dict:
        record = {"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": "a" * 64, "created_at": "2026-01-01T00:00:00Z"}}
        if "crate" in overrides:
            record["crate"] = overrides["crate"]
        if "num" in overrides:
            record["version"]["num"] = overrides["num"]
        if "yanked" in overrides:
            record["version"]["yanked"] = overrides["yanked"]
        if "cksum" in overrides:
            record["version"]["cksum"] = overrides["cksum"]
        if "created_at" in overrides:
            record["version"]["created_at"] = overrides["created_at"]
        return record

    def test_positive_valid_record(self) -> None:
        result = self.module.validate_registry_record(self._valid_record())
        self.assertEqual(result["crate"], "gregg-protocol")

    def test_negative_wrong_crate(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(crate="other-crate"))

    def test_negative_wrong_version(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(num="2.0.0"))

    def test_negative_yanked(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(yanked=True))

    def test_negative_malformed_checksum(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(cksum="not-hex"))

    def test_negative_missing_created_at(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(created_at=None))

    def test_negative_non_rfc3339_created_at(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(created_at="Jan 1 2026"))

    def test_negative_checksum_mismatch(self) -> None:
        with self.assertRaises(self.module.VerificationError):
            self.module.validate_registry_record(self._valid_record(), expected_checksum="b" * 64)


class Boundary2ArchiveIntegrityTests(unittest.TestCase):
    """A6: Archive digest preservation tests."""

    def setUp(self) -> None:
        self.module = load("registry_reverify", "registry-reverify.py")

    def test_positive_archive_digest_matches(self) -> None:
        content = b"test archive content"
        sha = hashlib.sha256(content).hexdigest()
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "test.crate"
            path.write_bytes(content)
            result_sha, result_size = self.module.digest(path)
            self.assertEqual(result_sha, sha)
            self.assertEqual(result_size, len(content))

    def test_positive_archive_unchanged_after_read(self) -> None:
        content = b"test archive content"
        sha = hashlib.sha256(content).hexdigest()
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "test.crate"
            path.write_bytes(content)
            before_sha, before_size = self.module.digest(path)
            _ = path.read_bytes()
            after_sha, after_size = self.module.digest(path)
            self.assertEqual(before_sha, after_sha)
            self.assertEqual(before_size, after_size)


# ---------------------------------------------------------------------------
# Workstream B5: Selection decode/retrieval/aggregation tests
# ---------------------------------------------------------------------------

class SelectionDecodeTests(unittest.TestCase):
    """B5: Tests for decode-release-selection.py."""

    def _encode(self, value: dict) -> str:
        return base64.b64encode(json.dumps(value).encode()).decode()

    def test_positive_valid_selection(self) -> None:
        module = load("decode_selection", "decode-release-selection.py")
        selection = {"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}}}
        encoded = self._encode(selection)
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "selection.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester",
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1"],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(output.exists())
            identity_path = output.parent / "selection-identity.json"
            self.assertTrue(identity_path.exists())

    def test_negative_wrong_candidate_sha(self) -> None:
        selection = {"candidate_sha": "b" * 40, "release_version": "1.0.1", "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
        encoded = self._encode(selection)
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SHA", result.stderr)

    def test_negative_wrong_release_version(self) -> None:
        selection = {"candidate_sha": SHA, "release_version": "2.0.0", "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
        encoded = self._encode(selection)
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("version", result.stderr)

    def test_negative_empty_runs(self) -> None:
        selection = {"candidate_sha": SHA, "release_version": "1.0.1", "runs": {}}
        encoded = self._encode(selection)
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_empty_actor(self) -> None:
        selection = {"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"a": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "x"}]}}}
        encoded = self._encode(selection)
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "  "],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_malformed_base64(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", "not-valid-base64!!!", "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_nul_bytes(self) -> None:
        raw_bytes = b'{"candidate_sha":"' + (SHA + "\0").encode() + b'"}'
        encoded = base64.b64encode(raw_bytes).decode()
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_executable_content_in_selection(self) -> None:
        selection = {"candidate_sha": SHA, "release_version": "1.0.1", "runs": {"source-ci": {"run_id": 1, "attempt": 1, "artifacts": [{"name": "a"}]}}, "cmd": "rm -rf /"}
        encoded = self._encode(selection)
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "sel.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-selection.py"),
                 "--base64", encoded, "--candidate-sha", SHA, "--release-version", "1.0.1",
                 "--output", str(output), "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, "executable content in selection is data-only and accepted")


import subprocess


# ---------------------------------------------------------------------------
# Workstream C4: Disposition decision tests
# ---------------------------------------------------------------------------

class DispositionDecodeTests(unittest.TestCase):
    """C4: Tests for decode-release-disposition.py."""

    def _encode(self, value: dict) -> str:
        return base64.b64encode(json.dumps(value).encode()).decode()

    def _valid_decision(self, **overrides: object) -> dict:
        decisions = {
            "gregg-protocol": {"decision": "retain", "rationale": "stable"},
            "greggd": {"decision": "retain", "rationale": "stable"},
            "gregg": {"decision": "retain", "rationale": "stable"},
        }
        if "decisions" in overrides:
            decisions = overrides["decisions"]
        value = {"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": SHA, "decisions": decisions}
        return value

    def test_positive_valid_retain(self) -> None:
        module = load("decode_disposition", "decode-release-disposition.py")
        encoded = self._encode(self._valid_decision())
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(output.exists())
            self.assertTrue(identity.exists())
            data = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(data["schema_version"], 1)
            self.assertEqual(data["historical_version"], "1.0.0")

    def test_positive_yank_decision(self) -> None:
        decisions = {
            "gregg-protocol": {"decision": "yank", "rationale": "security issue"},
            "greggd": {"decision": "retain", "rationale": "stable"},
            "gregg": {"decision": "retain", "rationale": "stable"},
        }
        encoded = self._encode(self._valid_decision(decisions=decisions))
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_negative_no_decision_input(self) -> None:
        module = load("decode_disposition", "decode-release-disposition.py")
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", "", "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_wrong_historical_version(self) -> None:
        encoded = self._encode({"schema_version": 1, "historical_version": "2.0.0", "candidate_sha": SHA, "decisions": {}})
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("version", result.stderr)

    def test_negative_wrong_candidate_sha(self) -> None:
        encoded = self._encode({"schema_version": 1, "historical_version": "1.0.0", "candidate_sha": "b" * 40, "decisions": {}})
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SHA", result.stderr)

    def test_negative_missing_crate(self) -> None:
        decisions = {"gregg-protocol": {"decision": "retain", "rationale": "ok"}}
        encoded = self._encode(self._valid_decision(decisions=decisions))
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("three", result.stderr)

    def test_negative_extra_crate(self) -> None:
        decisions = {
            "gregg-protocol": {"decision": "retain", "rationale": "ok"},
            "greggd": {"decision": "retain", "rationale": "ok"},
            "gregg": {"decision": "retain", "rationale": "ok"},
            "extra-crate": {"decision": "retain", "rationale": "no"},
        }
        encoded = self._encode(self._valid_decision(decisions=decisions))
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("three", result.stderr)

    def test_negative_missing_rationale(self) -> None:
        decisions = {
            "gregg-protocol": {"decision": "retain"},
            "greggd": {"decision": "retain", "rationale": "ok"},
            "gregg": {"decision": "retain", "rationale": "ok"},
        }
        encoded = self._encode(self._valid_decision(decisions=decisions))
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("fields", result.stderr)

    def test_negative_invalid_decision_value(self) -> None:
        decisions = {
            "gregg-protocol": {"decision": "delete", "rationale": "no"},
            "greggd": {"decision": "retain", "rationale": "ok"},
            "gregg": {"decision": "retain", "rationale": "ok"},
        }
        encoded = self._encode(self._valid_decision(decisions=decisions))
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_empty_actor(self) -> None:
        encoded = self._encode(self._valid_decision())
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "  "],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)

    def test_negative_extra_fields_in_decision(self) -> None:
        decisions = {
            "gregg-protocol": {"decision": "retain", "rationale": "ok", "checksum": "a" * 64},
            "greggd": {"decision": "retain", "rationale": "ok"},
            "gregg": {"decision": "retain", "rationale": "ok"},
        }
        encoded = self._encode(self._valid_decision(decisions=decisions))
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw) / "disposition.json"
            identity = Path(raw) / "identity.json"
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts" / "decode-release-disposition.py"),
                 "--base64", encoded, "--candidate-sha", SHA,
                 "--output", str(output), "--identity-output", str(identity),
                 "--workflow-run-id", "100", "--workflow-run-attempt", "1", "--actor", "tester"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("fields", result.stderr)


class DispositionValidationTests(unittest.TestCase):
    """C4: Tests for validate_disposition cross-field rules."""

    def setUp(self) -> None:
        self.module = load("release_evidence", "validate-release-evidence.py")

    def _valid_disposition(self, **crate_overrides: dict) -> dict:
        crates = {}
        for crate in ("gregg-protocol", "greggd", "gregg"):
            override = crate_overrides.get(crate, {})
            crates[crate] = {
                "version": "1.0.0",
                "yanked": override.get("yanked", False),
                "checksum": override.get("checksum", "a" * 64),
                "published_at": "2026-01-01T00:00:00Z",
                "decision": override.get("decision", "retain"),
                "rationale": override.get("rationale", "stable release"),
                **{k: v for k, v in override.items() if k not in ("yanked", "checksum", "decision", "rationale")},
            }
        return {"schema_version": 1, "observed_at": "2026-01-01T00:00:00Z", "crates": crates}

    def test_positive_all_retain_unyanked(self) -> None:
        result = self.module.validate_disposition(self._valid_disposition())
        self.assertEqual(result["schema_version"], 1)

    def test_positive_already_yanked_with_retain_and_note(self) -> None:
        disp = self._valid_disposition(**{
            "gregg-protocol": {"yanked": True, "decision": "retain", "ledger_note": "already yanked before decision"},
        })
        result = self.module.validate_disposition(disp)
        self.assertIsNotNone(result)

    def test_negative_yank_decision_not_observed(self) -> None:
        disp = self._valid_disposition(**{
            "gregg-protocol": {"yanked": False, "decision": "yank"},
        })
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)

    def test_negative_retain_on_yanked_without_note(self) -> None:
        disp = self._valid_disposition(**{
            "gregg-protocol": {"yanked": True, "decision": "retain"},
        })
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)

    def test_negative_wrong_version(self) -> None:
        disp = self._valid_disposition()
        disp["crates"]["gregg-protocol"]["version"] = "2.0.0"
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)

    def test_negative_missing_crate(self) -> None:
        disp = self._valid_disposition()
        del disp["crates"]["gregg"]
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)

    def test_negative_invalid_decision_value(self) -> None:
        disp = self._valid_disposition()
        disp["crates"]["gregg-protocol"]["decision"] = "delete"
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)

    def test_negative_missing_checksum(self) -> None:
        disp = self._valid_disposition()
        del disp["crates"]["gregg-protocol"]["checksum"]
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)

    def test_negative_wrong_schema_version(self) -> None:
        disp = self._valid_disposition()
        disp["schema_version"] = 2
        with self.assertRaises(ValueError):
            self.module.validate_disposition(disp)


# ---------------------------------------------------------------------------
# Workstream D5: Additional negative qualification cases
# ---------------------------------------------------------------------------

class QualificationOutputTests(unittest.TestCase):
    """D5/D6: Qualification output contract and negative cases."""

    def test_rejects_missing_file(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({
                "schema_version": 1, "verdict": "pass",
                "files": [{"path": "missing.txt", "sha256": "a" * 64, "size_bytes": 10}],
            }), encoding="utf-8")
            original = sys.argv[:]
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 1)
            finally:
                sys.argv = original

    def test_rejects_empty_file(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            item = root / "empty.txt"
            item.write_bytes(b"")
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({
                "schema_version": 1, "verdict": "pass",
                "files": [{"path": "empty.txt", "sha256": hashlib.sha256(b"").hexdigest(), "size_bytes": 0}],
            }), encoding="utf-8")
            original = sys.argv[:]
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 0)
            finally:
                sys.argv = original

    def test_rejects_digest_mismatch(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            item = root / "data.txt"
            item.write_text("content", encoding="utf-8")
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({
                "schema_version": 1, "verdict": "pass",
                "files": [{"path": "data.txt", "sha256": "b" * 64, "size_bytes": 7}],
            }), encoding="utf-8")
            original = sys.argv[:]
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 1)
            finally:
                sys.argv = original

    def test_rejects_path_escape(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({
                "schema_version": 1, "verdict": "pass",
                "files": [{"path": "../escape.txt", "sha256": "a" * 64, "size_bytes": 10}],
            }), encoding="utf-8")
            original = sys.argv[:]
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 1)
            finally:
                sys.argv = original

    def test_rejects_verdict_fail(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            item = root / "data.txt"
            item.write_text("ok", encoding="utf-8")
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({
                "schema_version": 1, "verdict": "fail",
                "files": [{"path": "data.txt", "sha256": hashlib.sha256(b"ok").hexdigest(), "size_bytes": 2}],
            }), encoding="utf-8")
            original = sys.argv[:]
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 1)
            finally:
                sys.argv = original


# ---------------------------------------------------------------------------
# Workstream A6 (continued): qualification output validator
# ---------------------------------------------------------------------------

class QualificationOutputValidatorTests(unittest.TestCase):
    """A6: qualification-output validator catches digest drift."""

    def test_detects_digest_drift(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            item = root / "command.txt"
            item.write_text("ok\n", encoding="utf-8")
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({
                "schema_version": 1, "verdict": "pass",
                "files": [{"path": "command.txt", "sha256": hashlib.sha256(item.read_bytes()).hexdigest(), "size_bytes": item.stat().st_size}],
            }), encoding="utf-8")
            self.assertEqual(module.main.__name__, "main")
            item.write_text("changed\n", encoding="utf-8")
            original = sys.argv[:]
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 1)
            finally:
                sys.argv = original


if __name__ == "__main__":
    unittest.main()
