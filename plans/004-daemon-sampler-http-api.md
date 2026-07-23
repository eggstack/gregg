# Phase 4: daemon sampler and HTTP API

## Objective

Turn the native collector into a reliable foreground daemon process that samples on its own cadence, publishes immutable cached snapshots, exposes a minimal read-only HTTP/1 API, reports readiness accurately, and shuts down cleanly.

The server must be platform-neutral above the collector boundary. HTTP requests must never trigger native collection and multiple clients must not multiply collection work.

## Runtime architecture

Use this data path:

```text
NativeCollector
      |
      v
periodic sampler task
      |
      +--> latest valid immutable snapshot
      +--> readiness/last-error state
                    |
                    v
             read-only HTTP handlers
```

Use a single runtime process with a modest Tokio configuration. A current-thread runtime is preferred unless measurements demonstrate a need for multiple worker threads. The workload consists of short periodic native calls, a cached snapshot swap, and small HTTP responses.

Define separable components:

- `Sampler<C, Clock>` owns collector cadence and baseline progression.
- `SnapshotStore` owns the latest immutable snapshot and readiness metadata.
- `ServerState` exposes cloned read handles to HTTP routes.
- `run()` wires signals, sampler lifetime, listener, and graceful shutdown.

Do not hold a lock while serializing JSON if an immutable `Arc<StatusSnapshot>` can be cloned first. A standard `RwLock` is acceptable at one write per second; use a more specialized swap primitive only if justified by measurement.

## Sampling cadence

Default native sampling interval: one second.

Configuration should enforce sensible bounds, for example 250 milliseconds to 60 seconds, while documenting the chosen range. The TUI poll interval is independent and remains five seconds by default.

The sampler must:

1. Initialize static identity/capabilities.
2. Take the first native CPU-counter sample.
3. Remain in warming-up readiness until a valid second sample produces interval-derived metrics.
4. Publish only complete, protocol-valid snapshots.
5. Preserve the most recent valid snapshot if a later sample fails, while separately marking freshness/error state according to the health contract.
6. Recover automatically after transient failures and counter resets.
7. Avoid drift-prone repeated `sleep(interval)` scheduling where practical; missed ticks should not produce a runaway catch-up loop.

Define stale-snapshot policy explicitly. A recommended approach is:

- `/v1/status` may continue serving the last valid snapshot with its original observation timestamp.
- `/healthz` becomes degraded/not-ready after a configurable number of consecutive failures or maximum snapshot age.
- The response never rewrites `observed_at` to make stale data appear fresh.

Keep version 1 simple: no historical ring buffer and no per-client sampling cadence.

## HTTP surface

Expose only:

```text
GET /
GET /v1/status
GET /healthz
```

`GET /` is an alias of the current status route for convenient browser/curl inspection. `/v1/status` returns the canonical protocol snapshot. `/healthz` returns a small structured response suitable for service checks.

Recommended status behavior:

- Ready snapshot: `200 OK` with JSON.
- Warming up and no valid snapshot: `503 Service Unavailable` with health/readiness JSON.
- Collector failure before any valid snapshot: `503`.
- Last valid snapshot exists but health is degraded: `/v1/status` may return `200` with timestamped stale data; `/healthz` returns `503` or a documented degraded status.
- Unsupported methods: `405 Method Not Allowed` where the framework permits.
- Unknown routes: `404 Not Found`.

Use `application/json` and consistent compact serialization. Do not include pretty printing in normal responses.

## Bind behavior

Default bind configuration:

```text
host = "0.0.0.0"
port = 11310
```

Validate the host as an IP address for version 1 unless hostname binding is deliberately supported and tested. Validate the port as nonzero and within `1..=65535`. Binding to port zero should be allowed only in tests or an explicit development mode, not persisted as production configuration.

Log the effective listen address at startup. A bind failure must terminate with a nonzero exit rather than silently retrying another address or port.

## HTTP implementation constraints

Axum or a comparably small Tokio-compatible server is suitable. Configure only needed features:

- HTTP/1.
- JSON serialization.
- Tokio networking/signal support.
- No TLS stack.
- No cookies, sessions, multipart, WebSockets, compression, static files, or remote configuration.

Apply conservative server limits where available:

- Small request header limit.
- Short header/read timeout.
- No request body required for GET routes.
- Bounded concurrent connections if implementation complexity remains low.

Because the endpoint is local-network oriented, do not introduce an authentication framework in version 1. Preserve the read-only surface and document that binding `0.0.0.0` exposes metrics to reachable network peers.

## Shutdown and signals

Handle Ctrl-C and platform termination signals. Graceful shutdown should:

1. Stop accepting new connections.
2. Allow in-flight status responses a short bounded completion period.
3. Cancel or stop the sampling loop.
4. Flush structured logs.
5. Exit without corrupting configuration or leaving auxiliary processes.

The daemon owns no child process and no PID file.

## Logging and diagnostics

Use structured logging with a configurable level through a standard environment filter or CLI flag. Default logs should include:

- Startup version and platform.
- Effective config path and listen address, avoiding sensitive/unnecessary contents.
- Readiness transition from warming to ready.
- Collector failures and recovery, rate-limited or transition-based to avoid one log per second indefinitely.
- Shutdown reason.

Do not log every successful sample or every HTTP request by default. Such logging would add noise and overhead.

## Test architecture

Use synthetic collectors and an injectable clock. Required collector behaviors:

- Warm then succeed.
- Always fail.
- Succeed then fail repeatedly.
- Counter-reset transient then recover.
- Return invalid normalized metrics.
- Block or exceed a test timeout if sampler isolation is evaluated.

Tests should bind to loopback with an ephemeral test port and verify exact status codes, content types, schema fixtures, and route behavior. Avoid production-duration sleeps; use paused Tokio time or controlled clocks where possible.

Add concurrency tests proving that many simultaneous HTTP requests observe cached data and do not increase collector call count.

## Acceptance criteria

Phase 4 is complete when:

1. `greggd run` selects the native collector at compile time and starts a foreground process on Linux and macOS.
2. The sampler publishes no CPU-derived ready snapshot before a valid counter delta exists.
3. Requests serialize a cached immutable snapshot and do not call the collector.
4. Concurrent request tests prove collection frequency is independent of client count.
5. `/`, `/v1/status`, and `/healthz` implement documented status codes and JSON responses.
6. Snapshot timestamps remain truthful during collector failures.
7. Transient collection failures and counter resets recover without process restart.
8. Bind address, port, and sampling interval are validated before serving.
9. Unsupported routes/methods and malformed requests do not panic or expose internal errors.
10. Signal-triggered shutdown terminates sampler and listener cleanly on Linux and macOS.
11. Default logging is useful but does not emit successful samples or requests continuously.
12. The daemon server package remains free of TUI/client dependencies and passes `cargo package` validation.

## Handoff to phase 5

Expose a single foreground entry point accepting a fully validated daemon configuration and a shutdown signal. Phase 5 will add config discovery, service-manager commands, and installation assets around this entry point without changing its process model.
