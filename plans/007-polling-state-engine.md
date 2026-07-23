# Phase 7: polling and application-state engine

## Objective

Implement the complete non-visual runtime for `gregg`: bounded concurrent HTTP polling, batch-generation control, response validation, reachability classification, stable online/offline ordering, selection, and variable-height viewport state.

This phase must leave terminal rendering as a downstream consumer of immutable or read-only application state. Network tasks must not mutate Ratatui widgets or terminal buffers.

## Architecture

Use explicit layers:

```text
validated ClientConfig
        |
        v
PollScheduler -----> PollBatch results
                         |
                         v
                    App reducer
                         |
                         v
                      AppState
```

Recommended modules:

```text
crates/gregg/src/
├── endpoint.rs
├── poller.rs
├── scheduler.rs
├── state.rs
├── action.rs
├── event.rs
└── clock.rs
```

The eventual TUI loop should exchange typed actions/events with the state engine rather than invoking arbitrary methods across layers.

## HTTP client policy

Create one long-lived `reqwest::Client` or comparably suitable client for the lifetime of the application. Compile only required features:

- Plain HTTP.
- JSON decoding or byte response handling.
- Tokio runtime integration.
- No TLS for version-1 daemon endpoints unless a later explicit requirement changes scope.
- No cookies, proxy discovery, multipart, HTTP/2, or compression unless measurement justifies it.

Set explicit limits:

- Total request timeout from config, default approximately 1500 ms.
- Response body size cap sufficient for the sub-2-KiB protocol with reasonable forward-compatible headroom, for example 64 KiB.
- Redirect policy disabled; endpoint redirects should be treated as protocol errors.
- Bounded connection pool behavior.

Construct URLs only from validated host and port values. IPv6 literals must be bracketed. Use fixed path `/v1/status`.

## Poll scheduling

Default refresh interval: five seconds.

Use a single scheduler that creates one polling generation per interval. A generation contains one request per configured endpoint, executed under a fixed semaphore/concurrency bound. Do not spawn an unmanaged immortal task per system.

Recommended model:

```rust
struct PollBatch {
    generation: u64,
    started_at: Instant,
    completed_at: Instant,
    results: Vec<PollResult>,
}

struct PollResult {
    system_id: SystemId,
    endpoint: Endpoint,
    outcome: PollOutcome,
    latency: Duration,
}
```

Generation numbers monotonically increase. The state reducer must reject a batch older than the most recently applied generation. Configuration changes that alter endpoints should advance or invalidate the active generation so late responses cannot resurrect removed systems or overwrite edited endpoints.

Cycle behavior:

- A slow/unreachable host consumes at most its request timeout and one concurrency permit.
- Reachable results should not wait for every failed host before becoming available if incremental updates can be implemented without violating coherent ordering; however, a completed batch model is acceptable for version 1 if total cycle latency remains bounded.
- Do not launch overlapping cycles indefinitely. Choose and document either skip-if-running or cancel-and-replace semantics when a cycle exceeds the refresh interval.
- Immediate refresh action starts a new generation subject to overlap policy.
- Scheduler timing should avoid cumulative drift and runaway catch-up.

## Poll outcome taxonomy

Represent outcomes internally with enough detail for diagnostics:

```text
Online(valid StatusSnapshot)
Timeout
ConnectionRefused
DnsFailure
NetworkError
HttpStatus(non-success)
BodyTooLarge
DecodeError
UnsupportedSchema
InvalidSnapshot
Cancelled
```

The compact TUI may collapse all failure outcomes to `offline`, but state should retain the category and concise safe message for logs, future detail display, or testing.

A response is online only when:

- HTTP status is successful under the API contract.
- Body is within limit.
- JSON decodes.
- Schema version is supported.
- Protocol validation passes.

Do not classify malformed JSON from a reachable endpoint as healthy.

## State model

Recommended shape:

```rust
struct AppState {
    systems: Vec<SystemState>,
    selected_id: Option<SystemId>,
    viewport_top_id: Option<SystemId>,
    last_applied_generation: u64,
    refresh_status: RefreshStatus,
    terminal_size: Option<(u16, u16)>,
}

struct SystemState {
    id: SystemId,
    endpoint: Endpoint,
    configured_name: Option<String>,
    reachability: Reachability,
    latest: Option<StatusSnapshot>,
    last_success_at: Option<Instant>,
    last_attempt_at: Option<Instant>,
    latency: Option<Duration>,
    last_error: Option<PollFailure>,
}
```

Preserve the configured system vector as the canonical order. Compute display order as a projection:

1. Online systems in configured order.
2. Offline/unavailable systems in configured order.

Do not mutate persistent config order when status changes.

During startup before first results, choose a documented representation. Treating systems as pending/offline for ordering is acceptable, but avoid flashing arbitrary reorderings as individual requests complete. A batch apply naturally avoids this.

## Selection semantics

Selection is by stable `SystemId`, not display index. When ordering changes:

- Preserve selection on the same system if it still exists.
- If the selected system was removed, select the nearest surviving configured neighbor or first displayed system.
- If no systems exist, selection is `None`.
- Moving next/previous uses current display order.

Required actions:

```text
SelectNext
SelectPrevious
PageDown
PageUp
SelectFirst
SelectLast
RefreshNow
ConfigReloaded
Resize
Quit
```

The TUI phase may map keys onto these actions, but state-transition tests belong here.

## Variable-height viewport

Online entries consume four rows; offline entries consume one. Scrolling therefore cannot be `offset += 1 row` over a flat text buffer.

Implement layout-neutral helpers:

```rust
fn entry_height(system: &SystemState) -> u16;
fn visible_range(display_order: &[SystemId], top: usize, height: u16) -> Range<usize>;
fn ensure_selected_visible(...);
```

The state engine may store the top system ID/index while the renderer computes exact clipping from the current terminal area. Define behavior when a four-row online entry cannot entirely fit:

- Prefer not to render partial entries.
- If the terminal has fewer than four usable rows, render the dedicated terminal-too-small state.
- Offline one-row entries may fill remaining rows naturally.

PageUp/PageDown should advance approximately one viewport while preserving logical-entry boundaries.

## Concurrency and cancellation

All tasks must terminate when the application exits. Use structured task ownership through a cancellation token, task set, or owned join handles. Do not detach requests that can continue after terminal restoration.

Configuration reload must cancel or invalidate requests for removed endpoints. A cancelled result is not rendered as a fresh offline transition unless policy explicitly says so.

Avoid shared mutable state inside request futures. Each future returns a `PollResult`; one reducer owns `AppState` mutations.

## Freshness behavior

Use daemon `observed_at_unix_ms` and local receipt time separately. Retain the daemon timestamp as authoritative sample time. Detect grossly stale snapshots if they exceed a documented threshold, but avoid rejecting valid hosts merely because wall clocks differ unless the protocol provides a freshness policy.

For version 1, connection success with a valid but old daemon snapshot can be classified as online with stale metadata internally; the compact rendering need not add a fifth line. Phase 9 should test and decide whether a subtle marker is necessary without violating vertical constraints.

## Tests

Use an in-process mock HTTP server or deterministic transport abstraction. Cover:

- All endpoints succeed with mixed Linux/macOS snapshots.
- One timeout among fast successes.
- Concurrency never exceeds configured bound.
- Redirect rejected.
- Oversized body rejected before unbounded allocation.
- Non-2xx, malformed JSON, unsupported schema, invalid numeric invariants.
- Old generation arriving after new generation.
- Removed endpoint receiving a late response.
- Immediate refresh while a cycle is active.
- Empty configuration.
- Online/offline reorder preserving configured relative order.
- Selection preservation across reorder and removal.
- Variable-height visibility for mixed 4-row/1-row entries.
- Cancellation on quit.

Inject clock/scheduler controls; do not sleep for real five-second intervals in ordinary tests.

## Acceptance criteria

Phase 7 is complete when:

1. One reusable HTTP client polls all endpoints with an explicit timeout and body cap.
2. Peak in-flight requests never exceeds configured concurrency in tests.
3. Poll cycles have documented, tested overlap behavior.
4. Stale/older generations cannot overwrite newer state or resurrect removed endpoints.
5. Every network/protocol failure maps to a typed internal outcome without panicking the application.
6. Online classification requires successful protocol validation.
7. Display order is online-first/offline-last while preserving configured relative order in each group.
8. Selection is stable by system ID across reordering and config changes.
9. Viewport helpers correctly handle mixed four-row and one-row entries.
10. All request tasks are cancelled/joined on shutdown.
11. The state and polling modules contain no Ratatui drawing calls or raw-terminal operations.
12. Mixed Linux/macOS snapshots, including null macOS I/O wait, pass the full polling/reducer path.

## Handoff to phase 8

Expose a read-only state projection and an action/event interface. The renderer should need no knowledge of HTTP clients, task handles, configuration locks, or platform collectors.
