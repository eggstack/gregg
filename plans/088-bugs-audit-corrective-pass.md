# Plan 088: Bugs audit corrective pass

Status: in progress.

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
