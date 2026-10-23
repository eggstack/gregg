# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

A compact terminal monitor for CPU, memory, swap, load, disk, and optional
live throughput across multiple machines over LAN.

A lightweight daemon (`greggd`) runs on each monitored machine and serves a
read-only JSON API on port `11310`. A per-user `gregg` client daemon polls your
fleet once per configuration and feeds the TUI, which only renders.

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
and HTTPS is never accepted. `gregg remove` accepts host-only input, including
a bare IPv6 literal: because `remove` takes a host on its own, the string in
the config always removes it, where `add` still rejects the ambiguous
`2001:db8::1:2` and asks for `[2001:db8::1:2]:11310`. See
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
- `c`: expand/collapse cron details for the selected system
- `Shift-J` / `Shift-K`: move between cron jobs in that block
- `PageDown` / `PageUp` (or `f` / `b`, or `Ctrl-D` / `Ctrl-U`): page forward and
  back through the system list
- `Ctrl-R`: ask the client daemon to re-read the config and poll immediately

## Cron observability

When a monitored `greggd` serves the scheduler routes, `c` expands a read-only
cron block for the selected system: every scheduled job, its state, its last
result, and the run history of one job selected with `Shift-J` / `Shift-K`.

The client daemon reads a compact summary on a 30-second cadence and downloads
the larger history document only when it changes, so opening the pane in ten
windows costs the fleet exactly what zero windows cost. The daemon keeps a
deeper, memory-only cache than the remote retains, so closing and reopening the
TUI does not reset what has been observed; nothing is written to disk.

Reading it honestly: schedules are the remote host's own local civil cron, while
each record's clock is labelled UTC (`10-05 07:00Z`) because the daemon does not
report the remote's timezone — so the two columns are deliberately not in the
same time base. A load gate names the relation it actually means
(`load15m 9.24 > 8.00`, `start load15m 1.20 <= 8.00`, `load15m unavailable`), and
a missing reading is never rendered as zero. `Shift-J` / `Shift-K` keep the
selected job visible however many jobs exist, and anything the terminal is too
short to show is marked as such rather than quietly dropped.

An older `greggd` that does not serve the routes is reported as *unsupported*,
which is the normal case in a mixed fleet and never marks the system offline.
Remote command output is escaped rather than stripped, so a hostile build script
cannot clear your screen or retitle your terminal. See
[Display](docs/display.md) and [Client](docs/client.md).

## The client daemon

`gregg` is a frontend. A separate, same-user process — the *client daemon* —
owns all remote polling for one configuration file and serves TUI windows over a
local endpoint, so a second window costs a socket instead of a second copy of
the fleet's polling, and closing the last window does not stop observation.

```text
gregg                    # the TUI; starts the daemon if it is not already up
gregg daemon run         # run it in the foreground (it never self-daemonizes)
gregg daemon status      # is one running for this config?
gregg daemon stop        # stop it
gregg daemon restart     # stop and start it again, then confirm it is serving
```

Each configuration gets its own daemon on its own `0600` endpoint. The TUI
attaches to it, reads no config file, and opens no network connection; if the
daemon is not there it says so instead of quietly polling behind its back.

You do not normally have to start the daemon yourself — bare `gregg` does it, and
only when the endpoint is genuinely empty, under a lock so two simultaneous
launches still produce one daemon. To keep it running when no TUI is open:

```text
gregg daemon startup install        # user-scoped: systemd --user, LaunchAgent, or Startup folder
gregg daemon startup instructions   # print it without changing anything
gregg daemon startup status         # what is registered
gregg daemon startup remove         # remove only Gregg's own entry
```

That is always a **user** registration. Gregg never installs a system service or
a `LocalService` SCM entry for the client daemon, never runs `sudo` internally,
and a root install registers nothing on anyone's behalf — each user registers
their own, or relies on lazy activation.

Where that registration is a managed user crontab (Linux without user systemd),
Gregg rewrites only its own marked block and nothing else. Because `crontab -`
replaces the whole table, it never rewrites a table it did not read successfully:
if `crontab -l` times out, fails to start, or returns something that is not
readable text, the install stops and tells you to run `gregg daemon startup
instructions` and add the line yourself, rather than risk your other jobs.

`gregg update` prepares and verifies the replacement before it touches a running
daemon, then relaunches it on the new binary and says so if that relaunch
fails. `gregg uninstall` stops an identified daemon and removes only a startup
entry that is provably Gregg's, leaving foreign ones alone; a daemon it cannot
confidently stop blocks the deletion rather than leaving a stale endpoint
behind. See [Client](docs/client.md).

## Optional desktop GUI

`apps/gregg-gui` is an **optional, experimental** Slint frontend — a system
tray entry and a compact monitor window for the same fleet the TUI shows. It
is a second way to *look* at the fleet, never a second way to *poll* it: it
attaches to the same per-config client daemon over the same local protocol, so
a TUI and a GUI window cost the fleet exactly what one window costs.

```bash
cargo build --manifest-path apps/gregg-gui/Cargo.toml --release

apps/gregg-gui/target/release/gregg-gui --config ~/.config/gregg/gregg.toml \
    --gregg ./target/release/gregg
```

The tray is the always-visible entry: a health-coloured icon whose tooltip
carries the fleet's counts — `7 online` for a healthy fleet, and
`7 online, 1 offline` when a host is not answering — and whose **click opens
the pulldown**. The pulldown
is a single borderless, always-on-top panel — there is no separate monitor
window — with **two views**: the **fleet**, which gives every host a **block of
its own** — a header line that names the host and carries the readings that are
not used shares (`IO`, the load, the core count, the frequency), and under it a
**usage bar** for each of the five used shares (`CPU`, `MEM`, `SWP`/`COMMIT`,
`DISK`, `NET`), each bar taking the full width of the panel less its
percentage; and the **detail**, which the pulldown's own footer opens. The
detail is the selected host's **own block, drawn by the same component the
fleet view draws every host with** — so there is exactly one bar per used share
in the panel — and, under it, one condensed list of groups: the numbers behind
the shares, each drive's own facts, and each interface's. The panel also
carries `Reload`, `About` and `Quit`, and can **add, remove, rename and
re-point** the hosts in the config file. Dismissing the panel hides it and
leaves the tray running; quitting the GUI never stops the client daemon.

The GUI is meant to live in the background, so it stays small: the pulldown's
window is **built when it is first opened** and a hidden pulldown is not
re-rendered, which takes the idle resident set from 82 MB to **45.5 MB** on
macOS.

The native tray menu is the **context** menu, not the dropdown — on macOS it is
detached entirely, and elsewhere it carries only `Quit Gregg`. A native menu
cannot host arbitrary Slint content, but that does not mean the content has to
live in a *separately opened* window either: Slint's `SystemTrayIcon` has a
**`clicked`** callback, and a native menu with no entries detaches itself so
the platform routes the tray's click to that callback instead of popping the
menu. So the tray's **click opens the pulldown and the pulldown carries the
content**, with no launcher entry. The pulldown is anchored to the trailing
corner of the display, on the side where the desktop's own shell lives, and its
placement is remembered across restarts — the placement **relative
to that display's own corner**, so it still means the same thing on a display
of another size. Its **size** follows its content, always. It **looks** like a
macOS dropdown too: a rounded card of
translucent material with a hairline edge, a soft shadow and a pointer toward
the tray item — and it **wastes no vertical space**: there is no band of dead
space between the title and the first host, because a Slint layout counts an
empty row as a row. And it **behaves** like a dropdown too: `Esc`, the tray's
click, its own × control **and a click away** dismiss it. The click away is
watched differently on each desktop, because another application's mouse is not
watchable everywhere: on **macOS** the pulldown watches the **mouse**, through
AppKit's global event monitor, so it closes only when a click really lands
elsewhere; on **Windows and Linux** it watches the **keyboard focus**, which is
the desktop saying the window is no longer active.

On a desktop with **no system tray** — a plain GNOME session has no
`StatusNotifierItem` host — the tray cannot appear at all. The GUI notices,
says so in the panel, and then **quits when you close the panel** instead of
hiding it, because a hidden panel with no tray is an application you can no
longer reach.

On macOS the pulldown also **reaches another application's full screen**:
it is a menu-bar-only application, and its window is told to join every space
and to sit at the level a native status item's own menu is drawn at. So the
tray's click opens the pulldown *over* the application you are looking at,
with that application still in full screen behind it, and the GUI has no Dock
icon of its own. The menu bar is still macOS's own business: it is hidden in
full screen, and the pointer at the top of the screen brings it back.

It is its own Cargo workspace with its own lockfile, because Slint 1.18 needs
Rust 1.92 and the five core crates keep their Rust 1.89 MSRV. Nothing in the
core workspace depends on it, it is not published to crates.io, and the
terminal client is unaffected by its existence. The GUI is the only thing that
may start the client daemon, and it does so by naming the `gregg` executable
explicitly rather than launching itself. Being a separate workspace means it
also has **its own CI** — it is formatted, linted, tested and release-built on
macOS, Windows and Linux, and its tray is exercised natively on the Windows and
Linux legs. The Windows leg also proves the **core** client-daemon launch with
the CLI and no GUI involved (`gregg daemon restart` → `status` → `stop`),
because the terminal client takes the same lock the GUI does.

**The GUI is a source build.** There is no installer, no bundle and no release
asset for it yet: it requires an existing Gregg installation, it has been
qualified natively on macOS only, and shipping an unsigned binary would meet
the operator with a Gatekeeper warning before the product. It has been measured
against the fleet: attaching it to a 25-host fleet leaves the daemon's remote
request budget **exactly** what it was with no frontend attached. See
[`apps/gregg-gui/README.md`](apps/gregg-gui/README.md) for the tray, the
helper resolution, the error states, the licence attribution and the packaging
decision.

## Live metrics and compatibility

Beyond the core CPU/memory/load/swap/drive gauges, v2 daemons may report
best-effort live telemetry: current CPU frequency, disk `R/s`/`W/s` byte
rates, and directional network `Rx/s`/`Tx/s` with link capacities. Values are
omitted (never fabricated) when the host API cannot provide them, and older
daemons stay online with their historical metrics intact. See
[Display](docs/display.md) for row and width policies.

`greggd` can also run optional local maintenance commands from five-field cron
schedules. Jobs execute directly as the daemon's OS user, one at a time, and
can defer against cached Unix load averages. See [Daemon](docs/daemon.md) for
the TOML schema, identity restrictions, and deferral behavior.

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
