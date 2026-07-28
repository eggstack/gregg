#!/usr/bin/env python3
"""Run the sustained mixed-fleet workload and validate product behavior.

Builds the Rust sustained workload driver, launches it as a child process,
samples its process resources while alive, enforces duration and sample
requirements, and writes validation output files.

Usage:
    python3 scripts/run-mixed-fleet-sustained.py \
        --duration-seconds 30 \
        --sample-interval-seconds 5 \
        --evidence-dir evidence
"""

from __future__ import annotations

import argparse
import json
import os
import resource
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


CONFIGURED_MAX_CONCURRENT_POLLS = 4
MIN_ENDPOINT_COUNT = 3


class SustainedEvidenceError(ValueError):
    """The sustained workload produced evidence that violates its contract."""


def monotonic_ns() -> int:
    """Return a monotonic clock timestamp in nanoseconds."""
    return time.monotonic_ns()


def _number(value: Any, name: str, *, integer: bool = False) -> int | float:
    """Validate JSON numeric values without accepting bools or non-finite values."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise SustainedEvidenceError(f"{name} must be a numeric value")
    if not __import__("math").isfinite(value):
        raise SustainedEvidenceError(f"{name} must be finite")
    if integer and not isinstance(value, int):
        raise SustainedEvidenceError(f"{name} must be an integer")
    return value


def validate_sustained_summary(
    summary: object,
    *,
    requested_secs: float,
    configured_endpoint_count: int,
    configured_max_concurrent_polls: int,
    externally_observed_secs: float,
) -> dict[str, Any]:
    """Validate the complete summary contract used by the runner and tests."""
    if not isinstance(summary, dict):
        raise SustainedEvidenceError("summary must be a JSON object")
    required = {
        "requested_duration_secs", "observed_duration_secs", "endpoint_count",
        "completed_generations", "first_generation", "last_generation",
        "configured_max_concurrent_polls", "observed_max_concurrent_polls",
        "online_results", "offline_results", "observed_transitions",
        "clean_shutdown", "panic_or_join_failure",
    }
    missing = sorted(required - summary.keys())
    if missing:
        raise SustainedEvidenceError(f"summary missing fields: {missing}")

    requested = _number(summary["requested_duration_secs"], "requested_duration_secs")
    observed = _number(summary["observed_duration_secs"], "observed_duration_secs")
    if requested != requested_secs:
        raise SustainedEvidenceError("requested_duration_secs does not match configured duration")
    if observed < requested_secs:
        raise SustainedEvidenceError("observed_duration_secs is shorter than requested duration")
    external = _number(externally_observed_secs, "externally observed duration")
    if external < requested_secs:
        raise SustainedEvidenceError("externally observed duration is shorter than requested duration")

    for name in ("endpoint_count", "completed_generations", "first_generation", "last_generation",
                 "configured_max_concurrent_polls", "observed_max_concurrent_polls",
                 "online_results", "offline_results"):
        _number(summary[name], name, integer=True)
    endpoint_count = summary["endpoint_count"]
    if endpoint_count != configured_endpoint_count or endpoint_count < MIN_ENDPOINT_COUNT:
        raise SustainedEvidenceError("endpoint_count does not match configured workload")
    completed = summary["completed_generations"]
    first = summary["first_generation"]
    last = summary["last_generation"]
    if completed < 2 or first <= 0 or last < first:
        raise SustainedEvidenceError("invalid sustained generation range")
    if completed != last - first + 1:
        raise SustainedEvidenceError("completed_generations is inconsistent with generation range")
    if summary["configured_max_concurrent_polls"] != configured_max_concurrent_polls:
        raise SustainedEvidenceError("configured concurrency does not match workload contract")
    observed_max = summary["observed_max_concurrent_polls"]
    if observed_max <= 0 or observed_max > configured_max_concurrent_polls:
        raise SustainedEvidenceError("observed concurrency is outside configured bounds")
    if summary["online_results"] <= 0 or summary["offline_results"] <= 0:
        raise SustainedEvidenceError("both online and offline results are required")
    transitions = summary["observed_transitions"]
    if not isinstance(transitions, list) or not transitions or not all(isinstance(item, str) and item for item in transitions):
        raise SustainedEvidenceError("observed_transitions must be a nonempty array of strings")
    if summary["clean_shutdown"] is not True:
        raise SustainedEvidenceError("clean_shutdown must be true")
    if summary["panic_or_join_failure"] is not None:
        raise SustainedEvidenceError("panic_or_join_failure must be null")
    return summary


def validate_resource_samples(
    samples: object,
    *,
    pid: int,
    workload_started_ns: int,
    workload_completed_ns: int,
    requested_secs: float,
    sample_interval_secs: float,
) -> list[dict[str, Any]]:
    """Validate resource samples captured for one sustained workload process."""
    if not isinstance(samples, list):
        raise SustainedEvidenceError("resource samples must be an array")
    minimum = max(3, int(requested_secs / sample_interval_secs) - 1)
    if len(samples) < minimum:
        raise SustainedEvidenceError(f"only {len(samples)} resource samples, need at least {minimum}")
    for index, sample in enumerate(samples):
        if not isinstance(sample, dict):
            raise SustainedEvidenceError(f"resource sample {index} must be an object")
        for field in ("sample_index", "monotonic_ns", "pid", "rss_bytes", "virtual_bytes", "thread_count"):
            _number(sample.get(field), f"sample {index}.{field}", integer=True)
        if sample["sample_index"] != index:
            raise SustainedEvidenceError("sample_index values must start at zero and be sequential")
        if sample["pid"] != pid:
            raise SustainedEvidenceError("resource samples reference more than one PID")
        if sample["rss_bytes"] <= 0 or sample["virtual_bytes"] <= 0 or sample["thread_count"] <= 0:
            raise SustainedEvidenceError(f"resource sample {index} has a non-positive metric")
        if sample.get("process_alive") is not True:
            raise SustainedEvidenceError(f"resource sample {index} does not prove process_alive")
        if not workload_started_ns < sample["monotonic_ns"] <= workload_completed_ns:
            raise SustainedEvidenceError(f"resource sample {index} is outside workload lifetime")
        if index and sample["monotonic_ns"] <= samples[index - 1]["monotonic_ns"]:
            raise SustainedEvidenceError("resource sample timestamps must be strictly increasing")
    elapsed = (samples[-1]["monotonic_ns"] - workload_started_ns) / 1e9
    if elapsed < requested_secs * 0.8:
        raise SustainedEvidenceError("last resource sample does not cover 80% of requested runtime")
    return samples


def sample_proc_status(pid: int) -> dict[str, Any] | None:
    """Read /proc/<pid>/status and /proc/<pid>/stat for resource info.

    Returns a dict with rss_bytes, virtual_bytes, thread_count,
    process_alive, or None if the process is gone.
    """
    status_path = Path(f"/proc/{pid}/status")

    if not status_path.exists():
        return None

    try:
        status_text = status_path.read_text(encoding="utf-8", errors="replace")
    except (OSError, PermissionError):
        return None

    rss_kb = 0
    vsize_kb = 0
    threads = 0
    for line in status_text.splitlines():
        if line.startswith("VmRSS:"):
            parts = line.split()
            if len(parts) >= 2:
                try:
                    rss_kb = int(parts[1])
                except ValueError:
                    pass
        elif line.startswith("VmSize:"):
            parts = line.split()
            if len(parts) >= 2:
                try:
                    vsize_kb = int(parts[1])
                except ValueError:
                    pass
        elif line.startswith("Threads:"):
            parts = line.split()
            if len(parts) >= 2:
                try:
                    threads = int(parts[1])
                except ValueError:
                    pass

    # Check if process is alive via kill(pid, 0).
    alive = True
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        alive = False
    except PermissionError:
        # Process exists but we don't have permission — still alive.
        pass

    return {
        "rss_bytes": rss_kb * 1024,
        "virtual_bytes": vsize_kb * 1024,
        "thread_count": threads,
        "process_alive": alive,
    }


def find_test_binary(profile: str) -> Path:
    """Locate the test binary for the sustained workload test.

    Uses `cargo test --no-run` to build and locate the test binary.
    """
    cmd = [
        "cargo",
        "test",
        "-p",
        "gregg",
        "--all-targets",
        "--all-features",
        "--no-run",
    ]
    if profile == "release":
        cmd.append("--release")

    result = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        cwd=str(Path(__file__).resolve().parent.parent),
    )
    if result.returncode != 0:
        print(f"cargo test --no-run failed:\n{result.stderr}", file=sys.stderr)
        sys.exit(1)

    # Parse the output to find the test binary path.
    # cargo test --no-run prints lines like:
    #   Finished test [unoptimized + debuginfo] target(s) in 0.05s
    #   Executable unittests src/main.rs (target/debug/deps/gregg-HASH)
    #   Executable: path/to/binary (or just the path)
    # Note: cargo may print to stdout or stderr depending on version.
    combined_output = result.stdout + "\n" + result.stderr
    for line in combined_output.splitlines():
        line = line.strip()
        if line.startswith("Executable:") or line.startswith("Running:"):
            parts = line.split(None, 1)
            if len(parts) >= 2:
                return Path(parts[1])
        # Handle "Executable unittests src/main.rs (path/to/binary)"
        if line.startswith("Executable "):
            idx = line.rfind("(")
            if idx >= 0:
                path_str = line[idx + 1 :].rstrip(")")
                candidate = Path(path_str)
                if candidate.exists() and candidate.is_file():
                    return candidate
        # Some cargo versions just print the path.
        if line and not line.startswith(("Finished", "Compiling", "Downloading", "Downloaded")):
            candidate = Path(line)
            if candidate.exists() and candidate.is_file():
                return candidate

    # Fallback: search in target directory for the test binary.
    target_dir = Path(__file__).resolve().parent.parent / "target"
    if profile == "release":
        search_dir = target_dir / "release"
    else:
        search_dir = target_dir / "debug"

    # Look for deps directory with test binaries.
    deps_dir = search_dir / "deps"
    if deps_dir.exists():
        for f in sorted(deps_dir.iterdir(), key=lambda p: p.stat().st_mtime, reverse=True):
            if f.name.startswith("gregg-") and f.is_file():
                # Check if it's executable and contains the test.
                try:
                    check = subprocess.run(
                        [str(f), "--list", "--exact", "sustained_workload::mixed_fleet_sustained_workload"],
                        capture_output=True,
                        text=True,
                        timeout=10,
                    )
                    if "mixed_fleet_sustained_workload" in check.stdout:
                        return f
                except (subprocess.TimeoutExpired, OSError):
                    continue

    print("ERROR: could not locate sustained workload test binary", file=sys.stderr)
    sys.exit(1)


def run_sustained(
    duration_secs: int,
    sample_interval_secs: float,
    evidence_dir: Path,
    profile: str,
) -> None:
    """Execute the sustained workload and collect evidence."""
    evidence_dir.mkdir(parents=True, exist_ok=True)

    # Step 1: Build the test binary.
    print(f"Building test binary (profile={profile})...")
    build_start = monotonic_ns()
    test_binary = find_test_binary(profile)
    build_completed = monotonic_ns()
    print(f"Test binary: {test_binary}")
    print(
        f"Build completed in {(build_completed - build_start) / 1e9:.1f}s"
    )

    # Step 2: Set up evidence paths.
    summary_path = evidence_dir / "sustained-summary.json"
    samples_path = evidence_dir / "resource-samples.jsonl"
    stdout_path = evidence_dir / "workload-stdout.txt"
    stderr_path = evidence_dir / "workload-stderr.txt"

    # Clean previous evidence.
    for p in [summary_path, samples_path, stdout_path, stderr_path]:
        p.unlink(missing_ok=True)

    # Step 3: Launch the workload.
    env = os.environ.copy()
    env["GREGG_SUSTAINED_SECONDS"] = str(duration_secs)
    env["GREGG_SUSTAINED_SUMMARY"] = str(summary_path)

    cmd = [
        str(test_binary),
        "--exact",
        "sustained_workload::mixed_fleet_sustained_workload",
        "--ignored",
        "--nocapture",
    ]

    print(f"Launching workload: duration={duration_secs}s, sample_interval={sample_interval_secs}s")
    workload_started_ns = monotonic_ns()

    stdout_file = open(stdout_path, "w", encoding="utf-8")
    stderr_file = open(stderr_path, "w", encoding="utf-8")

    proc = subprocess.Popen(
        cmd,
        stdout=stdout_file,
        stderr=stderr_file,
        env=env,
    )

    print(f"Workload PID: {proc.pid}")

    # Step 4: Confirm child is alive after startup.
    time.sleep(0.5)
    if proc.poll() is not None:
        stdout_file.close()
        stderr_file.close()
        print(
            f"FATAL: workload process exited immediately with code {proc.returncode}",
            file=sys.stderr,
        )
        sys.exit(1)

    # Step 5: Sample resources while child is alive.
    samples: list[dict[str, Any]] = []
    sample_index = 0
    while proc.poll() is None:
        sample_ns = monotonic_ns()
        info = sample_proc_status(proc.pid)
        if info is not None:
            # Discard samples taken while the process is already exiting;
            # VmRSS can drop to zero during teardown.
            if not info["process_alive"]:
                break
            sample = {
                "sample_index": sample_index,
                "monotonic_ns": sample_ns,
                "pid": proc.pid,
                **info,
            }
            samples.append(sample)
            with samples_path.open("a", encoding="utf-8") as f:
                f.write(json.dumps(sample, sort_keys=True) + "\n")
            sample_index += 1
            print(
                f"  Sample {sample_index}: rss={info['rss_bytes'] / 1024:.0f}KB "
                f"threads={info['thread_count']} alive={info['process_alive']}"
            )
        try:
            proc.wait(timeout=sample_interval_secs)
        except subprocess.TimeoutExpired:
            pass

    # Step 6: Wait for exit with bounded cleanup deadline.
    cleanup_deadline = 10.0
    try:
        proc.wait(timeout=cleanup_deadline)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5.0)
        print(
            f"FATAL: workload did not exit within {cleanup_deadline}s cleanup deadline",
            file=sys.stderr,
        )
        sys.exit(1)
    workload_completed_ns = monotonic_ns()
    stdout_file.close()
    stderr_file.close()

    exit_code = proc.returncode
    print(f"Workload exited with code {exit_code}")

    # Step 7: Reject nonzero exit before accepting any summary evidence.
    if exit_code != 0:
        print(
            f"FATAL: workload process exited with nonzero code {exit_code}; "
            "a valid-looking summary does not satisfy evidence requirements",
            file=sys.stderr,
        )
        sys.exit(1)

    # Step 8: Validate the workload summary.
    if not summary_path.exists():
        print("FATAL: sustained summary not written", file=sys.stderr)
        sys.exit(1)

    try:
        with summary_path.open(encoding="utf-8") as f:
            summary = json.load(f)
    except (json.JSONDecodeError, OSError) as e:
        print(f"FATAL: cannot parse sustained summary: {e}", file=sys.stderr)
        sys.exit(1)

    requested_secs = float(duration_secs)
    observed_ns = workload_completed_ns - workload_started_ns
    observed_secs = observed_ns / 1e9

    configured_endpoint_count = 10 if duration_secs >= 5 else 9
    validate_sustained_summary(
        summary,
        requested_secs=requested_secs,
        configured_endpoint_count=configured_endpoint_count,
        configured_max_concurrent_polls=CONFIGURED_MAX_CONCURRENT_POLLS,
        externally_observed_secs=observed_secs,
    )

    print(f"Observed duration: {observed_secs:.1f}s (requested: {requested_secs:.0f}s)")

    if observed_secs < requested_secs:
        print(
            f"FATAL: observed duration {observed_secs:.1f}s < requested {requested_secs:.0f}s",
            file=sys.stderr,
        )
        sys.exit(1)

    validate_resource_samples(
        samples,
        pid=proc.pid,
        workload_started_ns=workload_started_ns,
        workload_completed_ns=workload_completed_ns,
        requested_secs=requested_secs,
        sample_interval_secs=sample_interval_secs,
    )

    # Step 10: Report results.
    print(f"\nEvidence written to {evidence_dir}/")
    print(f"  Summary: {summary.get('completed_generations', 0)} generations")
    print(f"  Samples: {len(samples)}")
    print(f"  Transitions: {summary.get('observed_transitions', [])}")
    print(f"  Duration: {observed_secs:.1f}s")
    print("\nSustained workload evidence PASSED.")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run sustained mixed-fleet workload for product validation"
    )
    parser.add_argument(
        "--duration-seconds",
        type=int,
        default=30,
        help="Minimum workload duration in seconds (default: 30)",
    )
    parser.add_argument(
        "--sample-interval-seconds",
        type=float,
        default=5.0,
        help="Resource sample interval in seconds (default: 5.0)",
    )
    parser.add_argument(
        "--evidence-dir",
        type=Path,
        default=Path("evidence"),
        help="Evidence output directory (default: evidence)",
    )
    parser.add_argument(
        "--profile",
        choices=["debug", "release"],
        default="debug",
        help="Build profile (default: debug)",
    )
    args = parser.parse_args()

    if args.duration_seconds <= 0:
        print("FATAL: --duration-seconds must be positive", file=sys.stderr)
        return 1
    if args.sample_interval_seconds <= 0:
        print("FATAL: --sample-interval-seconds must be positive", file=sys.stderr)
        return 1

    try:
        run_sustained(
            duration_secs=args.duration_seconds,
            sample_interval_secs=args.sample_interval_seconds,
            evidence_dir=args.evidence_dir,
            profile=args.profile,
        )
        return 0
    except SustainedEvidenceError as error:
        print(f"FATAL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
