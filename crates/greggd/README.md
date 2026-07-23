# greggd

Lightweight Linux and macOS metrics daemon for the gregg monitoring ecosystem.

## Installation

```sh
cargo install greggd
```

## Usage

Run the daemon in the foreground (intended for systemd or launchd):

```sh
greggd run
greggd run --config /path/to/greggd.toml
```

Manage the system service:

```sh
greggd start
greggd stop
greggd restart
```

## Configuration

Default config path:

- **Linux:** `/etc/gregg/greggd.toml`
- **macOS:** `/Library/Application Support/gregg/greggd.toml`

Override the default with `--config PATH`.

## Network

This daemon is designed for private networks only. It exposes a read-only
HTTP/1 JSON API on the configured port (default 11310) and is not hardened
for public internet exposure.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
