# Plan 119 — eggfetch 0.1.7 lean client adoption

Planning baseline: `cab29c239dbef26acc3c64094936d43e1fe06c07` (`main`, 2026-09-17; Gregg 1.0.13)
Opened: 2026-09-18
Predecessor: completed `plans/118-eggfetch-client-http-consolidation.md`
Upstream release: `eggfetch-core 0.1.7` published to crates.io on 2026-09-18; coordinated release commit `43c3b312f2def887d0f0b7ce539faa626adf2cc8`; total-deadline executable qualification freeze `82f3f38631b44a9a5c5ec5b40790e5015aeb40f8`.

## Objective

Adopt the published eggfetch 0.1.7 client improvements that directly fit Gregg's existing Systems and EggPool polling workload:

1. upgrade `eggfetch-core` from 0.1.5 to 0.1.7;
2. replace the broad compatibility `http1` feature with the new lean `standard-http1` profile while retaining Rustls HTTPS;
3. remove runtime redirect configuration that is no longer compiled in the lean profile;
4. retain Gregg's existing high-level URL API, Bearer auth, typed network-failure provenance, connection pooling, response-body caps and five-field timeout configuration;
5. adopt 0.1.7's corrected absolute `Timeout.total` semantics through response-body EOF by classifying body-stage timeout errors as Gregg's existing `Timeout` outcomes; and
6. remeasure the actual Gregg fat-LTO release binary and dependency graph rather than assuming the upstream footprint numbers transfer exactly.

This is a dependency/profile tightening pass. It must not redesign Gregg's scheduler, protocol negotiation, EggPool worker, endpoint model, TUI, daemon, update path, or stable outcome taxonomy.

## Why this plan exists

Plan 118 intentionally migrated Gregg from reqwest to eggfetch 0.1.5 for maintenance consolidation, but the selected 0.1.5 profile had no standard-route-only feature boundary:

```toml
eggfetch-core = {
    version = "0.1.5",
    default-features = false,
    features = ["http1", "tls-rustls"],
}
```

That migration removed duplicated transport/error/body-limit code but increased the stripped `gregg` release binary from 3,609,472 to 4,264,912 bytes (+655,440 / +18.2%).

Eggfetch 0.1.7 now publishes the upstream footprint work motivated by that result. Its Cargo feature graph provides:

```text
transport-http1  -> primitive Hyper H1 support
standard-route   -> ordinary DNS -> TCP/TLS -> Hyper route
standard-http1   -> transport-http1 + standard-route + high-level-url

http1            -> native-http1 + high-level-url
                    + logical-retry + redirects + basic-auth
native-http1     -> transport-http1 + standard-route + advanced-routing
```

For Gregg, `standard-http1` is the intended profile. Gregg does not use custom Dialer, resolved-target pinning, SNI override, local-address/socket-option routing, UDS HTTP routing, logical eggfetch retries, redirect following, or Basic auth.

The published 0.1.7 tree also includes the earlier upstream dependency reductions: DashMap was removed from core pool/cache ownership and `eggfetch-http-connect` is proxy-owned instead of unconditional.

## Upstream review relevant to Gregg

### Lean standard-route profile

The upstream 2026-09-18 footprint qualification measured the same real high-level request path with Rustls/WebPKI:

```text
aligned reqwest:                     3,079,840 stripped
eggfetch full http1 compatibility:   3,669,840 stripped
eggfetch policy-lean only:           3,600,816 stripped
eggfetch standard-http1 lean:        3,111,904 stripped
```

On that x86_64 Linux / thin-LTO / unwind profile, `standard-http1` reduced the full eggfetch binary by 557,936 bytes and ended only 32,064 bytes (+1.0%) above aligned reqwest. `eggfetch_core` text dropped by roughly 49% because the advanced connector/client monomorphizations and retry/redirect pipeline were absent.

Those byte counts are evidence for the feature choice, not a Gregg acceptance threshold. Gregg uses fat LTO, one codegen unit, stripped symbols and `panic = "abort"`; its result must be measured independently.

### Capability retained by `standard-http1`

The lean profile still provides everything Gregg currently uses:

- high-level `Client` / `RequestBuilder` URL requests;
- `send_detailed()` and `RequestFailure`;
- typed `NetworkFailureKind::{Dns, ConnectionRefused, Connect}`;
- Bearer auth via `AuthScheme::bearer`;
- standard DNS/TCP/HTTP/1 transport;
- Rustls HTTPS when `tls-rustls` is selected;
- packaged WebPKI trust roots when `tls-native-roots` is not selected;
- logical pool admission and Hyper keep-alive reuse;
- pool/connect/write/read/total timeout fields;
- decoded-body caps and bounded `bytes()`;
- cancellation and normal response/status handling.

The lean profile intentionally does not compile:

- `advanced-routing`;
- `logical-retry`;
- `redirects`;
- `basic-auth`.

Because `redirects` is absent, `ClientBuilder::follow_redirects()` is also absent. A 3xx response is returned directly without a second hop. Gregg's existing redirect-not-followed tests therefore remain the product contract; the implementation should delete the now-redundant builder calls rather than re-enable `redirects` only to call `follow_redirects(false)`.

### eggfetch 0.1.7 total-deadline correction

The coordinated 0.1.7 patch restores `Timeout.total` as one absolute wall-clock deadline from logical request start through response-body EOF/trailers.

Relevant semantics:

- total starts once at logical request start;
- it covers pool/connect/headers and body consumption;
- it never resets on chunk arrival or first body poll;
- read remains a per-chunk inactivity timeout;
- `Total` wins an observable tie with `Read`;
- timeout terminalization releases the logical pool permit without waiting for response-body drop;
- `max_decoded_body_size` remains the authoritative hard bound for unknown, false or chunked body lengths;
- no new dependency, MSRV or public `ResponseBody` shape is required for this correction.

Gregg currently sets the same configured duration into all five fields, including `total`, but after headers it classifies every `response.bytes().await` error except `DecodedBodyTooLarge` as `NetworkError`. With 0.1.7, a real aggregate request timeout may now correctly surface during body consumption. Flattening that new `Error::Timeout` into `NetworkError` would make Gregg's stable category disagree with its configured whole-request deadline.

Plan 119 therefore intentionally narrows one Plan-118 preservation rule: a body-stage eggfetch timeout is `Timeout`; non-timeout post-header body failures remain `NetworkError`.

## Required implementation

### 1. Capture the current Gregg baseline

Before changing Cargo features, record on the current Plan-118 tree:

```sh
cargo build --release -p gregg
cargo tree -p gregg -e normal
cargo tree -p gregg -e features
cargo tree -p gregg --duplicates
cargo tree -p greggd -e normal
cargo tree -p gregg-update -e normal
```

Record at minimum:

- stripped `gregg` binary bytes;
- resolved `eggfetch-core` version/features;
- presence of `dashmap`, `eggfetch-http-connect`, `httpdate`, and direct core `base64`/retry-policy edges where visible;
- normal tree line count and duplicate-tree observation;
- confirmation that `greggd` and `gregg-update` do not depend on eggfetch.

Plan 118's 4,264,912-byte 0.1.5 measurement is the historical reference, but the implementation should still capture a fresh current-tree baseline.

### 2. Upgrade to the published lean profile

Change `crates/gregg/Cargo.toml` to:

```toml
eggfetch-core = {
    version = "0.1.7",
    default-features = false,
    features = ["standard-http1", "tls-rustls"],
}
```

Update `Cargo.lock` through ordinary Cargo resolution.

Do not enable:

```text
http1
native-http1
advanced-routing
logical-retry
redirects
basic-auth
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

Gregg already owns `serde_json` and `url`; do not move those responsibilities into optional eggfetch features.

Do not pin transitive Hyper/Rustls versions around eggfetch.

### 3. Remove redirect builder calls instead of restoring the redirect feature

Remove `.follow_redirects(false)` from:

- `HttpClient::new()`;
- `HttpClient::new_with_observer()`;
- `EggpoolClient::with_env_lookup()`.

Keep the existing redirect regression tests. They should prove the stronger compile-time contract:

```text
standard-http1 has no redirect-following engine
3xx is returned to Gregg as the first response
Gregg does not issue a second-hop request
```

Do not enable the `redirects` feature merely to preserve the old builder spelling.

Update comments that currently say "redirects disabled" to say, where useful, that redirect following is not compiled for Gregg's lean profile and 3xx responses pass through directly.

### 4. Keep the existing five-field timeout configuration

Retain the current explicit timeout constructor:

```rust
eggfetch_core::Timeout {
    pool: Some(timeout),
    connect: Some(timeout),
    write: Some(timeout),
    read: Some(timeout),
    total: Some(timeout),
}
```

Do not replace it with `Timeout::from_secs` or another scalar constructor: in 0.1.7 the scalar constructors still leave `total` unset.

This remains a deliberate Gregg policy: the configured request timeout is both the per-phase ceiling and the aggregate wall-clock ceiling.

### 5. Classify body-stage eggfetch timeouts as Gregg Timeout

Update the successful-header body-consumption paths in both `poller.rs` and `eggpool.rs`.

Today they effectively do:

```text
DecodedBodyTooLarge -> BodyTooLarge
anything else       -> NetworkError
```

Under 0.1.7, change the mapping to:

```text
DecodedBodyTooLarge                         -> BodyTooLarge
Error::Timeout { .. }                       -> Timeout
Error::TransportIoTimeout { .. }            -> Timeout
other post-header body error                -> NetworkError
```

Use typed enum matching only. Do not inspect error text.

Keep request-header/connect failures on the existing `RequestFailure::is_timeout()` + `NetworkFailureKind` path.

This does not add a new outcome variant. It makes body consumption honor the stable `PollOutcome::Timeout` / `EggpoolFetchOutcome::Timeout` categories already used for the configured whole-request deadline.

If implementation review proves `TransportIoTimeout` cannot reach these standard-route body paths, matching it is still acceptable for consistency with `RequestFailure::is_timeout()`; do not add a new transport category.

### 6. Preserve all other Plan-118 application semantics

Systems polling must remain:

```text
GET /v2/status first
404 only -> GET /v1/status
other v2 status -> surface status
malformed/invalid/unsupported v2 -> no v1 fallback
warming/failure payload semantics unchanged
same PollOutcome variants
same latency/generation/scheduler ownership
64 KiB body cap
four idle connections per host
```

EggPool must remain:

```text
request-local Bearer auth
missing/invalid secret behavior unchanged
401 -> Unauthorized
403 -> Forbidden
404 -> StatsUnavailable
other non-2xx -> HttpStatus
bad JSON -> DecodeError
semantic invalidity -> InvalidSummary
16 KiB body cap
two idle connections per host
same worker cadence/generation/cancellation
HTTP and HTTPS URL support
```

No automatic eggfetch logical retry is compiled or configured.

### 7. Prove the lean feature/dependency graph

After resolution, inspect:

```sh
cargo tree -p gregg -e features
cargo tree -p gregg -e normal
cargo tree -p gregg --duplicates
```

The Gregg feature graph must show `standard-http1` + `tls-rustls`, not the `http1` compatibility alias.

Confirm that Gregg does not activate:

```text
eggfetch-core/advanced-routing
eggfetch-core/logical-retry
eggfetch-core/redirects
eggfetch-core/basic-auth
eggfetch-core/proxy
```

Confirm `dashmap` is gone from the eggfetch pool/cache closure and `eggfetch-http-connect` is not resolved for Gregg's non-proxy profile. If either still appears because of an unrelated dependency, identify the actual owner instead of adding exclusions.

Do not treat `getrandom` or Base64-related crates anywhere in the whole tree as proof of a failed split: Rustls/ring/PEM may legitimately own transitive randomness/Base64. The relevant check is that eggfetch's retry/basic/proxy capabilities are not activated.

### 8. Add focused 0.1.7 timeout-lifecycle regressions

Keep all existing Plan-118 tests and add deterministic tests for the newly reachable body-timeout behavior.

#### Systems poller

Add a local server case where:

1. response headers arrive before the configured total deadline;
2. the body then stalls beyond that deadline;
3. `HttpClient::poll()` returns `PollOutcome::Timeout`, not `NetworkError`.

Add or adapt a continuous-progress case if practical:

- chunks arrive frequently enough that read inactivity does not fire;
- aggregate request lifetime exceeds `total`;
- result is still `PollOutcome::Timeout`.

The server must be loopback-only and deterministic. Do not use public networking.

#### EggPool

Add the equivalent headers-first/body-stall test and require
`EggpoolFetchOutcome::Timeout`.

Retain:

- fixed/chunked over-cap -> `BodyTooLarge`;
- ordinary premature body failure -> `NetworkError`;
- redirect response not followed;
- closed-port typed/generic behavior;
- Bearer secret non-leakage.

If one shared loopback helper can express these scenarios without obscuring the two clients' distinct 64 KiB/16 KiB policies, reuse it. Do not create a generalized HTTP test framework for this plan.

### 9. Remeasure Gregg's actual release footprint

After the final code/lockfile state:

```sh
cargo build --release -p gregg
cargo tree -p gregg -e normal
cargo tree -p gregg -e features
cargo tree -p gregg --duplicates
```

Record:

- stripped binary bytes;
- absolute/percentage delta versus the fresh Plan-119 baseline;
- comparison with Plan 118's 4,264,912-byte eggfetch 0.1.5 record;
- historical comparison with the 3,609,472-byte pre-eggfetch reqwest record;
- normal tree/dependency observations.

A substantial reduction is expected from upstream attribution, but no exact byte target is an acceptance criterion because Gregg's release profile differs from eggfetch's qualification profile.

If the binary does not materially shrink, use `cargo tree -e features` and, if already available in the developer environment, `cargo bloat --release -p gregg --crates` to explain why before closure. Do not add `cargo-bloat` as a project dependency or CI requirement.

### 10. Update active documentation without rewriting history

Update current references that describe Gregg's eggfetch profile:

- `architecture/workspace.md` — current Plan-118 dependency disposition should become eggfetch-core 0.1.7 with `standard-http1 + tls-rustls`; keep older Plan-117/reqwest history truthful;
- `architecture/gregg-client.md` — describe compile-time no-redirect/no-logical-retry profile and total-through-body semantics;
- `.opencode/skills/gregg-client/SKILL.md` — same operational ownership;
- `crates/gregg/README.md` only if its dependency/profile wording requires correction;
- `plans/README.md` — register and later close Plan 119.

Do not edit Plan 118's historical closure numbers or claim it used a feature profile that did not exist in 0.1.5.

## Implementation sequence

### Step 1 — baseline and manifest update

Capture the current release/tree baseline, then update to `eggfetch-core 0.1.7` + `standard-http1,tls-rustls` and resolve the lockfile.

Immediately run a targeted `cargo check -p gregg`. The expected compile fallout is the removed `follow_redirects(false)` API.

### Step 2 — remove redirect configuration and prove core API compatibility

Delete the three redirect-builder calls.

Confirm without application redesign that the lean profile still compiles Gregg's use of:

- `Client::builder()`;
- timeout/idle/body-limit builder methods;
- `Client::get()`;
- request `max_decoded_body_size()`;
- `send_detailed()`;
- `RequestFailure` / `NetworkFailureKind`;
- `AuthScheme::bearer()`;
- `Response::bytes()`.

Run the existing focused poller and EggPool redirect/auth/status/body tests before making timeout-classification changes.

### Step 3 — adopt the total-through-body timeout semantics

Update only the body-consumption error mapping in Systems and EggPool.

Add the deterministic headers-first/body-stall tests. Ensure non-timeout body failures remain generic network errors and body-limit failures remain `BodyTooLarge`.

### Step 4 — dependency and footprint qualification

Run the feature/tree checks and rebuild the fat-LTO release binary.

Record the evidence in the Plan 119 closure section. If the feature graph accidentally activates `http1`, advanced routing, retry, redirects or Basic auth, correct the manifest/feature owner before accepting any size measurement.

### Step 5 — docs and full verification

Update the active architecture/skill/index references and run the ordinary Gregg validation/release preflight.

## Validation

Required local checks on the final implementation tree:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
cargo +1.89 test --workspace --all-targets --all-features
./scripts/check-local.sh --release
```

Also run focused tests for the Systems poller and EggPool before the full suite.

Because the dependency and cfg surface changes across all supported targets, require the existing ordinary CI workflow to pass its current Linux, macOS arm64, macOS Intel, Windows and Rust 1.89 MSRV jobs.

No new workflow, job, matrix or size gate is needed.

## Acceptance criteria

Plan 119 is complete only when:

1. `crates/gregg/Cargo.toml` uses published `eggfetch-core 0.1.7` with `default-features = false, features = ["standard-http1", "tls-rustls"]`.
2. `Cargo.lock` resolves the published 0.1.7 core and no stale 0.1.5 eggfetch core remains.
3. Gregg does not activate eggfetch `advanced-routing`, `logical-retry`, `redirects`, `basic-auth` or `proxy`.
4. The three `.follow_redirects(false)` calls are removed; existing redirect tests prove no second hop occurs under the lean profile.
5. Systems and EggPool retain their existing pool sizes, five-field timeout configuration, body caps, typed request failure mapping and all stable non-timeout outcomes.
6. A response whose headers arrive before the deadline but whose body exceeds `Timeout.total` maps to Gregg's existing `Timeout` category for both Systems and EggPool.
7. Ordinary post-header non-timeout body failures remain `NetworkError`; oversized bodies remain `BodyTooLarge`.
8. Bearer auth continues working without enabling `basic-auth`, and secret redaction/non-leakage tests remain green.
9. `dashmap` is no longer owned by eggfetch in Gregg's graph and `eggfetch-http-connect` is absent from the non-proxy Gregg profile, subject to truthful unrelated-owner accounting.
10. No `eggfetch-core` production dependency enters `greggd` or `gregg-update`.
11. The final stripped `gregg` release size and dependency-tree deltas are recorded under the same release profile as Plan 118.
12. Current architecture/client-skill documentation describes 0.1.7, the lean standard route, compile-time no-redirect/no-logical-retry policy and total-through-body deadline accurately.
13. Local validation, Rust 1.89 validation, release preflight and the existing cross-platform CI workflow are green.
14. The closure record identifies the implementation SHA, resolved eggfetch version/features, final footprint delta and exact CI run.

## Non-goals

Do not expand this plan into:

- HTTPS support for ordinary greggd Systems endpoints;
- eggfetch logical retries or Gregg retry-policy changes;
- redirect following;
- Basic auth;
- proxy/cookie/compression/JSON/multipart/H2/H3 features;
- custom Dialer, resolved-target, SNI override, socket-option or UDS HTTP routing;
- migration to `native-http1` / `execute_http_body` solely for size;
- changes to EggPool cadence or Systems scheduler/backoff;
- protocol/schema/outcome redesign;
- TLS native-root policy changes;
- new CI or benchmark infrastructure;
- changes to `greggd`'s Axum/Hyper server;
- changes to `gregg-update`'s bounded external curl path;
- rewriting the truthful historical record in Plan 118.

## Closure record

Implementation complete; remote CI verification pending (run ID recorded
below once green).

- Baseline (Plan-118 tree, fresh measurement): stripped `gregg` release
  binary 4,264,912 bytes (matches the Plan 118 closure record);
  `eggfetch-core 0.1.5` with `http1` + `tls-rustls`; normal tree 362
  lines; `dashmap` owned by eggfetch pool/cache closure; no
  `eggfetch-core` in `greggd` or `gregg-update` (zero matches each).
- Manifest change: `crates/gregg/Cargo.toml` now uses published
  `eggfetch-core 0.1.7` with `default-features = false, features =
  ["standard-http1", "tls-rustls"]`. `Cargo.lock` resolves 0.1.7; no
  stale 0.1.5 eggfetch core remains. `cargo update -p eggfetch-core`
  removed `dashmap`/`hashbrown`/`crossbeam-utils` edges.
- Redirect configuration: removed all three `.follow_redirects(false)`
  calls (`HttpClient::new`, `HttpClient::new_with_observer`,
  `EggpoolClient::with_env_lookup`); the method no longer exists in the
  lean profile. Existing `redirect_response_301` and
  `redirect_is_not_followed` tests still prove 3xx passthrough with no
  second hop.
- Timeout semantics: five-field constructor retained verbatim (scalar
  constructors still leave `total` unset). Body-consumption paths in
  `poller.rs` and `eggpool.rs` now map `Error::Timeout { .. }` and
  `Error::TransportIoTimeout { .. }` to the existing `Timeout`
  outcomes via typed enum matching only; `DecodedBodyTooLarge` still
  maps to `BodyTooLarge` and other post-header failures stay
  `NetworkError`. Request-header/connect classification is unchanged.
- New regressions (verified to fail as `NetworkError` with the mapping
  neutered, pass with it live): Systems `body_stall_after_headers_is_timeout`,
  Systems `slow_body_progress_beyond_total_is_timeout` (trickle every
  50ms under a 300ms total; elapsed bound proves total fired during
  progress), EggPool `body_stall_after_headers_is_timeout`.
- Feature graph (`cargo tree -p gregg -e features -i eggfetch-core`):
  `standard-http1` → `transport-http1` + `standard-route` +
  `high-level-url`, plus `tls-rustls` → `hyper-rustls`. No
  `http1`/`native-http1`/`advanced-routing`/`logical-retry`/`redirects`/`basic-auth`/`proxy`
  edges. Normal tree 352 lines (-10); `dashmap` and
  `eggfetch-http-connect` absent; `getrandom`/Base64 remnants belong to
  Rustls/ring/PEM, not eggfetch capabilities.
- Footprint (same fat-LTO release profile as Plan 118): stripped
  `gregg` 3,740,592 bytes — delta -524,320 (-12.3%) versus the fresh
  4,264,912 baseline and the Plan 118 record alike; +131,120 (+3.6%)
  above the 3,609,472-byte pre-eggfetch reqwest record.
- Docs: `architecture/workspace.md` gains a Plan 119 disposition
  section; `architecture/gregg-client.md` and both client skills
  describe the lean profile, compile-time no-redirect/no-retry policy
  and total-through-body deadline; `AGENTS.md` dependency line updated.
  `crates/gregg/README.md` needed no change (no transport wording).
  Plan 118 history untouched.
- Local validation: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `cargo test --workspace
  --all-targets --all-features`, `cargo doc --workspace --no-deps`
  (only pre-existing `greggd`/`gregg` intra-doc-link warnings, also
  present on the base tree), `./scripts/check-local.sh`,
  `cargo +1.89 test --workspace --all-targets --all-features` all green.
  One transient `gregg-update` curl-timing failure under parallel load
  passed in isolation and on every re-run. `./scripts/check-local.sh
  --release` passed all gates up to the clean-tree check on the
  pre-commit tree; re-run on the clean post-commit tree below.
- Implementation SHA: pending commit. CI run: pending.
