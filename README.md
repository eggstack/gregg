# gregg

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
gregg add 192.168.1.10
gregg add 192.168.1.11
gregg add myserver.local
gregg add http://192.168.1.10:11310/
gregg refresh 30
gregg
```

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
greggd croncheck                   # bounded /v2/healthz probe; never starts the daemon
greggd configprint                 # print the configured bind address, e.g. 0.0.0.0:11310
greggd version                     # print the daemon version
```

The client stores its config at:

- Linux/macOS: `~/.config/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

## Client commands

```bash
gregg                          # launch the TUI
gregg add 192.168.1.10         # add an endpoint
gregg add server.local:11310   # add with custom port
gregg add http://server.local:11310/ # HTTP URL input; only host and port are persisted
gregg list                     # list configured endpoints
gregg remove 192.168.1.10      # remove an endpoint
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

## Display

Reachable systems show five rows (normal view):

```text
Deadpool · Ubuntu 24.04 x86_64 · Linux 6.8  IO 0.4%  L(8) 1.32/.91/.62
CPU  [||||||||||||                                  ] 25.2%
MEM  [||||||||||||||||||                            ] 37.8%  5.9/15.6 GiB
SWAP [                                                ]  0.0%  0/4.0 GiB
DISK [||||||||||||                                  ] 25.0% 238.0 GiB used / 714.0 GiB avail
```

Unreachable systems collapse to one row:

```text
Deadpool@192.168.1.10:11310 offline
```

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
