# gregg-gui — the optional Slint desktop frontend

Plan 188 Part B: a desktop GUI for Gregg, built on [Slint](https://slint.dev),
that attaches to the same per-config client daemon the TUI attaches to. It
is a **second way to look at the fleet**, not a second poller: both frontends
share one poll budget, one set of reducers and one local protocol.

This crate is **experimental and optional**. Nothing in the root workspace
depends on it, and the terminal client is unaffected by its existence.

## Status

Part B only. What exists:

- an independent Cargo workspace with its own lockfile;
- a Slint UI definition (`src/ui/main.slint`) with a `Window` and a
  `SystemTrayIcon` as separate top-level components;
- a bridge (`src/bridge.rs`) that owns the client-daemon attachment, reads the
  real local protocol, and publishes the real reducer's output;
- an entry point (`src/main.rs`) that wires the two together and runs the
  Slint event loop.

What does **not** exist yet (Plan 189): the full fleet rendering, selection,
the `EggPool` pane, the cron pane, the About/attribution surface, and the
explicit-daemon-executable launch (there is a marked `TODO` in
`src/bridge.rs`).

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

## Measured cost

Release builds, `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"` —
the same profile the root workspace uses, so the numbers are comparable with
the TUI's.

| Binary | Size |
| ------ | ---- |
| `gregg-gui` (this crate, with the `gregg` core library) | 9,561,232 bytes (9.1 MiB) |
| Slint-only baseline (same Slint features, same `.slint` file, no `gregg`) | 8,813,296 bytes (8.4 MiB) |
| **Difference** | **747,936 bytes (~731 KiB, 7.8 %)** |

So the Slint GUI itself costs about 8.4 MiB stripped, and the whole `gregg`
core library — Ratatui, Crossterm, Clap, `eggfetch-core` with rustls, the
config and reducer machinery — adds about 731 KiB on top of it. That is the
number Plan 188 wanted measured instead of guessed.

(The `libgregg-*.rlib` in `target/release/deps/` is 18.2 MiB, but that is an
unlinked archive with debug-free but unoptimised object code and heavy
generics; it is not a proxy for what the linker keeps.)

## Licence

Gregg is MIT. This crate is MIT, and it adds:

- **Slint** (`slint`, `slint-build`, `i-slint-*`) — licensed
  `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`.
  **The path chosen here is the royalty-free
  `LicenseRef-Slint-Royalty-free-2.0`**, not the GPL path. Slint's
  royalty-free desktop licence requires attribution: an "About Slint" entry in
  the application, or a badge linking to the Slint download page. Building
  that attribution surface is a **Plan 189 deliverable** — it is not
  implemented in this part, and the GUI must not ship without it.
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
│   ├── main.rs           # entry point: window, tray, timer, wiring
│   ├── bridge.rs         # the client-daemon attachment and the frame pump
│   └── ui/
│       └── main.slint    # the Window and the SystemTrayIcon
└── vendor/
    ├── VENDORED-PATCH.md # why the vendored crate exists
    └── annotate-snippets/
```

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
