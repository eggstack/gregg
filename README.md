# gregg

`gregg` is a compact, keyboard-first terminal monitor for observing CPU, memory, swap, load, and related host statistics across multiple machines.

The project is intentionally narrow. A lightweight daemon, `greggd`, runs on designated Linux or macOS systems and exposes one small read-only JSON API. The `gregg` client polls configured daemons and renders each reachable system in four terminal rows, with unreachable systems collapsed to one row and moved to the bottom of the view.

## Installation

```text
cargo install greggd           # daemon
cargo install gregg            # client + TUI
```

`gregg-protocol` is a library crate for Cargo dependencies; it is not installed directly. If you are building a tool that consumes the Gregg JSON contract, add `gregg-protocol` as a dependency in your `Cargo.toml`:

```toml
gregg-protocol = "1.0"
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
cargo build --release
```

The sustained workload evidence runner exercises the production polling and
state-reduction path for a configurable duration:

```text
python3 scripts/run-mixed-fleet-sustained.py \
  --duration-seconds 2 \
  --sample-interval-seconds 0.2 \
  --evidence-dir target/sustained-smoke
```

For release-candidate verification, use the manually dispatched staged workflow
at `.github/workflows/release-candidate.yml`. Run `protocol-prepublish` before
publication, then prove the indexed `gregg-protocol 1.0.1` with
`protocol-index-check` before running `binary-prepublish` or binary MSRV gates.
The native and protected operational stages are separate evidence runs, and
`postpublish-verify` installs the published binaries from crates.io.

The workflow owns package installation: it unpacks each `.crate`, installs into
an empty root, records checksum and size, and passes the installed `greggd`
path to `scripts/verify-installed-daemon.sh`. That verifier only tests a
supplied binary and never falls back to `target/release/greggd`.

Phase-35 release-control qualification is nonpublishing and can be dispatched
from `.github/workflows/phase35-qualification.yml` at an exact commit SHA. The
workflow runs the local gates, sustained smoke, and repository-owned
full-contract qualification harness, then independently validates and uploads
evidence fail-closed. The harness loads the production requirements and
dispatch contracts, exercises every required pre-tag and final logical stage,
and uses a loopback sparse Cargo registry to prove checksum-bearing Boundary-2
locks without touching crates.io. Both dependent-package runs retain and
validate a complete command-evidence index. The
finalizer receives post-freeze run selection and the operator's historical
`1.0.0` disposition as base64-encoded JSON, records canonical identity
documents, and executes only tooling from the immutable candidate checkout.
Final singleton evidence is resolved by canonical role and materialized before
aggregation. Boundary-2 unconditionally compares the generated lockfile
checksum with the validated registry response and retains replayable command
transcripts and their digests.

Phase 35 closes the evidence-lineage and production-finalizer defects found
after Phase 34: Boundary-2 candidates are real production-shaped artifacts
retrieved through the same mock API path, package archives are built once and
reused unchanged across all boundaries, the postpublish ZIP is a genuine
selected artifact containing every candidate-declared file, and final
aggregation consumes only role-indexed materialized paths through a shared
preparation helper.

The pinned toolchain lives in `rust-toolchain.toml` and tracks the current
stable Rust release. `rust-version` in every member manifest is set from the
workspace `rust-version = "1.75"`.
