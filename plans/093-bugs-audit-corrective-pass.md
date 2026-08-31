# Plan 093: Bugs audit corrective pass

Status: complete. Closing record below.

Depends on: Plan 092 and the confirmed findings recorded in the 2026-08-31
`bugs.md` audit.

## Objective

Close the actionable correctness findings in the audit with minimal,
behavior-preserving changes:

1. bound direct scheduler refresh intervals and preserve state across DNS host
   case changes;
2. deliver every completed drive refresh and fail closed on malformed warm
   samples;
3. normalize endpoint/config hosts consistently, including IPv6 zone IDs,
   bracket handling, default URL ports, DNS diagnostics, and config paths;
4. make configuration renames durable on Windows as well as Unix;
5. keep byte ratios and human-readable byte formatting numerically stable at
   large-unit boundaries.

## Acceptance criteria

- Each actionable behavior has focused deterministic regression coverage where
  practical.
- The default workspace check, all-target/all-feature tests, and clippy with
  warnings denied pass.
- The existing platform target checks continue to compile where the targets
  are available locally.
- `bugs.md` is removed after the findings are fixed.

## Preserved exclusions

- No scheduler redesign, worker pool, TUI cache redesign, protocol change,
  dependency, workflow, release, or feature addition.
- Performance-only observations O-01 through O-10 remain out of scope.
- BUG-11 is not changed because the current macOS cache creates its source
  clone once, when the cache is initialized, rather than once per refresh; a
  source-sharing redesign would expand the test-only mutation API.
- BUG-16 is an explicitly documented inefficiency rather than a correctness
  defect and remains out of scope.

## Closure record

Implementation landed in commit `01c9c53`.

Completed:

- direct scheduler intervals, DNS-host reconciliation, drive refresh delivery,
  malformed warm samples, endpoint/config normalization, URL default-port
  preservation, DNS diagnostics, and bracket handling are corrected;
- client and daemon atomic config writes sync the parent directory on Unix and
  Windows, including the bare-path fallback;
- large byte ratios and display formatting avoid precision and unit-boundary
  artifacts;
- focused regressions, the default check, all-target/all-feature tests, and
  strict clippy passed, and `bugs.md` was removed.

Verification passed:

- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 868 passed, 2 ignored;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- Windows/MSVC and macOS target checks were attempted but could not reach
  crate compilation because this Linux host lacks the required cross C
  compilers; the `ring` build stopped on target-specific compiler flags.
