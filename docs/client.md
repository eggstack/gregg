# Client (`gregg`)

`gregg` is the terminal interface. A per-user client daemon owns remote polling
for each configuration, so multiple TUI windows share one polling cycle. The
client daemon starts automatically when you run `gregg` and keeps polling after
the last window closes.

Install the client on your workstation. See [Installation](installation.md).
Each monitored host must run a reachable `greggd` instance.

## Add monitored hosts

```bash
gregg add 192.168.1.10:11310
gregg add deadpool@server.local:11310
gregg add server.local:11310 --name deadpool
gregg list
gregg refresh 30
gregg
```

`gregg add` requires an explicit port. It accepts `host:port`, `[ipv6]:port`,
`nickname@host:port`, or `http://host:port/`. HTTPS, host-only input, and a
nickname supplied both inline and with `--name` are rejected. The optional
`default_port` configuration setting is retained for compatibility but is not
used by `gregg add`.

Bracketed endpoint syntax is reserved for IPv6 literals. IPv6 link-local zone
identifiers are supported. `gregg remove` accepts a host without a port,
including a bare IPv6 literal, and removes the matching configured host.

Other useful commands:

```text
gregg remove HOST       Remove a configured host
gregg edit              Open the client configuration
gregg version           Print the client version
gregg update            Update this executable
gregg uninstall --dry-run  Preview removal
```

Offline hosts are polled at the configured interval and recover automatically.
The offline row may include the failure category, such as `refused` or an HTTP
status. A failed scheduler or EggPool request does not change system
reachability.

## Configuration

The client configuration is stored at:

- Linux: `~/.config/gregg/gregg.toml`, honoring `XDG_CONFIG_HOME`
- macOS: `~/Library/Application Support/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

`request_timeout_ms` must be between 100 and 60,000. Use `--config PATH` to
select another file where supported by the command.

In the Systems pane, `Ctrl-R` is the config reload boundary. The client daemon
re-reads the file and polls immediately. If the file is missing or invalid, it
keeps the last valid configuration and displays a diagnostic. In the EggPool
pane, `Ctrl-R` refreshes that pane. Commands that change the host list or
polling interval notify a running client daemon best-effort.

## Client daemon

The TUI connects to a local client daemon. One daemon serves one configuration;
different configuration paths have separate daemons. If a compatible daemon
is not running, `gregg` starts one automatically when it can confirm the local
endpoint is unused. If another or unrecognized process occupies that endpoint,
`gregg` reports the problem instead of starting a second poller. The TUI does
not poll remote hosts directly as a fallback.

Manage the daemon directly when needed:

```text
gregg daemon run       Run in the foreground
gregg daemon status    Check the daemon for this config
gregg daemon stop      Stop it
gregg daemon restart   Restart it and check readiness
```

To keep the client daemon running when no TUI is open, register a user startup
entry:

```text
gregg daemon startup install
gregg daemon startup status
gregg daemon startup instructions
gregg daemon startup remove
```

The entry is per-user. Linux uses user systemd when available and otherwise a
managed user crontab; macOS uses a LaunchAgent; Windows uses the current user's
Startup folder. A root or Administrator install does not register startup for
another user. Startup registration is optional because running `gregg` starts
the daemon on demand.

`gregg update` updates the exact executable you invoked. It prepares and
verifies the replacement before stopping a running client daemon, replaces the
binary, and relaunches it. If relaunch fails, it reports that the binary was
updated and gives a retry command.

`gregg uninstall --dry-run` previews the executable, daemon, and startup entry
that would be removed. Uninstall preserves configuration by default. Add
`--purge` to remove the client configuration as well. It removes only the
invoked client executable and does not remove a sibling `greggd` binary.

## EggPool

The optional EggPool pane uses one configured endpoint:

```text
gregg eggpool add pool.local:11300
gregg eggpool add pool.local:11300 --api-key-env EGGPOOL_API_KEY
```

Use `--https` for TLS and `--replace` to change an existing endpoint. The
configuration stores the environment variable name, not the API key. Omit the
`[eggpool]` configuration to disable the pane and its network activity.

The pane reads periodized metrics from `/api/stats/summary` and service health
separately from EggPool's schema-version-1 `/api/status`. Statistics include
accounted tokens, provider cache-read share, output tokens per second, and
average TTFT for `1h`, `24h`, `7d`, or `30d`. They require EggPool's dashboard
routes. The health route remains authenticated even when the dashboard is
public, so health may report authentication required while metrics remain
available. Older EggPool versions without the health route still provide
metrics when supported. `Ctrl-R` refreshes the active pane.

## Scheduled-job view

Press `c` to open scheduled-job details for the selected host. Use
`Shift-J` and `Shift-K` to select a job. The summary updates every 30 seconds;
run history is fetched when it changes. The client daemon keeps a bounded,
memory-only history cache, which is lost when that daemon restarts.

A daemon that does not support the scheduler routes is shown as unsupported,
not offline. Scheduler read failures keep the last data and mark it stale.
Schedules use the remote host's local time. Run timestamps are UTC because the
remote timezone is not reported. Remote command output is escaped before it is
displayed. Anyone who can reach a daemon listener can read its scheduler
history, so keep the listener on a trusted network. See [Daemon](daemon.md#scheduled-maintenance)
and [Display](display.md) for details.

## Optional live telemetry

The client displays CPU frequency, disk throughput, and network throughput when
the daemon and host provide them. These are best-effort measurements. CPU
frequency is the current OS-reported value, not the base or maximum frequency.
Disk and network rates are byte rates. Missing values remain unavailable
instead of being presented as zero. See [Display](display.md).
