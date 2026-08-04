# gregg

`gregg` is a compact, keyboard-first terminal monitor for observing CPU, memory, swap, load, mounted-local-filesystem capacity, and related host statistics across multiple machines.

The project is intentionally narrow. A lightweight daemon, `greggd`, runs on designated Linux, macOS, or Windows systems and exposes one small read-only JSON API. The `gregg` client polls configured daemons and renders each reachable system in five normal-view rows, with unreachable systems collapsed to one row and moved to the bottom of the view.

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
| Windows | x86-64 | Client supported; daemon foreground + service |

## Goals

- Keep the daemon suitable for servers, workstations, and resource-constrained single-board computers.
- Support Linux and macOS daemons in version 1, including x86-64, ARM64 Linux, Intel Macs, and Apple Silicon Macs.
- Provide foreground and native service support on Windows x86-64.
- Keep the TUI useful in a small terminal-multiplexer pane.
- Separate collection, protocol, polling/state management, and rendering so each can be tested independently.
- Prefer stable, read-only, local-network operation over broad monitoring-platform functionality.

## Workspace

The workspace contains three independently publishable crates:

| Crate | Binary/library | Responsibility |
| --- | --- | --- |
| `gregg-protocol` | library | Versioned JSON wire types, metric capabilities, endpoint identity, and compatibility rules. |
| `greggd` | `greggd` binary | Native Linux/macOS/Windows metrics collection, periodic sampling, cached immutable snapshots, read-only HTTP API, graceful shutdown, configuration management, and native service integration. |
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

`greggd run` is the foreground process used by systemd, launchd, or interactive diagnostics. It samples metrics on a configurable interval and serves a cached immutable snapshot over HTTP/1. The daemon does not self-daemonize or maintain PID files.

On Windows, the service entry point is `greggd service` (used internally by the SCM; not intended for interactive use).

### Configuration

The `--config` flag overrides the platform default configuration path:

- Linux: `/etc/gregg/greggd.toml`
- macOS: `/Library/Application Support/gregg/greggd.toml`
- Windows: `%ProgramData%\gregg\greggd.toml`

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

Windows (PowerShell, run as Administrator):

```powershell
# Build and install
cargo build --release -p greggd
.\packaging\install-windows.ps1 -SourcePath .\target\release\greggd.exe

# Manage
greggd start
greggd stop
greggd restart
Get-Service greggd
```

See `packaging/README.md` for detailed Windows installation instructions.

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
gregg eggpool add eggpool.local --name "Main EggPool" --api-key-env EGGPOOL_GREGG_API_KEY
gregg eggpool list              # use --json for a JSON array
gregg eggpool remove eggpool.local
```

The optional EggPool pane is a compact view of one configured EggPool's
accounted tokens, provider cache-read share, output-token throughput, and
average time to first token. It is client-only: Gregg supports one source and
does not change EggPool or `greggd`. EggPool's dashboard/statistics routes must
be enabled for `/api/stats/summary` to exist. Public installations need no key;
protected installations use a request-local Bearer key from the environment
variable named by `--api-key-env`:

```sh
export EGGPOOL_GREGG_API_KEY='set-this-in-your-environment'
gregg eggpool add eggpool.local --api-key-env EGGPOOL_GREGG_API_KEY
```

EggPool configuration defaults to HTTP port `11300`; `--https` selects HTTPS.
The add/list/remove commands store and display only the configured
environment-variable name, never its resolved secret, and do not check
connectivity. When `[eggpool]` is configured, the client consumes only
`GET /api/stats/summary` for the fixed `1h`, `24h`, `7d`, and `30d` periods.
Public installations send no authentication header; protected installations
send a request-local `Authorization: Bearer` header resolved from the named
environment variable. Missing or empty variables fail locally without a
request. Response bodies and summary values are bounded and semantically
validated. Omitting `[eggpool]` creates no EggPool worker or request.

### TUI navigation

- `j` / Down: move down (select a system, or choose a longer EggPool window)
- `k` / Up: move up (select a system, or choose a shorter EggPool window)
- `h` / Left and `l` / Right: cycle configured top-level panes
- `v`: toggle Normal/Condensed system layout
- `e`: expand or collapse drives for the selected system
- Viewport scrolls by system entry, not by raw row

On EggPool, `j`/Down selects the next window and `k`/Up the previous one:
`1h`, `24h`, `7d`, and `30d`. Pane, layout, period, and drive-expansion state
are transient. `Ctrl-R` refreshes only the visible pane; EggPool refreshes are
active-only and use a fixed 60-second cadence.

The normal view shows detailed five-row system blocks. The condensed view shows
one comparison row per system with host, CPU, memory, disk, load, and I/O-wait
columns; narrower terminals drop lower-priority columns without horizontal
scrolling. Pane, layout, period, and drive expansion state are transient.
EggPool shows exactly four summary values and keeps a same-period prior result
visible when a later refresh fails.

### Windows client

The `gregg` client is a native Windows application. It stores configuration in `%APPDATA%\gregg\gregg.toml` and uses Windows-native file locking for cross-process safety. Editor resolution falls back to `hx`, `code`, or `notepad` when `$VISUAL` and `$EDITOR` are not set.

### Windows daemon

The `greggd` daemon supports Windows x86-64 as both a foreground process and a native Windows service.

**Foreground mode** (`greggd run`): Runs in the current console, listening for Ctrl-C. Useful for development and diagnostics.

**Service mode** (`greggd service`): The SCM entry point. The service runs under `NT AUTHORITY\LocalService` with minimal privileges. Install/uninstall through the provided PowerShell scripts or manually via `sc.exe`. Service-manager logic is covered by native tests; administrator installation depends on local Windows SCM policy.

Windows collection uses native APIs (`GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo`, `GetComputerNameExW`, `RtlGetVersion`) and does not invoke external commands.

On Windows, `/v1/status` and `/` return `503 Service Unavailable` with a v2 health response, because a truthful v1 snapshot cannot be produced (load, swap, and CPU I/O-wait are absent). `/v2/healthz` also reports 503 when the cached v2 snapshot exceeds the configured stale-age policy. Clients should prefer `/v2/status` on Windows.

The daemon does not automatically create firewall rules. LAN exposure is operator-controlled and the daemon has no TLS or authentication.

## Display model

A reachable system consumes five base rows in the normal view:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
CPU  [||||||||||||                                  ] 25.2%
MEM  [||||||||||||||||||                            ] 37.8%  5.9/15.6 GiB
SWAP [                                                ]  0.0%  0/4.0 GiB
DISK [||||||||||||                                  ] 25.0% 238.0 GiB used / 714.0 GiB avail
```

Drive data that is unavailable or successfully empty is shown as `DISK —`,
not as measured zero. The selected system can expose one plain text detail row
per valid mounted volume when drive expansion is active; those rows show
`used / total` and a per-volume percentage. Offline and pending systems remain
one row.

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
GET /v2/status
GET /healthz
GET /v2/healthz
```

The daemon serves cached immutable snapshots. Requests do not trigger metric collection. The schema carries an explicit version and metric-capability flags so unsupported platform metrics remain distinguishable from measured zero values. V2 status may additionally contain a bounded `drives` list with a display `name` and numeric `used_bytes` and `total_bytes` fields. Missing or `null` means unavailable/legacy; an empty list means enumeration succeeded but found no eligible volumes. The client derives aggregate capacity values. Drive entries are best-effort native observations of eligible mounted local filesystems, not physical-disk telemetry.

The client requests v2 first and accepts only a v2 payload from `/v2/status`. It falls back to v1 only when that request returns 404, then accepts only a v1 payload from `/v1/status`; malformed, invalid, or wrong-version 2xx responses are rejected without fallback. Both v1 and v2 snapshots are normalized internally so the TUI renders a consistent view across mixed-version fleets.

## Platform notes

Linux collection uses native kernel interfaces (`/proc/stat`, `/proc/loadavg`, `/proc/meminfo`, `/proc/self/mountinfo`) and `statvfs`. macOS collection uses Mach host statistics, `sysctlbyname`, and native mounted-filesystem enumeration through a contained FFI boundary. Windows collection uses native system APIs (`GetSystemTimes`, `GlobalMemoryStatusEx`, `GetPerformanceInfo`, `GetComputerNameExW`, `RtlGetVersion`, `GetLogicalDriveStringsW`, `GetDriveTypeW`, `GetDiskFreeSpaceExW`). External utilities are diagnostic references, not runtime dependencies.

Service integration is native to each platform (systemd on Linux, launchd on macOS, Windows SCM on Windows).

## Security

The daemon is designed for **private-network** use only. It does not provide TLS, authentication, rate limiting, or public-internet hardening. See [SECURITY.md](SECURITY.md) for details.

## Known limitations

- macOS has no Linux-equivalent aggregate CPU I/O-wait state. It is reported as unsupported (`iowait_pct: null`) rather than fabricated as zero.
- Windows does not report load averages or swap. It reports memory commit charge instead, which is a distinct metric.
- Windows x86-64 supports up to 64 logical processors in a single processor group for aggregate CPU collection. Multi-group topologies are rejected with a clear error.
- Drive capacity is summed from displayed mounted volumes. Bind mounts and repeated filesystem views are deduplicated; network, pseudo, optical, RAM-backed, and unready volumes are omitted. macOS APFS container topology is intentionally not modeled.
- Per-process inspection, historical telemetry, alerting, and web dashboards are explicitly out of scope for version 1.

## Non-goals

`gregg` is not intended to become a replacement for htop, btop, Glances, Netdata, or a general monitoring platform. Version 1 excludes per-process inspection, remote command execution, historical databases, alerting, web dashboards, service discovery, plugins, Prometheus emulation, TLS automation, and public-internet hardening.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

The project is released under the [MIT License](LICENSE). Every published
crate inherits the same license expression from the workspace root.

## Local development

Fast local check (recommended during development):

```text
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows (PowerShell)
```

Release preflight (nonpublishing):

```text
./scripts/check-local.sh --release
.\scripts\check-local.ps1 -Release
```

The default local check runs `cargo fmt`, `cargo clippy`, `cargo test`,
`cargo doc`, and the current host's native collector tests. The `--release`
preflight adds clean-tree and version checks, package-content review, source
installation of `greggd`, an installed-binary v2 loopback smoke, and a single
protocol-only `cargo publish -p gregg-protocol --dry-run --locked`.
Dependent-crate dry-runs remain manual until the protocol version is visible on
crates.io.

Releases are published manually to crates.io and GitHub. CI never publishes.
Maintainer instructions are in [RELEASING.md](RELEASING.md).

The pinned toolchain lives in `rust-toolchain.toml` and tracks the current
stable Rust release. `rust-version` in every member manifest is set from the
workspace `rust-version = "1.75"`.
