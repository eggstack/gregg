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

- Reuses reqwest stack, disables redirects
- Sends `GET /api/stats/summary?period=...`
- 16 KiB body cap
- Bearer token from environment variable (never stored in outcomes)
- Fixed periods: `1h`, `24h`, `7d`, `30d`

## Worker

- Background task with command channel
- 60-second passive refresh when active
- Generation-based staleness like greggd polling
- Created only for configured EggPool state
- Activated when pane is visible, deactivated when hidden
- Cancelled during TUI shutdown

## TUI navigation

- `h`/`l` (and arrow keys): cycle between Systems and EggPool panes
- `j`/`Down`: select next window (`1h`, `24h`, `7d`, `30d`)
- `k`/`Up`: select previous window
- `Ctrl-R`: refresh only the visible pane; EggPool refreshes are active-only

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
- `mixed_fleet_evidence.rs` — integration test with fixture servers
- `sustained_workload.rs` — `#[ignore]`, exercises full polling loop
- Worker regression tests in `src/sustained_workload.rs` for generation, cadence, pressure, deactivation, cancellation
