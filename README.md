# gregg

`gregg` is a compact, keyboard-first terminal monitor for observing CPU, memory, swap, load, and related host statistics across multiple machines.

The project is intentionally narrow. A lightweight daemon, `greggd`, runs on designated Linux or macOS systems and exposes one small read-only JSON API. The `gregg` client polls configured daemons and renders each reachable system in four terminal rows, with unreachable systems collapsed to one row and moved to the bottom of the view.

## Installation

```text
cargo install gregg-protocol   # library crate, not needed directly
cargo install greggd           # daemon
cargo install gregg            # client + TUI
```

## Supported targets

| Platform | Architecture | Status |
| --- | --- | --- |
| Linux | x86-64 | Supported |
| Linux | ARM64 | Supported |
| macOS | Intel (x86-64) | Supported |
| macOS | Apple Silicon (arm64) | Supported |

## Goals

- Keep the daemon suitable for servers, workstations, and resource-constrained single-board computers.
- Support Linux and macOS daemons in version 1, including x86-64, ARM64 Linux, Intel Macs, and Apple Silicon Macs.
- Keep the TUI useful in a small terminal-multiplexer pane.
- Separate collection, protocol, polling/state management, and rendering so each can be tested independently.
- Prefer stable, read-only, local-network operation over broad monitoring-platform functionality.

## Workspace

The workspace contains three independently publishable crates:

| Crate | Binary/library | Responsibility |
| --- | --- | --- |
| `gregg-protocol` | library | Versioned JSON wire types, metric capabilities, endpoint identity, and compatibility rules. |
| `greggd` | `greggd` binary | Native Linux/macOS metrics collection, periodic sampling, cached immutable snapshots, read-only HTTP API, graceful shutdown, configuration management, and native service integration. |
| `gregg` | `gregg` binary | Endpoint configuration, bounded concurrent polling, application state, keyboard input, and compact Ratatui rendering. |

The protocol crate is intentionally dependency-light (serde, serde_json, thiserror) and must not depend on the daemon server stack or TUI stack.

## Daemon

### Running

```text
greggd run [--config PATH]
greggd start
greggd stop
greggd restart
greggd croncheck
```

`greggd run` is the foreground process used by systemd or launchd. It samples metrics on a configurable interval and serves a cached immutable snapshot over HTTP/1. The daemon does not self-daemonize or maintain PID files.

### Configuration

The `--config` flag overrides the platform default configuration path:

- Linux: `/etc/gregg/greggd.toml`
- macOS: `/Library/Application Support/gregg/greggd.toml`

Configuration-changing commands validate and atomically persist the new configuration before restarting the native service.

### Service installation

Linux (systemd):

```text
cp packaging/systemd/greggd.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now greggd
```

macOS (launchd):

```text
cp packaging/launchd/com.eggstack.greggd.plist /Library/LaunchDaemons/
launchctl bootstrap system /Library/LaunchDaemons/com.eggstack.greggd.plist
```

## Client

### Commands

```text
gregg                          # start the TUI
gregg add 192.168.182.8        # add an endpoint
gregg add deadpool.local:11320 # add with custom port
gregg list                     # list configured endpoints
gregg remove 192.168.182.8     # remove an endpoint
gregg refresh 30               # set polling interval (seconds)
gregg edit                     # open config in $EDITOR
```

### TUI navigation

- `j` / Down: move to the next system
- `k` / Up: move to the previous system
- Viewport scrolls by system entry, not by raw row

## Display model

A reachable system consumes exactly four rows:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
CPU  [||||||||||||                                  ] 25.2%
MEM  [||||||||||||||||||                            ] 37.8%  5.9/15.6 GiB
SWAP [                                                ]  0.0%  0/4.0 GiB
```

A macOS system uses the same layout. macOS does not expose a CPU accounting state equivalent to Linux `iowait`; that capability is reported as unavailable and rendered as `IO --`.

An unreachable system consumes one row:

```text
Deadpool@192.168.182.8:11310 offline
```

## API

The default port is `11310`. The read-only HTTP surface:

```text
GET /
GET /v1/status
GET /healthz
```

The daemon serves cached immutable snapshots. Requests do not trigger metric collection. The schema carries an explicit version and metric-capability flags so unsupported platform metrics remain distinguishable from measured zero values.

## Platform notes

Linux collection uses native kernel interfaces (`/proc/stat`, `/proc/loadavg`, `/proc/meminfo`). macOS collection uses Mach host statistics and `sysctlbyname` through a contained FFI boundary. External utilities are diagnostic references, not runtime dependencies.

Service integration is native to each platform (systemd on Linux, launchd on macOS).

## Security

The daemon is designed for **private-network** use only. It does not provide TLS, authentication, rate limiting, or public-internet hardening. See [SECURITY.md](SECURITY.md) for details.

## Known limitations

- macOS has no Linux-equivalent aggregate CPU I/O-wait state. It is reported as unsupported (`iowait_pct: null`) rather than fabricated as zero.
- Per-process inspection, historical telemetry, alerting, and web dashboards are explicitly out of scope for version 1.

## Non-goals

`gregg` is not intended to become a replacement for htop, btop, Glances, Netdata, or a general monitoring platform. Version 1 excludes per-process inspection, remote command execution, historical databases, alerting, web dashboards, service discovery, plugins, Prometheus emulation, TLS automation, and public-internet hardening.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

The project is released under the [MIT License](LICENSE). Every published
crate inherits the same license expression from the workspace root.

## Local development

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
cargo deny check
cargo package -p gregg-protocol --allow-dirty --no-verify
cargo package -p greggd --allow-dirty --no-verify
cargo package -p gregg --allow-dirty --no-verify
```

The pinned toolchain lives in `rust-toolchain.toml` and tracks the current
stable Rust release. `rust-version` in every member manifest is set from the
workspace `rust-version = "1.75"`.
