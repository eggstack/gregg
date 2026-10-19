# Plan 184: TUI page-scroll key bindings

Status: complete. See the closure record at the end.

Depends on: current main at `1760ef12f6db358afde33fc3d38e331070c8cd9e`.
Independent of Plan 091 and Plans 179-183.

## Objective

Give the Systems pane a second, always-available way to page through a long
host list, so an operator who never touches the dedicated paging keys can still
scroll a fleet of hosts without reaching for them.

`Ctrl-U` pages up and `Ctrl-D` pages down. They are aliases of the existing
`PageUp` / `PageDown` actions, not a second scroll model: the same
`Action::PageUp` / `Action::PageDown` reducer, the same viewport fix-up, and
the same Systems-pane gating apply.

## Why an alias is the whole change

The paging behavior already exists and is already correct:

- `Action::PageUp` / `Action::PageDown` move the selection by one computed
  viewport page (`AppState::page_size`), then call
  `ensure_selected_visible_with_order` so the viewport follows the selection;
- `PageUp` / `PageDown` (and their `f` / `b` aliases) are already bound in
  `crates/gregg/src/event.rs::key_to_action`;
- the action is a no-op outside the Systems pane, exactly like the paging keys
  that already exist.

What is missing is only the `Ctrl-U` / `Ctrl-D` spelling. The `ctrl` branch of
`key_to_action` currently answers `Ctrl-C` (quit) and `Ctrl-R` (reload) and
returns `None` for everything else, so both modified keys are silently
unmapped today — an unmapped key is ignored without a redraw, which is exactly
the "the key does nothing" experience this plan removes.

Adding an alias is deliberately the *only* production change. A second scroll
state, a separate viewport offset, a "scroll without selecting" mode, or a
new `Action` variant would each be a new way to be wrong, and the repository
already records that there is one scroll model.

## Scope decisions

- `Ctrl-U` maps to `Action::PageUp` and `Ctrl-D` maps to
  `Action::PageDown` — the same actions the dedicated paging keys produce.
- The bindings live in the existing `ctrl` branch of `key_to_action`, beside
  `Ctrl-C` and `Ctrl-R`, so the whole modified-key surface stays in one place.
- `Ctrl-U` / `Ctrl-D` with `alt` held stay unmapped, matching the existing
  `ctrl`+`alt` refusal and the `alt` refusal for unmodified keys.
- The bindings are pane-scoped exactly as the action is: on the EggPool pane
  they are a no-op, like `PageUp` / `PageDown` / `f` / `b` are today.
- No new `Action` variant, no new `Key` variant, no new dependency, no
  renderer change, no state change, no daemon or protocol change.

## Deliberate non-goals

- Not a "scroll without moving the selection" mode. The selection and the
  viewport are one model here.
- Not a half-page or line-wise scroll. `Ctrl-U` / `Ctrl-D` are spelled as
  page aliases, and half-page scrolling would be a second page size.
- Not a configurable keymap. A fixed, discoverable set is what this client has
  always had.
- Not a change to the one-line key hint in the TUI footer. The hint is
  width-tiered and already saturates a narrow terminal; the bindings are
  documented in `README.md`, `docs/client.md`, the crate README, the
  architecture deep dive, and the `gregg-client` skill.

## Acceptance criteria

- [x] `Ctrl-U` translates to `Action::PageUp` and `Ctrl-D` translates to
      `Action::PageDown`, proven by deterministic unit tests on
      `key_to_action`.
- [x] The unmodified `d` (toggle drives), `u`-family, and `Ctrl-D` spellings
      are not confused: unmodified `d` still toggles drive expansion and
      unmodified `u` stays unmapped.
- [x] `Ctrl-U` / `Ctrl-D` with `alt` held stay unmapped.
- [x] The dedicated `PageUp` / `PageDown` / `f` / `b` bindings are unchanged.
- [x] Applying the action through the real reducer moves the Systems selection
      by one viewport page, so the alias is proven end-to-end and not only at
      the translation layer.
- [x] `README.md`, `crates/gregg/README.md`, `docs/client.md`,
      `architecture/gregg-client.md`, the `gregg-client` skill, and
      `CHANGELOG.md` describe the new bindings in the same pass.
- [x] `cargo fmt --all -- --check`, workspace Clippy with `-D warnings`,
      workspace tests, and `./scripts/check-local.sh` pass.

## Preserved exclusions

- The `Ctrl-R` config-reload boundary and the `Ctrl-C` quit binding.
- The one scroll model, the mixed-height viewport, the transient selection
  highlight, and the ten-second highlight deadline.
- Client-daemon IPC, cron polling and rendering, EggPool behavior, the
  scheduler, the collectors, the wire protocol, and the release pipeline.
- Plan 091's soak record.
- New CI jobs, workflows, or matrices.

## Closure record

Implementation `540b56234dfd0d470948f268aa9f9e3788028854`. Client-side feature
in `crates/gregg/src/event.rs`; no state, renderer, daemon, protocol, or
dependency change.

### The alias, not a second scroll model

The production change is two match arms in the `ctrl` branch of
`key_to_action`:

```rust
Key::Char('u') => Some(Action::PageUp),
Key::Char('d') => Some(Action::PageDown),
```

They produce the *same* actions `PageUp`/`PageDown` (and their `f`/`b`
spellings) already produce, so the reducer, the computed viewport page
(`AppState::page_size`), the viewport fix-up
(`ensure_selected_visible_with_order`), the transient selection highlight, and
the Systems-pane gating are all unchanged. There is no new `Action` variant,
no new `Key` variant, no second page size, and no scroll-without-selecting
mode — the repository already records that the client has one scroll model, and
a second offset would be a second way to be wrong.

`Ctrl-U`/`Ctrl-D` with `alt` held stay unmapped, matching the pre-existing
`ctrl`+`alt` refusal and the `alt` refusal for unmodified keys.

### One existing test changed, and why

`event::tests::modified_view_controls_are_unmapped` asserted that `Ctrl-D` was
unmapped. That assertion *is* the mapping this plan deliberately replaces, so
it could not survive unchanged. It now asserts that `Ctrl-D` is the page-down
alias, that every other modified view control (`h`, `l`, `Left`, `Right`, `e`)
stays unmapped under ctrl, and — new coverage — that `Ctrl-Alt` stays refused
for `d` and `e`. Every other assertion in that test is retained.

### The five new tests

| Test | What it locks |
|---|---|
| `event::tests::key_to_action_ctrl_u_and_ctrl_d_page` | `Ctrl-U` is `PageUp`, `Ctrl-D` is `PageDown` |
| `event::tests::ctrl_page_aliases_are_the_same_actions_as_the_paging_keys` | The alias and the dedicated key produce the same action |
| `event::tests::ctrl_page_aliases_do_not_shadow_the_unmodified_keys` | Plain `d` still toggles drives; plain `u` stays unmapped |
| `event::tests::ctrl_page_aliases_with_alt_are_unmapped` | `Ctrl-Alt` is refused |
| `state::tests::ctrl_page_aliases_move_the_selection_by_one_page` | End-to-end: the real key event is translated and the real action applied, and the Systems selection really pages |

The end-to-end test is the one that matters most: a mapping that stopped at
the action boundary would still pass the four translation-level tests.

### Mutation check

Removing the two match arms fails three of the five new tests in 0.00 s:
`key_to_action_ctrl_u_and_ctrl_d_page`,
`ctrl_page_aliases_are_the_same_actions_as_the_paging_keys`, and
`state::tests::ctrl_page_aliases_move_the_selection_by_one_page`. The two
remaining new tests pass either way *by design* — they are invariant guards
(the unmodified keys, and the `Ctrl-Alt` refusal) whose property holds with or
without the alias — so they are recorded as guards, not as proofs of the
binding.

### Verification

- `cargo fmt --all -- --check` clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` clean.
- `./scripts/check-local.sh` green: measured against the rebased base
  `1760ef1`, the `gregg` crate's tests went 955 → 960 (the five new tests),
  and the workspace suite went 1762 → 1767 passed, 0 failed, 2 ignored.
- The manual TUI checklist (`plans/tui-manual-tests.md`) gained a "Paging
  Through a Long Host List" section for the operator-side confirmation that a
  unit test cannot give.

No CI run is claimed here: the change adds no platform-specific code, wire
change, or CI infrastructure, so the default local check plus the workspace
Clippy/test gate is the lightest appropriate mechanism under this repository's
completion rule. The ordinary workflow on the pull request remains the
platform truth for the existing jobs.

### The branch also carries two pre-existing corrective fixes

`d0d24c9` is not part of this plan and is not scoped by it. It clears the two
clippy failures that `207c9d4` left on `main`, which are why CI run
`37731156030` on `1760ef1` is red and why every push to `main` since has been
red: `clippy::assert_is_empty` in `crates/gregg/src/startup_support.rs` on the
Linux job, and `unused_variables` in `crates/gregg-update/src/stage.rs` on
the Windows job. Both are one-line, test-only changes with no production
behavior, no protocol, and no dependency change. They are recorded here
because this branch cannot demonstrate a green Clippy gate without them —
the closure claim above is "clippy is clean on this tree", not "clippy was
already clean before this branch".

### Preserved

`Ctrl-R` is still the only config-reload boundary and `Ctrl-C` still quits.
The one-line key hint in the TUI footer is unchanged: it is width-tiered and
already saturates a narrow terminal, so the new spellings are documented in
`README.md`, `docs/client.md`, the crate README, the architecture deep dive,
and the `gregg-client` skill instead.
