---
name: eggpool
description: Work with the optional EggPool summary pane in the gregg client
---

## What I do

Guide agents through the EggPool summary pane implementation in the gregg client.

## When to use me

Use this when modifying EggPool configuration, client, worker, rendering, or testing.

## Overview

EggPool is an optional compact summary pane showing one configured EggPool's:
- Accounted tokens
- Provider cache-read share
- Output-token throughput
- Average time to first token

It is client-only: Gregg supports one source and does not change EggPool or `greggd`.

## Key modules

| Module | File | Purpose |
|--------|------|---------|
| `eggpool` | `src/eggpool.rs` | EggPool summary client and background worker |
| `eggpool_endpoint` | `src/eggpool_endpoint.rs` | EggPool-specific endpoint parsing |
| `ui/eggpool` | `src/ui/eggpool.rs` | EggPool summary pane rendering |

## Configuration

```toml
[eggpool]
scheme = "http"
host = "localhost"
port = 11300
api_key_env = "EGGPOOL_API_KEY"
```

- Defaults to HTTP port `11300`; `--https` selects HTTPS
- Stores environment-variable name, never the resolved secret
- `add/list/remove` commands perform no network or environment lookup

## CLI commands

```text
gregg eggpool add <host> --name "Main EggPool" --api-key-env EGGPOOL_GREGG_API_KEY
gregg eggpool list              # use --json for a JSON array
gregg eggpool remove <host>
```

## Client

- Dedicated `eggfetch-core` client (lean `standard-http1` 0.2 + Rustls: redirect following not compiled, two idle per host, explicit whole-request deadline with absolute `total` through response-body EOF, no retry)
- Two independent read-only planes:
  - summary: `GET /api/stats/summary?period=...` with a per-request 16 KiB cap
  - health: `GET /api/status` (Plan 152) with a per-request 1 MiB cap, the same
    scheme/host/port normalization, no query, and the same 401/403/404
    classification split (`AuthenticationRequired`/`Forbidden`/`Unsupported`)
- The client-wide default cap stays at 16 KiB, so the per-route status cap
  never widens the summary route
- Bearer token from environment variable via request-local `AuthScheme::bearer`
  (invalid values map to `InvalidSummary`/`InvalidApiKey`; never stored in
  outcomes). An absent key stops the summary request but still sends the health
  request, because EggPool keeps `/api/status` authenticated even when its
  dashboard is public
- Fixed periods: `1h`, `24h`, `7d`, `30d`

## Health plane (`/api/status`, Plan 152)

- `EggpoolProxyHealth` = `Ready`/`Degraded`/`Unready`;
  `EggpoolProviderHealth` = `Ready`/`Degraded`/`Unavailable`/`Disabled`/`Unknown`;
  `EggpoolProviderObservation` = `Verified`/`Failed`/`Stale`/`Never`
- These are `EggPool`-reported facts. A transport failure is never rendered as
  a proxy-reported unavailable state, and health is never inferred from the
  local worker state or the summary outcome
- `schema_version` must be `1`; a future version is an explicit unsupported
  health state, not a decode crash, and never invalidates the summary plane
- Bounded before rendering: ≤256 provider rows, ≤96-byte provider IDs,
  ≤64-byte reason codes, finite non-negative uptime. These are EggPool's own
  producer bounds (`MAX_STATUS_PROVIDERS`, `MAX_PROVIDER_ID_CHARS`,
  `MAX_REASON_CODE_CHARS`), not conservative substitutes
- `AppState` holds health separately from summary state; a failed health read
  keeps the previous snapshot but marks it stale, and a period change applies to
  the summary payload only
- The worker reads both planes concurrently in its single request task and
  delivers both outcomes, so partial success is preserved
- Never add a provider/account/model drill-down, a second cadence, a second
  credential field, or a health-triggered provider probe

## Schema-v1 wire contract (Plan 153)

Gregg decodes EggPool's serialized
`rust/src/operations/status.rs::ProxyStatusSnapshot`. The canonical field
names and placement are:

```text
root.schema_version            root.providers[].provider_id
root.proxy.status              root.providers[].status
root.proxy.available           root.providers[].last_observation
root.proxy.uptime_seconds
root.proxy.model_count
root.proxy.routable_accounts
root.proxy.enabled_accounts
root.proxy.reason_code
```

Provenance of that shape: `eggstack/eggpool` commit
`43c987ea458bd563d5108fd8051ad31185704bb0`, serde JSON from
`ProxyStatusSnapshot` and its `ProxyHealthSummary`, `ProviderHealthSummary`,
and `RuntimeHealthSummary` nested types. The Gregg fixture is structurally
canonical with synthetic values; it is not a byte-for-byte live response.

Fields EggPool emits that Gregg deliberately does not model, and must keep
ignoring: `observed_at`; `runtime.{generation,digest_prefix,reload,tasks,db,retiring}`;
`proxy.ready`, `proxy.version`, `proxy.base_url`; and provider detail
`enabled_accounts`, `total_accounts`, `routable_accounts`, `backoff_accounts`,
`unavailable_accounts`, `model_count`, `last_probe_age_seconds`,
`last_probe_latency_ms`, `last_probe_status_code`, and `reason_code`. Unknown
extra JSON fields stay tolerated.

- `canonical_status_body` in `src/eggpool.rs::tests` is the single passing
  status fixture and records the upstream provenance in its doc comment. Its
  ignored runtime/provider details use the exact schema-v1 field names listed
  above. Use it for new status cases instead of hand-rolling a payload.
- Never add a serde alias or fallback for the never-upstream Plan-152 shape
  (`id` instead of `provider_id`, `observation` instead of
  `last_observation`, root-level account counts). `gregg_local_status_shape_is_not_a_supported_schema`
  locks that out.
- EggPool's CLI-only proxy `unavailable` state is not a server-reported value:
  an unknown proxy status stays `InvalidStatus`.

## Worker

- Background task holding one `EggpoolDesiredState` watch receiver plus a
  bounded result channel
- 60-second passive refresh when active
- Generation-based staleness like greggd polling
- Created only for configured EggPool state
- Activated when pane is visible, deactivated when hidden
- Cancelled during TUI shutdown; no queued shutdown command exists
- Plan 151 contract: the reducer owns one latest desired state
  (`active`/`period`/`generation`) and publishes it synchronously through
  `EggpoolControl::publish`. Publication is capacity-free, so the input path
  never waits on the worker and no activation, period change, manual refresh,
  or deactivation is ever dropped. There is no `Busy` state.
- The worker converges on the newest desired state, coalescing states it did
  not observe individually, aborting obsolete in-flight work, arming the
  request-relative passive deadline only after completion, and reusing the
  reducer generation for passive refreshes
- A closed control channel is the only failure and surfaces
  `EggpoolWorkerState::WorkerUnavailable`

## Local worker state vs EggPool health

- `EggpoolWorkerState` is `Idle` / `Refreshing` / `WorkerUnavailable` and
  describes only Gregg machinery
- `EggpoolFetchOutcome` values are summary-transport facts and are never
  folded into worker lifecycle
- EggPool's own proxy/provider service health is a separate model sourced from
  `/api/status` (Plan 152) and must not be inferred from worker state

## TUI navigation

- `h`/`l` (and arrow keys): cycle between Systems and EggPool panes
- `j`/`Down`: select next window (`1h`, `24h`, `7d`, `30d`)
- `k`/`Up`: select previous window
- `Ctrl-R`: on Systems, the client reloads its resolved system config and
  refreshes endpoints; on EggPool, refresh remains active-only and pane-local

## Rendering

- Compact pending/success/stale/error states
- Shows exactly four summary values
- Header: `EggPool — <identity>    Health: <ready|degraded|unready|unknown>    Window: <period>`;
  the health token is dropped before identity/window are truncated
- Footer priority: worker unavailable/refreshing, summary refresh failure, then
  a bounded `Providers: N ready · M degraded ...` count
- Keeps same-period prior result visible when a later refresh fails, and marks
  a retained health snapshot stale after a failed health refresh
- Pane, period, and drive-expansion state are transient

## Key constraints

- One optional endpoint only; no aggregation, multiple instances, or drill-down
- No runtime diagnostics, charts, history, alerts, or exports
- No configurable cadence; fixed 60-second passive deadline
- Authentication is request-local; never stored in outcomes
- The worker is wired from the *current* config, so `Ctrl-R` can add or remove
  the entry; adding one must bring up a live worker, never a pane stuck in
  `Refreshing`
- The converged memo records the generation the worker was actually driven
  with, after the fleet bumps it — never the pre-bump one, which costs a
  wasted fetch per event
- Result delivery waits for a channel slot inside a `select!` biased toward
  cancellation, so a pane that stops reading can never park the worker
- A failed publication records the converged state anyway: one dead worker
  publishes once, not once per reduce tick

## Tests

- Unit tests in every module
- Compatibility matrix in `src/eggpool.rs`: ready/degraded/unready proxy,
  every provider state, public summary + 401 status, older EggPool 404,
  dashboard-disabled summary, independent malformed/oversized cases, unknown
  schema version, bounded-contract rejection, and no secret in outcomes
- Schema-v1 wire qualifications in `src/eggpool.rs`: an upstream-provenance
  `ProxyStatusSnapshot` payload that decodes `Online` while ignoring
  unmodeled canonical fields, a negative regression proving the Plan-152
  Gregg-local shape is not a supported schema, and exact-boundary acceptance
  plus one-over rejection for 96-byte provider IDs, 64-byte reason codes, and
  256 provider rows
- Worker regression tests in `src/eggpool.rs`: generation retention across
  passive refresh, request-relative deadlines tied to activation triggers,
  nonblocking latest-desired-state publication under rapid input, convergence
  on the final period/generation, deactivation arming no passive refresh,
  in-flight cancellation, closed control channel, and panic-in-fetch recovery
- Use a loopback summary server that can hold responses open plus paused Tokio
  time for cadence; bound positive waits with a real-clock watchdog thread
  rather than a Tokio timer, because a paused-time timer lets virtual clock
  auto-advance race the loopback round trip
- Full polling-loop drivers (`mixed_fleet_evidence.rs`,
  `sustained_workload.rs`) exercise greggd systems polling; they are not
  EggPool-specific


## Corrective-plan status

- Plan 151 is complete: the drop-on-full `try_send` + `Busy` design from `d31d72f` is superseded by the retained latest-desired-state contract described above. `d31d72f` remains a truthful historical record.
- Plan 152 is complete on top of Plan 151 and adds EggPool schema-v1 `GET /api/status` as an independent health plane alongside the existing summary metrics.
- Plan 153 is complete: the private schema-v1 consumer matches EggPool's canonical consumed wire shape (`proxy.routable_accounts` / `proxy.enabled_accounts`, `providers[].provider_id`, `providers[].last_observation`) with producer-aligned 256/96/64 bounds. Plan 152's `id` / `observation` / root-count synthetic fixture is not a supported schema and no alias preserves it.
- Plan 154 is complete: the Plan-153 fixture's ignored runtime/provider examples now use EggPool's actual `RuntimeHealthSummary` / `ProviderHealthSummary` field names. Do not change production decoding or expand `EggpoolHealthSnapshot` merely to retain ignored evidence fields.

Do not alter the four summary metric semantics or broaden Gregg into an EggPool dashboard, and do not reintroduce `Busy` or a lossy control queue.
