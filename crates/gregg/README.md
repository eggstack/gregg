# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

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
gregg add http://192.168.1.10:11310/
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
| `v` | Toggle Normal/Condensed Systems view |
| `e` | Expand or collapse selected-system drives |
| `Ctrl-R` | Reload Systems config and refresh, or refresh EggPool |

## Requirements

Each monitored host must have `greggd` running and reachable on the
configured port (default 11310).

The normal view uses five rows for an online system, including aggregate disk
capacity. The condensed view uses one comparison row per system. `e` adds
bounded detail rows for valid mounted-local-filesystem records belonging only
to the selected online system; view and expansion state are not persisted.

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
The optional `gregg eggpool add HOST` command configures one EggPool source,
using HTTP port `11300` by default. Use `--https`, `--name`,
`--api-key-env ENV_NAME`, and `--replace` as needed. Only the environment
variable name is persisted; the secret is never read by these commands.
The TUI consumes only `/api/stats/summary` and requires EggPool's dashboard/
statistics routes to be enabled. It displays accounted tokens, provider
cache-read share, output tokens per second, and average TTFT for `1h`, `24h`,
`7d`, or `30d`; it does not display a request-level cache hit rate or invent
values when a metric is unavailable. `j`/Down and `k`/Up select the period,
while `h`/Left and `l`/Right enter or leave the pane. `Ctrl-R` refreshes only
the active pane, and EggPool's active refresh cadence is fixed at 60 seconds.
Omit `[eggpool]` to remove the pane and all EggPool worker/network activity.
Editor fallbacks: `hx`, `vim`, `vi` (Unix) or `hx`, `code`, `notepad` (Windows).

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
