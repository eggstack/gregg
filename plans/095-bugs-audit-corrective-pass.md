# Plan 095: Bugs audit corrective pass

Status: complete. Closing record below.

Depends on: Plan 094 and the actionable findings recorded in the supplied
`bugs.md` audit.

## Objective

Close the concrete correctness and reliability findings with minimal,
behavior-preserving changes:

1. make the drive-refresh regression test resilient to scheduler contention;
2. require complete non-ready health envelopes in both protocol versions;
3. reject blank wire identities and malformed IPv6 zone identifiers;
4. avoid duplicate-address diagnostics for invalid hosts;
5. keep typed DNS evidence authoritative for DNS classification without adding
   a logging dependency to the client;
6. keep scheduler and sampler failure paths typed and lock-efficient;
7. handle a pre-epoch system clock without publishing an invalid timestamp or
   treating an uncheckable cached snapshot as fresh.

The protocol documentation will explicitly state that deserialization allocates
owned strings before semantic validation and that untrusted callers must bound
the input before deserializing.

## Acceptance criteria

- Every practical correctness/reliability finding in `bugs.md` has a focused
  regression test or an explicit documentation resolution.
- Existing protocol, endpoint, scheduler, sampler, and server behavior remains
  unchanged outside the corrected invalid-input and failure cases.
- `bugs.md` is deleted after all actionable findings are addressed.
- Formatting, workspace tests, all-target/all-feature tests, and strict clippy
  pass.

## Preserved exclusions

- No new product feature, dependency, protocol version, scheduler redesign, or
  release automation.
- Speculative logical-core/timestamp upper bounds and the derived-default
  capability foot-gun remain unchanged for source compatibility.
- Allocation/performance-only suggestions and non-reproducible display
  fallback observations remain out of scope.

## Closure record

Implementation landed in commit `cfc1c84`.

Completed:

- bounded the drive-refresh regression wait so worker scheduling contention no
  longer creates a false failure;
- required categories for non-ready v1/v2 health responses and rejected
  whitespace-only protocol identities;
- rejected empty URL-escaped IPv6 zones and kept malformed hosts out of
  duplicate-address indexing;
- converted closed scheduler semaphores to typed cancellation, consolidated
  sampler collector access, and handled pre-epoch clocks without publishing
  zero timestamps or serving age-uncheckable cached data as fresh; and
- documented the protocol's deserialize-before-validate allocation boundary.

Verification passed:

- `./scripts/check-local.sh`;
- `cargo test --workspace --all-targets --all-features` — 882 passed, 2 ignored;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`; and
- `cargo test -p greggd --all-features -- collector::linux` — 43 passed.

The supplied `bugs.md` was deleted after the actionable findings were
addressed. DNS classification retained the existing typed resolver evidence
and narrow compatibility fallbacks; no logging dependency was added to the
client.
