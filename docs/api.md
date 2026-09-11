# HTTP API

The daemon serves cached immutable snapshots on its configured port
(default `11310`):

```text
GET /           # root
GET /v1/status  # v1 status (Linux/macOS only; Windows returns 503)
GET /v2/status  # v2 status (all platforms)
GET /healthz    # v1 health
GET /v2/healthz # v2 health
```

Clients request `/v2/status` first and fall back to `/v1/status` only on
404. `/v2/status` is the universal cross-platform endpoint.

If the system clock moves backward, a snapshot timestamp that is temporarily
in the future is treated as fresh rather than stale; age-based staleness
resumes once the clock catches up.

Wire-format details (schema versions, capabilities, validation) live in
`architecture/protocol.md`.

Schema-v2 status may add optional `cpu_frequency_hz` (current host frequency
in Hz), `disk_io` (daemon-selected aggregate and bounded device byte rates),
and `network` (directional aggregate/interface byte rates and optional link
capacities in bits per second). Older daemons omit these fields and clients
normalize them as unavailable. Network utilization uses the larger valid
receive/transmit directional percentage, so simultaneous full-duplex traffic
does not double-count; missing capacity leaves throughput available but no
utilization percentage.

The daemon derives live rates from cumulative native counters using monotonic
elapsed time. The first observation and any reset or hotplug observation warms
the corresponding identity. The aggregate disk set is independent of mounted
filesystem rows and excludes obvious parent/child duplication; network detail
may include loopback, but loopback never contributes aggregate capacity.
