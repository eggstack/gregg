# Plan 094: Bugs audit corrective pass

Status: complete. Closing record below.

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

Implementation landed in commit `4930377`.

Completed:

- reusable listener and lock-parent errors now return through typed boundaries;
- bracketed endpoints require IPv6 literals, URL construction propagates host
  normalization failures, and DNS classification uses typed connect/resolver
  evidence with narrow compatibility fallbacks;
- drive collector panics are contained and logged so the refresh worker
  continues; synchronous config mutation remains outside the Tokio runtime;
- the generation wrap contract is documented and its skipped-wrap rejection
  is covered by a regression test; and
- `bugs.md` was deleted after all eight actionable findings were addressed.

Verification passed:

- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 873 passed, 2 ignored;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`; and
- `cargo test -p greggd --all-features -- collector::linux` — 43 passed.
