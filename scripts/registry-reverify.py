#!/usr/bin/env python3
"""Re-verify an exact dependent crate archive against a registry record.

Boundary 2 deliberately consumes the Boundary 1 archive bytes.  It never
repackages the archive, and it retains every external command's transcript so
the result can be independently replayed.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 fallback is tested by CI policy.
    tomllib = None  # type: ignore[assignment]


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
PROTOCOL = "gregg-protocol"
PROTOCOL_VERSION = "1.0.1"
CRATES_IO_SOURCES = {
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
}


class VerificationError(ValueError):
    """A Boundary-2 input or command failed validation."""


def fail(message: str) -> None:
    raise VerificationError(message)


def digest(path: Path) -> tuple[str, int]:
    h = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            h.update(chunk)
            size += len(chunk)
    return h.hexdigest(), size


def parse_lockfile_protocol(lockfile: Path, *, expected_source: str | None = None) -> dict[str, str]:
    """Return the one semantic protocol package record from a Cargo.lock."""
    if tomllib is None:
        fail("Python tomllib is required to parse Cargo.lock semantically")
    try:
        with lockfile.open("rb") as handle:
            document = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse Cargo.lock: {error}")
    packages = document.get("package", [])
    matches = [item for item in packages if isinstance(item, dict) and item.get("name") == PROTOCOL]
    if len(matches) != 1:
        fail(f"Cargo.lock must contain exactly one {PROTOCOL} package record; found {len(matches)}")
    record = matches[0]
    if record.get("version") != PROTOCOL_VERSION:
        fail(f"Cargo.lock protocol version must be {PROTOCOL_VERSION}")
    source = record.get("source")
    if not isinstance(source, str):
        fail("Cargo.lock protocol package must have a registry source")
    if expected_source is not None and source != expected_source:
        fail(f"Cargo.lock protocol source {source!r} does not match expected {expected_source!r}")
    checksum = record.get("checksum")
    if not isinstance(checksum, str) or not SHA256_RE.fullmatch(checksum):
        fail("Cargo.lock protocol checksum must be lowercase 64-character SHA-256")
    return {"name": PROTOCOL, "version": PROTOCOL_VERSION, "source": source, "checksum": checksum}


def validate_registry_record(value: Any, *, expected_checksum: str | None = None) -> dict[str, Any]:
    """Validate the crates.io API response used for Boundary 2."""
    if not isinstance(value, dict) or value.get("crate") != PROTOCOL:
        fail("registry record must describe gregg-protocol")
    version = value.get("version")
    if not isinstance(version, dict) or version.get("num") != PROTOCOL_VERSION:
        fail("registry record must describe gregg-protocol 1.0.1")
    if version.get("yanked") is not False:
        fail("registry protocol version must not be yanked")
    checksum = version.get("cksum")
    if not isinstance(checksum, str) or not SHA256_RE.fullmatch(checksum):
        fail("registry protocol checksum must be lowercase 64-character SHA-256")
    created = version.get("created_at")
    if not isinstance(created, str) or not created.endswith("Z"):
        fail("registry protocol created_at must be RFC3339 UTC")
    try:
        dt.datetime.fromisoformat(created[:-1] + "+00:00")
    except ValueError as error:
        fail(f"registry protocol created_at is invalid: {error}")
    if expected_checksum is not None and checksum != expected_checksum:
        fail("registry checksum does not match supplied protocol checksum")
    return value


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def capture(command: list[str], *, cwd: Path | None, evidence_dir: Path, name: str, env: dict[str, str]) -> dict[str, Any]:
    """Run one command and persist argv, stdout, stderr, and status metadata."""
    evidence_dir.mkdir(parents=True, exist_ok=True)
    started = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    stdout_path = evidence_dir / f"{name}.stdout"
    stderr_path = evidence_dir / f"{name}.stderr"
    completed = subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True, check=False)
    stdout_path.write_text(completed.stdout, encoding="utf-8")
    stderr_path.write_text(completed.stderr, encoding="utf-8")
    finished = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    stdout_sha, stdout_size = digest(stdout_path)
    stderr_sha, stderr_size = digest(stderr_path)
    record = {
        "argv": command, "started_at": started, "completed_at": finished,
        "exit_status": completed.returncode,
        "stdout": {"path": stdout_path.name, "sha256": stdout_sha, "size_bytes": stdout_size},
        "stderr": {"path": stderr_path.name, "sha256": stderr_sha, "size_bytes": stderr_size},
    }
    _write_json(evidence_dir / f"{name}.json", record)
    if completed.returncode != 0:
        fail(f"Boundary-2 command {name} exited {completed.returncode}")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--package", choices=["greggd", "gregg"], required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--protocol-checksum", required=True)
    parser.add_argument("--registry-record", type=Path)
    parser.add_argument("--registry-source", default="sparse+https://index.crates.io/")
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    if not SHA256_RE.fullmatch(args.expected_sha256) or not SHA256_RE.fullmatch(args.protocol_checksum):
        parser.error("checksums must be lowercase 64-character SHA-256 values")
    if args.registry_source not in CRATES_IO_SOURCES and not args.registry_source.startswith("sparse+"):
        parser.error("registry source is not an approved crates.io source")
    if not args.registry_record:
        parser.error("--registry-record is required so the API response is retained")
    try:
        registry_value = json.loads(args.registry_record.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        parser.error(f"cannot read registry record: {error}")
    registry = validate_registry_record(registry_value, expected_checksum=args.protocol_checksum)
    before_sha, before_size = digest(args.archive)
    if before_sha != args.expected_sha256:
        parser.error("Boundary-1 archive checksum mismatch")
    if args.archive.name != f"{args.package}-1.0.1.crate":
        parser.error("archive filename does not match package and version")
    evidence_dir = args.evidence_dir or args.output.parent / "command-evidence"
    _write_json(evidence_dir / "protocol-registry-record.json", registry)
    _write_json(evidence_dir / "archive-identity.json", {"sha256": before_sha, "size_bytes": before_size})
    with tempfile.TemporaryDirectory(prefix=f"gregg-registry-reverify-{args.package}-") as raw:
        root = Path(raw)
        subprocess.run(["tar", "xf", str(args.archive), "-C", str(root)], check=True)
        members = [item for item in root.iterdir() if item.is_dir()]
        if len(members) != 1:
            parser.error("archive must contain exactly one package directory")
        package_root = members[0]
        manifest = package_root / "Cargo.toml"
        manifest_text = manifest.read_text(encoding="utf-8")
        if "[patch.crates-io]" in manifest_text or re.search(r"path\s*=", manifest_text):
            parser.error("packaged dependent archive contains a path or patch dependency")
        if not re.search(r'gregg-protocol\s*=\s*\{[^}]*version\s*=\s*"1\.0\.1"', manifest_text):
            parser.error("packaged manifest does not require gregg-protocol 1.0.1")
        _write_json(evidence_dir / "normalized-manifest.json", {"sha256": hashlib.sha256(manifest_text.encode()).hexdigest(), "manifest": manifest_text})
        env = {**os.environ, "CARGO_NET_OFFLINE": "false"}
        records: list[dict[str, Any]] = []
        records.append(capture(["cargo", "generate-lockfile", "--manifest-path", str(manifest)], cwd=package_root, evidence_dir=evidence_dir, name="generate-lockfile", env=env))
        lock = package_root / "Cargo.lock"
        lock_record = parse_lockfile_protocol(lock, expected_source=args.registry_source)
        if lock_record["checksum"] != registry["version"]["cksum"]:
            parser.error("Cargo.lock protocol checksum does not match registry record")
        lock_sha, lock_size = digest(lock)
        _write_json(evidence_dir / "lockfile-identity.json", {**lock_record, "sha256": lock_sha, "size_bytes": lock_size})
        for name, command in (("metadata", ["cargo", "metadata", "--locked", "--format-version", "1"]), ("build", ["cargo", "build", "--all-features", "--locked"]), ("test", ["cargo", "test", "--all-features", "--locked"])):
            records.append(capture(command, cwd=package_root, evidence_dir=evidence_dir, name=name, env=env))
        install_root = root / "install"
        records.append(capture(["cargo", "install", "--path", ".", "--locked", "--root", str(install_root)], cwd=package_root, evidence_dir=evidence_dir, name="install", env=env))
        binary = install_root / "bin" / args.package
        records.append(capture([str(binary), "--help"], cwd=package_root, evidence_dir=evidence_dir, name="binary-help", env=env))
        records.append(capture([str(binary), "--version"], cwd=package_root, evidence_dir=evidence_dir, name="binary-version", env=env))
        records.append(capture(["cargo", "--version"], cwd=package_root, evidence_dir=evidence_dir, name="cargo-version", env=env))
        records.append(capture(["rustc", "--version", "--verbose"], cwd=package_root, evidence_dir=evidence_dir, name="rustc-version", env=env))
        records.append(capture(["uname", "-a"], cwd=package_root, evidence_dir=evidence_dir, name="host-identity", env=env))
        index = {"commands": records}
        _write_json(evidence_dir / "command-evidence-index.json", index)
        binary_sha, binary_size = digest(binary)
    after_sha, after_size = digest(args.archive)
    if (after_sha, after_size) != (before_sha, before_size):
        parser.error("Boundary-1 archive changed during verification")
    result = {
        "schema_version": 1, "package": args.package, "release_version": PROTOCOL_VERSION,
        "archive_sha256": before_sha, "archive_size_bytes": before_size,
        "protocol_registry_checksum": registry["version"]["cksum"], "lockfile_protocol_checksum": lock_record["checksum"],
        "lockfile_source": lock_record["source"], "lockfile_sha256": lock_sha, "lockfile_size_bytes": lock_size,
        "installed_binary_sha256": binary_sha, "installed_binary_size_bytes": binary_size,
        "command_evidence_index": str((evidence_dir / "command-evidence-index.json").name),
        "verification": "pass",
    }
    _write_json(args.output, result)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except VerificationError as error:
        raise SystemExit(f"FATAL: {error}")
