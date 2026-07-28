from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class StrictLockSourceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.module = load("phase34_registry", "registry-reverify.py")

    def _lock(self, root: Path, *, source: str, checksum: str | None = "a" * 64) -> Path:
        checksum_line = "" if checksum is None else f'checksum = "{checksum}"\n'
        path = root / "Cargo.lock"
        path.write_text(
            'version = 3\n\n[[package]]\nname = "gregg-protocol"\n'
            f'version = "1.0.1"\nsource = "{source}"\n{checksum_line}',
            encoding="utf-8",
        )
        return path

    def test_rejects_directory_source_against_expected_registry(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            with self.assertRaisesRegex(self.module.VerificationError, "does not match expected"):
                self.module.parse_lockfile_protocol(
                    self._lock(Path(raw), source="directory+/tmp/vendor"),
                    expected_source="sparse+http://127.0.0.1:1234/",
                )

    def test_rejects_git_source_against_expected_registry(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            with self.assertRaisesRegex(self.module.VerificationError, "does not match expected"):
                self.module.parse_lockfile_protocol(
                    self._lock(Path(raw), source="git+https://example.invalid/repo"),
                    expected_source="sparse+https://index.crates.io/",
                )

    def test_rejects_null_checksum_in_every_mode(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            with self.assertRaisesRegex(self.module.VerificationError, "checksum"):
                self.module.parse_lockfile_protocol(
                    self._lock(Path(raw), source="sparse+http://127.0.0.1:1234/", checksum=None)
                )


class SparseRegistryFixtureTests(unittest.TestCase):
    def test_index_path_and_checksum_record(self) -> None:
        module = load("phase34_sparse", "local_sparse_registry.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            archive = root / "gregg-protocol-1.0.1.crate"
            archive.write_bytes(b"exact packaged bytes")
            fixture = module.LocalSparseRegistry(
                root / "registry", archive, crate="gregg-protocol", version="1.0.1"
            )
            source = fixture.start()
            try:
                self.assertTrue(source.startswith("sparse+http://127.0.0.1:"))
                record = json.loads(
                    (fixture.root / "gr" / "eg" / "gregg-protocol").read_text(encoding="utf-8")
                )
                self.assertEqual(record["cksum"], hashlib.sha256(archive.read_bytes()).hexdigest())
                cargo_home = root / "cargo-home"
                fixture.write_cargo_home(cargo_home, source)
                config = (cargo_home / "config.toml").read_text(encoding="utf-8")
                self.assertIn("[registries.phase34-local-registry]", config)
                self.assertNotIn("directory =", config)
            finally:
                fixture.shutdown()


class CandidateArtifactValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.module = load("phase34_evidence", "validate-release-evidence.py")

    def _args(self, root: Path, artifacts: list[dict]):
        artifacts_path = root / "artifacts.json"
        artifacts_path.write_text(json.dumps(artifacts), encoding="utf-8")
        return type("Args", (), {
            "candidate_sha": "a" * 40, "release_version": "1.0.1",
            "artifacts_json": str(artifacts_path), "artifact_root": str(root),
            "executable": [], "output": root / "candidate.json",
            "stage": "source-ci", "workflow_run_id": "1",
            "workflow_run_attempt": "1", "job_name": "source-ci",
            "runner_os": "Linux", "runner_architecture": "x86_64",
            "started_at": None, "completed_at": None, "result": "success",
            "source_identity_mode": "pre-tag-full-sha", "ref_input": None,
            "tag_object_sha": None, "peeled_commit_sha": None,
            "tagger_name": None, "tagger_email": None,
            "tagger_timestamp": None, "tagger_timestamp_original": None,
            "tag_object_content_sha256": None, "head_sha": None, "note": [],
        })()

    def test_rejects_nonexistent_declared_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            args = self._args(root, [{
                "name": "missing.json", "path": "missing.json", "role": "summary",
                "sha256": "a" * 64, "size_bytes": 1,
            }])
            with self.assertRaisesRegex(self.module.EvidenceError, "missing"):
                self.module.write_candidate(args)

    def test_rejects_digest_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "result.json").write_text("{}\n", encoding="utf-8")
            args = self._args(root, [{
                "name": "result.json", "path": "result.json", "role": "summary",
                "sha256": "a" * 64, "size_bytes": 3,
            }])
            with self.assertRaisesRegex(self.module.EvidenceError, "digest/size"):
                self.module.write_candidate(args)

    def test_rejects_path_escape(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            args = self._args(root, [{
                "name": "../outside", "path": "../outside", "role": "summary",
                "sha256": "a" * 64, "size_bytes": 1,
            }])
            with self.assertRaisesRegex(self.module.EvidenceError, "escapes"):
                self.module.write_candidate(args)


class QualificationContractTests(unittest.TestCase):
    def test_contract_names_every_mandatory_chain_and_singleton(self) -> None:
        contract = json.loads(
            (ROOT / "plans/evidence/phase34-qualification-contract.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            contract["required_chains"],
            ["candidate_pre_tag", "protocol_publication", "boundary_2", "final"],
        )
        self.assertEqual(
            set(contract["required_singleton_roles"]),
            {"registry-summary", "version-1.0.0-disposition"},
        )
        self.assertEqual(len(contract["required_command_names"]), 10)
        self.assertEqual(len(contract["required_negative_cases"]), 45)


class Phase35QualificationContractTests(unittest.TestCase):
    """Phase 35 supersedes Phase 34 for evidence-lineage and finalizer defects."""

    def test_phase35_contract_names_every_mandatory_chain_and_singleton(self) -> None:
        contract = json.loads(
            (ROOT / "plans/evidence/phase35-qualification-contract.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            contract["required_chains"],
            ["candidate_pre_tag", "protocol_publication", "boundary_2", "final"],
        )
        self.assertEqual(
            set(contract["required_singleton_roles"]),
            {"registry-summary", "version-1.0.0-disposition"},
        )
        self.assertEqual(len(contract["required_command_names"]), 10)
        # Phase 35 adds cross-binding and restored Phase-34 rejection cases.
        self.assertGreater(len(contract["required_negative_cases"]), 45)

    def test_phase35_contract_preserves_all_phase34_negative_cases(self) -> None:
        phase34 = json.loads(
            (ROOT / "plans/evidence/phase34-qualification-contract.json").read_text(encoding="utf-8")
        )
        phase35 = json.loads(
            (ROOT / "plans/evidence/phase35-qualification-contract.json").read_text(encoding="utf-8")
        )
        phase34_cases = set(phase34["required_negative_cases"])
        phase35_cases = set(phase35["required_negative_cases"])
        missing = phase34_cases - phase35_cases
        self.assertEqual(missing, set(), f"Phase 35 dropped Phase 34 cases: {missing}")

    def test_phase35_contract_includes_cross_binding_cases(self) -> None:
        contract = json.loads(
            (ROOT / "plans/evidence/phase35-qualification-contract.json").read_text(encoding="utf-8")
        )
        required_cross_binding = {
            "boundary2-generic-replacement-greggd",
            "boundary2-generic-replacement-gregg",
            "boundary2-artifact-swapped-packages",
            "boundary2-candidate-digest-mismatch",
            "boundary2-selected-run-mismatch",
            "boundary2-selected-attempt-mismatch",
            "boundary2-selected-artifact-id-mismatch",
            "boundary2-selected-zip-digest-mismatch",
            "boundary1-boundary2-archive-mismatch-greggd",
            "boundary1-boundary2-archive-mismatch-gregg",
            "protocol-archive-registry-mismatch",
            "final-provenance-archive-replacement",
            "final-provenance-package-swap",
            "postpublish-file-absent-from-zip",
            "postpublish-manual-artifact-attribution",
            "postpublish-candidate-role-missing",
            "postpublish-candidate-role-wrong-path",
            "postpublish-candidate-role-digest-mismatch",
            "postpublish-role-wrong-stage",
            "postpublish-duplicate-singleton-role",
            "direct-final-registry-summary-path",
            "direct-final-disposition-path",
            "missing-final-role-index",
            "role-index-not-from-retrieved-artifact",
            "materialized-file-mutated",
            "role-materialization-after-aggregate",
            "contract-requirements-digest-mismatch",
            "contract-dispatch-digest-mismatch",
            "contract-qualification-digest-mismatch",
            "hosted-implementation-sha-mismatch",
            "final-sequence-order-invalid",
        }
        cases = set(contract["required_negative_cases"])
        missing = required_cross_binding - cases
        self.assertEqual(missing, set(), f"Phase 35 contract missing cases: {missing}")

    def test_phase35_contract_no_duplicate_ids(self) -> None:
        contract = json.loads(
            (ROOT / "plans/evidence/phase35-qualification-contract.json").read_text(encoding="utf-8")
        )
        cases = contract["required_negative_cases"]
        self.assertEqual(len(cases), len(set(cases)), "Phase 35 contract has duplicate negative-case IDs")
