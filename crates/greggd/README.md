# greggd

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

Probe daemon health on any platform without changing process state. The probe
uses a short timeout and a fixed 512-byte HTTP/1.x status-line limit; it
requires CRLF termination and accepts only HTTP/1.0 or HTTP/1.1 status 200:

```sh
greggd croncheck
greggd croncheck --target HOST:PORT
greggd configprint
greggd version
```

Without `--target`, `croncheck` probes the configured local daemon address.
The optional target is useful for checking a reachable daemon from another
host; it does not start, stop, or modify any process or configuration.

On Windows, the service entry point is `greggd service` (internal, used by the SCM). Install/uninstall via the provided PowerShell scripts in `packaging/`.

## Configuration

Default config path:

- **Linux:** `/etc/gregg/greggd.toml`
- **macOS:** `/Library/Application Support/gregg/greggd.toml`
- **Windows:** `%ProgramData%\gregg\greggd.toml`

Override the default with `--config PATH`.

`configprint` is read-only and prints exactly the configured bind address as a
canonical socket address, such as `0.0.0.0:11310` or `[::]:11310`. It does not
probe, bind, start, stop, or modify the daemon.

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
