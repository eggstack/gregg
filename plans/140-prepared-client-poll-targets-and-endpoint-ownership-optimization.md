# Plan 140: prepared client poll targets and endpoint ownership optimization

Status: complete.

Depends on: Plan 138 and the settled Plans 118-125 eggfetch polling contract.

## Objective

Reduce repeated endpoint string cloning and URL normalization in the Systems polling hot path while preserving `PollScheduler`, `HttpClient`, `PollResult`, endpoint ordering, panic isolation, cancellation, concurrency, protocol negotiation, and public APIs.

Current generations rebuild v1/v2 URLs for every endpoint and clone each `Endpoint` into the poll task, again into panic-recovery bookkeeping, and again when constructing the normal `PollResult`.

## Implementation

### 1. Add a private prepared-target representation

Introduce a scheduler-private/preparation-private record conceptually containing:

~~~text
PreparedTarget {
    endpoint
    v1_status_target
    v2_status_target
}
~~~

Prepare targets:

- when the scheduler starts;
- when `ReplaceEndpoints` installs a new endpoint list.

Do not rebuild them on ordinary periodic/manual generations.

The prepared URL fields may be `Arc<str>`, boxed strings, or another small private shared representation. Choose the smallest implementation that measurably/structurally avoids repeated normalization.

### 2. Preserve invalid-target behavior

Public `PollScheduler::new`, `run`, and `HttpClient::poll(&Endpoint)` callers may bypass normal config validation.

Do not turn an endpoint that currently yields a per-poll `NetworkError` into scheduler-construction failure merely because URL preparation moved earlier.

If target preparation can fail, retain the failure as prepared state and reproduce the current poll outcome on each generation.

### 3. Add an internal owned/prepared polling path

Keep public:

~~~text
HttpClient::poll(&Endpoint, &impl Clock) -> PollResult
~~~

logically unchanged.

Add a private/internal path used by the scheduler that accepts one generation-owned endpoint plus prepared v1/v2 targets and moves that endpoint into the resulting `PollResult`.

The normal successful path should require only the unavoidable stable-ID duplication imposed by the current public `PollResult { system_id, endpoint, ... }` shape, not repeated full Endpoint clones.

Do not change `PollResult` fields or make callers consume an `Arc<Endpoint>`.

### 4. Make panic recovery index-based

The scheduler currently keeps an additional cloned Endpoint beside each `JoinHandle` so a panicked task can synthesize the required Cancelled result.

Replace that ordinary-path clone with a stable endpoint index or equivalent lightweight recovery token. Only the exceptional panic branch may clone/reconstruct from the installed endpoint list.

Preserve:

- one result per configured endpoint;
- configured result order;
- synthetic Cancelled result on task panic;
- exact endpoint identity in that result.

### 5. Move retained state on config reconciliation

In `AppState::reconcile_systems`, consume/remove retained entries from the old-ID map instead of `get(...).cloned()` when the stable endpoint remains equivalent.

This avoids deep-cloning `NormalizedSnapshot` during reload while preserving every public field and reset rule.

Do not combine this with config schema or reload-semantics changes.

## Deterministic evidence

Add cfg(test) instrumentation or narrow helpers proving:

- URL normalization occurs once per installed endpoint list, not once per generation;
- manual/periodic generations reuse the same prepared targets;
- ReplaceEndpoints rebuilds exactly the replacement targets;
- invalid prepared target still maps to the same PollOutcome;
- v2 404 still performs v1 fallback against the prepared v1 target;
- task panic still yields exactly one Cancelled result in configured position;
- cancellation and semaphore ceiling remain unchanged;
- public `HttpClient::poll` tests remain green;
- config reconciliation retains snapshot/state by move and resets changed host/port exactly as before.

Avoid global allocator instrumentation.

## Measurement

Use the existing scheduler concurrency tests and an ad hoc release-mode synthetic fleet if useful. Record structural URL-build/clone reduction as the primary evidence.

Record stripped `gregg` size before/after and investigate unexplained growth above roughly 1%.

## Verification

~~~text
cargo test -p gregg --all-targets --all-features -- poller
cargo test -p gregg --all-targets --all-features -- scheduler
cargo test -p gregg --all-targets --all-features -- state
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

## Acceptance criteria

- [x] Periodic/manual generations reuse prepared v1/v2 targets.
- [x] ReplaceEndpoints atomically replaces endpoint and prepared-target state.
- [x] The ordinary poll path no longer clones the full Endpoint for both task execution and panic bookkeeping.
- [x] PollResult public shape and public HttpClient polling API are unchanged.
- [x] v2-first/v1-fallback semantics are unchanged.
- [x] Invalid target, timeout, body-limit, DNS/refused/network, and HTTP classifications are unchanged.
- [x] Endpoint ordering, generation, concurrency, cancellation, offline retry, and panic isolation are unchanged.
- [x] Config reconciliation moves retained state without changing reset/selection/viewport behavior.
- [x] No new dependency or scheduler architecture is introduced.
- [x] Focused tests, workspace gates, and Rust 1.89 remain green.

## Explicit non-goals

Do not include:

- FuturesUnordered/buffer_unordered scheduler replacement;
- changing `PollResult` to Arc-backed public data;
- endpoint backoff/suppression;
- connection-pool policy changes;
- transport/retry/redirect changes;
- config schema changes;
- HTTPS support;
- timing gates.

## Handoff note

Implement preparation as a private layer around the existing scheduler. The easiest regression to introduce is changing direct-invalid-endpoint behavior by moving URL validation earlier; freeze that case before refactoring ownership.

## Closure record

Implemented at `83df89e` with toolchain `rustc 1.98.1`. Local
verification: `cargo test -p gregg --lib --all-features --
scheduler` (34 passed, including 6 new `plan140_*` tests),
`poller` (46 passed), `state` (51 passed), `cargo fmt --check`,
workspace clippy `-D warnings`, and `./scripts/check-local.sh`
green. Final campaign CI run is recorded in Plan 138.

Deterministic evidence (`crates/gregg/src/scheduler.rs`,
`poller.rs`, `state.rs`):

- `prepare_targets(&endpoints)` builds `Arc<str>` v1/v2 URLs once per
  installed list; `poll_loop` rebuilds only on `ReplaceEndpoints`;
  two `poll_generation` calls against the same prepared slice reuse
  the same allocations (`Arc::ptr_eq`) and poll successfully;
- replacement diffusely rebuilds exactly the replacement URLs and the
  end-to-end `ReplaceEndpoints` scheduler test still polls only the
  replacement endpoint;
- invalid prepared targets reproduce the public `poll` `NetworkError`
  discriminant per generation (no scheduler-construction failure);
- prepared v2 404 falls back to the prepared v1 target (`Online`);
- panic bookkeeping is index-based: handles keep `(usize, JoinHandle)`
  and only the exceptional panic branch clones from the installed
  list, preserving one ordered result per endpoint with exact identity;
- public `HttpClient::poll(&Endpoint)` and `PollResult` shape
  unchanged; new `poll_prepared(endpoint, v1, v2, clock)` moves one
  generation-owned endpoint into the result (stable-ID duplication
  only);
- `AppState::reconcile_systems` uses `old_by_id.remove` (move) instead
  of `get().cloned()`; existing reconcile/reset/selection/viewport
  tests green.

Measurement: stripped release `gregg` 3,740,592 bytes (delta 0 versus
the Plan-125 baseline). No new dependencies; scheduler architecture,
ordering, generation, semaphore ceiling, cancellation, offline retry,
and panic isolation unchanged.
