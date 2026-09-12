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
- `restart` is manager-aware: systemd via `systemctl restart greggd`,
  launchd via `launchctl kickstart -k`, Windows via SCM, otherwise via local
  `stop` plus a detached `run`. Manager calls are bounded with stderr
  preserved; privilege failures print the exact elevated command and return
  `PermissionDenied` without a competing fallback.
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
  atomically replaces the current executable. A missing exact asset (HTTP
  404) permits a staged `cargo install --locked --version "=X.Y.Z"`
  fallback; checksum/version mismatch, transport failure, or 5xx never fall
  back. Config and startup registration are preserved; only a
  running/managed daemon is restarted, intentionally stopped services stay
  stopped, and a replacement whose restart fails reports
  `Installed X.Y.Z but not activated` with the exact restart command. No
  background checks, package-manager integration, or automatic `sudo`.
  The download/verify/stage/replace mechanism is shared with `gregg update`
  in the internal `gregg-update` crate; `greggd` owns only
  activation/restart coordination.

## Platform notes

- Collectors use kernel interfaces (`/proc`), Mach APIs, or Windows native
  APIs. No external commands are executed for metrics collection.
- V2 live telemetry is best-effort and native: Linux reads CPUFreq policy data,
  top-level block statistics, and procfs/sysfs network data; macOS reads public
  AF_LINK and IOKit records; Windows reads processor power information, direct
  disk performance IOCTLs, and IP Helper interface rows. Rates use monotonic
  elapsed time and re-warm after reset, restart, disappearance, or hotplug.
- macOS does not expose an aggregate CPU I/O-wait state; it is reported as
  `null`, never fabricated as zero.
- Windows does not report load averages or swap; it reports memory commit
  charge instead.
- Drive capacity is summed from mounted local volumes. Network, pseudo,
  optical, and RAM-backed volumes are omitted.
- The daemon has no TLS, authentication, rate limiting, or public-internet
  hardening. See [SECURITY.md](../SECURITY.md).
