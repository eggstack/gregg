# Vendored patches

This directory holds two vendored copies of crates.io crates. Each section
below says what the crate is, why the graph or the product needs the vendored
copy, exactly what was changed, and how to remove it.

| Crate                | Version | Why it is vendored                                    |
| -------------------- | ------- | ----------------------------------------------------- |
| `annotate-snippets`  | 0.12.9  | the dependency graph cannot otherwise be resolved       |
| `softbuffer`         | 0.4.8   | macOS drops the framebuffer's alpha channel without it |

Both are MIT OR Apache-2.0 licensed, which the root workspace's `deny.toml`
already allows.

---

# Vendored patch: `annotate-snippets` 0.12.9

This directory is a vendored copy of
[`annotate-snippets`](https://crates.io/crates/annotate-snippets) 0.12.9
(MIT OR Apache-2.0), used only because the `apps/gregg-gui` dependency graph
cannot otherwise be resolved.

## The conflict

`apps/gregg-gui` depends on two things that disagree about `unicode-width`:

| Crate                     | Requirement        | Why it is in the graph                                  |
| ------------------------- | ------------------ | ------------------------------------------------------- |
| `ratatui` 0.29            | `unicode-width =0.2.0` (exact) | pulled in by the `gregg` core library, which this app links against |
| `annotate-snippets` 0.12.9 | `unicode-width ^0.2.2` | pulled in by Slint's build-time compiler (`i-slint-compiler`'s `display-diagnostics` feature) |

Cargo keeps at most one version of a crate per semver-compatibility range.
`0.2.0` and `0.2.2` are in the same range (`0.2.x`), so the two requirements
have to be satisfied by a single version — and no such version exists. The
result is a resolution failure for the whole workspace:

```
error: failed to select a version for `unicode-width`.
    ... required by package `annotate-snippets v0.12.9`
    ... which satisfies dependency `annotate-snippets = "^0.12.9"` of package `i-slint-compiler v1.18.1`
    ... which satisfies dependency `i-slint-compiler = "=1.18.1"` of package `slint-build v1.18.1`
    ... which satisfies dependency `slint-build = "=1.18.1"` of package `gregg-gui v0.1.0`
versions that meet the requirements `^0.2.2` are: 0.2.2

all possible versions conflict with previously selected packages

  previously selected package `unicode-width v0.2.0`
    ... which satisfies dependency `unicode-width = "^0.2"` of package `gregg v1.0.15`
```

## The patch

The vendored copy relaxes one bound:

```toml
[dependencies.unicode-width]
version = "0.2"   # upstream: "0.2.2"
```

With that bound, every requirement in the graph agrees on `unicode-width`
0.2.0, and the graph resolves.

### Why this is safe

The only `unicode-width` API this crate uses is
`UnicodeWidthChar::width(ch) -> Option<usize>`, at
`src/renderer/render.rs`:

```rust
_ => unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1),
```

That method exists, with exactly this signature, in unicode-width 0.2.0 —
the Option-returning `width` was introduced in 0.2.0. Upstream raised the
bound to 0.2.2 for freshness, not for a new API: 0.2.1 and 0.2.2 only added
a `rust-version` field and dropped the `compiler_builtins` dependency of the
`cjk` feature.

No source file of the vendored crate is modified.

## Other changes to the vendored copy

All of them are to the crates.io-normalized `Cargo.toml`; the `src/` tree is
byte-identical to upstream 0.12.9.

1. `unicode-width` relaxed from `"0.2.2"` to `"0.2"` — the patch above.
2. The `[[example]]` targets and `[dev-dependencies]` removed. This copy is
   only ever built as a library dependency of the Slint compiler; the
   examples exist upstream to demonstrate the renderer and would drag
   `anstream`, `divan` and `snapbox` into the graph for nothing.
3. The `[profile.*]` tables removed. Cargo ignores profiles declared in a
   non-root package and warns about them.
4. An empty `[workspace]` table added, so this copy is its own workspace
   root and is not pulled into the `apps/gregg-gui` workspace as a member.
5. The `examples/` directory is kept, because `src/snippet.rs` uses
   `#[doc = include_str!("../examples/...")]` and those includes are
   resolved at compile time.

## Alternatives that were considered and rejected

- **Bump `ratatui`.** No ratatui release relaxes the pin: 0.29.0,
  0.29.1-alpha.0 and every 0.30.0-alpha require `unicode-width =0.2.0`.
- **Vendor `slint-build` instead and drop the compiler's
  `display-diagnostics` feature.** That removes `annotate-snippets` from the
  graph, but `slint-macros` enables the same feature, so `slint-macros`
  would have to be vendored and patched too — two vendored crates and three
  edited call sites instead of one, and it costs the Slint compiler its
  pretty diagnostic rendering.
- **Vendor `unicode-width`.** Impossible: the patched version would have to
  satisfy both `=0.2.0` and `^0.2.2` at once.
- **Drop the `gregg` library dependency.** This would remove `ratatui` from
  the graph entirely, but the plan is explicit that the GUI prototypes
  against the real core library, and the GUI is supposed to show the same
  data the TUI shows.

## Removing the patch

If the `gregg` core library ever stops depending on `ratatui` 0.29 (or
`ratatui` ever relaxes its `unicode-width` pin), delete this directory and
the `[patch.crates-io]` section of `../Cargo.toml`.

---

# Vendored patch: `softbuffer` 0.4.8

This is a vendored copy of
[`softbuffer`](https://crates.io/crates/softbuffer) 0.4.8 (MIT OR
Apache-2.0). `softbuffer` is not a new dependency: Slint's winit backend
already depends on it to present the software renderer's frames, and it was
already in this workspace's dependency graph before the copy was made. Only
the **macOS** presentation path is changed, and only by one word.

## The defect

On macOS a frameless Slint window is a transparent `NSWindow`: `winit` asks
for it explicitly (`NSWindow.opaque = false`, `backgroundColor = clear`), and
`i-slint-backend-winit` asks for transparency exactly when the window's
background is not opaque or the window has no frame. The window server
honours that — the window really is transparent, and `CGWindowListCreateImage`
reports the unpainted region as alpha 0.

The **framebuffer** that carries the window's pixels is the problem.
`softbuffer`'s macOS backend hands CoreGraphics a `CGImage` whose pixel format
is declared as:

```rust
CGImageAlphaInfo::NoneSkipFirst
```

`NoneSkipFirst` means *there is no alpha channel at all*: the first component
of each 32-bit pixel is an unused filler byte. So although the software
renderer writes a correct premultiplied alpha into that byte, CoreGraphics
throws it away and presents every pixel opaque.

The visible consequence is exactly the defect this patch exists to fix: the
pulldown's shadow margin — the 14 px around the card — is transparent in the
framebuffer, so it is presented as **solid black**. The pulldown wore a thick
black frame instead of a soft shadow, and its rounded corners were black
squares.

## The patch

One word, in `src/backends/cg.rs`:

```rust
CGImageAlphaInfo::PremultipliedFirst   // upstream: CGImageAlphaInfo::NoneSkipFirst
```

`PremultipliedFirst` declares the **same 32-bit layout** with the alpha
channel present. "First" is the most significant byte of the pixel word, which
for `CGBitmapByteOrderInfo::Order32Little` is the last byte in memory — which
is exactly where this backend puts the alpha: the buffer is packed as
`alpha << 24 | red << 16 | green << 8 | blue`. The colour components are
already premultiplied by the renderer that fills the buffer, which is what
`PremultipliedFirst` expects.

Nothing else about the presentation path changes: the pixel layout, the byte
order, the blending, the `CGColorSpace` and the layer's `contents` are
identical. The only difference is that the alpha survives the trip to the
screen.

### Why this is safe

- The change is inside `#[cfg(target_vendor = "apple")]` code. The X11,
  Wayland, KMS, Win32, Android, Orbital and web backends are byte-identical to
  upstream 0.4.8, so Linux and Windows are unaffected.
- The declared format now *matches the data*. Upstream was describing a
  buffer that carries alpha as one that does not, which is a
  representation bug, not a deliberate choice: an alpha-carrying framebuffer
  presented as opaque is wrong for every consumer, not only for a window that
  wants transparency.
- An opaque window is unaffected in appearance. When the framebuffer is fully
  opaque, premultiplied and straight alpha are the same values.

### How the fix was verified

The behaviour was measured, not inferred:

- A purpose-built AppKit probe presented the **same** premultiplied RGBA
  bytes under both declarations. `noneSkipFirst` produced an opaque black
  border (`alpha = 255`); `premultipliedFirst` produced a transparent one
  (`alpha = 0`). The window's own pixels were read back with
  `CGWindowListCreateImage`.
- The pulldown itself was captured with `screencapture -l`, which reports the
  window's own alpha. Before the patch the 14 px margin read
  `(0, 0, 0, 255)`; after it, `(0, 0, 0, 0)`, and the shadow rings became
  genuinely translucent.
- The composited result was measured over a known light background: the
  shadow ramps smoothly from 17 % darkening at the card's edge to nothing 14 px
  out, with no banding.

## Other changes to the vendored copy

All of them are to the crates.io-normalized `Cargo.toml`; the `src/` tree is
byte-identical to upstream 0.4.8 **except** for `src/backends/cg.rs`.

1. `src/backends/cg.rs` — the alpha declaration described above. The file
   carries the rationale in a comment at the patch site.
2. The `[[bench]]` target and every `[dev-dependencies]` table removed. This
   copy is only ever built as a library dependency of Slint's winit backend,
   and cargo refuses to load a manifest whose declared target file is missing.
3. `include` replaces `exclude`, so the vendored copy packages only the files
   this crate actually builds from.
4. An empty `[workspace]` table added, so this copy is its own workspace root
   and is not pulled into the `apps/gregg-gui` workspace as a member. That
   also keeps it out of the GUI workspace's `cargo fmt` and `cargo clippy`
   gates, which is correct: it is upstream's code, not this project's.

## Alternatives that were considered and rejected

- **Move the GUI to a GPU renderer.** Slint's femtovg and Skia renderers do
  present transparency, but enabling either adds a GPU stack to a build that
  is deliberately software-only, and it is a much larger change than the
  defect deserves.
- **Make the pulldown's window opaque and drop the shadow.** Without an alpha
  channel there is no shadow and no rounded corner — the window would be a
  square rectangle. That trades a cosmetic defect for a worse one.
- **Patch the window instead of the framebuffer.** The `NSWindow` is already
  transparent and needs no change; only the pixels it is handed were wrong.
- **Present the frame ourselves.** Building a `CGImage` with alpha in the GUI
  and setting it on the layer by hand would mean re-implementing the whole
  presentation path, in `unsafe` AppKit code, inside the application. It
  would be far more code and far more risk than the one word upstream needs.

## Removing the patch

When `softbuffer` releases a version whose macOS backend declares the
framebuffer's alpha channel, delete `vendor/softbuffer` and its
`[patch.crates-io]` entry in `../Cargo.toml`. The pulldown's shadow and
rounded corners depend on it, so check the release notes for the alpha
declaration before removing the copy.
