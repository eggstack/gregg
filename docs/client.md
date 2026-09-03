# Client (`gregg`)

The client polls configured daemons and renders a live TUI. Install it on
your workstation (see [installation](installation.md)).

## Endpoints

```bash
gregg add 192.168.1.10:11310              # add an endpoint (explicit port required)
gregg add server.local:11310              # add with custom port
gregg add deadpool@server.local:11310     # nickname@host:port
gregg add http://server.local:11310/      # HTTP URL input; only host and port are persisted
gregg add 192.168.1.10:11310 --name deadpool  # explicit `--name` instead of `@`
gregg list                                # list configured endpoints
gregg remove 192.168.1.10                 # host-only remove is still supported
gregg refresh 30                          # set polling interval (seconds)
gregg edit                                # open config in $EDITOR
gregg version                             # print the client version
gregg update                              # binary-first update to latest stable crates.io version
```

`gregg add` requires an explicit port. Accepted: `host:port`, `[ipv6]:port`,
`http://host:port/`, and `nickname@host:port`. Rejected: host-only (`host`,
`192.168.182.146`, `::1`), HTTP URL without a port, `nickname@host` without
a port, `nickname@`, and the ambiguous combination of inline `nickname@`
with `--name`. HTTPS is never accepted and is not downgraded to HTTP.
`gregg remove` still accepts host-only input. Persisted fields are normalized
`host` and `port`; the inline `nickname@` form populates the existing
`SystemEntry.name` field. `default_port` remains in the configuration schema
for compatibility but is not used by `gregg add`. Do not rely on implicit
ports.

IPv6 link-local zone identifiers are accepted in either `fe80::1%eth0` or
`[fe80::1%25eth0]:11310` form and are stored in URL-safe `%25` form for
polling. Bracketed endpoint syntax is reserved for IPv6 literals; values such
as `[server.local]:11310` are rejected.

Offline endpoints continue to be polled on every configured cadence (no
backoff); they automatically recover and switch to the normal view as soon
as the daemon becomes reachable again.

The client stores its config at:

- Linux: `~/.config/gregg/gregg.toml` (honors `XDG_CONFIG_HOME`)
- macOS: `~/Library/Application Support/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

Client `request_timeout_ms` values must be between 100 and 60,000
milliseconds; invalid values are rejected before polling starts.

Only one EggPool endpoint is supported (`gregg eggpool add
pool.local:11300`); use `--replace` to change an existing one. Without it,
the command reports an already-configured endpoint conflict.

## TUI navigation

- `j` / `k` (or arrow keys): move between systems
- `h` / `l`: cycle panes
- `v`: toggle normal/condensed layout
- `e`: expand/collapse drives for the selected system
- `Ctrl-R`: reload the current Systems config, reliably deliver its endpoint
  replacement, and poll it immediately; on EggPool, refresh that pane

If a Systems config reload is missing, malformed, or invalid, the
last-known-good configuration remains active and the error is shown in the
diagnostic line until a later reload succeeds. When the bounded scheduler
command channel is full, replacement delivery waits in the event loop's
pending-command branch so input and poll results remain responsive.

The selected system keeps its logical selection (`e` still toggles its drive
details), but the reverse-video highlight is transient — it appears when you
navigate, and fades after roughly ten seconds of inactivity so stale
reverse-video does not survive a quiet screen. Leaving the Systems pane or
returning to it does not extend or re-trigger the highlight.

See [display](display.md) for what each view renders.
