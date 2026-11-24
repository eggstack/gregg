# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A keyboard-driven terminal monitor for system metrics across multiple hosts.
Each monitored host needs a reachable `greggd` daemon. The `gregg` client polls
through a per-user client daemon, so multiple TUI windows share one polling
cycle.

## Install

Prebuilt releases are recommended and do not require Rust. Linux or macOS:

```sh
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg
```

For a source install, use Rust 1.89 or newer:

```sh
cargo install gregg --locked
```

See the [installation guide](https://github.com/eggstack/gregg/blob/main/docs/installation.md)
for Windows, other platforms, updates, and uninstall instructions.

## Start monitoring

```sh
gregg add 192.168.1.10:11310
gregg add deadpool@server.local:11310
gregg
```

`gregg add` requires an explicit port. It accepts `host:port`, `[ipv6]:port`,
`nickname@host:port`, and `http://host:port/`. HTTPS is not supported.

## Configure endpoints

```text
gregg list
gregg remove
gregg remove server.local
gregg eggpool add pool.local:11300 --https
gregg eggpool list
gregg eggpool remove
```

`gregg remove` without an endpoint opens a selector for monitored machines;
`gregg eggpool remove` does the same for the configured EggPool endpoint. Use
`j`/`k` or Up/Down to move, Enter to remove the highlighted item, and Esc or
`q` to cancel. A supplied target keeps the existing non-interactive behavior:
host-only system removal removes every matching host, while `host:port` removes
one endpoint. EggPool target matching remains host-wide or exact by port.
Without a terminal, provide a target for either remove command.

## Navigation

| Key | Action |
| --- | --- |
| `j` / Down, `k` / Up | Select a host |
| `h` / Left, `l` / Right | Switch panes |
| `v` | Toggle normal and condensed layouts |
| `d` | Expand drive details |
| `n` | Expand network details |
| `c` | Expand scheduled-job details when supported |
| `Ctrl-R` | Reload the Systems config or refresh the active pane |

The client daemon starts on demand and continues polling after the last TUI
window closes. It does not fall back to polling directly if it cannot connect.
See the [client guide](https://github.com/eggstack/gregg/blob/main/docs/client.md)
for configuration, EggPool, and daemon management.

## Security

The `greggd` HTTP listener has no TLS or authentication and is intended for
trusted private networks. See the project's [security guidance](https://github.com/eggstack/gregg/blob/main/SECURITY.md).

## License

MIT
