# Plan 125: eggfetch 0.2.0 lean client adoption

Status: complete.

Depends on: completed Plan 119's lean eggfetch client baseline and the current post-Plan-124 main branch.

## Objective

Upgrade Gregg's existing client-side `eggfetch-core` dependency from 0.1.7 to the newly published 0.2.0 release without widening the compiled transport capability set, changing observable polling behavior, or regressing the current client footprint.

This is deliberately a narrow dependency adoption. It does not move `gregg-update` off external `curl`; that experiment is Plan 126.

## Why this plan exists

Plan 119 adopted `eggfetch-core 0.1.7` with:

~~~toml
eggfetch-core = { version = "0.1.7", default-features = false, features = ["standard-http1", "tls-rustls"] }
~~~

That profile removed the broad compatibility-policy and advanced-routing graph while preserving the exact Gregg Systems/EggPool requirements. It also established deterministic regressions for redirect passthrough, absolute total deadlines through body EOF, body limits, and typed transport classification.

`eggfetch-core 0.2.0` was published on 2026-09-22. Upstream records 0.2.0 as API-preserving relative to 0.1.7: the Rust/Python/C/CLI surfaces, feature graph/defaults, MSRV 1.89, and dependency policy are intentionally unchanged. The 0.2.0 manifest retains the same `standard-http1` and `tls-rustls` profile Gregg already uses.

The release also includes private core ownership/allocation improvements on Gregg's compiled H1 path, including reduced request/header cloning and avoidable origin/response-lifecycle bookkeeping. These are useful but must not be treated as a reason to change Gregg's public behavior or to assume a footprint win without measurement.

The headline 0.2.0 streaming decompression fix for eggfetch issue #24 is not directly exercised by Gregg because Gregg does not enable any compression feature. Do not add compression merely because the fix exists.

## Required behavior contract

The Plan-119 transport contract remains authoritative.

### Systems polling

Preserve:

- v2-first polling with v1 fallback only on HTTP 404;
- endpoint/schema validation and all existing `PollOutcome` classifications;
- `standard-http1` single-dispatch behavior;
- 3xx passthrough without a second hop;
- no automatic retry;
- Rustls HTTPS support with the same trust profile currently compiled;
- four idle connections per host;
- the 64 KiB decoded-body cap;
- typed DNS / connection-refused / connect failure classification;
- body-stage `Error::Timeout { .. }` and `Error::TransportIoTimeout { .. }` mapping to Gregg's existing `Timeout` outcome;
- other post-header body failures remaining `NetworkError`;
- absolute `Timeout.total` behavior through response-body EOF.

### EggPool polling

Preserve:

- the independent EggPool client;
- two idle connections per host;
- 16 KiB decoded-body cap;
- request-local Bearer authentication;
- no credential material in errors/outcomes/logging;
- no redirects or retry;
- existing endpoint validation and stable outcome taxonomy;
- the same body-stage timeout mapping used by Systems polling.

### Capability exclusions

The final resolved feature graph must continue to exclude:

- `advanced-routing`;
- `logical-retry`;
- `redirects`;
- `basic-auth`;
- `proxy`;
- `eggfetch-http-connect`;
- HTTP/2 and HTTP/3;
- cookies;
- compression;
- multipart;
- eggfetch JSON;
- eggfetch tracing.

Do not replace the lean profile with the default feature set or the broad `http1` compatibility alias.

## Implementation

### A. Upgrade only the direct eggfetch dependency

Change `crates/gregg/Cargo.toml` to the 0.2 semver line while retaining the exact feature selection:

~~~toml
eggfetch-core = { version = "0.2", default-features = false, features = ["standard-http1", "tls-rustls"] }
~~~

Resolve the initial lockfile specifically to 0.2.0 rather than opportunistically updating unrelated packages:

~~~text
cargo update -p eggfetch-core --precise 0.2.0
~~~

Do not add direct Hyper, Rustls, URL, timeout, or transport-policy pins around eggfetch.

### B. Compile the current client source before changing application code

The upstream 0.2.0 Rust compatibility record still contains the Gregg-used surfaces:

- `Client::builder`;
- `Client::get`;
- `ClientBuilder::max_idle_connections_per_host`;
- `ClientBuilder::max_decoded_body_size`;
- `RequestBuilder::send`;
- `Response::status`;
- `Response::bytes`;
- `AuthScheme::bearer`;
- `Error::DecodedBodyTooLarge`;
- `Error::Timeout`;
- `Error::TransportIoTimeout`;
- `RequestFailure::network_failure_kind` / `NetworkFailureKind`.

Therefore first attempt the dependency-only change.

If compilation fails, adapt only to a documented 0.2.0 public-surface difference and record that difference in this plan. Do not refactor polling, endpoint, scheduler, worker, or state ownership as part of this upgrade.

### C. Re-run the Plan-119 semantic regressions

Retain and run the existing deterministic tests covering at minimum:

- redirect responses are returned and not followed;
- headers-first/body-stall maps to `Timeout`;
- slow body progress beyond the total deadline still maps to `Timeout`;
- oversized body maps to `BodyTooLarge`;
- connection refused and DNS failures preserve their existing typed outcomes;
- malformed/wrong-version payload behavior is unchanged;
- EggPool Bearer auth and endpoint/error handling remain unchanged.

Do not weaken assertions to accommodate the dependency update.

### D. Prove the feature graph stayed lean

Record the output or equivalent evidence from:

~~~text
cargo tree -p gregg -e features -i eggfetch-core
cargo tree -p gregg | grep -E 'eggfetch|dashmap|httpdate|base64|eggfetch-http-connect'
~~~

Use a platform-appropriate equivalent on Windows where needed.

Expected eggfetch feature ownership is still:

~~~text
standard-http1
  -> transport-http1
  -> standard-route
  -> high-level-url
tls-rustls
~~~

If a previously absent eggfetch capability appears, stop and investigate before closure.

### E. Remeasure the actual Gregg release footprint

The current recorded stripped fat-LTO `gregg` binary is 3,740,592 bytes after Plans 119 and 120-124.

Build and measure under the same release profile and target/toolchain used by the existing footprint record. Record:

- pre-change current-main size;
- post-change 0.2.0 size;
- byte delta and percentage;
- relevant `cargo tree` delta if size moves materially.

Do not attribute an upstream microbenchmark or upstream thin-LTO result directly to Gregg.

A small change is acceptable if explained by the new eggfetch implementation. Material unexplained growth must block closure until attributed or the dependency/profile is corrected.

### F. Documentation/state reconciliation

Update only documentation that currently describes the live dependency as 0.1.7.

At minimum inspect:

- `AGENTS.md`;
- `architecture/gregg-client.md`;
- `architecture/workspace.md`;
- relevant Gregg client skill files;
- `plans/README.md`.

Keep Plan 119 as the historical 0.1.7 adoption record. Add a short pointer from current-state documentation to Plan 125 rather than rewriting Plan 119's historical measurements.

Do not add release notes claiming compression fixes affect Gregg's current feature set.

## Verification

Run focused transport tests first, then the ordinary repository gates:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo +1.89 test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
~~~

Run the same release build/strip measurement used by Plan 119/Plans 121-124.

Finally run one ordinary existing CI workflow. Do not add a new workflow, matrix, benchmark service, or performance gate.

## Acceptance criteria

- [x] `crates/gregg/Cargo.toml` uses published `eggfetch-core 0.2` and the lockfile resolves 0.2.0.
- [x] The selected features remain exactly the lean `standard-http1 + tls-rustls` capability set needed by Gregg.
- [x] No advanced-routing/retry/redirect/Basic/proxy/compression/HTTP2/HTTP3 capability enters the Gregg graph.
- [x] Existing Systems and EggPool source compiles without behavior-expanding refactors.
- [x] 3xx responses remain unfollowed.
- [x] Absolute total-deadline/body-stall/trickle regressions remain green.
- [x] Body-limit and typed network classifications remain unchanged.
- [x] Bearer-auth handling remains unchanged and secrets remain redacted.
- [x] Current-main and post-upgrade stripped `gregg` sizes are recorded under the same build conditions.
- [x] Any material binary-size change is attributed before closure.
- [x] Full workspace tests, strict clippy, Rust 1.89 tests, local checks, and one ordinary CI run are green.
- [x] Current-state docs refer to eggfetch 0.2 while Plan 119 remains truthful historical evidence.
- [x] No `gregg-update` transport change is mixed into this plan.

## Closure record

Complete. Dependency-only change: `crates/gregg/Cargo.toml` now requires
`eggfetch-core 0.2` (lockfile resolves 0.2.0, checksum
`6cd254b8...`); no application source change was needed, confirming the
upstream API-preserving record for every Gregg-used surface
(`Client::builder/get`, `max_idle_connections_per_host`,
`max_decoded_body_size`, `RequestBuilder::send`, `Response::status/bytes`,
`AuthScheme::bearer`, `Error::DecodedBodyTooLarge/Timeout/TransportIoTimeout`,
`RequestFailure::network_failure_kind/NetworkFailureKind`).

- Feature graph stayed lean: `cargo tree -p gregg -e features -i
  eggfetch-core` shows only `standard-http1` (→ `transport-http1`,
  `standard-route`, `high-level-url`) plus `tls-rustls`; no
  advanced-routing/retry/redirect/Basic/proxy/compression/HTTP2/HTTP3
  capability entered the graph (only `base64ct` via `tls-rustls`'s
  `pem-rfc7468`, not Basic-auth `base64`).
- Plan-119 semantic regressions green: 46 poller tests plus EggPool tests,
  including redirect-301 passthrough, header/body stall → `Timeout`, slow
  trickle beyond total → `Timeout`, 64 KiB/60+10 KiB/chunked/close-delimited
  body caps → `BodyTooLarge`, refused/DNS classifications, and
  malformed/wrong-version handling.
- Footprint: stripped fat-LTO `cargo build --release -p gregg` measured
  3,740,592 bytes both at pre-change HEAD (separate worktree, same
  environment) and post-change — delta 0, no attribution needed.
- Gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace --all-targets
  --all-features` (stable 1.98.1 and `+1.89`), `cargo doc --workspace
  --no-deps`, and `./scripts/check-local.sh` (default) all green.
- Docs: `AGENTS.md`, `architecture/workspace.md` (new Plan 125 disposition,
  Plan 119 kept historical), `gregg-client` skill, `CHANGELOG.md`, and
  `plans/README.md` updated; no compression-fix release notes (Gregg enables
  no compression feature).
- `gregg-update` untouched (still external `curl`); transport consolidation
  is Plan 126.
- Remote CI run `35736793227` green across all five jobs (Linux, macOS
  arm64, macOS Intel, Windows, MSRV Rust 1.89).
