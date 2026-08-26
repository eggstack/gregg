# Plan 088: Bugs audit corrective pass

Status: complete. Closing record below.

Depends on: completed Plan 087 and the confirmed findings recorded in the
2026-08-26 `bugs.md` audit.

## Objective

Close the three confirmed actionable findings from the audit with the smallest
behavior-preserving changes:

1. make macOS byte-ratio percentages use the collector-wide normalization
   helper, including its non-finite narrowing behavior;
2. ensure a failure while awaiting non-Unix Ctrl-C is returned through the
   reusable daemon runtime error boundary instead of panicking;
3. use a dedicated configuration violation for rejecting a second EggPool
   endpoint without `--replace`.

The audit's informational observations, possible fixture race, routine-loop
coverage distinction, and accepted optimization trade-offs are not findings
for this corrective pass. No new product features, dependencies, workflows,
or service-management behavior are in scope.

## Acceptance criteria

- macOS normalization delegates to `collector::clamped_usage_pct`; its
  existing zero, ordinary, clamp, and overflow tests describe the shared
  behavior.
- The non-Unix shutdown future maps `tokio::signal::ctrl_c()` errors to its
  result, and the supervision path converts that result to an ordinary runtime
  error. Existing injected shutdown callers continue to work.
- Duplicate `gregg eggpool add` rejection without `--replace` yields a
  dedicated violation variant and retains the existing message intent; a test
  matches the violation kind.
- `bugs.md` is removed after all confirmed findings are fixed.
- Formatting, focused tests, the default workspace check, and the applicable
  full-feature checks pass.
- The active documentation and changelog describe any changed user-visible
  diagnostic or error-boundary behavior. This plan records the implementation
  commit when closed.

## Preserved exclusions

- No changes to the scheduler, TUI, protocol wire types, collectors beyond the
  shared macOS percentage call, or daemon lifecycle architecture.
- No implementation of the audit's observations or accepted optimizations.
- No release publication, tagging, workflow changes, or new dependencies.

## Closure record

Implementation landed in commit
`58b332b51021e3950fa14d8888a46ed6d069a687`.

Completed:

- macOS percentage normalization delegates to
  `collector::clamped_usage_pct`, preserving the shared finite/clamping rules;
- non-Unix Ctrl-C failures return through `RunOutcome::ShutdownError`;
- duplicate EggPool configuration uses `EggpoolAlreadyConfigured` and has a
  command-level regression test;
- `bugs.md` was deleted.

Verification passed:

- `cargo fmt --all -- --check`;
- `cargo test -p greggd --lib run`;
- `cargo test -p gregg --lib cli`;
- `cargo test -p gregg --lib config`;
- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 819 passed, 2 ignored;
- `cargo check -p greggd --target x86_64-apple-darwin --all-features`;
- `cargo check -p greggd --target x86_64-pc-windows-msvc --all-features`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
