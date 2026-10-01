# Plan 148: eggserve 0.4 direct-server adoption

Status: planned.

Depends on: completed Plan 127's EggServe direct-H1 daemon transport adoption and the current main branch. Independent of the remaining Plan 091 soak record and independent of Plan 147.

## Objective

Upgrade `greggd` from the currently resolved `eggserve-server 0.2.1` / `eggserve-primitives 0.2.0` pair to the current published EggServe direct-server line, `eggserve-server 0.4.0` with `eggserve-primitives 0.2.2`, without changing Gregg's public HTTP contract, daemon supervision model, cached-response ownership, runtime-limit policy, platform behavior, or lightweight dependency boundary.

This is a dependency-currentness and compatibility-qualification pass, not a server redesign.

## Current state

At plan creation on 2026-10-01:

- `crates/greggd/Cargo.toml` declares:

~~~toml
eggserve-primitives = { version = "0.2", default-features = false }
eggserve-server = { version = "0.2", default-features = false }
~~~

- `Cargo.lock` resolves:
  - `eggserve-primitives 0.2.0`;
  - `eggserve-server 0.2.1`.
- upstream `eggstack/eggserve` has published release `v0.4.0`;
- that release publishes:
  - `eggserve-server 0.4.0`;
  - `eggserve-primitives 0.2.2`;
- `eggserve-server 0.4.0` directly requires `eggserve-primitives 0.2.2`.

The existing `eggserve-server = "0.2"` requirement is the blocker: Cargo cannot resolve the current 0.4 server line until Gregg changes its direct semver requirement.

Plan 127 remains the historical record for the Axum-to-EggServe migration and the 0.2.x qualification. Do not rewrite its closure record.

## Upstream compatibility review

A source review of the published `v0.4.0` tag confirms that the Gregg-used direct-H1 API still exists.

Gregg currently consumes:

- `eggserve_primitives::canonical::{Response, ResponseBody, ResponseStream, StatusCode}`;
- `eggserve_primitives::request::Request`;
- `eggserve_primitives::request_body_policy::RequestBodyPolicy`;
- `eggserve_server::{service_fn_with_policy, RuntimeConfig, Server, ServerCompletion, ServerControl, Service, ServiceError}`;
- `Server::builder()`;
- `ServerBuilder::runtime(...)`;
- `ServerBuilder::from_listener(...)`;
- `ServerBuilder::build()`;
- `Server::start_with_service(...)`;
- `ServerHandle::into_parts()`;
- `ServerControl::shutdown()`;
- cancel-safe borrowed `ServerCompletion::wait(&mut self)`;
- `ResponseStream::with_known_length(...)`;
- the existing `RuntimeConfigBuilder` setters used by `greggd`.

Those surfaces remain present in the 0.4.0 server/primitives sources. The 0.4 release's headline Tower feature-edge cleanup and H1 trailer wire-delivery changes do not require Gregg to adopt Tower, HTTP interop, trailers, static serving, TLS, H2, H3, or QUIC.

Therefore implementation must attempt the dependency-only upgrade before changing application source.

## Required behavior contract

### HTTP wire behavior

Preserve the current public read-only daemon contract exactly:

- `GET /`;
- `GET /v1/status` where v1 is supported;
- `GET /v2/status`;
- `GET /healthz`;
- `GET /v2/healthz`;
- existing `HEAD` behavior on known routes;
- existing `405 Method Not Allowed` behavior and `Allow` header;
- existing `404` fallback behavior;
- exact JSON schemas and staleness/readiness semantics;
- current content types;
- current `Date` behavior and lack of a `Server` header;
- persistent HTTP/1.1 keep-alive behavior;
- no TLS/public-internet semantics added.

Do not reinterpret current behavior from Plan 127's historical baseline if current main has intentionally changed since then. Current source/tests are the compatibility oracle.

### Current runtime-limit policy

Preserve the values currently selected in `crates/greggd/src/server/mod.rs::runtime_config()` unless a 0.4 API incompatibility makes an exactly equivalent spelling necessary:

- `max_connections = 512`;
- `max_in_flight_requests = 512`;
- `max_headers = 100`;
- `max_buf_size = 417_792`;
- `max_header_bytes = 417_792`;
- `max_request_target_bytes = 65_536`;
- `max_request_body_bytes = 64 KiB`;
- standard response policy;
- `header_read_timeout = 10 s`;
- `handler_timeout = 30 s`;
- `body_read_timeout = 30 s`;
- `keep_alive_idle_timeout = 60 s`;
- `connection_total_timeout = 300 s`;
- `max_requests_per_connection = Some(1000)`;
- `response_write_timeout = 30 s`;
- `graceful_shutdown_timeout = 8 s`.

The current finite total-lifetime/request-count policy supersedes Plan 127's historical unlimited-lifetime adoption setting. Do not "restore" old Plan-127 values during this upgrade.

New EggServe 0.4 configuration fields that Gregg does not currently own must remain at safe upstream defaults unless current behavior requires an explicit value. Do not broaden this plan into a runtime-policy redesign.

### Daemon lifecycle and supervision

Preserve:

- Gregg binds the listener before readiness publication;
- bind failure remains a startup error;
- EggServe receives that pre-bound listener rather than rebinding;
- `ServerControl` remains separately clonable shutdown authority;
- `ServerCompletion` remains a single-owner, awaitable critical-task completion authority;
- unexpected EggServe termination/error remains a daemon-critical failure;
- Unix signal/control-socket and Windows SCM shutdown still converge on Gregg's shared cleanup path;
- Gregg's outer ten-second cleanup deadline remains authoritative;
- EggServe's inner graceful shutdown remains eight seconds;
- no global tracing subscriber or process-global lifecycle ownership moves into EggServe.

### Cached-response ownership

Preserve the current allocation/serialization boundary:

- publication-time v1/v2 status JSON remains cached as cheap-clone `Bytes`;
- ready-health single-flight behavior from Plans 139/144 remains unchanged;
- `json_response` and fallback responses continue to use a one-chunk known-length EggServe response stream;
- repeated fresh status requests do not deep-copy or reserialize the cached status payload;
- Gregg does not adopt response trailers merely because EggServe 0.4 improves H1 trailer delivery.

## Implementation

### A. Upgrade the direct dependencies narrowly

Change the server requirement to the current minor line and raise the primitives minimum to the current published patch:

~~~toml
eggserve-primitives = { version = "0.2.2", default-features = false }
eggserve-server = { version = "0.4", default-features = false }
~~~

Then perform targeted resolution:

~~~text
cargo update -p eggserve-primitives --precise 0.2.2
cargo update -p eggserve-server --precise 0.4.0
~~~

If command ordering causes Cargo to resolve both in one step, that is acceptable; the final lockfile must contain exactly the published target versions and must not opportunistically update unrelated packages.

Do not add `eggserve-core`, `eggserve-static`, `eggserve-h3`, direct Hyper server code, or a compatibility framework.

### B. Compile before changing `greggd` source

The inspected 0.4.0 public surface retains every Gregg-used direct-H1 API listed above.

Attempt the manifest/lockfile-only change first.

If current `greggd` compiles unchanged, keep it unchanged.

If compilation fails, constrain edits to the private EggServe adapter in `crates/greggd/src/server/` and the smallest directly related tests. Any adaptation must preserve current wire/lifecycle/runtime-limit semantics exactly.

Do not refactor daemon startup, sampler, control socket, SCM, protocol, collector, client, or updater code as part of the dependency upgrade.

### C. Freeze and exercise the existing server compatibility oracle

Run the existing Plan-127-derived server tests before weakening or rewriting anything.

The test set must cover at minimum:

- all five public routes;
- `GET` and `HEAD`;
- unknown-route `404`;
- wrong-method `405` and `Allow`;
- content type and content length/framing;
- `Date` / no-`Server` response metadata policy;
- accepted bounded request bodies;
- configured body/target/header limits where existing tests cover them;
- repeated persistent requests over one connection;
- stale/warming/failure/ready state behavior;
- v1-unavailable platform behavior;
- cached-body serialization reuse;
- Plan-144 concurrent ready-health single-flight behavior.

Do not update expected wire behavior solely because EggServe 0.4 emits something differently. A wire delta is a compatibility finding to investigate, not an automatically accepted new baseline.

### D. Qualify lifecycle/error propagation

Prove the current server ownership path still holds:

~~~text
greggd
  -> bind TcpListener
  -> Server::builder().from_listener(listener)
  -> start_with_service(...)
  -> handle.into_parts()
       -> ServerControl
       -> ServerCompletion
  -> publish readiness
  -> supervise sampler + HTTP completion + shutdown
  -> ServerControl::shutdown()
  -> join under Gregg outer deadline
~~~

At minimum retain deterministic tests for:

- clean graceful shutdown;
- completion observation independent from control ownership;
- unexpected runtime termination mapping to Gregg's existing critical error path;
- no missed shutdown transition;
- no hang past the outer deadline.

If EggServe 0.4 changes terminal error text internally, Gregg tests should assert the stable Gregg error classification/ownership contract rather than brittle upstream prose unless wire/operator behavior intentionally exposes that prose today.

### E. Preserve the direct-H1 feature boundary

Record:

~~~text
cargo tree -p greggd -e features -i eggserve-server
cargo tree -p greggd -e features -i eggserve-primitives
cargo tree -p greggd | grep -E 'eggserve|tower|axum|h3|quinn|rustls'
~~~

The final Gregg server graph must not enable or directly depend on:

- `eggserve-core`;
- `eggserve-static`;
- `eggserve-h3`;
- EggServe `tower`;
- EggServe `http-interop`;
- Axum;
- TLS/rustls server orchestration;
- HTTP/2 or HTTP/3;
- QUIC;
- Python bindings;
- static-file policy.

`eggserve-server`'s own direct H1 Hyper/http-body implementation remains expected transitive implementation detail.

The v0.4 release note about Tower no longer activating `tower-layer` is irrelevant to Gregg unless the feature graph unexpectedly enables Tower; if it does, treat that as a regression.

### F. Preserve known-length cached response semantics

The 0.2.2 primitives line still provides `ResponseStream::with_known_length`.

Keep Gregg's current one-chunk shared-`Bytes` status/fallback response construction unless a strictly mechanical API spelling change is required.

Do not migrate status bodies to:

- buffered copies;
- unknown-length/chunked streams;
- trailer-bearing streams;
- a new static-file response abstraction.

Existing serialization counters/tests must continue to prove publication-time caching and ready-health single-flight behavior.

### G. Measure the daemon footprint

Plan 127's historical EggServe adoption increased stripped `greggd` from 2,432,408 to 2,629,008 bytes. That is historical context only; implementation must measure a fresh current-main baseline because substantial work has landed since then.

Under the same target/toolchain/release profile, record:

- current-main stripped `greggd` size;
- post-upgrade stripped size;
- byte and percentage delta;
- relevant dependency/code attribution for material movement.

Use Plan 127's review threshold as a diagnostic trigger: if the candidate grows by both at least 5% and at least 128 KiB, stop and attribute the change before closure. This threshold requires review, not automatic rollback; the goal is still to adopt the maintained current server line when compatibility and footprint remain acceptable.

Also note any meaningful reduction. Do not infer Gregg performance or size from EggServe's release notes.

### H. Run a lightweight persistent-loopback smoke

Use existing tests or a temporary local probe, not permanent benchmark infrastructure, to confirm:

- repeated `/v1/status` / `/v2/status` requests reuse a persistent connection;
- current configured 300-second total lifetime and 1000-request cap are not accidentally replaced by 0.4 defaults;
- ordinary low-rate status polling shows no material functional regression;
- graceful drain remains bounded.

A permanent microbenchmark, Criterion suite, performance CI threshold, or synthetic load framework is not required.

### I. Reconcile current-state documentation

If the upgrade lands, update live documentation that names the old EggServe server line, at minimum inspecting:

- `AGENTS.md`;
- `architecture/greggd-daemon.md`;
- `architecture/workspace.md`;
- `.opencode/skills/greggd-daemon/SKILL.md`;
- `CHANGELOG.md`;
- `plans/README.md`.

Preserve Plan 127 as the historical 0.2 migration/qualification record.

Do not rewrite old plans to claim they used EggServe 0.4.

## Verification

Run focused server tests first, including the actual Plan-127/139/144 filters present at implementation time, then:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo +1.89 test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
~~~

Because the HTTP daemon runs across all supported targets, run one ordinary existing CI workflow and require the existing Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.89, and FreeBSD-native jobs to remain green.

No new workflow, matrix, self-hosted runner, benchmark service, or evidence bundle is required.

## Acceptance criteria

- [ ] `crates/greggd/Cargo.toml` permits `eggserve-server 0.4.x` and requires at least `eggserve-primitives 0.2.2` with default features disabled.
- [ ] `Cargo.lock` resolves `eggserve-server 0.4.0` and `eggserve-primitives 0.2.2`.
- [ ] No unrelated dependency is opportunistically upgraded.
- [ ] The dependency-only upgrade is attempted before any application source change.
- [ ] Any required source adaptation is confined to the private Gregg/EggServe server boundary and is justified by a concrete 0.4 API difference.
- [ ] All five public routes, GET/HEAD, 404/405, headers, framing, schemas, and readiness/staleness behavior remain compatible with current main.
- [ ] The current explicit runtime limits remain semantically unchanged, including 512 connection/request concurrency, 300-second total lifetime, 1000-request per-connection cap, and eight-second inner graceful shutdown.
- [ ] Pre-bound listener ownership and readiness ordering remain unchanged.
- [ ] Split `ServerControl` / `ServerCompletion` supervision remains intact and unexpected server completion remains daemon-critical.
- [ ] Gregg's outer ten-second cleanup deadline remains authoritative.
- [ ] Cached status bodies remain shared known-length `Bytes` streams without full-body copy/reserialization.
- [ ] Plan-144 ready-health single-flight semantics remain green.
- [ ] No trailers are introduced on Gregg responses.
- [ ] No EggServe core/static/H3/Tower/http-interop/TLS/H2/H3/QUIC/Python capability enters Gregg's server graph.
- [ ] Fresh current-main and post-upgrade stripped `greggd` sizes are recorded under the same conditions.
- [ ] Any >=5% and >=128 KiB growth is attributed and explicitly accepted or corrected before closure.
- [ ] Persistent-loopback behavior and bounded shutdown remain healthy.
- [ ] Strict clippy, full workspace tests, Rust 1.89 tests, docs, default local checks, and the existing six-job CI matrix are green.
- [ ] Current-state documentation names the 0.4 server line truthfully while Plan 127 remains historical.
- [ ] Plan 091 and Plan 147 remain independent.

## Explicit non-goals

Do not include:

- a daemon HTTP architecture rewrite;
- protocol/schema changes;
- new endpoints;
- TLS/HTTPS or public-internet exposure;
- HTTP/2, HTTP/3, QUIC, WebSockets, SSE, or response compression;
- Tower/Axum compatibility layers;
- static-file serving or embedded web UI;
- response trailers;
- trusted-proxy policy work;
- tunnel/upgrade feature work;
- changing current runtime limits for hardening or throughput experiments;
- client polling/scheduler changes;
- collector/sampler redesign;
- startup/control/SCM/updater/installer changes;
- permanent performance infrastructure;
- release publication or tagging.

## Handoff note

Begin with the two manifest/version changes and targeted Cargo resolution, then compile `greggd` unchanged. Upstream 0.4.0 source inspection shows the APIs Gregg actually uses are still exported, so application changes should be treated as exceptional and narrowly justified. Current `runtime_config()` and current raw-wire/lifecycle tests are the source of truth; do not accidentally revert to Plan 127's older unlimited-lifetime configuration while following its historical migration record.
