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
| `h` / Left | Previous view |
| `l` / Right | Next view |
| `e` | Expand or collapse selected-system drives |

## Requirements

Each monitored host must have `greggd` running and reachable on the
configured port (default 11310).

## Supported platforms

| Platform | Architecture | Status |
| --- | --- | --- |
| Linux | x86-64 | Supported |
| Linux | ARM64 | Supported |
| macOS | Intel (x86-64) | Supported |
| macOS | Apple Silicon (arm64) | Supported |
| Windows | x86-64 | Supported |

On Windows, configuration is stored in `%APPDATA%\gregg\gregg.toml`.
Cross-process config locking uses Windows-native `LockFileEx`.
Editor fallbacks: `hx`, `code`, `notepad`.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
