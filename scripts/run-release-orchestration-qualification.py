#!/usr/bin/env python3
"""Run the nonpublishing release-control qualification harness.

This entry point is intentionally side-effect free with respect to Git: it
executes repository-owned validation paths and writes a replayable command
record.  Network-backed artifact qualification remains the responsibility of
the hosted release workflows, where GitHub assigns immutable artifact IDs.
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import platform
import subprocess
import sys
from pathlib import Path


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
    started = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    commands = []
    for command in (["git", "rev-parse", "HEAD"], [sys.executable, "scripts/validate-release-workflow.py"]):
        result = subprocess.run(command, text=True, capture_output=True, check=False)
        commands.append({"argv": command, "exit_status": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
        if result.returncode:
            parser.error(f"qualification command failed: {' '.join(command)}")
    commands_path = args.evidence_dir / "qualification-commands.json"
    commands_path.write_text(json.dumps(commands, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    finished = dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")
    files = []
    for path in sorted(args.evidence_dir.rglob("*")):
        if path.is_file() and path.name != "qualification-summary.json":
            raw = path.read_bytes()
            files.append({"path": str(path.relative_to(args.evidence_dir)), "sha256": hashlib.sha256(raw).hexdigest(), "size_bytes": len(raw), "role": "qualification-command-evidence"})
    summary = {"schema_version": 1, "qualification": "nonpublishing-release-control", "candidate_sha": args.candidate_sha, "release_version": args.release_version, "started_at": started, "completed_at": finished, "runner": {"system": platform.system(), "machine": platform.machine(), "python": platform.python_version()}, "files": files, "verdict": "pass"}
    (args.evidence_dir / "qualification-summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
