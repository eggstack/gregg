# Plan 092: Bugs audit corrective pass

Status: complete. Closing record below.

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

## Closure record

Implementation landed in commit `6efa52cc272e6d250eded684b5a662e69407313f`.

Completed:

- IPv6 zone identifiers are canonicalized to URL-safe `%25` form at endpoint
  parsing and URL construction boundaries;
- common resolver lookup messages are classified as DNS failures by both the
  Systems poller and EggPool path, while unrelated network errors remain so;
- client and daemon zero-port checks explicitly compare against `0`;
- future-dated snapshots remain serviceable when the wall clock moves
  backward;
- focused regressions, required documentation, and the low-risk endpoint
  precondition assertion were added, and `bugs.md` was deleted.

Verification passed:

- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 859 passed, 2 ignored;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- focused endpoint, poller, EggPool, config, and daemon server tests.
