# Plan 092: Bugs audit corrective pass

Status: implementation in progress.

Depends on: Plan 091 and the confirmed findings recorded in the 2026-08-31
`bugs.md` audit.

## Objective

Close the actionable correctness findings from the audit with minimal,
behavior-preserving changes:

1. canonicalize IPv6 zone identifiers into URL-safe endpoint hosts;
2. classify common resolver lookup failures as DNS failures for both client
   polling paths;
3. make zero-port validation express the actual `u16` boundary;
4. treat future-dated snapshots as fresh when the wall clock moves backward;
5. document the guarded endpoint split precondition with a debug assertion.

## Acceptance criteria

- Each listed behavior has focused deterministic regression coverage.
- Endpoint, poller, EggPool, and daemon staleness documentation remains
  accurate for the changed user-visible behavior.
- `bugs.md` is removed after the findings are fixed.
- Formatting, focused tests, the default workspace check, all-feature checks,
  and clippy with warnings denied pass.
- This plan records the implementation commit when closed.

## Preserved exclusions

- No concurrent daemon-config locking redesign; the audit labels that race a
  low-probability edge and the daemon's atomic-write contract does not promise
  serialized multi-process mutations.
- No v2 fallback policy change, scheduler redesign, drive-worker change,
  protocol schema change, dependency, workflow, release, or feature addition.
- No changes to accepted `gregg add` forms beyond canonicalizing an already
  accepted IPv6 zone identifier for HTTP transport.

