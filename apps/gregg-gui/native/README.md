# Native exercises for the GUI

These are the helpers the `gui` CI job runs against the **built** GUI. They
are not part of the GUI's own Cargo workspace: the GUI is a separate workspace
with its own lockfile, and everything here is test scaffolding rather than
product code.

| Path | What it is |
| --- | --- |
| `sni-host/` | A Rust helper crate that acts as a real **StatusNotifierItem host** on a D-Bus session bus, so the GUI's Linux system tray can be exercised for real. It is its own Cargo workspace (`[workspace]` in its `Cargo.toml`), so it never becomes a member of `apps/gregg-gui`. |
| `linux-tray.sh` | The Linux driver: starts the repository's fleet fixture, writes a Gregg configuration that points at it, and runs `sni-host` twice — once with a tray host present and once without. |
| `windows-tray.ps1` | The Windows driver: starts the GUI, waits for its `tray=` startup line, proves a **native top-level window** was created for that process, and checks that the GUI and its client daemon are still running. |

## Why a real tray host is needed

Slint 1.18 implements the Linux system tray with the `ksni` crate, and `ksni`
is a StatusNotifierItem **client**. A client needs a StatusNotifierItem
**host** — on a real desktop that is the panel. A headless CI runner has none,
so without a host Slint silently has no tray at all, and a green build would
say nothing about the tray.

`apps/gregg-gui/native/sni-host` is that host. It owns
`org.kde.StatusNotifierWatcher` (the GUI's own Linux probe asks the session bus
whether that name has an owner) and serves the
`org.kde.StatusNotifierWatcher` interface at `/StatusNotifierWatcher`.

**The order matters.** `ksni` registers once when the tray is created and
gives up if no watcher answers then, so the host has to exist *before* the GUI
starts. Each of the two passes therefore runs under its own `dbus-run-session`,
so the second pass is guaranteed to find an unowned watcher name.

## What the Linux exercise asserts

With a tray host present:

- the GUI prints `gregg-gui: tray=available`;
- the GUI registers a StatusNotifierItem, and the watcher's own
  `RegisteredStatusNotifierItems` property lists it;
- the item's `ToolTip` carries the live fleet status (not the placeholder), and
  a tray menu label names the configured host;
- the item has an icon and a menu;
- the GUI is still running.

With **no** tray host present:

- the GUI prints `gregg-gui: tray=unavailable`;
- the GUI is still running three seconds later: a desktop with no tray is a
  degraded desktop, not a dead one.

## What the Windows exercise asserts

- the GUI starts and prints its `tray=` startup line;
- the GUI created a **native top-level window**, proved by enumerating the
  process's windows with `EnumWindows` and matching the window's process id;
- the GUI process is still alive afterwards;
- the per-config client daemon the GUI launched is running, identified through
  the daemon's own local protocol handshake (`gregg daemon status`), not
  through a process name.

## What the Windows exercise does NOT assert

It does **not** assert that a notification-area (system tray) icon was
rendered. The GitHub-hosted Windows runner executes the agent as a service in
session 0: there is no interactive desktop and no `explorer.exe` shell, so
there is no notification area to put an icon in, and nothing there can be
observed. The script reports which `tray=` value the GUI printed and asserts
only that the tray start did not take the process down. A green Windows run is
evidence that the tray start path runs and that a native window was really
created — it is **not** evidence that the icon appears in a real desktop's
notification area.

## Running them by hand

```bash
# Build what they exercise.
cargo build --manifest-path apps/gregg-gui/Cargo.toml --release
cargo build --release -p gregg
cargo build --release --manifest-path apps/gregg-gui/native/sni-host/Cargo.toml

# Linux: a display and a session bus are both required.
xvfb-run -a bash apps/gregg-gui/native/linux-tray.sh \
  --gui "$PWD/apps/gregg-gui/target/release/gregg-gui" \
  --gregg "$PWD/target/release/gregg" \
  --helper "$PWD/apps/gregg-gui/native/sni-host/target/release/sni-host"

# Windows:
pwsh -File apps/gregg-gui/native/windows-tray.ps1 \
  -GuiPath "$PWD/apps/gregg-gui/target/release/gregg-gui.exe" \
  -GreggPath "$PWD/target/release/gregg.exe"
```

Both drivers fail loudly rather than skipping: a missing display, a missing
session bus, a missing fixture, or a missing fleet port is an error, not a
quiet pass.
