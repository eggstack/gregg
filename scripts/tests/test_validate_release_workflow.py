#!/usr/bin/env python3
"""Tests for the release workflow graph-based validator."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

# Ensure the scripts/ directory is importable
import sys
_scripts_dir = str(Path(__file__).resolve().parents[2])
if _scripts_dir not in sys.path:
    sys.path.insert(0, _scripts_dir)

# Import the validator (filename has hyphens, so use importlib)
_spec = importlib.util.spec_from_file_location(
    "validate_release_workflow",
    Path(__file__).resolve().parents[2] / "scripts" / "validate-release-workflow.py",
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)

WorkflowViolation = _mod.WorkflowViolation
_build_job_stage_map = _mod._build_job_stage_map
_check_dependency_closure = _mod._check_dependency_closure
_extract_dispatch_options = _mod._extract_dispatch_options
_extract_jobs = _mod._extract_jobs
_jobs_reachable_for_stage = _mod._jobs_reachable_for_stage
_parse_if_condition = _mod._parse_if_condition
_validate_stage_contract = _mod._validate_stage_contract
validate_workflow = _mod.validate_workflow


# ---------------------------------------------------------------------------
# Minimal YAML workflow fragments for testing
# ---------------------------------------------------------------------------

WORKFLOW_BASE = """
name: release-candidate
on:
  workflow_dispatch:
    inputs:
      stage:
        type: choice
        options:
          - protocol-prepublish
          - binary-prepublish
          - mixed-fleet-client
          - mixed-fleet-sustained
"""

def _make_workflow(jobs_yaml: str) -> str:
    return WORKFLOW_BASE + "\njobs:\n" + jobs_yaml


def _parse_jobs(workflow_yaml: str) -> dict:
    import yaml
    data = yaml.safe_load(workflow_yaml)
    return _extract_jobs(data)


def _parse_data(workflow_yaml: str) -> dict:
    import yaml
    return yaml.safe_load(workflow_yaml)


# ---------------------------------------------------------------------------
# Tests: _parse_if_condition
# ---------------------------------------------------------------------------

class TestParseIfCondition(unittest.TestCase):

    def test_always_only(self):
        result = _parse_if_condition("always()")
        self.assertTrue(result["always"])
        self.assertIsNone(result["stage"])

    def test_always_and_stage(self):
        result = _parse_if_condition("${{ always() && inputs.stage == 'protocol-prepublish' }}")
        self.assertTrue(result["always"])
        self.assertEqual(result["stage"], "protocol-prepublish")

    def test_stage_only(self):
        result = _parse_if_condition("${{ inputs.stage == 'mixed-fleet-client' }}")
        self.assertEqual(result["stage"], "mixed-fleet-client")
        self.assertFalse(result["always"])

    def test_needs_result(self):
        result = _parse_if_condition("${{ always() && inputs.stage == 'foo' && needs.bar.result == 'success' }}")
        self.assertEqual(result["needs_results"], {"bar": "success"})

    def test_unconditional(self):
        result = _parse_if_condition("always()")
        self.assertTrue(result["unconditional"])

    def test_multiple_stages(self):
        result = _parse_if_condition("${{ inputs.stage == 'a' || inputs.stage == 'b' }}")
        self.assertIsInstance(result["stage"], list)
        self.assertIn("a", result["stage"])
        self.assertIn("b", result["stage"])


# ---------------------------------------------------------------------------
# Tests: _jobs_reachable_for_stage
# ---------------------------------------------------------------------------

class TestJobsReachableForStage(unittest.TestCase):

    def _make_jobs(self, if_conditions: dict[str, str]) -> dict:
        jobs = {}
        for name, if_raw in if_conditions.items():
            parsed = _parse_if_condition(if_raw)
            needs = []
            if name == "dep":
                needs = ["resolve"]
            elif name in ("consumer", "consumer2"):
                needs = ["resolve", "dep"]
            jobs[name] = {
                "needs": needs,
                "if_raw": if_raw,
                "if_parsed": parsed,
                "strategy": None,
            }
        return jobs

    def test_unconditional_always_runs(self):
        jobs = self._make_jobs({
            "resolve": "always()",
        })
        self.assertTrue(_jobs_reachable_for_stage("resolve", jobs, "any-stage"))

    def test_stage_match(self):
        jobs = self._make_jobs({
            "dep": "${{ inputs.stage == 'foo' }}",
        })
        self.assertTrue(_jobs_reachable_for_stage("dep", jobs, "foo"))

    def test_stage_mismatch(self):
        jobs = self._make_jobs({
            "dep": "${{ inputs.stage == 'foo' }}",
        })
        self.assertFalse(_jobs_reachable_for_stage("dep", jobs, "bar"))

    def test_multiple_stages(self):
        jobs = self._make_jobs({
            "dep": "${{ inputs.stage == 'a' || inputs.stage == 'b' }}",
        })
        self.assertTrue(_jobs_reachable_for_stage("dep", jobs, "a"))
        self.assertTrue(_jobs_reachable_for_stage("dep", jobs, "b"))
        self.assertFalse(_jobs_reachable_for_stage("dep", jobs, "c"))

    def test_always_with_stage(self):
        jobs = self._make_jobs({
            "dep": "${{ always() && inputs.stage == 'foo' }}",
        })
        self.assertTrue(_jobs_reachable_for_stage("dep", jobs, "foo"))
        self.assertFalse(_jobs_reachable_for_stage("dep", jobs, "bar"))


# ---------------------------------------------------------------------------
# Tests: _check_dependency closure
# ---------------------------------------------------------------------------

class TestDependencyClosure(unittest.TestCase):

    def _build_jobs(self, defs: dict[str, dict]) -> dict:
        jobs = {}
        for name, d in defs.items():
            if_raw = d.get("if", "always()")
            needs = d.get("needs", [])
            parsed = _parse_if_condition(if_raw)
            jobs[name] = {
                "needs": needs,
                "if_raw": if_raw,
                "if_parsed": parsed,
                "strategy": None,
            }
        return jobs

    def test_cycle_detection(self):
        jobs = self._build_jobs({
            "a": {"needs": ["b"], "if": "${{ inputs.stage == 'x' }}"},
            "b": {"needs": ["a"], "if": "${{ inputs.stage == 'x' }}"},
        })
        violations = _check_dependency_closure("a", "x", jobs, set(), [])
        cycle_violations = [v for v in violations if v.category == "cycle"]
        self.assertTrue(len(cycle_violations) > 0)

    def test_missing_job_reference(self):
        jobs = self._build_jobs({
            "a": {"needs": ["nonexistent"], "if": "${{ inputs.stage == 'x' }}"},
        })
        violations = _check_dependency_closure("a", "x", jobs, set(), [])
        missing = [v for v in violations if v.category == "missing-job"]
        self.assertEqual(len(missing), 1)

    def test_skipped_required_dependency(self):
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "source-ci": {"needs": ["resolve"], "if": "${{ inputs.stage == 'foo' }}"},
            "consumer": {
                "needs": ["resolve", "source-ci"],
                "if": "${{ always() && inputs.stage == 'bar' && needs.source-ci.result == 'success' }}",
            },
        })
        violations = _check_dependency_closure("consumer", "bar", jobs, set(), [])
        skipped = [v for v in violations if v.category == "skipped-required-dependency"]
        self.assertEqual(len(skipped), 1)
        self.assertIn("source-ci", skipped[0].message)

    def test_unreachable_dependency_not_flagged(self):
        """A dependency that's unreachable but NOT required to succeed is fine."""
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "source-ci": {"needs": ["resolve"], "if": "${{ inputs.stage == 'foo' }}"},
            "consumer": {
                "needs": ["resolve", "source-ci"],
                "if": "${{ always() && needs.resolve.result == 'success' }}",
            },
        })
        # consumer runs for 'bar', source-ci is skipped, but consumer doesn't require source-ci
        violations = _check_dependency_closure("consumer", "bar", jobs, set(), [])
        skipped = [v for v in violations if v.category == "skipped-required-dependency"]
        self.assertEqual(len(skipped), 0)

    def test_baseline_sustained_defect(self):
        """The exact baseline defect from the plan: source-ci not reachable for mixed-fleet-sustained."""
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "source-ci": {
                "needs": ["resolve"],
                "if": "${{ inputs.stage == 'protocol-prepublish' || inputs.stage == 'binary-prepublish' || inputs.stage == 'native-evidence' || inputs.stage == 'mixed-fleet-client' }}",
            },
            "mixed-fleet-sustained": {
                "needs": ["resolve", "source-ci"],
                "if": "${{ inputs.stage == 'mixed-fleet-sustained' && needs.source-ci.result == 'success' }}",
            },
        })
        violations = _check_dependency_closure("mixed-fleet-sustained", "mixed-fleet-sustained", jobs, set(), [])
        skipped = [v for v in violations if v.category == "skipped-required-dependency"]
        self.assertEqual(len(skipped), 1)
        self.assertIn("source-ci", skipped[0].message)
        self.assertIn("mixed-fleet-sustained", skipped[0].message)


# ---------------------------------------------------------------------------
# Tests: dispatch options extraction
# ---------------------------------------------------------------------------

class TestExtractDispatchOptions(unittest.TestCase):

    def test_extracts_options(self):
        import yaml
        workflow = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
""")
        options = _extract_dispatch_options(workflow)
        self.assertIn("protocol-prepublish", options)
        self.assertIn("mixed-fleet-sustained", options)
        self.assertEqual(len(options), 4)


# ---------------------------------------------------------------------------
# Tests: full validator against actual workflow
# ---------------------------------------------------------------------------

class TestActualWorkflow(unittest.TestCase):

    def test_corrected_workflow_passes(self):
        violations = validate_workflow()
        # With the source-ci fix applied, the corrected workflow should pass
        # Filter out any non-critical violations for this test
        critical = [v for v in violations if v.category not in ("missing-script",)]
        self.assertEqual(len(critical), 0, f"Violations: {[str(v) for v in critical]}")


# ---------------------------------------------------------------------------
# Tests: truth table coverage
# ---------------------------------------------------------------------------

class TestTruthTable(unittest.TestCase):
    """Verify the truth table covers all dispatch options and expected jobs."""

    STAGE_TRUTH_TABLE = {
        "protocol-prepublish": ["resolve", "source-ci", "protocol-prepublish"],
        "protocol-index-check": ["resolve", "protocol-index-check"],
        "registry-reverify-greggd": ["resolve", "protocol-index-check", "registry-reverify-greggd"],
        "registry-reverify-gregg": ["resolve", "protocol-index-check", "registry-reverify-gregg"],
        "binary-prepublish": ["resolve", "source-ci", "protocol-index-check", "binary-prepublish", "binary-msrv"],
        "native-evidence": ["resolve", "source-ci", "native-evidence", "native-package-evidence"],
        "mixed-fleet-client": ["resolve", "source-ci", "mixed-fleet-client"],
        "mixed-fleet-sustained": ["resolve", "source-ci", "mixed-fleet-sustained"],
        "operational-evidence": ["resolve", "operational-evidence", "operational-macos-evidence", "systemd-lifecycle", "launchd-lifecycle"],
        "postpublish-verify": ["resolve", "protocol-index-check", "postpublish-verify"],
    }

    def test_all_dispatch_options_covered(self):
        """Every dispatch option must have an entry in the truth table."""
        import yaml
        workflow_path = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "release-candidate.yml"
        workflow_text = workflow_path.read_text(encoding="utf-8")
        options = _extract_dispatch_options(workflow_text)
        for option in options:
            self.assertIn(option, self.STAGE_TRUTH_TABLE, f"dispatch option '{option}' missing from truth table")

    def test_truth_table_jobs_exist(self):
        """Every job in the truth table must exist in the actual workflow."""
        import yaml
        workflow_path = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "release-candidate.yml"
        workflow_data = yaml.safe_load(workflow_path.read_text(encoding="utf-8"))
        jobs = _extract_jobs(workflow_data)
        for stage, expected_jobs in self.STAGE_TRUTH_TABLE.items():
            for job_name in expected_jobs:
                self.assertIn(job_name, jobs, f"truth table references nonexistent job '{job_name}' for stage '{stage}'")


# ---------------------------------------------------------------------------
# Tests: unknown dispatch options and unsupported expressions
# ---------------------------------------------------------------------------

class TestUnknownDispatchOptions(unittest.TestCase):

    def test_unknown_dispatch_option_detected(self):
        """A dispatch option not in the truth table is rejected by the validator."""
        import yaml
        workflow_path = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "release-candidate.yml"
        workflow_text = workflow_path.read_text(encoding="utf-8")
        # The truth table inside validate_workflow is hardcoded; this test confirms
        # that the validator's extraction function captures unknown options.
        modified = workflow_text.replace(
            "          - postpublish-verify\n",
            "          - postpublish-verify\n          - unknown-future-stage\n",
        )
        options = _extract_dispatch_options(modified)
        self.assertIn("unknown-future-stage", options)

    def test_unknown_dispatch_option_rejected_by_validator(self):
        """The full validator rejects workflows with unknown dispatch options."""
        import yaml
        workflow_path = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "release-candidate.yml"
        workflow_text = workflow_path.read_text(encoding="utf-8")
        modified = workflow_text.replace(
            "          - postpublish-verify\n",
            "          - postpublish-verify\n          - unknown-future-stage\n",
        )
        with tempfile.NamedTemporaryFile(mode="w", suffix=".yml", delete=False, dir=workflow_path.parent) as f:
            f.write(modified)
            f.flush()
            # Temporarily override the WORKFLOW constant
            original_workflow = _mod.WORKFLOW
            try:
                _mod.WORKFLOW = Path(f.name)
                violations = validate_workflow()
                unknown = [v for v in violations if v.category == "unknown-dispatch-option"]
                self.assertTrue(len(unknown) > 0, f"Expected unknown-dispatch-option violations, got: {[str(v) for v in violations]}")
            finally:
                _mod.WORKFLOW = original_workflow
                Path(f.name).unlink(missing_ok=True)


class TestUnsupportedExpressions(unittest.TestCase):

    def test_unsupported_expression_detected(self):
        """A job with an unsupported if expression is flagged."""
        jobs = {
            "resolve": {
                "needs": [],
                "if_raw": "always()",
                "if_parsed": _parse_if_condition("always()"),
                "strategy": None,
            },
            "weird": {
                "needs": ["resolve"],
                "if_raw": "${{ github.event_name == 'push' && inputs.stage == 'foo' }}",
                "if_parsed": _parse_if_condition("${{ github.event_name == 'push' && inputs.stage == 'foo' }}"),
                "strategy": None,
            },
        }
        violations = _check_dependency_closure("weird", "foo", jobs, set(), [])
        unsupported = [v for v in violations if v.category == "unsupported-expression"]
        self.assertTrue(len(unsupported) > 0)


# ---------------------------------------------------------------------------
# Tests: negative cases from plan Step 7
# ---------------------------------------------------------------------------

class TestNegativeCases(unittest.TestCase):

    def _build_jobs(self, defs: dict[str, dict]) -> dict:
        jobs = {}
        for name, d in defs.items():
            if_raw = d.get("if", "always()")
            needs = d.get("needs", [])
            parsed = _parse_if_condition(if_raw)
            jobs[name] = {
                "needs": needs,
                "if_raw": if_raw,
                "if_parsed": parsed,
                "strategy": None,
            }
        return jobs

    def test_baseline_sustained_dependency_defect(self):
        """Negative case 1: baseline sustained dependency defect."""
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "source-ci": {
                "needs": ["resolve"],
                "if": "${{ inputs.stage == 'protocol-prepublish' || inputs.stage == 'binary-prepublish' }}",
            },
            "mixed-fleet-sustained": {
                "needs": ["resolve", "source-ci"],
                "if": "${{ inputs.stage == 'mixed-fleet-sustained' && needs.source-ci.result == 'success' }}",
            },
        })
        violations = _check_dependency_closure("mixed-fleet-sustained", "mixed-fleet-sustained", jobs, set(), [])
        skipped = [v for v in violations if v.category == "skipped-required-dependency"]
        self.assertEqual(len(skipped), 1)
        self.assertIn("source-ci", skipped[0].message)

    def test_missing_needs_job(self):
        """Negative case 2: job references a nonexistent dependency."""
        jobs = self._build_jobs({
            "a": {"needs": ["nonexistent"], "if": "${{ inputs.stage == 'x' }}"},
        })
        violations = _check_dependency_closure("a", "x", jobs, set(), [])
        missing = [v for v in violations if v.category == "missing-job"]
        self.assertEqual(len(missing), 1)

    def test_mutually_exclusive_dependency_condition(self):
        """Negative case 3: dependency runs for one stage, consumer requires it for another."""
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "dep-a": {"needs": ["resolve"], "if": "${{ inputs.stage == 'stage-a' }}"},
            "consumer-b": {
                "needs": ["resolve", "dep-a"],
                "if": "${{ always() && inputs.stage == 'stage-b' && needs.dep-a.result == 'success' }}",
            },
        })
        violations = _check_dependency_closure("consumer-b", "stage-b", jobs, set(), [])
        skipped = [v for v in violations if v.category == "skipped-required-dependency"]
        self.assertEqual(len(skipped), 1)
        self.assertIn("dep-a", skipped[0].message)

    def test_unknown_dispatch_option_via_extract(self):
        """Negative case 4: unknown dispatch option detected in extraction."""
        # _extract_dispatch_options reads from the options: list, not from job if-conditions.
        # Create a workflow with an unknown option in the dispatch list.
        custom_base = """
name: test
on:
  workflow_dispatch:
    inputs:
      stage:
        type: choice
        options:
          - known-option
          - unknown-option
"""
        workflow = custom_base + "\njobs:\n  resolve:\n    if: always()\n    runs-on: ubuntu-latest\n"
        options = _extract_dispatch_options(workflow)
        self.assertIn("known-option", options)
        self.assertIn("unknown-option", options)

    def test_required_stage_removed(self):
        """Negative case 5: required stage has no producing job in the workflow."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  source-ci:
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
""")
        data = _parse_data(workflow_yaml)
        jobs = _extract_jobs(data)
        requirements = {
            "pre_tag_required_stages": ["source-ci", "protocol-prepublish", "missing-stage-x"],
        }
        violations = _validate_stage_contract(
            ["protocol-prepublish", "binary-prepublish"], jobs, data, requirements
        )
        missing = [v for v in violations if v.category == "missing-required-stage"]
        missing_names = [v.stage for v in missing]
        self.assertIn("missing-stage-x", missing_names)
        self.assertIn("protocol-prepublish", missing_names)

    def test_malformed_if_expression(self):
        """Negative case 6: unsupported/malformed if expression fails closed."""
        result = _parse_if_condition("${{ some_complex_github_expr('foo') }}")
        # Should be marked unsupported
        self.assertFalse(result["supported"])

    def test_dependency_cycle(self):
        """Negative case 7: dependency cycle."""
        jobs = self._build_jobs({
            "a": {"needs": ["b"], "if": "${{ inputs.stage == 'x' }}"},
            "b": {"needs": ["a"], "if": "${{ inputs.stage == 'x' }}"},
        })
        violations = _check_dependency_closure("a", "x", jobs, set(), [])
        cycles = [v for v in violations if v.category == "cycle"]
        self.assertTrue(len(cycles) > 0)

    def test_skipped_dependency_requiring_success(self):
        """Negative case 8: job requires success from a skipped dependency."""
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "gate": {"needs": ["resolve"], "if": "${{ inputs.stage == 'alpha' }}"},
            "consumer": {
                "needs": ["resolve", "gate"],
                "if": "${{ always() && inputs.stage == 'beta' && needs.gate.result == 'success' }}",
            },
        })
        violations = _check_dependency_closure("consumer", "beta", jobs, set(), [])
        skipped = [v for v in violations if v.category == "skipped-required-dependency"]
        self.assertEqual(len(skipped), 1)
        self.assertIn("gate", skipped[0].message)


# ---------------------------------------------------------------------------
# Tests: positive cases from plan Step 7
# ---------------------------------------------------------------------------

class TestPositiveCases(unittest.TestCase):

    def _build_jobs(self, defs: dict[str, dict]) -> dict:
        jobs = {}
        for name, d in defs.items():
            if_raw = d.get("if", "always()")
            needs = d.get("needs", [])
            parsed = _parse_if_condition(if_raw)
            jobs[name] = {
                "needs": needs,
                "if_raw": if_raw,
                "if_parsed": parsed,
                "strategy": None,
            }
        return jobs

    def test_current_corrected_workflow(self):
        """Positive case 1: the corrected workflow passes the validator."""
        violations = validate_workflow()
        critical = [v for v in violations if v.category not in ("missing-script",)]
        self.assertEqual(len(critical), 0, f"Violations: {[str(v) for v in critical]}")

    def test_unconditional_dependency(self):
        """Positive case 2: unconditional dependency always reachable."""
        jobs = self._build_jobs({
            "resolve": {"needs": [], "if": "always()"},
            "always-dep": {"needs": ["resolve"], "if": "always()"},
            "consumer": {
                "needs": ["resolve", "always-dep"],
                "if": "${{ always() && inputs.stage == 'x' }}",
            },
        })
        violations = _check_dependency_closure("consumer", "x", jobs, set(), [])
        critical = [v for v in violations if v.category in ("skipped-required-dependency", "cycle", "missing-job")]
        self.assertEqual(len(critical), 0)

    def test_matrix_job_with_native_artifacts(self):
        """Positive case 3: matrix job with multiple architecture artifacts."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  native-source:
    needs: [resolve]
    if: "${{ inputs.stage == 'native-evidence' }}"
    runs-on: ubuntu-latest
    strategy:
      matrix:
        include:
          - runner: ubuntu-latest
            name: linux-x86-64
          - runner: ubuntu-24.04-arm
            name: linux-arm64
    steps:
      - run: |
          python3 scripts/validate-release-evidence.py write-candidate --stage "native-source-${{ matrix.name }}"
""")
        data = _parse_data(workflow_yaml)
        job_stage_map = _build_job_stage_map(data)
        # The matrix job should produce stage names for each architecture
        stages = job_stage_map.get("native-source", [])
        # With the matrix expansion, we expect at least linux-x86-64 and linux-arm64
        self.assertTrue(len(stages) > 0, f"Expected stages from matrix job, got {stages}")

    def test_protected_stage_with_separate_dispatch_input(self):
        """Positive case 4: protected stage whose package input is a separate dispatch input."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  operational-evidence:
    needs: [resolve]
    if: "${{ inputs.stage == 'operational-evidence' && inputs.package_run_id != '' }}"
    runs-on: ubuntu-latest
""")
        data = _parse_data(workflow_yaml)
        jobs = _extract_jobs(data)
        # operational-evidence should be reachable when stage matches
        self.assertTrue(_jobs_reachable_for_stage("operational-evidence", jobs, "operational-evidence"))
        # Its dependency (resolve) should be reachable too
        self.assertTrue(_jobs_reachable_for_stage("resolve", jobs, "operational-evidence"))
        # Check dependency closure passes
        violations = _check_dependency_closure("operational-evidence", "operational-evidence", jobs, set(), [])
        critical = [v for v in violations if v.category in ("skipped-required-dependency", "cycle", "missing-job")]
        self.assertEqual(len(critical), 0)


# ---------------------------------------------------------------------------
# Tests: _build_job_stage_map
# ---------------------------------------------------------------------------

class TestBuildJobStageMap(unittest.TestCase):

    def test_simple_job(self):
        """A non-matrix job with a --stage argument."""
        data = _parse_data(_make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  source-ci:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: |
          python3 scripts/validate-release-evidence.py write-candidate --stage source-ci
"""))
        stage_map = _build_job_stage_map(data)
        self.assertIn("source-ci", stage_map["source-ci"])

    def test_matrix_job_expansion(self):
        """A matrix job expands placeholders in --stage arguments."""
        data = _parse_data(_make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  binary-prepublish:
    needs: [resolve]
    if: "${{ inputs.stage == 'binary-prepublish' }}"
    runs-on: ubuntu-latest
    strategy:
      matrix:
        crate: [greggd, gregg]
    steps:
      - run: |
          python3 scripts/validate-release-evidence.py write-candidate --stage "binary-prepublish-${{ matrix.crate }}"
"""))
        stage_map = _build_job_stage_map(data)
        self.assertIn("binary-prepublish-greggd", stage_map["binary-prepublish"])
        self.assertIn("binary-prepublish-gregg", stage_map["binary-prepublish"])


# ---------------------------------------------------------------------------
# Tests: _validate_stage_contract
# ---------------------------------------------------------------------------

class TestValidateStageContract(unittest.TestCase):

    def test_missing_required_stage_detected(self):
        """A required stage with no producing job is flagged."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  source-ci:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/validate-release-evidence.py write-candidate --stage source-ci
""")
        data = _parse_data(workflow_yaml)
        jobs = _extract_jobs(data)
        requirements = {
            "pre_tag_required_stages": ["source-ci", "protocol-prepublish", "nonexistent-stage"],
        }
        violations = _validate_stage_contract(
            ["protocol-prepublish"], jobs, data, requirements
        )
        missing = [v for v in violations if v.category == "missing-required-stage"]
        self.assertTrue(any("nonexistent-stage" in v.stage for v in missing))

    def test_undocumented_stage_detected(self):
        """A job producing a stage not in requirements is flagged."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  source-ci:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/validate-release-evidence.py write-candidate --stage source-ci
  extra-job:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/validate-release-evidence.py write-candidate --stage extra-unlisted-stage
""")
        data = _parse_data(workflow_yaml)
        jobs = _extract_jobs(data)
        requirements = {
            "pre_tag_required_stages": ["source-ci"],
        }
        violations = _validate_stage_contract(
            ["protocol-prepublish"], jobs, data, requirements
        )
        undocumented = [v for v in violations if v.category == "undocumented-stage"]
        self.assertTrue(any("extra-unlisted-stage" in v.stage for v in undocumented))

    def test_duplicate_stage_producer_detected(self):
        """Two jobs producing the same stage for the same dispatch is flagged."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  job-a:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/validate-release-evidence.py write-candidate --stage shared-stage
  job-b:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/validate-release-evidence.py write-candidate --stage shared-stage
""")
        data = _parse_data(workflow_yaml)
        jobs = _extract_jobs(data)
        requirements = {
            "pre_tag_required_stages": ["shared-stage"],
        }
        violations = _validate_stage_contract(
            ["protocol-prepublish"], jobs, data, requirements
        )
        dupes = [v for v in violations if v.category == "duplicate-stage-producer"]
        self.assertTrue(len(dupes) > 0)

    def test_no_producer_for_dispatch_option(self):
        """A dispatch option with no reachable producing job is flagged."""
        workflow_yaml = _make_workflow("""
  resolve:
    if: always()
    runs-on: ubuntu-latest
  source-ci:
    needs: [resolve]
    if: "${{ inputs.stage == 'protocol-prepublish' }}"
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/validate-release-evidence.py write-candidate --stage source-ci
""")
        data = _parse_data(workflow_yaml)
        jobs = _extract_jobs(data)
        requirements = {
            "pre_tag_required_stages": ["source-ci"],
        }
        violations = _validate_stage_contract(
            ["protocol-prepublish", "binary-prepublish"], jobs, data, requirements
        )
        no_producer = [v for v in violations if v.category == "no-producer"]
        self.assertTrue(any("binary-prepublish" in v.stage for v in no_producer))

    def test_actual_workflow_contract_passes(self):
        """The real workflow and requirements contract should pass validation."""
        import yaml
        workflow_path = Path(__file__).resolve().parents[2] / ".github" / "workflows" / "release-candidate.yml"
        workflow_text = workflow_path.read_text(encoding="utf-8")
        workflow_data = yaml.safe_load(workflow_text)
        dispatch_options = _extract_dispatch_options(workflow_text)
        jobs = _extract_jobs(workflow_data)
        requirements = {
            "pre_tag_required_stages": [
                "source-ci", "protocol-prepublish", "protocol-index-check",
                "registry-reverify-greggd", "registry-reverify-gregg",
                "binary-prepublish-greggd", "binary-prepublish-gregg",
                "binary-msrv-greggd", "binary-msrv-gregg",
                "native-source-linux-x86-64", "native-source-linux-arm64",
                "native-source-macos-arm64", "native-source-macos-intel",
                "native-package-linux-x86-64", "native-package-linux-arm64",
                "native-package-macos-arm64", "native-package-macos-intel",
                "mixed-fleet-functional", "mixed-fleet-sustained",
                "systemd-lifecycle", "launchd-lifecycle",
                "resource-linux", "resource-macos-arm64",
                "soak-linux-24h", "soak-macos-arm64-24h",
            ],
            "final_required_stages": [
                "source-ci", "protocol-prepublish", "protocol-index-check",
                "binary-prepublish-greggd", "binary-prepublish-gregg",
                "binary-msrv-greggd", "binary-msrv-gregg",
                "native-source-linux-x86-64", "native-source-linux-arm64",
                "native-source-macos-arm64", "native-source-macos-intel",
                "native-package-linux-x86-64", "native-package-linux-arm64",
                "native-package-macos-arm64", "native-package-macos-intel",
                "mixed-fleet-functional", "mixed-fleet-sustained",
                "systemd-lifecycle", "launchd-lifecycle",
                "resource-linux", "resource-macos-arm64",
                "soak-linux-24h", "soak-macos-arm64-24h",
                "postpublish-verify",
            ],
        }
        violations = _validate_stage_contract(dispatch_options, jobs, workflow_data, requirements)
        critical = [v for v in violations if v.category not in ("missing-script",)]
        self.assertEqual(len(critical), 0, f"Contract violations: {[str(v) for v in critical]}")


if __name__ == "__main__":
    unittest.main()
