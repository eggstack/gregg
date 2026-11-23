"""Regression coverage for the pinned Eggpack representation of Gregg's release contract."""

import json
import os
import re
import subprocess
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
PROGRAMS = ("gregg", "greggd")
RELEASE_ID = "1.2.3"
RELEASE_TAG = f"v{RELEASE_ID}"
SOURCE_REVISION = "0" * 40


def _targets_from_source_table():
    targets = {}
    for raw in (ROOT / "scripts/release-targets.txt").read_text().splitlines():
        row = raw.split("#", 1)[0].split()
        if not row:
            continue
        assert len(row) <= 2, f"malformed canonical target row: {raw!r}"
        target = row[0]
        suffix = row[1] if len(row) == 2 else ""
        assert suffix in ("", ".exe"), f"unsupported executable suffix: {suffix!r}"
        assert target not in targets, f"duplicate canonical target: {target}"
        targets[target] = suffix
    return targets


def _assert_target_parity(expected, actual):
    expected = set(expected)
    actual_items = list(actual)
    actual = set(actual_items)
    duplicates = sorted({target for target in actual_items if actual_items.count(target) > 1})
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    assert not missing and not extra and not duplicates, (
        f"target mismatch: missing={missing}, extra={extra}, duplicates={duplicates}"
    )


def _assert_product_id(contract, expected):
    actual = contract["product"]["id"]
    assert actual == expected, f"contract product ID mismatch: expected {expected}, got {actual}"


def _cli():
    value = os.environ.get("EGGPACK_CLI")
    if not value:
        pytest.skip("set EGGPACK_CLI to the pinned Eggpack CLI to run contract integration tests")
    cli = Path(value)
    if not cli.is_absolute():
        cli = ROOT / cli
    assert cli.is_file(), f"EGGPACK_CLI does not name a file: {cli}"
    return str(cli)


def _run_eggpack(*args):
    return subprocess.run(
        [_cli(), *map(str, args)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def _expand(cli, contract_path, target, field):
    result = subprocess.run(
        [
            cli,
            "contract",
            "expand",
            "--contract",
            str(contract_path),
            "--release-id",
            RELEASE_ID,
            "--target",
            target,
            "--field",
            field,
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def _assert_exact_asset(actual, expected):
    assert actual == expected, f"asset name mismatch: expected {expected}, got {actual}"


def test_eggpack_inputs_match_current_release_mappings():
    target_suffixes = _targets_from_source_table()
    expected_targets = set(target_suffixes)
    assert len(expected_targets) == 5
    assert {target for target, suffix in target_suffixes.items() if suffix} == {
        "x86_64-pc-windows-msvc"
    }

    pin = (ROOT / "packaging/eggpack/revision.txt").read_text().strip()
    assert re.fullmatch(r"[0-9a-f]{40}", pin), "Eggpack pin must be one immutable 40-hex commit"

    pack = tomllib.loads((ROOT / "packaging/eggpack/pack-config.toml").read_text())
    _assert_target_parity(expected_targets, [row["target"] for row in pack["targets"]])
    by_target = {row["target"]: row for row in pack["targets"]}
    for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
        policy = by_target[target]
        assert policy["strategy"] == "cargo_zigbuild"
        assert policy["floor"] == {"kind": "glibc", "major": 2, "minor": 17}
        assert policy["toolchain"]["cargo_zigbuild"] == "0.23.4"
        assert policy["toolchain"]["zig"] == "0.14.0"
        assert policy["qualification"] == "native"

    for program in PROGRAMS:
        contract = tomllib.loads(
            (ROOT / f"packaging/eggpack/distribution-{program}.toml").read_text()
        )
        _assert_product_id(contract, program)
        _assert_target_parity(expected_targets, [row["triple"] for row in contract["targets"]])

        bindings = tomllib.loads(
            (ROOT / f"packaging/eggpack/build-bindings-{program}.toml").read_text()
        )
        _assert_target_parity(expected_targets, bindings["targets"].keys())
        for outputs in bindings["targets"].values():
            assert outputs == [
                {"selector": {"kind": "direct"}, "package": program, "binary": program}
            ]

        qualification = tomllib.loads(
            (ROOT / f"packaging/eggpack/qualification-bindings-{program}.toml").read_text()
        )
        _assert_target_parity(expected_targets, qualification["targets"].keys())
        for smoke in qualification["targets"].values():
            assert smoke["smoke"]["selector"] == {"kind": "direct"}
            assert smoke["smoke"]["argv"] == ["version"]

    rust = (ROOT / "crates/gregg-update/src/target.rs").read_text()
    rust_marker = "pub const SUPPORTED_TARGETS: &[&str] = &["
    assert rust_marker in rust, "could not find updater's supported target mapping"
    rust_target_body = rust.split(rust_marker, 1)[1].split("];", 1)[0]
    _assert_target_parity(expected_targets, re.findall(r'"([^"]+)"', rust_target_body))
    assert 'if target == "x86_64-pc-windows-msvc"' in rust
    assert 'format!("{program}-{target}.exe")' in rust

    unix_installer = (ROOT / "packaging/install.sh").read_text()
    for target in expected_targets - {"x86_64-pc-windows-msvc"}:
        assert target in unix_installer
    windows_installer = (ROOT / "packaging/install.ps1").read_text()
    assert "x86_64-pc-windows-msvc" in windows_installer

    workflow = (ROOT / ".github/workflows/release-binaries.yml").read_text()
    for target in expected_targets:
        assert target in workflow
    asset_checker = (ROOT / "scripts/release-check-assets.sh").read_text()
    assert 'TARGETS_FILE="$SCRIPT_DIR/release-targets.txt"' in asset_checker
    assert "for program in gregg greggd" in asset_checker
    assert 'asset="${program}-${target}${suffix}"' in asset_checker


def test_target_parity_rejects_missing_and_extra_targets():
    expected = set(_targets_from_source_table())
    with pytest.raises(AssertionError, match="missing="):
        _assert_target_parity(expected, expected - {"aarch64-apple-darwin"})
    with pytest.raises(AssertionError, match="extra="):
        _assert_target_parity(expected, expected | {"x86_64-unknown-linux-musl"})


def test_eggpack_expands_exact_assets_and_asset_checker_accepts_inventory(tmp_path):
    cli = _cli()
    target_suffixes = _targets_from_source_table()
    expected_files = set()

    for program in PROGRAMS:
        contract_path = ROOT / f"packaging/eggpack/distribution-{program}.toml"
        contract = tomllib.loads(contract_path.read_text())
        _assert_product_id(contract, program)
        _assert_target_parity(target_suffixes, [row["triple"] for row in contract["targets"]])
        for target, suffix in target_suffixes.items():
            assert _expand(cli, contract_path, target, "canonical-target") == target
            asset = _expand(cli, contract_path, target, "asset")
            _assert_exact_asset(asset, f"{program}-{target}{suffix}")
            sidecar = _expand(cli, contract_path, target, "sidecar")
            _assert_exact_asset(sidecar, f"{asset}.sha256")
            expected_files.update((asset, sidecar))

    for filename in expected_files:
        (tmp_path / filename).write_bytes(b"deterministic release inventory fixture")
    for filename in expected_files:
        if filename.endswith(".sha256"):
            binary_name = filename[: -len(".sha256")]
            (tmp_path / filename).write_text(f"{'0' * 64}  {binary_name}\n")

    checked = subprocess.run(
        ["bash", str(ROOT / "scripts/release-check-assets.sh"), str(tmp_path)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert checked.returncode == 0, checked.stderr
    reported = {
        line[len("OK: ") :]
        for line in checked.stdout.splitlines()
        if line.startswith("OK: ")
    }
    assert reported == expected_files
    assert "All expected assets present (20 files)" in checked.stdout


def test_product_name_mutation_is_detected_by_expansion(tmp_path):
    cli = _cli()
    source = ROOT / "packaging/eggpack/distribution-gregg.toml"
    original = source.read_text()
    mutated = original.replace('id = "gregg"', 'id = "wrong-product"', 1)
    assert mutated != original
    path = tmp_path / "wrong-product.toml"
    path.write_text(mutated)
    actual = _expand(cli, path, "x86_64-unknown-linux-gnu", "asset")
    with pytest.raises(AssertionError, match="asset name mismatch"):
        _assert_exact_asset(actual, "gregg-x86_64-unknown-linux-gnu")


def test_windows_suffix_mutation_is_detected_by_expansion(tmp_path):
    cli = _cli()
    source = ROOT / "packaging/eggpack/distribution-greggd.toml"
    original = source.read_text()
    mutated = original.replace(
        'asset = "{product}-{target}.exe"', 'asset = "{product}-{target}"', 1
    )
    assert mutated != original
    path = tmp_path / "wrong-windows-suffix.toml"
    path.write_text(mutated)
    actual = _expand(cli, path, "x86_64-pc-windows-msvc", "asset")
    with pytest.raises(AssertionError, match="asset name mismatch"):
        _assert_exact_asset(actual, "greggd-x86_64-pc-windows-msvc.exe")


@pytest.mark.parametrize("invalid_tag", ["v01.2.3", "v1.2.3-extra", "1.2.3"])
def test_eggpack_rejects_invalid_release_identity(invalid_tag):
    program = "gregg"
    targets = ",".join(_targets_from_source_table())
    result = _run_eggpack(
        "check",
        "--contract",
        ROOT / f"packaging/eggpack/distribution-{program}.toml",
        "--pack-config",
        ROOT / "packaging/eggpack/pack-config.toml",
        "--build-bindings",
        ROOT / f"packaging/eggpack/build-bindings-{program}.toml",
        "--qualification-bindings",
        ROOT / f"packaging/eggpack/qualification-bindings-{program}.toml",
        "--selected",
        targets,
        "--release-id",
        invalid_tag,
        "--identity-mode",
        "v_prefixed_stable_semver",
        "--source-revision",
        SOURCE_REVISION,
    )
    assert result.returncode != 0
    assert "release plan resolution failed:" in result.stderr


def test_eggpack_check_and_plan_preserve_release_identity_and_floor_status(tmp_path):
    cli = _cli()
    selected = ",".join(_targets_from_source_table())
    for program in PROGRAMS:
        common = [
            "--contract",
            ROOT / f"packaging/eggpack/distribution-{program}.toml",
            "--pack-config",
            ROOT / "packaging/eggpack/pack-config.toml",
            "--build-bindings",
            ROOT / f"packaging/eggpack/build-bindings-{program}.toml",
            "--qualification-bindings",
            ROOT / f"packaging/eggpack/qualification-bindings-{program}.toml",
            "--selected",
            selected,
            "--release-id",
            RELEASE_TAG,
            "--identity-mode",
            "v_prefixed_stable_semver",
            "--source-revision",
            SOURCE_REVISION,
        ]
        checked = subprocess.run(
            [cli, "check", *map(str, common)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        assert checked.returncode == 0, checked.stderr
        assert checked.stdout.strip() == "check: ok"

        plan_path = tmp_path / f"{program}-release-plan.json"
        planned = subprocess.run(
            [cli, "plan", *map(str, common), "--output", str(plan_path)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        assert planned.returncode == 0, planned.stderr
        assert planned.stdout.strip() == "plan: ok"
        plan = json.loads(plan_path.read_text())
        assert plan["release_id"] == RELEASE_ID
        assert plan["source_revision"] == SOURCE_REVISION
        targets = {row["target"]: row for row in plan["targets"]}
        _assert_target_parity(_targets_from_source_table(), targets)
        for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
            assert targets[target]["floor_status"] == "toolchain_bound"
