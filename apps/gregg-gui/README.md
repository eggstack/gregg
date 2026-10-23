# gregg-gui — the optional Slint desktop frontend

A desktop GUI for Gregg, built on [Slint](https://slint.dev), that attaches to
the same per-config client daemon the TUI attaches to. It is a **second way to
look at the fleet**, not a second poller: both frontends share one poll
budget, one set of reducers and one local protocol.

This crate is **experimental and optional**. Nothing in the root workspace
depends on it, and the terminal client is unaffected by its existence.

## Status

Plans 188, 189, 191 and 192 are implemented. What exists:

- an independent Cargo workspace with its own lockfile;
- a Slint UI definition (`src/ui/main.slint`) with a `Pulldown` and a
  `SystemTrayIcon` as separate top-level components;
- a bridge (`src/bridge.rs`) that owns the client-daemon attachment, reads the
  real local protocol, and publishes the real reducer's output;
- a **pure projection** (`src/model.rs`) from a daemon document to a typed view
  model, with the truthfulness rules (absent is absent, unpolled is not
  offline, selection is keyed by stable id) testable without a desktop session;
- a **native system-tray entry** whose **click opens the pulldown**. The
  native menu is the *context* menu — a right-click reaches it — and it
  carries `Quit Gregg` and nothing else. On macOS it is **detached entirely**,
  because AppKit pops an attached menu on the very click that is supposed to
  open the pulldown;
- **one tray pulldown** (`Pulldown`): a borderless, always-on-top, rounded
  card with a soft shadow and a pointer toward the tray item. It has **two
  views**: the **fleet**, which gives every host a **block of its own** — a
  header line that names the host and carries the readings that are not used
  shares, and under it a bar for each of the five used shares — and the
  **detail**, which is the selected host's metrics, drives and interfaces, its
  scheduler jobs and records, and the `EggPool` plane. The pulldown's own footer moves between them, and it also
  carries the fleet summary, the host add/remove/edit forms, and the
  application's own `Reload`, `About` and `Quit`. There is **no separate
  monitor window**. The list that grows with the fleet is **scrollable with a
  bounded height**, so a large fleet cannot push the pulldown off the
  desktop;
- a pulldown that is **sized to its content** and **placed by the platform's
  convention**: it opens in the trailing corner of the work area, and the
  operator's own placement is **remembered across restarts** in a small state
  file beside the configuration;
- a pulldown that **reaches another application's full screen** on macOS: the
  GUI is a **menu-bar-only** application, and its window is told to join every
  space and to sit at the level a native status item's own menu is drawn at,
  so the tray's click opens the pulldown *over* the application the operator
  is looking at, with that application still in full screen behind it (see
  [Another application's full screen](#another-applications-full-screen));
- **host management** in the pulldown: add, remove, rename and re-point a
  host. The pulldown writes the configuration through the **CLI's own library
  code** against the same `ConfigStore`, so the endpoint grammar, the config
  lock, the whole-config validation and the atomic write are the CLI's,
  unchanged;
- a **placement module** (`src/placement.rs`) that keeps the pulldown on the
  display it opens on, remembers where the operator put it, and owns the
  pulldown's measurements;
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
pulldown**: one borderless, always-on-top panel carrying all the metrics,
cron and `EggPool` surfaces the compact monitor window carried, with the
separate monitor window dropped. The same plan adds host **add / remove /
edit** to that panel, which the GUI could not do before: it writes the config
through the **CLI's own library code** against the same `ConfigStore`, so no
protocol change and no second poller is involved. Plan 191 also **retains
Slint 1.18.1** after reviewing the alternatives — it is the latest published
release and the only Rust option with a first-class native tray, and the
measured footprint is the software renderer's rather than the window's. See
`plans/191-tray-pulldown-gui-and-host-management.md`.

**Plan 192 is implemented.** It **reverses the interaction model** Plan 191
built. The tray's native menu was a *launcher*: it offered a summary, a flat
list of host jump entries and `Open Gregg`, and `Open Gregg` opened a
**separate** window that carried the content — so the content was not in the
dropdown, and the window that held it read as a small application rather
than a popover.

A native menu cannot host arbitrary Slint content — that part is true — but
the content does not have to live in a *separately opened* window. Slint's
`SystemTrayIcon` also has a **`clicked`** callback, and a native menu with no
entries **detaches itself**, so the platform routes the tray's click to that
callback instead of popping the menu: on macOS
(`i-slint-core-1.18.1/items/system_tray/appkit.rs:248`), on Windows
(`windows.rs:290-298`, where a right-click still opens the native menu), and on
Linux (`ksni.rs:284`, where the desktop decides the gesture).

So the direction is now: **the tray's click opens the pulldown, and the
pulldown carries the content**. The `Open Gregg` launcher entry and the host
jump entries are gone; on macOS there is no native menu at all, and the
pulldown carries `Reload`, `About` and `Quit`. The pulldown is still a Slint
window, because a native menu cannot host Slint widgets, and it is still
anchored to the platform's trailing corner, because Slint 1.18 does not expose
the tray item's position.

Plan 192 also closed two defects that reproducing the complaint exposed:

- **the panel was never positioned on the startup path.** Slint's
  `with_winit_window` "will only succeed when the event loop is active", and
  the startup path called it before the event loop ran, so `work_area()`
  returned `None`: the panel was not positioned, not clamped to the work area,
  and its size was not clamped either. It landed wherever the window manager
  put it. The first open is now deferred into the event loop, and the
  placement is retried for a bounded number of ticks until the native window
  exists.
- **the position and size were not kept across a restart.** They lived in
  `UiState`, in memory, and the GUI wrote no state file — so the claim that
  the panel "remembers where the operator put it" was true for one session and
  false across a restart. The geometry now lives in `<config>.gui-state`
  beside the configuration, and the read is bounded and validated.

See `plans/192-tray-pulldown-carries-the-panel.md`.

Plan 191's own acceptance gate is **not** fully closed: the **interactive**
native-platform runs are still outstanding, exactly as they were for Plan 190.
The pulldown has been built and its logic is covered by tests on all three
targets, but nobody has yet clicked a real macOS menu bar icon, a real
Windows notification-area icon or a real GNOME session's tray icon.

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
| Attached | **Nothing.** A healthy attachment has nothing to report, and a line that reports nothing is chrome. The daemon's identity is not reported either: it is a hash that means nothing to an operator looking at a pulldown. |
| The daemon could not be reached | The disconnect reason, verbatim, in the pulldown's status line. |
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
- **The click opens the pulldown.** The pulldown is the surface: it carries
  the fleet, the metrics, the cron, the `EggPool` plane, the forms and the
  application's own actions.
- The **hover text** carries a **bounded** fleet summary (the counts plus at
  most a handful of host names), so the tray alone tells the operator what the
  fleet is doing.
- The **native menu** is the *context* menu: a right-click reaches it, and it
  carries `Quit Gregg` and nothing else. On **macOS** it is **detached
  altogether** — AppKit attaches a menu to the status item and pops it on the
  click, which would swallow the click that is supposed to open the
  pulldown — so on macOS the pulldown's own `Quit` is the only way out.
- On a desktop with **no tray service**, the pulldown is still shown on launch,
  so the GUI is never an invisible background process with no way to quit.
  See [A desktop with no system
  tray](#a-desktop-with-no-system-tray) below: the GUI asks the desktop
  whether it has a tray, and it does not hide its only surface when it has not.

### A desktop with no system tray

On Linux the tray is a `StatusNotifierItem`, and the desktop has to run a
**host** for it — the panel, the GNOME Shell extension, and so on. A desktop
without one has no tray at all, and Slint fails to register **silently**: the
icon simply never appears. That is a trap, because the pulldown's close
control *dismisses* the pulldown, so on such a desktop closing it would leave
an invisible, unquittable GUI with the daemon still polling.

So the GUI asks the desktop, and answers in one machine-readable line:

```text
gregg-gui: tray=available
gregg-gui: tray=unavailable reason="no StatusNotifierItem host on this desktop"
```

And when the pulldown has been placed, one line says where it went and where it
was allowed to go:

```text
gregg-gui: pulldown=placed x=3226 y=36 width=606 height=264 area=8,36,3824x1036
```

That is what the native tray exercises assert on: they check the **position**,
not only that a window appeared.

- **macOS and Windows** always have a tray: their native APIs always work.
- **Linux** asks the session bus whether the
  `org.kde.StatusNotifierWatcher` name has an owner — exactly what Slint's
  own `ksni` backend requires. The probe is **bounded** (one second), and "the
  bus did not answer" counts as "there is no tray", because that is the safe
  answer: the operator keeps a window that quits instead of a window that
  vanishes.

With no tray, the pulldown says so in a line of its own, and **closing the
pulldown ends the GUI** instead of hiding it. With a tray, closing hides the
pulldown and the tray's click brings it back, exactly as before.

The decision is taken when the pulldown is closed, not at startup: a tray host
can start after the desktop session is up, and Slint retries the registration
whenever the icon is set again. The pulldown's warning line is refreshed when
the pulldown comes back, so it cannot keep claiming there is no tray after one
appears.

The classification, the close decision and the status line are **pure
functions** in `src/platform.rs`, so both branches are unit-tested without a
desktop session — 9 tests cover the three probe outcomes, the two close
outcomes, and the exact shape of the line a CI job asserts on.

### Actions

| Where | Action | Effect |
| ----- | ------ | ------ |
| Tray | **Click the icon** | Opens the pulldown, or dismisses it if it is already open. |
| Tray | Right-click → `Quit Gregg` | Ends the GUI. The client daemon keeps running. |
| Pulldown | Click a host row | Selects that host. The selection is keyed by the stable system id, so a host that reorders does not move the selection. |
| Pulldown | Click a scheduler job | Opens that job's retained records. Only an open job asks the daemon for history; closing the view releases the intent. |
| Pulldown | `Detail` (or `Fleet`, in the detail) | Moves between the two views: the fleet's blocks, one per host, and the selected host's own numbers. |
| Pulldown | `Reload` | The `Ctrl-R` boundary: the daemon re-reads its configuration and polls immediately. |
| Pulldown | `Add host` | Opens the form that adds a host. |
| Pulldown | `EggPool` | Opens and closes the `EggPool` plane. The intent is sent only while it is open. |
| Pulldown | `About` | Shows the licence attribution in the pulldown. |
| Pulldown | `Quit` | Ends the GUI. The client daemon keeps running. |
| Pulldown | The selected host's `Edit` | Opens the form that renames the host, or that re-points it at another endpoint. |
| Pulldown | The selected host's `Remove` | Removes the host from the configuration. |
| Pulldown | **×** (in the header strip) | **Hides** the pulldown, and the tray brings it back. On a desktop with **no tray** it **ends the GUI** instead — a hidden pulldown with no tray is an application you can no longer reach. |
| Pulldown | **Esc** | The same as ×. |
| Pulldown | The header strip | **Drags** the pulldown. It has no window frame, so it has no title bar to drag. |
| Pulldown | The grip in the bottom-right corner | **Resizes** it. |

The host's own actions are **the pulldown's affordances**, aimed at the
*selected* host. They are not on every row: a button on every row crowds the
row and blurs into the selection tint.

Every action is reachable by pointer, and the pulldown never becomes the only
way to quit.

### The pulldown's shape and size

A dropdown has a shape, and this one draws its own.

- It is a **rounded card** with a **one-pixel outline** and a **soft shadow**
  behind it. Slint's software renderer does not implement drop shadows — its
  `draw_box_shadow` is an empty stub — so the shadow is drawn: **fourteen
  one-pixel rings** around the card, each a little larger and fainter than the
  last, with a **Gaussian alpha profile** (`exp(-x²/50)`) rather than a few
  hard steps. The peak is about 17 % black at the card's edge, fading to
  nothing 14 px out, with no visible banding.
- The card's **outline is a macOS hairline**, not Slint's palette border:
  `rgba(255, 255, 255, 0.098)` in the dark scheme and
  `rgba(0, 0, 0, 0.098)` in the light one — the measured value of
  `NSColor.separatorColor`. Slint's own `Palette.border` is 45 % black in a
  single pixel in the light scheme, which is a hard dark line, not a
  separator. The same hairline is used for the card's edge, the section
  separators, the pointer's outline and the resize grip.
- The card's **fill is a translucent material**: `Palette.background` at 94 %
  opacity, so a little of the desktop shows through, the way a macOS popover's
  vibrant material does. Slint cannot blur what is behind the window, so this
  is the closest approximation available in the software renderer.
- A **pointer** points at the tray item: a small triangle on the card's
  **trailing edge** — the edge the tray is on — drawn twice, once in the
  card's outline colour and a little larger and once in the card's own colour
  on top. It points **up** where the tray is at the top of the screen
  (macOS, Linux) and **down** where the tray is at the bottom (Windows).
- On **macOS** the window is transparent, so the rounded corners and the
  shadow are the desktop's own colours showing through. **Windows and Linux**
  have no transparent window, so the background is the card's own colour and
  the shadow reads as a soft edge instead. That is a platform limit, not a
  choice.
- The **size follows the content**: a fleet of one host is a short pulldown
  and a fleet of twenty is a taller one, up to the bounds the panes have. The
  height is *computed* rather than measured, because Slint 1.18 cannot report
  a layout's preferred size back to a `.slint` file. The **width** is the
  card's own chrome plus the width a fleet row needs — 606 px — because a
  bar takes the whole row and a pulldown narrower than the row would clip the
  bar rather than lay it out. The operator may make it wider; it does not get
  narrower than the row.
- The pulldown opens in the **trailing corner of the work area** — the
  platform's convention — and the operator's own placement is **remembered
  across restarts** in `<config>.gui-state` beside the configuration. The read
  is bounded and validated: a file that is missing, too large or malformed
  means the operator has not left a placement yet, and the pulldown opens
  where the convention says.
- On **macOS** the default placement opens a little clear of the menu bar
  rather than flush under it: AppKit places a window whose content is
  requested flush with the work area's top edge **26 px lower**. A placement
  the operator chose is honoured **exactly**, so this affects the first open
  only.

### The fleet's blocks

The pulldown's **fleet view** is the TUI's condensed fleet row **turned on its
side**: every host is a block of its own, so the fleet is a table of blocks
rather than a table of rows.

```
Deadpool  IO 40.7%  9.87/11.42/12.26  16C  912.96MHz
CPU   [████████████████████░░░░░░░░░░░░░░]   7%
MEM   [██████████████████████████░░░░░░░░░░]  91%
SWP   [███████░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  24%
DISK  [███████████████████████████████░░░░░░]  93%
NET   [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]   0%
```

- **The header line names the host and carries what is not a used share**: the
  I/O wait, the load, the core count and the frequency, in the TUI's own words
  and the TUI's own order — `Deadpool  IO 40.7%  9.87/11.42/12.26  16C
  912.96MHz`. A reading that is not a used share has no bar, so it can never be
  drawn as one: I/O wait in particular is a fraction of *time*, and a bar that
  meant something else would misread.
- **The five rows under it are the TUI's five used shares** — `CPU`, `MEM`,
  `SWP` (or `COMMIT`), `DISK` and `NET` — and each row carries **only its
  share**: the bar takes the full width of the panel less the percentage, and
  the percentage is the last thing in the row. The bar carries the precision
  and the number carries the glance.
- The **shares are compact** — `7%`, not `6.8%` — the way the TUI's condensed
  table writes them.
- A row with **no reading** draws an **empty bar** and an em dash: the share
  is negative, so an empty bar says *not measured* and never *measured as
  zero*.
- The block **names the host and not where it is**: the endpoint is not in the
  fleet view, the way the TUI's condensed row does not carry it either. The
  detail view names the host's endpoint.
- A host that is **not online** has no rows at all: its header says why it is
  offline and nothing else. That is the TUI's own rule — an offline condensed
  row is the name and the status, and five rows of dashes would only say it a
  second way.
- The **selected host** is marked by a quiet tint and by its name in the
  accent colour. It is deliberately *not* the platform's selection colour:
  that colour is a saturated accent, and the palette's own foreground for it
  is black on macOS, which turns a bar into what reads as a hole.
- The list is **bounded**: a fleet can be large, and the pulldown is a
  popover. The host list scrolls when the fleet is larger than the bound.

### The detail pane

The **detail view** is behind the pulldown's own footer control, and it is the
pane an operator actually reads. It is **the fleet view's own block, opened
up** — the selected host's block, drawn by the very same component the fleet
view draws every host with — and then the extra facts underneath it in one
condensed list.

```
Deadpool  IO 0.1%  1.73/2.22/4.72  16C  3.05GHz
CPU   [████████░░░░░░░░░░░░░░░░░░░░]  11%
MEM   [██████████████░░░░░░░░░░░░░░]  39%
SWP   [███░░░░░░░░░░░░░░░░░░░░░░░░░░]   9%
DISK  [█████████████████████████░░░░░]  86%
NET   [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]   0%

RESOURCES
  MEM         6.0 / 15.5 GiB
  SWP         7.3 / 84.0 GiB

DRIVES
  /              791.2 GiB / 914.8 GiB · 86.5% · 77.1 GiB free
  /boot/efi        6.1 MiB / 1.0 GiB · 0.6% · 1.0 GiB free
  /snap/core22     74.0 MiB / 74.0 MiB · 100% · 0 B free
```

- **There is no second set of progress bars.** The detail view has exactly the
  one block the fleet view draws, drawn by the one `HostBlock` component, so
  the two views cannot drift apart and a host is never shown twice with two
  different bar styles. The bars are the fleet's bars, in the fleet's columns,
  at the fleet's widths.
- The groups underneath are **one condensed list**: a quiet heading and a
  two-column row under it — the name on the left, the value on the right, in
  the quiet colour, because the name is what the row is about.
- **`RESOURCES` carries the numbers behind the shares** the block already
  shows: the block says `MEM 39%`, and this says how much memory that is. The
  row's name is the block's own label, so the two are obviously the same
  measure.
- **`DRIVES` carries each mount's own facts**, in the TUI's own words: the
  used and the total, the drive's own share, what is left of it, and — when
  the platform reported it — the read and write rates. The free space falls
  back to what is left of the total when the platform reported none, which is
  the TUI's own rule.
- **`NETWORK` carries the aggregate first and then each interface**, because
  the block's network share is the aggregate's share of its link. A link
  capacity that was not measured is **not stated**: the row says `RX` and
  `TX` and no `LINK`.
- The detail is **one scroll container**, so it scrolls as a whole and there
  is one scroll model.

### How the pulldown is dismissed

**`Esc`, the tray's click, and the pulldown's own × control.** That is the
whole list.

There is **no click-away dismissal**, and that is a real limitation rather than
an oversight. Slint 1.18 exposes no "the window lost focus" event: a window
cannot be told that the click landed somewhere else, so a pulldown cannot close
itself when the operator clicks the desktop behind it. The pulldown is also
`always-on-top`, so it stays over whatever is behind it.

The consequences are worth stating plainly:

- a pulldown left open stays open until it is dismissed deliberately;
- `Esc` works, because the pulldown's own focus scope handles the key, and the
  key reaches it even when a text field has the focus.

On a desktop with **no tray**, `Esc` and the × end the GUI instead, because a
hidden pulldown with no tray is an application the operator can no longer
reach.

### Host management

The pulldown can **add**, **remove**, **rename** and **re-point** the hosts in
the configuration file.

It writes that file through the **CLI's own library code** — `gregg::cli::dispatch`
against the same `ConfigStore` the daemon reads — so the pulldown inherits the
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
in the pulldown. A successful one asks the daemon to re-read its configuration
at its own reload boundary, and the daemon republishes.

**Changing a host's endpoint is a replacement, not an edit.** The endpoint is
part of a host's identity, so the host gets a new stable id and its selection
and history do not follow it. The pulldown says so before it applies one, and
the confirming button is labelled **Replace endpoint** rather than Save, so the
replacement is the operator's explicit choice. The new endpoint is added
**before** the old one is removed, so a refusal — a malformed endpoint, or one
that is already configured — happens before anything is removed.

**Renaming is in place.** The same endpoint with a different name keeps the
host's identity, because the endpoint — the thing the identity is made of —
did not move. An empty name field clears the configured name.

The pulldown is a **second writer of the configuration file**, never a second
writer of fleet state. The client daemon remains the only poller and the only
owner of the fleet's state.

### Application activation

On macOS the application **activates itself** before it raises the pulldown.
This is not decoration: a tray application is a *background*
application, macOS does not activate the app that owns an `NSStatusItem` menu,
and a background app cannot bring its own window to the front. Without the
activation, the tray's click does nothing at all when the GUI was launched the
way a real one is launched — a login item, a launcher, an agent.

Slint 1.18 exposes no activation API, so `src/platform.rs` calls
`NSApplication activateIgnoringOtherApps:` through the `objc2`/`objc2-app-kit`
bindings. Those crates are **macOS-only** and never enter the Linux or Windows
builds, and only the `NSApplication` and `NSWindow` features are enabled, so
the whole of AppKit is not linked. On Windows and Linux the function is a
no-op: there, showing a window the process owns is enough to bring it up.

### Another application's full screen

A full-screen application owns a **space** of its own, and that space is what
the operator is looking at. A window that is not in that space is not on the
screen at all: the click reaches the tray, the pulldown is built and placed,
and the operator sees **nothing happen** — and the full-screen application is
dropped out of its space on the way, which is worse than nothing.

macOS keeps a **Dock** application's windows out of another application's
space. Not at any window level, and not with any collection behaviour: the
right to reach another application's space is fixed when the window is
created, and a Dock application does not have it. A **menu-bar-only**
application does. The GUI is one now: it installs the `Accessory` activation
policy before any window exists, through winit's event-loop builder, which is
where the desktop makes that decision itself.

The native window is then told how to behave, through AppKit:

| | |
| --- | --- |
| `collectionBehavior` | `CanJoinAllSpaces`, `FullScreenAuxiliary`, `Stationary`, `IgnoresCycle` |
| `level` | `NSPopUpMenuWindowLevel` (101) — the level a native status item's own menu is drawn at |
| `hidesOnDeactivate` | `false` — the pulldown stays up when the operator clicks away |

`always-on-top` alone is not enough: winit maps it to the *floating* level
(3), and the floating level is below full-screen content.

The native window is found by its **title**, and that title is **owned by the
`.slint` file** — bound to a property the Rust side reads — so the two cannot
drift apart. The pulldown is the only window this application owns that
carries a title at all; the status item's own windows are AppKit's, and they
carry AppKit's titles.

Two consequences worth stating:

- **There is no Dock icon.** An accessory application has none, and a
  tray-only GUI has no business having one: the pulldown carries `Reload`,
  `About` and `Quit` itself, and the tray carries the rest.
- **The menu bar stays the other application's.** Activating a menu-bar-only
  application does not take the menu bar away from the application the
  operator is using, so the full-screen application keeps its menus — which
  is what a native menu-bar extra does.

The menu bar itself is macOS's own business: in full screen it is hidden, and
the operator brings it back by moving the pointer to the top of the screen
(unless "Automatically hide and show the menu bar in full screen" is set to
Never). That is true of every menu-bar extra, and nothing in this GUI can
change it.

### Redraws

There is **no polling timer**. The bridge wakes the UI only when it actually
published something, and the update is scheduled on the UI thread with
`slint::invoke_from_event_loop`. An idle GUI redraws for no reason at all, and
a redraw only ever happens because something changed.

### What a background GUI costs

The GUI is meant to **run constantly in the background**, so the numbers that
matter are the ones it costs while nobody is looking at it.

**The pulldown is built when the operator first opens it.** A window costs
about thirty megabytes before it draws anything — the software renderer's
buffer, its scene and its first frame — and a background GUI that nobody has
opened yet should not be paying for it. So the panel's component is created on
the first tray click, not at start-up, and it lives for as long as the process
does once it exists. `--panel` (and a desktop with no tray at all) still opens
it at start-up, because that is what those two paths are for.

**A hidden panel does not rebuild the view.** The daemon publishes a document
every few seconds, and the whole model is a few thousand formatted strings —
every host's every measurement, formatted. Building that for a panel nobody is
looking at is work nobody can see, and it churned the heap for nothing. So a
hidden panel is **not** re-rendered: the tray keeps its two facts — the
fleet's counts and the summary line — straight from the document, and the
pulldown is rebuilt **the moment it is opened**, so what it shows is the
newest document and not the last one it happened to be open for.

The measured cost, on this machine (macOS 15, arm64), with the real daemon and
a seven-host fleet:

| | resident set | CPU |
| --- | --- | --- |
| **idle, the pulldown never opened** | **45.5 MB** (35.8 min, 45.5 max over 60 s) | **about 1.6 %** of one core |
| the pulldown open | 107 MB | about 8 % of one core |

**Before the pulldown was built on demand** the same idle binary measured
**82 MB** (82.0 MB median over a minute) — so the deferred window is worth
**about 37 MB**, and the idle footprint is now close to what a Slint app costs
with **no window at all** on this desktop.

**The floor is Slint's, not ours.** The same measurements on a *trivial* Slint
app — the same features, the same software renderer — give:

| | resident set |
| --- | --- |
| an event loop with no component at all | 27–35 MB |
| a tray icon and no window | 44 MB |
| a window component, never shown | 62–74 MB |

So a Slint app on this desktop costs about 65 MB before it draws anything, and
a window is about 30 MB of that. There is **no leak**: `leaks` reports 287
leaks for 14 KB, all of them in Apple's own XPC and Objective-C machinery.

Two honest consequences:

- **A background GUI on this desktop still costs tens of megabytes, and that is
  Slint's software renderer and the window server, not the model.** The
  cheapest way to make it smaller would be a different renderer, and Plan 192
  pins the software renderer.
- **The model is not the cost.** The same binary measured before and after the
  hidden-panel rule differs by **0.6 MB** of resident set and **0.06 s** of
  CPU over a minute — noise. The cost is the daemon's document arriving and
  being decoded, which the TUI pays too.
- **Destroying the window does not give the memory back.** Dropping the panel's
  component and watching for ninety-six seconds left the resident set exactly
  where it was (75,168 KB, flat), so the deferred window is a **one-off**
  saving: it is worth thirty megabytes to a GUI that has never been opened,
  and nothing to one that has.
- **macOS reclaims most of it over hours.** A five-hour-old instance of the
  same binary measured **14–30 MB** — the dirty pages the allocator was
  holding are purged under the system's memory pressure. The number `ps`
  reports for a *fresh* process is the high-water mark, not the steady state.

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
  registered an item whose **tooltip carries the live fleet status** — the
  counts and the host names — and whose menu carries `Quit Gregg`; and it
  checks the GUI's own `pulldown=placed` line, so the exercise asserts the
  pulldown's **position** and not only that a window exists. Then, in a
  second pass on a bus with **no** host, it asserts that the GUI reports
  `tray=unavailable` and keeps running;
- on **Windows**, proves the **core** client-daemon launch path first, with
  the CLI and no GUI involved: `gregg daemon restart` takes the launch lock and
  spawns a daemon, `gregg daemon status` identifies it through the daemon's own
  handshake, and `gregg daemon stop` releases it. That is the path the terminal
  client takes, and it is checked on its own so the GUI is not what proves it.
  Then the exercise starts the GUI and proves it created a **native top-level
  window** for its process, and that the client daemon it launched is running
  and really polling the remote.

Both exercises **wait** for the GUI's `pulldown=placed` line, within their own
timeouts, rather than reading the output once: the line is printed when the
pulldown's native window exists, and the native window is created on the event
loop's own terms — so the line comes when it comes.

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
  and `NSWindow` features. They exist so the GUI can activate itself when it
  raises the pulldown panel, and so the pulldown's **native window** can be
  told to float over another application's full-screen space; see
  "Application activation" and "Another application's full screen" above.
  They never enter the Linux or Windows builds.

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
| `backend-winit` | The windowing backend that draws the panel on Linux, macOS and Windows. |
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

This workspace carries a vendored crate, `vendor/annotate-snippets`, because
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

### Vendored patch: `softbuffer` 0.4.8

This workspace also carries `vendor/softbuffer`. `softbuffer` is **not** a new
dependency: Slint's winit backend already uses it to present the software
renderer's frames, and it was already in this graph. The vendored copy changes
**one word** in the macOS backend.

`softbuffer`'s macOS backend hands CoreGraphics a `CGImage` whose pixel format
is declared as `CGImageAlphaInfo::NoneSkipFirst` — *no alpha channel at all*,
the first byte of each pixel is an unused filler. The software renderer writes
a correct premultiplied alpha into that byte, and CoreGraphics throws it away,
so every transparent pixel of a window is presented **opaque**.

For the pulldown that meant the 14 px margin around the card — transparent by
design, so the shadow could show the desktop through it — was presented as a
**thick black frame**, and the card's rounded corners were black squares. The
vendored copy declares `CGImageAlphaInfo::PremultipliedFirst` instead: the same
32-bit layout, with the alpha the buffer already carries. The change is inside
Apple's platform only; every other backend is byte-identical to upstream.

Without this copy the pulldown cannot have a soft shadow or rounded corners
at all on macOS, because the alternative is an opaque window with square
corners and no shadow.

Full rationale, the measurements that established the defect, and the
alternatives that were rejected:
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
| The GUI after Plan 191 (the tray pulldown, with host management) | 11,896,608 bytes (11.3 MiB) |
| The GUI with the fleet view as a table of blocks (the previous build) | 12,012,560 bytes (11.5 MiB) |
| **The GUI now** (the same fleet view, and a detail view that is the *same block* plus one condensed list) | **11,996,064 bytes** (11.4 MiB) |
| **Difference** (before Plan 191 → now) | **+1,192,704 bytes (+11.0%)** |
| The same GUI calling `gregg::cli::dispatch` (the rejected alternative) | 14,761,600 bytes |

So the pulldown and the host-management forms cost about **1.02 MiB** more
than the monitor window they replaced. The revised chrome and the metric bars
cost **+66,224 bytes (+0.6%)** on top of Plan 191, the fleet view as a table of
blocks costs **−49,568 bytes (−0.4%)** on top of the two-view fleet table it
replaced — the block's rows are five `Text`s and one bar each instead of five
bar columns and their headings — and the vendored `softbuffer` patch costs
**nothing measurable**: it changes one word in a pixel-format declaration that
was already in the binary.

The detail view's rewrite cost **−16,496 bytes (−0.1%)**: the host's block is
drawn by the *one* `HostBlock` component now, so the detail view's own metric
table — a second set of bar columns, a second set of headings and a second
set of column widths — is gone, and the condensed list of groups is a heading
and two `Text`s per row.

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

**Corrected on 2026-10-22: the table above is `ps` RSS, and RSS is the wrong
instrument on macOS.** It counts shared, clean, reclaimable pages — the dyld
shared cache, `icudt78l.dat`, the system font collections, the macOS
appearance resources — which every app on the machine maps. The **physical
footprint**, which is what the machine actually pays and what Activity Monitor
shows, was measured on the same build with `footprint`:

| configuration | `ps` RSS | **physical footprint** | dirty | Rust heap (`MALLOC_SMALL`) |
| ------------- | -------- | ---------------------- | ----- | ------------------------- |
| The panel open | 71–92 MB | **18 MB** | ~20 MB | 9 MB |
| The panel hidden | 67–76 MB | **11 MB** | — | 7 MB |

So the per-process cost is roughly a **quarter** of the RSS figure, and the
"72–83 MB resident" is better read as *the size of the shared system frameworks
this process touches*. Three further measured facts, all new:

- **The cost does not scale with the fleet.** 1 host on a 5 s poll: 18 MB
  footprint, 9 MB heap. 25 hosts on a 3 s poll: 19 MB footprint, 9 MB heap.
- **There is no leak.** 150 s of continuous polling left the footprint flat at
  22–24 MB, with RSS fluctuating and no trend.
- **One real lever, and it is not reachable from inside the process.** With
  `MallocSpaceEfficient=1` set **before** the process starts, the heap drops
  from 14 MB to 8 MB and the footprint from 26–28 MB to **14–15 MB**, with the
  CPU unchanged. macOS's malloc reads that variable in `malloc_init()`, before
  `main()` runs — setting it from inside the process was measured and does
  **not** work — so it would have to come from a launcher's environment, an
  `LSEnvironment` plist entry, or a re-exec. `mimalloc` as a global allocator
  was measured and gives **no** gain (13–16 MB heap); a smaller panel is
  worth about 1 MB; and a GPU renderer is not worth its dependency.

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
│   ├── main.rs           # entry point: the pulldown, the tray, wiring, the render push
│   ├── bridge.rs         # the client-daemon attachment, the intents, the frame pump
│   ├── model.rs          # the pure FrontendSnapshot -> GuiViewModel projection
│   ├── hosts.rs          # the configuration mutations, behind an injectable seam
│   ├── placement.rs      # where the pulldown opens, how big it is, how far it may be moved
│   ├── platform.rs       # the per-desktop bits: macOS application activation, the tray's menu
│   ├── state.rs          # where the operator left the pulldown, between runs
│   └── ui/
│       └── main.slint    # the Pulldown and the SystemTrayIcon
└── vendor/
    ├── VENDORED-PATCH.md # why each vendored crate exists
    ├── annotate-snippets/
    └── softbuffer/
```

### The layers

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
- **`placement.rs` owns where the pulldown opens and how big it is.** The work
  area, the anchor and the clamping are pure functions, so they are tested
  without a desktop session. Slint 1.18 cannot report a layout's preferred
  size to a `.slint` file, so the pulldown's height is *computed* from the
  rows the model has; the numbers the computation uses are written down in
  `placement::metrics`, and a test checks them against the `.slint` file so
  the two cannot drift apart.
- **`state.rs` owns where the operator left the pulldown.** The geometry is
  written to `<config>.gui-state` beside the configuration, and the read is
  bounded and validated: a file that is missing, too large or malformed means
  the operator has not left a placement yet.

## Slint 1.18 notes worth knowing

- `SystemTrayIcon` is a **top-level** component: it is derived with
  `inherits SystemTrayIcon`, never placed inside a `Window`. A tray-rooted
  component does not share `Globals` with a window-rooted one — each instance
  gets its own copy of every global, so shared state has to be pushed into
  each of them explicitly.
- A `SystemTrayIcon` must contain **exactly one `Menu` child**, and no other
  child element types are allowed in it. The menu **may** be inside an `if`:
  Slint 1.18 lowers a conditional menu to a shadow tree that is switched on
  and off with the condition (`i-slint-compiler-1.18.1/passes/lower_menus.rs`),
  and a menu with **no entries** makes the platform detach it — which is how
  the macOS status item is left with no menu, and so with a click that reaches
  the tray's `clicked` callback.
- The tray's `clicked` callback belongs to the `SystemTrayIcon` **item**, and a
  derived component does not re-export an inherited item's callback to the
  language binding: there is no `on_clicked` in Rust. Declare a callback of
  your own and forward the inherited one to it (`TrayIcon.tray-clicked`).
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
