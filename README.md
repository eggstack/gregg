# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

A compact terminal monitor for observing CPU, memory, swap, load, disk usage,
and optional live throughput across multiple machines over LAN.

A lightweight daemon (`greggd`) runs on each machine you want to monitor and exposes a read-only JSON API on port `11310`. The `gregg` client polls configured daemons and renders a live TUI.

Schema-v2 status responses may include best-effort native live telemetry: current
CPU frequency, de-duplicated disk read/write bytes per second, and directional
network throughput with link capacities. Values are omitted when the host API
cannot provide them; no privilege escalation or external metrics command is
used.

## Supported targets

| Platform | Architecture | Rust target | Asset suffix |
| --- | --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 (64-bit Raspberry Pi / Le Potato) | `aarch64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu` |
| macOS | Intel (x86-64) | `x86_64-apple-darwin` | `x86_64-apple-darwin` |
| macOS | Apple Silicon (arm64) | `aarch64-apple-darwin` | `aarch64-apple-darwin` |
| Windows | x86-64 | `x86_64-pc-windows-msvc` | `x86_64-pc-windows-msvc.exe` |

Linux assets target glibc 2.17. macOS binaries are unsigned (approve via System Settings or `xattr -d com.apple.quarantine`). Linux ARMv7 is source-build only.

## Quickstart

### 1. Install the daemon on each machine

Linux / macOS:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- greggd
```

This installs the daemon and registers automatic startup. For a user-local
install without a system service, run without `sudo`:

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
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | bash -s -- gregg
```

Windows (PowerShell):

```powershell
.\install.ps1 -Component Gregg
```

Alternative (any platform with Rust, or source-only hosts such as ARMv7):

```bash
cargo install gregg --locked
cargo install greggd --locked
```

Make sure the install directory is on your `PATH` (`$HOME/.local/bin` for
user-local Unix installs). See [docs/installation.md](docs/installation.md)
for pinned versions, direct downloads, and installer details.

### 3. Add endpoints and launch

```bash
gregg add 192.168.1.10:11310
gregg add 192.168.1.11:11310
gregg add deadpool@192.168.1.10:11310     # `nickname@host:port` form
gregg refresh 30
gregg
```

`gregg add` requires an explicit port (`host:port`,
`nickname@host:port`, or `http://host:port/`). Host-only input is rejected.

## Configuration

Daemon config file:

| Platform | Path |
| --- | --- |
| Linux | `/etc/gregg/greggd.toml` |
| macOS | `/Library/Application Support/gregg/greggd.toml` |
| Windows | `%ProgramData%\gregg\greggd.toml` |

```bash
greggd host 127.0.0.1              # restrict to localhost (SSH tunnel only)
greggd port 11311                  # change the listen port
greggd startup install             # register automatic startup (systemd / launchd / cron / SCM)
greggd restart                     # manager-aware restart
greggd update                      # update to the latest stable release
greggd stop                        # stop the local daemon
greggd configprint                 # print the configured bind address
greggd status                      # read-only diagnostics: version, bind, health, startup state
```

Client config: Linux `~/.config/gregg/gregg.toml` (honors `XDG_CONFIG_HOME`),
macOS `~/Library/Application Support/gregg/gregg.toml`, Windows
`%APPDATA%\gregg\gregg.toml`.

```bash
gregg list                         # list configured endpoints
gregg remove 192.168.1.10          # host-only remove is supported
gregg edit                         # open config in $EDITOR
gregg update                       # update the client
```

## TUI navigation

- `j` / `k` (or arrow keys): move between systems
- `h` / `l`: cycle panes
- `v`: toggle normal/condensed layout
- `e`: expand/collapse drives for the selected system
- `n`: expand/collapse network details for the selected system
- `Ctrl-R`: reload config and poll immediately

## Live metrics and compatibility

Schema-v2 live telemetry is additive and optional. The client still tries
`/v2/status` first and falls back to a v1-only daemon only on HTTP 404, so
older daemons remain online with CPU, memory, load, swap, and historical drive
behavior intact. Pre-feature v2 daemons likewise remain healthy; they simply
omit the newer fields. Gregg does not track daemon versions over the wire.

CPU frequency, when present, is the current OS-reported frequency in raw Hz,
not a base or maximum-clock claim. macOS may omit it because Gregg uses no
privileged or undocumented frequency mechanism. Disk capacity (`used / total`)
and disk I/O are separate accounting domains: `R/s` and `W/s` are byte rates,
and per-drive rates appear only for exact device associations. Network `Rx/s`
and `Tx/s` are byte rates; NET utilization uses the maximum valid directional
rate against its link capacity, so full-duplex traffic is not double-counted.
Loopback can appear in `n` detail but is excluded from aggregate capacity.

Use `e` for disk details, `n` for network/interface details, and `v` for the
width-aware condensed view. See [Display](docs/display.md) for the mixed-fleet
row and width policies.

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
