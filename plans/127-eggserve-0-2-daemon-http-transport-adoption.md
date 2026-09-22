# Plan 127: EggServe 0.2 daemon HTTP transport adoption

Status: ready for implementation once the upstream runtime gates below are satisfied.

Depends on: completed Plans 123-126, the current post-Plan-126 daemon/runtime baseline, and a published EggServe 0.2.x direct-server API that satisfies the lifecycle and connection-lifetime gates in this plan. This work is independent of the remaining Plan 091 soak record.

## Objective

Replace the Axum-only HTTP transport/router inside `greggd` with the published EggServe direct H1 application-server substrate while preserving Gregg's existing daemon protocol, state publication, critical-task supervision, long-lived polling behavior, cross-platform runtime ownership, and small-binary goals.

The intended production dependency boundary is:

~~~text
greggd
  -> eggserve-server 0.2.x
  -> eggserve-primitives 0.2.x
~~~

Do not add `eggserve-core`, `eggserve-static`, TLS, HTTP/2, HTTP/3, QUIC, Python, static-file serving, WebSockets, or another application framework merely to complete this migration.

This is a transport/runtime ownership change only. `gregg-protocol`, the sampler, collectors, cached publication model, client polling contract, service management, updater, and TUI remain application-owned and behaviorally unchanged.

## Why this plan exists

Current `greggd` uses Axum only for a small read-only HTTP surface:

- `GET /`;
- `GET /v1/status`;
- `GET /v2/status`;
- `GET /healthz`;
- `GET /v2/healthz`;
- fallback 404 behavior.

The application semantics already live outside Axum in `ServerState`: coherent v1/v2 publication, staleness, failed-health preservation, Windows v2-only behavior, and publication-time compact JSON caching. Axum is therefore serving primarily as the HTTP transport/router facade.

EggServe 0.2.0 introduced a direct embeddable H1 substrate in `eggserve-server`, with canonical transport-independent request/response values in `eggserve-primitives`. It supports caller-supplied `Service` implementations, pre-bound Tokio listeners, parser/header/target limits, bounded admission, request/handler/write deadlines, graceful connection shutdown, response normalization, and per-runtime observability without requiring the static-file or multiprotocol compatibility crates.

That is now a strong architectural match for `greggd`, but exact EggServe 0.2.0 has two important integration gaps that must not be papered over:

1. the direct `ServerHandle` owns its internal accept task and exposes `shutdown()` plus consuming `wait()`, but does not provide Gregg with an independently awaitable critical-task termination/error signal while retaining shutdown control; and
2. the direct runtime requires a nonzero `connection_total_timeout` and defaults it to 60 seconds, whereas Gregg's current Axum path permits a healthy keep-alive connection to remain reusable indefinitely and the Gregg client intentionally uses a long-lived pooled `eggfetch_core::Client`.

The migration is justified only if those two runtime contracts can be preserved cleanly through a published EggServe API. Do not weaken Gregg's supervision or silently impose periodic reconnects merely to adopt an EggStack dependency.

## Hard upstream gates

Before modifying Gregg's production HTTP path, inspect the exact published EggServe version that will be resolved.

### Gate A: critical-task lifecycle and error propagation

Gregg's current daemon supervisor treats the sampler and HTTP server as critical tasks:

~~~text
tokio::select! {
    shutdown signal
    HTTP server task completion
    sampler task completion
}
~~~

An unexpected clean HTTP-server exit, runtime error, or panic is a daemon failure. On an ordinary shutdown, the outer daemon owns one bounded cleanup deadline.

The EggServe integration must preserve all of the following:

- the listener is bound before the daemon publishes ready/running state;
- bind failure is returned synchronously through the existing startup error path;
- Gregg can await unexpected HTTP-runtime termination while the server is running;
- Gregg retains an independent way to request graceful HTTP shutdown;
- server runtime failure or panic is not swallowed;
- shutdown remains bounded by the daemon's existing supervision policy;
- Windows SCM readiness still occurs only after the listener is successfully bound;
- Unix SIGTERM/SIGINT and the local control socket still converge on the same shared daemon shutdown path.

Acceptable upstream shapes include a cloneable shutdown/controller half plus an independently awaitable completion future, a non-consuming `terminated()`/completion receiver with error propagation, or another documented direct-server API that proves the same semantics.

Exact API names are not prescribed by this plan.

**Do not** satisfy this gate by:

- removing the HTTP task from `RunOutcome` supervision;
- polling `ServerHandle::state()` on a timer;
- treating an internal EggServe task panic as success;
- moving readiness publication before bind;
- writing a second generic accept/runtime loop in Gregg solely to work around the handle API;
- importing the much broader `eggserve-core` compatibility stack solely for lifecycle control.

If the published direct API cannot satisfy the gate, stop before changing production dependencies and record the upstream blocker. Partial migration is not closure.

### Gate B: preserve healthy keep-alive lifetime semantics

The current `gregg` poller keeps a long-lived `eggfetch_core::Client` and a bounded idle pool per host. The current Axum server does not deliberately expire an otherwise healthy connection after a fixed total lifetime.

Do not inherit EggServe 0.2.0's 60-second `connection_total_timeout` default unnoticed.

The selected EggServe release must provide either:

- an explicit supported no-total-lifetime setting; or
- another documented configuration whose semantics are equivalent to the current Gregg server for a healthy keep-alive connection.

A merely "very large" finite timeout is not behavior parity and must not be presented as such.

If upstream deliberately cannot represent an unlimited healthy connection lifetime, stop and record that product tradeoff for a separate decision instead of silently introducing periodic reconnects in this plan.

## Required behavior contract

### HTTP routes and protocol payloads

Preserve the existing public daemon API exactly:

- `/` and `/v1/status` return the same v1 result;
- `/v2/status` remains universal;
- `/healthz` and `/v2/healthz` retain their current readiness/failure semantics;
- Windows v2-only publication continues to return v1 `NotServing` with the existing message;
- stale retained snapshots preserve the Plan-124 distinction between stored collector failure messages and ready-but-age-stale `"cached snapshot is stale"`;
- successful status responses remain compact JSON;
- JSON responses retain `content-type: application/json`;
- no schema field, serde representation, status code, route, or client negotiation rule changes.

The EggServe service must call into the existing `ServerState` decision functions. Do not move staleness/readiness policy into EggServe middleware or duplicate it in a second router model.

### Method and wire compatibility

Before deleting Axum, add/strengthen raw-wire regression coverage against the current server to freeze behavior that Axum currently supplies implicitly.

At minimum characterize and then preserve:

- GET on every documented route;
- HEAD on documented GET routes, including body suppression and representation metadata;
- unsupported method on a known route (current 405 behavior and `Allow` header, if present);
- unknown path 404 behavior;
- unknown method + unknown path behavior;
- response `Content-Length` / transfer framing for fixed status and health bodies;
- connection-close versus keep-alive behavior for ordinary requests;
- current automatically generated response headers.

Do not assume a framework default is irrelevant merely because it is not documented.

If EggServe would add a `Date`, `Server`, or other header that current Gregg does not emit, configure EggServe's response policy to preserve the established wire surface unless a separate user-visible change is explicitly approved and documented.

### Cached status-byte invariant

Plan 123 deliberately serializes v1/v2 successful status JSON once per publication and stores cheap-clone `bytes::Bytes`.

Preserve that invariant.

A migrated request path must not perform:

~~~text
cached_bytes.to_vec()
~~~

or otherwise copy the complete cached JSON buffer on every request merely to satisfy `ResponseBody::Bytes(Vec<u8>)`.

For EggServe 0.2.x without a shared fixed-body primitive, use its canonical known-length response stream over a single cheap-cloned `Bytes` chunk, or an equivalent upstream-supported zero-payload-copy path. The runtime must still emit fixed known-length framing rather than converting every status response to unknown-length chunked transfer.

If EggServe gains a canonical shared-`Bytes` fixed response body before implementation, prefer that simpler public API after verifying it does not copy.

Existing serialization-count tests remain authoritative: repeated fresh status requests after one publication must not increment the v1/v2 serialization counters.

### Health and fallback bodies

Health and error envelopes may continue to serialize on demand exactly as they do now.

Keep application-level error bodies under Gregg's control where Gregg currently supplies them. EggServe-generated transport/parser errors may use EggServe's bounded sanitized representation, but must not leak internal details.

### Runtime limits

Do not blindly inherit EggServe defaults.

Document every selected EggServe runtime field and classify it as one of:

1. exact current-behavior preservation;
2. transport hardening already equivalent to the current Hyper/Axum behavior; or
3. a genuinely new admission/timeout behavior.

This plan should not introduce category (3) merely because EggServe has a default.

In particular review:

- `max_connections`;
- `max_in_flight_requests`;
- header count and aggregate header bytes;
- request-target length;
- header-read timeout;
- handler timeout;
- request-body ceiling/policy;
- keep-alive idle timeout;
- total connection lifetime;
- max requests per connection;
- response-write timeout;
- graceful-shutdown timeout.

For connection/request concurrency, prefer behavioral parity for the migration. If a new finite admission ceiling is desirable, make it a separately justified follow-up after the transport replacement is proven.

The outer Gregg 10-second shutdown deadline remains authoritative. Configure EggServe so an inner graceful-drain deadline cannot race or exceed the daemon's outer cleanup policy.

### Request bodies

Gregg is a read-only status API and never needs request payloads, but current raw-wire behavior for GET-with-body must be characterized before migration.

Do not accidentally change a currently accepted/rejected request into a materially different response solely because EggServe's default body policy is stricter. Select an explicit service/request-body policy that preserves the established Gregg behavior where practical and keep the server protected by a finite body ceiling.

## Dependency boundary

### Add

Use ordinary semver on the smallest published 0.2.x line that passes the two hard upstream gates:

~~~toml
eggserve-server = { version = "0.2", default-features = false }
eggserve-primitives = { version = "0.2", default-features = false }
bytes = "1" # direct only if Gregg continues naming bytes::Bytes directly
~~~

If a post-0.2.0 patch/minor is required for the hard gates, record the exact initially resolved version and the upstream capability that made adoption possible.

### Remove when migration is proven

Remove direct dependencies that exist only for the Axum server/test facade and are no longer used, expected to include:

- `axum`;
- `tower` dev usage for `ServiceExt`;
- `http-body-util` dev usage for Axum body collection.

Do not remove a dependency that remains used elsewhere; prove with `cargo tree` and source search.

Do not add direct Hyper usage to Gregg. EggServe should remain the HTTP transport authority.

### Feature graph

The final `greggd` graph must not gain:

- EggServe static serving;
- TLS/rustls through the server side;
- HTTP/2 or HTTP/3;
- QUIC;
- Python bindings;
- filesystem-serving policy;
- Tower/Axum compatibility layers;
- compression;
- cookie/session machinery.

Use the direct H1 leaf crates only.

## Implementation sequence

### A. Freeze current Axum wire behavior

Before the dependency swap:

1. extend the existing raw-TCP server tests rather than introducing a new test framework;
2. capture the exact route/method/status/header/framing behavior enumerated above;
3. keep those tests transport-agnostic enough to run unchanged after the migration.

The tests are the compatibility oracle. Do not derive expected behavior from memory or from EggServe documentation.

### B. Verify the upstream gates

Against the actual published EggServe crate resolved by Cargo:

1. prove pre-bound-listener adoption;
2. prove independently observable unexpected server termination/error while retaining shutdown authority;
3. prove graceful shutdown/drain can be driven under Gregg's outer supervisor;
4. prove an unlimited healthy total connection lifetime can be configured;
5. record the exact APIs/config values used.

If any item fails, leave the production Axum path intact and stop. Do not commit a half-migrated transport or broaden dependencies to force completion.

### C. Implement one Gregg service adapter

Create a small private EggServe service around cloned `ServerState`.

The service owns only method/path dispatch and canonical response construction.

Preferred shape:

~~~text
GreggHttpService {
    state: ServerState
}

Service::call(request)
    -> inspect canonical method + target
    -> call existing ServerState decision
    -> construct canonical EggServe Response
~~~

Keep the existing state/publication helpers testable independently of HTTP.

Avoid a generic routing abstraction: five static read-only routes do not justify a second router framework.

### D. Preserve zero-copy cached status serving

Bridge `FreshCached(Bytes)` to EggServe without an O(payload) clone.

For a one-chunk known-length stream:

- clone the cached `Bytes` reference;
- declare the exact byte length;
- yield exactly that buffer once;
- let EggServe own HTTP framing and `Content-Length`;
- preserve the on-demand typed serialization fallback only when the publication-time cache is absent.

Add a focused regression demonstrating repeated requests reuse the publication-time serialization result.

### E. Replace the Axum serving boundary

Keep Gregg's existing bind-before-ready structure.

The target runtime ownership remains:

~~~text
greggd run
  -> bind TcpListener
  -> publish readiness callback
  -> start sampler critical task
  -> start EggServe HTTP critical runtime
  -> supervise shutdown/server/sampler
  -> request EggServe graceful shutdown
  -> join under one bounded outer deadline
~~~

Do not move signal/control-socket handling into EggServe.

Do not let EggServe initialize a global tracing subscriber.

### F. Rework tests around the transport-neutral boundary

Replace Axum `Router::oneshot`-specific unit tests with either:

- direct service-call tests over EggServe canonical requests for pure dispatch semantics; and
- raw loopback TCP tests for actual parser/framing/lifecycle semantics.

Keep state-only tests as state-only tests.

The result should reduce framework-specific testing rather than replacing one test-only router framework with another.

### G. Dependency and footprint review

Record before/after:

~~~text
cargo tree -p greggd
cargo tree -p greggd -e features
cargo tree -p greggd -i axum
cargo tree -p greggd -i eggserve-server
cargo tree -p greggd -i eggserve-primitives
~~~

Use platform-appropriate equivalents where needed.

Measure a clean stripped fat-LTO `greggd` release binary under the same target/toolchain/profile on current main and the EggServe candidate.

The historical post-Plan-123/126 reference is 2,432,408 bytes, but implementation must measure a fresh pre-change current-main baseline in the same environment rather than treating the historical number as the comparison sample.

If the candidate grows by both at least 5% and at least 128 KiB, stop and attribute the growth before closure. Unlike Plan 126 this threshold is a review gate, not an automatic rejection: a modest, explained transport-hardening cost may be justified, but the closure record must state the decision and evidence explicitly.

### H. Lightweight runtime comparison

Run a local release-mode loopback comparison using one fixed published snapshot and repeated `/v1/status` and `/v2/status` requests.

Record only enough descriptive evidence to catch a clear regression in:

- request throughput/CPU;
- allocation/copy behavior where practical;
- persistent keep-alive reuse;
- shutdown/drain behavior.

Do not add Criterion, iai, a permanent benchmark workflow, or CI performance thresholds.

The structural no-reserialization/no-payload-copy evidence is more important than unstable microsecond timing.

### I. Documentation reconciliation

If adoption lands, update current-state documentation at minimum:

- `AGENTS.md`: `greggd` server authority and dependency boundary;
- `architecture/greggd-daemon.md`: replace Axum references and update supervision diagram/text;
- `architecture/workspace.md`: direct EggServe dependency and feature boundary;
- `.opencode/skills/greggd-daemon/SKILL.md`: implementation guidance;
- `CHANGELOG.md`: internal server-runtime consolidation with no protocol change;
- `plans/README.md`: closure state.

Do not rewrite Plans 004, 009, 120, 123, or 124 as though they used EggServe historically. They remain truthful records of the architecture at those times.

## Verification

Run focused server/state tests first:

~~~text
cargo test -p greggd --all-features server
cargo test -p greggd --all-features stale
cargo test -p greggd --all-features status_serialization
~~~

Use the actual test filters available after implementation; do not add duplicate tests merely to satisfy these example names.

Then run:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo +1.89 check --workspace --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
~~~

Because the HTTP daemon runs on every supported platform, run one ordinary existing CI workflow and require Linux, macOS arm64, macOS Intel, Windows/SCM smoke, and MSRV Rust 1.89 to pass.

No new workflow, matrix, privileged runner, benchmark service, or evidence bundle is required.

## Acceptance criteria

- [ ] A published EggServe 0.2.x direct-server API satisfies the critical-task lifecycle/error-propagation gate without a Gregg-owned duplicate generic accept loop.
- [ ] The selected EggServe runtime can preserve unlimited healthy keep-alive total lifetime semantics rather than silently imposing the 0.2.0 60-second default.
- [ ] `greggd` production HTTP uses only `eggserve-server` + `eggserve-primitives` from EggServe; no core/static/TLS/H2/H3 compatibility surface is pulled in.
- [ ] The listener is still bound before readiness publication and bind failures retain the existing startup error path.
- [ ] Unexpected HTTP-runtime exit/error remains a critical daemon failure.
- [ ] Unix signal/control-socket and Windows SCM shutdown still converge on the existing shared daemon cleanup path.
- [ ] The outer Gregg bounded shutdown deadline remains authoritative and active requests drain/cancel deterministically.
- [ ] All existing public routes, v1/v2 payloads, status codes, staleness messages, Windows v2-only behavior, and content types are unchanged.
- [ ] HEAD/405/404/framing/header behavior is characterized before migration and remains equivalent afterward.
- [ ] Publication-time compact v1/v2 JSON is still serialized once and cached as cheap-clone bytes.
- [ ] Repeated fresh status requests do not copy the full cached JSON body per request.
- [ ] Long-lived Eggfetch polling reuses healthy keep-alive connections without forced periodic reconnects from an EggServe total-lifetime default.
- [ ] Runtime limits are explicit and no new finite admission/timeout policy is introduced accidentally.
- [ ] Direct Axum/Tower/body-util dependencies that became unused are removed.
- [ ] The final feature graph contains no EggServe static/TLS/H2/H3/QUIC/Python compatibility capability.
- [ ] Fresh before/after stripped `greggd` sizes are recorded and any material growth is attributed before closure.
- [ ] Lightweight loopback comparison finds no material regression requiring rollback.
- [ ] Focused tests, full workspace tests, strict clippy, Rust 1.89 check, docs, default local check, and one ordinary existing CI run are green.
- [ ] Current architecture/agent/skill documentation names EggServe as the server transport authority while historical plans remain intact.

## Explicit non-goals

Do not include:

- TLS, HTTPS, authentication, authorization, reverse-proxy behavior, or public-internet exposure;
- HTTP/2, HTTP/3, QUIC, WebSockets, SSE, streaming telemetry, push updates, compression, ETags, or new cache semantics;
- static-file serving or an embedded web UI;
- protocol v3 or daemon-version transport;
- client polling/scheduler changes;
- collector or sampler redesign;
- service-manager, control-socket, updater, installer, or release-asset changes;
- a new HTTP framework abstraction around five routes;
- a second production HTTP implementation or long-term Axum/EggServe feature flag;
- a Gregg-owned generic EggServe accept loop solely to bypass a missing upstream lifecycle API;
- permanent benchmark infrastructure or performance CI gates.

## Handoff note

At the Plan-127 creation baseline, Gregg main is `5b2c7e8c67616f872070e2acdcce90790f750fbf`.

The exact EggServe 0.2.0 direct server already supplies the desired application-service, pre-bound-listener, limits, canonical response, connection driver, observability, and shutdown primitives. The two unresolved adoption gates are the independently observable critical-task lifecycle/error channel and an explicit way to disable the hard total connection lifetime.

Check the currently published EggServe 0.2.x API first. If those capabilities have landed upstream, proceed with the migration. If not, stop cleanly and report the upstream requirements rather than weakening Gregg's current guarantees.
