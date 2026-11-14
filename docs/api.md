# HTTP API

`greggd` serves cached JSON documents on its configured port, `11310` by
default.

| Method | Route | Description |
| --- | --- | --- |
| `GET`, `HEAD` | `/` | Service root |
| `GET`, `HEAD` | `/v1/status` | Legacy status schema on Linux and macOS |
| `GET`, `HEAD` | `/v2/status` | Current status schema on all supported platforms |
| `GET`, `HEAD` | `/healthz` | Legacy health |
| `GET`, `HEAD` | `/v2/healthz` | Current health |
| `GET`, `HEAD` | `/v2/scheduler` | Optional scheduler summary |
| `GET`, `HEAD` | `/v2/scheduler/history` | Optional retained job history |

The Gregg client requests `/v2/status` first and falls back to `/v1/status` only
when the v2 route returns 404. `/v2/status` is the cross-platform status route.
Scheduler routes are available on newer daemons. Older daemons return 404 for
them; this does not affect system reachability.

Known routes accept only `GET` and `HEAD`. Other methods return `405` with an
`Allow: GET,HEAD` header. Unknown routes return `404`. Request bodies are
bounded at 64 KiB; larger bodies are rejected.

The daemon serves cached snapshots and evaluates freshness on each request. If
the system clock moves backward and a snapshot timestamp is now in the future,
the daemon treats that snapshot as stale while age-based staleness is enabled.
It becomes eligible as fresh again when the clock catches up.

## Optional v2 telemetry

The v2 status schema may include:

- `cpu_frequency_hz`: current OS-reported CPU frequency in hertz, not base or
  maximum frequency.
- `disk_io`: daemon-selected aggregate and device read/write byte rates.
- `network`: aggregate and interface receive/transmit byte rates, with optional
  link capacities in bits per second.

These fields are optional. Older daemons omit them, and clients treat missing
data as unavailable. Rates use cumulative native counters and monotonic elapsed
time. A reset, restart, or device change establishes a new baseline before a
rate is reported. Missing capacity leaves network throughput available but
utilization unavailable. Network utilization uses the larger valid receive or
transmit percentage, not their sum. Loopback may appear in interface details,
but does not contribute to aggregate capacity.

Wire schemas, capabilities, and validation rules are documented in
[Protocol architecture](../architecture/protocol.md). For scheduler job
configuration and output security, see [Daemon](daemon.md#scheduled-maintenance).
