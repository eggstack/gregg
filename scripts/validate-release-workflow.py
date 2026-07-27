#!/usr/bin/env python3
"""Static graph-based validation for the release workflow's dispatch DAG and dependency closure."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

try:
    import yaml

    HAS_YAML = True
except ImportError:
    HAS_YAML = False


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "release-candidate.yml"
FINALIZE = ROOT / ".github" / "workflows" / "release-finalize.yml"
REQUIREMENTS = ROOT / "plans" / "evidence" / "release-requirements.json"
DISPATCH_CONTRACT = ROOT / "plans" / "evidence" / "release-dispatch-contract.json"


class WorkflowViolation:
    """A single violation found during validation."""

    def __init__(self, category: str, message: str, *, job: str | None = None, stage: str | None = None) -> None:
        self.category = category
        self.message = message
        self.job = job
        self.stage = stage

    def __str__(self) -> str:
        parts = [f"[{self.category}]"]
        if self.stage:
            parts.append(f"stage={self.stage}")
        if self.job:
            parts.append(f"job={self.job}")
        parts.append(self.message)
        return " ".join(parts)


def _parse_if_condition(if_str: str) -> dict[str, Any]:
    """Parse a constrained subset of GitHub Actions 'if' expressions.

    Returns a dict with keys like 'stage', 'needs_results', 'unconditional', 'always'.
    Only the patterns used in this repository are supported; unsupported patterns
    cause the expression to be treated as unreachable (fail closed).
    """
    result: dict[str, Any] = {
        "stage": None,
        "needs_results": {},
        "unconditional": False,
        "always": False,
        "supported": True,
    }

    expr = if_str.strip()
    if expr.startswith("${{"):
        expr = expr[3:]
    if expr.endswith("}}"):
        expr = expr[:-2]
    expr = expr.strip()

    if expr == "always()":
        result["always"] = True
        result["unconditional"] = True
        return result

    # Check for always() && ... pattern — strip it before unsupported check
    always_prefix = re.match(r"always\(\)\s*&&\s*(.*)", expr, re.DOTALL)
    if always_prefix:
        result["always"] = True
        expr = always_prefix.group(1).strip()

    # Detect unsupported expression patterns before partial parsing.
    # These patterns are not handled by the constrained parser and must fail closed.
    # Note: always() is handled above; we only flag other function calls or references.
    if re.search(r"github\.\w+", expr):
        result["supported"] = False
        return result
    # Detect any function call other than always()
    func_match = re.search(r"\b\w+\(", expr)
    if func_match and func_match.group() != "always(":
        result["supported"] = False
        return result

    # Extract stage condition: inputs.stage == 'value'
    stage_match = re.search(r"inputs\.stage\s*==\s*'([^']+)'", expr)
    if stage_match:
        result["stage"] = stage_match.group(1)

    # Also handle multiple stage conditions (OR)
    stage_matches = re.findall(r"inputs\.stage\s*==\s*'([^']+)'", expr)
    if len(stage_matches) > 1:
        result["stage"] = stage_matches  # list of alternatives

    # Extract needs results: needs.<job>.result == 'success'
    for m in re.finditer(r"needs\.([a-zA-Z0-9_-]+)\.result\s*==\s*'([^']+)'", expr):
        result["needs_results"][m.group(1)] = m.group(2)

    # Check for unconditional (no stage condition at all)
    if "inputs.stage" not in expr:
        result["unconditional"] = True

    # An expression that is not unconditional, not always, and has no stage condition
    # contains conditions we cannot parse — fail closed.
    if (
        not result["unconditional"]
        and not result["always"]
        and not result["stage"]
    ):
        result["supported"] = False

    return result


def _extract_dispatch_options(workflow_text: str) -> list[str]:
    """Extract the ordered list of dispatch stage options from the workflow."""
    options = []
    in_options = False
    for line in workflow_text.splitlines():
        stripped = line.strip()
        if stripped == "options:" and "stage" in workflow_text[:workflow_text.index(line) + len(line)][-200:]:
            in_options = True
            continue
        if in_options:
            if stripped.startswith("- "):
                options.append(stripped[2:].strip().strip("'\""))
            elif stripped and not stripped.startswith("#"):
                break
    return options


def _extract_jobs(workflow_data: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Extract job definitions with their needs and if conditions."""
    jobs = {}
    for job_name, job_def in workflow_data.get("jobs", {}).items():
        needs = job_def.get("needs", [])
        if isinstance(needs, str):
            needs = [needs]
        if_cond = job_def.get("if", "")
        parsed = _parse_if_condition(str(if_cond))
        jobs[job_name] = {
            "needs": needs,
            "if_raw": str(if_cond),
            "if_parsed": parsed,
            "strategy": job_def.get("strategy"),
        }
    return jobs


def _jobs_reachable_for_stage(job_name: str, jobs: dict[str, dict[str, Any]], stage: str) -> bool:
    """Check if a job would be selected to run for a given dispatch stage."""
    if job_name not in jobs:
        return False

    job = jobs[job_name]
    cond = job["if_parsed"]

    if cond["unconditional"]:
        return True

    # Check if the stage matches
    job_stage = cond["stage"]
    if job_stage is None:
        # No stage condition - if unconditional or always, check
        if cond["always"]:
            return True
        return cond["unconditional"]

    if isinstance(job_stage, list):
        if stage in job_stage:
            return True
    elif job_stage == stage:
        return True

    return False


def _check_dependency_closure(
    job_name: str,
    stage: str,
    jobs: dict[str, dict[str, Any]],
    visited: set[str],
    path: list[str],
) -> list[WorkflowViolation]:
    """Recursively validate dependency closure for a job under a dispatch stage.

    Returns violations if:
    - A dependency is not reachable for the same stage
    - A dependency requires success from a skipped job
    - Cycles exist
    - References to nonexistent jobs
    - Unsupported expression patterns
    """
    violations: list[WorkflowViolation] = []

    if job_name in visited:
        cycle = path[path.index(job_name):] + [job_name]
        violations.append(WorkflowViolation(
            "cycle",
            f"dependency cycle detected: {' -> '.join(cycle)}",
            job=job_name,
            stage=stage,
        ))
        return violations

    if job_name not in jobs:
        violations.append(WorkflowViolation(
            "missing-job",
            f"references nonexistent job '{job_name}'",
            job=job_name,
            stage=stage,
        ))
        return violations

    visited.add(job_name)
    path.append(job_name)

    job = jobs[job_name]
    cond = job["if_parsed"]

    # Check if the job's condition is supported
    if not cond["supported"]:
        violations.append(WorkflowViolation(
            "unsupported-expression",
            f"job has unsupported 'if' expression: {job['if_raw']!r}",
            job=job_name,
            stage=stage,
        ))

    for dep_name in job["needs"]:
        # Check if the dependency exists at all
        if dep_name not in jobs:
            violations.append(WorkflowViolation(
                "missing-job",
                f"references nonexistent job '{dep_name}'",
                job=job_name,
                stage=stage,
            ))
            continue

        dep_reachable = _jobs_reachable_for_stage(dep_name, jobs, stage)

        if not dep_reachable:
            # Check if the current job requires this dependency to succeed
            dep_result = cond["needs_results"].get(dep_name)
            if dep_result == "success":
                violations.append(WorkflowViolation(
                    "skipped-required-dependency",
                    f"job requires needs.{dep_name}.result == 'success' but "
                    f"'{dep_name}' is not reachable for stage '{stage}'",
                    job=job_name,
                    stage=stage,
                ))
            # Don't recurse into unreachable dependencies
            continue

        # Recurse into the dependency
        dep_violations = _check_dependency_closure(dep_name, stage, jobs, visited.copy(), path.copy())
        violations.extend(dep_violations)

    path.pop()
    visited.discard(job_name)

    return violations


def _build_job_stage_map(workflow_data: dict[str, Any]) -> dict[str, list[str]]:
    """Build a mapping from job name to the list of evidence stage names it produces.

    This is derived from the workflow YAML: each job's ``--stage`` argument to
    ``validate-release-evidence.py write-candidate`` identifies the stage(s) it
    emits.  Matrix jobs expand the ``${{ matrix.<field> }}`` placeholder.
    """
    jobs = workflow_data.get("jobs", {})
    stage_map: dict[str, list[str]] = {}

    for job_name, job_def in jobs.items():
        stages: list[str] = []
        strategy = job_def.get("strategy", {})
        matrix = strategy.get("matrix", {}) if strategy else {}
        includes = matrix.get("include", [])

        steps = job_def.get("steps", [])
        for step in steps:
            run_text = step.get("run", "")
            # Find --stage arguments in validate-release-evidence.py write-candidate invocations.
            # The value may be quoted (single or double) or unquoted.
            for m in re.finditer(
                r"--stage\s+(?:'([^']+)'|\"([^\"]+)\"|(\S+))", run_text
            ):
                raw = m.group(1) or m.group(2) or m.group(3)
                # Expand matrix placeholders: e.g. binary-prepublish-${{ matrix.crate }}
                field_match = re.search(r"\$\{\{\s*matrix\.(\w+)\s*\}\}", raw)
                if field_match:
                    field = field_match.group(1)
                    if includes:
                        # matrix.include: list of dicts with field values
                        for inc in includes:
                            if field in inc:
                                expanded = re.sub(
                                    r"\$\{\{\s*matrix\.\w+\s*\}\}",
                                    str(inc[field]),
                                    raw,
                                )
                                stages.append(expanded)
                    else:
                        # matrix.field: [v1, v2, ...] pattern
                        field_values = matrix.get(field, [])
                        if isinstance(field_values, list):
                            for val in field_values:
                                expanded = re.sub(
                                    r"\$\{\{\s*matrix\.\w+\s*\}\}",
                                    str(val),
                                    raw,
                                )
                                stages.append(expanded)
                        else:
                            stages.append(raw)
                else:
                    stages.append(raw)

        stage_map[job_name] = sorted(set(stages)) if stages else []

    return stage_map


def _validate_stage_contract(
    dispatch_options: list[str],
    jobs: dict[str, dict[str, Any]],
    workflow_data: dict[str, Any],
    requirements: dict[str, Any],
) -> list[WorkflowViolation]:
    """Validate consistency between dispatch options, jobs, and the requirements contract."""
    violations: list[WorkflowViolation] = []

    # Every dispatch option must have at least one producing job
    for option in dispatch_options:
        producers = [
            name for name, job in jobs.items()
            if _jobs_reachable_for_stage(name, jobs, option)
            and name != "resolve"
        ]
        if not producers:
            violations.append(WorkflowViolation(
                "no-producer",
                f"dispatch option '{option}' has no producing job",
                stage=option,
            ))

    # Map jobs to the evidence stages they produce
    job_stage_map = _build_job_stage_map(workflow_data)

    # Build reverse map: stage name -> list of jobs that produce it
    stage_producers: dict[str, list[str]] = {}
    for job_name, stages in job_stage_map.items():
        for stage_name in stages:
            stage_producers.setdefault(stage_name, []).append(job_name)

    # Check required stages have at least one producer
    required_stages = requirements.get("pre_tag_required_stages", [])
    for stage_name in required_stages:
        if stage_name not in stage_producers:
            violations.append(WorkflowViolation(
                "missing-required-stage",
                f"required stage '{stage_name}' has no producing job in the workflow",
                stage=stage_name,
            ))

    # Check every produced stage is in the requirements (unless auxiliary)
    # "Auxiliary" stages are those that a matrix job emits as side-effects
    # (e.g. systemd-lifecycle, launchd-lifecycle are operational-evidence sub-stages).
    required_set = set(required_stages)
    # Also include final_required_stages — jobs that run in the finalize workflow
    # may produce stages only listed there.
    required_set.update(requirements.get("final_required_stages", []))
    for job_name, stages in job_stage_map.items():
        for stage_name in stages:
            if stage_name not in required_set:
                violations.append(WorkflowViolation(
                    "undocumented-stage",
                    f"job '{job_name}' produces stage '{stage_name}' not in "
                    f"release requirements contract",
                    job=job_name,
                    stage=stage_name,
                ))

    # Check for duplicate logical stage producers (same dispatch path, same stage)
    for stage_name, producers in stage_producers.items():
        if len(producers) > 1:
            # Determine which dispatch stages each producer is reachable from
            dispatch_overlap: dict[str, list[str]] = {}
            for producer in producers:
                for option in dispatch_options:
                    if _jobs_reachable_for_stage(producer, jobs, option):
                        dispatch_overlap.setdefault(option, []).append(producer)
            for option, overlapping in dispatch_overlap.items():
                if len(overlapping) > 1:
                    violations.append(WorkflowViolation(
                        "duplicate-stage-producer",
                        f"dispatch stage '{option}' has multiple jobs producing "
                        f"'{stage_name}': {', '.join(sorted(overlapping))}",
                        stage=option,
                    ))

    return violations


def _validate_dispatch_contract(
    dispatch_options: list[str],
    jobs: dict[str, dict[str, Any]],
    requirements: dict[str, Any],
) -> list[WorkflowViolation]:
    """Validate the machine-readable dispatch contract against the workflow and requirements."""
    violations: list[WorkflowViolation] = []

    if not DISPATCH_CONTRACT.is_file():
        violations.append(WorkflowViolation(
            "missing-contract",
            f"dispatch contract file not found: {DISPATCH_CONTRACT}",
        ))
        return violations

    try:
        contract = json.loads(DISPATCH_CONTRACT.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as error:
        violations.append(WorkflowViolation(
            "invalid-contract",
            f"cannot parse dispatch contract: {error}",
        ))
        return violations

    dispatches = contract.get("dispatches", {})
    if not isinstance(dispatches, dict) or not dispatches:
        violations.append(WorkflowViolation(
            "invalid-contract",
            "dispatch contract has no dispatches",
        ))
        return violations

    # Every dispatch option must have a contract entry.
    for option in dispatch_options:
        if option not in dispatches:
            violations.append(WorkflowViolation(
                "missing-contract-entry",
                f"dispatch option '{option}' has no entry in release-dispatch-contract.json",
                stage=option,
            ))

    # Every contract entry must correspond to a dispatch option.
    for entry_name in dispatches:
        if entry_name not in dispatch_options:
            violations.append(WorkflowViolation(
                "orphan-contract-entry",
                f"contract entry '{entry_name}' has no corresponding workflow dispatch option",
                stage=entry_name,
            ))

    # Validate each contract entry.
    required_stages_set = set(
        requirements.get("pre_tag_required_stages", [])
    )
    required_stages_set.update(requirements.get("final_required_stages", []))

    for entry_name, entry in dispatches.items():
        if not isinstance(entry, dict):
            violations.append(WorkflowViolation(
                "invalid-contract-entry",
                f"contract entry '{entry_name}' is not an object",
                stage=entry_name,
            ))
            continue

        # Validate required_stages.
        entry_stages = entry.get("required_stages", [])
        if not isinstance(entry_stages, list) or not entry_stages:
            violations.append(WorkflowViolation(
                "invalid-contract-entry",
                f"contract entry '{entry_name}' has no required_stages",
                stage=entry_name,
            ))
            continue

        for stage in entry_stages:
            if stage not in required_stages_set:
                violations.append(WorkflowViolation(
                    "unregistered-required-stage",
                    f"contract entry '{entry_name}' requires stage '{stage}' "
                    f"not found in release-requirements.json",
                    stage=entry_name,
                ))

        # Validate producer_jobs are reachable.
        producer_jobs = entry.get("producer_jobs", [])
        for job_name in producer_jobs:
            if job_name not in jobs:
                violations.append(WorkflowViolation(
                    "invalid-contract-entry",
                    f"contract entry '{entry_name}' references non-existent job '{job_name}'",
                    stage=entry_name,
                    job=job_name,
                ))

        # Validate aggregation type.
        agg = entry.get("aggregation", {})
        if not isinstance(agg, dict) or agg.get("type") not in ("current-run", "cross-run"):
            violations.append(WorkflowViolation(
                "invalid-contract-entry",
                f"contract entry '{entry_name}' has invalid aggregation type",
                stage=entry_name,
            ))

    return violations


def validate_workflow() -> list[WorkflowViolation]:
    """Run all validation checks and return violations."""
    if not HAS_YAML:
        print("WARNING: PyYAML not installed; skipping YAML-based validation", file=sys.stderr)
        return []

    violations: list[WorkflowViolation] = []

    text = WORKFLOW.read_text(encoding="utf-8")
    finalize_text = FINALIZE.read_text(encoding="utf-8")

    # --- Legacy substring checks (preserved from original validator) ---

    if "continue-on-error" in text:
        violations.append(WorkflowViolation("gate-weakness", "release workflow must not continue on gate errors"))

    if "GREGG_SYSTEMD_BINARY" in text or "GREGG_LAUNCHD_BINARY" in text or "GREGG_INSTALLED_GREGGD" in text:
        violations.append(WorkflowViolation("gate-weakness", "protected jobs must not consume arbitrary binary path inputs"))

    if 'cargo install --path "${package_dir}" --locked' not in text:
        violations.append(WorkflowViolation("security", "unpacked binary installation is not locked"))

    for line in text.splitlines():
        if "install-verified-package.sh" in line and "--candidate-sha" not in line:
            violations.append(WorkflowViolation("security", "package installation is not bound to the resolved candidate SHA"))
        if "install-verified-package.sh" in line and "--lockfile" not in line:
            violations.append(WorkflowViolation("security", "package installation does not require the verified lockfile"))
        if "verify-package-provenance.sh" in line and "--candidate-sha" not in line:
            violations.append(WorkflowViolation("security", "package provenance verification is not bound to the resolved candidate SHA"))

    if text.find("actions/checkout@v4", text.find("postpublish-verify")) == -1:
        violations.append(WorkflowViolation("ordering", "postpublish-verify has no checkout"))
    postpublish = text[text.find("postpublish-verify"):]
    if postpublish.find("actions/checkout@v4") > postpublish.find("scripts/verify-installed-daemon.sh"):
        violations.append(WorkflowViolation("ordering", "postpublish repository scripts are used before checkout"))

    for script in re.findall(r"(?:bash |python3 |\./|scripts/)(scripts/[A-Za-z0-9_./-]+\.(?:sh|py))", text):
        if not (ROOT / script).is_file():
            violations.append(WorkflowViolation("missing-script", f"workflow references missing repository script: {script}"))
    for script in re.findall(r"(?:bash |python3 |\./|scripts/)(scripts/[A-Za-z0-9_./-]+\.(?:sh|py))", finalize_text):
        if not (ROOT / script).is_file():
            violations.append(WorkflowViolation("missing-script", f"finalize workflow references missing repository script: {script}"))

    for manifest in (ROOT / "crates/greggd/Cargo.toml", ROOT / "crates/gregg/Cargo.toml"):
        if manifest.read_text(encoding="utf-8").count('gregg-protocol = { version = "1.0.1"') != 2:
            violations.append(WorkflowViolation("dependency-alignment", f"protocol dependency is not aligned in {manifest}"))

    if "|| true" in text:
        for i, line in enumerate(text.splitlines(), 1):
            if "|| true" in line and "cleanup" in line.lower():
                violations.append(WorkflowViolation("gate-weakness", f"release workflow masks cleanup failure at line {i}: {line.strip()}"))

    if "needs.binary-prepublish.result" in text and "native-package-evidence" in text:
        native_section = text[text.find("native-package-evidence"):]
        if "needs.binary-prepublish.result" in native_section[:2000]:
            violations.append(WorkflowViolation("gate-weakness", "native-package-evidence still depends on binary-prepublish result"))

    if "mixed-fleet-sustained" not in text:
        violations.append(WorkflowViolation("missing-stage", "mixed-fleet-sustained stage has no producing job in release-candidate workflow"))

    if 'ref: ${{ inputs.candidate_sha }}' not in finalize_text:
        violations.append(WorkflowViolation("ordering", "release-finalize.yml does not checkout immutable candidate SHA"))

    if "--mode" not in finalize_text:
        violations.append(WorkflowViolation("ordering", "release-finalize.yml does not pass --mode to aggregation"))

    # --- Graph-based DAG validation ---

    workflow_data = yaml.safe_load(text)
    dispatch_options = _extract_dispatch_options(text)
    jobs = _extract_jobs(workflow_data)

    # Stage/job truth table: for each dispatch option, what jobs must be reachable
    STAGE_TRUTH_TABLE: dict[str, list[str]] = {
        "protocol-prepublish": ["resolve", "source-ci", "protocol-prepublish"],
        "protocol-index-check": ["resolve", "protocol-index-check"],
        "binary-prepublish": ["resolve", "source-ci", "protocol-index-check", "binary-prepublish", "binary-msrv"],
        "native-evidence": ["resolve", "source-ci", "native-evidence", "native-package-evidence"],
        "mixed-fleet-client": ["resolve", "source-ci", "mixed-fleet-client"],
        "mixed-fleet-sustained": ["resolve", "source-ci", "mixed-fleet-sustained"],
        "operational-evidence": ["resolve", "operational-evidence", "operational-macos-evidence", "systemd-lifecycle", "launchd-lifecycle"],
        "postpublish-verify": ["resolve", "protocol-index-check", "postpublish-verify"],
    }

    for option in dispatch_options:
        if option not in STAGE_TRUTH_TABLE:
            violations.append(WorkflowViolation(
                "unknown-dispatch-option",
                f"dispatch option '{option}' is not in the stage/job truth table",
                stage=option,
            ))

    for stage, expected_jobs in STAGE_TRUTH_TABLE.items():
        for job_name in expected_jobs:
            if job_name not in jobs:
                violations.append(WorkflowViolation(
                    "missing-job-in-truth-table",
                    f"truth table references nonexistent job '{job_name}' for stage '{stage}'",
                    job=job_name,
                    stage=stage,
                ))

    # Validate dependency closure for each dispatch stage
    for stage in dispatch_options:
        reachable_for_stage = {name for name in jobs if _jobs_reachable_for_stage(name, jobs, stage)}
        for job_name in reachable_for_stage:
            visited: set[str] = set()
            dep_violations = _check_dependency_closure(job_name, stage, jobs, visited, [])
            violations.extend(dep_violations)

    # Validate requirements contract consistency
    requirements = json.loads(REQUIREMENTS.read_text(encoding="utf-8"))
    contract_violations = _validate_stage_contract(dispatch_options, jobs, workflow_data, requirements)
    violations.extend(contract_violations)

    # Validate machine-readable dispatch contract
    dispatch_contract_violations = _validate_dispatch_contract(dispatch_options, jobs, requirements)
    violations.extend(dispatch_contract_violations)

    return violations


def main() -> int:
    violations = validate_workflow()

    if violations:
        for v in violations:
            print(f"FATAL: {v}", file=sys.stderr)
        return 1

    print("release workflow graph-based validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
