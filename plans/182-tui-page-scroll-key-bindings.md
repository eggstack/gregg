# Plan 182: TUI page-scroll key bindings

Status: in progress. See the closure record at the end.

Depends on: current main at `17ad1a7`.
Independent of Plan 091 and Plans 179-181.

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

- [ ] `Ctrl-U` translates to `Action::PageUp` and `Ctrl-D` translates to
      `Action::PageDown`, proven by deterministic unit tests on
      `key_to_action`.
- [ ] The unmodified `d` (toggle drives), `u`-family, and `Ctrl-D` spellings
      are not confused: unmodified `d` still toggles drive expansion and
      unmodified `u` stays unmapped.
- [ ] `Ctrl-U` / `Ctrl-D` with `alt` held stay unmapped.
- [ ] The dedicated `PageUp` / `PageDown` / `f` / `b` bindings are unchanged.
- [ ] Applying the action through the real reducer moves the Systems selection
      by one viewport page, so the alias is proven end-to-end and not only at
      the translation layer.
- [ ] `README.md`, `crates/gregg/README.md`, `docs/client.md`,
      `architecture/gregg-client.md`, the `gregg-client` skill, and
      `CHANGELOG.md` describe the new bindings in the same pass.
- [ ] `cargo fmt --all -- --check`, workspace Clippy with `-D warnings`,
      workspace tests, and `./scripts/check-local.sh` pass.

## Preserved exclusions

- The `Ctrl-R` config-reload boundary and the `Ctrl-C` quit binding.
- The one scroll model, the mixed-height viewport, the transient selection
  highlight, and the ten-second highlight deadline.
- Client-daemon IPC, cron polling and rendering, EggPool behavior, the
  scheduler, the collectors, the wire protocol, and the release pipeline.
- Plan 091's soak record.
- New CI jobs, workflows, or matrices.
