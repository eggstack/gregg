# gregg

Compact keyboard-first terminal monitor for observing system metrics across
multiple machines.

## Installation

```sh
cargo install gregg
```

## Usage

Start the TUI:

```sh
gregg
```

Manage endpoints:

```sh
gregg add 192.168.1.10
gregg add deadpool.local:11320
gregg list
gregg remove 192.168.1.10
gregg refresh 30
gregg edit
```

## Navigation

| Key | Action |
| --- | --- |
| `j` / Down | Next system |
| `k` / Up | Previous system |

## Requirements

Each monitored host must have `greggd` running and reachable on the
configured port (default 11310).

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
