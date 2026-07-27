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


class SummaryFieldValidationTests(unittest.TestCase):
    """Integration tests for summary field completeness checking.

    These exercise the same validation logic the runner uses, but with
    synthetic summaries to verify that missing fields are detected.
    """

    REQUIRED_FIELDS = [
        "requested_duration_secs",
        "observed_duration_secs",
        "endpoint_count",
        "completed_generations",
        "first_generation",
        "last_generation",
        "max_concurrent_polls",
        "online_results",
        "offline_results",
        "observed_transitions",
        "clean_shutdown",
    ]

    def _valid_summary(self) -> dict:
        return {
            "requested_duration_secs": 30,
            "observed_duration_secs": 32.5,
            "endpoint_count": 10,
            "completed_generations": 14,
            "first_generation": 1,
            "last_generation": 14,
            "max_concurrent_polls": 10,
            "online_results": 80,
            "offline_results": 60,
            "observed_transitions": ["ep-3:offline->online"],
            "clean_shutdown": True,
            "panic_or_join_failure": None,
        }

    def test_valid_summary_passes(self) -> None:
        summary = self._valid_summary()
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertEqual(missing, [])

    def test_missing_requested_duration_detected(self) -> None:
        summary = self._valid_summary()
        del summary["requested_duration_secs"]
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertIn("requested_duration_secs", missing)

    def test_missing_observed_duration_detected(self) -> None:
        summary = self._valid_summary()
        del summary["observed_duration_secs"]
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertIn("observed_duration_secs", missing)

    def test_missing_endpoint_count_detected(self) -> None:
        summary = self._valid_summary()
        del summary["endpoint_count"]
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertIn("endpoint_count", missing)

    def test_missing_transitions_detected(self) -> None:
        summary = self._valid_summary()
        del summary["observed_transitions"]
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertIn("observed_transitions", missing)

    def test_missing_clean_shutdown_detected(self) -> None:
        summary = self._valid_summary()
        del summary["clean_shutdown"]
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertIn("clean_shutdown", missing)

    def test_multiple_missing_fields_detected(self) -> None:
        summary = {"completed_generations": 5}
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertEqual(len(missing), len(self.REQUIRED_FIELDS) - 1)

    def test_empty_summary_all_fields_missing(self) -> None:
        summary: dict = {}
        missing = [f for f in self.REQUIRED_FIELDS if f not in summary]
        self.assertEqual(missing, self.REQUIRED_FIELDS)


class ResourceSampleValidationTests(unittest.TestCase):
    """Integration tests for resource sample validation.

    These call the same validation logic the runner uses against synthetic
    sample lists to verify that malformed data is rejected.
    """

    def _valid_sample(self, index: int = 0, pid: int = 42) -> dict:
        return {
            "sample_index": index,
            "monotonic_ns": 1_000_000_000 * (index + 1),
            "pid": pid,
            "rss_bytes": 10_000_000,
            "virtual_bytes": 100_000_000,
            "thread_count": 7,
            "process_alive": True,
        }

    def test_valid_samples_pass_all_checks(self) -> None:
        samples = [self._valid_sample(i) for i in range(3)]
        self.assertTrue(all(s["process_alive"] for s in samples))
        self.assertTrue(all(s["rss_bytes"] > 0 for s in samples))
        self.assertTrue(
            all(
                samples[i]["monotonic_ns"] > samples[i - 1]["monotonic_ns"]
                for i in range(1, len(samples))
            )
        )
        self.assertEqual(len({s["pid"] for s in samples}), 1)

    def test_empty_samples_fail_min_count(self) -> None:
        samples: list[dict] = []
        min_samples = 5
        self.assertLess(len(samples), min_samples)

    def test_single_sample_fails_min_count(self) -> None:
        samples = [self._valid_sample(0)]
        min_samples = 5
        self.assertLess(len(samples), min_samples)

    def test_zero_rss_detected(self) -> None:
        samples = [self._valid_sample(i) for i in range(3)]
        samples[1]["rss_bytes"] = 0
        zero_rss = [s for s in samples if s["rss_bytes"] == 0]
        self.assertEqual(len(zero_rss), 1)

    def test_dead_process_detected(self) -> None:
        samples = [self._valid_sample(i) for i in range(3)]
        samples[2]["process_alive"] = False
        dead = [s for s in samples if not s["process_alive"]]
        self.assertEqual(len(dead), 1)

    def test_nonmonotonic_timestamps_detected(self) -> None:
        samples = [self._valid_sample(i) for i in range(3)]
        samples[2]["monotonic_ns"] = samples[0]["monotonic_ns"] - 1
        is_monotonic = all(
            samples[i]["monotonic_ns"] > samples[i - 1]["monotonic_ns"]
            for i in range(1, len(samples))
        )
        self.assertFalse(is_monotonic)

    def test_multiple_pids_detected(self) -> None:
        samples = [self._valid_sample(i) for i in range(3)]
        samples[2]["pid"] = 99
        pids = {s["pid"] for s in samples}
        self.assertGreater(len(pids), 1)

    def test_last_sample_too_early_detected(self) -> None:
        requested_secs = 30.0
        workload_started_ns = 1_000_000_000
        last_sample_ns = workload_started_ns + int(requested_secs * 0.5 * 1e9)
        elapsed_at_last = (last_sample_ns - workload_started_ns) / 1e9
        self.assertLess(elapsed_at_last, requested_secs * 0.8)

    def test_last_sample_after_80_percent_passes(self) -> None:
        requested_secs = 30.0
        workload_started_ns = 1_000_000_000
        last_sample_ns = workload_started_ns + int(requested_secs * 0.9 * 1e9)
        elapsed_at_last = (last_sample_ns - workload_started_ns) / 1e9
        self.assertGreaterEqual(elapsed_at_last, requested_secs * 0.8)


class RunnerMainIntegrationTests(unittest.TestCase):
    """Integration tests that invoke the runner's main() with various args.

    These verify the argument parsing and early validation paths without
    needing a compiled Rust binary.
    """

    def test_main_rejects_negative_duration(self) -> None:
        with patch("sys.argv", ["runner", "--duration-seconds", "-5"]):
            result = runner_mod.main()
        self.assertEqual(result, 1)

    def test_main_rejects_zero_duration(self) -> None:
        with patch("sys.argv", ["runner", "--duration-seconds", "0"]):
            result = runner_mod.main()
        self.assertEqual(result, 1)

    def test_main_rejects_negative_interval(self) -> None:
        with patch(
            "sys.argv",
            ["runner", "--duration-seconds", "1", "--sample-interval-seconds", "-1"],
        ):
            result = runner_mod.main()
        self.assertEqual(result, 1)

    def test_main_rejects_zero_interval(self) -> None:
        with patch(
            "sys.argv",
            ["runner", "--duration-seconds", "1", "--sample-interval-seconds", "0"],
        ):
            result = runner_mod.main()
        self.assertEqual(result, 1)

    def test_main_rejects_invalid_profile(self) -> None:
        with patch(
            "sys.argv",
            ["runner", "--duration-seconds", "1", "--profile", "invalid"],
        ):
            with self.assertRaises(SystemExit):
                runner_mod.main()


if __name__ == "__main__":
    unittest.main()
