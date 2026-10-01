# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

A compact terminal monitor for CPU, memory, swap, load, disk, and optional
live throughput across multiple machines over LAN.

A lightweight daemon (`greggd`) runs on each monitored machine and serves a
read-only JSON API on port `11310`. The `gregg` client polls your fleet and
renders a live TUI.

## Quickstart

### 1. Install the daemon on each machine

Linux / macOS:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- greggd
```

Without `sudo` for a user-local install (no system service):

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | bash -s -- greggd
```

Windows (PowerShell, Administrator for service registration):

```powershell
irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
.\install.ps1 -Component Greggd
```

Verify it is serving:

```bash
curl -fsS http://127.0.0.1:11310/v2/healthz
```

### 2. Install the client on your workstation

Linux / macOS:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg \
  && export PATH="$HOME/.local/bin:$PATH"
```

(Append the trailing `export` only when `~/.local/bin` is not yet on `PATH`.)

Windows (PowerShell):

```powershell
.\install.ps1 -Component Gregg
```

No compiler needed. With Rust 1.89+ (or on source-only hosts such as ARMv7):

```bash
cargo install gregg --locked
cargo install greggd --locked
```

See [Installation](docs/installation.md) for pinned versions, direct
downloads, PATH/profile behavior, and the Cargo fallback.

### 3. Add endpoints and launch

```bash
gregg add 192.168.1.10:11310
gregg add 192.168.1.11:11310
gregg add deadpool@192.168.1.10:11310     # `nickname@host:port` form
gregg refresh 30
gregg
```

`gregg add` requires an explicit port (`host:port`,
`nickname@host:port`, or `http://host:port/`); host-only input is rejected
and HTTPS is never accepted. `gregg remove` accepts host-only input. See
[Client](docs/client.md).

## Supported targets

| Platform | Architecture | Rust target / asset suffix |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 (Pi / Le Potato) | `aarch64-unknown-linux-gnu` |
| macOS | Intel (x86-64) | `x86_64-apple-darwin` |
| macOS | Apple Silicon (arm64) | `aarch64-apple-darwin` |
| Windows | x86-64 | `x86_64-pc-windows-msvc.exe` |

Linux assets target glibc 2.17. macOS binaries are unsigned (approve via
System Settings or `xattr -d com.apple.quarantine`). Linux ARMv7 is
source-build only.

## Essential commands

```bash
greggd host 127.0.0.1              # restrict to localhost (SSH tunnel only)
greggd port 11311                  # change the listen port
greggd startup install             # register automatic startup (systemd / launchd / cron / SCM)
greggd restart                     # manager-aware restart
greggd update                      # update to the latest stable release
greggd stop                        # stop the local daemon
greggd configprint                 # print the configured bind address
greggd status                      # read-only diagnostics: version, bind, health, startup state
greggd uninstall --dry-run         # preview removal (mutates nothing)

gregg list                         # list configured endpoints
gregg remove 192.168.1.10          # host-only remove is supported
gregg edit                         # open config in $EDITOR
gregg update                       # update the client
gregg uninstall --dry-run          # preview removal (mutates nothing)
```

Add `--purge` to either `uninstall` to also remove its config file
(destructive); config is preserved by default. Reinstalling at the same scope
replaces that component in place; `update` upgrades the exact invoked binary.
See [Daemon](docs/daemon.md) and [Client](docs/client.md).

## TUI navigation

- `j` / `k` (or arrow keys): move between systems
- `h` / `l`: cycle panes
- `v`: toggle normal/condensed layout
- `d`: expand/collapse drives for the selected system
- `n`: expand/collapse network details for the selected system
- `Ctrl-R`: reload config and poll immediately

## Live metrics and compatibility

Beyond the core CPU/memory/load/swap/drive gauges, v2 daemons may report
best-effort live telemetry: current CPU frequency, disk `R/s`/`W/s` byte
rates, and directional network `Rx/s`/`Tx/s` with link capacities. Values are
omitted (never fabricated) when the host API cannot provide them, and older
daemons stay online with their historical metrics intact. See
[Display](docs/display.md) for row and width policies.

## Docs

- [Installation](docs/installation.md) — installer behavior, pinned versions, direct downloads, Cargo fallback
- [Daemon](docs/daemon.md) — config, startup, `croncheck`, `stop`, updates, platform notes
- [Client](docs/client.md) — endpoint forms, offline polling, TUI details, EggPool
- [Display](docs/display.md) — metric rows, compact mode, offline rendering
- [API](docs/api.md) — HTTP endpoints
- [Development](docs/development.md) — local builds and operator-managed installs

## Security

The daemon is designed for **private-network** use only. It has no TLS, authentication, rate limiting, or public-internet hardening. See [SECURITY.md](SECURITY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
