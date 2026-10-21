# gregg-gui — the optional Slint desktop frontend

A desktop GUI for Gregg, built on [Slint](https://slint.dev), that attaches to
the same per-config client daemon the TUI attaches to. It is a **second way to
look at the fleet**, not a second poller: both frontends share one poll
budget, one set of reducers and one local protocol.

This crate is **experimental and optional**. Nothing in the root workspace
depends on it, and the terminal client is unaffected by its existence.

## Status

Plans 188 and 189 are implemented. What exists:

- an independent Cargo workspace with its own lockfile;
- a Slint UI definition (`src/ui/main.slint`) with a `Window` and a
  `SystemTrayIcon` as separate top-level components;
- a bridge (`src/bridge.rs`) that owns the client-daemon attachment, reads the
  real local protocol, and publishes the real reducer's output;
- a **pure projection** (`src/model.rs`) from a daemon document to a typed view
  model, with the truthfulness rules (absent is absent, unpolled is not
  offline, selection is keyed by stable id) testable without a desktop session;
- a **native system-tray entry** with a fleet summary, Open Monitor, Reload,
  About Gregg and Quit Gregg;
- a **compact monitor window** showing the fleet, the selected host's metrics,
  drives and interfaces, its scheduler jobs and records, and the `EggPool`
  plane;
- an entry point (`src/main.rs`) that wires the two together and runs the
  Slint event loop.

Plan 190 owns native cross-platform qualification, the footprint measurement
and the packaging decision.

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
  handful of host names), then Open Monitor, Reload, About Gregg and Quit
  Gregg. The full list belongs in the window, not in a native menu.
- On a desktop with **no tray service**, the monitor window is still shown on
  launch, so the GUI is never an invisible background process with no way to
  quit.

### Actions

| Where | Action | Effect |
| ----- | ------ | ------ |
| Tray | Open Monitor | Brings the monitor window back to the front. |
| Tray | Reload | The `Ctrl-R` boundary: the daemon re-reads its configuration and polls immediately. |
| Tray | About Gregg | Shows the licence attribution in the window. |
| Tray | Quit Gregg | Ends the GUI. The client daemon keeps running. |
| Window | Click a host row | Selects that host. The selection is keyed by the stable system id, so a host that reorders does not move the selection. |
| Window | Click a scheduler job | Opens that job's retained records. Only an open job asks the daemon for history; closing the view releases the intent. |
| Window | Reload / EggPool / About / Quit buttons | The same actions as the tray items. The `EggPool` button opens and closes the `EggPool` plane, and the intent is sent only while it is open. |
| Window | Close button | **Hides** the window. The tray and the observation keep running. |
| Window | Quit button | Ends the GUI. The client daemon keeps running. |

The GUI is mouse-driven; there are no keyboard shortcuts in this MVP. Every
action is reachable by pointer, and the window never becomes the only way to
quit.

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
| `gregg-gui` (this crate, with the `gregg` core library) | 9,776,560 bytes (9.3 MiB) |
| Slint-only baseline (same Slint features, same `main.slint`, same activation code, no `gregg`) | 8,929,216 bytes (8.5 MiB) |
| **Difference** | **847,344 bytes (~828 KiB, +9.5%)** |

So the Slint GUI itself costs about 8.5 MiB stripped, and everything
`gregg-gui` adds on top of a pure-Slint build of the same UI — the `gregg`
core library with Ratatui, Crossterm, Clap, `eggfetch-core` with rustls and
the config and reducer machinery, plus the GUI's own projection, bridge and
wiring — is about 828 KiB.

(For comparison, Plan 188 measured the same drag against the *scaffold* UI,
before the monitor window existed: 747,936 bytes over an 8,813,296-byte
baseline. The GUI grew on both sides, because the window grew.)

(The `libgregg-*.rlib` in `target/release/deps/` is 18.2 MiB, but that is an
unlinked archive with debug-free but unoptimised object code and heavy
generics; it is not a proxy for what the linker keeps.)

## Measured runtime cost

Measured on macOS (Apple M4 Pro) against a 25-host synthetic fleet, with the
measurement window aligned to the daemon's own start so every configuration is
sampled over the same phase of the poll cadence. CPU is `getrusage`, and
"wakeups" is the count of voluntary context switches — the observable proxy
for a sleep/wake cycle.

| configuration | remote requests | daemon RSS | daemon CPU | daemon wakeups | GUI RSS | GUI CPU | GUI wakeups |
| --- | --- | --- | --- | --- | --- | --- | --- |
| daemon alone | 414 | 7.2 MB | 0.111 s | 8,279 | — | — | — |
| daemon + GUI, tray only | **414** | — | 0.157 s | 11,471 | 73–83 MB | 0.491 s | 22,884 |
| daemon + GUI, monitor visible | **414** | — | 0.154 s | 11,333 | 72–82 MB | 0.476 s | 21,985 |

Over a ~73-second lifetime the GUI costs **about 0.65% of one core**, whether
the window is hidden or visible. The remote request count is **identical** in
all three configurations: the GUI adds nothing to the fleet's budget.

Two things worth reading carefully:

- **The daemon's wakeup delta is the documented one.** 8,279 alone → ~11,400
  with a frontend attached is about 44 extra wakeups per second, which is the
  "one timer wake per attached frontend every 20 ms" that `clientd/daemon.rs`
  documents. The GUI adds one more frontend to a cost the architecture
  already accepted; it does not add a new kind of cost.
- **The memory is Slint's, not the protocol's.** Hiding the monitor window
  saves under a megabyte. The rest is the software renderer and the
  system-font machinery.

The measurements are noisy — RSS at the moment of sampling varied between
72 MB and 83 MB for the same configuration — so the numbers above are ranges,
not constants. See `EVIDENCE.md` for the deterministic request-count proof.

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
│   ├── main.rs           # entry point: window, tray, wiring, the render push
│   ├── bridge.rs         # the client-daemon attachment, the intents, the frame pump
│   ├── model.rs          # the pure FrontendSnapshot -> GuiViewModel projection
│   ├── platform.rs       # the per-desktop bits: macOS application activation
│   └── ui/
│       └── main.slint    # the Window and the SystemTrayIcon
└── vendor/
    ├── VENDORED-PATCH.md # why the vendored crate exists
    └── annotate-snippets/
```

### The three layers

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
- There is **no built-in `Button`**. This file defines a small
  `ActionButton` from `Rectangle` + `Text` + `TouchArea`.
- `Image::from_rgba8` takes a `SharedPixelBuffer<Rgba8Pixel>` (not a size and
  a byte vector), and `Timer` is created with `Timer::default()` and started
  with `timer.start(mode, interval, callback)`.
