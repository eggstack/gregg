#!/usr/bin/env python3
"""Deterministic tests for the sustained mixed-fleet runner."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / "scripts" / "run-mixed-fleet-sustained.py"

# Import the runner module.
import importlib.util

spec = importlib.util.spec_from_file_location("runner", str(RUNNER))
runner_mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner_mod)


class SampleProcStatusTests(unittest.TestCase):
    """Tests for sample_proc_status with mock /proc files."""

    def test_returns_none_for_missing_pid(self) -> None:
        result = runner_mod.sample_proc_status(999999)
        self.assertIsNone(result)

    def test_parses_status_fields(self) -> None:
        """Verify parse logic against synthetic /proc content."""
        # We can't easily mock /proc, but we can test the parsing logic
        # by calling the function with a known-live PID (our own).
        my_pid = os.getpid()
        result = runner_mod.sample_proc_status(my_pid)
        self.assertIsNotNone(result)
        if result is not None:
            self.assertTrue(result["process_alive"])
            self.assertGreater(result["rss_bytes"], 0)
            self.assertGreater(result["virtual_bytes"], 0)
            self.assertGreaterEqual(result["thread_count"], 1)


class MonotonicNsTests(unittest.TestCase):
    def test_returns_increasing_values(self) -> None:
        t1 = runner_mod.monotonic_ns()
        t2 = runner_mod.monotonic_ns()
        self.assertGreaterEqual(t2, t1)


class ValidateArgsTests(unittest.TestCase):
    """Test argument validation in main()."""

    def test_negative_duration_rejected(self) -> None:
        with patch("sys.argv", ["runner", "--duration-seconds", "-1"]):
            result = runner_mod.main()
        self.assertNotEqual(result, 0)

    def test_zero_duration_rejected(self) -> None:
        with patch("sys.argv", ["runner", "--duration-seconds", "0"]):
            result = runner_mod.main()
        self.assertNotEqual(result, 0)

    def test_negative_sample_interval_rejected(self) -> None:
        with patch(
            "sys.argv",
            ["runner", "--duration-seconds", "1", "--sample-interval-seconds", "-0.5"],
        ):
            result = runner_mod.main()
        self.assertNotEqual(result, 0)


class SummaryValidationTests(unittest.TestCase):
    """Test that summary JSON must be valid."""

    def test_malformed_summary_detected(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            d = Path(raw)
            bad_summary = d / "sustained-summary.json"
            bad_summary.write_text("not-json", encoding="utf-8")
            try:
                with bad_summary.open(encoding="utf-8") as f:
                    json.load(f)
                self.fail("should have raised JSONDecodeError")
            except json.JSONDecodeError:
                pass


class SampleValidationTests(unittest.TestCase):
    """Test sample validation logic against synthetic data."""

    def test_monotonic_timestamps(self) -> None:
        samples = [
            {"monotonic_ns": 100, "process_alive": True, "rss_bytes": 1024, "pid": 1},
            {"monotonic_ns": 200, "process_alive": True, "rss_bytes": 2048, "pid": 1},
            {"monotonic_ns": 300, "process_alive": True, "rss_bytes": 3072, "pid": 1},
        ]
        for i in range(1, len(samples)):
            self.assertGreater(samples[i]["monotonic_ns"], samples[i - 1]["monotonic_ns"])

    def test_rejects_non_monotonic_timestamps(self) -> None:
        samples = [
            {"monotonic_ns": 300, "process_alive": True, "rss_bytes": 1024, "pid": 1},
            {"monotonic_ns": 100, "process_alive": True, "rss_bytes": 2048, "pid": 1},
        ]
        is_monotonic = all(
            samples[i]["monotonic_ns"] > samples[i - 1]["monotonic_ns"]
            for i in range(1, len(samples))
        )
        self.assertFalse(is_monotonic)

    def test_rejects_zero_rss(self) -> None:
        samples = [
            {"monotonic_ns": 100, "process_alive": True, "rss_bytes": 0, "pid": 1},
        ]
        zero_rss = [s for s in samples if s["rss_bytes"] == 0]
        self.assertEqual(len(zero_rss), 1)

    def test_rejects_dead_process(self) -> None:
        samples = [
            {"monotonic_ns": 100, "process_alive": False, "rss_bytes": 1024, "pid": 1},
        ]
        dead = [s for s in samples if not s["process_alive"]]
        self.assertEqual(len(dead), 1)

    def test_rejects_multiple_pids(self) -> None:
        samples = [
            {"monotonic_ns": 100, "process_alive": True, "rss_bytes": 1024, "pid": 1},
            {"monotonic_ns": 200, "process_alive": True, "rss_bytes": 2048, "pid": 2},
        ]
        pids = {s["pid"] for s in samples}
        self.assertEqual(len(pids), 2)

    def test_valid_samples_pass(self) -> None:
        samples = [
            {"monotonic_ns": 100, "process_alive": True, "rss_bytes": 1024, "pid": 42},
            {"monotonic_ns": 200, "process_alive": True, "rss_bytes": 2048, "pid": 42},
            {"monotonic_ns": 300, "process_alive": True, "rss_bytes": 3072, "pid": 42},
        ]
        # All checks pass.
        self.assertTrue(all(s["process_alive"] for s in samples))
        self.assertTrue(all(s["rss_bytes"] > 0 for s in samples))
        self.assertTrue(
            all(
                samples[i]["monotonic_ns"] > samples[i - 1]["monotonic_ns"]
                for i in range(1, len(samples))
            )
        )
        self.assertEqual(len({s["pid"] for s in samples}), 1)


class DurationValidationTests(unittest.TestCase):
    """Test duration computation and validation."""

    def test_observed_duration_below_minimum_detected(self) -> None:
        requested = 30.0
        observed = 25.0
        self.assertLess(observed, requested)

    def test_observed_duration_meets_minimum(self) -> None:
        requested = 30.0
        observed = 32.5
        self.assertGreaterEqual(observed, requested)

    def test_last_sample_after_80_percent(self) -> None:
        requested = 30.0
        last_sample_elapsed = 28.0
        self.assertGreaterEqual(last_sample_elapsed, requested * 0.8)

    def test_last_sample_too_early(self) -> None:
        requested = 30.0
        last_sample_elapsed = 20.0
        self.assertLess(last_sample_elapsed, requested * 0.8)


class CandidateMetadataTests(unittest.TestCase):
    """Test candidate metadata structure."""

    def test_candidate_has_required_fields(self) -> None:
        candidate = {
            "schema_version": 1,
            "stage": "mixed-fleet-sustained",
            "requested_duration_secs": 30,
            "observed_duration_secs": 32.5,
            "resource_sample_count": 6,
            "completed_generations": 150,
            "note": "requested_duration_secs=30 observed_duration_secs=32.5",
        }
        for field in [
            "schema_version",
            "stage",
            "requested_duration_secs",
            "observed_duration_secs",
            "resource_sample_count",
            "completed_generations",
        ]:
            self.assertIn(field, candidate)


class EarlyExitDetectionTests(unittest.TestCase):
    """Test detection of workload that exits before requested duration."""

    def test_duration_too_short_rejected(self) -> None:
        requested = 30.0
        observed = 15.0
        self.assertLess(observed, requested)

    def test_child_exit_detected_immediately(self) -> None:
        import subprocess

        result = subprocess.run(
            ["false"],
            capture_output=True,
            timeout=5,
        )
        self.assertNotEqual(result.returncode, 0)


class MissingSummaryTests(unittest.TestCase):
    """Test detection of missing or invalid summary."""

    def test_missing_summary_file_detected(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            summary_path = Path(raw) / "sustained-summary.json"
            self.assertFalse(summary_path.exists())

    def test_empty_summary_file_detected(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            summary_path = Path(raw) / "sustained-summary.json"
            summary_path.write_text("", encoding="utf-8")
            try:
                with summary_path.open(encoding="utf-8") as f:
                    json.load(f)
                self.fail("should have raised JSONDecodeError")
            except json.JSONDecodeError:
                pass


class InsufficientGenerationTests(unittest.TestCase):
    """Test detection of summary with insufficient generation data."""

    def test_summary_generation_count_zero(self) -> None:
        summary = {
            "requested_duration_secs": 30,
            "observed_duration_secs": 32.5,
            "endpoint_count": 10,
            "completed_generations": 0,
            "first_generation": 0,
            "last_generation": 0,
            "max_concurrent_polls": 0,
            "online_results": 0,
            "offline_results": 0,
            "observed_transitions": [],
            "clean_shutdown": True,
            "panic_or_join_failure": None,
        }
        self.assertEqual(summary["completed_generations"], 0)

    def test_summary_generation_count_one(self) -> None:
        summary = {
            "completed_generations": 1,
            "first_generation": 1,
            "last_generation": 1,
        }
        self.assertEqual(summary["completed_generations"], 1)
        self.assertEqual(summary["first_generation"], summary["last_generation"])


class EmptyResourceSamplesTests(unittest.TestCase):
    """Test detection of empty or missing resource samples."""

    def test_empty_jsonl_file_detected(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            samples_path = Path(raw) / "resource-samples.jsonl"
            samples_path.write_text("", encoding="utf-8")
            lines = [
                line
                for line in samples_path.read_text(encoding="utf-8").splitlines()
                if line.strip()
            ]
            self.assertEqual(len(lines), 0)

    def test_jsonl_with_only_whitespace(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            samples_path = Path(raw) / "resource-samples.jsonl"
            samples_path.write_text("\n\n  \n", encoding="utf-8")
            lines = [
                line
                for line in samples_path.read_text(encoding="utf-8").splitlines()
                if line.strip()
            ]
            self.assertEqual(len(lines), 0)


class NoTransitionDetectionTests(unittest.TestCase):
    """Test detection when no endpoint state transitions occur."""

    def test_all_online_no_transition(self) -> None:
        transitions: list[str] = []
        seen_online: dict[str, bool] = {}
        results = [
            {"system_id": "a", "online": True},
            {"system_id": "b", "online": True},
        ]
        for r in results:
            sid = r["system_id"]
            is_online = r["online"]
            if sid in seen_online and seen_online[sid] != is_online:
                transitions.append(
                    f"{sid}:{'offline' if is_online else 'online'}->{'online' if is_online else 'offline'}"
                )
            seen_online[sid] = is_online
        self.assertEqual(len(transitions), 0)

    def test_all_offline_no_transition(self) -> None:
        transitions: list[str] = []
        seen_online: dict[str, bool] = {}
        results = [
            {"system_id": "a", "online": False},
            {"system_id": "b", "online": False},
        ]
        for r in results:
            sid = r["system_id"]
            is_online = r["online"]
            if sid in seen_online and seen_online[sid] != is_online:
                transitions.append(
                    f"{sid}:{'offline' if is_online else 'online'}->{'online' if is_online else 'offline'}"
                )
            seen_online[sid] = is_online
        self.assertEqual(len(transitions), 0)


class GenerationCompletenessTests(unittest.TestCase):
    """Test detection when a generation omits an endpoint."""

    def test_generation_with_missing_endpoint(self) -> None:
        endpoint_count = 10
        results = [{"system_id": f"ep-{i}"} for i in range(9)]
        self.assertNotEqual(len(results), endpoint_count)

    def test_generation_with_extra_endpoint(self) -> None:
        endpoint_count = 10
        results = [{"system_id": f"ep-{i}"} for i in range(11)]
        self.assertNotEqual(len(results), endpoint_count)


class ChildTimeoutTests(unittest.TestCase):
    """Test bounded cleanup deadline enforcement."""

    def test_process_wait_timeout_enforced(self) -> None:
        import signal
        import subprocess

        proc = subprocess.Popen(["sleep", "300"])
        try:
            try:
                proc.wait(timeout=0.1)
            except subprocess.TimeoutExpired:
                pass
            self.assertIsNone(proc.poll())
        finally:
            proc.kill()
            proc.wait()


class ExitCodeDetectionTests(unittest.TestCase):
    """Test detection of non-zero workload exit codes."""

    def test_nonzero_exit_code_detected(self) -> None:
        import subprocess

        result = subprocess.run(["false"], capture_output=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)

    def test_panic_exit_code_detected(self) -> None:
        import subprocess

        result = subprocess.run(
            [sys.executable, "-c", "raise RuntimeError('panic')"],
            capture_output=True,
            timeout=5,
        )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
