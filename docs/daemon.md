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

## Managing the daemon

```bash
greggd run                                # foreground (normal command)
greggd host 127.0.0.1                     # restrict to localhost (SSH tunnel only)
greggd port 11311                         # change the listen port
greggd configprint                        # print the configured bind address
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

## Platform notes

- Collectors use kernel interfaces (`/proc`), Mach APIs, or Windows native
  APIs. No external commands are executed for metrics collection.
- macOS does not expose an aggregate CPU I/O-wait state; it is reported as
  `null`, never fabricated as zero.
- Windows does not report load averages or swap; it reports memory commit
  charge instead.
- Drive capacity is summed from mounted local volumes. Network, pseudo,
  optical, and RAM-backed volumes are omitted.
- The daemon has no TLS, authentication, rate limiting, or public-internet
  hardening. See [SECURITY.md](../SECURITY.md).
