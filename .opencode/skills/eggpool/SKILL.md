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
- Sends `GET /api/stats/summary?period=...`
- 16 KiB decoded-body cap (eggfetch owns the limit; fixed, chunked, and close-delimited over-cap bodies map to `BodyTooLarge`)
- Bearer token from environment variable via request-local `AuthScheme::bearer` (invalid values map to `InvalidSummary`; never stored in outcomes)
- Fixed periods: `1h`, `24h`, `7d`, `30d`

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
- Keeps same-period prior result visible when a later refresh fails
- Pane, period, and drive-expansion state are transient

## Key constraints

- One optional endpoint only; no aggregation, multiple instances, or drill-down
- No runtime diagnostics, charts, history, alerts, or exports
- No configurable cadence; fixed 60-second passive deadline
- Authentication is request-local; never stored in outcomes

## Tests

- Unit tests in every module
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
- Plan 152 follows Plan 151 and adds EggPool schema-v1 `GET /api/status` as an independent health plane alongside the existing summary metrics.

Do not alter the four summary metric semantics or broaden Gregg into an EggPool dashboard, and do not reintroduce `Busy` or a lossy control queue.
