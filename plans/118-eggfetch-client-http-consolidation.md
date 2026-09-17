# Plan 118: eggfetch client HTTP consolidation

Status: complete at `66a0102` (plus `cda51a4`); CI `35184430460` green.

Depends on: Plan 117's Rust 1.89 workspace/dependency baseline. The implementation target is `eggfetch-core` 0.1.5 from crates.io. This plan is independent of the remaining Plan 091 soak record.

## Objective

Replace Gregg's production client-side `reqwest` transport with the current `eggfetch-core` 0.1.5 API, consolidating generic HTTP/TLS timeout, body-limit, authentication, pooling, and connection-failure diagnosis behind the shared eggstack HTTP client while preserving Gregg's application-level polling and EggPool contracts.

The intended ownership boundary after this phase is:

```text
eggfetch-core
  owns URL dispatch, HTTP framing, pooling, TLS, request deadlines,
       response-body limits, Bearer header safety/redaction, and typed
       DNS/refused/connect provenance

Gregg poller / EggPool client
  own endpoint construction, v2->v1 negotiation, status mapping,
      JSON/protocol validation, stable PollOutcome/EggpoolFetchOutcome,
      scheduler/worker behavior, and UI-facing provenance
```

The migration is justified primarily by maintenance consolidation. Binary-size or dependency-count improvements must be measured rather than assumed.

## Baseline findings

Production `reqwest` use is confined to the `gregg` client crate.

`crates/gregg/src/poller.rs` currently owns:

- a long-lived `reqwest::Client`;
- redirects disabled;
- four idle connections per host;
- one configurable whole-request timeout;
- manual `Content-Length` rejection and streaming accumulation to a 64 KiB cap;
- a reqwest-specific error classifier;
- manual error-source walking for `ConnectionRefused`;
- platform-specific DNS diagnosis using `io::ErrorKind`, resolver OS codes, and fallback message matching.

`crates/gregg/src/eggpool.rs` independently owns a second long-lived reqwest client with:

- redirects disabled;
- two idle connections per host;
- the same configured request deadline;
- HTTP and HTTPS support;
- request-local Bearer authentication;
- a separate 16 KiB bounded streaming loop;
- duplicated timeout/DNS/refused/network classification.

`crates/gregg/src/endpoint.rs::EndpointSpec::parse_add_input()` also uses `reqwest::Url` only as a URL parser even though `url` is already a direct Gregg dependency.

`main.rs` currently propagates reqwest construction errors because both HTTP client constructors return `Result<_, reqwest::Error>`.

Eggfetch 0.1.5 now supplies the missing generic transport surfaces needed for a clean replacement:

- `RequestBuilder::send_detailed()` / `Client::send_detailed()`;
- evidence-backed `NetworkFailureKind::{Dns, ConnectionRefused, Connect}`;
- `RequestFailure::is_timeout()` and phase-aware timeout errors;
- request/client `max_decoded_body_size` enforcement;
- buffered `Response::bytes()` plus streaming APIs;
- request-local `AuthScheme::bearer()` with secret redaction and CR/LF/header validation;
- redirect policy and per-host idle-pool configuration;
- HTTP/1 + Rustls feature-minimal builds.

The 0.1.5 changelog specifically tightens typed DNS provenance on the standard Hyper HTTP/HTTPS path, allowing Gregg to remove its resolver-code/string classifier without moving Gregg-specific policy into eggfetch.

## Scope decisions

### 1. Use a feature-minimal `eggfetch-core` dependency

Replace reqwest in `crates/gregg/Cargo.toml` with the narrow Rust client engine:

```toml
eggfetch-core = {
    version = "0.1.5",
    default-features = false,
    features = ["http1", "tls-rustls"]
}
```

The exact formatting may be one line, but the feature set is intentional.

Do **not** enable by default:

```text
tls-native-roots
http2
http3
proxy
cookies
json
compression-*
multipart
tracing
```

Gregg's current reqwest profile disables reqwest defaults and enables `rustls-tls` plus streaming; it therefore uses the packaged WebPKI Rustls path rather than native TLS roots. `http1 + tls-rustls` is the closest eggfetch profile while retaining EggPool HTTPS.

Keep `serde_json` owned by Gregg for protocol/application decoding; do not enable eggfetch's optional JSON feature merely to replace `serde_json::from_slice` calls.

### 2. Keep the greggd poller and EggPool clients separate

Do not create one shared global HTTP client just to deduplicate two constructor calls.

The existing clients have intentionally different pool policy and subsystem ownership:

```text
greggd polling  max idle per host = 4
EggPool summary max idle per host = 2
```

Keep those independent long-lived clients. EggPool remains optional and isolated from the Systems poll scheduler.

Do not introduce a `gregg-http` crate, generalized transport trait, dependency-injection framework, or reqwest-compatibility facade.

### 3. Preserve reqwest whole-request timeout semantics explicitly

This is the most important semantic migration detail.

Eggfetch's scalar timeout constructors configure per-phase limits but deliberately do not set the `total` deadline. Gregg's current reqwest client timeout is a whole-request deadline that includes body consumption.

Construct an eggfetch timeout that preserves both the whole-request cap and bounded phases:

```rust
Timeout {
    pool: Some(timeout),
    connect: Some(timeout),
    write: Some(timeout),
    read: Some(timeout),
    total: Some(timeout),
}
```

Equivalent builder syntax is acceptable if it sets the same five fields.

Do not use only `Timeout::from_secs(...)`/a scalar constructor unless its behavior has changed and the implementation proves `total` is also populated. Review the 0.1.5 API at implementation time rather than relying on an assumption.

No automatic application retry is to be added. Gregg's scheduler already retries configured endpoints on later generations; EggPool's worker performs its existing activation/periodic/manual requests. Do not enable eggfetch `RetryPolicy` in this phase.

### 4. Migrate `poller.rs` without changing Gregg protocol policy

Replace the internal `reqwest::Client` with `eggfetch_core::Client` configured with:

- the explicit timeout above;
- redirects disabled;
- four idle connections per host;
- no automatic retry;
- request-level or client-level 64 KiB decoded-body cap.

Use `send_detailed()` for the request-header/connect phase so typed transport provenance is available.

Keep all existing Gregg policy unchanged:

```text
GET /v2/status first
404 only -> GET /v1/status
other v2 status -> surface status
malformed/invalid/unsupported v2 -> no v1 fallback
warming/failure payload semantics unchanged
same stable PollOutcome variants
same latency accounting and scheduler generation behavior
```

Do not move schema parsing/validation into eggfetch.

### 5. Replace manual transport diagnosis with typed eggfetch classification

Delete the reqwest-specific classifier and the generic error-chain/string resolver logic after equivalent mapping is covered.

For a failed detailed send, preserve Gregg's stable categories with logic equivalent to:

```text
Error::DecodedBodyTooLarge
  -> BodyTooLarge

RequestFailure::is_timeout()
  -> Timeout

NetworkFailureKind::Dns
  -> DnsFailure

NetworkFailureKind::ConnectionRefused
  -> ConnectionRefused

NetworkFailureKind::Connect / unknown future subtype / None
  -> NetworkError
```

`NetworkFailureKind` is non-exhaustive; matching must remain forward-compatible.

Do not inspect eggfetch `Display` strings to recover DNS/refused semantics. If 0.1.5 does not provide typed evidence for a route, classify it as generic network failure rather than recreating heuristic message parsing.

### 6. Let eggfetch own the response-size guard; keep Gregg's bounded JSON parsing

Apply `max_decoded_body_size(MAX_RESPONSE_BYTES)` to each status request or equivalently on the dedicated client.

After successful status validation, use the bounded response's `bytes()` result and continue decoding via Gregg's existing `serde_json` + protocol validation code.

The size cap must remain authoritative for:

- valid `Content-Length` over the cap;
- missing `Content-Length`;
- incorrect/underreported length;
- chunked bodies that cross the cap while being consumed.

Retain `BodyTooLarge` as a distinct Gregg outcome. Do not allocate an unbounded buffer before the eggfetch body guard runs.

For errors that occur only after response headers while consuming the body, preserve current Gregg observable behavior unless a separate correctness change is explicitly justified. In particular, the existing reqwest streaming loop treats ordinary post-header body failures as `NetworkError`; do not silently reclassify these into a new UI-visible category as part of transport consolidation. `DecodedBodyTooLarge` remains `BodyTooLarge`.

### 7. Migrate the EggPool summary client on the same transport change

Leaving EggPool on reqwest would retain the duplicate HTTP/TLS stack and defeat most of the dependency consolidation.

Replace its internal client with a separate eggfetch client configured with:

- the same explicit five-field timeout policy;
- redirects disabled;
- two idle connections per host;
- HTTP/1 + Rustls HTTPS;
- no automatic retry;
- a 16 KiB decoded-body cap.

Preserve all existing `EggpoolFetchOutcome` behavior:

```text
missing/empty API-key env -> MissingApiKeyEnv
401                       -> Unauthorized
403                       -> Forbidden
404                       -> StatsUnavailable
other non-2xx             -> HttpStatus(code)
oversized body            -> BodyTooLarge
bad JSON                   -> DecodeError
semantic invalidity        -> InvalidSummary
timeout/refused/DNS        -> stable transport categories
other transport failure   -> NetworkError
superseded worker          -> Cancelled
bad configured URL         -> InvalidEndpoint
```

Do not change refresh cadence, worker generation/cancellation, pane behavior, or endpoint configuration.

### 8. Use eggfetch request-local Bearer authentication

Replace manual reqwest `HeaderValue` construction with `AuthScheme::bearer(token)` and request-local `.auth(...)`.

Eggfetch validates and redacts the secret at the generic transport boundary. If Bearer construction rejects the configured environment value, preserve Gregg's current behavior by mapping that invalid header/token condition to `InvalidSummary` rather than `MissingApiKeyEnv` or a transport failure.

Never place the resolved secret in outcomes, logs, debug values, URLs, or error detail strings.

### 9. Replace `reqwest::Url` with the existing `url::Url`

In `EndpointSpec::parse_add_input`, use `url::Url::parse()` directly.

Keep all existing endpoint behavior exactly:

- only HTTP URL convenience for `gregg add`;
- explicit port requirement;
- credentials rejected;
- IPv4/IPv6/zone-ID behavior preserved;
- path/query/fragment parsed/validated then discarded as today;
- HTTPS remains rejected for greggd Systems endpoints;
- EggPool's separate parser continues allowing HTTP/HTTPS.

Do not redesign endpoint parsing during the transport migration.

### 10. Simplify now-infallible client construction only where the eggfetch API supports it

Eggfetch 0.1.5 `ClientBuilder::build()` returns `Client` directly. Under the selected fixed feature profile, convert:

```text
HttpClient::new(timeout) -> Result<Self, reqwest::Error>
EggpoolClient::new(timeout) -> Result<Self, reqwest::Error>
```

into infallible constructors if the implementation still matches the reviewed 0.1.5 API.

Then remove obsolete reqwest-construction error plumbing from `main.rs`, including `spawn_eggpool_worker()`'s reqwest-specific result type and test `.expect("test HTTP client construction")` noise where appropriate.

Do not hide genuine request-time TLS/configuration failures; they should surface through the normal stable transport outcome mapping.

### 11. Remove reqwest and only dependencies proven obsolete

Once all production and test references are gone, remove reqwest completely from `crates/gregg/Cargo.toml` and the lockfile.

Do not assume that `futures-util`, `tokio-util`, `libc`, `url`, or another current dependency becomes unused merely because the manual reqwest streaming path disappears. Verify source references and `cargo tree` before removing anything else.

Plan 117 should already have removed Rust-1.75-only transitive guard dependencies; Plan 118 must not reintroduce direct Hyper/Rustls/transitive pins around eggfetch.

If Gregg needs a generic HTTP capability that eggfetch lacks, stop and document the gap rather than adding a Gregg-specific eggfetch adapter inside this repository or reaching through eggfetch to its private Hyper stack.

### 12. Update active architecture and agent guidance

Update at least the active references that currently describe a reqwest client:

- `architecture/workspace.md`;
- `architecture/gregg-client.md`;
- `crates/gregg/README.md` where transport implementation is named;
- `.opencode/skills/gregg-client/SKILL.md`;
- `plans/README.md` status/closure text when the phase lands.

Describe eggfetch as an implementation dependency, not a new Gregg product abstraction. Gregg's stable public behavior remains its own domain outcomes and protocol handling.

Do not rewrite old plans that truthfully document reqwest at the time they were implemented.

### 13. Measure footprint instead of claiming it

Before changing the dependency graph, record a baseline from the settled Plan-117 tree. After the migration, rerun the same measurements.

At minimum record:

```text
cargo build --release -p gregg
cargo tree -p gregg -e normal
cargo tree -p gregg -e normal --duplicates
```

Record the stripped release binary size and a concise before/after dependency observation in the Plan 118 closure.

A binary-size decrease is desirable but is not assumed. The primary acceptance criterion is removal of duplicated transport/error/body-management code and reqwest-specific maintenance. If the binary grows materially, explain the source of the delta before closing rather than claiming a footprint win.

`greggd` should not gain an `eggfetch-core` production dependency as a side effect. Confirm with `cargo tree -p greggd -e normal`.

## Implementation sequence

### Step 1: capture the Plan-117 baseline

Before editing the transport, record:

```text
release gregg binary bytes
normal dependency tree / duplicates
presence of reqwest and its relevant transitive HTTP/TLS families
absence of eggfetch from greggd
```

Do not mix this measurement with the Plan-117 dependency modernization baseline.

### Step 2: add feature-minimal eggfetch and migrate endpoint URL parsing

Add `eggfetch-core` 0.1.5 with only `http1` and `tls-rustls`.

Switch `reqwest::Url` to `url::Url` in the endpoint adapter and run endpoint tests before modifying poll transport. This removes one trivial reqwest reference independently.

### Step 3: migrate the greggd poller

Build the eggfetch client with exact redirect/pool/deadline behavior.

Use detailed sends, the eggfetch body limit, and buffered bounded body consumption. Remove `classify_reqwest_error`, `is_connection_refused`, `is_dns_failure`, resolver OS-code helpers, and manual response accumulation only after replacement tests pass.

Keep the v2/v1 protocol path and `PollOutcome` definition stable.

### Step 4: migrate EggPool

Build the separate two-idle-connection eggfetch client, move Bearer auth to `AuthScheme`, apply the 16 KiB body cap, and map typed failures into the existing `EggpoolFetchOutcome` taxonomy.

Keep the worker unchanged except for constructor type/signature fallout.

### Step 5: simplify constructor plumbing and remove reqwest

Make constructors infallible if still supported by 0.1.5, adjust `main.rs` and tests, remove every reqwest import/reference, then remove the manifest dependency.

Use a repository-wide source search plus `cargo tree -p gregg` to prove reqwest is absent from Gregg's production graph. If another unrelated dependency still brings reqwest transitively, record that fact separately; do not add exclusions or pins solely to force a cosmetic tree result.

### Step 6: adapt and strengthen focused regression coverage

Existing deterministic tests already cover core poll semantics such as timeout before response headers, connection refusal, HTTP status, malformed JSON, oversized fixed-length/chunked bodies, URL construction, protocol validation, and v2 fallback. Adapt them to the new constructor/API without weakening assertions.

Add or preserve focused coverage for the migration-specific contracts:

```text
poller:
  redirect response is not followed
  timeout before headers -> Timeout
  closed local port -> ConnectionRefused where typed evidence is available,
                       otherwise generic NetworkError is acceptable only if
                       eggfetch documents no typed evidence on that platform
  >64 KiB Content-Length -> BodyTooLarge
  chunked/underreported response crossing 64 KiB -> BodyTooLarge
  v2 404 -> v1 only; other v2 failures never fall back

EggPool:
  HTTP success
  HTTPS path remains compile/runtime-capable (use existing safe fixture style;
  do not disable certificate verification in production)
  valid Bearer auth reaches the request without leaking into diagnostics
  invalid Bearer header value -> InvalidSummary
  401/403/404/other status mappings unchanged
  >16 KiB fixed/chunked body -> BodyTooLarge
  timeout/refused/DNS/generic failure mapping remains stable
```

Do not create flaky public-network tests merely to prove DNS. Gregg may rely on eggfetch's own typed-DNS transport tests for the low-level provenance and test only its deterministic mapping where a stable local/injected route is available. Never reintroduce textual DNS heuristics just to make a Gregg unit test easy.

### Step 7: update active docs/skills and measure the final footprint

Update active architecture/skill references from reqwest to eggfetch.

Rebuild the release binary and dependency graphs using the same commands as Step 1. Record truthful deltas in the closure record.

### Step 8: run full bounded verification

Required:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
cargo +1.89 test --workspace --all-targets --all-features
```

Because this changes cross-platform networking/TLS dependencies, run the existing ordinary CI workflow once and require its Linux, macOS arm64, macOS Intel, Windows, and Rust 1.89 MSRV jobs to pass.

No new workflow/job/matrix is required.

The release preflight is appropriate before closure because the dependency/lockfile changes affect Cargo installation/package paths:

```text
./scripts/check-local.sh --release
```

## Acceptance criteria

Plan 118 is complete only when:

1. `crates/gregg` uses `eggfetch-core` 0.1.5 with the minimal `http1` + `tls-rustls` feature set and no unnecessary eggfetch default features.
2. There are no production/test `reqwest::` references in Gregg and no direct reqwest dependency in `crates/gregg/Cargo.toml`.
3. `EndpointSpec::parse_add_input()` uses the existing `url` crate with no endpoint behavior regression.
4. `HttpClient` preserves redirects-off, four-idle-per-host, no-automatic-retry, explicit total deadline, 64 KiB bounded response, and v2-first/404-only-v1 fallback semantics.
5. Gregg's custom reqwest source-chain/string DNS/refused classifier is deleted and detailed eggfetch typed provenance drives connection failure mapping.
6. All existing `PollOutcome` variants and offline-provenance semantics remain stable.
7. `EggpoolClient` preserves redirects-off, two-idle-per-host, HTTPS, explicit total deadline, 16 KiB response limit, no automatic retry, request-local Bearer authentication, and every existing stable outcome mapping.
8. Invalid EggPool Bearer values remain an application validation failure without secret leakage.
9. Manual duplicate response-body accumulation/limit loops are removed where eggfetch's authoritative body limit replaces them.
10. Client-construction error plumbing is simplified only to the extent justified by eggfetch 0.1.5's infallible builder API.
11. No `eggfetch-core` production dependency is introduced into `greggd` or `gregg-update`.
12. Active architecture/client skill documentation describes the new transport ownership accurately; truthful historical plans remain unchanged.
13. Before/after release binary size and dependency-tree observations are recorded without claiming an unmeasured footprint reduction.
14. Focused poller/EggPool migration tests, the default local check, release preflight, Rust 1.89 check/test, and existing native CI workflow pass on the final tree.
15. The closure record identifies the implementation SHA, eggfetch version/features, measured footprint delta, and exact CI run used for cross-platform evidence.

## Preserved exclusions

- no greggd server migration from Axum/Hyper;
- no `gregg-update` rewrite from bounded external `curl` to eggfetch;
- no HTTPS support for ordinary greggd Systems polling;
- no proxy/cookie/compression/HTTP2/HTTP3 feature expansion;
- no automatic per-request retries or offline backoff policy;
- no scheduler/worker/state/TUI redesign;
- no protocol/schema changes;
- no generalized HTTP abstraction crate or trait layer;
- no eggfetch changes specialized only for Gregg;
- no reaching through eggfetch into private Hyper/Rustls implementation details;
- no new CI workflow/job/matrix/evidence system;
- no rewriting historical plans that correctly describe reqwest-era behavior.

## Closure record

Implementation `66a0102810d9d313b372cd0e5be42077219208ec`
("feat: replace gregg client reqwest with eggfetch-core 0.1.5 (Plan 118)")
plus `cda51a45c65e902849002503bd98df7191696eb8`
("fix: give EggPool closed-port test poller-scale deadline for Windows"),
verified by remote CI run `35184430460` green across Linux, macOS arm64,
macOS Intel, Windows, and MSRV Rust 1.89. The prior run `35183954733`
failed only the new EggPool closed-port test on Windows (`Timeout` instead
of `ConnectionRefused`/`NetworkError` under a 2s deadline); the fix gives
that test the same 5s deadline as the Systems poller so a slow Windows
refusal surfaces as typed evidence.

Eggfetch version/features: `eggfetch-core 0.1.5` with
`default-features = false, features = ["http1", "tls-rustls"]` in
`crates/gregg/Cargo.toml`. No `tls-native-roots`, `http2`, `http3`,
`proxy`, `cookies`, `json`, `compression-*`, `multipart`, or `tracing`.
`serde_json` stays owned by Gregg. `ClientBuilder::build()` is infallible
as reviewed, so `HttpClient::new` / `EggpoolClient::new` are infallible
and `main.rs` reqwest-construction plumbing is removed. No
`eggfetch-core` dependency in `greggd` or `gregg-update`
(`cargo tree -p greggd -e normal` and `cargo tree -p gregg-update`
show neither `eggfetch` nor `reqwest`).

Measured footprint (same commands before/after, Plan-117 baseline):

```text
before: cargo build --release -p gregg -> 3609472 bytes (stripped)
after:  cargo build --release -p gregg -> 4264912 bytes (stripped)
delta:  +655440 bytes (+18.2%)
cargo tree -p gregg -e normal: 377 lines -> 362 lines (-15)
  before: reqwest v0.12.28 (+ hyper, hyper-rustls, hyper-util, rustls,
    tokio-rustls, tower-http v0.6.11)
  after:  eggfetch-core v0.1.5 (+ hyper, hyper-rustls, hyper-util, rustls,
    tokio-rustls, dashmap; tower-http gone)
cargo tree -p gregg --duplicates: 175 lines -> 188 lines
Cargo.lock: reqwest and tower-http gone; eggfetch-core present
```

The binary grows materially despite fewer top-level tree lines. Source of
the delta is the eggfetch engine itself (connection pool with `dashmap`,
typed `Error`/`RequestFailure`/`NetworkFailureKind`, body-limit and
phase-aware timeout machinery) replacing reqwest's leaner
`rustls-tls` + `stream` profile; `tower-http` leaves while `dashmap` (via
eggfetch) arrives. No footprint win is claimed. The primary acceptance
criterion is met: duplicated transport/error/body-management code and the
reqwest-specific maintenance surface are removed, with no `reqwest::`
references and no direct reqwest dependency remaining.

Focused coverage: poller redirect-not-followed, timeout-before-headers,
closed-port refused-or-network, >64 KiB fixed-length, two-write over-cap,
chunked over-cap, close-delimited over-cap, v2-404-only fallback and
non-fallback cases; EggPool HTTP success, Bearer auth without leakage,
invalid Bearer to `InvalidSummary`, 401/403/404/other mappings, 16 KiB
fixed/chunked over-cap, timeout, closed-port, redirect-not-followed, and
https URL representability. DNS relies on eggfetch's typed provenance;
no textual heuristics were reintroduced and no flaky public-network DNS
test was added.

Local verification on the final tree: `cargo fmt --all -- --check` clean,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean, `cargo test --workspace --all-targets --all-features` green,
`cargo doc --workspace --no-deps` (only pre-existing warnings),
`./scripts/check-local.sh` green, `./scripts/check-local.sh --release`
green except the expected clean-tree failure on the uncommitted tree,
`cargo +1.89 test --workspace --all-targets --all-features` green.
