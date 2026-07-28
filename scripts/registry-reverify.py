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
import subprocess
import tempfile
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

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
REQUIRED_COMMANDS = (
    "generate-lockfile", "metadata", "build", "test", "install",
    "binary-help", "binary-version", "cargo-version", "rustc-version",
    "host-identity",
)


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


def parse_lockfile_protocol(
    lockfile: Path, *, expected_source: str | None = None,
    valid_sources: set[str] | None = None,
) -> dict[str, str]:
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
    if valid_sources is not None:
        if source not in valid_sources:
            fail(f"Cargo.lock protocol source {source!r} is not in expected sources {valid_sources!r}")
    elif expected_source is not None and source != expected_source:
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


def _identity(path: Path, root: Path) -> dict[str, Any]:
    sha256, size = digest(path)
    return {
        "path": path.relative_to(root).as_posix(),
        "sha256": sha256,
        "size_bytes": size,
    }


def _resolve_index_file(root: Path, value: Any, label: str) -> Path:
    if not isinstance(value, dict):
        fail(f"command index {label} identity is missing")
    relative = value.get("path")
    if not isinstance(relative, str):
        fail(f"command index {label} path is missing")
    relpath = Path(relative)
    if relpath.is_absolute() or ".." in relpath.parts:
        fail(f"command index {label} path escapes index root")
    path = root / relpath
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        fail(f"command index {label} file is missing: {error}")
    if root != resolved.parent and root not in resolved.parents:
        fail(f"command index {label} path escapes index root")
    if path.is_symlink() or not resolved.is_file():
        fail(f"command index {label} must be a regular non-symlink file")
    sha256, size = digest(resolved)
    if value.get("sha256") != sha256 or value.get("size_bytes") != size:
        fail(f"command index {label} digest/size mismatch")
    return resolved


def validate_command_index(
    index_path: Path, *, expected_package: str | None = None,
    expected_version: str = PROTOCOL_VERSION,
) -> dict[str, Any]:
    """Independently validate a Boundary-2 command evidence index."""
    try:
        value = json.loads(index_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read command evidence index: {error}")
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        fail("command evidence index schema_version must be 1")
    if value.get("package") not in {"greggd", "gregg"}:
        fail("command evidence index package is invalid")
    if expected_package and value.get("package") != expected_package:
        fail("command evidence index package does not match expected package")
    if value.get("release_version") != expected_version:
        fail("command evidence index release version is invalid")
    if value.get("verdict") != "pass":
        fail("command evidence index verdict must be pass")

    root = index_path.parent.resolve()
    archive = _resolve_index_file(root, value.get("archive"), "archive")
    registry_path = _resolve_index_file(root, value.get("registry_record"), "registry_record")
    lockfile_path = _resolve_index_file(root, value.get("lockfile"), "lockfile")
    _resolve_index_file(root, value.get("normalized_manifest"), "normalized_manifest")
    registry = validate_registry_record(json.loads(registry_path.read_text(encoding="utf-8")))
    lock_record = parse_lockfile_protocol(
        lockfile_path, expected_source=value["lockfile"].get("source")
    )
    checksum = registry["version"]["cksum"]
    if value["lockfile"].get("protocol_checksum") != checksum or lock_record["checksum"] != checksum:
        fail("command evidence index registry and lockfile checksums differ")
    archive_sha, archive_size = digest(archive)
    before = value.get("archive_before")
    after = value.get("archive_after")
    expected_identity = {"sha256": archive_sha, "size_bytes": archive_size}
    if before != expected_identity or after != expected_identity or value.get("archive_unchanged") is not True:
        fail("command evidence index archive before/after identity is invalid")

    commands = value.get("commands")
    if not isinstance(commands, list):
        fail("command evidence index commands must be an array")
    names = [item.get("name") for item in commands if isinstance(item, dict)]
    if sorted(names) != sorted(REQUIRED_COMMANDS) or len(names) != len(set(names)):
        fail("command evidence index required commands are missing, extra, or duplicated")
    indexed_transcripts: set[str] = set()
    for command in commands:
        name = command["name"]
        record_path = _resolve_index_file(root, command.get("record"), f"{name} record")
        stdout_path = _resolve_index_file(root, command.get("stdout"), f"{name} stdout")
        stderr_path = _resolve_index_file(root, command.get("stderr"), f"{name} stderr")
        indexed_transcripts.update((record_path.name, stdout_path.name, stderr_path.name))
        record = json.loads(record_path.read_text(encoding="utf-8"))
        if record.get("exit_status") != 0 or command.get("exit_status") != 0:
            fail(f"command evidence index {name} exit status is nonzero")
        for stream, path in (("stdout", stdout_path), ("stderr", stderr_path)):
            identity = record.get(stream)
            actual_sha, actual_size = digest(path)
            if not isinstance(identity, dict) or identity.get("sha256") != actual_sha or identity.get("size_bytes") != actual_size:
                fail(f"command evidence record {name} {stream} identity mismatch")
    expected_transcripts = {
        f"{name}{suffix}" for name in REQUIRED_COMMANDS
        for suffix in (".json", ".stdout", ".stderr")
    }
    if indexed_transcripts != expected_transcripts:
        fail("command evidence index does not cover every required transcript")
    binary = value.get("installed_binary")
    if not isinstance(binary, dict) or not SHA256_RE.fullmatch(str(binary.get("sha256", ""))) or not isinstance(binary.get("size_bytes"), int) or binary["size_bytes"] <= 0:
        fail("command evidence index installed binary identity is invalid")
    return value


def write_candidate_artifacts(
    *, index_path: Path, summary_path: Path, artifact_root: Path, output: Path,
) -> None:
    """Derive candidate artifact declarations only from validated local files."""
    index = validate_command_index(index_path)
    root = artifact_root.resolve()

    def declaration(path: Path, role: str, media_type: str | None = None) -> dict[str, Any]:
        resolved = path.resolve(strict=True)
        if root != resolved.parent and root not in resolved.parents:
            fail(f"candidate artifact {role} escapes artifact root")
        sha256, size = digest(resolved)
        value = {
            "name": resolved.relative_to(root).as_posix(),
            "path": resolved.relative_to(root).as_posix(),
            "role": role, "sha256": sha256, "size_bytes": size,
        }
        if media_type:
            value["media_type"] = media_type
        return value

    command_root = index_path.parent
    commands = {item["name"]: item for item in index["commands"]}
    artifacts = [
        declaration(summary_path, "registry-reverify-summary", "application/json"),
        declaration(command_root / index["registry_record"]["path"], "protocol-registry-record", "application/json"),
        declaration(index_path, "command-evidence-index", "application/json"),
        declaration(command_root / commands["build"]["stdout"]["path"], "build-transcript", "text/plain"),
        declaration(command_root / commands["test"]["stdout"]["path"], "test-transcript", "text/plain"),
        declaration(command_root / commands["install"]["stdout"]["path"], "install-transcript", "text/plain"),
        declaration(command_root / commands["cargo-version"]["stdout"]["path"], "command-versions", "text/plain"),
        declaration(command_root / index["normalized_manifest"]["path"], "normalized-manifest", "application/json"),
        declaration(command_root / index["lockfile"]["path"], "verification-lockfile", "text/x-toml"),
        declaration(command_root / index["archive"]["path"], "package-archive", "application/gzip"),
        declaration(command_root / commands["metadata"]["stdout"]["path"], "resolution-proof", "application/json"),
    ]
    _write_json(output, artifacts)


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
    if len(os.sys.argv) > 1 and os.sys.argv[1] == "validate-index":
        parser = argparse.ArgumentParser(description="Validate a Boundary-2 command evidence index")
        parser.add_argument("validate-index")
        parser.add_argument("--index", required=True, type=Path)
        parser.add_argument("--package", choices=["greggd", "gregg"])
        args = parser.parse_args()
        validate_command_index(args.index, expected_package=args.package)
        print(f"validated command evidence index: {args.index}")
        return 0
    if len(os.sys.argv) > 1 and os.sys.argv[1] == "candidate-artifacts":
        parser = argparse.ArgumentParser(description="Derive candidate artifacts from a validated command index")
        parser.add_argument("candidate-artifacts")
        parser.add_argument("--index", required=True, type=Path)
        parser.add_argument("--summary", required=True, type=Path)
        parser.add_argument("--artifact-root", required=True, type=Path)
        parser.add_argument("--output", required=True, type=Path)
        args = parser.parse_args()
        write_candidate_artifacts(
            index_path=args.index, summary_path=args.summary,
            artifact_root=args.artifact_root, output=args.output,
        )
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--package", choices=["greggd", "gregg"], required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--protocol-checksum", required=True)
    parser.add_argument("--registry-record", type=Path)
    parser.add_argument("--registry-source", default="sparse+https://index.crates.io/")
    parser.add_argument("--evidence-dir", type=Path)
    parser.add_argument("--qualification-local-registry", action="store_true",
                        help="Permit one exact loopback sparse registry (qualification only)")
    parser.add_argument("--cargo-home", type=Path,
                        help="Isolated Cargo home containing qualification source replacement")
    args = parser.parse_args()
    if not SHA256_RE.fullmatch(args.expected_sha256) or not SHA256_RE.fullmatch(args.protocol_checksum):
        parser.error("checksums must be lowercase 64-character SHA-256 values")
    if args.qualification_local_registry:
        parsed = urlparse(args.registry_source.removeprefix("sparse+"))
        if not args.registry_source.startswith("sparse+") or parsed.scheme not in {"http", "https"}:
            parser.error("qualification registry must be an HTTP(S) sparse registry")
        if parsed.username or parsed.password or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}:
            parser.error("qualification registry must be loopback-only with no userinfo")
        if not args.cargo_home:
            parser.error("--cargo-home is required with --qualification-local-registry")
    elif args.registry_source not in CRATES_IO_SOURCES:
        parser.error("production registry source is not an approved crates.io source")
    elif args.cargo_home:
        parser.error("--cargo-home is qualification-only")
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
    retained_archive = evidence_dir / args.archive.name
    retained_archive.parent.mkdir(parents=True, exist_ok=True)
    retained_archive.write_bytes(args.archive.read_bytes())
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
        if args.cargo_home:
            env["CARGO_HOME"] = str(args.cargo_home.resolve())
        records: list[dict[str, Any]] = []
        lock = package_root / "Cargo.lock"
        records.append(capture(["cargo", "generate-lockfile", "--manifest-path", str(manifest)], cwd=package_root, evidence_dir=evidence_dir, name="generate-lockfile", env=env))
        lock_record = parse_lockfile_protocol(lock, expected_source=args.registry_source)
        if lock_record["checksum"] != registry["version"]["cksum"]:
            parser.error("Cargo.lock protocol checksum does not match registry record")
        lock_sha, lock_size = digest(lock)
        lock_identity_path = evidence_dir / "lockfile-identity.json"
        _write_json(lock_identity_path, {**lock_record, "sha256": lock_sha, "size_bytes": lock_size})
        retained_lockfile = evidence_dir / "verification-Cargo.lock"
        retained_lockfile.write_bytes(lock.read_bytes())
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
        binary_sha, binary_size = digest(binary)
    after_sha, after_size = digest(args.archive)
    if (after_sha, after_size) != (before_sha, before_size):
        parser.error("Boundary-1 archive changed during verification")
    index = {
        "schema_version": 1, "package": args.package, "release_version": PROTOCOL_VERSION,
        "archive": _identity(retained_archive, evidence_dir),
        "registry_record": _identity(evidence_dir / "protocol-registry-record.json", evidence_dir),
        "lockfile": {
            **_identity(retained_lockfile, evidence_dir),
            "protocol_checksum": lock_record["checksum"],
            "source": lock_record["source"],
        },
        "normalized_manifest": _identity(evidence_dir / "normalized-manifest.json", evidence_dir),
        "archive_before": {"sha256": before_sha, "size_bytes": before_size},
        "archive_after": {"sha256": after_sha, "size_bytes": after_size},
        "archive_unchanged": True,
        "commands": [
            {
                "name": name,
                "record": _identity(evidence_dir / f"{name}.json", evidence_dir),
                "stdout": _identity(evidence_dir / f"{name}.stdout", evidence_dir),
                "stderr": _identity(evidence_dir / f"{name}.stderr", evidence_dir),
                "exit_status": record["exit_status"],
            }
            for name, record in zip(REQUIRED_COMMANDS, records, strict=True)
        ],
        "installed_binary": {"sha256": binary_sha, "size_bytes": binary_size},
        "verdict": "pass",
    }
    index_path = evidence_dir / "command-evidence-index.json"
    _write_json(index_path, index)
    validate_command_index(index_path, expected_package=args.package)
    result = {
        "schema_version": 1, "package": args.package, "release_version": PROTOCOL_VERSION,
        "archive_sha256": before_sha, "archive_size_bytes": before_size,
        "protocol_registry_checksum": registry["version"]["cksum"], "lockfile_protocol_checksum": lock_record["checksum"],
        "checksum_match": True,
        "lockfile_source": lock_record["source"], "lockfile_sha256": lock_sha, "lockfile_size_bytes": lock_size,
        "installed_binary_sha256": binary_sha, "installed_binary_size_bytes": binary_size,
        "command_evidence_index": str(index_path.name),
        "command_evidence_index_sha256": digest(index_path)[0],
        "command_evidence_index_size_bytes": digest(index_path)[1],
        "archive_before": {"sha256": before_sha, "size_bytes": before_size},
        "archive_after": {"sha256": after_sha, "size_bytes": after_size},
        "archive_unchanged": True,
        "verification": "pass",
    }
    _write_json(args.output, result)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except VerificationError as error:
        raise SystemExit(f"FATAL: {error}")
