# Plan 089: Bugs audit corrective pass

Status: in progress.

Depends on: completed Plan 088 and the confirmed findings recorded in the
2026-08-26 `bugs.md` audit.

## Objective

Close the actionable correctness and CI findings in the audit with minimal,
behavior-preserving changes:

1. never publish snapshots with a fabricated blank identity when identity
   collection fails;
2. make IPv6 zone-ID parsing and malformed-zone diagnostics deterministic;
3. surface rejected Systems config reloads while retaining last-known-good
   state;
4. keep pre-epoch wall-clock handling from falsely declaring cached snapshots
   stale;
5. calculate large byte ratios with widened integer scaling;
6. align daemon display-name validation with client control-character rules;
7. resolve the CI-blocking clippy diagnostics.

Performance observations and dependency-version duplication in the audit are
preserved as out of scope because they are not correctness defects.

## Acceptance criteria

- Each listed behavior has focused regression coverage where deterministic
  coverage is practical.
- `cargo fmt --all -- --check`, workspace tests, and clippy with warnings
  denied pass.
- Active documentation and the changelog describe changed user-visible
  reload, endpoint, or daemon-name diagnostics.
- `bugs.md` is removed after the findings are fixed.

## Preserved exclusions

- No scheduler, protocol, collector architecture, TUI redesign, dependency
  upgrade, workflow, release, or service-management changes.
- No implementation of the audit's accepted performance-only observations.
