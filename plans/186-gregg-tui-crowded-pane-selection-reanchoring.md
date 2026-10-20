# Plan 186: `gregg` TUI still scrolls the live fleet off screen in a crowded pane

Status: complete. See the closure record at the end.

Depends on: Plan 185 (`185-gregg-tui-offline-anchor-viewport-corrective.md`),
which is complete but was scoped too narrowly. This plan supersedes it for the
crowded-pane case and corrects one of its triggers.

Numbering: these two plans were written as 184 and 185 and renumbered to 185
and 186 on the way in, because `main` had meanwhile taken Plan 184 for the
TUI page-scroll key bindings (`184-tui-page-scroll-key-bindings.md`). Both
plans were rebased onto that merge.

Opened from an operator follow-up on Plan 185: the TUI lives inside a crowded
tmux environment and takes up a small amount of space, so a fleet of eight
hosts may only show three rows at a time. Plan 185's fix was verified against a
fleet that *fit* on one screen, which is the unusual case for that operator.

## The failure, exactly

Plan 185 repaired the viewport **anchor**. It did not repair the viewport
**selection**, and the window is kept on the selection. So the two rules fought
each other, and the older one won.

Measured on an eight-host fleet in a three-row condensed pane, with the host at
the top of the list failing:

~~~text
[fits ] a-offline: order=[b,c,d,e,f,g,h,a] sel=Some("a") top=Some("b") VISIBLE=[b,c,d,e,f,g,h,a]
[small] a-offline: order=[b,c,d,e,f,g,h,a] sel=Some("a") top=Some("g") VISIBLE=[g,h,a]
~~~

The `fits` line is Plan 185's world: the whole fleet fits, so the window can
hold every row and nothing is hidden. The `small` line is the operator's world:
the window was dragged to `g,h,a` — the **bottom** of the list — and the live
hosts `b` and `c` the operator was looking at left the screen.

The reported symptom survived. Only its shape changed.

### Why Plan 185's tests could not see it

Plan 185's regression tests used a 24-row terminal and a three-host fleet, so
the entire fleet fit on one screen at all times. A window that can hold every
row never hides anything, so those tests could not distinguish a fix from a
no-op on the thing that actually matters. The plan's acceptance criterion — "a
host that goes offline moves to the bottom of the display order, and the live
hosts above it stay on screen" — was **met as written and false in practice**.

The plan's own stop condition anticipated a follow-up ("open a follow-up
rather than broadening if the fix would require moving the logical selection
off a host that went down"). That follow-up is this plan, and it is not
optional: without it the fix does not work in the primary use case.

## Root cause

`viewport_top_id` and `selected_id` both name a *host*, and the display order
is reachability-sorted. When a host fails it is pushed to the bottom of the
list, so both names are dragged down with it.

Plan 185 repaired the anchor, so the window was momentarily correct. It was
then undone by `ensure_selected_visible`, which runs at the end of
`adopt_snapshot_inner` and scrolls the window onto the selected system — and
the selected system was the host that had just failed, now at the bottom of
the list.

The two rules were in direct conflict:

- *the window must not follow a host out of the live group* (Plan 185), and
- *the selected system is always visible* (pre-existing, documented).

Plan 185 resolved the conflict in favour of the older rule, which is why the
crowded case was not fixed.

The selection is not incidental here: on the first document that carries
polled reachability, `selected_id` and `viewport_top_id` are both pinned to
`display_order()[0]`. The host at the top of the list is therefore *both* the
anchor and the selection, which is exactly the host in the operator's report.

## Scope

Keep the dead host moving to the bottom of the list, keep the window on the
same part of the fleet, and keep the selection on a live host. One scroll
model, unchanged. Nothing else.

The operator's decision on the selection, recorded here because it is a
product decision and not a mechanical one:

> When the host you had selected is the one that goes offline, where should the
> selection go? — **Move the selection to the live host that took its place.**

The alternative — keeping the selection on the dead host and suppressing the
scroll for that transition — was considered and rejected: the selection would
be off-screen, the operator would lose the visible highlight, and `d`/`n`/`c`
would target a host that is down, so pressing `d` would appear to do nothing.

Also out of scope, by the operator's decision: surfacing a dead host that has
scrolled out of a small pane (for example an offline count in the Systems
header). A crowded pane can hide a host that failed entirely, and that is a
real gap, but it is a separate change and it adds chrome to a pane that is
deliberately space-efficient.

## Acceptance criteria

- [x] In a pane too small to hold the fleet, a host at the top of the list
      going down leaves the live hosts the operator was looking at on screen.
- [x] The selection moves to the live host that took the failed host's place,
      so the "selected system is always visible" rule stays true and does not
      drag the window to the bottom of the list.
- [x] The repair lives in the single fleet writer (`AppState::adopt_snapshot`),
      so the renderer, `ensure_selected_visible`, and the state agree on the
      window; no second scroll model is introduced.
- [x] A host that fails while it is *not* the selected system does not move
      the selection.
- [x] A host that a reload repointed is not treated as a failed host: its
      state was reset on purpose, and both the selection and the window stay
      on it.
- [x] A host that was reset to *pending* rather than failed is not treated as
      dead — a reload repointing it, or a client daemon restarting and
      rebuilding the fleet, must not move the operator's selection.
- [x] `cargo fmt`, workspace Clippy, and `./scripts/check-local.sh` are clean.

## Stop conditions

Open a follow-up rather than broadening if the fix would require changing
`display_order_of` (the online-first/offline-last policy is deliberate and
shared with the daemon's `FleetState`), if it would require rendering a dead
host outside its sorted position, or if it would require adding chrome to the
Systems pane.

## Handoff

A host that goes down moves to the bottom of the list, the window stays on the
same part of the fleet, and the selection lands on a live host — in a pane
sized for three rows just as well as in one sized for the whole fleet.

## Closure record

### The fix

`AppState::repair_selection` runs inside `adopt_snapshot_inner`, right after
the fleet is replaced and *before* `repair_cron_selection`, because the
retained cron job is derived from the selected system and must not be left
naming a job the new selection does not have.

It uses the same rule as Plan 185's anchor repair, and the two share one
helper, `AppState::live_replacement`:

- it fires only when the selected host **failed** — it was `Online` and is now
  `Offline`;
- it fires only when the selected host's **target is unchanged**, so a host a
  reload repointed is not treated as a failed host;
- the selection moves to the host that now occupies the failed host's previous
  display position, preferring a live host at or before that position.

The anchor repair and the selection repair are independent, because they name
different hosts in general. When they name the same host — the common case,
because the top of the list is both the anchor and the selection — the two
agree, because they apply the same rule to the same old position.

### The trigger is now "failed", not "not online"

Plan 185 fired on `Online` → *not* `Online`. That is looser than it should be.
`Reachability::Pending` means "no poll result received yet", so a host that
was reset to pending has not failed: a reload repointed it on purpose, and a
client daemon that restarts rebuilds the fleet as pending. Under Plan 185's
trigger, either of those moved the operator's viewport anchor.

The trigger is now `Online` → `Offline`: only a failed poll makes a host dead.
Plan 185's own tests are unaffected — they all fail a host with a refused
connection — and the reconcile test is unaffected twice over, since a
repointed host is both pending and a changed target.

### Why the selection moves rather than the scroll being suppressed

The window is kept on the selected system; that is what makes `j`/`k` land
where the operator expects. Suppressing that rule for one transition would
leave the selection off-screen, and `d`/`n`/`c` would act on a host that is
down, with its detail rows rendered off-screen. Moving the selection keeps
every rule true at once: the window stays, the selection is visible, the
selection is live, and the dead host is still listed at the bottom of the
list with its offline reason.

### Evidence

Mutation-verified in the repo's usual sense: the pre-fix behavior was
reinstated in place — `AppState::repair_selection` made a no-op, which is
exactly the state Plan 185 left behind — and the new regression tests were
observed **failing with the recorded symptom** before the fix was restored.

- `state::tests::offline_top_host_keeps_the_live_fleet_visible_in_a_crowded_pane`
  — failed with `left: Some("g"), right: Some("b")`: the window had been
  dragged to the bottom of the list.
- `state::tests::selected_host_failure_moves_the_selection_to_the_live_replacement`
  — failed with `left: Some("c"), right: Some("d")`: the selection stayed on
  the failed host.
- `ui::tests::offline_top_host_does_not_hide_the_live_fleet_in_a_crowded_pane`
  — failed with the operator's exact symptom on screen: the window showed the
  bottom of the list, with the failed host and the two hosts below it, and
  every live host the operator had been looking at cut off.

  ~~~text
  g     25%  38%     —    —  1.32     0.4
  h     25%  38%     —    —  1.32     0.4
  a  offline (refused)
  ~~~

The guard tests — `host_below_the_selection_failing_does_not_move_the_selection`
and `reconfigured_selected_host_keeps_the_selection` — **passed** against the
pre-fix logic, which is correct: they assert that nothing moves, so they must
not depend on the repair. So did Plan 185's five tests, including the two that
assert the dead host stays rendered at the bottom of a list that fits.

With the fix restored:

- `cargo fmt --all -- --check` — clean.
- `cargo test -p gregg --all-features` — green.
- `cargo test --workspace --all-targets --all-features` — green.
- `./scripts/check-local.sh` — `=== all checks passed (mode: default) ===`.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
  clean.

### One test of Plan 185's had to be made reachable

Plan 185's `offline_host_above_the_anchor_does_not_slide_the_window` pinned
the viewport anchor to the last host while leaving the selection on the first
one. That state cannot occur: the window is kept on the selected system, so
the selection is never above the anchor. The test reached it by poking
`viewport_top_id` directly, and it passed only because the selection was never
repaired.

With the selection repaired, the selection moved to a live host and the window
correctly followed its own selection. The test now selects the same host it
anchors on — the reachable state it meant to describe — and still guards the
behaviour it was written for: a host *above* the anchor going down must not
slide the window.


### Regression tests

- `state::tests::offline_top_host_keeps_the_live_fleet_visible_in_a_crowded_pane`
  — the reported defect in the shape the operator hits it: eight hosts, three
  condensed rows, the top host fails. The dead host sorts to the bottom, the
  window stays on live hosts, the selection lands on one of them, and the
  visible rows are the live ones.
- `state::tests::selected_host_failure_moves_the_selection_to_the_live_replacement`
  — the selected host fails; the selection moves to the host that took its
  place and the window does not move.
- `state::tests::host_below_the_selection_failing_does_not_move_the_selection`
  — the guard against over-correcting: an unrelated host's failure leaves the
  selection alone.
- `state::tests::reconfigured_selected_host_keeps_the_selection` — a reload
  that repoints the selected host resets it on purpose; the selection stays.
- `ui::tests::offline_top_host_does_not_hide_the_live_fleet_in_a_crowded_pane`
  — the same defect asserted through the renderer, so the fix is proven at the
  level the operator actually sees.
