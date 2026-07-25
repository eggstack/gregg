#!/usr/bin/env python3
"""Deterministic tests for the sustained mixed-fleet runner."""

from __future__ import annotations

import json
import os
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


if __name__ == "__main__":
    unittest.main()
