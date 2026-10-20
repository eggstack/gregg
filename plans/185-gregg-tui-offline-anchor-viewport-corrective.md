# Plan 185: `gregg` TUI scrolls the live fleet off screen when the top host goes down

Status: complete. See the closure record at the end.

> **Superseded in part by Plan 186.** The fix below repairs the viewport
> *anchor*, but the window is kept on the *selection*, and the selection was
> still left on the host that failed. In a pane too small to hold the fleet
> that undid the repair and dragged the window to the bottom of the list —
> the case this plan's tests could not see, because they used a fleet that fit
> on one screen. Plan 186 repairs the selection as well and tightens the
> trigger from "left the live group" to "failed". The analysis and the
> evidence below are accurate for what they covered; the acceptance
> criterion "the live hosts above it stay on screen" was met as written and
> was false in a crowded pane.

Depends on: main at `fbecb81` (the merged Plan 184 page-scroll key bindings),
rebased from `1760ef1` ("gregg, greggd, gregg-update, gregg-protocol: close
the fourth audit pass (Plan 183)"). Independent of Plan 091 and Plans 179-183.

Numbering: this plan was written as Plan 184 and renumbered to 185 on the way
in, because `main` had meanwhile taken Plan 184 for the TUI page-scroll key
bindings (`184-tui-page-scroll-key-bindings.md`). The companion plan was
renumbered with it, to 186.

Opened from an operator report against the `gregg` client TUI: with several hosts
configured, the host at the top of the list going down caused the TUI to scroll
down and cut the other hosts off the screen. The requested behavior was that a
dead host moves to the bottom of the list **without** the scroll moving in a way
that hides the other hosts.

## The failure, exactly

`crates/gregg/src/state.rs` keeps the viewport anchor as `viewport_top_id`: the
*identity* of the first visible system. The list it indexes into,
`display_order_of`, is **reachability-sorted**: online systems first in
configured order, then offline/pending systems in configured order.

Those two facts are only consistent while the anchored host's reachability does
not change. When the anchored host is the one that goes offline, the display
order moves it to the bottom of the list, and the identity anchor follows it
there — taking the whole window with it.

Observed directly, on a three-host fleet in an 80x12 terminal, with the top
host failing:

~~~text
after online:    order=[0, 1, 2]  top=Some("a")  sel=Some("a")
after a offline: order=[1, 2, 0]  top=Some("a")  sel=Some("a")
VISIBLE SYSTEMS = ["a"]
~~~

The dead host is the **only** thing rendered. `b` and `c` are gone from the
screen even though all three rows fit comfortably in twelve lines. The
scroll is not merely unhelpful here: it hides every live host in the fleet.

## Why the existing guards did not catch it

Two guards already exist, and both are real:

- `AppState::adopt_snapshot_inner` pins `selected_id` and `viewport_top_id` to
  `display_order()[0]` on the **first** document that carries polled
  reachability. Its intent is exactly this defect — the comment says an offline
  first-configured system must not "drag the viewport below the online entries
  that came back first" — but it fires **once**, on first reachability.
- `ensure_selected_visible_with_order` keeps the selected system visible. It
  cannot help here: the selected system *is* the anchor, so it is visible by
  construction and the function returns without adjusting anything.

After the first batch, nothing guarded the anchor against its own host being
re-sorted out of the online group. The defect was a *steady-state* one wearing
the first-batch guard's clothes.

## Root cause

`viewport_top_id` is an identity, `display_order` is a position, and the repair
that keeps the viewport coherent (`adopt_snapshot_inner` → identity repair →
`ensure_selected_visible`) only ever checks that the anchor's *id* still
exists. It never checks whether the anchor's *position* still means what it
meant. When the anchor host leaves the online group, its position changes from
"top of the list" to "bottom of the list", and the window silently follows.

The fix has to be in the state, not the renderer: the renderer is handed a
`display_order` and an anchor and faithfully draws the window they describe.
The two must simply never be allowed to disagree in this way.

## Scope

Keep the dead host moving to the bottom of the list, and keep the window on the
same part of the fleet. One scroll model, unchanged. Nothing else.

One unrelated change rode along, and is called out here rather than buried: the
workspace Clippy gate (`-D warnings`) was already **red on `main`** under the
current stable toolchain (1.99.0), because `crates/gregg/src/startup_support.rs`
contained `assert!(bytes.is_empty())`, which trips `clippy::assert_is_empty`.
That line was written by `207c9d4` (2026-10-06) and is unrelated to this plan.
It is now `assert_eq!(bytes, Vec::<u8>::new())` — the same check — so the
blocking CI gate is green and this plan's own verification claim is honest.

## Acceptance criteria

- [x] A host that goes offline moves to the bottom of the display order, and
      the live hosts above it stay on screen.
- [x] The re-anchor happens in the single fleet writer (`AppState::adopt_snapshot`),
      so the renderer, `ensure_selected_visible`, and the state agree on the
      window; no second scroll model is introduced.
- [x] An anchor that merely shifted **up** — because hosts *above* it went
      offline — still names the same top row and is left alone: the window must
      not slide when unrelated hosts change state.
- [x] When the anchor host and another host go offline together, the window
      lands on a live host, never on a dead row.
- [x] A host that was *reconfigured* (its `host:port` changed by a reload) is
      not treated as a failed host: its state was reset on purpose, and the
      window stays on it.
- [x] The dead host is still rendered, at the bottom of the list.
- [x] `cargo fmt`, workspace Clippy, and `./scripts/check-local.sh` are clean.

## Stop conditions

Open a follow-up rather than broadening if the fix would require changing
`display_order_of` (the online-first/offline-last policy is deliberate and
shared with the daemon's `FleetState`), or if it would require moving the
logical selection off a host that went down (selection semantics are a separate
decision, recorded below).

## Handoff

A host that goes down moves to the bottom of the list, and the operator keeps
looking at the fleet they were looking at.

## Closure record

### The fix

`AppState::repair_viewport_anchor` runs inside `adopt_snapshot_inner`, right
after the fleet is replaced and the logical IDs are repaired, and before
`ensure_selected_visible`.

It fires only when the anchored host itself left the live group
(`Online` → not `Online`), because that is the only transition that re-sorts
the anchor out from under the window. The anchor is then re-pointed at the host
that now occupies the anchor's **previous display position**, preferring a live
host at or before that position.

It also fires only when the anchored host's **target is unchanged**. A host
whose `host:port` changed was not "down": a reload reset its state on purpose,
because the old result no longer describes the new target, and the window
belongs to the configured system rather than to the target that went away.
That distinction is not cosmetic — see the correction below.

The anchor's previous position is captured before the fleet is replaced, by
`AppState::anchor_state`, which returns the anchor's id, its reachability, its
index in the *old* display order, and the target it was polled at. Capturing
it in the old order is what makes the repair meaningful: the new position of
the anchor says only that it moved, not what the window used to be showing.

Re-anchoring to the host at the old position — rather than to the first live
host — keeps the operator's visible *set* stable, not just the top row. On a
fleet `[a, b, c, d]` with the window on `b`, a failure of `b` re-sorts to
`[a, c, d, b]`; the old position 1 now holds `c`, so the window becomes
`{c, d}` — the same two slots it showed before, minus the host that died.

### Why not simply "the first live host"

That would also fix the reported symptom, but it scrolls the window *up* when
the operator had deliberately scrolled down, which trades one surprise for
another. The old-position rule keeps the window where it was.

### Why not a position anchor

Storing the anchor as a display index instead of an identity would make the
window slide by one row whenever any host above it changed state, because the
list re-sorts. The identity anchor is right; it just needed to be repaired
when — and only when — the identity it names was itself re-sorted.

### A correction this plan's own first implementation needed

The first implementation fired on `Online` → not `Online` alone, and it broke
`state::tests::reconcile_systems_replaces_targets_preserves_unchanged_state_and_repairs_ids`:
the window moved from `changed` to `same`.

The cause was the same word doing two jobs. `Online` → `Pending` is *not* only
a failure: a reload that repoints a host at a different `host:port` resets that
host's state to `Pending`, because the old result no longer describes the new
target. The reloaded host is not down, and it is still the system the operator
was looking at.

So the repair additionally requires the anchored host's **target to be
unchanged**. A host that is reconfigured is not re-sorted out from under the
window; it was deliberately reset, and the window stays on the configured
system. The existing test is the evidence for that distinction, and a new
regression test (`reconfigured_anchor_host_keeps_the_window`) now pins it.

### The selection is deliberately left alone

`selected_id` stays on the host that went down, so the operator can still see
its offline reason and last-known detail. This interacts with the documented
"selected system is always visible" rule: on a fleet that does **not** fit on
one screen, `ensure_selected_visible` will still scroll down to the selected
dead host. That is the pre-existing selection rule doing its job, not the
anchor defect, and changing it would alter what `d`/`n`/`c` act on. It is
recorded here rather than folded into this plan.

### Evidence

Mutation-verified in the repo's usual sense: the pre-fix behavior was
reinstated in place (`repair_viewport_anchor` made a no-op), and the new
regression tests were observed **failing with the recorded symptom** before
the fix was restored.

- `state::tests::offline_anchor_does_not_drag_the_viewport_to_the_bottom` —
  failed with `left: Some("a"), right: Some("b")`: the anchor stayed on the
  dead host.
- `state::tests::offline_anchor_reanchors_onto_a_live_host` — failed.
- `ui::tests::offline_top_host_does_not_hide_the_other_hosts` — failed with
  the operator's exact symptom on screen: the dead host alone, and every other
  host cut off.

  ~~~text
  a@host0.local:11310 offline (refused) ................
  (every other row blank)
  ~~~

- The two guard tests — `offline_host_above_the_anchor_does_not_slide_the_window`
  and `reconfigured_anchor_host_keeps_the_window` — **passed** against the
  pre-fix logic, which is correct: they assert that nothing moves, so they
  must not depend on the repair.

With the fix restored:

- `cargo fmt --all -- --check` — clean.
- `cargo test -p gregg --all-features` — green, including the five new
  regression tests.
- `cargo test --workspace --all-targets --all-features` — green.
- `./scripts/check-local.sh` — `=== all checks passed (mode: default) ===`.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  clean.

### Regression tests

- `state::tests::offline_anchor_does_not_drag_the_viewport_to_the_bottom` —
  the reported defect, asserted at the viewport level: the dead host sorts to
  the bottom, the anchor moves to the live host that took its place, and all
  three hosts remain in the computed viewport.
- `state::tests::offline_host_above_the_anchor_does_not_slide_the_window` —
  the guard against over-correcting: a failure *above* the anchor leaves the
  anchor alone.
- `state::tests::offline_anchor_reanchors_onto_a_live_host` — the anchor and a
  neighbour fail together; the window lands on the live host, not on a dead
  row.
- `state::tests::reconfigured_anchor_host_keeps_the_window` — a reload that
  repoints the anchored host resets it on purpose; the window stays on it.
- `ui::tests::offline_top_host_does_not_hide_the_other_hosts` — the same
  defect asserted end to end through the renderer, so the fix is proven at the
  level the operator actually sees.
