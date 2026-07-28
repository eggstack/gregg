#!/usr/bin/env python3
"""Decode and validate the operator-authorized 1.0.0 disposition input."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
from pathlib import Path

CRATES = {"gregg-protocol", "greggd", "gregg"}
MAX_BYTES = 64 * 1024


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base64", required=True)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--identity-output", required=True, type=Path)
    parser.add_argument("--workflow-run-id", required=True)
    parser.add_argument("--workflow-run-attempt", required=True)
    parser.add_argument("--actor", required=True)
    args = parser.parse_args()
    try:
        raw = base64.b64decode(args.base64.encode("ascii"), validate=True)
        if not raw or len(raw) > MAX_BYTES or b"\0" in raw:
            raise ValueError("empty, oversized, or NUL-containing decision")
        value = json.loads(raw.decode("utf-8"))
        if not isinstance(value, dict) or value.get("schema_version") != 1:
            raise ValueError("decision schema_version must be 1")
        if value.get("historical_version") != "1.0.0" or value.get("candidate_sha") != args.candidate_sha:
            raise ValueError("decision historical version or candidate SHA is incorrect")
        decisions = value.get("decisions")
        if not isinstance(decisions, dict) or set(decisions) != CRATES:
            raise ValueError("decision must contain exactly the three release crates")
        for crate, decision in decisions.items():
            if not isinstance(decision, dict) or set(decision) != {"decision", "rationale"}:
                raise ValueError(f"decision for {crate} has unsupported or missing fields")
            if decision["decision"] not in {"retain", "yank"} or not isinstance(decision["rationale"], str) or not decision["rationale"].strip():
                raise ValueError(f"decision for {crate} needs a valid decision and rationale")
        if not args.actor.strip():
            raise ValueError("decision actor must be nonempty")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(raw)
        identity = {"schema_version": 1, "source": "workflow-dispatch-base64", "sha256": hashlib.sha256(raw).hexdigest(), "size_bytes": len(raw), "workflow_run_id": args.workflow_run_id, "workflow_run_attempt": args.workflow_run_attempt, "actor": args.actor, "candidate_sha": args.candidate_sha}
        args.identity_output.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(json.dumps(identity, sort_keys=True))
        return 0
    except (ValueError, UnicodeDecodeError, json.JSONDecodeError) as error:
        print(f"FATAL: invalid disposition input: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
