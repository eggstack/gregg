# Plan 090: Bugs audit corrective pass

Status: complete. Closing record below.

Depends on: completed Plan 089 and the confirmed findings recorded in the
2026-08-27 `bugs.md` audit.

## Objective

Close the remaining actionable correctness findings with minimal,
behavior-preserving changes:

1. preserve configuration metadata errors instead of treating them as missing;
2. bound client request timeouts and normalize endpoint-address comparisons;
3. reject incomplete v2 capability objects;
4. bound protocol identity fields and require categories on failed v1 health;
5. use typed DNS error classification;
6. keep EggPool refresh deadlines on the injected clock;
7. preserve permissions on operator-managed daemon config directories.

## Acceptance criteria

- Each listed behavior has focused regression coverage where deterministic
  coverage is practical.
- Protocol and user-facing configuration documentation describe the new
  bounds and required v2 capability fields.
- `bugs.md` is removed after the findings are fixed.
- Formatting, focused tests, the default workspace check, all-feature checks,
  and clippy with warnings denied pass.
- This plan records the implementation commit when closed.

## Preserved exclusions

- No scheduler, collector, TUI, daemon lifecycle, dependency, workflow,
  release, or protocol schema-major redesign.
- No implementation of performance-only observations, dead-code cleanup, or
  documented false positives from the audit.

## Closure record

Implementation landed in commit `8193643`.

Completed:

- configuration metadata errors now propagate, client request timeouts are
  bounded, and endpoint deduplication uses ASCII case folding;
- v2 capability objects require all four explicit flags, protocol identity
  fields are bounded to 512 UTF-8 bytes, and failed v1 health responses require
  a category;
- poller and EggPool error classification no longer relies on fragile display
  strings, and EggPool refresh deadlines honor the clock abstraction;
- existing daemon config-directory permissions are preserved while newly
  created directories remain private;
- focused regression coverage and all affected documentation were updated;
- `bugs.md` was deleted as requested.

Verification passed:

- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 852 passed, 2 ignored;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- focused protocol, client, and daemon test suites.
