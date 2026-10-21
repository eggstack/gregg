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
