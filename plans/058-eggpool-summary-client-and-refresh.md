# Phase 58: EggPool summary client and refresh behavior

Status: planned.

## Objective

Add the smallest typed HTTP client and asynchronous refresh path needed to read EggPool's existing summary API safely from the `gregg` TUI.

This phase ends when:

- Gregg can fetch `GET /api/stats/summary` for the four fixed periods;
- public and API-key-protected EggPool installations are supported;
- response size, redirects, timeout, decoding, and semantic validation are bounded;
- errors are classified into stable TUI-safe outcomes without leaking secrets or raw bodies;
- one small worker performs immediate and periodic refreshes without blocking terminal input;
- no EggPool worker or request exists when configuration omits EggPool.

Pane state, key routing, and rendering are Phase 59. Final event-loop integration is Phase 60.

## Dependencies and execution position

Depends on Phase 57's `EggpoolEntry`, scheme/address formatter, and API-key environment-variable reference.

Must complete before Phase 60 runtime closure.

Phase 59 may proceed in parallel using synthetic `EggpoolSummary` and outcome values.

## Governing invariants

1. The only EggPool endpoint consumed is `/api/stats/summary`.
2. The only accepted preset query values are `1h`, `24h`, `7d`, and `30d`.
3. The existing long-lived `reqwest` dependency is reused.
4. Redirects remain disabled.
5. Response bodies are explicitly bounded.
6. The resolved API key exists only transiently in request construction and is never included in debug/error/output values.
7. Network work never runs in rendering or blocks the terminal event loop.
8. A failed refresh does not terminate Gregg.
9. A later failed refresh does not discard a prior successful summary.
10. No generalized EggPool SDK, datasource scheduler, retry framework, cache layer, or telemetry database is introduced.
11. No request is sent while the EggPool pane is inactive except completion of an already in-flight request.
12. No new dependency or CI service is added.

## Scope

### In scope

- fixed period enum and API/display mappings;
- typed summary response;
- semantic validation and normalized display-ready snapshot;
- one `reqwest::Client` configuration;
- optional Bearer authentication from `api_key_env`;
- bounded body reading;
- stable error/outcome classification;
- request cancellation/stale-result handling;
- one command/result channel worker;
- immediate refresh and active-pane 60-second cadence;
- synthetic local HTTP tests.

### Out of scope

- any EggPool endpoint other than summary;
- direct database access;
- retries beyond the next scheduled/manual request;
- exponential backoff, circuit breakers, health scoring, or persistence;
- multiple EggPool instances;
- arbitrary/custom periods;
- streaming, SSE, WebSocket, long polling, or dashboard HTML parsing;
- custom TLS roots, insecure TLS, proxies, cookies, redirects, or compression-specific behavior;
- TUI layout or key mapping;
- EggPool server changes;
- new CI workflows/services/evidence.

## Workstream A: define the fixed period model

Create a small enum in the EggPool client/state boundary:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EggpoolPeriod {
    Hour,
    Day,
    Week,
    Month,
}
```

Required direct methods:

```rust
pub const fn api_value(self) -> &'static str;
pub const fn display_label(self) -> &'static str;
pub const fn longer(self) -> Self;
pub const fn shorter(self) -> Self;
```

Exact mappings:

```text
Hour   -> `1h`  -> `1 hour`
Day    -> `24h` -> `1 day`
Week   -> `7d`  -> `7 days`
Month  -> `30d` -> `30 days`
```

Movement clamps:

```text
Month.longer() -> Month
Hour.shorter() -> Hour
```

Do not parse arbitrary period strings from user input or configuration. Do not send `1d`.

Required tests should exhaustively cover all variants, mappings, and edge clamping.

### Workstream A acceptance criteria

- [ ] All four API tokens are exact.
- [ ] Display labels are separate from API tokens.
- [ ] Longer/shorter movement is deterministic and nonwrapping.
- [ ] No dynamic period parser or collection is introduced.

## Workstream B: define the wire and normalized summary types

Deserialize only fields needed for this pane and semantic decisions:

```rust
#[derive(Debug, Deserialize)]
struct EggpoolSummaryWire {
    period: String,
    accounted_tokens: u64,
    cache_read_ratio: Option<f64>,
    tokens_per_second: f64,
    avg_ttft_ms: f64,
    streamed_requests: u64,
}
```

The public/internal snapshot stored in application state should avoid retaining unvalidated wire values:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct EggpoolSummary {
    pub period: EggpoolPeriod,
    pub accounted_tokens: u64,
    pub cache_read_ratio: Option<f64>,
    pub output_tokens_per_second: f64,
    pub avg_ttft_ms: Option<f64>,
}
```

Normalization rules:

- returned `period` must equal the requested API token;
- `cache_read_ratio`, when present, must be finite and in `0.0..=1.0`;
- `tokens_per_second` must be finite and nonnegative;
- `avg_ttft_ms` must be finite and nonnegative;
- if `streamed_requests == 0`, normalized `avg_ttft_ms = None` regardless of numeric zero;
- if streamed requests exist, normalized TTFT is `Some(avg_ttft_ms)`;
- no numeric value is silently clamped except formatting-level rounding;
- malformed semantics produce `InvalidSummary`, not a panic.

Do not deserialize or expose costs, provider counts, request counts, latency percentiles, bytes, reasoning tokens, or other summary fields.

### Workstream B acceptance criteria

- [ ] The wire type contains only required fields.
- [ ] The normalized type encodes no-stream TTFT as unavailable.
- [ ] Null cache ratio remains unavailable.
- [ ] Negative, nonfinite, out-of-range, and period-mismatch values are rejected.
- [ ] No legacy `total_tokens` fallback is introduced.

## Workstream C: define stable request outcomes

Add a focused outcome/error model suitable for reducer state:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum EggpoolFetchOutcome {
    Online(EggpoolSummary),
    MissingApiKeyEnv { name: String },
    Unauthorized,
    Forbidden,
    StatsUnavailable,
    Timeout,
    ConnectionRefused,
    DnsFailure,
    NetworkError,
    HttpStatus(u16),
    BodyTooLarge,
    DecodeError,
    InvalidSummary,
    Cancelled,
}
```

Requirements:

- 401 maps to `Unauthorized`;
- 403 maps to `Forbidden`;
- 404 maps to `StatsUnavailable` because EggPool does not register stats routes when its dashboard/statistics surface is disabled;
- all other non-success statuses map to `HttpStatus(code)`;
- request errors use bounded classification similar to the existing greggd poller;
- no variant contains a raw response body, URL with credentials, request headers, or reqwest error string;
- `MissingApiKeyEnv` carries only the configured environment-variable name;
- cancellation is not displayed as a failure when superseded by a newer request or shutdown.

A separate `EggpoolClientError` is optional only if it materially simplifies internal tests. Do not duplicate full error chains into state.

### Workstream C acceptance criteria

- [ ] Every required status/network/decode failure has a stable variant.
- [ ] Outcome values are safe to clone/store/render.
- [ ] No secret or raw body can enter application state.
- [ ] 404 has actionable semantics distinct from generic HTTP failure.

## Workstream D: construct one bounded HTTP client

Create a focused module such as:

```text
crates/gregg/src/eggpool.rs
```

Reuse one long-lived `reqwest::Client` with:

- the existing Gregg request timeout or a directly supplied `Duration`;
- redirects disabled;
- a small idle pool per host;
- existing rustls configuration/features;
- no cookies;
- no additional default headers containing credentials.

Build the URL from validated configuration:

```text
{scheme}://{host}:{port}/api/stats/summary?period={api_value}
```

Use URL construction that safely brackets IPv6 and percent-encodes the query value. Since host/scheme/port are already validated and the path is fixed, do not accept a configurable base path.

The response body cap should be substantially below or equal to the existing greggd 64 KiB cap. A summary payload is small; a dedicated constant such as 16 KiB is sufficient unless tests show a reason otherwise.

Read the body incrementally or reject from `Content-Length` first and then bound actual chunks, following the existing poller pattern.

### Workstream D acceptance criteria

- [ ] One reusable client performs all EggPool requests.
- [ ] Redirects are disabled.
- [ ] IPv4/DNS/IPv6 URLs are constructed correctly.
- [ ] The path and query are fixed/bounded.
- [ ] Content-Length and streamed body growth cannot exceed the cap.
- [ ] No new HTTP dependency or client stack is added.

## Workstream E: resolve and apply optional authentication

Authentication logic per request:

```text
api_key_env absent
    -> send no Authorization header

api_key_env present and environment variable set/nonempty
    -> send `Authorization: Bearer <value>`

api_key_env present but missing/empty
    -> return MissingApiKeyEnv without sending the request
```

Requirements:

- use `std::env::var_os` or equivalent to distinguish absence and preserve non-UTF-8 handling deliberately;
- EggPool API keys are constrained to ASCII by EggPool, so non-UTF-8 values should be treated as missing/invalid locally without logging bytes;
- construct `HeaderValue` with sensitive marking where reqwest/http supports it;
- never store the resolved key in `EggpoolClient`, application state, outcome, debug output, or a long-lived command;
- do not add `X-API-Key` fallback unless implementation evidence shows Bearer is incompatible; EggPool explicitly accepts Bearer;
- do not validate key shape beyond safe header construction; EggPool remains authoritative.

Tests should set a synthetic environment variable under a process-level test lock or use an injected environment lookup function to avoid parallel-test races. Prefer injection if small.

Required tests:

- public request sends no auth;
- configured env sends exact Bearer header to synthetic server;
- missing env sends no request and returns the correct outcome;
- empty env is rejected;
- synthetic secret is absent from debug/error/outcome strings.

### Workstream E acceptance criteria

- [ ] Public and protected modes work.
- [ ] Missing credentials are detected before network I/O.
- [ ] Secret values are request-local and redacted from all state/output.
- [ ] No secret storage subsystem is added.

## Workstream F: implement fetch and response validation

`EggpoolClient::fetch` should conceptually accept:

```rust
pub async fn fetch(
    &self,
    endpoint: &EggpoolEntry,
    period: EggpoolPeriod,
) -> EggpoolFetchOutcome
```

Execution order:

1. resolve optional credential;
2. build fixed URL;
3. send GET;
4. classify non-success status without reading/logging an unbounded body;
5. enforce body limit;
6. deserialize `EggpoolSummaryWire`;
7. validate/normalize against requested period;
8. return `Online`.

Do not retry automatically. A failed request is retried only by a later period change, manual refresh, or periodic active-pane tick.

Do not follow redirects because a redirect could disclose the Bearer header to an unintended host.

Synthetic server tests should cover:

- valid public success for each period;
- valid authenticated success;
- 401/403/404/500;
- malformed JSON;
- missing required field;
- returned period mismatch;
- null cache ratio;
- zero streamed requests;
- invalid ratio;
- negative/nonfinite numeric values represented in parseable JSON where possible;
- declared oversized body;
- chunked oversized body;
- timeout/connection close.

### Workstream F acceptance criteria

- [ ] Success produces one validated normalized summary.
- [ ] All failure modes produce stable outcomes.
- [ ] No response body or error chain leaks into state.
- [ ] No automatic retry or redirect occurs.

## Workstream G: add a bounded command/result worker

Create one EggPool-specific worker only when `Config::eggpool` is present.

Suggested command model:

```rust
pub enum EggpoolCommand {
    Activate { period: EggpoolPeriod },
    Deactivate,
    SetPeriod(EggpoolPeriod),
    Refresh(EggpoolPeriod),
    Shutdown,
}
```

A simpler model is acceptable if it preserves the required behavior without races. Do not generalize into a trait-based scheduler.

Suggested result model:

```rust
pub struct EggpoolResult {
    pub generation: u64,
    pub period: EggpoolPeriod,
    pub started_at: Instant,
    pub completed_at: Instant,
    pub outcome: EggpoolFetchOutcome,
}
```

Worker behavior:

- no worker is created when unconfigured;
- first `Activate` triggers an immediate fetch;
- `SetPeriod` cancels/obsoletes the older period and immediately fetches the new period;
- `Refresh` triggers an immediate fetch unless an identical request is already in flight, in which case coalescing is allowed;
- while active, a 60-second interval triggers refresh;
- while inactive, interval ticks perform no request;
- `Deactivate` does not need to abort a request already close to completion, but its result must not force pane activation;
- newer commands supersede older results through generation or `(period, request_id)` matching;
- channel capacity remains small and commands may coalesce to latest desired state;
- shutdown cancels promptly.

Avoid spawning an unbounded task per keypress. One worker may own one in-flight request at a time.

### Workstream G acceptance criteria

- [ ] Worker cardinality is zero or one.
- [ ] At most one EggPool request is in flight.
- [ ] Period changes produce an immediate latest-period request.
- [ ] Inactive pane produces no periodic requests.
- [ ] Active passive cadence is no faster than 60 seconds.
- [ ] Stale results are distinguishable and safely ignored.
- [ ] Shutdown is bounded.

## Workstream H: preserve last success across refresh failures

The worker result does not itself own presentation state, but tests must define the reducer contract expected by Phase 59:

```text
first request pending       -> no summary, refreshing
first request fails         -> no summary, error
request succeeds            -> summary stored, error cleared
later refresh pending       -> prior summary retained, refreshing
later refresh fails         -> prior summary retained, error/stale marker set
period changes              -> old-period summary must not be displayed as the new period's value
```

Recommended approach: state stores summaries keyed only by the currently selected period or clears the visible summary on period change while retaining no generalized cache. To keep scope small, retain one current-period success only.

Do not create a four-period cache unless it materially simplifies correctness; fetching on period change is already required.

### Workstream H acceptance criteria

- [ ] The API/result shape supports last-success retention for same-period refresh.
- [ ] Old-period data cannot masquerade as the new period.
- [ ] No historical cache or persistence layer is introduced.

## Expected files

Likely files:

```text
crates/gregg/src/eggpool.rs
crates/gregg/src/eggpool_endpoint.rs   # formatter reuse only
crates/gregg/src/main.rs               # module declaration/scaffolding
crates/gregg/Cargo.toml                # no dependency change expected
crates/gregg/src adjacent tests
```

Do not change UI renderers in this phase. State may gain compile-only types only if necessary for worker/result tests; behavioral reducer work belongs to Phase 59.

## Implementation sequence

1. Add exhaustive period mapping tests.
2. Define wire and normalized types with semantic validation tests.
3. Define safe outcome classification.
4. Build the bounded no-redirect client.
5. Add injected credential resolution and auth/redaction tests.
6. Implement fetch against a synthetic local server.
7. Add status/body/decode/semantic failure tests.
8. Implement one worker with generation/latest-period semantics.
9. Add immediate/period-change/active-interval/inactive/shutdown tests using paused or injected time where possible.
10. Inspect for accidental retries, generic scheduler abstractions, extra endpoints, or secret retention.

## Required verification

Focused checks:

```text
cargo fmt --all -- --check
cargo test -p gregg eggpool --all-features
cargo test -p gregg --all-targets --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

Timing tests must use Tokio paused time, injected intervals, or very short controlled durations. Do not sleep for 60 seconds.

Do not add a live EggPool dependency to CI.

## Phase acceptance criteria

Phase 58 is complete only when:

- [ ] Exact period mappings are `1h`, `24h`, `7d`, and `30d`.
- [ ] The client consumes only `/api/stats/summary`.
- [ ] `accounted_tokens`, cache-read share, output tok/s, and TTFT are validated with correct semantics.
- [ ] Null cache ratio and zero streamed requests normalize to unavailable values.
- [ ] Public requests send no auth header.
- [ ] Protected requests send Bearer auth from the configured environment variable.
- [ ] Missing/empty credential variables fail locally without sending a request.
- [ ] Resolved secrets never enter state, output, debug text, or error variants.
- [ ] Redirects are disabled and response bodies are bounded.
- [ ] 401, 403, 404, other status, timeout, connection, DNS/network, oversized, decode, and invalid-summary failures are classified.
- [ ] One optional worker performs immediate first-entry, period-change, and manual refresh requests.
- [ ] Passive active-pane refresh is no faster than 60 seconds.
- [ ] Inactive or unconfigured operation performs no periodic EggPool requests.
- [ ] At most one request is in flight and stale results can be ignored.
- [ ] No retry/backoff framework, generalized scheduler/API client, extra endpoint, new dependency, or new infrastructure was added.

## Handoff guidance for a smaller implementation model

- Start with the period enum and response semantic tests.
- Copy the existing poller's body-limit and reqwest-error classification style where practical.
- Keep the API key inside request construction only.
- Use one worker and one in-flight request; coalesce to the latest period.
- Do not add automatic retries. The next explicit or periodic refresh is the retry.
- Do not build a reusable dashboard client, endpoint trait, cache, or datasource scheduler.
