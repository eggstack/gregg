# Roadmap: optional EggPool summary pane

Status: in progress; Phases 57–60 implementation is present, with Phase 60 hosted closure pending.

## Purpose

Add one optional EggPool statistics pane to the `gregg` client TUI without broadening Gregg into a general dashboard or changing `greggd`/`gregg-protocol`.

The work adds:

- one optional EggPool endpoint in Gregg's existing TOML configuration;
- `gregg eggpool add`, `gregg eggpool list`, and `gregg eggpool remove` commands that mirror the existing greggd endpoint-management conventions;
- authenticated or public reads from EggPool's existing `GET /api/stats/summary` endpoint;
- a compact four-value pane for accounted tokens, provider cache-read share, output-token throughput, and average TTFT;
- top-level pane cycling with `h`/Left and `l`/Right;
- EggPool period selection with `j`/Down and `k`/Up across 1 hour, 1 day, 7 days, and 30 days;
- no EggPool pane and no EggPool network activity when no EggPool entry exists in configuration.

The implementation must preserve the current compact system-monitoring product, existing greggd polling behavior, normal/condensed system layouts, atomic configuration storage, and local-first verification policy.

## Problem statement

Gregg already provides a small fleet-oriented TUI for `greggd` systems. EggPool already records and aggregates LLM usage statistics, but viewing the most useful top-line values currently requires opening its web dashboard or querying its JSON API separately.

EggPool's current summary endpoint already returns the required metrics and accepts the exact rolling windows needed by the requested interaction model:

```text
GET /api/stats/summary?period=1h
GET /api/stats/summary?period=24h
GET /api/stats/summary?period=7d
GET /api/stats/summary?period=30d
```

The endpoint is available only when EggPool's dashboard/statistics routes are enabled. It is unauthenticated when `[dashboard].public = true` and uses the normal EggPool API key when `[dashboard].public = false`.

This means the requested feature can be implemented entirely in the `gregg` client crate against EggPool's existing API. It does not require a new EggPool aggregation query, a Gregg protocol extension, or any `greggd` change.

## API compatibility finding

The existing EggPool summary response contains the required fields:

```text
accounted_tokens
cache_read_ratio
tokens_per_second
avg_ttft_ms
streamed_requests
period
```

The TUI must preserve EggPool's actual metric semantics:

| TUI label | EggPool field | Exact meaning |
| --- | --- | --- |
| `Accounted tokens` | `accounted_tokens` | input + output + provider cache-read + provider cache-write tokens |
| `Cache read share` | `cache_read_ratio` | cache-read tokens divided by input + cache-read + cache-write tokens |
| `Output tok/s` | `tokens_per_second` | output tokens divided by accumulated upstream latency |
| `Avg TTFT` | `avg_ttft_ms` | average first-byte latency for streamed requests |

Do not label `cache_read_ratio` as a request-level cache hit rate. Do not label `tokens_per_second` as request throughput or wall-clock traffic throughput. Do not use legacy `total_tokens` for the headline because EggPool defines it as fresh input + output only.

When `cache_read_ratio` is null, render `—`. When `streamed_requests == 0`, render average TTFT as `—` rather than `0 ms`.

## Governing principles

### 1. Keep EggPool optional and client-only

EggPool integration exists only in the `gregg` binary. Do not modify:

- `gregg-protocol`;
- `greggd` collectors, sampler, server, service management, or API;
- EggPool's database schema, query layer, or summary response;
- workspace dependency direction.

When no EggPool is configured:

- the available TUI pane set contains only the systems pane;
- `h`/Left and `l`/Right do not expose a placeholder EggPool pane;
- no EggPool request worker, interval, or network request is created.

### 2. Support one EggPool endpoint

The first implementation supports one optional EggPool endpoint:

```rust
pub eggpool: Option<EggpoolEntry>
```

This matches the requested two-pane interaction model: one systems pane and one EggPool pane. It avoids inventing instance selection, unsafe cross-instance aggregation, or another navigation dimension.

A second `eggpool add` must fail unless `--replace` is supplied. Supporting multiple EggPool instances is separate future work because total tokens may be summed, but cache ratios, average TTFT, and throughput cannot be correctly combined from the current summary payload without additional denominators/sample counts.

### 3. Reuse configuration persistence, not endpoint semantics

Reuse `ConfigStore`, atomic writes, locking, validation aggregation, UUID identity, `--config`, human/JSON list output, duplicate rejection, and `--replace` behavior.

Do not reuse the greggd `EndpointSpec` unchanged because:

- greggd defaults to port `11310`;
- EggPool defaults to port `11300`;
- EggPool may use HTTP or HTTPS;
- EggPool may require an API key environment reference.

Use a small EggPool-specific parser/model rather than making the greggd endpoint abstraction generic.

### 4. Never persist resolved credentials

Gregg configuration may store only an optional environment-variable name:

```toml
[eggpool]
id = "..."
host = "eggpool.local"
port = 11300
scheme = "http"
name = "Main EggPool"
api_key_env = "EGGPOOL_GREGG_API_KEY"
```

The secret value remains in the process environment. `eggpool list`, `eggpool list --json`, diagnostics, tests, and logs must never emit the resolved key.

When `api_key_env` is absent, Gregg sends no authentication header. When it is present and resolves, Gregg sends:

```text
Authorization: Bearer <value>
```

When it is present but missing from the environment, the pane reports a bounded local configuration error and does not send a request with an empty credential.

### 5. Separate top-level pane from systems layout

The current `ViewMode` models Normal versus Condensed system presentation and currently consumes `h`/`l`. The requested behavior assigns `h`/Left and `l`/Right to top-level pane cycling.

Represent these as two independent state dimensions:

```rust
enum Pane {
    Systems,
    Eggpool,
}

enum SystemViewMode {
    Normal,
    Condensed,
}
```

Use:

```text
h / Left    previous available pane
l / Right   next available pane
v           toggle Normal/Condensed while on Systems
j / Down    next system on Systems; longer period on EggPool
k / Up      previous system on Systems; shorter period on EggPool
```

Do not add a generic screen registry, nested focus framework, or configurable keymap.

### 6. Use one small EggPool request path

The client performs only:

```text
GET /api/stats/summary?period=<preset>
```

Do not consume timeseries, account, model, latency, events, runtime, or recent-request endpoints. Do not add charts, history storage, provider drill-down, per-model tables, or generalized EggPool API bindings.

### 7. Poll at a proportionate cadence

EggPool statistics queries should not run on Gregg's default five-second greggd cadence.

Required behavior:

- fetch immediately when the EggPool pane is first entered;
- fetch immediately when the period changes;
- fetch on `Ctrl-R` while the EggPool pane is active;
- refresh at most once every 60 seconds while the EggPool pane remains active;
- preserve the most recent successful summary while a refresh is pending or a later transient request fails, while visibly reporting stale/error state;
- stop issuing periodic EggPool requests when the systems pane is active.

Use one bounded worker/channel or equivalent direct Tokio task. Do not build a generalized datasource scheduler.

### 8. Keep verification lightweight

Use focused config/CLI tests, request parsing tests against a local synthetic HTTP server, reducer tests, and Ratatui buffer tests. Final closure uses existing local checks and ordinary CI.

Do not add:

- a new workflow;
- a dedicated EggPool service in CI;
- retained JSON evidence;
- screenshot artifacts;
- long-running polling tests;
- live provider/account credentials;
- release automation.

## Target product behavior

### Configuration and CLI

Representative commands:

```text
gregg eggpool add eggpool.local
gregg eggpool add 192.168.1.20:11300 --name "Main EggPool"
gregg eggpool add eggpool.local --api-key-env EGGPOOL_GREGG_API_KEY
gregg eggpool add eggpool.example.net:443 --https --replace

gregg eggpool list
gregg eggpool list --json

gregg eggpool remove eggpool.local
gregg eggpool remove eggpool.local:11300
```

Default behavior:

```text
port    11300
scheme  http
```

The command surface remains nested under `eggpool` so the existing top-level `add`, `list`, and `remove` continue to mean greggd systems.

### Available panes

```text
systems present, no EggPool  -> Systems only
systems present, EggPool     -> Systems, EggPool
no systems, EggPool          -> EggPool only
neither configured           -> existing empty-configuration diagnostic, updated to mention both add commands
```

The initial pane is Systems when at least one system exists. It is EggPool when EggPool is the only configured source.

### EggPool period model

Use a fixed enum and direct mapping:

```text
Hour   -> API `1h`  -> UI `1 hour`
Day    -> API `24h` -> UI `1 day`
Week   -> API `7d`  -> UI `7 days`
Month  -> API `30d` -> UI `30 days`
```

Initial period is `Hour`.

`j`/Down moves toward longer periods. `k`/Up moves toward shorter periods. The ends clamp rather than wrap so the keys retain an intuitive shorter/longer meaning.

Never send `1d`; EggPool's accepted daily preset is `24h`.

### EggPool pane

Wide representative layout:

```text
EggPool — Main EggPool                         Window: 1 hour

Accounted tokens      Cache read share
12.48M                74.2%

Output tok/s          Avg TTFT
83.6                  412 ms

Updated 17:42:18        h/l pane  j/k window  Ctrl-R refresh
```

Narrow layouts may stack one metric per row. No chart, sparkline, border-heavy card framework, table, or scrolling metric list is required.

### Failure rendering

The pane must remain usable and nonfatal for:

```text
missing API-key environment variable
401 unauthorized
403 forbidden
404 statistics routes unavailable
request timeout
connection refused
DNS failure
other network failure
oversized response
invalid JSON
unsupported/malformed summary values
```

Actionable examples:

```text
EggPool authentication required
EggPool API key environment variable is not set
EggPool stats unavailable — enable EggPool dashboard/statistics routes
EggPool unreachable — connection refused
EggPool response invalid
```

Do not print raw response bodies, credentials, or long reqwest error chains in the TUI.

## Phase map

| Phase | Plan | Outcome |
| --- | --- | --- |
| 57 | `057-eggpool-config-and-cli.md` | Add one optional validated EggPool entry and nested add/list/remove commands through the existing atomic config store. |
| 58 | `058-eggpool-summary-client-and-refresh.md` | Add the typed summary response, conditional authentication, bounded request/error handling, fixed periods, and proportionate refresh worker. |
| 59 | `059-eggpool-pane-state-controls-and-rendering.md` | Separate pane state from system layout, remap controls, add period-aware state and the compact EggPool renderer. |
| 60 | `060-eggpool-pane-integration-and-lightweight-closure.md` | Reconcile runtime wiring, compatibility, docs, focused tests, and ordinary local/CI closure without new infrastructure. |

## Dependency graph

```text
57 -> 58
57 -> 59
58 + 59 -> 60
```

Phase 59 may use synthetic summary values before the HTTP worker is complete. Phase 60 is the only phase that should perform final cross-module runtime reconciliation.

## Program scope

### In scope

- one optional EggPool endpoint;
- host/port/scheme/name/API-key-environment-reference configuration;
- nested EggPool add/list/remove commands;
- public and API-key-protected EggPool summary reads;
- fixed `1h`, `24h`, `7d`, and `30d` periods;
- four accurately labeled summary values;
- a top-level Systems/EggPool pane model;
- `h`/Left and `l`/Right pane cycling;
- `j`/Down and `k`/Up context-sensitive system/period movement;
- one explicit key for Normal/Condensed system layout;
- bounded refresh/error/stale behavior;
- EggPool-only and systems-only configurations;
- focused tests and active documentation updates.

### Out of scope

- multiple EggPool endpoints;
- aggregation across EggPool instances;
- modifying EggPool's statistics API or database;
- making EggPool stats available when its dashboard/statistics routes are disabled;
- direct SQLite access to EggPool's database;
- provider/account/model drill-down;
- request lists, error logs, costs, reliability, routing, runtime, or event panes;
- charts, history, alerts, thresholds, notifications, exports, or persistence of sampled summaries;
- generic plugin, datasource, dashboard, widget, or screen frameworks;
- configurable keybindings, pane order, metric selection, or layout;
- credential files, keyrings, secret storage, or interactive login;
- TLS configuration beyond selecting `http` or `https` for the configured endpoint;
- redirects, cookies, compression requirements, browser embedding, or dashboard HTML scraping;
- changes to `greggd`, `gregg-protocol`, or EggPool;
- new dependencies unless implementation proves an existing dependency cannot perform the required work;
- new CI workflows, services, evidence files, release gates, or publication automation.

## Core invariants

1. Existing greggd system configuration remains valid and unchanged.
2. Config schema version remains `1`; the EggPool field is optional and defaults to absent.
3. No resolved API key is written to disk or emitted in output.
4. No configured EggPool means no EggPool pane, worker, timer, or request.
5. The EggPool pane uses only `/api/stats/summary`.
6. The period-to-query mapping is exactly `1h`, `24h`, `7d`, `30d`.
7. Metric labels preserve EggPool's documented semantics.
8. Rendering performs no I/O.
9. Network work does not block terminal input or drawing.
10. Systems selection/viewport behavior remains unchanged inside the Systems pane.
11. Normal/Condensed system layouts remain available after `h`/`l` becomes pane navigation.
12. EggPool errors never terminate the TUI.
13. Response bodies remain bounded and redirects remain disabled.
14. No new CI/release/evidence infrastructure is introduced.

## Lightweight validation strategy

### Deterministic local tests

- optional config deserialization and default compatibility;
- EggPool entry validation, duplicate/singleton replacement, list/remove behavior, and credential redaction;
- endpoint parsing for DNS, IPv4, IPv6, default/explicit port, HTTP/HTTPS, and invalid input;
- exact period token mapping and clamped movement;
- synthetic HTTP responses for public, authenticated, 401, 404, malformed, oversized, timeout, and success paths;
- summary semantic validation including null cache ratio, no streamed requests, nonfinite/negative numbers, and returned-period mismatch;
- pane availability and initial-pane selection;
- context-sensitive input/reducer behavior;
- systems Normal/Condensed regression behavior under the replacement layout key;
- Ratatui wide/narrow/empty/error/stale buffer tests.

### Existing repository checks

During phases, use the smallest relevant `cargo test -p gregg` filters plus formatting and crate-level clippy. Final closure runs the repository's existing local check and ordinary CI.

A live EggPool smoke may be performed manually against a LAN instance, but it is not a completion requirement and must not produce a checked-in evidence file.

## Risks and controls

### Risk: credential leakage

Control: persist only `api_key_env`; never serialize the resolved value; do not include request headers or response bodies in errors; assert list/JSON output and debug formatting do not contain a synthetic secret.

### Risk: h/l remapping regresses Normal/Condensed

Control: split `Pane` from `SystemViewMode`, assign one simple replacement key (`v`) to system layout, and retain reducer/buffer tests for both system layouts.

### Risk: analytics queries run too frequently

Control: fixed 60-second active-pane cadence, immediate requests only on first entry/period change/manual refresh, and no worker when unconfigured.

### Risk: summary values are mislabeled

Control: pin field-to-label semantics in typed tests and documentation; render null/no-sample values as unavailable.

### Risk: singleton design silently blocks future needs

Control: state the one-instance contract in CLI help and config docs. Do not aggregate incompatible metrics. A future multi-instance feature requires a separate navigation/API-semantics plan.

### Risk: EggPool routes are disabled

Control: classify 404 as `StatsUnavailable` with actionable text. Do not modify EggPool or scrape its database/dashboard in this roadmap.

### Risk: runtime integration becomes a generalized scheduler

Control: one endpoint-specific worker, one command/result channel, direct enums, no traits or registries.

## Program acceptance criteria

This roadmap is complete only when:

- [ ] Plans 57 through 60 meet their individual acceptance criteria.
- [ ] Existing configurations without `[eggpool]` load unchanged.
- [ ] `gregg eggpool add/list/remove` persist one optional endpoint through the existing atomic store.
- [ ] No secret value is persisted or printed.
- [ ] Public and API-key-protected EggPool summary endpoints work.
- [ ] The client sends exactly `1h`, `24h`, `7d`, and `30d` for the four periods.
- [ ] The pane accurately renders accounted tokens, cache-read share, output tok/s, and average TTFT.
- [ ] Null cache data and absent streamed TTFT samples render `—`.
- [ ] `h`/Left and `l`/Right cycle only available top-level panes.
- [ ] `j`/Down and `k`/Up retain system selection on Systems and change the period on EggPool.
- [ ] Normal and Condensed system layouts remain available through the documented replacement key.
- [ ] No EggPool configuration means no pane and no EggPool network work.
- [ ] EggPool-only configuration opens directly to the EggPool pane.
- [ ] Authentication, 404, network, decoding, body-limit, and semantic failures render nonfatal bounded states.
- [ ] Refresh behavior is immediate where required and no more frequent than once per 60 seconds during passive viewing.
- [ ] Existing greggd polling, system ordering, viewport, drive expansion, and rendering remain correct.
- [ ] Existing local checks and ordinary cross-platform CI pass without new infrastructure.
- [ ] Active documentation accurately states configuration, controls, metric semantics, auth, and EggPool route requirements.

## Handoff rules

1. Implement only the active phase.
2. Keep all product code inside `crates/gregg` unless an active documentation file must change.
3. Do not touch `greggd`, `gregg-protocol`, or EggPool.
4. Do not generalize the existing endpoint model; add one narrow EggPool model.
5. Store only an API-key environment-variable name.
6. Keep one EggPool endpoint and one summary route.
7. Use direct enums and matches rather than registries, trait objects, or plugin abstractions.
8. Preserve existing system-layout behavior while moving it to one replacement key.
9. Do not poll EggPool on the greggd refresh cadence.
10. Do not add workflows, services, screenshots, evidence documents, or release machinery.
11. Treat multi-EggPool, drill-down, charts, alerts, and headless EggPool stats-route changes as separate future work.
