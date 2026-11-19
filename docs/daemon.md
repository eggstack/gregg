# Daemon (`greggd`)

Install `greggd` on each machine you want to monitor. It collects local system
metrics and serves a read-only HTTP API on port `11310` by default. See
[Installation](installation.md) for installation options and
[HTTP API](api.md) for routes and telemetry.

## Configuration

Default configuration paths:

| Platform | Path |
| --- | --- |
| Linux | `/etc/gregg/greggd.toml` |
| macOS | `/Library/Application Support/gregg/greggd.toml` |
| Windows | `%ProgramData%\gregg\greggd.toml` |

A minimal configuration is:

```toml
name = "greggd"
host = "127.0.0.1"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
```

Override the file path with `greggd run --config PATH`. The default bind address
is loopback, so a fresh install answers only on this machine. Config changes
apply after the daemon is restarted.

### LAN monitoring

To collect from one `gregg` client across several machines, set `host` to a
reachable address on each daemon — `host = "0.0.0.0"` (or `::`) for every
interface, or a concrete interface address to narrow the bind:

```toml
host = "0.0.0.0"
```

This is an explicit opt-in, not a default. The API has no authentication and no
TLS, so anything that can route to the port can read fleet metrics and scheduled
job output; firewall it to the client hosts. A non-loopback bind logs one
startup warning naming the address. Use `greggd host 127.0.0.1` to return a
daemon to loopback, for example when access is through an SSH tunnel.

## Scheduled maintenance

Scheduled jobs are optional. They use five-field local cron schedules and run
commands as direct argv arrays, without an implicit shell. For example:

```toml
allow_privileged_jobs = false
scheduler_history_limit = 5

[[jobs]]
name = "refresh-index"
schedule = "15 */6 * * *"
command = ["/usr/local/bin/refresh-index"]
working_dir = "/home/user/projects"

[[jobs]]
name = "weekly-cleanup"
schedule = "0 3 * * 0"
command = ["/usr/local/bin/cleanup", "--deep"]
max_load = 8.0
load_window = "15m"
retry_interval_ms = 300000
max_wait_ms = 86400000
```

Schedules accept numeric fields with lists, ranges, wildcards, and steps. Weekday
values are `0` through `6`, with Sunday as `0`. The aliases `@hourly`, `@daily`,
`@weekly`, and `@monthly` are supported. When both day-of-month and day-of-week
are restricted, either may match. Impossible calendar schedules are rejected
before the daemon starts.

Schedules use the daemon host's local civil time. A time skipped by a
spring-forward clock change is skipped; both instances of a repeated fall-back
time run in order. Missed jobs are not replayed after a daemon restart. Wall
clock changes are reconciled within about a minute. Load retries and maximum
waits use monotonic time.

Commands are executed directly. To use shell syntax, include a shell explicitly
in the argv, such as `["/bin/sh", "-c", "command"]`. Jobs inherit the daemon's
OS identity. On Unix, a daemon running as root refuses configured jobs unless
`allow_privileged_jobs = true` is set. This option does not change the process
identity or service sandbox. The standard Linux service runs as the `greggd`
user and generally cannot access a developer's home directory. Windows supports
time-based schedules but not load gates.

The scheduler runs at most one command at a time. Load-gated jobs defer when
load is high or unavailable, retry every five minutes by default, and wait up
to 24 hours by default. Retry intervals range from 10 seconds to one hour;
maximum wait is at most seven days, and the retry interval cannot exceed it.
Load thresholds are inclusive. Repeated occurrences coalesce while a job is
pending. A nonzero command exit
is recorded but not retried until the next scheduled occurrence. Load windows
are `1m`, `5m`, or `15m`, with `15m` as the default. See the configuration
example above for optional load settings.

### Scheduler history and security

Scheduler state is available through two read-only routes:

```text
GET/HEAD /v2/scheduler
GET/HEAD /v2/scheduler/history
```

A daemon with no jobs returns a valid empty document. Older daemons that do not
provide these routes return 404; the Gregg client treats this as unsupported,
not as a host failure.

History is memory-only and is cleared when the daemon restarts. The
`scheduler_history_limit` setting retains up to 10 terminal records per job. It
defaults to `5`; set it to `0` to disable record retention while keeping live
job state. Retained records include bounded stdout and stderr tails. Output is
captured only when history retention is enabled. The direct command's exit is
the end of a run; output from background descendants may not be captured.

> **Scheduler output is not private.** The daemon listener has no authentication
> or TLS. Anyone who can reach it can read retained command output, though
> command arguments and working directories are not published. Keep the
> listener on a trusted network and do not put credentials in command arguments
> or output.

## Running and managing the daemon

```text
greggd run                 Run in the foreground
greggd status              Show version, bind address, health, and startup state
greggd configprint         Print the configured bind address
greggd croncheck           Start only if the health endpoint refuses connections
greggd stop                Stop the local daemon
greggd restart             Restart the daemon
greggd update              Update the installed executable
greggd uninstall --dry-run Preview removal
```

`greggd run` stays in the foreground. It does not daemonize itself. `status`,
`configprint`, and `croncheck` are bounded diagnostic commands. `croncheck` is
intended for cron or other supervisors without readiness checks: it starts the
daemon only when a connection is refused. A responding but invalid or
unrecognized service is not replaced.

Register automatic startup with:

```text
greggd startup install
greggd startup instructions
greggd startup status
```

On Linux, the default is systemd when it is running and cron otherwise. macOS
uses launchd. Systemd and launchd registration require elevated privileges and
the binary at `/usr/local/bin/greggd`; they do not fall back to cron on a
permission error. Cron registration can run without root and preserves
unrelated crontab entries. Use `greggd startup instructions` to see the
commands for your host. On Windows, the PowerShell installer registers the SCM
service when run as Administrator. `greggd startup install` on Windows reports
service state and does not create the registration. Gregg does not invoke
`sudo` itself.

## Service account

The daemon runs as a dedicated, non-login account, never as `root`/`SYSTEM`:

| Platform | Account | Configured by |
| --- | --- | --- |
| Linux | `greggd` system user | `systemd` unit `User=greggd` |
| macOS | hidden `_greggd` user and group (uid/gid 391) | plist `UserName`/`GroupName` |
| Windows | `NT AUTHORITY\LocalService` | the SCM registration |

The ids are fixed rather than generated, so a reinstall is idempotent and file
ownership survives an upgrade. If that id already belongs to another record, the
macOS install stops and names the conflict instead of adopting the account.

The configuration file stays owned by `root` (`0644`) on Linux and macOS: the
daemon can read its configuration but cannot rewrite its own job set. On macOS
the log `/var/log/greggd.log` is owned by `_greggd` at `0640`, because launchd
redirects the job's output into it.

Uninstall removes the startup registration and the binary and **keeps** the
service account, which is inert without the job. Because the macOS configuration
directory is root-owned, `_greggd` cannot bind the config-adjacent control
socket, so `greggd stop` falls back to the `0600` temp socket
`<temp dir>/greggd-<id>.control.sock`; `stop` tries both paths.

## Updating and uninstalling

`greggd update` updates the exact executable that was invoked. It verifies the
candidate before replacing the binary and restarts only a daemon that was
running under an owned startup registration or direct run. A stopped daemon
remains stopped. If activation fails after replacement, the command reports
that state and prints a retry command.

Use `greggd uninstall --dry-run` to preview the executable and startup
integration that would be removed. Uninstall preserves configuration by
default. Add `--purge` to remove the resolved daemon configuration and, on
macOS, the daemon log. Only startup entries that target the exact executable
are removed; foreign or ambiguous entries are preserved. If a running daemon
cannot be confidently stopped, uninstall refuses to delete its executable.

## Network and telemetry

The daemon's HTTP API is unauthenticated and has no TLS. Do not expose it to the
public Internet. Restrict access with your network firewall or bind to localhost
and use a secure tunnel. No firewall rules are created automatically.

Collectors use native operating-system interfaces. Optional CPU-frequency,
disk-I/O, and network telemetry is best-effort. Missing or unsupported values
are not fabricated, and optional telemetry does not make an otherwise valid
core sample unavailable. See [HTTP API](api.md) for field behavior and
[Security](../SECURITY.md) for the project's security guidance.
