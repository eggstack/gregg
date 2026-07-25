#!/usr/bin/env python3
"""Tests for the release workflow graph-based validator."""

from __future__ import annotations

import importlib.util
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
_check_dependency_closure = _mod._check_dependency_closure
_extract_dispatch_options = _mod._extract_dispatch_options
_extract_jobs = _mod._extract_jobs
_jobs_reachable_for_stage = _mod._jobs_reachable_for_stage
_parse_if_condition = _mod._parse_if_condition
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


if __name__ == "__main__":
    unittest.main()
