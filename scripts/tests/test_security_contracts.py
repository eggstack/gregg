#!/usr/bin/env python3
"""Static contract tests for the packaging and CI supply-chain surface.

Four properties are stated on the packaging surface and cannot be seen by the
Rust gates:

* every generated default configuration binds loopback, so a fresh install does
  not expose the unauthenticated API on every interface (Plan 187);
* the installer's EXIT trap is a fixed command over a path list, so a
  ``TMPDIR`` carrying shell syntax stays data instead of becoming source
  (Plan 190);
* every HTTPS ``curl`` invocation pins the redirect hop as well as the
  initial protocol (Plan 190);
* every workflow action is pinned to an immutable 40-hex commit SHA (Plan 190).

The checks are intentionally textual, and they read the shipped files (never
this test's own source), so a reverted construct fails instead of silently
passing.
"""

from __future__ import annotations

import re
from pathlib import Path

# ``scripts/tests/<this file>`` -> repository root. Resolved from the file so
# pytest works from any working directory.
REPO_ROOT = Path(__file__).resolve().parents[2]

# Files that fetch over HTTPS. Each ``--proto`` pin must be paired with a
# ``--proto-redir`` pin, otherwise a redirect may downgrade to plain HTTP.
PROTO_PINNED_FILES = (
    "crates/gregg-update/src/exec.rs",
    "packaging/install.sh",
    "scripts/release-install-zig.sh",
    "scripts/release-preflight.sh",
)

# Rust passes the pin as two argv entries (``"--proto", "=https"``) while shell
# passes one quoted pair (``--proto '=https'``), so the pin is counted by flag
# name. The negative lookahead keeps ``--proto-redir`` occurrences out of the
# initial-protocol count.
INITIAL_PROTO_PIN = re.compile(r"--proto(?!-redir)")
REDIRECT_PROTO_PIN = re.compile(r"--proto-redir")

# ``uses: owner/repo@ref`` (optionally ``- uses: ...``); the optional trailing
# ``# tag`` comment is not part of the value.
USES_VALUE = re.compile(r"^\s*(?:-\s*)?uses:\s*(\S+)", re.MULTILINE)
PINNED_COMMIT = re.compile(r"@[0-9a-f]{40}$")

# Installers that write a default daemon configuration. A wildcard default puts
# the unauthenticated API on every interface without the operator choosing it, so
# loopback is the contract and LAN exposure is an explicit edit (Plan 187).
INSTALLERS_WITH_DEFAULT_CONFIG = (
    "packaging/install-linux.sh",
    "packaging/install-macos.sh",
    "packaging/install.ps1",
    "packaging/install-windows.ps1",
)
LOOPBACK_HOST = re.compile(r'host\s*=\s*"127\.0\.0\.1"')
WILDCARD_HOST = re.compile(r'host\s*=\s*"0\.0\.0\.0"')

WORKFLOWS_DIR = REPO_ROOT / ".github" / "workflows"


def read_repo_file(relative_path: str) -> str:
    """Return the text of a repository-relative file."""
    return (REPO_ROOT / relative_path).read_text(encoding="utf-8")


def test_installer_exit_trap_is_a_fixed_command() -> None:
    """The EXIT trap names a function; no path is interpolated into its body."""
    text = read_repo_file("packaging/install.sh")

    assert "trap cleanup_paths EXIT" in text, (
        "packaging/install.sh must register `trap cleanup_paths EXIT`: the trap "
        "body has to be a fixed command that reads registered paths as data"
    )
    assert 'trap "' not in text, (
        "packaging/install.sh must not define a double-quoted trap: the body "
        "would be re-parsed at exit, so a TMPDIR containing `$(...)` would run "
        "as a command"
    )


def test_https_pins_cover_redirects() -> None:
    """Every initial-protocol pin is paired with a redirect-protocol pin."""
    for relative_path in PROTO_PINNED_FILES:
        text = read_repo_file(relative_path)
        initial = len(INITIAL_PROTO_PIN.findall(text))
        redirects = len(REDIRECT_PROTO_PIN.findall(text))

        assert initial >= 1, f"{relative_path} has no `--proto` HTTPS pin"
        assert redirects >= 1, f"{relative_path} has no `--proto-redir` pin"
        assert initial == redirects, (
            f"{relative_path}: {initial} `--proto` pin(s) but {redirects} "
            "`--proto-redir` pin(s); every HTTPS request must also forbid a "
            "non-HTTPS redirect"
        )


def test_workflow_actions_are_pinned_to_commit_shas() -> None:
    """Every `uses:` reference names a 40-hex commit, not a mutable tag."""
    workflows = sorted(WORKFLOWS_DIR.glob("*.yml"))
    assert workflows, f"no workflow files found under {WORKFLOWS_DIR}"

    unpinned = []
    for workflow in workflows:
        for match in USES_VALUE.finditer(workflow.read_text(encoding="utf-8")):
            value = match.group(1)
            if not PINNED_COMMIT.search(value):
                unpinned.append(f"{workflow.name}: {value}")

    assert not unpinned, (
        "workflow actions must be pinned to a 40-hex commit SHA with a "
        "trailing `# <tag>` comment:\n" + "\n".join(unpinned)
    )


def test_installer_default_binds_loopback() -> None:
    """A fresh install must not expose the unauthenticated API on every interface."""
    for relative_path in INSTALLERS_WITH_DEFAULT_CONFIG:
        text = read_repo_file(relative_path)
        assert LOOPBACK_HOST.search(text), (
            f"{relative_path} does not generate `host = \"127.0.0.1\"`; a "
            "wildcard default exposes the unauthenticated API on every interface"
        )
        assert not WILDCARD_HOST.search(text), (
            f"{relative_path} still generates a wildcard bind; LAN exposure must "
            "be an explicit operator edit, not a default"
        )
