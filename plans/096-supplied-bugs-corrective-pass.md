# Plan 096: Supplied bugs corrective pass

Status: complete. Closing record below.

Depends on: Plan 095 and the supplied `bugs.md` audit.

## Objective

Close the actionable current-code findings in the supplied audit with small,
behavior-preserving fixes:

1. preserve existing daemon directory permissions without a mode-bit
   pre-check or directory-creation race;
2. retry a panicked drive refresh promptly with bounded backoff and keep
   editor/config durability barriers cross-platform;
3. compare equivalent endpoint spellings canonically, measure v2-to-v1
   fallback latency as one poll, and normalize EggPool IPv6 authorities;
4. classify typed resolver errors including temporary failures while avoiding
   proxy diagnostics, and keep sampler/worker failure cleanup typed and
   bounded; and
5. use the platform access check for Unix editor discovery.

## Acceptance criteria

- Each actionable finding in `bugs.md` has a focused regression test or an
  explicit documentation/code resolution.
- No optimization-only suggestions, unreachable validation paths, explicit
  non-bugs, or coverage-only gaps are added to scope.
- `bugs.md` is deleted after the fixes are verified.
- Formatting, workspace tests, all-target/all-feature tests, and strict clippy
  pass.

## Preserved exclusions

- No new product feature, dependency, protocol version, scheduler redesign,
  release automation, or platform-specific test infrastructure.
- The existing pre-epoch age-policy behavior remains: age-based staleness is
  conservative until the clock is corrected; only hot-path log severity is
  adjusted.
- The existing byte-format output and protocol validation boundaries remain
  unchanged; BUG-11 and BUG-12 receive code-level rationale rather than
  speculative behavior changes, and BUG-15 is not a product defect.

## Closure record

Implementation landed in commit `de7ef0d`.

Completed:

- replaced the daemon's racy mode-bit pre-check with component-wise private
  directory creation that leaves existing operator-managed modes untouched;
- contained drive-refresh panics, returned a typed failure, and retried with
  bounded exponential backoff;
- made editor temp-file syncing cross-platform and distinguished cancelled
  sampler tasks from panics in diagnostics;
- canonicalized endpoint comparisons, included both legs in v2-to-v1 latency,
  classified `EAI_AGAIN` and proxy errors correctly, and normalized EggPool
  host authorities with a typed invalid-endpoint result when the pinned URL
  parser cannot represent a zone literal;
- made bracketed IPv6 diagnostics consistent, cleaned cancellation request
  ownership, used `access(X_OK)` for Unix editor discovery, and documented the
  byte-format boundary rationale; and
- deleted the supplied `bugs.md` audit after addressing its actionable items.

Verification passed:

- `rtk ./scripts/check-local.sh` — formatting and workspace tests passed;
- `rtk cargo test --workspace --all-targets --all-features` — 888 passed,
  2 ignored;
- `RUSTFLAGS="-D warnings" rtk cargo clippy --workspace --all-targets
  --all-features -- -D warnings` — clean; and
- `rtk cargo test -p greggd --all-features -- collector::linux` — 43 passed.

BUG-11, BUG-12, BUG-15, optimization-only notes, and coverage-only gaps were
not product defects and remain explicitly out of scope.
