# Plan 094: Bugs audit corrective pass

Status: implementation in progress.

Depends on: Plan 093 and the actionable findings recorded in `bugs.md`.

## Objective

Close every actionable finding in the supplied audit with minimal,
behavior-preserving changes:

1. return listener and lock-directory errors from reusable code;
2. reject non-IPv6 bracketed endpoint hosts and stop URL helpers from masking
   host-normalization errors;
3. make DNS classification rely on typed connection/resolver evidence before
   retaining narrow message fallbacks;
4. keep drive-refresh worker panics from silently terminating refreshes;
5. document and enforce the synchronous config-mutation boundary outside the
   Tokio runtime; and
6. document and test the scheduler's single-step generation wrap invariant.

## Acceptance criteria

- BUG-01 through BUG-08 are either fixed or have a focused, explicit
  behavior-preserving resolution.
- Focused regression coverage exists for each practical runtime/input finding.
- `bugs.md` is deleted after all actionable findings are addressed.
- Formatting, workspace tests, all-target/all-feature tests, and strict clippy
  pass.

## Preserved exclusions

- No new product feature, protocol change, scheduler redesign, worker restart
  architecture, dependency, or release automation.
- Performance-only observations OPT-01 through OPT-07 remain out of scope.
- The drive worker remains detached on drop so an uninterruptible filesystem
  call cannot block daemon shutdown; collector panics are contained and
  reported while the worker continues.

## Closure record

Pending verification and implementation commit.
