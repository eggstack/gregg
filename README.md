# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

A compact terminal monitor for observing CPU, memory, swap, load, and disk usage across multiple machines over LAN.

A lightweight daemon (`greggd`) runs on each machine you want to monitor and exposes a read-only JSON API on port `11310`. The `gregg` client polls configured daemons and renders a live TUI.

## Supported targets

| Platform | Architecture | Status |
| --- | --- | --- |
| Linux | x86-64, ARM64 | Supported |
| macOS | Intel (x86-64), Apple Silicon (arm64) | Supported |
| Windows | x86-64 | Supported |

## Quick start

### 1. Install the daemon on each machine

**Linux (direct foreground or optional systemd):**

```bash
cargo build --release -p greggd
sudo ./packaging/install-linux.sh target/release/greggd
greggd run
# Optional operator-managed service: sudo systemctl enable --now greggd
```

**macOS (launchd):**

```bash
cargo build --release -p greggd
sudo ./packaging/install-macos.sh target/release/greggd
# Optional operator-managed service: sudo launchctl bootstrap system /Library/LaunchDaemons/com.eggstack.greggd.plist
```

**Windows (PowerShell, as Administrator):**

```powershell
cargo build --release -p greggd
.\packaging\install-windows.ps1 -SourcePath .\target\release\greggd.exe
greggd run
```

### 2. Install the client on your workstation

```bash
cargo install gregg
```

### 3. Add endpoints and launch

```bash
gregg add 192.168.1.10:11310
gregg add 192.168.1.11:11310
gregg add deadpool@192.168.1.10:11310     # `nickname@host:port` form
gregg add http://192.168.1.10:11310/      # HTTP URL input; only host and port are persisted
gregg refresh 30
gregg
```

`gregg add` requires an explicit port. Host-only input such as
`gregg add 192.168.1.10` is rejected; the port must be supplied either
as `host:port`, as an HTTP URL (`http://host:port/`), or through
`nickname@host:port`. The retained `default_port` setting is kept for
configuration compatibility but is not used by `gregg add`. Use
`gregg remove HOST` if you want host-only matching for removal.

## Configuration

The daemon configuration file locations:

| Platform | Path |
| --- | --- |
| Linux | `/etc/gregg/greggd.toml` |
| macOS | `/Library/Application Support/gregg/greggd.toml` |
| Windows | `%ProgramData%\gregg\greggd.toml` |

Default config:

```toml
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
```

Edit the config file to change the display name (`name` field).

Change the bind address or port:

```bash
greggd host 127.0.0.1              # restrict to localhost (SSH tunnel only)
greggd port 11311                  # change the listen port
greggd croncheck                   # ensure the daemon is running; start it if not (cron watchdog)
greggd stop                        # stop a running foreground greggd via the local control socket (Unix) or SCM (Windows)
greggd configprint                 # print the configured bind address with wildcards resolved to the local IP, e.g. 192.168.182.143:11310
greggd version                     # print the daemon version
```

`greggd stop` only targets the local `greggd` instance associated with the
same resolved config identity as `greggd run`. On Linux/macOS, existing config
files use their filesystem-canonical path for that identity, so relative,
absolute, and symlink spellings of the same file converge; a missing implicit
default config uses a deterministic lexical absolute path. Stop speaks to a
local Unix-domain control socket owned by the daemon; on Windows it asks the
Service Control Manager. The HTTP API is read-only and has no shutdown
endpoint.

`greggd croncheck` is a watchdog for cron, Task Scheduler, and other
supervisors without built-in readiness monitoring. It opens a bounded TCP
connect to the configured local bind address (with wildcards normalized to
loopback). If a listener accepts the connection, it exits silently with
status `0`. If nothing is listening, it spawns `greggd run` as a detached
child with stdio closed; on Unix the child runs in a new process group so
signals sent to croncheck's group do not reach the daemon. Run it on a
schedule and the daemon is kept running without `systemd`, `launchd`, or
PID-file management.

The client stores its config at:

- Linux/macOS: `~/.config/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

## Client commands

```bash
gregg                          # launch the TUI
gregg add 192.168.1.10:11310   # add an endpoint (explicit port required)
gregg add server.local:11310   # add with custom port
gregg add deadpool@server.local:11310  # nickname@host:port
gregg add http://server.local:11310/   # HTTP URL input; only host and port are persisted
gregg add 192.168.1.10:11310 --name deadpool  # explicit `--name` instead of `@`
gregg list                     # list configured endpoints
gregg remove 192.168.1.10      # host-only remove is still supported
gregg refresh 30               # set polling interval (seconds)
gregg edit                     # open config in $EDITOR
gregg version                  # print the client version
```

### TUI navigation

- `j` / `k` (or arrow keys): move between systems
- `h` / `l`: cycle panes
- `v`: toggle normal/condensed layout
- `e`: expand/collapse drives for the selected system
- `Ctrl-R`: reload the current Systems config, reliably deliver its endpoint replacement, and poll it immediately; on EggPool, refresh that pane

The selected system keeps its logical selection (`e` still toggles its
drive details), but the reverse-video highlight is transient — it
appears when you navigate, and fades after roughly ten seconds of
inactivity so stale reverse-video does not survive a quiet screen.
Leaving the Systems pane or returning to it does not extend or
re-trigger the highlight.

## Display

Reachable systems show five rows (normal view). All four metric rows
share the same fleet-wide `bar_width` so the opening `[` and closing
`]` columns always align across every online system, and the metric
rows are indented by exactly four spaces:

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
    CPU  [||||||||||||                                  ] 25.2% 8 cores
    MEM  [||||||||||||||||||                            ] 37.8% 5.9 GiB / 15.6 GiB
    SWP  [                                                ]  0.0% 0 B / 4.0 GiB
    DISK [||||||||||||                                  ] 25.0% 238.0 GiB / 952.0 GiB
```

The DISK suffix is `<used bytes> / <total bytes>` so the slash
denominator matches the percentage calculation; explicit caller-available
capacity is preserved by the normalized model and surfaced only through
the expanded per-drive rows. On Windows, the third row uses `COMMIT`
(memory commit charge) instead of `SWP`. Unreachable rows render `—`
instead of fabricating a `0.0%`.

When the longest natural metric suffix across the entire online fleet
exceeds one quarter of the terminal width, every normal-view metric row
collapses to bar-only — the bars remain aligned, but the percentage,
core counts, and byte counts disappear until the terminal widens again.
Resizing wider restores them dynamically with no restart.

The header line omits the `IO` token entirely when CPU I/O wait is
unsupported (macOS) or no real value is available, rather than
rendering a placeholder; the remaining fields keep their normal
separators.

Unreachable systems collapse to one row. With a configured nickname:

```text
deadpool@192.168.1.10:11310 offline
```

Without a nickname the host is rendered once:

```text
192.168.1.10:11310 offline
```

Offline endpoints continue to be polled on every configured cadence; they
automatically recover and switch to the normal view as soon as the
daemon becomes reachable again.

Condensed view shows one comparison row per system with CPU, memory, disk, load, and I/O-wait columns.

## API

The daemon serves cached immutable snapshots on port `11310`:

```text
GET /           # root
GET /v1/status  # v1 status (Linux/macOS only; Windows returns 503)
GET /v2/status  # v2 status (all platforms)
GET /healthz    # v1 health
GET /v2/healthz # v2 health
```

Clients request `/v2/status` first and fall back to `/v1/status` only on 404.

## Platform notes

- macOS does not expose an aggregate CPU I/O-wait state; it is reported as `null`, not fabricated as zero.
- Windows does not report load averages or swap; it reports memory commit charge instead.
- Drive capacity is summed from mounted local volumes. Network, pseudo, optical, and RAM-backed volumes are omitted.
- Per-process inspection, historical telemetry, alerting, and web dashboards are out of scope.

## Security

The daemon is designed for **private-network** use only. It has no TLS, authentication, rate limiting, or public-internet hardening. See [SECURITY.md](SECURITY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
