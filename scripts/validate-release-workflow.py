#!/usr/bin/env python3
"""Static checks for the release workflow's repository-owned invariants."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "release-candidate.yml"
FINALIZE = ROOT / ".github" / "workflows" / "release-finalize.yml"


def fail(message: str) -> None:
    print(f"FATAL: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> int:
    text = WORKFLOW.read_text(encoding="utf-8")
    finalize_text = FINALIZE.read_text(encoding="utf-8")

    # No continue-on-error in qualifying paths.
    if "continue-on-error" in text:
        fail("release workflow must not continue on gate errors")

    # No arbitrary binary path inputs.
    if "GREGG_SYSTEMD_BINARY" in text or "GREGG_LAUNCHD_BINARY" in text or "GREGG_INSTALLED_GREGGD" in text:
        fail("protected jobs must not consume arbitrary binary path inputs")

    # Binary installation is locked.
    if 'cargo install --path "${package_dir}" --locked' not in text:
        fail("unpacked binary installation is not locked")

    # Package installation is bound to candidate SHA and lockfile.
    for line in text.splitlines():
        if "install-verified-package.sh" in line and "--candidate-sha" not in line:
            fail("package installation is not bound to the resolved candidate SHA")
        if "install-verified-package.sh" in line and "--lockfile" not in line:
            fail("package installation does not require the verified lockfile")
        if "verify-package-provenance.sh" in line and "--candidate-sha" not in line:
            fail("package provenance verification is not bound to the resolved candidate SHA")

    # Postpublish-verify has checkout before scripts.
    if text.find("actions/checkout@v4", text.find("postpublish-verify")) == -1:
        fail("postpublish-verify has no checkout")
    postpublish = text[text.find("postpublish-verify"):]
    if postpublish.find("actions/checkout@v4") > postpublish.find("scripts/verify-installed-daemon.sh"):
        fail("postpublish repository scripts are used before checkout")

    # All referenced scripts exist.
    for script in re.findall(r"(?:bash |python3 |\./|scripts/)(scripts/[A-Za-z0-9_./-]+\.(?:sh|py))", text):
        if not (ROOT / script).is_file():
            fail(f"workflow references missing repository script: {script}")
    for script in re.findall(r"(?:bash |python3 |\./|scripts/)(scripts/[A-Za-z0-9_./-]+\.(?:sh|py))", finalize_text):
        if not (ROOT / script).is_file():
            fail(f"finalize workflow references missing repository script: {script}")

    # Required stages exist in requirements.
    requirements = (ROOT / "plans/evidence/release-requirements.json").read_text(encoding="utf-8")
    for stage in ("native-source-linux-arm64", "native-source-macos-intel", "native-package-linux-x86-64", "native-package-macos-arm64", "soak-linux-24h", "mixed-fleet-sustained"):
        if stage not in requirements:
            fail(f"mandatory stage missing from requirements: {stage}")

    # Protocol dependency alignment.
    for manifest in (ROOT / "crates/greggd/Cargo.toml", ROOT / "crates/gregg/Cargo.toml"):
        if manifest.read_text(encoding="utf-8").count('gregg-protocol = { version = "1.0.1"') != 2:
            fail(f"protocol dependency is not aligned in {manifest}")

    # Protected cleanup has no masked failure.
    if "|| true" in text:
        for i, line in enumerate(text.splitlines(), 1):
            if "|| true" in line and "cleanup" in line.lower():
                fail(f"release workflow masks cleanup failure at line {i}: {line.strip()}")

    # Package-native jobs are reachable (not gated by mutually exclusive binary-prepublish).
    if "needs.binary-prepublish.result" in text and "native-package-evidence" in text:
        native_section = text[text.find("native-package-evidence"):]
        if "needs.binary-prepublish.result" in native_section[:2000]:
            fail("native-package-evidence still depends on binary-prepublish result")

    # Mixed-fleet-sustained has a producing job.
    if "mixed-fleet-sustained" not in text:
        fail("mixed-fleet-sustained stage has no producing job in release-candidate workflow")

    # Finalize uses immutable checkout (candidate SHA).
    if 'ref: ${{ inputs.candidate_sha }}' not in finalize_text:
        fail("release-finalize.yml does not checkout immutable candidate SHA")

    # Finalize uses --mode.
    if '--mode' not in finalize_text:
        fail("release-finalize.yml does not pass --mode to aggregation")

    print("release workflow static validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
