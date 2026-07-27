#!/usr/bin/env python3
"""Run the sustained mixed-fleet workload and validate evidence.

Builds the Rust sustained workload driver, launches it as a child process,
samples its process resources while alive, enforces duration and sample
requirements, and writes evidence files.

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


def monotonic_ns() -> int:
    """Return a monotonic clock timestamp in nanoseconds."""
    return time.monotonic_ns()


def sample_proc_status(pid: int) -> dict[str, Any] | None:
    """Read /proc/<pid>/status and /proc/<pid>/stat for resource info.

    Returns a dict with rss_bytes, virtual_bytes, thread_count,
    process_alive, or None if the process is gone.
    """
    status_path = Path(f"/proc/{pid}/status")
    stat_path = Path(f"/proc/{pid}/stat")

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
    artifacts_path = evidence_dir / "artifacts.json"
    candidate_path = evidence_dir / "candidate.json"

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

    # Step 6: Wait for exit.
    proc.wait()
    workload_completed_ns = monotonic_ns()
    stdout_file.close()
    stderr_file.close()

    exit_code = proc.returncode
    print(f"Workload exited with code {exit_code}")

    # Step 7: Validate the workload summary.
    if not summary_path.exists():
        print("FATAL: sustained summary not written", file=sys.stderr)
        sys.exit(1)

    try:
        with summary_path.open(encoding="utf-8") as f:
            summary = json.load(f)
    except (json.JSONDecodeError, OSError) as e:
        print(f"FATAL: cannot parse sustained summary: {e}", file=sys.stderr)
        sys.exit(1)

    # Step 8: Validate duration.
    observed_ns = workload_completed_ns - workload_started_ns
    observed_secs = observed_ns / 1e9
    requested_secs = float(duration_secs)

    print(f"Observed duration: {observed_secs:.1f}s (requested: {requested_secs:.0f}s)")

    if observed_secs < requested_secs:
        print(
            f"FATAL: observed duration {observed_secs:.1f}s < requested {requested_secs:.0f}s",
            file=sys.stderr,
        )
        sys.exit(1)

    # Step 9: Validate resource samples.
    min_samples = max(3, int(requested_secs / sample_interval_secs) - 1)
    if len(samples) < min_samples:
        print(
            f"FATAL: only {len(samples)} samples, need at least {min_samples}",
            file=sys.stderr,
        )
        sys.exit(1)

    # All samples must show process alive.
    dead_samples = [s for s in samples if not s["process_alive"]]
    if dead_samples:
        print(
            f"FATAL: {len(dead_samples)} samples show process not alive",
            file=sys.stderr,
        )
        sys.exit(1)

    # All samples must have positive RSS.
    zero_rss = [s for s in samples if s["rss_bytes"] == 0]
    if zero_rss:
        print(
            f"FATAL: {len(zero_rss)} samples have zero RSS",
            file=sys.stderr,
        )
        sys.exit(1)

    # Monotonic timestamps.
    for i in range(1, len(samples)):
        if samples[i]["monotonic_ns"] <= samples[i - 1]["monotonic_ns"]:
            print(
                f"FATAL: non-monotonic timestamps at sample {i}",
                file=sys.stderr,
            )
            sys.exit(1)

    # All samples must reference the same PID.
    pids = {s["pid"] for s in samples}
    if len(pids) > 1:
        print(
            f"FATAL: samples reference multiple PIDs: {pids}",
            file=sys.stderr,
        )
        sys.exit(1)

    # Last sample must occur after 80% of requested duration.
    last_sample_ns = samples[-1]["monotonic_ns"]
    elapsed_at_last = (last_sample_ns - workload_started_ns) / 1e9
    if elapsed_at_last < requested_secs * 0.8:
        print(
            f"FATAL: last sample at {elapsed_at_last:.1f}s, "
            f"need >= {requested_secs * 0.8:.1f}s (80% of {requested_secs:.0f}s)",
            file=sys.stderr,
        )
        sys.exit(1)

    # Step 10: Write artifacts metadata.
    artifact_list = [
        {"name": "sustained-summary.json", "role": "sustained-summary"},
        {"name": "resource-samples.jsonl", "role": "resource-samples"},
        {"name": "workload-stdout.txt", "role": "workload-stdout"},
        {"name": "workload-stderr.txt", "role": "workload-stderr"},
    ]
    with artifacts_path.open("w", encoding="utf-8") as f:
        json.dump(artifact_list, f, indent=2, sort_keys=True)
        f.write("\n")

    # Step 11: Write candidate metadata.
    completed_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    candidate = {
        "schema_version": 1,
        "stage": "mixed-fleet-sustained",
        "requested_duration_secs": int(requested_secs),
        "observed_duration_secs": round(observed_secs, 2),
        "resource_sample_count": len(samples),
        "completed_generations": summary.get("completed_generations", 0),
        "note": f"requested_duration_secs={int(requested_secs)} "
        f"observed_duration_secs={round(observed_secs, 2)} "
        f"resource_sample_count={len(samples)} "
        f"completed_generations={summary.get('completed_generations', 0)}",
    }
    with candidate_path.open("w", encoding="utf-8") as f:
        json.dump(candidate, f, indent=2, sort_keys=True)
        f.write("\n")

    print(f"\nEvidence written to {evidence_dir}/")
    print(f"  Summary: {summary.get('completed_generations', 0)} generations")
    print(f"  Samples: {len(samples)}")
    print(f"  Transitions: {summary.get('observed_transitions', [])}")
    print(f"  Duration: {observed_secs:.1f}s")
    print("\nSustained workload evidence PASSED.")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run sustained mixed-fleet workload for release evidence"
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

    run_sustained(
        duration_secs=args.duration_seconds,
        sample_interval_secs=args.sample_interval_seconds,
        evidence_dir=args.evidence_dir,
        profile=args.profile,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
