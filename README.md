# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

A compact terminal monitor for observing CPU, memory, swap, load, and disk usage across multiple machines over LAN.

A lightweight daemon (`greggd`) runs on each machine you want to monitor and exposes a read-only JSON API on port `11310`. The `gregg` client polls configured daemons and renders a live TUI.

## Supported targets

Prebuilt release binaries are published for the common platforms below. The
GitHub release tag (`vX.Y.Z`) already carries the version, so asset names are
stable within a release (e.g., `gregg-x86_64-unknown-linux-gnu`). `SHA-256`
files sit alongside each executable. Linux GNU assets use a conservative
glibc 2.17 floor so they run on long-lived Debian/Ubuntu/Armbian SBC images.

| Platform | Architecture | Rust target | Asset suffix | Status |
| --- | --- | --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` | `x86_64-unknown-linux-gnu` | Supported (glibc 2.17) |
| Linux | ARM64 (64-bit Raspberry Pi / Le Potato / other AArch64 SBCs) | `aarch64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu` | Supported (glibc 2.17) |
| macOS | Intel (x86-64) | `x86_64-apple-darwin` | `x86_64-apple-darwin` | Supported (unsigned) |
| macOS | Apple Silicon (arm64) | `aarch64-apple-darwin` | `aarch64-apple-darwin` | Supported (unsigned) |
| Windows | x86-64 | `x86_64-pc-windows-msvc` | `x86_64-pc-windows-msvc.exe` | Supported |

macOS binaries are unsigned; Gatekeeper may quarantine them until the operator
approves via System Settings or `xattr -d com.apple.quarantine`. Linux ARMv7
(`armv7-unknown-linux-gnueabihf`) is source-build only in this phase and is
not a published prebuilt target — the installer falls back to Cargo when
available.

## Quick start

### 1. Install the daemon on each machine

**Linux / macOS — prebuilt binary (recommended):**

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
# hardened variant:
# curl --proto '=https' --tlsv1.2 -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
```

For a pinned version, pass `--version`:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/download/v1.0.11/install.sh | sudo sh -s -- greggd --version 1.0.11
```

Or download directly without the bootstrap:

```bash
curl -fsSL -o greggd-x86_64-unknown-linux-gnu https://github.com/eggstack/gregg/releases/latest/download/greggd-x86_64-unknown-linux-gnu
curl -fsSL -o greggd-x86_64-unknown-linux-gnu.sha256 https://github.com/eggstack/gregg/releases/latest/download/greggd-x86_64-unknown-linux-gnu.sha256
sha256sum -c greggd-x86_64-unknown-linux-gnu.sha256 && chmod +x greggd-x86_64-unknown-linux-gnu && sudo install -m 755 greggd-x86_64-unknown-linux-gnu /usr/local/bin/greggd
```

**Linux — local build / systemd (developer path):**

```bash
cargo build --release -p greggd
sudo ./packaging/install-linux.sh target/release/greggd
greggd run
# Optional operator-managed service: sudo systemctl enable --now greggd
```

**macOS — local build / launchd (developer path):**

```bash
cargo build --release -p greggd
sudo ./packaging/install-macos.sh target/release/greggd
# Optional operator-managed service: sudo launchctl bootstrap system /Library/LaunchDaemons/com.eggstack.greggd.plist
```

**Windows — prebuilt binary (PowerShell, as Administrator for daemon):**

```powershell
irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
# Or with explicit component:
.\packaging\install.ps1 -Component Greggd
.\packaging\install.ps1 -Component Both -Version 1.0.11
```

**Windows — local build (developer path):**

```powershell
cargo build --release -p greggd
.\packaging\install-windows.ps1 -SourcePath .\target\release\greggd.exe
greggd run
```

The bootstrap installers are binary-first: they detect `uname -s`/`uname -m`,
download the matching `gregg-<target>`/`greggd-<target>` asset and its
`.sha256`, verify checksum and candidate `version`, then install to
`/usr/local/bin` when privileged or `$HOME/.local/bin` otherwise
(`%ProgramFiles%\Gregg` vs `%LOCALAPPDATA%\Gregg` on Windows). When no
matching asset exists (e.g., `armv7l` or an unknown OS/arch), they fall back
to `cargo install --locked` if Cargo is available. No installer silently
invokes `sudo`.

### 2. Install the client on your workstation

**Prebuilt binary (recommended):**

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
```

**From crates.io / source (fallback):**

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

IPv6 link-local zone identifiers are accepted in either `fe80::1%eth0` or
`[fe80::1%25eth0]:11310` form and are stored in URL-safe `%25` form for polling.
Bracketed endpoint syntax is reserved for IPv6 literals; values such as
`[server.local]:11310` are rejected.

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

Edit the config file to change the display name (`name` field). The name must
be non-empty, at most 128 bytes, and contain no control characters.

Change the bind address or port:

```bash
greggd host 127.0.0.1              # restrict to localhost (SSH tunnel only)
greggd port 11311                  # change the listen port
greggd croncheck                   # ensure the daemon is running; starts only when the health endpoint is refused (cron watchdog)
greggd stop                        # stop a running foreground greggd via the local control socket (Unix) or SCM (Windows)
greggd configprint                 # print the configured bind address with wildcards resolved to the local IP, e.g. 192.168.182.143:11310
greggd version                     # print the daemon version
```

Automatic startup, restart, and update:

```bash
greggd startup install                        # auto: systemd on Linux (if running), launchd on macOS, cron elsewhere
greggd startup install --method systemd       # explicit: systemd, launchd, or cron
greggd startup instructions                   # read-only: prints exact commands/paths for the detected method
greggd startup instructions --method cron     # read-only for a specific method
greggd restart                                # manager-aware: systemd / launchd / SCM / direct (stop + detached run)
greggd update                                 # binary-first update to latest stable crates.io version; restarts if running
```

`greggd startup install` is `auto` by default: Windows→SCM, macOS→launchd, Linux with running systemd→systemd, otherwise cron. Standard systemd paths are `/usr/local/bin/greggd`, `/etc/gregg/greggd.toml`, `greggd` user/group, `/etc/systemd/system/greggd.service` (atomic, `daemon-reload` + `enable` + `start`/`restart`); launchd uses `/Library/LaunchDaemons/com.eggstack.greggd.plist`; cron uses an idempotent `# greggd managed watchdog` block with `@reboot` + `* * * * *` `croncheck` (shell-quoted, preserves unrelated crontab, never edits `/var/spool/cron`). An identified systemd/launchd host never silently falls back to cron on permission failure; the exact `sudo <exe> startup install --method <...>` is printed and exit 4 is returned. No internal `sudo`. `startup instructions` never mutates state. `restart` on systemd runs `systemctl restart greggd`; on launchd `launchctl kickstart -k`; on Windows via SCM; otherwise via local `stop` + detached `run`. Privilege failures print the exact elevated `systemctl`/`launchctl` command and return `PermissionDenied` without competing fallback; `restart` is factored for `update` reuse.

`gregg update` and `greggd update` are binary-first: they query the latest stable `gregg`/`greggd` crate on crates.io (authoritative; `max_stable_version`), compare SemVer-safely with `env!("CARGO_PKG_VERSION")`, and if newer, download the exact `vX.Y.Z` GitHub Release asset for the current host (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc[.exe]`) plus its `.sha256`, verify SHA-256 and candidate `version` before any replacement, stage to a private temp dir, then atomically replace the current executable (`self-replace` on Windows for running-image semantics, same-filesystem rename on Unix). Missing exact asset (HTTP 404) permits `cargo install --locked --version "=X.Y.Z"` fallback staged to a temp `--root`; checksum/version mismatch, transport failure, or 5xx never falls back. Permission failures print `sudo <exe> update` and occur before any `greggd` shutdown. Symlinked installs replace the resolved target and preserve the symlink. `greggd update` preserves config and startup registration and restarts only when the daemon was running/managed (systemd active, launchd loaded, SCM running, or direct/cron running); intentionally stopped services remain stopped and a successful replacement with failed restart is reported as `Installed X.Y.Z but not activated` with the exact `greggd restart`/`systemctl`/`launchctl` command and nonzero exit. No background checks, TUI notifications, package-manager integration, or automatic `sudo`.

`greggd stop` only targets the local `greggd` instance associated with the
same resolved config identity as `greggd run`. On Linux/macOS, existing config
files use their filesystem-canonical path for that identity, so relative,
absolute, and symlink spellings of the same file converge; a missing implicit
default config uses a deterministic lexical absolute path. Stop speaks to a
local Unix-domain control socket owned by the daemon; on Windows it asks the
Service Control Manager. A missing or unreachable socket is an idempotent
"not running" success; if the daemon cannot be confirmed stopped (for
example it accepts the stop command but never replies), `greggd stop`
warns and exits nonzero instead of claiming the daemon is not running.
The HTTP API is read-only and has no shutdown
endpoint.

`greggd croncheck` is a watchdog for cron, Task Scheduler, and other
supervisors without built-in readiness monitoring. It sends a bounded raw HTTP
probe to `/v2/healthz` on the configured local bind address, with wildcards
normalized to loopback. A valid Gregg Ready, Warming, or Failed response means
the daemon is running and exits silently with status `0`. A refused connection
proves the endpoint is absent, so it spawns `greggd run` as a detached child
with stdio closed; on Unix the child runs in a new process group. An unrelated,
malformed, silent, or otherwise ambiguous peer returns nonzero and never starts
a second daemon. Run it on a schedule without `systemd`, `launchd`, or PID-file
management.

The client stores its config at:

- Linux: `~/.config/gregg/gregg.toml` (honors `XDG_CONFIG_HOME`)
- macOS: `~/Library/Application Support/gregg/gregg.toml`
- Windows: `%APPDATA%\gregg\gregg.toml`

Client `request_timeout_ms` values must be between 100 and 60,000 milliseconds;
invalid values are rejected before polling starts.

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
gregg update                   # update to latest stable crates.io version (binary-first, Cargo fallback)
gregg eggpool add pool.local:11300
```

Only one EggPool endpoint is supported; use `--replace` to change an existing
one. Without it, the command reports an already-configured endpoint conflict.

### TUI navigation

- `j` / `k` (or arrow keys): move between systems
- `h` / `l`: cycle panes
- `v`: toggle normal/condensed layout
- `e`: expand/collapse drives for the selected system
- `Ctrl-R`: reload the current Systems config, reliably deliver its endpoint replacement, and poll it immediately; on EggPool, refresh that pane

If a Systems config reload is missing, malformed, or invalid, the last-known-good
configuration remains active and the error is shown in the diagnostic line until
a later reload succeeds.
When the bounded scheduler command channel is full, replacement delivery waits
in the event loop's pending-command branch so input and poll results remain
responsive; the replacement is still applied only after delivery.

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

If the system clock moves backward, a snapshot timestamp that is temporarily
in the future is treated as fresh rather than stale; age-based staleness resumes
once the clock catches up.

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
