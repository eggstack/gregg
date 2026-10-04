# Client (`gregg`)

A per-user client daemon polls your configured daemons; the TUI renders what it
serves, so a second window costs a socket rather than a second copy of the
fleet's polling. Install it on your workstation (see
[installation](installation.md)).

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

## Cron observability

The client daemon owns a second, deliberately slower polling plane for the
scheduler routes a `greggd` may serve:

- `GET /v2/scheduler` is read on a **30-second cadence** for every configured
  system. It carries only scalar per-job state and no command output, so it is
  cheap and is what the at-a-glance job rows need.
- `GET /v2/scheduler/history` is fetched **only on first support discovery and
  when the summary's `history_revision` changes**. It is the largest document in
  the system, and downloading it on every metrics poll would put it on the hot
  path to draw five job rows.

The gate compares the epoch as well as the revision. A restarted `greggd` resets
`history_revision` and can reset it to the same small value it used before, so a
revision-only check would conclude "nothing changed" and never fetch the new
epoch at all. Deduplication identity is therefore `(epoch, sequence)`, never
`sequence` alone — a remote legitimately reissues sequences from zero, and a
sequence-only identity would drop the new epoch's records as duplicates.

### What the daemon keeps, and why it is bounded

The remote ring is the authority. The client daemon additionally keeps a
**memory-only** cache that is deeper than the remote retains, because it is
always running and observes successive rings. It is bounded three ways:

1. a per-job depth (`[cron] cache_history`, hard maximum 50),
2. a **global record ceiling** across every system, job, and epoch, and
3. the remote contract's own per-stream output cap, which bounds a single
   record's size and therefore makes a record *count* a real memory bound.

Bound 2 is the one that matters at scale: per-job depth alone does not survive
`endpoints × jobs × depth`. Eviction is oldest-first across the whole fleet and
tie-broken by `(system, job)`, so identical inputs evict the same record and the
cache's contents are reproducible. Nothing is written to disk; a daemon restart
reseeds from whatever the remote still holds.

A job the remote no longer serves, and a system that left the fleet, are
dropped. A record that was genuinely observed is not retroactively erased by a
remote restart, but neither does the bound grow forever.

### Capabilities, not errors

Scheduler outcomes are three-valued, and the distinction is the point:

| Outcome | Meaning |
|---|---|
| Supported | The routes exist and a valid summary arrived. |
| Unsupported | A 404: an older `greggd`. Expected, not a failure. |
| Failed | Transport, 5xx, an oversized body, or a document that failed its own wire validation. |

A pre-scheduler `greggd` is a *healthy* daemon, so it is never reported as
errored — doing so would put a permanent badge on every old system in a mixed
fleet. Nothing in the cron plane can reach system reachability: a cron route
failure on an otherwise healthy system must not say the system is down. A
transient failure retains the last known data and is labelled stale.

A remote that serves the summary but not its history is a real inconsistency
and is reported as such, while the job list that did arrive is still shown.

### What crosses the local channel

Only the **existing** state document carries cron state; there is no second
socket and no separate request path.

The summary needs no request to display, so it always rides along. The history
records are the largest thing in the document, so they are included only for
systems a frontend currently has open. A frontend publishes a cron intent —
system, job, and requested depth — and the daemon reduces every frontend's
intent into a `(system, job) -> depth` set, taking the maximum depth per pair.
The reduction is a union and is order-independent, so a window asking for fewer
records is never shrunk by a neighbour that wants more, and a departed window's
intent is retired on disconnect so the daemon stops transmitting records nobody
is reading.

**The intent governs transmission and never fetching.** The daemon polls whether
or not a TUI is attached, which is why ten windows with the pane open cost the
fleet exactly what zero windows cost. An intent change alters the document
without touching the cache, and is therefore its own publication reason.
