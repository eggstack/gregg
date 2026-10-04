# Daemon (`greggd`)

The daemon runs on each machine you want to monitor and serves cached
immutable snapshots on its configured port (default `11310`).

## Configuration

| Platform | Path |
| --- | --- |
| Linux | `/etc/gregg/greggd.toml` |
| macOS | `/Library/Application Support/gregg/greggd.toml` |
| Windows | `%ProgramData%\gregg\greggd.toml` |

Default config:

```toml
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
```

### Scheduled maintenance

Jobs are optional and default to an empty list, so existing configs keep their
monitoring behavior. A load-gated weekly job can be configured as:

```toml
allow_privileged_jobs = false

[[jobs]]
name = "cargo-cleanme-deep"
schedule = "0 3 * * 0"
command = ["/usr/local/bin/cargo-cleanme", "scan", "--deep"]
working_dir = "/home/user/projects"
max_load = 8.0
load_window = "15m"
retry_interval_ms = 300000
max_wait_ms = 86400000
```

A time-only job omits all load fields:

```toml
[[jobs]]
name = "refresh-local-index"
schedule = "15 */6 * * *"
command = ["/usr/local/bin/refresh-index"]
```

Schedules use exactly five numeric cron fields with lists, ranges, wildcards,
and steps; weekdays are `0` through `6` (Sunday is `0`). The supported aliases
are `@hourly`, `@daily`, `@weekly`, and `@monthly`. Day-of-month and day-of-week
use traditional cron OR matching when both are restricted. A schedule that no
Gregorian date can satisfy, such as `0 0 31 2 *`, is rejected as an
`InvalidJobs` configuration violation before the daemon binds its listener, so
an impossible expression never reaches runtime. Schedules use local civil
time. A spring-forward time that does not exist is skipped; both real instants
in a fall-back repeated minute run in chronological order. Startup chooses the
first occurrence strictly after its reference time. Missed work is not
replayed after daemon downtime. Large wall-clock adjustments are reconciled
within approximately one minute: the scheduler re-reads civil time at least
once per minute, so a forward jump's skipped occurrences coalesce into at
most one pending occurrence per job rather than replaying, and a backward
jump never launches before the stored occurrence is actually due. Load retry
and maximum wait stay on monotonic time and are unaffected by wall-clock
movement.

Commands are argv arrays and execute directly. Gregg does not parse a command
string or add a shell; shell syntax requires an explicit shell argv such as
`["/bin/sh", "-c", "..."]`. There is no per-job environment or secret map.
The config is readable by diagnostic commands, so command arguments must not
contain credentials or tokens. stdin, stdout, and stderr are discarded.

Jobs always run as greggd's current OS principal. The Linux system service runs
as `greggd` with `ProtectHome=true` and strict filesystem sandboxing, so it
generally cannot maintain a developer's home directory; use a user-owned,
rootless daemon for such jobs. On Unix, euid 0 with any configured jobs fails
closed unless `allow_privileged_jobs = true` is set. This flag does not change
the process identity or weaken service sandboxing. In particular, jobs under
the privileged macOS system LaunchDaemon need that explicit opt-in. Windows
supports time-only jobs and rejects `max_load` while load averages are
unsupported.

Load windows are `1m`, `5m`, or `15m` (default `15m`). Thresholds are inclusive.
Missing, warming, failed, or high load defers the occurrence; retry defaults
to five minutes and maximum wait defaults to 24 hours. Retry is bounded from
10 seconds to one hour, max wait is at most seven days, and retry cannot exceed
max wait. An expired pending occurrence is dropped once. At most one pending
occurrence is retained per job, repeated matches coalesce without extending
the original deadline, and exactly one scheduled command runs globally. Load
is rechecked before each sequential launch. Nonzero command exits are logged
and are not retried; the next cron occurrence is authoritative.

Shutdown stops new launches, terminates the direct child, and waits for it
within a two-second child bound inside the daemon's shared ten-second cleanup
deadline. Gregg does not guarantee termination of descendants created by an
explicit shell or pipeline.

The display name (`name`) must be non-empty, at most 128 bytes, and contain
no control characters. Override the file location per-invocation with
`greggd run --config /path/to/greggd.toml`.

System configs contain no secrets and are world-readable (`0644`) so
unprivileged `croncheck`/`status`/`configprint` work; the Unix control
socket stays owner-only (`0600`). If an older install still reports
`Permission denied (os error 13)` for those read-only commands, repair it
with `sudo greggd startup install --method systemd` (Linux) or
`--method launchd` (macOS).

## Managing the daemon

```bash
greggd run                                # foreground (normal command)
greggd host 127.0.0.1                     # restrict to localhost (SSH tunnel only)
greggd port 11311                         # change the listen port
greggd configprint                        # print the configured bind address
greggd status                             # read-only local diagnostics (version, bind, health, startup state)
greggd croncheck                          # start only when the health endpoint is refused (cron watchdog)
greggd stop                               # stop the local instance via control socket (Unix) or SCM (Windows)
greggd version                            # print the daemon version
```

Automatic startup and restart:

```bash
greggd startup install                    # auto: systemd / launchd / cron / Windows SCM
greggd startup install --method systemd   # explicit: systemd, launchd, or cron
greggd startup instructions               # read-only: exact commands/paths for the detected method
greggd startup instructions --method cron # read-only for a specific method
greggd restart                            # manager-aware restart
greggd update                             # binary-first update; restarts only if running/managed
greggd uninstall --dry-run                # preview removal (mutates nothing)
greggd uninstall                          # remove this binary + Gregg startup integration (config preserved)
greggd uninstall --purge                  # also remove the daemon config/data files (destructive)
```

Details:

- `configprint` is read-only: it prints the configured bind address with
  wildcards resolved to the local IP (for example `192.168.182.143:11310`).
  It does not probe, bind, mutate config, or manage services.
- `status` is read-only local diagnostics. It prints the binary version,
  resolved config path, canonical bind address, the bounded `/v2/healthz`
  classification (`ready`, `warming`, `failed`, `unreachable`, or
  `not-gregg` for a peer that answered but is not a valid Gregg endpoint),
  and the detected startup-manager state. It reuses the same bounded probe
  authority as `croncheck` and never infers process ownership from port
  occupancy. Exit `0` only when a valid Gregg endpoint answered; otherwise
  the report is still printed and a nonzero exit is returned. It never
  starts, stops, restarts, installs, mutates config, or invokes `sudo`.
- `startup install` is `auto` by default: Windows→SCM, macOS→launchd, Linux
  with running systemd→systemd, otherwise cron. Standard systemd paths are
  `/usr/local/bin/greggd`, `/etc/gregg/greggd.toml`, `greggd` user/group,
  `/etc/systemd/system/greggd.service` (atomic install, `daemon-reload` +
  `enable` + `start`/`restart`); launchd uses
  `/Library/LaunchDaemons/com.eggstack.greggd.plist`; cron uses an idempotent
  `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck`
  (shell-quoted, preserves unrelated crontab entries, never edits
  `/var/spool/cron` directly). An identified systemd/launchd host never
  silently falls back to cron on permission failure: the exact
  `sudo <exe> startup install --method <...>` command is printed and exit 4
  (`PermissionDenied`) is returned. No internal `sudo`. `startup
  instructions` never mutates state.
- `restart` is manager-aware and exact-executable-aware: systemd and launchd
  receive restart mutation only when their parsed registration targets the
  invoked executable. Foreign registrations using the selected config and
  unknown active ownership fail closed; a foreign registration with a different
  known config leaves the manager untouched and permits the config-specific
  direct path. Windows queries the SCM image path and preserves foreign,
  unknown, and not-installed states without a direct fallback. Otherwise Unix
  uses local `stop` plus a detached `run`. Manager calls are bounded with
  stderr preserved; privilege failures print the exact elevated command and
  return `PermissionDenied` without a competing fallback.
- `stop` (Linux/macOS) targets only the local instance matching the resolved
  config identity via one Unix-domain control socket (`STOP\n` → `OK\n`).
  Identity is a digest of the normalized config path, so two configs in one
  directory cannot cross-stop. Sockets are created `0600`; stale-socket
  cleanup unlinks only after confirming a socket whose connect fails with
  `ConnectionRefused` or `NotFound`. A missing or unreachable socket is an
  idempotent "not running" success. The HTTP API is read-only and has no
  shutdown endpoint.
- `croncheck` is a watchdog for cron, Task Scheduler, and other supervisors
  without built-in readiness monitoring. It sends a bounded raw HTTP probe to
  `/v2/healthz` on the configured local bind address (wildcards normalized to
  loopback). A valid Gregg Ready, Warming, or Failed response means the
  daemon is running (exit `0`). A refused connection proves the endpoint is
  absent, so it spawns `greggd run` as a detached child with stdio closed
  (new process group on Unix). An unrelated, malformed, silent, or otherwise
  ambiguous peer returns nonzero and never starts a second daemon.
- `update` is binary-first: it queries the latest stable `greggd` crate on
  crates.io (authoritative), compares SemVer-safely with the compiled-in
  version, and if newer downloads the exact `vX.Y.Z` GitHub Release asset for
  the current host plus its `.sha256`, verifies SHA-256 and candidate
  `version` before any replacement, stages to a private temp dir, then
  observes exact-executable `UpdateLifecycle` after preparation and
  immediately before mutation (never host-global `startup_state()`), quiesces
  only where owned, and atomically replaces the current executable. Unix
  combines systemd/launchd ownership with the selected config's bounded
  health: owned active restarts, owned inactive stays stopped, foreign active
  using the selected config is preserved without direct stop/restart, foreign
  inactive/absent or foreign different-config plus running direct intent may
  direct-restart, and unknown active (or unknown inactive with a running
  endpoint) fails before replacement. Windows uses `query_registration()`
  revalidated immediately before quiescence: owned running/start-pending may
  stop then restart, owned stopped stays stopped, owned stop-pending waits
  stopped without restart, `NotInstalled` and foreign do zero SCM mutation,
  missing/ambiguous identity fails before replacement, and an
  owned-to-foreign transition fails with zero mutation. A missing exact asset
  (HTTP 404) permits a staged `cargo install --locked --version "=X.Y.Z"`
  fallback; checksum/version mismatch, transport failure, or 5xx never fall
  back. Config and startup registration are preserved; only
  `ManagedRunning`/`DirectRunning` restart via ownership-aware
  `restart_daemon()`, stopped/foreign stay stopped/preserved, and a
  replacement whose restart fails reports
  `Installed X.Y.Z but not activated` with the exact restart command. No
  background checks, package-manager integration, or automatic `sudo`.
  The download/verify/stage/replace mechanism is shared with `gregg update`
  in the internal `gregg-update` crate; `greggd` owns only
  activation/restart coordination.

`uninstall` removes only the exact invoked `greggd` executable plus startup
integration whose command target matches that executable: the canonical
systemd unit (`ExecStart`), the `com.eggstack.greggd` launchd
`ProgramArguments`, the managed cron block, or the `greggd` SCM image path.
Foreign or ambiguous artifacts are reported by `--dry-run` and preserved;
SCM query uncertainty blocks mutation. Discovery is independent per artifact,
so multiple owned artifacts can be removed together. Permissions are
preflighted before any teardown mutation and nothing invokes `sudo`
internally (rerun the printed platform-correct elevated command instead). An unmanaged
Unix daemon is stopped via the existing control-socket identity; an uncertain
stop blocks deletion rather than orphaning a running process. The `greggd`
system account is left in place. Unix Cargo-owned installs complete owned
startup/direct-stop work, delegate executable removal to `cargo uninstall`,
then apply `--purge`; Windows prints the exact zero-mutation Cargo handoff.
The legacy `packaging/uninstall-windows.ps1` is a thin wrapper around
`greggd uninstall` (`-RemoveConfig` maps to `--purge`) and no longer
recursively deletes the shared install directory.

## Platform notes

- Collectors use kernel interfaces (`/proc`), Mach APIs, or Windows native
  APIs. No external commands are executed for metrics collection.
- V2 live telemetry is best-effort and native: Linux reads CPUFreq policy data,
  top-level block statistics, and procfs/sysfs network data; macOS prefers
  `NET_RT_IFLIST2`/`if_msghdr2` 64-bit network counters (typed
  `getifaddrs`/`if_data` fallback) and IOKit block-storage driver statistics;
  Windows reads processor power information, direct disk performance IOCTLs,
  and IP Helper interface rows. Rates use monotonic elapsed time and re-warm
  after reset, restart, disappearance, or hotplug.
- macOS does not expose an aggregate CPU I/O-wait state; it is reported as
  `null`, never fabricated as zero.
- Windows does not report load averages or swap; it reports memory commit
  charge instead.
- Drive capacity is summed from mounted local volumes. Network, pseudo,
  optical, and RAM-backed volumes are omitted.
- The daemon has no TLS, authentication, rate limiting, or public-internet
  hardening. See [SECURITY.md](../SECURITY.md).
