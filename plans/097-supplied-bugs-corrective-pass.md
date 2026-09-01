# Plan 097: Supplied bugs corrective pass

Status: complete. Closing record below.

Implementation landed in commit `2601fb5`.

Depends on: Plan 096 and the supplied `bugs.md` audit.

## Objective

Address every actionable bug in the supplied audit with the smallest
behavior-preserving change: harden control-socket publication and config
temporary files, remove production panic paths, keep TUI refreshes responsive,
make timer and endpoint behavior consistent, and correct the identified
state, validation, rendering, and collector maintainability defects.

## Acceptance criteria

- Each of BUG-01 through BUG-18 is fixed with focused coverage or an explicit
  code/documentation resolution where it is a documentation-only robustness
  observation.
- The seven optimization-only suggestions remain out of scope.
- No new product feature, protocol version, dependency, scheduler redesign,
  or release automation is introduced.
- `bugs.md` is deleted only after all actionable findings are addressed.
- Formatting, workspace tests, all-target/all-feature tests, and strict clippy
  pass.

## Preserved exclusions

- The bounded scheduler channel and ordered replacement-delivery invariant
  remain intact; responsiveness is corrected by moving delivery waiting out
  of the event-loop command path.
- Existing public configuration and endpoint wire formats remain unchanged.
- Performance-only optimizations, speculative generation-type redesign, and
  fallback-socket deployment guidance that does not change runtime behavior
  are resolved by rationale/documentation rather than new infrastructure.
- EggPool's zone-ID URL observation is explicitly retained as a pinned
  `url`/`reqwest` representation limitation: the systems poller accepts and
  transports zone IDs, while EggPool reports `InvalidEndpoint` rather than
  attempting an unsafe or incomplete alternate transport.

## Closure record

Completed:

- replaced control-socket check-then-rename publication with exclusive final
  `bind` and immediate permission verification;
- propagated HTTP-client, input-thread runtime, and TOML serialization errors
  through the production boundaries;
- added `O_NOFOLLOW`/descriptor checks to Unix config temp files, reaped stale
  temp files on client startup and daemon writes, and made cleanup failures
  visible to callers;
- kept Systems endpoint replacement ordered and reliable while moving full
  channel waiting out of the TUI event-loop action path;
- aligned fake-clock timer deadlines, state page/index fallbacks, endpoint
  diagnostics, truncation helpers, control error severity, host normalization,
  renderer parameters, and collector percentage finalization; and
- recorded the fallback-socket and header-truncation resolutions in code and
  documentation, with the pinned EggPool zone-authority limitation explicit.

Verification passed:

- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 888 passed, 2 ignored;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `cargo test -p greggd --all-features -- collector::linux` — 43 passed; and
- `cargo fmt --all -- --check` and `git diff --check`.

The supplied `bugs.md` audit is deleted after this closure record. The seven
optimization-only suggestions remain out of scope.
