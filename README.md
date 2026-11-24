# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

Gregg is a keyboard-driven terminal monitor for CPU, memory, swap, load, disks,
and optional live I/O across multiple machines.

Each monitored machine runs `greggd`, which serves a read-only JSON API. Your
workstation runs `gregg`, the terminal interface, and a per-user client daemon
that polls your configured machines. Multiple TUI windows share that polling.

## Quickstart

### 1. Install `greggd` on each machine to monitor

Linux or macOS, for a system install with automatic startup:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | sudo bash -s -- greggd
```

Windows PowerShell, run as Administrator to register the Windows service:

```powershell
$installer = Join-Path $env:TEMP 'gregg-install.ps1'
Invoke-WebRequest 'https://github.com/eggstack/gregg/releases/latest/download/install.ps1' -OutFile $installer
& $installer -Component Greggd
```

A non-Administrator Windows install is user-local and does not register a
system service. Check the daemon from its host:

```bash
curl -fsS http://127.0.0.1:11310/v2/healthz
```

In Windows PowerShell, run:

```powershell
Invoke-RestMethod http://127.0.0.1:11310/v2/healthz
```

Fresh installs bind to loopback, so remote `gregg` clients cannot reach the
daemon yet. Before adding remote machines, configure a LAN-reachable bind and
restrict it to your client hosts. See [LAN monitoring](docs/daemon.md#lan-monitoring).

### 2. Install `gregg` on your workstation

Linux or macOS:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg
```

If the installer says `~/.local/bin` is not on your current `PATH`, run the
printed `export` command in that shell. The installer can also add the path to
supported shell profiles for future shells.

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP 'gregg-install.ps1'
Invoke-WebRequest 'https://github.com/eggstack/gregg/releases/latest/download/install.ps1' -OutFile $installer
& $installer -Component Gregg
```

Prebuilt installs need no compiler. Source installs require Rust 1.89 or newer:

```bash
cargo install gregg --locked
cargo install greggd --locked
```

See [Installation](docs/installation.md) for platform details, version pinning,
direct downloads, and uninstall instructions.

### 3. Add monitored machines and open the TUI

```bash
gregg add 192.168.1.10:11310
gregg add deadpool@192.168.1.11:11310
gregg
```

`gregg add` requires an explicit port and accepts `host:port`, `[ipv6]:port`,
`nickname@host:port`, or `http://host:port/`. HTTPS is not supported.

## Navigation

- `j` / `k` or arrow keys: select a machine
- `h` / `l`: switch panes
- `v`: toggle normal and condensed layouts
- `d`: expand drive details
- `n`: expand network details
- `c`: expand scheduled-job details when the daemon supports them
- `Shift-J` / `Shift-K`: select a scheduled job
- `PageDown` / `PageUp`, `f` / `b`, or `Ctrl-D` / `Ctrl-U`: page through the machine list
- `Ctrl-R`: reload the Systems config, or refresh the active pane

See [Display](docs/display.md) for metric rows, compact layout, offline status,
and scheduled-job rendering.

## Useful commands

```text
gregg list                         List configured machines
gregg remove                       Select one machine to remove
gregg remove HOST[:PORT]           Remove by host or exact endpoint
gregg eggpool add HOST             Configure the EggPool endpoint
gregg eggpool list                 List the EggPool endpoint
gregg eggpool remove               Select the EggPool endpoint to remove
gregg eggpool remove HOST[:PORT]   Remove the matching EggPool endpoint
gregg refresh 30                   Set the polling interval in seconds
gregg edit                         Edit the client configuration
gregg update                       Update this client
gregg uninstall --dry-run          Preview removal

greggd status                      Check local daemon health and startup state
greggd job add NAME --schedule '0 2 * * *' -- PROGRAM [ARGS...]
greggd job list                    List jobs in the selected config
greggd job remove [NAME]           Remove by name or select interactively
greggd rehash                      Apply job-only changes to a running daemon
greggd edit                        Edit the complete daemon config
greggd startup install             Register Unix startup, report Windows service state
greggd restart                     Restart the daemon
greggd update                      Update the daemon
greggd uninstall --dry-run         Preview removal
```

The client daemon starts automatically when you run `gregg`. To keep it running
when no TUI is open, use `gregg daemon startup install`. Client startup is
always per-user. The monitored `greggd` daemon can be installed as a system
service. See [Client](docs/client.md) and [Daemon](docs/daemon.md).

`greggd job add` stores the executable and each argument as separate argv values after `--`; it does not invoke a shell. `job remove` without a name uses an interactive terminal selector. These commands and `greggd edit` honor `--config` and validate and atomically update that file, but never reload or restart a running daemon implicitly. Run `greggd [--config PATH] rehash` to apply a valid jobs-only change; mixed changes are rejected atomically. Changes to `name`, `host`, `port`, sampling/staleness settings, `allow_privileged_jobs`, or `scheduler_history_limit` require a restart. If the daemon does not rehash, file edits take effect on its next start. Rehash rechecks root-job authority; adding a job never enables root execution, and Unix root still requires `allow_privileged_jobs = true` in the config.

## Optional features

The TUI can show best-effort CPU frequency, disk throughput, and network
throughput when the host and daemon provide them. Missing telemetry is left
unavailable rather than shown as zero. Older daemons remain usable.

Optional `greggd` scheduled jobs run local maintenance commands from cron
schedules. The client can display their state and recent output. Scheduler
output is available to any client that can reach the unauthenticated daemon
listener, so keep that listener on a trusted network. See the [scheduled-job
configuration and TOML example](docs/daemon.md#scheduled-maintenance).

The optional EggPool pane displays periodized usage metrics and separate
service-health information. See the
[EggPool setup and TOML example](docs/client.md#eggpool).

## Supported release targets

| Platform | Architecture | Release asset target |
| --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64 | `aarch64-unknown-linux-gnu` |
| macOS | Intel | `x86_64-apple-darwin` |
| macOS | Apple Silicon | `aarch64-apple-darwin` |
| Windows | x86-64 | `x86_64-pc-windows-msvc.exe` |

Linux release binaries target glibc 2.17. macOS binaries are unsigned and may
need Gatekeeper approval. Linux ARMv7 and Windows ARM64 use the Cargo fallback
when Rust is available.

## Documentation

- [Installation](docs/installation.md): install, update, and uninstall
- [Client](docs/client.md): endpoints, client daemon, EggPool, and scheduler views
- [Daemon](docs/daemon.md): configuration, startup, scheduled jobs, and security
- [Display](docs/display.md): TUI behavior and layout
- [HTTP API](docs/api.md): daemon endpoints and telemetry
- [Development](docs/development.md): local builds and packaging helpers

## Security

The daemon is intended for private networks. It has no TLS or authentication and
is not hardened for public Internet exposure. It binds loopback (`127.0.0.1`) by
default; binding a wildcard or LAN address is an explicit configuration choice
that logs a startup warning. See [Security](SECURITY.md).

## Contributing and license

See [CONTRIBUTING.md](CONTRIBUTING.md). Gregg is licensed under the [MIT
License](LICENSE).
