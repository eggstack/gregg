# Plan 191: Tray pulldown GUI and host management

Status: planned. Depends on Plan 190 (implemented, pending release qualification) and the
187-189 line; independent of Plan 091. Unblocks nothing yet.

## Objective

Turn the optional `gregg-gui` frontend into a **single tray pulldown** — one
borderless, always-on-top panel opened from the tray, carrying **all** the
metrics, drive and interface detail, cron, and `EggPool` surfaces the compact
monitor window carries today — and drop the separate monitor window entirely.
Add the ability to **add, remove and edit** hosts from that panel, against the
same Gregg config and the same per-config client daemon the GUI already
attaches to.

The daemon's ownership of remote polling is untouched: the pulldown is still a
frontend over the per-config client daemon, and still opens no HTTP client of
its own.

## Framework review: is Slint still the right target for a pulldown-only GUI?

The question was asked explicitly, so it is answered here with evidence rather
than assumed. The criteria for this product are narrow: a **native** system
tray icon on macOS, Windows and Linux; a **lightweight** resident footprint; no
webview; one Rust codebase; and a panel that can be shown and dismissed.

| Option | Native tray | Footprint / residency | Toolchain | Verdict |
| --- | --- | --- | --- | --- |
| **Slint 1.18.1** (current) | First-class `SystemTrayIcon` with a native menu on all three platforms | 10,803,360 B stripped; 72–83 MB resident; ~0.65% of one core | Rust 1.92 (its own floor) | **Retain** |
| `egui`/`eframe` + `tray-icon` | No built-in tray; a second crate (`tray-icon` 0.26.1) plus a hand-written native menu | Materially smaller binary and RSS, but immediate-mode redraw | `egui` 0.37 needs **Rust 1.95**; the 0.34/0.35 line fits 1.92 | Rejected |
| Tauri / any webview | `tauri-plugin-tray` | A webview process dominates RSS | — | Excluded by existing rules |
| GTK4 / Qt6 / per-platform native | First-class | Heavy, or three codebases | Not Rust | Rejected |

Findings:

1. **Slint 1.18.1 is the latest published release** (crates.io, published
   2026-09-21; `max_version` and `newest_version` are both `1.18.1`). There is
   no newer Slint to move to, so "still the best target" also means "and there
   is nothing newer on the table".
2. **The footprint is Slint's renderer, not the window count.** The measured
   Slint-only baseline is 9,956,032 B and the GUI adds 847,328 B over it. The
   72–83 MB resident figure is the software renderer and the system-font
   machinery, and it is paid whether a window is open or not. Dropping the
   separate monitor window does not make the process lighter; only the
   renderer choice would.
3. **The tray is the hard part of this product, and Slint is the only Rust
   option that ships it first-class.** `SystemTrayIcon` renders a real
   `NSStatusItem` on macOS, `Shell_NotifyIcon` on Windows and a
   `StatusNotifierItem` on Linux, and the GUI's own CI already exercises the
   Windows and Linux tray paths natively. The `egui` route replaces that with
   a hand-rolled tray, which means re-implementing a native menu on three
   platforms — precisely the work Slint has already done and this line has
   already proven.
4. **The `egui` alternative also raises the toolchain floor.** `egui` 0.37
   declares `rust-version = 1.95`, above this workspace's 1.92 floor; only the
   0.34/0.35 line fits. The 1.92 floor exists because Slint 1.18 needs it and
   the root workspace must stay at 1.89; raising an optional frontend's floor
   to 1.95 to save a few megabytes is a poor trade.
5. **What *can* be trimmed inside Slint is measured, not assumed.** Importing
   `std-widgets` for `ScrollView` grew the binary by ~1.03 MB on both sides of
   the comparison, because the import pulls the whole common widget library
   (buttons, line edits, list views, their assets). A pulldown that scrolls
   with its own bounded implementation could avoid that import. That is a
   **measurement to take**, not a claim: the import is replaced only if the
   rebuilt binary is smaller, and both figures are recorded either way.

**Decision: RETAIN SLINT 1.18.1.** The alternatives trade the one component
this product cannot do without — a native tray — for a footprint that is
dominated by the renderer rather than by the window. The pulldown therefore
keeps Slint, and Plan 191 is a change of *shape*, not of framework.

## The pulldown

### One surface, all metrics

The operator's decision is explicit: the pulldown carries **all** the current
metrics, and there is **no separate GUI window**. The panel is therefore the
present monitor window's content, without window chrome, sized to its content
(about 560 × 520 logical pixels at the current density), resizable, and
clamped to the work area of the display it opens on.

This is recorded as a tension rather than smoothed away: a pulldown that
carries every pane is a **large** popup, not a compact one. A genuinely small
pulldown (roughly 320 × 400) would have to become hierarchical — summary, then
host list, then detail on demand — which is the *other* product shape and is
not what was chosen. The panel keeps every pane, and the "compact" claim is
made about the **tray-first interaction**, not about the panel's pixel size.

### Show, position and dismiss

Slint 1.18's `SystemTrayIcon` supports only `icon`, `tooltip`, `title` and
**exactly one `Menu`** (`i-slint-core-1.18.1/items/system_tray.rs`). Clicking
the tray icon opens the *platform's* menu; there is no API for a custom panel
anchored to the tray icon, and no API that reports where the tray icon is.
That fixes the interaction shape:

- the native menu stays, and gains a host-aware summary and an **Open Gregg**
  action that opens the pulldown;
- the panel is a separate `Window` with `no_frame = true` and
  `always_on_top = true` (both exist in Slint 1.18 —
  `i-slint-core-1.18.1/items.rs:1268-1270`);
- the panel is positioned adjacent to where the native menu was drawn, which is
  where the pointer was when the operator chose the menu item, clamped to the
  work area so it never opens off-screen;
- the panel is dismissed by `Esc`, by the tray action again (a toggle), and by
  its own close control. It keeps its last position and size.

**Named limitation, not a solved problem:** Slint 1.18 exposes no focus-lost
event, so the panel **cannot dismiss itself when the operator clicks
elsewhere**. Dismissing on click-away would need a per-platform input hook
that does not exist portably, and it is excluded. The first implementation
ships Esc/toggle/close dismissal, and the limitation is reported rather than
hidden. If Esc-and-toggle dismissal proves unusable in practice, that is a
stop condition, not something to paper over.

### Style

The panel is a **native-looking** tray surface, so the hard-coded monochrome
palette is replaced with Slint's `Palette` global (`background-color`,
`foreground-color`, `dark-color-scheme`), which follows the desktop colour
scheme. The buttons gain hover and pressed states, because a popover is
operated with the pointer already over it. The host list keeps its bounded
scroll; every pane keeps a bounded height so a large fleet cannot push the
metrics, cron and `EggPool` sections out of the panel.

### The tray menu

The native menu remains bounded and honest: the fleet summary, the host
entries the operator can jump to, **Open Gregg**, **Reload**, **About** and
**Quit**. It is a menu, not a table: a fleet of 25 hosts is summarised, not
listed as 25 native items, because a native menu does not scale and the
pulldown is the surface that does.

## Host management

### What the GUI can do today, and cannot

The GUI's entire control surface over the daemon is three calls
(`apps/gregg-gui/src/bridge.rs`): `request_reload`, `set_eggpool_intent` and
`set_cron_intent`. The protocol has **no** config-mutation message:
`DaemonRequest` has exactly five variants — `Handshake`, `ReloadConfig`,
`Shutdown`, `SetEggpoolIntent`, `SetCronIntent`
(`crates/gregg/src/clientd/protocol.rs`). Host management lives in the CLI,
which mutates the config through `ConfigStore::mutate_with_result` — locking,
validating the whole config, writing atomically — and then nudges the running
daemon to reload.

So today the GUI is **read + reload**: `gregg add host:port`, then **Reload**,
and the host appears. This plan makes the GUI able to do the first step
itself.

### The write path

The GUI writes the config through the **CLI's own library code**, not through
a second implementation of the rules:

- `gregg::cli` is a public module of the `gregg` crate, and `apps/gregg-gui`
  already depends on that crate, so the pulldown can call
  `gregg::cli::dispatch(Command::Add { .. } | Command::Remove { .. }, store)`
  against the same `ConfigStore` the daemon uses;
- that call inherits the CLI's exact behaviour: the same endpoint grammar
  (explicit port, `nickname@host:port`, `[ipv6]:port`, no HTTPS), the same
  refusal of a portless or nickname-only host, the same config lock, the same
  whole-config validation, and the same atomic temp-file write;
- the GUI then calls the bridge's existing `request_reload`, so the daemon
  re-reads the config at its own reload boundary and republishes. The GUI
  never asks the daemon to apply a mutation, because the daemon has no such
  message and this plan does not add one.

**The one-writer statement this plan makes explicit:** the client daemon is
the only writer of *fleet state* and the only *poller*; the config *file* may
be written by the CLI or by the GUI, never concurrently, because both go
through the same lock in `ConfigStore::mutate_with_result`. Every write is
followed by an explicit reload, so there is no watcher and no background
mutation.

### Add, remove, edit

- **Add** — the operator supplies `host:port` in the same grammar the CLI
  accepts. A refusal (portless host, HTTPS, a nickname without a host) is
  surfaced in the panel as a visible, non-destructive message, and the config
  is left byte-identical.
- **Remove** — the same call the CLI's `remove` makes, which accepts a
  host-only spelling.
- **Edit** — two distinct operations, because they are not the same kind of
  change:
  - **Rename** — a name change against the same endpoint. In place, same
    host identity.
  - **Endpoint change** — a *replacement*, not an edit: the endpoint is part
    of the system's identity, so the host's stable id changes with it. The
    panel states this before it is applied, because the host's selection and
    its history belong to the old id. This is the same reason the TUI treats
    a re-pointed endpoint as a different host.

### What the GUI must not do

- It must not write the config on a background thread, or speculatively; a
  mutation is the operator's explicit action.
- It must not add a second polling plane, an HTTP client, or its own cron or
  `EggPool` fetch.
- It must not change `DaemonRequest`, the protocol version, the handshake, or
  the daemon's ownership of polling.

## Verification baseline

- `cargo test --manifest-path apps/gregg-gui/Cargo.toml` (the GUI's own
  workspace) and `./scripts/check-gui.sh`.
- `cargo test -p gregg --all-features -- clientd` for the core side.
- The GUI's model stays pure: the config-mutation call is behind a narrow
  seam so the panel's behaviour is testable without a desktop **and** without
  touching a real config. Tests inject a fake mutation seam; they never sleep
  and never write a developer's config.
- The config-mutation behaviour is proven against a **real** `ConfigStore` in a
  temporary directory: an add is persisted, validated, atomically written and
  visible to a subsequent load; a refused add leaves the file byte-identical;
  a concurrent write is serialised by the lock; the reload that follows makes
  the new host appear in the document the daemon publishes.
- The pulldown's geometry is proven by construction where it can be (bounded
  heights, clamping to the work area, a panel that cannot be resized to
  nothing) and by native exercise where it cannot.

## Closure gates

- [ ] The framework decision above is recorded with its evidence, and the
      `std-widgets` import is either removed with a smaller measured binary or
      kept with the measurement recorded.
- [ ] The separate monitor window is gone: one tray pulldown carries the
      fleet summary, the host list, the selected host's metrics, drives,
      interfaces, cron, records and `EggPool`.
- [ ] The panel opens from the tray, is positioned within the work area,
      keeps its position and size, and is dismissed by `Esc`, the tray toggle
      and its close control. The absence of click-away dismissal is stated in
      the user documentation.
- [ ] The panel follows the desktop colour scheme through `Palette`, and the
      buttons have hover and pressed states.
- [ ] Add, remove and rename work from the panel against the real config
      path, with the CLI's validation and atomic write, and a refused
      mutation leaves the config byte-identical and shows a visible reason.
- [ ] An endpoint change is presented as a replacement, states that the
      host's identity changes, and is applied only on the operator's explicit
      confirmation.
- [ ] The GUI still opens no HTTP client, still starts no polling plane, and
      the daemon's `DaemonRequest` set is unchanged.
- [ ] The GUI's own CI job still formats, lints, tests and release-builds on
      macOS, Windows and Linux, and the native tray exercises still pass.
- [ ] The host-management path is proven on **all three** targets in CI
      without a desktop session, because it is a config-and-reload path rather
      than a pointer path.
- [ ] The **interactive** native acceptance — a visible notification-area
      icon, the pulldown opening from it, and the panel being usable on a
      real desktop — is recorded per target, or is **left unchecked** and
      blocks shipment for that target, exactly as Plan 190 leaves it.

## Preserved exclusions

- A second polling plane, a second daemon, or any HTTP client in the GUI.
- A protocol change: no new `DaemonRequest`, no protocol-version bump, no
  handshake change.
- Any change to `greggd`'s API, the cron plane, the `EggPool` plane, or the
  daemon's ownership of polling.
- Any change to the TUI's behaviour, its key bindings, or its rendering.
- A new dependency in the GUI workspace; the host-management path uses the
  `gregg` crate it already depends on.
- A new CI job, workflow, matrix, artifact or evidence bundle; the existing
  `gui` job carries the new checks.
- A release asset, installer, bundle, packaging job, or publication step.
- A GUI that becomes an invisible background process with no way to quit.
- Click-away dismissal of the panel, per-platform input hooks, and
  hand-rolled tray icons.

## Stop conditions

- The pulldown cannot be dismissed in a way an operator accepts, and the
  Esc/toggle dismissal is judged unusable — stop and re-scope rather than
  shipping a panel that traps attention.
- Host management turns out to require a protocol change to stay correct —
  stop; a protocol change is a different plan with a different blast radius.
- The panel can only be built at a size that no longer fits the work area of
  a common laptop display — stop and re-scope the surface, because a panel
  that cannot open is worse than a window that can.
- The framework decision above is overturned by evidence (for example a
  Slint release that adds a first-class tray-anchored popup, or a `egui`
  release that ships a native tray) — stop and re-run the review rather than
  reimplementing the frontend under a deadline.

## Closure record

To be completed when the gates above are met, with the native evidence, the
measurements, and an honest statement of what was not performed.
