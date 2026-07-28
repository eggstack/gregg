from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
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


class Phase33ContractTests(unittest.TestCase):
    def test_lockfile_checksum_is_semantic_and_matches_registry(self) -> None:
        module = load("registry_reverify", "registry-reverify.py")
        checksum = "a" * 64
        with tempfile.TemporaryDirectory() as raw:
            lock = Path(raw) / "Cargo.lock"
            lock.write_text(
                f'''version = 3\n\n[[package]]\nname = "gregg-protocol"\nversion = "1.0.1"\nsource = "sparse+https://index.crates.io/"\nchecksum = "{checksum}"\n''',
                encoding="utf-8",
            )
            record = module.parse_lockfile_protocol(lock)
            self.assertEqual(record["checksum"], checksum)
            with self.assertRaises(module.VerificationError):
                module.validate_registry_record({"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": "b" * 64, "created_at": "2026-01-01T00:00:00Z"}}, expected_checksum=checksum)

    def test_registry_response_requires_identity_and_timestamp(self) -> None:
        module = load("registry_reverify_record", "registry-reverify.py")
        response = {"crate": "gregg-protocol", "version": {"num": "1.0.1", "yanked": False, "cksum": "c" * 64, "created_at": "2026-01-01T00:00:00Z"}}
        self.assertEqual(module.validate_registry_record(response)["crate"], "gregg-protocol")
        response["version"]["yanked"] = True
        with self.assertRaises(module.VerificationError):
            module.validate_registry_record(response)

    def test_qualification_output_validator_detects_digest_drift(self) -> None:
        module = load("qualification_output", "validate-qualification-output.py")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            item = root / "command.txt"
            item.write_text("ok\n", encoding="utf-8")
            summary = root / "qualification-summary.json"
            summary.write_text(json.dumps({"schema_version": 1, "verdict": "pass", "files": [{"path": "command.txt", "sha256": hashlib.sha256(item.read_bytes()).hexdigest(), "size_bytes": item.stat().st_size}]}), encoding="utf-8")
            self.assertEqual(module.main.__name__, "main")
            item.write_text("changed\n", encoding="utf-8")
            original = sys.argv
            try:
                sys.argv = ["validate-qualification-output.py", "--summary", str(summary)]
                self.assertEqual(module.main(), 1)
            finally:
                sys.argv = original


if __name__ == "__main__":
    unittest.main()
