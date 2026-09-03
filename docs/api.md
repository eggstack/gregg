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
