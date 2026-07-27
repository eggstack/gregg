#!/usr/bin/env python3
"""Re-verify an exact dependent crate archive against crates.io protocol 1.0.1."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import urllib.request
from pathlib import Path


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def digest(path: Path) -> tuple[str, int]:
    h = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            h.update(chunk)
            size += len(chunk)
    return h.hexdigest(), size


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--package", choices=["greggd", "gregg"], required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--protocol-checksum", required=True)
    args = parser.parse_args()
    if not SHA256_RE.fullmatch(args.expected_sha256) or not SHA256_RE.fullmatch(args.protocol_checksum):
        parser.error("checksums must be lowercase 64-character SHA-256 values")
    actual, size = digest(args.archive)
    if actual != args.expected_sha256:
        parser.error("Boundary-1 archive checksum mismatch")
    if not args.archive.name == f"{args.package}-1.0.1.crate":
        parser.error("archive filename does not match package and version")

    with tempfile.TemporaryDirectory(prefix=f"gregg-registry-reverify-{args.package}-") as raw:
        root = Path(raw)
        subprocess.run(["tar", "xf", str(args.archive), "-C", str(root)], check=True)
        members = [item for item in root.iterdir() if item.is_dir()]
        if len(members) != 1:
            parser.error("archive must contain exactly one package directory")
        package_root = members[0]
        manifest = package_root / "Cargo.toml"
        text = manifest.read_text(encoding="utf-8")
        if "[patch.crates-io]" in text or re.search(r"path\s*=", text):
            parser.error("packaged dependent archive contains a path or patch dependency")
        if not re.search(r'gregg-protocol\s*=\s*\{[^}]*version\s*=\s*"1\.0\.1"', text):
            parser.error("packaged manifest does not require gregg-protocol 1.0.1")
        env = {**os.environ, "CARGO_NET_OFFLINE": "false"}
        subprocess.run(["cargo", "generate-lockfile", "--manifest-path", str(manifest)], cwd=package_root, env=env, check=True)
        lock = package_root / "Cargo.lock"
        lock_text = lock.read_text(encoding="utf-8")
        if 'name = "gregg-protocol"' not in lock_text or 'version = "1.0.1"' not in lock_text or 'registry+https://github.com/rust-lang/crates.io-index' not in lock_text and 'sparse+https://index.crates.io/' not in lock_text:
            parser.error("lockfile does not prove registry gregg-protocol 1.0.1 resolution")
        subprocess.run(["cargo", "build", "--all-features", "--locked"], cwd=package_root, env=env, check=True)
        subprocess.run(["cargo", "test", "--all-features", "--locked"], cwd=package_root, env=env, check=True)
        install_root = root / "install"
        subprocess.run(["cargo", "install", "--path", ".", "--locked", "--root", str(install_root)], cwd=package_root, env=env, check=True)
        binaries = [install_root / "bin" / args.package]
        for binary in binaries:
            subprocess.run([str(binary), "--help"], check=True)
            subprocess.run([str(binary), "--version"], check=True)
        lock_sha, lock_size = digest(lock)
        result = {"package": args.package, "archive_sha256": actual, "archive_size_bytes": size,
                  "protocol_registry_checksum": args.protocol_checksum, "lockfile_sha256": lock_sha,
                  "lockfile_size_bytes": lock_size, "verification": "build-test-install-help-version"}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
