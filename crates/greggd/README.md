# greggd

[![Crates.io](https://img.shields.io/crates/v/greggd.svg)](https://crates.io/crates/greggd)
[![Docs.rs](https://docs.rs/greggd/badge.svg)](https://docs.rs/greggd)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/greggd.svg)](https://crates.io/crates/greggd)

Lightweight Linux, macOS, and Windows metrics daemon for the gregg monitoring ecosystem.

## Installation

```sh
cargo install greggd
```

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

Ensure the daemon is running. `croncheck` is a watchdog for cron, Task
Scheduler, and other supervisors without built-in readiness monitoring: it
probes the configured local TCP port and, if nothing is listening, spawns
`greggd run` as a detached child. The HTTP API is not consulted and no
service manager is invoked.

```sh
greggd croncheck
greggd configprint
greggd version
```

On Windows, the service entry point is `greggd service` (internal, used by the SCM). Install/uninstall via the provided PowerShell scripts in `packaging/`.

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

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
