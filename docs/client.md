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
gregg uninstall --dry-run                 # preview removal (mutates nothing)
gregg uninstall                           # remove only this client binary (config preserved)
gregg uninstall --purge                   # also remove the client config file (destructive)
```

`gregg uninstall` always applies to the component whose binary is executing:
it deletes only the exact invoked `gregg` executable (never a directory, so
a sibling `greggd` sharing the folder survives) and preserves configuration
by default. `--purge` additionally removes the resolved client config file
above (only the exact file; a custom `--config` parent is never removed, and
the standard Gregg parent only when empty). `--dry-run` prints the exact
resources without stopping, mutating, or deleting anything. There is no
interactive prompt and no `sudo` is invoked internally. A directly
`cargo install`ed client keeps Cargo bookkeeping: on Unix the uninstall
delegates to `cargo uninstall --root <root> gregg`, while on Windows it
prints the exact `cargo uninstall` command to run after the process exits.

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
as the daemon becomes reachable again. Each accepted failure stores a stable
provenance category (`timeout`, `dns`, `refused`, `network`, `http`,
`too-large`, `invalid`, `unsupported`) shown in the offline row
(`offline (refused)`); recovery clears it in the same poll generation.

The client stores its config at:

- Linux: `~/.config/gregg/gregg.toml` (honors `XDG_CONFIG_HOME`)
- macOS: `~/Library/Application Support/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

Client `request_timeout_ms` values must be between 100 and 60,000
milliseconds; invalid values are rejected before polling starts.

### Optional live telemetry

The client accepts additive v2 CPU-frequency, disk-I/O, and network fields.
It remains compatible with v1-only and pre-feature v2 daemons: those daemons
stay online and simply provide no newer telemetry. CPU frequency means the
current OS-reported frequency, not base or maximum frequency; macOS may omit
it because Gregg does not use privileged or undocumented mechanisms. Disk
capacity is independent of disk I/O, and `R/s`/`W/s` plus `Rx/s`/`Tx/s` are
byte-throughput rates. Network utilization uses the maximum valid directional
percentage against link capacity, not `Rx + Tx`; loopback can be shown in
`n` detail but does not contribute to aggregate capacity. Daemon version
transport remains deferred.

Only one EggPool endpoint is supported (`gregg eggpool add
pool.local:11300`); use `--replace` to change an existing one. Without it,
the command reports an already-configured endpoint conflict.

The pane has two independent read-only planes. `/api/stats/summary` supplies
the four periodized metrics for the selected window. EggPool's
schema-version-1 `/api/status` supplies current service health: the header
shows a plain `Health: ready`, `degraded`, `unready`, or `unknown` token, and
the footer may add a bounded provider count summary such as
`Providers: 2 ready · 1 degraded · 1 unavailable`. Health is never inferred
from the summary outcome, and a failed health read never hides valid metrics.
EggPool keeps `/api/status` authenticated even when its dashboard is public, so
a public EggPool with no configured `api_key_env` shows metrics with health
reported as auth required. An older EggPool without the route keeps the
metrics and reports health as unsupported.

## TUI navigation

- `j` / `k` (or arrow keys): move between systems
- `h` / `l`: cycle panes
- `v`: toggle normal/condensed layout
- `d`: expand/collapse drives for the selected system
- `n`: expand/collapse network details for the selected system
- `Ctrl-R`: ask the client daemon to re-read the config and poll immediately;
  on the EggPool pane, refresh that pane

`Ctrl-R` is still the only config-reload boundary, and there is still no
filesystem watcher. The difference is *where* the reload happens: the client
daemon re-reads the file, reconciles, and republishes, and the TUI renders
whatever arrives. If the file is missing, malformed, or invalid, the
last-known-good fleet stays active and the diagnostic line shows the rejection
until a later reload succeeds.

### The client daemon

`gregg` is a frontend. All remote polling for a configuration happens in a
separate, same-user process — the *client daemon* — which the TUI attaches to
over a local endpoint:

```text
gregg daemon run     # run the client daemon in the foreground
gregg daemon status   # is one running for this config?
gregg daemon stop     # stop it
gregg                  # the TUI; attaches to the running daemon
```

Each configuration gets its own daemon, identified by a digest of the
normalized config path, reachable on a `0600` Unix socket beside that config
(a Windows named pipe with an owner-only DACL where relevant). Two different
configurations never share a daemon, and one daemon never serves another
config's fleet.

This exists so that opening a second window costs a socket instead of a second
copy of the fleet's polling, and so that closing the last window does not stop
observation. It is also why the TUI **fails** rather than falling back to
polling directly: a silent fallback would double the request budget exactly
when the daemon is unhealthy, and would hide that from you. If you see
"no client daemon is listening for this config", start one with
`gregg daemon run`.

You do not normally start it by hand. Bare `gregg` probes the endpoint, and
only if it is *genuinely* empty does it take a per-config launch lock, re-probe
under that lock, and spawn one daemon with the exact executable you just ran. Two
`gregg` commands started at the same moment still produce one daemon. If
something else is on the endpoint — a refusal, a silent peer, a malformed frame
— Gregg reports it and starts nothing, because spawning over a peer you cannot
identify is how you end up with two daemons and no idea which one you are
looking at.

If the daemon is an older `gregg` than you are running, `gregg` stops it and
relaunches it on the current binary. If it is *newer*, `gregg` tells you to
upgrade instead: it may be serving a newer window elsewhere, and killing it
would downgrade that session because this one is old.

`gregg add`, `gregg remove`, and `gregg refresh` nudge a running daemon to
reload, best-effort and silently — "no daemon is running" is the common case and
never turns a successful mutation into an error.

The daemon also keeps polling with no TUI attached at all. That is deliberate:
continuous background observation is the point of the design, and a TUI is a
viewer. There is no idle timeout.

### Keeping it running

```text
gregg daemon startup install
gregg daemon startup instructions
gregg daemon startup status
gregg daemon startup remove
```

| Platform | What `install` writes |
|----------|------------------------|
| Linux | A `systemctl --user` unit. Without user systemd, a managed **user** crontab `@reboot` watchdog |
| macOS | A `~/Library/LaunchAgents` agent — never `/Library/LaunchDaemons` |
| Windows | A current-user Startup-folder entry — never a `LocalService` SCM registration |

Every artifact is named for the config's identity, so two configs never collide,
and Gregg keeps no global registry of active configs. Ownership is proven by
parsing the entry back: a unit, plist, or Startup entry that names a different
executable or config is left alone, and so is one that cannot be read at all.
`uninstall` and `startup remove` use the same check, so they cannot disagree
about what is yours.

A root or Administrator install registers **nothing** — there is no honest way
to pick which human a shared binary should watch for. Each user gets lazy
activation on their first `gregg` and can run their own `daemon startup
install`.

### Updating and uninstalling

`gregg update` identifies whether a daemon is running, prepares and verifies the
replacement, and only then stops the daemon, replaces the executable, and
relaunches. If the relaunch fails it says so with the exact command, because a
replaced binary with no daemon is a state you want to know about. Other configs'
daemons sharing the replaced executable are left alone; they reconcile on their
own next attach through the same version handshake.

`gregg uninstall` plans first and shows everything in `--dry-run`, including the
client daemon's startup entry and whether a daemon is running. Execution stops
an identified daemon, removes only an owned startup entry, and preserves
foreign and unparseable ones. If it cannot confidently stop a running daemon it
refuses to delete the executable and tells you what to run.

Installing as the current unprivileged user also registers the default-config
startup entry, and a failure to do so is a warning plus the exact command, never
a failed install: bare `gregg` works either way.

The selected system keeps its logical selection (`d` still toggles its drive
details and `n` toggles network details independently), but the reverse-video highlight is transient — it appears when you
navigate, and fades after roughly ten seconds of inactivity so stale
reverse-video does not survive a quiet screen. Leaving the Systems pane or
returning to it does not extend or re-trigger the highlight.

See [display](display.md) for what each view renders.
