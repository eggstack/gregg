# Phase 64: status, protocol, and package correctness

Status: planned.

## Objective

Correct the two remaining product defects identified by the August 2026 review and remove adjacent package/dead-code inaccuracies without changing Gregg's features or architecture.

This phase owns only:

- v2 snapshot age/staleness correctness, especially for the Windows v2-only daemon;
- strict schema parsing tied to the requested status endpoint;
- removal of the test-only `lock_helper` binary from normal package/install output;
- removal of confirmed unreachable config-reload reducer machinery;
- correction of active documentation affected by these items.

This is a corrective phase, not a protocol redesign or platform expansion.

## Dependencies and execution position

Depends on the repository state after completed Plans 036-062 and on Roadmap 063.

Phase 65 must not begin production cleanup until the Phase 64 regression tests pass. Phase 64 does not depend on Phase 65's local/CI simplification.

```text
63 -> 64 -> 65
```

## Defect 1: Windows v2 snapshot age is not the staleness source

### Current behavior

`ServerState` stores v1 and v2 snapshots separately. The age branch of `is_snapshot_stale()` reads only the v1 snapshot timestamp.

Linux and macOS normally publish both v1 and v2, so the v1 timestamp exists. Windows publishes only v2 because load average and swap cannot be truthfully represented in v1. The Windows v1 slot remains empty.

With the current server configuration:

```text
max_consecutive_failures = 0
max_snapshot_age = stale_after_ms
```

failure-count expiration is disabled. If collection fails after a valid Windows v2 snapshot, the age test has no v1 timestamp to inspect. `/v2/status` may therefore continue serving the old snapshot with `200 OK` after the configured age limit.

### Required behavior

The server must derive staleness from the latest published observation timestamp independent of wire version.

Acceptable small implementations, in preference order:

1. Store one `last_observed_at_unix_ms: Option<u64>` in `ServerState` and update it whenever either snapshot form is published.
2. Read the v1 timestamp when present and otherwise read the v2 timestamp.
3. Store one small immutable publication record containing both optional snapshots and one timestamp.

Choose the smallest diff that avoids divergent v1/v2 age behavior. Do not perform a broad server-state/locking rewrite in this phase.

### Required route behavior

For a configured nonzero `stale_after_ms`:

```text
fresh v1 snapshot -> /v1/status 200
fresh v2 snapshot -> /v2/status 200
stale v1 snapshot -> /v1/status 503
stale v2 snapshot -> /v2/status 503
```

On Windows/v2-only publication:

- `/v2/status` returns `200` while fresh;
- `/v2/status` returns `503` after the observation age reaches the configured threshold;
- `/v2/healthz` returns `503` when stale;
- v1 routes continue to report their existing unsupported/unavailable behavior rather than fabricating v1 metrics.

Do not add a Windows-specific timeout, special timer, or background invalidation task. Staleness remains request-evaluated from the configured policy.

### Failure-state preservation

A collector failure may preserve the most recent snapshot until it becomes stale. This existing behavior remains valid. The correction is only that v2-only data must age out under the same policy.

### Focused tests

Add deterministic server-state or handler tests covering:

1. v2-only snapshot is fresh before the age threshold;
2. the same v2-only snapshot is stale at or after the threshold;
3. `/v2/status` returns `503` for stale v2-only state;
4. `/v2/healthz` returns `503` for stale v2-only state;
5. a later successful v2 publication resets the observation timestamp and becomes fresh;
6. dual v1/v2 publication retains existing Linux/macOS behavior;
7. `max_snapshot_age == Duration::ZERO` continues to disable age expiration.

Prefer injecting or directly supplying `now_unix_ms` to the private staleness helper rather than sleeping in tests. Do not add a clock framework to the server unless a tiny parameterized helper is insufficient.

### Workstream A acceptance criteria

- [x] Staleness no longer depends on the presence of a v1 snapshot.
- [x] Windows v2-only snapshots age out under `stale_after_ms`.
- [x] Linux/macOS dual-snapshot behavior is unchanged.
- [x] Disabled staleness remains disabled.
- [x] No platform-specific invalidation task or new runtime component is added.

## Defect 2: endpoint responses are not parsed against an expected schema

### Current behavior

The poller requests `/v2/status` first. `poll_single_url()` then calls a shared parser that attempts to deserialize v2 and, if that fails, attempts v1. The same parser is also used for `/v1/status`.

Consequences:

- a valid v1 payload returned with `200 OK` from `/v2/status` can be accepted as online v1 data;
- a valid v2 payload returned from `/v1/status` can be accepted as v2 data;
- endpoint contract violations are hidden instead of classified as invalid or unsupported responses.

### Required behavior

Bind parsing to the endpoint requested.

Use a small internal discriminator such as:

```rust
enum ExpectedSchema {
    V1,
    V2,
}
```

or two narrowly named parsing functions:

```text
parse_v1_response
parse_v2_response
```

Required negotiation:

```text
GET /v2/status
  2xx + valid v2 -> OnlineV2
  404            -> GET /v1/status
  other status   -> HttpStatus
  2xx + v1 body  -> invalid/unsupported v2 response; no fallback
  malformed body -> DecodeError or InvalidSnapshot; no fallback

GET /v1/status after v2 404
  2xx + valid v1 -> Online
  2xx + v2 body  -> invalid/unsupported v1 response
  other status   -> HttpStatus
```

The only condition that triggers v1 fallback remains an HTTP 404 from `/v2/status`.

### Error classification

Do not add a large new taxonomy. Reuse the current stable outcomes where possible:

- valid JSON with the wrong schema version: `UnsupportedSchema`;
- JSON not decodable as the expected wire type: `DecodeError`;
- expected type decoded but invariant validation fails: `InvalidSnapshot`.

If Serde's structural mismatch makes a valid other-version body appear as `DecodeError`, that is acceptable. The key requirement is that it is not accepted and does not trigger fallback.

### Focused tests

Required cases:

1. `/v2/status` returns valid v2 and succeeds;
2. `/v2/status` returns 404, `/v1/status` returns valid v1, and fallback succeeds;
3. `/v2/status` returns valid v1 with 200 and is rejected without requesting `/v1/status`;
4. `/v2/status` returns malformed JSON and is rejected without fallback;
5. `/v2/status` returns invalid v2 and is rejected without fallback;
6. fallback `/v1/status` returns valid v2 with 200 and is rejected;
7. non-404 v2 status never requests v1;
8. IPv4, IPv6, and DNS URL construction remains unchanged.

The synthetic server should count or record requested paths so tests prove that forbidden fallback did not occur.

### Workstream B acceptance criteria

- [x] `/v2/status` accepts only v2.
- [x] `/v1/status` accepts only v1.
- [x] v1 fallback occurs only after v2 404.
- [x] malformed, invalid, and wrong-version v2 responses do not trigger fallback.
- [x] Existing body limits, redirect policy, timeouts, and validation remain unchanged.
- [x] No generalized content-negotiation or protocol registry is introduced.

## Packaging correction: `lock_helper` must be test-only

### Current behavior

`lock_helper` is declared as a normal `[[bin]]` target. Its source is explicitly a cross-process lock test helper. It is not part of the user-facing CLI.

### Required behavior

Normal packaging and installation expose only the intended `gregg` binary.

Preferred correction:

```toml
[features]
default = []
test-helper = []

[[bin]]
name = "lock_helper"
path = "src/bin/lock_helper.rs"
required-features = ["test-helper"]
```

The exact feature name may differ, but it must be private/documented as test-only. Relevant tests may enable the feature explicitly or use Cargo's test binary facilities if a smaller reliable approach exists.

Do not move cross-process locking into production code or remove the useful contention test solely to avoid the helper target.

### Package verification

Use:

```text
cargo package --list -p gregg
cargo install --path crates/gregg --locked --root <temporary-root>
```

Acceptance requires:

```text
<temporary-root>/bin/gregg
```

and no installed `lock_helper` executable.

The package source may still contain the helper source if needed for tests, but it must not become a normal installed binary.

### Workstream C acceptance criteria

- [x] `cargo install gregg` installs only `gregg`.
- [x] Cross-process lock tests remain available.
- [x] No user-visible helper command is documented.
- [x] No new helper crate is created.

## Dead-path cleanup: config reload

### Required investigation

Before editing, perform repository-wide searches for:

```text
ConfigReloaded
rebuild_from_config
config reload
reload config
```

Classify every caller as production, test, documentation, or historical plan text.

### Decision rule

If no production event source constructs `Action::ConfigReloaded`, remove:

- the action variant;
- `AppState::rebuild_from_config()`;
- tests that exist only for this unreachable path;
- active architecture documentation that claims live reload behavior.

Do not remove the CLI's normal read-edit-write configuration functionality.

If a real production caller exists, do not broaden this phase to complete hot reload. Document the caller in Phase 64 and limit changes to a demonstrated correctness issue. Any full live-reload implementation becomes separate future work.

### Explicit non-goals

Do not add:

- file-system watchers;
- SIGHUP configuration reload;
- scheduler endpoint replacement;
- EggPool worker migration;
- transient-state reconciliation frameworks;
- generic actions/effects infrastructure.

### Workstream D acceptance criteria

- [x] Repository search establishes whether the reload path is production-reachable.
- [x] If unreachable, the dead action/rebuild code and exclusive tests are deleted.
- [x] If reachable, no broad reload redesign is attempted.
- [x] Normal CLI configuration commands continue to work.

## Active documentation reconciliation

Correct only statements that disagree with current behavior or the Phase 64 implementation.

At minimum inspect:

```text
README.md
architecture/overview.md
architecture/gregg-client.md
architecture/greggd-daemon.md
architecture/collectors.md
architecture/protocol.md
architecture/scripts-and-packaging.md
AGENTS.md
crates/*/README.md
```

Known corrections from the review:

- macOS v2 swap capability must match the collector implementation;
- Windows drive eligibility text must match whether fixed and removable drives are accepted;
- Windows v1 route/health wording must match the actual response type and status behavior;
- status endpoint negotiation text must state that wrong-schema 200 responses are rejected;
- package documentation must not imply `lock_helper` is an installable command;
- config reload must not be described as active behavior if its code is removed.

Do not rewrite completed historical plans to modernize wording. Update Roadmap 063, Phase 64, Phase 65, and `plans/README.md` statuses only during implementation closure.

### Workstream E acceptance criteria

- [x] Active platform capability documentation matches code.
- [x] Active route documentation matches handlers and poller behavior.
- [x] No active document advertises the test helper.
- [x] Historical completed plans remain unchanged except concise status cross-references when required.

## Files likely to change

Expected narrow set:

```text
crates/greggd/src/server/mod.rs
crates/gregg/src/poller.rs
crates/gregg/src/action.rs
crates/gregg/src/state.rs
crates/gregg/Cargo.toml
crates/gregg/src/bin/lock_helper.rs
README.md
architecture/*.md
plans/063-narrow-correctness-and-simplification-roadmap.md
plans/064-status-protocol-package-correctness.md
plans/README.md
```

Tests may live in the existing modules or existing integration-test locations. Do not add a new harness directory unless current test placement cannot cover the behavior.

## Lightweight verification

During implementation use focused tests first:

```text
cargo fmt --all -- --check
cargo test -p greggd server
cargo test -p gregg poller
cargo test -p gregg config
cargo package --list -p gregg
```

Then run the existing ordinary workspace check once before handoff:

```text
cargo test --workspace
```

If the repository's current all-target/all-feature command is needed to compile the gated helper test, run it once. Do not add repeated platform-specific local reruns.

Hosted confirmation:

- one ordinary Windows job for native Windows compile/tests;
- existing macOS jobs for source compatibility;
- no dedicated Phase 64 workflow or artifact.

## Phase acceptance criteria

### Correctness

- [x] Windows v2-only snapshots honor age-based staleness.
- [x] `/v2/status` and `/v2/healthz` return stale status correctly.
- [x] New successful publication restores freshness.
- [x] `/v2/status` accepts only v2 payloads.
- [x] `/v1/status` accepts only v1 payloads.
- [x] Fallback is still exactly v2 404 -> v1.
- [x] Wrong-version, malformed, and invalid v2 responses do not fall back.

### Package and dead code

- [x] Normal `gregg` installation contains no `lock_helper` executable.
- [x] Cross-process lock contention coverage remains.
- [x] Unreachable config-reload machinery is removed, or a production caller is documented and no expansion is attempted.

### Documentation and scope

- [x] Active platform/route/package documentation matches implementation.
- [x] No new product feature, config field, protocol version, dependency, workflow, or evidence system is added.
- [x] No broad server-state, scheduler, supervision, or protocol refactor is included.

### Verification and closure

- [x] Focused regression tests pass.
- [x] `cargo package --list -p gregg` confirms package truth.
- [x] One ordinary workspace test pass succeeds.
- [x] One ordinary CI run succeeds at the implementation SHA or a source-equivalent descendant.
- [x] Phase 64 and Roadmap 063 status text is updated truthfully without a separate evidence file.

## Handoff notes

Implementation should be delivered as one or a small number of cohesive commits. Keep the staleness and parser tests near the production modules so later maintainers can see the contract directly.

Any discovery outside the listed defects is not part of Phase 64 unless it prevents these acceptance criteria. Record unrelated findings separately; do not create Phase 64 subphases.