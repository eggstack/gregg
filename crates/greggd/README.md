# greggd

Lightweight Linux, macOS, and Windows metrics daemon for the gregg monitoring ecosystem.

## Installation

```sh
cargo install greggd
```

## Usage

Run the daemon in the foreground (intended for systemd, launchd, or interactive diagnostics):

```sh
greggd run
greggd run --config /path/to/greggd.toml
```

Manage the system service:

```sh
greggd start
greggd stop
greggd restart
greggd croncheck
```

On Windows, the service entry point is `greggd service` (internal, used by the SCM). Install/uninstall via the provided PowerShell scripts in `packaging/`.

## Configuration

Default config path:

- **Linux:** `/etc/gregg/greggd.toml`
- **macOS:** `/Library/Application Support/gregg/greggd.toml`
- **Windows:** `%ProgramData%\gregg\greggd.toml`

Override the default with `--config PATH`.

## Network

This daemon is designed for private networks only. It exposes a read-only
HTTP/1 JSON API on the configured port (default 11310) and is not hardened
for public internet exposure. No firewall rules are created automatically.
LAN exposure is operator-controlled and the daemon has no TLS or authentication.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
