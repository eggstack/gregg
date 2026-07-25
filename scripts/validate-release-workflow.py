#!/usr/bin/env python3
"""Static checks for the release workflow's repository-owned invariants."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "release-candidate.yml"


def fail(message: str) -> None:
    print(f"FATAL: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> int:
    text = WORKFLOW.read_text(encoding="utf-8")
    if "continue-on-error" in text:
        fail("release workflow must not continue on gate errors")
    if "GREGG_SYSTEMD_BINARY" in text or "GREGG_LAUNCHD_BINARY" in text or "GREGG_INSTALLED_GREGGD" in text:
        fail("protected jobs must not consume arbitrary binary path inputs")
    if "cargo install --path \"${package_dir}\" --locked" not in text:
        fail("unpacked binary installation is not locked")
    for line in text.splitlines():
        if "install-verified-package.sh" in line and "--candidate-sha" not in line:
            fail("package installation is not bound to the resolved candidate SHA")
        if "verify-package-provenance.sh" in line and "--candidate-sha" not in line:
            fail("package provenance verification is not bound to the resolved candidate SHA")
    if text.find("actions/checkout@v4", text.find("postpublish-verify")) == -1:
        fail("postpublish-verify has no checkout")
    postpublish = text[text.find("postpublish-verify") :]
    if postpublish.find("actions/checkout@v4") > postpublish.find("scripts/verify-installed-daemon.sh"):
        fail("postpublish repository scripts are used before checkout")
    for script in re.findall(r"(?:bash |python3 |\./|scripts/)(scripts/[A-Za-z0-9_./-]+\.(?:sh|py))", text):
        if not (ROOT / script).is_file():
            fail(f"workflow references missing repository script: {script}")
    requirements = (ROOT / "plans/evidence/release-requirements.json").read_text(encoding="utf-8")
    for stage in ("native-linux-arm64", "native-macos-intel", "soak-linux-24h", "postpublish-verify"):
        if stage not in requirements:
            fail(f"mandatory stage missing from requirements: {stage}")
    for manifest in (ROOT / "crates/greggd/Cargo.toml", ROOT / "crates/gregg/Cargo.toml"):
        if manifest.read_text(encoding="utf-8").count('gregg-protocol = { version = "1.0.1"') != 2:
            fail(f"protocol dependency is not aligned in {manifest}")
    print("release workflow static validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
