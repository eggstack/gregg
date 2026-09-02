# greggd

[![Crates.io](https://img.shields.io/crates/v/greggd.svg)](https://crates.io/crates/greggd)
[![Docs.rs](https://docs.rs/greggd/badge.svg)](https://docs.rs/greggd)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/greggd.svg)](https://crates.io/crates/greggd)

Lightweight Linux, macOS, and Windows metrics daemon for the gregg monitoring ecosystem.

## Installation

Prebuilt binaries are published on GitHub Releases for common platforms; the
bootstrap installer is binary-first with Cargo fallback.

```sh
# Prebuilt binary (recommended)
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
# or pinned version / both components:
curl -fsSL https://github.com/eggstack/gregg/releases/download/v1.0.11/install.sh | sudo sh -s -- greggd --version 1.0.11
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
# Windows (PowerShell, as Administrator)
# irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
#   .\packaging\install.ps1 -Component Greggd

# From crates.io / source (fallback)
cargo install greggd
```

Prebuilt assets (glibc 2.17 on Linux, unsigned on macOS):

- `greggd-x86_64-unknown-linux-gnu` · `greggd-aarch64-unknown-linux-gnu` (covers 64-bit Raspberry Pi/Le Potato)
- `greggd-x86_64-apple-darwin` · `greggd-aarch64-apple-darwin`
- `greggd-x86_64-pc-windows-msvc.exe`

Linux ARMv7 (`armv7l`) is source-build only and uses `cargo install` when available.
See `packaging/README.md` for the full bootstrap contract, checksum verification,
and `install.sh`/`install.ps1` details.

## Usage

Run the daemon directly in the foreground. On Unix, systemd and launchd are optional
operator-managed deployment mechanisms; `greggd` does not invoke them.

```sh
greggd run
greggd run --config /path/to/greggd.toml
```

On Windows, native SCM lifecycle commands remain available:

```sh
greggd start
greggd stop
greggd restart
```

Automatic startup and restart (cross-platform `restart`, Unix `startup`):

```sh
greggd startup install                        # auto: systemd / launchd / cron / Windows SCM
greggd startup install --method systemd       # explicit method
greggd startup instructions                   # read-only, prints exact commands/paths
greggd startup instructions --method cron
greggd restart                                # manager-aware restart (systemd / launchd / SCM / direct)
```

`startup install` defaults to `auto`: Windows→SCM, macOS→launchd, Linux with running systemd→systemd, else cron. Systemd uses `/usr/local/bin/greggd`, `/etc/gregg/greggd.toml`, `greggd` user/group, `/etc/systemd/system/greggd.service` (atomic, `daemon-reload` + `enable` + `start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist`; cron uses an idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab, never edits `/var/spool/cron`). An identified systemd/launchd host never silently falls back to cron on permission failure; the exact `sudo <exe> startup install --method <...>` is printed and exit 4 is returned. No internal `sudo`. `startup instructions` never mutates state. `restart` is manager-aware and factored for `update` reuse (systemd via `systemctl restart greggd`, launchd via `launchctl kickstart -k`, Windows via SCM, otherwise `stop` + detached `run`).

Ensure the daemon is running. `croncheck` is a watchdog for cron, Task
Scheduler, and other supervisors without built-in readiness monitoring. It
probes `/v2/healthz` with bounded raw HTTP on the configured local endpoint,
normalizing wildcard binds to loopback. Valid Gregg Ready, Warming, and Failed
responses all mean the daemon is running. Only a refused connection proves the
endpoint absent and permits spawning `greggd run` as a detached child; unrelated,
malformed, silent, or ambiguous peers return nonzero without spawning. No service
manager is invoked.

```sh
greggd croncheck
greggd configprint
greggd version
```

On Windows, the service entry point is `greggd service` (internal, used by the SCM). Install/uninstall via the provided PowerShell scripts in `packaging/`. For startup, the PowerShell installer remains the canonical SCM registration; `startup install` on Windows reports service state and `startup instructions` prints SCM commands.

## Configuration

Default config path:

- **Linux:** `/etc/gregg/greggd.toml`
- **macOS:** `/Library/Application Support/gregg/greggd.toml`
- **Windows:** `%ProgramData%\gregg\greggd.toml`

Override the default with `--config PATH`.

`configprint` is read-only and prints the configured bind address as a
canonical socket address, with bind wildcards resolved to the host's primary
local IP so the output is a usable address a remote client can dial. A
specific configured host is preserved unchanged, and a wildcard is preserved
verbatim if the local IP cannot be resolved. The output looks like
`192.168.182.143:11310` or `[fd00::10]:11310`. The command does not probe
the network, bind a listener, start, stop, or modify the daemon; wildcard
resolution uses a transient UDP `connect()` that performs a local route
lookup only and transmits no packets.

## Network

This daemon is designed for private networks only. It exposes a read-only
HTTP/1 JSON API on the configured port (default 11310) and is not hardened
for public internet exposure. No firewall rules are created automatically.
LAN exposure is operator-controlled and the daemon has no TLS or authentication.

`/v2/status` is the universal status endpoint and may include a bounded
`drives` list of mounted local filesystem names with numeric used and total
bytes. Missing or `null` drive data means unavailable/legacy; an empty list
means successful enumeration with no eligible volumes. Collection is
best-effort and does not model physical disks or storage topology. Linux and
macOS retain `/v1/status`; Windows is v2-only for status semantics.

The configured daemon display name must be non-empty, at most 128 bytes, and
contain no control characters. If identity collection fails, the daemon does
not publish a blank identity; it remains warming or failed and preserves any
previous valid snapshot.

When writing configuration, a newly created parent directory is restricted to
mode `0700`; an existing operator-managed directory keeps its current
permissions. Metadata errors while loading a default config are reported
instead of silently falling back to defaults.

On Unix, `greggd stop` uses the config-specific local control socket. The
socket path is reserved by the kernel before its restrictive permissions are
verified, so a concurrent path occupant is never replaced; the temp-directory
fallback remains best effort when the config-adjacent directory is unavailable.

If the system clock moves backward, a future-dated cached snapshot is not
treated as stale solely because its timestamp is ahead of the current clock.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
