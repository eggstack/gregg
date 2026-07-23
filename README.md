# gregg

`gregg` is a compact, keyboard-first terminal monitor for observing CPU, memory, swap, load, and related host statistics across multiple machines.

The project is intentionally narrow. A lightweight daemon, `greggd`, runs on designated Linux or macOS systems and exposes one small read-only JSON API. The `gregg` client polls configured daemons and renders each reachable system in four terminal rows, with unreachable systems collapsed to one row and moved to the bottom of the view.

> Project status: phases 1 through 4 are implemented. Phase 4 adds the
> daemon sampler, HTTP server, and graceful shutdown. Client and TUI
> work continues in phases 5-8 per [`plans/`](plans/).

## Goals

- Keep the daemon suitable for servers, workstations, and resource-constrained single-board computers.
- Support Linux and macOS daemons in version 1, including x86-64, ARM64 Linux, Intel Macs, and Apple Silicon Macs.
- Keep the TUI useful in a small terminal-multiplexer pane.
- Separate collection, protocol, polling/state management, and rendering so each can be tested independently.
- Publish all three workspace crates to crates.io for the version-1 release.
- Prefer stable, read-only, local-network operation over broad monitoring-platform functionality.

## Workspace

The intended workspace contains three independently publishable crates:

| Crate | Binary/library | Responsibility |
| --- | --- | --- |
| `gregg-protocol` | library | Versioned JSON wire types, metric capabilities, endpoint identity, and compatibility rules. |
| `greggd` | `greggd` binary | Native Linux/macOS metrics collection, periodic sampling, cached immutable snapshots, read-only HTTP API, and graceful shutdown. |
| `gregg` | `gregg` binary | Endpoint configuration, bounded concurrent polling, application state, keyboard input, and compact Ratatui rendering. |

The protocol crate must remain lightweight and must not depend on the daemon server stack or TUI stack.

## Version-1 display model

A reachable system consumes exactly four rows and does not require a border:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
CPU  [||||||||||||                                  ] 25.2%
MEM  [||||||||||||||||||                            ] 37.8%  5.9/15.6 GiB
SWAP [                                                ]  0.0%  0/4.0 GiB
```

A macOS system uses the same schema and layout. Because macOS does not expose a CPU accounting state equivalent to Linux `iowait`, that capability is reported as unavailable and rendered as `IO —` rather than as a fabricated zero.

An unreachable system consumes one row:

```text
Deadpool@192.168.182.8:11310 offline
```

Reachable systems preserve configured order. Unreachable systems preserve their relative configured order but are displayed after all reachable systems.

## Intended commands

Daemon commands:

```text
greggd run
greggd start
greggd stop
greggd restart
greggd croncheck
greggd host 127.0.0.1
greggd port 11320
```

`greggd run` is the foreground process used by systemd or launchd. It starts a process that samples metrics on a configurable interval and serves a cached immutable snapshot over HTTP/1. The daemon does not self-daemonize or maintain PID files. Configuration-changing commands validate and atomically persist the new configuration before restarting the native service.

Client commands:

```text
gregg
gregg add 192.168.182.8
gregg add deadpool.local:11320
gregg list
gregg remove 192.168.182.8
gregg refresh 30
gregg edit
```

Running `gregg` without a subcommand starts the TUI. Default polling is every five seconds. Navigation is keyboard-first: `j`/Down moves to the next system, `k`/Up moves to the previous system, and the viewport scrolls by system rather than by raw row.

## API direction

The default port is `11310`; `113100` is outside the valid TCP port range. The intended read-only surface is:

```text
GET /
GET /v1/status
GET /healthz
```

The daemon samples metrics on its own cadence and serves a cached immutable snapshot. Requests do not trigger metric collection. The schema carries an explicit version and metric-capability flags so unsupported platform metrics remain distinguishable from measured zero values.

## Platform model

Linux collection is expected to use native kernel interfaces such as `/proc/stat`, `/proc/loadavg`, and `/proc/meminfo`. macOS collection is expected to use Mach host statistics and `sysctlbyname` through a small, contained FFI boundary. External utilities such as `top`, `vm_stat`, `sysctl`, or `sw_vers` are diagnostic references, not runtime dependencies.

Service integration is native to each platform:

- Linux: systemd system service.
- macOS: launchd system daemon.

The client remains platform-neutral wherever Ratatui, Crossterm, and the HTTP client support the target.

## Non-goals for version 1

`gregg` is not intended to become a replacement for htop, btop, Glances, Netdata, or a general monitoring platform. Version 1 excludes per-process inspection, remote command execution, historical databases, alerting, web dashboards, service discovery, plugins, Prometheus emulation, TLS automation, and public-internet hardening.

## Planning

The implementation roadmap and execution-ready phase plans are stored in [`plans/`](plans/). Work should follow the phase acceptance criteria and preserve the scope boundaries above.

## License

The project is released under the [MIT License](LICENSE). Every published
crate inherits the same license expression from the workspace root.

## Local development

Phase 1 enforces the following on every commit through CI. Phase 2 adds the
Linux collector, which is tested on `ubuntu-latest` in CI:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
cargo package -p gregg-protocol --allow-dirty --no-verify
```

The pinned toolchain lives in `rust-toolchain.toml` and tracks the current
stable Rust release. `rust-version` in every member manifest is set from the
workspace `rust-version = "1.75"`.
