# Plan 192: The pulldown carries the panel

Status: planned. Depends on Plan 191 (implemented, pending native interactive acceptance) and the
187-190 line; unblocks nothing yet. Independent of Plan 091.

## Objective

Make the **tray's click open the pulldown**, and make the pulldown **carry the panel's contents**
— the fleet, the metrics, the drives, the interfaces, the cron, the records, the `EggPool` plane
and the host forms. Not a launcher that opens a separate window: the pulldown *is* the surface
the operator looks at.

This plan **changes the direction** Plan 191 set. It does not change what Plan 191 built — the
host management, the CLI write path and the single surface all stay — it changes what the
operator's click does and what the tray's dropdown contains.

## The direction being corrected

Plan 191, and the Plan 187 roadmap it came from, made the **native tray menu the dropdown** and
the panel an **independently opened window** that the menu launched. The Plan 187 roadmap
states the premise:

> "Slint's tray icon is its own top-level component and renders a native menu; it is not a
> general-purpose arbitrary-content tray popover. The first UI has a native tray menu and a
> separately opened Slint monitor window; do not promise identical popup positioning across
> macOS, Windows, X11 and Wayland."

and Plan 191 wrote the consequence into its own source, in `apps/gregg-gui/src/ui/main.slint`:

> "A native menu is not a scrollable viewport, so the list is bounded: **the panel is the
> surface that carries the whole fleet**."

The premise is half true and the conclusion drawn from it was wrong. The **native menu** cannot
host arbitrary Slint content — that is true. But it does not follow that the content must live
in a *separately opened* window. Slint's `SystemTrayIcon` has a **`clicked`** callback, and
when the native menu has no entries the platform routes the tray's click to that callback
instead of popping the menu. So the tray's click can open a Slint popup that carries the
content, placed under the tray item. **The popup is the pulldown; the native menu is not
required to be the dropdown at all.**

The operator sees the difference immediately. As built, the tray's menu offers a summary, a
flat list of host jump entries, and `Open Gregg` / `Reload` / `About Gregg` / `Quit Gregg`;
the content lives in a separate frameless window that the menu launches. That is a launcher
with a dashboard behind it, and it does not read as a pulldown.

## What the current build actually is

Reproduced on 2026-10-22 on macOS (Apple M4 Pro, 3840×1080), with the GUI attached to a
client daemon and a two-host configuration:

- the panel is a fixed **560×520** rectangle: a header strip with a `Close` button that
  overhangs the strip's edge, the daemon's identity hash printed as a line of UI, the host
  list with `Edit` and `Remove` buttons on every row, `METRICS` and `CRON` headings over large
  empty regions, a bottom button bar, and a resize grip;
- it has **no rounded corners, no drop shadow, and no pointer** toward the menu-bar item;
- it is **not positioned**: it lands wherever the window manager puts it (x=1640 on a
  3840-wide display — dead centre), because `placement::work_area()` returns `None` on the
  startup path (see *Two defects found while reproducing this*, below);
- the tray's native menu is the only dropdown, and it is a **launcher**.

## Feasibility

Read from the pinned Slint 1.18.1 sources, not assumed.

**The tray's click is available on all three platforms.**
`SystemTrayIcon.clicked` — "Invoked when the user left-clicks the tray icon itself, as opposed
to picking an entry from its menu" (`i-slint-compiler-1.18.1/builtin_elements.rs:3993`).

| Platform | Source | Behaviour |
| --- | --- | --- |
| macOS | `i-slint-core-1.18.1/items/system_tray/appkit.rs:64`, `:221`, `:248` | The status item's button action is `tray_clicked:`, which calls `clicked`. `rebuild_menu` calls `setMenu(None)` when the Slint menu has **no entries**, so AppKit forwards the click to the button's action instead of popping the menu. The button's `sendActionOn` mask is `LeftMouseUp`, so it is a left-click. |
| Windows | `i-slint-core-1.18.1/items/system_tray/windows.rs:290-298` | `WM_LBUTTONUP` → `clicked`. `WM_RBUTTONUP` / `WM_CONTEXTMENU` → the native menu. |
| Linux | `i-slint-core-1.18.1/items/system_tray/ksni.rs:45`, `:284` | The `StatusNotifierItem` `Activate` event → `clicked`. |

**A detached menu is reachable from the `.slint` file.** The `clicked` documentation states the
mechanism: "Invoked on a left-click when no `Menu` is attached (or when an
`if cond : Menu { ... }`'s condition is currently false)", and the macOS backend's
`rebuild_menu` implements it — a false condition reports zero entries, the `NSMenu` is
detached, and the click routes to `clicked`. **Verified by compiling it**: `if cond : Menu { … }`
is accepted as the `SystemTrayIcon`'s single `Menu` child in this workspace.

**What is not available, and is recorded rather than hidden.**

- The **tray item's screen position is not exposed** by Slint 1.18.1, so the pulldown cannot be
  placed exactly under the icon. It is anchored to the platform's trailing corner under the
  menu bar (macOS, Linux) or notification area (Windows), clamped to the display's bounds,
  with the same documented shell-strip approximation Plan 191 recorded — 28 logical px of strip
  at the top on macOS and Linux, 44 at the bottom on Windows. On a desktop whose tray is not in
  the trailing corner, the pulldown will not point at the icon.
- A **native menu cannot host Slint widgets**, so the pulldown is a Slint window. The native
  menu is reduced to what a native menu is good at — and on macOS it is detached entirely,
  which means the pulldown must carry `Quit`, `About` and `Reload` itself.
- **Click-away dismissal is not available**: Slint 1.18.1 exposes no focus-lost event. The
  pulldown is dismissed by `Esc`, by the tray's click, and by its own close control. This is
  stated in the user documentation, not papered over.
- On **Linux the `Activate` gesture is decided by the desktop environment** or its shell
  extension, so on some desktops the primary click may be claimed by the host. Which gesture
  opens the pulldown has to be observed in a real session, and the plan records which.

## Two defects found while reproducing this

Both were found on 2026-10-22 by running the committed build, and both are corrected here
because they are what make the pulldown impossible to get right.

**1. The pulldown is never positioned on the startup path.**
`placement::work_area()` asks Slint for the winit window, and Slint's own documentation says
`with_winit_window` "will only succeed when the event loop is active"
(`i-slint-backend-winit-1.18.1/lib.rs:1099-1103`). The `--panel` startup path — and every desktop
with no tray host — calls it **before** the event loop runs, so it returns `None`: the panel is
never positioned, never clamped to the work area, and its size is never clamped either.
Measured on the committed build: `work_area=None`, no `set_position` at all, and the window at
x=1640 on a 3840-wide display. Deferring the first open into the event loop makes it resolve:
`work_area=Some(left 8, top 36, 3824×1036)` and the window at **(3272, 36)** — under the menu
bar. **`placement.rs` is inert on the path the GUI's own native CI exercises use**, because
they launch with `--panel`.

**2. The pulldown's position and size are not kept across a restart.**
`UiState` holds `panel_position` and `panel_size` in memory; the GUI writes **no state file
at all**. "Keeps its position and size" is true for one session and false across a restart,
while the GUI README and Plan 191's closure record both say it is remembered. A menu-bar
operator drags the panel once and expects it to stay there.

## Scope

### Required

1. **The tray's click opens the pulldown**, and the pulldown shows the panel's contents. The
   `Open Gregg` launcher entry is removed.
2. **The pulldown carries the content**: the fleet summary, the host list, the selected host's
   metrics, drives, interfaces, cron, records, the `EggPool` plane, and the host
   add/remove/rename/re-point forms. `Reload`, `About` and `Quit` move into it, because on
   macOS no native menu survives.
3. **It reads as a dropdown**: rounded corners, a drop shadow, a pointer or notch toward the
   tray item, and it is sized to its content rather than a fixed 560×520.
4. **Placement is correct on every path that opens it** — the tray's click, the startup path,
   and a desktop with no tray host — by deferring the first open into the event loop so the
   work area is available, and by clamping the size to it.
5. **The pulldown's position and size survive a restart**, written to a small state file
   beside the config, with a bounded and validated read.
6. **The application chrome is dropped**: the `Close` button that overhangs the header strip,
   the `METRICS` / `CRON` headings, the bottom button bar that duplicated the tray menu, the
   daemon-identity line, and the per-row `Edit` / `Remove` buttons that crowd the row and blur
   into the selection tint. The host's actions live in the pulldown's own affordances.
7. **The no-tray fallback still holds**: with no tray host, the pulldown is the only control
   surface, it opens at startup, and closing it quits the GUI.

### Excluded

- a second polling plane, a second daemon, or any HTTP client in the GUI;
- a protocol change: no new `DaemonRequest`, no version bump, no handshake change;
- any change to `greggd`'s API, the cron plane, the `EggPool` plane, or the daemon's ownership
  of polling;
- any change to the TUI's behaviour, key bindings or rendering;
- click-away dismissal, per-platform input hooks, and hand-rolled tray icons;
- a GPU renderer, and a vendored patch of Slint's runtime code;
- a new dependency in the GUI workspace;
- a new CI job, workflow, matrix, artifact or evidence bundle;
- a release asset, installer, bundle, packaging job or publication step;
- rewriting the historical records of Plans 187-190.

## Closure gates

- [ ] Clicking the tray icon opens the pulldown on macOS, Windows and Linux, and the pulldown
      shows the panel's content. There is no launcher entry, and the content is in the
      pulldown rather than in a separately opened window.
- [ ] The native menu is not the primary surface on any platform. On macOS it is detached, and
      the pulldown carries `Reload`, `About` and `Quit`.
- [ ] The pulldown is positioned and clamped within the work area on **every** path that opens
      it: the tray's click, the startup path, and a desktop with no tray host. The native
      exercises assert the position, not only that a window exists.
- [ ] The pulldown's position and size survive a restart.
- [ ] The pulldown reads as a dropdown: rounded, shadowed, with a pointer toward the tray
      item, and sized to its content.
- [ ] The forms still add, remove, rename and re-point hosts through the CLI's own code, and a
      refused mutation leaves the config byte-identical and shows a visible reason.
- [ ] The GUI still opens no HTTP client, still starts no polling plane, and the daemon's
      `DaemonRequest` set is unchanged.
- [ ] The GUI's own CI job still formats, lints, tests and release-builds on macOS, Windows
      and Linux, and the native tray exercises still pass.
- [ ] The **interactive** native acceptance — a visible notification-area icon, the click
      opening the pulldown, and the pulldown being usable on a real desktop — is recorded per
      target, or is **left unchecked** and blocks shipment for that target, exactly as Plan
      190 leaves it.

## Release qualification

Plan 192 inherits Plan 190's and Plan 191's outstanding qualification, and adds one item.

- **The macOS interactive run must be repeated for the new surface.** Plan 190's recorded macOS
  run exercised *Open Monitor* and a monitor window; that surface no longer exists, and the
  recorded run says nothing about the pulldown.
- **Windows** needs a run in a real desktop session: a **visible** notification-area icon, a
  left-click that opens the pulldown, a right-click that opens the native menu, and the
  pulldown rendering its content.
- **Linux** needs a run in a real GNOME session, and it must record **which gesture** opens
  the pulldown, because the `StatusNotifierItem` `Activate` gesture is the desktop's decision.
  The CI leg runs under **Xvfb (X11)**; the Wayland path is compiled in (`winit/x11` and
  `winit/wayland` are both enabled) and has never been executed, and Wayland is the modern
  GNOME default.
- The **macOS Intel** build has never been produced, and there is still **no recorded DPI
  measurement**. The pulldown is placed in logical pixels against the shell-strip
  approximation, so it must be checked at 125 % and 150 % scaling.
- **New in this plan**: the pulldown's *content* must be observed at each of those settings —
  the click opening it, its position relative to the tray item, and the host forms being
  usable — not merely that a window appeared.

Nothing in this plan reopens the headless `greggd` API, the completed client-daemon Plans
161-172, or the independent Plan 091 soak record.

## Risks and deviations

- **On macOS the tray icon will have no menu at all.** An operator who expects a menu-bar icon
  to have a menu will not find one, and `Quit` will be inside the pulldown. That is a direct
  consequence of "no launcher", and the pulldown is where those actions live. It is the
  operator's call to accept it; this plan states it rather than hiding it.
- **The pulldown cannot point exactly at the icon**, because Slint 1.18.1 does not expose the
  tray item's position. The corner anchor is an approximation, unchanged from Plan 191 and
  documented.
- **Linux's primary gesture is the desktop's decision**, so the pulldown may open on a
  different gesture than on the other two platforms. The plan records what the session does
  rather than assuming.
- The **panel geometry's state file** is new state on disk. It is bounded, validated on read,
  and holds only the position and the size; a corrupt or out-of-range file falls back to the
  default rather than failing the GUI.
