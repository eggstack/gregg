# gregg-gui — the optional Slint desktop frontend

A desktop GUI for Gregg, built on [Slint](https://slint.dev), that attaches to
the same per-config client daemon the TUI attaches to. It is a **second way to
look at the fleet**, not a second poller: both frontends share one poll
budget, one set of reducers and one local protocol.

This crate is **experimental and optional**. Nothing in the root workspace
depends on it, and the terminal client is unaffected by its existence.

## Status

Plans 188, 189 and 191 are implemented. What exists:

- an independent Cargo workspace with its own lockfile;
- a Slint UI definition (`src/ui/main.slint`) with a `Pulldown` and a
  `SystemTrayIcon` as separate top-level components;
- a bridge (`src/bridge.rs`) that owns the client-daemon attachment, reads the
  real local protocol, and publishes the real reducer's output;
- a **pure projection** (`src/model.rs`) from a daemon document to a typed view
  model, with the truthfulness rules (absent is absent, unpolled is not
  offline, selection is keyed by stable id) testable without a desktop session;
- a **native system-tray entry** with a fleet summary, the hosts the menu can
  jump to, Open Gregg, Reload, About Gregg and Quit Gregg;
- **one tray pulldown** (`Pulldown`): a borderless, always-on-top panel that
  opens from the tray menu and carries the fleet summary, the host list, the
  selected host's metrics, drives and interfaces, its scheduler jobs and
  records, and the `EggPool` plane. There is **no separate monitor window**.
  The lists that grow with the fleet — the hosts, the metrics and the
  scheduler jobs — are **scrollable with bounded heights**, so a large fleet
  cannot push the cron and `EggPool` panes, or the buttons, out of the panel;
- **host management** in the panel: add, remove, rename and re-point a host.
  The panel writes the configuration through the **CLI's own library code**
  against the same `ConfigStore`, so the endpoint grammar, the config lock,
  the whole-config validation and the atomic write are the CLI's, unchanged;
- a **placement module** (`src/placement.rs`) that keeps the panel on the
  display it opens on, and remembers where the operator put it;
- a **no-tray fallback**: the GUI asks the desktop whether it has a tray,
  reports the answer in one machine-readable line, and never hides its only
  control surface when the answer is no (see [A desktop with no system
  tray](#a-desktop-with-no-system-tray));
- an entry point (`src/main.rs`) that wires the two together and runs the
  Slint event loop.

Plan 190 owns native cross-platform qualification, the footprint measurement
and the packaging decision. A corrective pass on 2026-10-21 fixed the
Windows build of the `gregg` crate this frontend depends on, implemented the
no-tray fallback the native matrix required, made the lists scroll, and gave
this workspace **its own CI** — see [CI](#ci).

Plan 190's status is **implemented, pending release qualification**: its
native-platform acceptance gate is still unchecked, because the **interactive**
Windows and Linux runs were never performed and the macOS matrix is partial
(Apple Silicon only). The GUI is therefore **not shippable for Windows or
Linux**, and it is **not shipped for any target** — it stays a source build.
The plan's *Release qualification* section lists what is outstanding.

**Plan 191 is implemented.** The product direction is a **single tray
pulldown**: one borderless, always-on-top panel opened from the native tray
menu, carrying all the metrics, cron and `EggPool` surfaces the compact
monitor window carried, with the separate monitor window dropped. The same
plan adds host **add / remove / edit** to that panel, which the GUI could not
do before: it writes the config through the **CLI's own library code** against
the same `ConfigStore`, so no protocol change and no second poller is
involved. Plan 191 also **retains Slint 1.18.1** after reviewing the
alternatives — it is the latest published release and the only Rust option
with a first-class native tray, and the measured footprint is the software
renderer's rather than the window's. See
`plans/191-tray-pulldown-gui-and-host-management.md`.

Plan 191's own acceptance gate is **not** fully closed: the **interactive**
native-platform runs are still outstanding, exactly as they were for Plan 190.
The panel has been built and its logic is covered by tests on all three
targets, but nobody has yet opened it from a real macOS menu bar, a real
Windows notification area or a real GNOME session.

## Running it

```bash
cargo build --manifest-path apps/gregg-gui/Cargo.toml --release

# the GUI attaches to the client daemon for a config
apps/gregg-gui/target/release/gregg-gui --config ~/.config/gregg/gregg.toml

# ...and names the `gregg` client executable it should launch
apps/gregg-gui/target/release/gregg-gui --config ./gregg.toml --gregg ./target/release/gregg
```

| Option | Meaning |
| ------ | ------- |
| `--config <path>` | The configuration file to read. Defaults to the same path the TUI uses (`gregg::config::Config::default_path`). |
| `--gregg <path>` | The `gregg` client executable to launch as the client daemon. See "Finding the client daemon" below. |
| `--panel` | Open the panel at startup instead of waiting for the tray. The tray still works either way. |
| `--help` | Print the usage text. |

### Finding the client daemon

The GUI is not a Gregg client, so it cannot launch the client daemon by using
its own image. `gregg-gui daemon run` exits immediately, and the readiness
wait would then time out and blame the daemon — a misleading diagnostic for a
simple packaging mistake. The GUI therefore resolves the client in this order:

1. **An explicit `--gregg <path>`.** The path must be an existing executable
   file, and it must *identify itself*: the GUI runs `<path> version` with a
   ten-second deadline and requires the answer to be `gregg <version>`. A
   binary that is not a Gregg client is refused **with the reason**, and the
   refusal happens before anything is stopped.
2. **An installer-owned client beside the GUI.** A `gregg` executable sitting
   next to `gregg-gui` is accepted without a probe, because the installer put
   it there.
3. **Nothing else.** If neither applies, the GUI exits with an actionable
   diagnostic naming `--gregg`. It never falls back to launching itself, and
   it never starts polling the fleet directly.

A daemon that is **already serving** is attached to as-is. The client
executable is only needed where a spawn is actually authorized, so a missing
helper can neither stop nor refuse a working daemon.

### Error states

The GUI reports what happened rather than looking healthy:

| State | What the operator sees |
| ----- | ---------------------- |
| Connecting | `connecting to the client daemon…` |
| Attached | `connected to <daemon id>` |
| The daemon could not be reached | The disconnect reason, verbatim, in the window's status line. |
| The daemon speaks another local protocol version | An explicit *incompatible* state — **not** "disconnected", because a version mismatch is not a network problem. |
| The daemon is shutting down | The shutdown reason, and the connection state is not stale-green. |
| A host has never been polled | `pending` — **never** `offline`, and never `0.00`. |
| A host's last poll failed | `offline` plus the reason when the daemon knows it. |
| A metric family the host does not report | The row is absent, or the value is `—`. Never a fabricated zero. |

## The tray

The tray is the always-visible entry. On macOS it is an `NSStatusItem`; on
Windows the Shell notification area; on Linux a `StatusNotifierItem` /
AppIndicator.

- The icon is a **health-coloured dot**: light green when every host answered,
  light amber when a host has not been polled yet, light red when a host's last
  poll failed. The colours are deliberately *light*, because a menu bar is
  dark on macOS and on most Linux desktops, and a dark icon on a dark bar is
  invisible.
- The icon is drawn procedurally. No binary asset is committed, so it cannot
  go stale.
- The menu contains a **bounded** fleet summary (the counts plus at most a
  handful of host names), the **hosts the menu can jump to**, then Open Gregg,
  Reload, About Gregg and Quit Gregg. The full list belongs in the panel, not
  in a native menu.
- On a desktop with **no tray service**, the panel is still shown on launch,
  so the GUI is never an invisible background process with no way to quit.
  See [A desktop with no system
  tray](#a-desktop-with-no-system-tray) below: the GUI asks the desktop
  whether it has a tray, and it does not hide its only panel when it has not.

### A desktop with no system tray

On Linux the tray is a `StatusNotifierItem`, and the desktop has to run a
**host** for it — the panel, the GNOME Shell extension, and so on. A desktop
without one has no tray at all, and Slint fails to register **silently**: the
icon simply never appears. That is a trap, because the monitor window's close
button *hides* the window, so on such a desktop closing it would leave an
invisible, unquittable GUI with the daemon still polling.

So the GUI asks the desktop, and answers in one machine-readable line:

```text
gregg-gui: tray=available
gregg-gui: tray=unavailable reason="no StatusNotifierItem host on this desktop"
```

- **macOS and Windows** always have a tray: their native APIs always work.
- **Linux** asks the session bus whether the
  `org.kde.StatusNotifierWatcher` name has an owner — exactly what Slint's
  own `ksni` backend requires. The probe is **bounded** (one second), and "the
  bus did not answer" counts as "there is no tray", because that is the safe
  answer: the operator keeps a window that quits instead of a window that
  vanishes.

With no tray, the panel says so in a line of its own, and **closing the panel
ends the GUI** instead of hiding it. With a tray, closing hides the panel and
the tray brings it back, exactly as before.

The decision is taken when the panel is closed, not at startup: a tray host
can start after the desktop session is up, and Slint retries the registration
whenever the icon is set again. The panel's warning line is refreshed when
the panel comes back, so it cannot keep claiming there is no tray after one
appears.

The classification, the close decision and the status line are **pure
functions** in `src/platform.rs`, so both branches are unit-tested without a
desktop session — 9 tests cover the three probe outcomes, the two close
outcomes, and the exact shape of the line a CI job asserts on.

### Actions

| Where | Action | Effect |
| ----- | ------ | ------ |
| Tray | A host entry | Opens the panel with that host selected. |
| Tray | Open Gregg | Opens the panel, or closes it if it is already open. |
| Tray | Reload | The `Ctrl-R` boundary: the daemon re-reads its configuration and polls immediately. |
| Tray | About Gregg | Opens the panel and shows the licence attribution in it. |
| Tray | Quit Gregg | Ends the GUI. The client daemon keeps running. |
| Panel | Click a host row | Selects that host. The selection is keyed by the stable system id, so a host that reorders does not move the selection. |
| Panel | Click a scheduler job | Opens that job's retained records. Only an open job asks the daemon for history; closing the view releases the intent. |
| Panel | Reload / Add host / EggPool / About / Quit buttons | The same actions as the tray items. The `EggPool` button opens and closes the `EggPool` plane, and the intent is sent only while it is open. |
| Panel | A host row's **Edit** button | Opens the form that renames the host, or that re-points it at another endpoint. |
| Panel | A host row's **Remove** button | Removes the host from the configuration. |
| Panel | **Add host** | Opens the form that adds a host. |
| Panel | **Close** (in the header strip) | **Hides** the panel, and the tray brings it back. On a desktop with **no tray** it **ends the GUI** instead — a hidden panel with no tray is an application you can no longer reach. |
| Panel | **Esc** | The same as Close. |
| Panel | The header strip | **Drags** the panel. The panel has no window frame, so it has no title bar to drag. |
| Panel | The grip in the bottom-right corner | **Resizes** the panel. |
| Panel | Quit button | Ends the GUI. The client daemon keeps running. |

Every action is reachable by pointer, and the panel never becomes the only way
to quit.

### How the panel is dismissed

**`Esc`, the tray's Open Gregg entry, and the panel's own Close control.** That
is the whole list.

There is **no click-away dismissal**, and that is a real limitation rather than
an oversight. Slint 1.18 exposes no "the window lost focus" event: a window
cannot be told that the click landed somewhere else, so a panel cannot close
itself when the operator clicks the desktop behind it. The panel is also
`always-on-top`, so it stays over whatever is behind it.

The consequences are worth stating plainly:

- a panel left open stays open until it is dismissed deliberately;
- `Esc` works, because the panel's own focus scope handles the key, and the
  key reaches it even when a text field has the focus.

On a desktop with **no tray**, `Esc` and Close end the GUI instead, because a
hidden panel with no tray is an application the operator can no longer reach.

### Host management

The panel can **add**, **remove**, **rename** and **re-point** the hosts in the
configuration file.

It writes that file through the **CLI's own library code** — `gregg::cli::dispatch`
against the same `ConfigStore` the daemon reads — so the panel inherits the
CLI's exact behaviour rather than a second copy of its rules:

| Rule | Where it comes from |
| ---- | ------------------- |
| An explicit port is required; a portless host is refused | `gregg add`'s endpoint grammar |
| HTTPS is never accepted or downgraded | the same grammar |
| A nickname may not be combined with an explicit name | the same grammar |
| The configuration is validated as a whole before it is written | `ConfigStore::mutate_with_result` |
| The write is atomic — temp file, flush, rename | the same |
| Two writers cannot interleave | the store's lock, the same one the CLI takes |

A **refused** mutation leaves the configuration **byte-identical** and says why
in the panel. A successful one asks the daemon to re-read its configuration at
its own reload boundary, and the daemon republishes.

**Changing a host's endpoint is a replacement, not an edit.** The endpoint is
part of a host's identity, so the host gets a new stable id and its selection
and history do not follow it. The panel says so before it applies one, and the
confirming button is labelled **Replace endpoint** rather than Save, so the
replacement is the operator's explicit choice. The new endpoint is added
**before** the old one is removed, so a refusal — a malformed endpoint, or one
that is already configured — happens before anything is removed.

**Renaming is in place.** The same endpoint with a different name keeps the
host's identity, because the endpoint — the thing the identity is made of —
did not move. An empty name field clears the configured name.

The panel is a **second writer of the configuration file**, never a second
writer of fleet state. The client daemon remains the only poller and the only
owner of the fleet's state.

### Application activation

On macOS the application **activates itself** before it raises the monitor
window. This is not decoration: a tray application is a *background*
application, macOS does not activate the app that owns an `NSStatusItem` menu,
and a background app cannot bring its own window to the front. Without the
activation, "Open Monitor" from the tray does nothing at all when the GUI was
launched the way a real one is launched — a login item, a launcher, an agent.

Slint 1.18 exposes no activation API, so `src/platform.rs` calls
`NSApplication activateIgnoringOtherApps:` through the `objc2`/`objc2-app-kit`
bindings. Those crates are **macOS-only** and never enter the Linux or Windows
builds, and only the `NSApplication` feature is enabled, so the whole of
AppKit is not linked. On Windows and Linux the function is a no-op: there,
showing a window the process owns is enough to bring it up.

### Redraws

There is **no polling timer**. The bridge wakes the UI only when it actually
published something, and the update is scheduled on the UI thread with
`slint::invoke_from_event_loop`. An idle GUI redraws for no reason at all, and
a redraw only ever happens because something changed.

## Build

```bash
# from the repository root
cargo build --manifest-path apps/gregg-gui/Cargo.toml
cargo build --manifest-path apps/gregg-gui/Cargo.toml --release
cargo test  --manifest-path apps/gregg-gui/Cargo.toml

# or from this directory
cd apps/gregg-gui
cargo build --release
```

### Toolchain

**Rust >= 1.92** is required, because Slint 1.18.1 declares
`rust-version = "1.92"`. This is higher than the root workspace's MSRV of
1.89, which is exactly why this crate is its own workspace (see below).

`rust-toolchain.toml` in this directory pins the `stable` channel, which is
1.99.1 today. `rustup` resolves a toolchain file from the *current working
directory*, so building from inside `apps/gregg-gui/` uses this file and
building from the repository root uses the root's `rust-toolchain.toml` —
both pin `stable`, so both resolve to the same toolchain.

The Slint compiler ships inside the `slint-build` crate, so **no extra
system tooling is needed**: no cmake, no clang, no C++ toolchain, no
headless display. The software renderer is pure Rust.

### Independent workspace

This crate is its own workspace root:

```toml
[workspace]
resolver = "2"
```

The root `Cargo.toml` lists `apps/gregg-gui` under `exclude`, so the root's
`cargo test --workspace` and its MSRV CI job never see Slint's toolchain
floor. This directory has **its own `Cargo.lock`** (`apps/gregg-gui/Cargo.lock`),
which is not the root workspace's lockfile and is not affected by it.

## CI

Because this workspace is separate, **no root job builds or tests it** — so it
has a job of its own. The `gui` job in `.github/workflows/ci.yml` runs on
**macOS, Windows and Linux**, natively, and:

- checks the format, the lints (`-D warnings`), the tests and a release build
  of this workspace;
- on **Linux**, runs a native tray exercise against a **real
  `StatusNotifierItem` host**: `apps/gregg-gui/native/sni-host` owns
  `org.kde.StatusNotifierWatcher`, starts the GUI, and asserts that the GUI
  registered an item whose tooltip and menu carry the live fleet state — and
  then, in a second pass on a bus with **no** host, asserts that the GUI
  reports `tray=unavailable` and keeps running;
- on **Windows**, proves the **core** client-daemon launch path first, with
  the CLI and no GUI involved: `gregg daemon restart` takes the launch lock and
  spawns a daemon, `gregg daemon status` identifies it through the daemon's own
  handshake, and `gregg daemon stop` releases it. That is the path the terminal
  client takes, and it is checked on its own so the GUI is not what proves it.
  Then the exercise starts the GUI and proves it created a **native top-level
  window** for its process, and that the client daemon it launched is running
  and really polling the remote.

`scripts/check-gui.sh` runs the same format, lint, test and build checks on a
developer machine, and prints the release binary's size.

The Windows exercise does **not** claim that the notification-area icon was
*rendered*: the CI runner is session 0 with no `explorer.exe`, so there is no
notification area to observe. What it proves is that the native window and the
native tray path start without error on a native Windows host. The
**interactive** desktop runs — a visible icon in a real notification area, a
real GNOME session — are still outstanding, and are still not claimed.

## Dependencies

### Direct dependencies

```
gregg-gui v0.1.0 (/Users/davidbowman/projects/gregg/apps/gregg-gui)
├── gregg v1.0.15 (/Users/davidbowman/projects/gregg/crates/gregg)
├── slint v1.18.1
└── tokio v1.54.0
[build-dependencies]
└── slint-build v1.18.1
```

- **`gregg`** — the existing core library, by path. The GUI renders the same
  reducer output the TUI renders. Pulling it in drags Ratatui and Crossterm
  in transitively; that is expected and accepted for this plan (the point of
  Part B is to *measure* that cost, not to speculate about it). See
  "Measured cost" below.
- **`slint`** — the UI toolkit.
- **`tokio`** — the client-daemon attachment is asynchronous
  (`ensure_running` and `FrameStream::next` are `async`), and it runs on a
  background thread beside Slint's main-thread event loop.
- **`slint-build`** — compiles `src/ui/main.slint` at build time.
- **`objc2` + `objc2-app-kit`** — **macOS only**, and only for the `NSApplication`
  feature. They exist so the GUI can activate itself when it raises the
  monitor window; see "Application activation" above. They never enter the
  Linux or Windows builds.

### Locked Slint versions

From `apps/gregg-gui/Cargo.lock`:

| Crate         | Version |
| ------------- | ------- |
| `slint`       | 1.18.1  |
| `slint-build` | 1.18.1  |
| `i-slint-compiler` | 1.18.1 |

Both are pinned with `=1.18.1` in `Cargo.toml`. Slint's minor releases carry
language and feature changes, and this workspace's lockfile is maintained
separately from the root workspace's, so an exact pin is what keeps the
lockfile the thing that decides.

### Slint feature list

```toml
slint = { version = "=1.18.1", default-features = false, features = [
    "std",
    "backend-winit",
    "renderer-software",
    "software-renderer-systemfonts",
    "system-tray",
    "compat-1-18",
    "unstable-winit-030",
] }
```

`default-features = false` is load-bearing: Slint's defaults pull in the
femtovg (OpenGL) renderer and its GPU stack.

| Feature | Why it is needed |
| ------- | ---------------- |
| `std` | The standard-library backend. Slint's `no_std` mode is not usable with the `winit` backend or the system tray. |
| `backend-winit` | The windowing backend that draws the monitor window on Linux, macOS and Windows. |
| `renderer-software` | The CPU renderer. It needs no GPU, no OpenGL/Metal shader toolchain and no GPU driver, which keeps the build free of graphics system dependencies and the binary portable. |
| `software-renderer-systemfonts` | System font discovery for the software renderer. Without it the software renderer has no font to draw text with, and every label renders blank. |
| `system-tray` | The native tray entry (`NSStatusItem` on macOS, the notification area on Windows, `StatusNotifierItem` on Linux). Required by the `SystemTrayIcon` component. |
| `compat-1-18` | Enables the Slint 1.18 language and version features, so the `.slint` file compiles with 1.18 semantics. The older `compat-1-0`/`compat-1-2` features are not what is wanted: they chain to 1.18 but also imply the software renderer as the default. |
| `unstable-winit-030` | The `slint::winit_030` module, which is the **only** way to ask the display for its own bounds. The panel has to know where the screen ends so it can never open off it, and Slint's stable API exposes no screen information at all. The winit backend is already in this build through `backend-winit`, so the feature adds **no new crate and no new code to the binary** — it only exposes the module that re-exports what is already there. "Unstable" means Slint may move the module in a future minor release; this crate pins Slint **exactly** (`=1.18.1`), so that cannot happen underneath us: a move would be a deliberate version bump and a compile error, not a silent change. |

Deliberately **not** enabled: `renderer-femtovg`, `renderer-skia`,
`renderer-wgpu`, `backend-qt`, `accessibility`.

### Resolved dependency tree

The full tree is in [`dependency-tree.txt`](dependency-tree.txt), generated
with:

```bash
cargo tree --manifest-path apps/gregg-gui/Cargo.toml --prefix none
```

| Measure | Count |
| ------- | ----- |
| Packages in `Cargo.lock` | 604 |
| Distinct package versions in the tree | 352 |
| Unique package names in the tree | 326 |

### Vendored patch: `annotate-snippets` 0.12.9

This workspace carries one vendored crate, `vendor/annotate-snippets`, because
the graph cannot otherwise be resolved: `ratatui` 0.29 (pulled in by the
`gregg` core library) pins `unicode-width =0.2.0` exactly, while Slint's
build-time compiler reaches `annotate-snippets` 0.12.9, which requires
`unicode-width ^0.2.2`. Cargo keeps at most one version of a crate per
semver-compatibility range, so the two requirements cannot be unified.

The vendored copy relaxes that one bound to `unicode-width = "0.2"`. The only
`unicode-width` API the crate uses is
`UnicodeWidthChar::width(ch) -> Option<usize>`, which exists unchanged in
0.2.0, and no source file is modified. It is a build-time-only workaround and
adds no code to the shipped binary. `annotate-snippets` is MIT OR Apache-2.0,
both of which the root workspace's `deny.toml` already allows.

Full rationale, including the alternatives that were rejected:
[`vendor/VENDORED-PATCH.md`](vendor/VENDORED-PATCH.md).

## Packaging and install policy

**Decision (Plan 190): the GUI is source-build-only in this phase. No installer,
no bundle, no release asset.**

That is a decision, not an omission. Shipping an installer or a `.app` bundle
would put an unsigned, un-notarized binary in front of operators, and macOS
Gatekeeper would quarantine it — so the first thing a user would meet would be
a warning dialog, which is a worse first impression than a build command. The
cross-platform evidence a shipping decision needs (a native Windows run, a
Linux desktop session with a `StatusNotifierItem` host) does not exist yet
either. The GUI therefore stays a source build until that evidence does.

### The one operator path

**The GUI requires an existing Gregg installation.** It is not a standalone
product and does not bundle a second `gregg`.

The GUI resolves the client daemon in exactly one order, and the order is the
same on every platform:

1. `--gregg <path>` — an explicit absolute path, which must prove it is a
   Gregg client by answering `gregg <version>` to a bounded `version` probe.
2. An **installer-owned `gregg` beside the GUI**. A `gregg` executable in the
   same directory as `gregg-gui` is accepted without a probe, because whoever
   put it there put it there on purpose.
3. Otherwise the GUI exits with an actionable diagnostic naming `--gregg`.

There is no fourth case. It never launches itself, never does a `PATH`
lookup, and never polls the fleet directly.

### Why there is no second daemon

Packaging may include the existing `gregg` binary — it is the *same* binary,
not a second daemon implementation — but the GUI does not need to: the client
daemon is a **per-config, per-user** process with a `0600` socket beside the
config, and the TUI already starts it on demand. The GUI attaches to that
same daemon, so a user who already uses Gregg gets the GUI for free, with no
second daemon and no second configuration.

### When `gregg` is upgraded separately

The GUI and the daemon negotiate the local protocol version at attach time. An
**incompatible or newer** daemon is reported as *incompatible*, with
guidance, and is **never** downgraded or silently replaced. The direction
rules live in the core launcher and are unchanged by the GUI.

### Release assets

There is no GUI release asset, so there is no asset-naming collision with
`gregg` or `greggd` — the assets that exist are untouched by this line. The
binary name `gregg-gui` is deliberately distinct from both, so a GUI build can
never overwrite a `gregg` or `greggd` installation.

### Licensing

The Slint licence path is `LicenseRef-Slint-Royalty-free-2.0`, **not** the GPL
path, and no GPL dependency is added. The royalty-free licence requires
attribution, and the attribution is **in the application**: the `About` button
and the tray's `About Gregg` item show the Slint name, the licence identifier
and the download page. It is verified present in the shipped release binary,
not only in the source.

## Measured cost


Release builds, `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"` —
the same profile the root workspace uses, so the numbers are comparable with
the TUI's.

The baseline is a **separate throwaway crate** with the same Slint features,
the **same `main.slint` file**, and the same macOS activation code, but with
no dependency on the `gregg` library at all. Building it the same way is what
makes the difference attributable.

| Binary | Size |
| ------ | ---- |
| The GUI before Plan 191 (the separate monitor window) | 10,803,360 bytes (10.3 MiB) |
| **The GUI now** (the tray pulldown, with host management) | **11,896,608 bytes** (11.3 MiB) |
| **Difference** | **+1,093,248 bytes (+10.1%)** |
| The same GUI calling `gregg::cli::dispatch` (the rejected alternative) | 14,761,600 bytes |

So the pulldown and the host-management forms cost about **1.04 MiB** more
than the monitor window they replaced.

**Where that megabyte goes**, from `cargo bloat --release --crates`, which
attributes the linked binary per crate:

| Crate | Before | Now | Change |
| ----- | ------ | --- | ------ |
| `gregg_gui` (the generated Slint code and the GUI's own Rust) | 285.1 KiB | 703.6 KiB | **+418.5 KiB** |
| `std` | 759.8 KiB | 870.7 KiB | +110.9 KiB |
| `toml_edit` | — | 114.7 KiB | +114.7 KiB |
| `i_slint_core` | 452.4 KiB | 527.2 KiB | +74.8 KiB |
| `gregg` | 100.2 KiB | 148.9 KiB | +48.7 KiB |
| `serde_core` | 163.7 KiB | 188.4 KiB | +24.7 KiB |
| the font stack (`harfrust`, `skrifa`, `zeno`, `parley`, `read_fonts`, `fontique`) | 1,356.9 KiB | 1,358.0 KiB | **+1.1 KiB** |
| the SVG stack (`usvg`, `resvg`, `tiny_skia`, `svgtypes`, `roxmltree`, `simplecss`) | 949.4 KiB | 947.3 KiB | **−2.1 KiB** |
| the HTTP and TLS stack (`rustls`, `ring`, `hyper`, `http`, `webpki`, `eggfetch_core`) | — | — | **absent** |

Three things follow, and they are the measurement Plan 191's gate asked for.

1. **The style's own assets did not grow.** The font stack and the SVG stack —
   the bulk of what the `std-widgets` import brings — are unchanged to within
   a kilobyte, because the scroll container had already paid for them. What
   the `Button` and the `LineEdit` add is **418.5 KiB of generated widget
   code**.
2. **Removing `std-widgets` would not be worth it.** The `Button` would have
   to be hand-rolled again, and the `LineEdit` would have to be a text field
   with a caret, a selection and keyboard editing written from scratch. That
   is not a saving of 418 KiB; it is a worse text field for a slightly
   smaller binary. The import is **kept**, with this measurement recorded.
3. **The rejected alternative is measured, not speculated.** Calling
   `gregg::cli::dispatch` from the GUI cost **2,864,992 bytes more**, because
   the dispatcher also reaches the daemon commands, and the daemon reaches
   the HTTP client and its TLS stack. The GUI calls
   `gregg::cli::add_host` and `gregg::cli::remove_host` instead — the same
   code paths `gregg add` and `gregg remove` take — and those crates are
   absent from the binary it builds.

**The earlier baseline, for the record.** Before Plan 191 the comparison was
against a throwaway crate with the same Slint features, the same `main.slint`
and the same activation code but no `gregg` dependency: 9,956,032 bytes
against the GUI's 10,803,360, a difference of 847,328 bytes. That baseline
was itself rebuilt for the 2026-10-21 corrective pass, because the pass gave
the window's lists a scroll container and a scroll container comes from
`std-widgets`: *both* binaries grew by about a megabyte — 1,026,800 and
1,026,816 bytes — and the **difference** stayed where it was, 16 bytes apart
on a ten-megabyte binary. That is what a matched baseline is for: a baseline
that was not rebuilt would have reported the whole growth as the GUI's cost.

(For comparison, Plan 188 measured the same drag against the *scaffold* UI,
before the monitor window existed: 747,936 bytes over an 8,813,296-byte
baseline. The GUI grew on both sides, because the window grew.)

(The `libgregg-*.rlib` in `target/release/deps/` is 18.2 MiB, but that is an
unlinked archive with debug-free but unoptimised object code and heavy
generics; it is not a proxy for what the linker keeps.)

## Measured runtime cost

Plan 190 measured the **monitor window** on macOS (Apple M4 Pro) against a
25-host synthetic fleet, with the measurement window aligned to the daemon's own
start so every configuration is sampled over the same phase of the poll
cadence. CPU is `getrusage`, and "wakeups" is the count of voluntary context
switches — the observable proxy for a sleep/wake cycle.

Those figures are the **monitor window's**. Plan 191 changed the shape, so they
are kept here as the record of that shape and are **not** a claim about the
pulldown.

The pulldown was measured separately, on the same host, against a **one-host**
configuration — the 25-host harness is not in the repository, so it was not
repeated — with the GUI launched against a real client daemon:

| configuration | observed resident memory |
| ------------- | ----------------------- |
| The panel open | **69,792 – 106,496 KB** (68–104 MB) |
| The panel hidden | **66,672 – 78,304 KB** (65–76 MB) |

The spread is wide and the two ranges **overlap**, so the honest reading is
that the pulldown's residency is around **67–104 MB** on this host and that
this measurement cannot cleanly separate the panel from the renderer: the
software renderer and the system-font machinery dominate, and they dominated
Plan 190's 72–83 MB too.

What the samples do say is that the pulldown is **not** lighter in memory
than the monitor window was: the first open-panel run was 106/94/100 MB
against a hidden 78 MB in the same session. One surface instead of two is not
a memory saving. The **request-budget** claim is unaffected — the GUI still
attaches to the same daemon and still adds no request of its own (see
`EVIDENCE.md`) — but the CPU and request figures above are the old shape's
and were not re-measured.

Two things worth reading carefully about the old shape's figures:

- **The daemon's wakeup delta is the documented one.** 8,279 alone → ~11,400
  with a frontend attached is about 44 extra wakeups per second, which is the
  "one timer wake per attached frontend every 20 ms" that `clientd/daemon.rs`
  documents. The GUI adds one more frontend to a cost the architecture
  already accepted; it does not add a new kind of cost.
- **The memory is Slint's, not the protocol's.** The rest is the software
  renderer and the system-font machinery.

The measurements are noisy — RSS at the moment of sampling varied by tens of
megabytes for the same configuration — so the numbers above are ranges, not
constants. See `EVIDENCE.md` for the deterministic request-count proof.

## Licence

Gregg is MIT. This crate is MIT, and it adds:

- **Slint** (`slint`, `slint-build`, `i-slint-*`) — licensed
  `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`.
  **The path chosen here is the royalty-free
  `LicenseRef-Slint-Royalty-free-2.0`**, not the GPL path. Slint's
  royalty-free desktop licence requires attribution: an "About Slint" entry in
  the application, or a badge linking to the Slint download page.

  **That attribution surface exists.** The text lives in the application, not
  in a README nobody reads: `SLINT_ATTRIBUTION` in `src/main.rs` names Slint,
  the licence identifier and the download page, and the window's **About**
  button (and the tray's **About Gregg** item) show it. The GUI must not ship
  without it.
- **`annotate-snippets`** (vendored, build-time only) — MIT OR Apache-2.0.

No GPL dependency is added.

## Layout

```
apps/gregg-gui/
├── Cargo.toml            # its own workspace root + [patch.crates-io]
├── Cargo.lock            # its own lockfile
├── rust-toolchain.toml   # pins `stable` (>= 1.92) for this workspace only
├── build.rs              # slint_build::compile("src/ui/main.slint")
├── dependency-tree.txt   # the full resolved tree
├── src/
│   ├── main.rs           # entry point: the panel, the tray, wiring, the render push
│   ├── bridge.rs         # the client-daemon attachment, the intents, the frame pump
│   ├── model.rs          # the pure FrontendSnapshot -> GuiViewModel projection
│   ├── hosts.rs          # the configuration mutations, behind an injectable seam
│   ├── placement.rs      # where the panel opens, and how far it may be moved
│   ├── platform.rs       # the per-desktop bits: macOS application activation
│   └── ui/
│       └── main.slint    # the Pulldown and the SystemTrayIcon
└── vendor/
    ├── VENDORED-PATCH.md # why the vendored crate exists
    └── annotate-snippets/
```

### The four layers

- **`model.rs` is pure.** One daemon document in, one view model out. No
  threads, no channels, no Slint. Every rule the GUI keeps — absent is absent,
  unpolled is not offline, the selection is keyed by stable id, a missing
  value is `—` and never `0` — lives there, where it can be tested without a
  desktop session.
- **`bridge.rs` owns the protocol.** It attaches to the client daemon, reads
  the frame stream, and publishes plain data. It never touches a Slint
  component, owns no HTTP client, no poll scheduler and no history store. The
  `EggPool` and cron intents are *desired state* carried on a `watch` channel:
  the newest value wins, a rejected write stays dirty and is retried, and a
  reconnect publishes the **new full** desired state rather than a stale
  delta.
- **`main.rs` owns the Slint components.** It maps bridge messages onto the
  connection state, projects the document into the view model, and pushes the
  result into the components — on the UI thread, and only when something
  changed.
- **`hosts.rs` owns the configuration mutations.** It describes what the
  operator asked for and performs it through the CLI's own library code,
  behind a seam a test can replace. It never writes a file by itself.
- **`placement.rs` owns where the panel opens.** The work area, the anchor and
  the clamping are pure functions, so they are tested without a desktop
  session.

## Slint 1.18 notes worth knowing

- `SystemTrayIcon` is a **top-level** component: it is derived with
  `inherits SystemTrayIcon`, never placed inside a `Window`. A tray-rooted
  component does not share `Globals` with a window-rooted one — each instance
  gets its own copy of every global, so shared state has to be pushed into
  each of them explicitly.
- A `SystemTrayIcon` must contain **exactly one `Menu` child**, the menu may
  not be inside an `if` or a `for`, and no other child element types are
  allowed in it.
- A tray-rooted component has **no `run()`** and no window accessor; it is
  shown with `show()`/`hide()` (which set its `visible` property), and it
  keeps the event loop alive on its own.
- A property that is **bound** in the `.slint` file is compiled as a
  constant and gets **no Rust setter**. To set the inherited `icon` or
  `tooltip` from Rust, declare a property of your own and bind the inherited
  one to it (see `TrayIcon.tray-icon` and `TrayIcon.status-text`).
- A `Window` may not set both `width` and `min-width` (or `height` and
  `min-height`): the compiler rejects it.
- There **is** a built-in `Button`, in `std-widgets.slint`, and Plan 191 uses
  it: a popover is operated with the pointer already over it, so a button with
  no hover state and no pressed state is a button the operator cannot feel.
  The hand-rolled `ActionButton` this file used before is gone.
- `LineEdit` comes from `std-widgets.slint` too, and it is what makes the
  host-management forms possible: a single-line text field with a placeholder,
  a caret and keyboard editing is not something to re-implement, and a
  half-working one would be worse than none.
- `std-widgets.slint` **is** the style: importing `Button` from it brings the
  style's own assets with it. That cost is measured below.
- The `Palette` global has to be **imported** from `std-widgets.slint`; it is
  not available unqualified.
- `Image::from_rgba8` takes a `SharedPixelBuffer<Rgba8Pixel>` (not a size and
  a byte vector), and `Timer` is created with `Timer::default()` and started
  with `timer.start(mode, interval, callback)`.
