# Plan 192: The pulldown carries the panel

Status: implemented, pending native interactive acceptance. Depends on Plan 191 (implemented,
pending native interactive acceptance) and the 187-190 line; unblocks nothing yet. Independent of
Plan 091. The interactive native acceptance gate is **unchecked**, so this plan is **not closed** and
the GUI is **not shippable** for Windows or Linux.

> **Amendment, 2026-10-22 — the pulldown's chrome was revised after this plan was implemented.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. Two of the ways this plan recorded meeting it are **superseded**, and the plan's evidence
> for them no longer describes what ships:
>
> - The shadow is no longer "three translucent rounded rectangles" (see the amendment notes
>   below). It is **fourteen one-pixel rings** with a Gaussian alpha profile, because the
>   three-layer version banded visibly.
> - The macOS pulldown is **not transparent** as this plan's measurements recorded. It was
>   *supposed* to be, and it was not: `softbuffer`'s macOS backend declares the framebuffer's
>   pixel format with **no alpha channel**, so the shadow margin — transparent by design — was
>   presented as a **thick black frame**. That was a defect in the presentation layer, not a
>   platform limit, and it is fixed by a **vendored copy** of `softbuffer` 0.4.8 whose macOS
>   backend declares the alpha channel. See `apps/gregg-gui/vendor/VENDORED-PATCH.md`.
>
> The card's outline is also no longer Slint's `Palette.border` (45 % black in one pixel in the
> light scheme): it is a **measured macOS hairline**, `NSColor.separatorColor`.
>
> CI run **`39833577564`** on `ee83e43` is **green on all ten jobs**, including the three `GUI`
> legs — the only authority for the vendored `softbuffer` copy on macOS, Windows and Linux.
>
> The amendment changes the pulldown's *chrome*, not its *behaviour*: the tray's click still
> opens the pulldown, the pulldown still carries the content, there is still no launcher entry,
> and the protocol is still untouched.

> **Amendment, 2026-10-22 (second) — the pulldown now has two views, and the fleet view is
> a compact table.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. What changed is **what the pulldown carries at once**. The pulldown now has **two views**:
>
> - The **fleet** is the TUI's **condensed fleet row with bars in it**: one line per host, a usage
>   bar for each of the five used shares (`CPU`, `MEM`, `SWP`/`COMMIT`, `DISK`, `NET`) under a
>   row of column headings. The row **names the host and not where it is** — the endpoint is the
>   detail view's business, the way the TUI's condensed row does not carry it either — and the
>   readings that are **not** used shares are carried beside the row in the TUI's own places: the
>   **I/O wait after the host's name**, the **frequency after the CPU percentage**.
> - The **detail** — the selected host's metrics, drives and interfaces, its scheduler jobs and
>   records, and the `EggPool` plane — is behind the pulldown's **own footer control**, so the
>   main window is a list of hosts and nothing else.
>
> The pulldown's height follows the view it is carrying, and the host's own `Edit` and `Remove`
> actions are the pulldown's affordances, aimed at the selection, rather than a button on every
> row.
>
> Two defects were fixed in the same pass:
>
> - **A usage bar filled from the middle of its track instead of from the left.** Slint centres a
>   child that has a size and no explicit `x`/`y` inside a parent that is not a layout (the
>   compiler's `maybe_center_in_parent` pass), so a bar's fill without an `x` grew out of the
>   middle of its track. Every element the pulldown places inside another element now carries its
>   own `x` and `y`.
> - **The pulldown clipped its last host row**, because the attribution text took a line of its
>   own even when it was empty and the height calculation did not know about it. The attribution
>   is now wrapped in a rectangle with an explicit height, like the status line and the message
>   before it.
>
> The amendment changes the pulldown's *shape*, not its *behaviour*: the tray's click still opens
> the pulldown, the pulldown still carries the content, there is still no launcher entry, and the
> protocol is still untouched.
>
> CI run **`39846098748`** on `7e56a3c` is **green on all ten jobs**, including the three `GUI`
> legs — the only authority for this workspace's Windows and Linux behaviour.

> **Amendment, 2026-10-22 (third) — the fleet view is a table of blocks, and the
> selection is a quiet tint.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. What changed is the **shape of the fleet view**, which the second amendment recorded as
> "one line per host under a row of column headings". That is now **superseded**: the fleet view is
> the TUI's condensed fleet row **turned on its side**.
>
> - Every host is a **block of its own**. A block is a **header line** — the host's name and the
>   readings that are **not** used shares, in the TUI's own words and the TUI's own order, e.g.
>   `Deadpool  IO 40.7%  9.87/11.42/12.26  16C  912.96MHz` — and the **five used shares** under it:
>   `CPU`, `MEM`, `SWP`/`COMMIT`, `DISK` and `NET`, one row each.
> - Each row carries **only its share**: the bar takes the **full width of the row** less the
>   percentage, and the percentage is the last thing in the row. A reading that is not a used share
>   has no bar, so it can never be drawn as one — the I/O wait in particular is a fraction of *time*.
> - A host that is not online has **no rows at all**: its header says why it is offline and nothing
>   else, which is the TUI's own rule.
> - The **selected host** is marked with a quiet tint and by its name in the accent colour, and
>   deliberately **not** with the platform's selection colour. That colour is a saturated accent, and
>   the palette's own foreground for it is **black on macOS**: a bar drawn in it reads as a hole
>   rather than as a measurement. A bar is a measurement, and a measurement looks the same wherever
>   it is, so the bars are the same on a selected block as on any other.
>
> The pulldown's height still follows the view it is carrying, and the host list is still bounded
> and scrolling: the pulldown is a popover, and a fleet can be large.
>
> The amendment changes the pulldown's *shape*, not its *behaviour*: the tray's click still opens
> the pulldown, the pulldown still carries the content, there is still no launcher entry, and the
> protocol is still untouched.
>
> CI run **`39863031513`** on `9932b57` is **green on all ten jobs**, including the three `GUI`
> legs — the only authority for this workspace's Windows and Linux behaviour.

> **Amendment, 2026-10-23 (fourth) — the detail view is the fleet view's own block, and the
> window waits for the operator.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. What changed is the pulldown's **detail view** and the moment its **window** is built:
>
> - The detail view is now **the fleet view's own block, opened up**: the selected host's block,
>   drawn by the **same `HostBlock` component** the fleet view draws every host with. There is
>   therefore **exactly one bar per used share in the panel**, and the two views cannot drift
>   apart — a host shown twice is shown twice the same way.
> - The extra facts are **one condensed list** under quiet group headings — `RESOURCES`, `DRIVES`,
>   `NETWORK` and `EGGPOOL` — a two-column row each: the name on the left, the value on the right.
>   The old detail was a stack of app-like tables; it is now a single list, and the cron jobs and
>   their records follow it.
> - The pulldown's **window is built on the first tray click**, not at start-up, and a hidden
>   pulldown is not re-rendered. A menu-bar app that nobody has opened yet should not be paying for
>   a window it never shows: the idle resident set fell from **82 MB** to **45.5 MB** on macOS 15
>   (arm64) against the real daemon and a seven-host fleet. The component is built **once** and
>   toggled afterwards, because dropping it does not give the memory back.
>
> The amendment changes the pulldown's *shape*, not its *behaviour*: the tray's click still opens
> the pulldown, the pulldown still carries the content, there is still no launcher entry, and the
> protocol is still untouched.
>
> CI run **`39909338640`** on `62e74f9` is **green on all ten jobs**, including the three `GUI`
> legs — the only authority for this workspace's Windows and Linux behaviour.

> **Amendment, 2026-10-23 (fifth) — the pulldown reaches another application's full screen.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. What changed is **where the pulldown can appear**: it now opens over another application
> that is in full screen, which it could not do before.
>
> - A full-screen application owns a **space** of its own, and a window that is not in that space is
>   not on the screen at all: the click reaches the tray, the pulldown is built and placed, and the
>   operator sees **nothing** — and the full-screen application is dropped out of its space on the
>   way. macOS keeps a **Dock** application's windows out of another application's space, and that
>   right is **fixed when the window is created**: no window level and no collection behaviour
>   changes it. The GUI is now a **menu-bar-only** application — it installs the `Accessory`
>   activation policy before any window exists, through winit's event-loop builder, which is where
>   the desktop makes that decision itself.
> - The pulldown's **native window** is told to join every space (`CanJoinAllSpaces`,
>   `FullScreenAuxiliary`, `Stationary`, `IgnoresCycle`), to stay visible when the application is
>   deactivated, and to sit at **`NSPopUpMenuWindowLevel`** — the level a native status item's own
>   menu is drawn at. `always-on-top` was not enough: it reaches the *floating* level, and the
>   floating level is below full-screen content.
> - The native window is found by its **title**, and that title is **owned by the `.slint` file** —
>   bound to a property the Rust side reads — so the two cannot drift apart: a title that lived in
>   two places would be a title that could drift, and the overlay would silently stop working.
>
> Two consequences are deliberate: the GUI has **no Dock icon**, and the menu bar stays the other
> application's — which is what a native menu-bar extra does. The menu bar itself is still macOS's
> business: it is hidden in full screen, and the pointer at the top of the screen brings it back.
>
> The amendment changes **where the pulldown can appear**, not its *behaviour*: the tray's click
> still opens the pulldown, the pulldown still carries the content, there is still no launcher
> entry, and the protocol is still untouched. The whole change is inside
> `#[cfg(target_os = "macos")]`.
>
> CI run **`39927427831`** on `22e1bce` is **green on all ten jobs**, including the three `GUI`
> legs — the only authority for this workspace's Windows and Linux behaviour. The implementation
> itself, `d0cd31c`, was green on all ten jobs on run **`39922180617`**.
>
> **Amendment, 2026-10-23 (seventh) — the pulldown spends no vertical space on nothing, and
> its title says only what matters.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. What changed is **how much room the pulldown spends on nothing**, and **what its title
> line says**.
>
> - There was **forty-five points of dead space** between the fleet's title and the first host's
>   block, and it was not margin. A Slint layout adds its spacing between **every** cell, and a
>   cell that is empty still takes part in the layout: the body's first three cells — the
>   attachment's state, the last action's message and the tray's warning — were all empty on a
>   healthy fleet, and each still took a six-point gap. Eighteen points of nothing, on every panel.
>   The notices are **one cell** now and it takes **no room at all** when it is empty, the body's
>   own spacing is **zero**, every group carries the gap that follows it, and the title strip and
>   the body's top padding are smaller. Measured on the same fleet: the gap went from **45 points
>   to 19**, and the panel from **478 points tall to 446**.
> - The title line said `7 online, 0 pending, 0 offline — Deadpool, MacbookPro, … (+2 mo…)`, which
>   was elided away on any fleet of any size. It says **`7 online`** now, and names the hosts that
>   are not answering **only when there are any**.
> - The **corner grip is gone**: the hairline square in the bottom-right corner looked like a
>   button and did nothing anybody could see, and the size it controlled was a size the operator
>   could set once and never change again. The pulldown's size is its content's size, always, and
>   the state file holds a placement and nothing else.
>
> The amendment changes the pulldown's **shape and its title**, not its *behaviour*: the tray's
> click still opens the pulldown, the pulldown still carries the content, there is still no
> launcher entry, and the protocol is still untouched. The change is in the platform-neutral
> `.slint` file and the placement module, so all three targets carry it.
>
> The evidence is in `apps/gregg-gui/EVIDENCE.md`, in the section *"The panel had a band of dead
> space above the fleet"*, with the before-and-after pixel measurements taken from the running
> panel on the 3840×1080 display.
>
> The CI run that covers this work, **`39982133374`**, was **green on nine jobs and failed the
> `GUI (ubuntu-latest)` leg** on one test — `bridge::tests::an_explicit_client_must_identify_itself`
> — with `Text file busy (os error 26)`. That is the repository's known `ETXTBSY` flake: the test
> writes a `/bin/sh` stub with `fs::write` and the identity probe `execve`s it a moment later, and
> the close-to-exec boundary intermittently fails on Linux under load. It has been root-caused
> twice before, in plans 161 and 167, where the repair was deferred because it belonged to another
> crate. **Here it belongs to this branch's own crate**, so the repair is applied here: a
> **test-only** retry of a probe whose diagnostic says the executable could not be run. The
> product changed nothing — it reported the failed spawn as a diagnostic, which is correct.
>
> The next run, **`39985312878`**, was **green on seven jobs** and failed **both** of the two
> remaining `GUI` legs, and both failures were this work's: the retry helper was `#[cfg(unix)]`
> only in the *tests* that use it, so on Windows it was dead code and the CI's `-D warnings` made
> that a failure; and the **native Linux exercise** was asserting the *old* summary — it waited
> for the tooltip to name the fixture's host, which is exactly what this amendment removed. The
> exercise now asserts what the summary **is** (the counts) and what it **is not** (a host's
> name), and its `--expect-host-name` became `--host-name`.
>
> The run that closes this work, **`39989322526`** against `e303d05`, is **green on all ten jobs**,
> including the three `GUI` legs — and the `ubuntu-latest` leg is where the **native** Linux tray
> exercise runs against a real `StatusNotifierItem` host, with the *new* summary.

> **Amendment, 2026-10-23 (sixth) — the remembered placement is an offset, not a position.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal. What changed is **where the pulldown opens on a display that is not the one it was placed
> on**: it now opens against the trailing corner of the display it is on, whatever that display's
> size.
>
> - The pulldown's placement was remembered as an **absolute position in logical pixels**
>   (`position=844,65`), and a remembered position won over the anchor. The anchor itself was never
>   wrong — it is computed from the work area of the display the window is on, so it accounts for
>   that display's size and scale — but a position names a place on *one* display. A pulldown
>   placed on a 1512-point laptop display and opened on a 3840×1080 one landed **2382 points from
>   the tray**: on screen, the right size, and nowhere near the icon whose click opened it.
> - What is remembered is now an **offset** — the gap at the trailing edge and the gap at the edge
>   the pulldown hangs from — measured against the display it was placed on (`offset=12.5,0.0`). An
>   offset means the same thing on every display, at every scale, on either side of a resolution
>   change, and the offset is measured to the panel's *edges*, so the pulldown grows away from the
>   corner it hangs from.
> - The file's older `position=` key is **not read**: a place on the display it was measured on
>   cannot be carried to a display it was never measured against, and reading it would reproduce the
>   very defect the offset exists to fix. A file that says only `position=` is read as no placement
>   at all, so the one-time cost of the change is that an operator who had dragged the pulldown on
>   another display finds it back at the corner.
> - The offset is measured against the position the GUI **asked for**, never against the position
>   the window reports back: in another application's full-screen space macOS draws the window 26
>   points lower than it was asked to be, and a placement measured from the window's own report
>   would carry that shift into the memory, so the pulldown would open a little lower every time
>   and never come back.
>
> The amendment changes **where the pulldown opens on a different display**, not its *behaviour*:
> the tray's click still opens the pulldown, the pulldown still carries the content, there is still
> no launcher entry, and the protocol is still untouched. The change is in the platform-neutral
> placement code, so all three targets carry it.
>
> CI run **`39968561479`** on `fd11201` is **green on all ten jobs**, including the three `GUI`
> legs — the only authority for this workspace's Windows and Linux behaviour. The implementation
> itself, `7c19581`, was green on all ten jobs on run **`39963924905`**. The run in between,
> **`39966757278`** against `4e7c073`, failed the `GUI (windows-2022)` leg: the two tests that
> commit added assumed the **top** anchor, and Windows hangs the pulldown from the bottom, where
> the taskbar is. The fix is `fd11201`.

> **Amendment, 2026-10-23 (eighth) — the pulldown closes when the operator clicks away.**
> The direction this plan set — *the pulldown reads as a dropdown* — is unchanged and is still the
> goal, and this amendment brings the pulldown's **behaviour** in line with the **shape** the
> earlier amendments gave it: a dropdown that stays on screen when the operator clicks the desktop
> behind it does not read as a dropdown.
>
> This plan **excluded** click-away dismissal, on the stated ground that *"Slint 1.18 exposes no
> focus-lost event"*. **That ground was never checked against the source, and it is wrong.**
> `FocusScope` has a `focus-lost(reason: FocusReason)` callback, and `FocusReason` has a
> **`window-activation`** variant — the desktop saying the window is no longer the active one,
> which is what a click away does. The winit backend delivers it: `Focused` → `activation_changed`
> → `WindowEvent::WindowActiveChanged` → `WindowInner::set_active` → `FocusOut(WindowActivation)`.
>
> That signal was built first, and it was **measured** before it was trusted. On macOS it is **not**
> a reliable click-away signal: an application that takes the foreground for its own reasons takes
> the focus away too, and the pulldown closes itself while the operator is reading it. On this
> machine it **self-dismissed in four opens out of five**, every time because another application
> had taken the foreground.
>
> So the click away is now **two signals on two platforms**, because another application's mouse is
> not watchable everywhere:
>
> - **macOS watches the mouse.** A click that lands outside the pulldown belongs to the application
>   that was clicked, so it cannot be seen from inside the pulldown's own window. AppKit's global
>   event monitor reports exactly those events — the ones dispatched to *other* applications — and
>   the pointer's position says where the click landed. It is **safe** AppKit API: no event tap, no
>   accessibility permission, and the GUI crate still contains **no `unsafe` at all**. The **menu bar
>   is excluded**, because the tray's own click *toggles* the pulldown and the watch must not
>   dismiss it as well; the exclusion is a **band**, not a line, because the pointer's position is
>   read when the click is delivered rather than out of the click itself.
> - **Windows and Linux watch the keyboard focus**, which is the signal above. There is no portable
>   way to watch another application's mouse there.
>
> - The pulldown aggregates **everything that can hold the keyboard focus**: the scope that handles
>   the keys, and the **four fields** of the two forms. A text field takes the focus away from the
>   scope when it is clicked, so a click away with a form open has to be caught too.
> - The form's fields are **named**, which is why the form's pane is always in the tree and only its
>   visibility changes: a field inside a conditional block cannot be named from the top of the
>   component, and the aggregate lives at the top.
> - When the **view changes** the pulldown **takes the focus back**, because a form that goes away
>   takes the focus with it, and the pulldown must not mistake that for a click away.
> - Every dismissal — `Esc`, the tray's click and the click away — goes through **one**
>   function now, so they leave the same thing behind: the fleet view, and no half-typed form.
>   (The pulldown's own `×` was a fourth dismissal control and was **removed** by the tenth
>   amendment.)
> - The **geometry** of the click away — inside the pulldown, in the menu bar, or elsewhere — is one
>   **pure** function, so it is unit-tested on every platform, including the ones that do not use
>   the mouse watch.
>
> The limits that are left are stated rather than papered over: on Windows and Linux the signal is
> *the focus was lost*, not *a click landed elsewhere*, so a desktop that takes the focus away for
> another reason dismisses the pulldown too; and on macOS the watch sees the clicks dispatched to
> *other* applications, so a click another application swallows before dispatch is not seen. A
> native status-item menu does not have the first problem, because it is **modal** — it grabs the
> mouse and the keyboard, so nothing else can take the focus while it is open. A Slint window
> cannot be modal, so macOS watches the mouse instead.
>
> Verified natively on macOS 15 (arm64) against the live window list: a click **away** dismisses
> the pulldown (`onscreen=false`), a click **inside** does not, and the **tray's** own click — in
> the menu bar — neither dismisses it by the watch nor fails to toggle it (three tray clicks in a
> row opened, closed and opened it again, with no dismissal line in between). The same session
> measured the GUI against the terminal client: **46.9 MB** of resident set — **14 MB** of physical
> footprint — for a GUI whose pulldown was never opened, and **41.9 MB** (**26 MB**) with the
> window on screen, against **3.2 MB** (**2.3 MB**) for the TUI and **5.2 MB** (**4.0 MB**) for the
> client daemon that does the polling — and `lsof` on the GUI's process lists **no network socket
> at all**, so the fleet is not polled twice.
>
> The run that covers this work, **`40017131833`** against `481cda0`, is **green on all ten jobs**,
> including the three `GUI` legs — which are the only authority for this workspace's Windows and
> Linux behaviour, because the workspace is not a member of the root workspace and no root job
> compiles it.

> **Amendment, 2026-10-23 (ninth) — the scroll box reaches the bottom, the two views share one
> box, and the pulldown is narrower.**
> The direction this plan set is unchanged. This amendment fixes a **defect** the earlier amendments
> left in the pulldown's *layout*, and it is a defect that made the plan's own goal — *the
> pulldown reads as a dropdown* — untrue in two ways: the pulldown carried a band of dead space
> that no dropdown has, and it was **too wide** for what it carries.
>
> **What was wrong.** The fleet's scroll box was **201 points** tall in a body that had **348**, so
> **147 points of dead space** sat between the last host and the footer's strip. The detail view's
> box was **51 points**, which showed a host's name and three rows and nothing else — so "the
> detail button does not do its job" and "there is no way back" were the same defect seen from
> two sides: the pane was too short to read, and the way back was a link that had moved
> twenty-two points up, because the panel resized when the detail opened.
>
> **Why.** Two panes that were merely **invisible** were still **220 points tall each**: the *Add a
> host* and *Edit a host* forms, which are always in the tree because their fields have to be
> nameable — the eighth amendment made that necessary, because the focus aggregate names them.
> A Slint layout lays out the **invisible** items as if they were visible: they are not painted,
> but they still claim their height, and the layout squeezes the items around them to make room.
> The two forms took **440 points** of a body that had **348**. `visible: false` hides a pane; it
> does not unmake it.
>
> **What changed.**
>
> - A hidden pane declares **zero height** — its preferred height, its maximum height *and* its
>   minimum height, because the fluent scroll container has a fifty-point floor of its own and a
>   floor is a floor whether the pane is visible or not.
> - The fleet view and the detail view scroll in **one** box, and so they share **one** height:
>   opening the detail does not resize the panel, and the footer's control that brings the fleet
>   back never moves under the pointer. The box is the fleet's blocks, bounded so that a large
>   fleet scrolls, and floored so that the detail always has room to be read. The detail's own
>   content — the drives, the interfaces, the scheduler and the records — is very often taller
>   than the fleet's list, and it **scrolls**; the pulldown's size is the fleet's business.
> - The pulldown is **194 points narrower**: **412 px** rather than 606. It is a dropdown, and
>   most of what it carries is vertical, so a panel wider than the row is mostly empty. The bar
>   still takes the whole row less the percentage.
>
> **Measured, on the same fleet, before and after.** The fleet's box: **201 → 348** points — the
> whole body, down to the footer. The detail's box: **51 → 348**. The panel: **606 → 412** points
> wide. The panel's height does not change between the two views any more, so the footer does not
> move: the *same* control brings the fleet back.
>
> The Rust side follows: `placement::box_height()` is the one number the `.slint` is given, the
> detail's own height estimate is gone with the detail's own row counts, and two tests hold the
> shape — `a_hidden_form_takes_no_room` and `the_fleet_and_the_detail_share_one_box`.
>
> The run that covers this work, **`40029395421`** against `f7c3fb7`, is **green on all ten jobs**,
> including the three `GUI` legs — the only authority for this workspace's Windows and Linux
> behaviour, because the workspace is not a member of the root workspace and no root job compiles
> it. The GUI workspace's own suite is **177 passed / 0 failed**, its clippy is clean under
> `-D warnings`, its formatting is clean, and its release binary is **11,979,552 bytes** —
> **+16 bytes**, because a narrower panel is the same code drawing fewer pixels.

> **Amendment, 2026-10-23 (tenth) — the `×` is gone, and the `EggPool` plane is always shown.**
> The direction this plan set is unchanged. This amendment takes two things out of the pulldown's
> chrome that did not earn their place, and it moves the `EggPool` plane out from behind the
> detail view.
>
> **The `×` served no function.** It was a fourth dismissal control: the tray's click, `Esc` and a
> click away already dismiss the pulldown, and the `×` did the same thing in a corner, as a target
> the operator can miss. The header strip now carries the fleet summary and the drag area and
> nothing else. `Esc` still dismisses.
>
> **The `EggPool` plane was behind the detail view, in two ways.** The footer's link was shown only
> in the detail (`visible: root.mode == PanelMode.Detail;`), and the plane's numbers were one of
> the **selected host's groups**. So a fleet that was not opened in the detail carried no
> `EggPool` at all, and the operator had to know that the plane lived in the detail to find it.
>
> **Why the plane is not a group.** It describes the **service**, and not a host. The detail's
> groups are the selected host's own facts — the numbers behind its shares, its drives, its
> interfaces — and the plane is none of those. Putting it among them was the mistake: it made
> the service's plane depend on which host happened to be selected.
>
> **What changed.**
>
> - The plane is **pinned below the scrolling box** — outside the `if` that gates the two views,
>   and outside the box they share — so it is shown in **every** view and never scrolls out of
>   reach. The link that opens and closes it is in the footer in every view too.
> - The plane is **open from the first frame**: the GUI publishes the `EggPool` intent when it
>   attaches, so the daemon is already sending the plane the panel shows. The intent governs what
>   the daemon *transmits*, never what it *fetches*, and a configuration with no `EggPool` entry
>   has nothing to fetch.
> - The panel is **exactly as tall as the plane it shows**, and a plane with nothing to say takes
>   **no room at all** — the same rule the forms follow, and for the same reason: a Slint layout
>   lays out the invisible items as if they were visible.
>
> **Measured, natively on macOS 15 (arm64), against the real seven-host fleet and a live
> `EggPool`.** The panel is **412 × 536** with the plane and **412 × 446** without it, and it is
> **412 × 536** in **both** views — so opening the detail still moves nothing under the pointer.
> The plane's own ninety points are measured in the panel's coordinates: the fleet's last row ends
> at **y = 385**, the scrolling box ends at **y = 396**, the `EGGPOOL` heading is at **y = 399**
> and the plane's last row ends at **y = 482**. In the detail view the last `DRIVES` row ends at
> **y = 395** — clipped at the box's edge — and the heading is still at **y = 399**, so the detail
> scrolls inside the box and the plane does not move. A configuration with **no** `EggPool` entry
> is **412 × 446** with no plane and no gap, exactly as before this work.
>
> The GUI workspace's own suite is **182 passed / 0 failed** (five new tests: the two that hold the
> chrome and the plane's presence, the one that holds the plane's own rule, and the two that hold
> the height), its clippy is clean under `-D warnings`, its formatting is clean, and its release
> binary is **11,979,552 bytes** — **+0 bytes** on the build before this work.

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
- **Click-away dismissal was not available** when this plan was written, on the stated ground that
  Slint 1.18.1 exposes no focus-lost event. That ground was wrong, and the dismissal now ships:
  see the eighth amendment. The pulldown is dismissed by `Esc`, by the tray's click **and by a
  click away**, and the limits of that last one are stated rather than
  papered over. (Its own `×` was a fourth control that did the same thing and was **removed**
  by the tenth amendment.)
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
  > **Amended by the eighth amendment.** Click-away dismissal is **no longer excluded**, and the
  > ground for excluding it was wrong: Slint 1.18 **does** expose a focus-lost event, with the
  > reason attached, and `FocusReason.window-activation` is the signal a click away produces.
  > **Amended again by the same amendment.** The focus turned out to be an unreliable click-away
  > signal on macOS — **measured**, the pulldown dismissed itself in four opens out of five because
  > another application took the foreground — so macOS now watches the **mouse** through AppKit's
  > global event monitor, and only Windows and Linux watch the focus. There is **no event tap** and
  > **no accessibility permission**: the watch is safe AppKit API, and the GUI crate still contains
  > no `unsafe`. There is still no hand-rolled tray icon, and `block2` — the crate that carries the
  > monitor's Objective-C block — was **already in the GUI workspace's graph** through `objc2`, so
  > naming it directly adds no new crate to the build.
- a GPU renderer, and a vendored patch of Slint's runtime code;
- a new dependency in the GUI workspace;
  > **Amended by the eighth amendment.** `block2` is now a **direct** dependency of the GUI
  > workspace, because the macOS mouse watch's handler is an Objective-C block and the type has
  > to be named. It is **already in the graph** through `objc2`, so **no new crate** is built and
  > the lockfile still holds the same 604 packages — the same rule the `std-widgets` and `zbus`
  > edges follow. `objc2-app-kit` also gains the `NSEvent`, `NSScreen` and `block2` features.
- a new CI job, workflow, matrix, artifact or evidence bundle;
- a release asset, installer, bundle, packaging job or publication step;
- rewriting the historical records of Plans 187-190.

## Closure gates

- [x] Clicking the tray icon opens the pulldown on macOS, Windows and Linux, and the pulldown
      shows the panel's content. There is no launcher entry, and the content is in the
      pulldown rather than in a separately opened window. — **The content and the launcher are
      as described**: the pulldown carries the fleet summary, the host list, the selected host's
      metrics, drives, interfaces, cron, records, the `EggPool` plane and the forms, and there is
      no `Open Gregg` anywhere. The *click itself* has not been performed on any platform — that
      is the interactive acceptance below, and it is left unchecked.
      — **Amended 2026-10-22 (second):** the content is carried in **two views** now. The
      **fleet** is one line per host with a bar for each of the five used shares, and the
      **detail** — the selected host's metrics, drives, interfaces, cron, records and the
      `EggPool` plane — is behind the pulldown's own footer control. The forms are still
      carried. The launcher is still gone.
- [x] The native menu is not the primary surface on any platform. On macOS it is detached, and
      the pulldown carries `Reload`, `About` and `Quit`. — `platform::native_tray_menu()` is
      `false` on macOS and `true` elsewhere; the `.slint`'s menu carries `Quit Gregg` and
      nothing else; the pulldown's footer carries `Reload · Add host · EggPool · About · Quit`.
      — **Amended 2026-10-22 (second):** the footer also carries the **view control**
      (`Detail` in the fleet, `Fleet` in the detail) and the selected host's own `Edit` and
      `Remove`.
- [x] The pulldown is positioned and clamped within the work area on **every** path that opens
      it: the tray's click, the startup path, and a desktop with no tray host. The native
      exercises assert the position, not only that a window exists. — The first open is deferred
      into the event loop and the placement is retried until the native window exists, so the
      work area is available on every path. The exercises parse the GUI's own
      `pulldown=placed` line and assert the position, and **both legs passed** in CI run
      `39811582623` — Linux placed the pulldown at (852, 36) inside a 1264×980 work area, and
      Windows at (596, 460) inside 1008×708, which is the bottom of that work area, where the
      Windows taskbar is.
- [x] The pulldown's position and size survive a restart. — Observed end to end: a drag of
      (−200, +100) moved the pulldown by exactly that and wrote `position=2300,220` /
      `size=500,600` to `<config>.gui-state`; the next start opened at
      `pulldown=placed x=2300 y=220 width=500 height=600`.
- [x] The pulldown reads as a dropdown: rounded, shadowed, with a pointer toward the tray
      item, and sized to its content. — Measured on a screen capture: a 12 px corner radius, the
      three shadow layers, the pointer toward the menu bar, and **420×658** for a seven-host
      configuration.
      **Amended 2026-10-22:** the "three shadow layers" are now **fourteen one-pixel rings**
      with a Gaussian alpha profile (the three-layer version banded visibly), and the outline is
      a measured macOS hairline rather than `Palette.border`. The size and the pointer are
      unchanged.
- [x] The forms still add, remove, rename and re-point hosts through the CLI's own code, and a
      refused mutation leaves the config byte-identical and shows a visible reason. — Unchanged
      from Plan 191, and covered by the GUI's own `hosts.rs` tests.
- [x] The GUI still opens no HTTP client, still starts no polling plane, and the daemon's
      `DaemonRequest` set is unchanged. — `crates/gregg/src/clientd/protocol.rs` is untouched.
- [x] The GUI's own CI job still formats, lints, tests and release-builds on macOS, Windows
      and Linux, and the native tray exercises still pass. — `./scripts/check-gui.sh` passes
      locally (fmt, clippy `-D warnings`, **128 tests**, a release build of 11,929,712 bytes),
      and CI run **`39811582623`** is **green on all ten jobs** — the core `Linux`, `macOS`,
      `macOS Intel`, `Windows`, `Scripts`, `MSRV` and `FreeBSD` jobs, and the three `GUI` legs
      (`macos-15`, `windows-2022`, `ubuntu-latest`), including the native tray exercises on
      Linux and Windows.
- [ ] The **interactive** native acceptance — a visible notification-area icon, the click
      opening the pulldown, and the pulldown being usable on a real desktop — is recorded per
      target, or is **left unchecked** and blocks shipment for that target, exactly as Plan
      190 leaves it. — **Left unchecked.** No tray icon was clicked on any platform. This blocks
      shipment for Windows and Linux, and the GUI stays a source build.

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

## Closure record

Implemented on 2026-10-22. The plan is **not closed**: the interactive native acceptance gate
below is unchecked, exactly as Plan 190 and Plan 191 leave it.

### What was built

| change | where | what it does |
| ------ | ----- | ------------ |
| The tray's click opens the pulldown | `apps/gregg-gui/src/ui/main.slint`, `src/main.rs` | `TrayIcon` forwards the inherited `SystemTrayIcon.clicked` to a new `tray-clicked` callback, and `main.rs` toggles the pulldown from it |
| The native menu is a context menu | `src/ui/main.slint`, `src/platform.rs` | the menu is `if root.native-menu : Menu { MenuItem { title: "Quit Gregg" } }`, and `platform::native_tray_menu()` is **false on macOS**, so the menu is **detached there** and the click reaches the callback |
| No launcher entry | `src/ui/main.slint` | there is no `Open Gregg`; the pulldown's own footer carries `Reload · Add host · EggPool · About · Quit` |
| The pulldown is a dropdown | `src/ui/main.slint` | a rounded card with a 1 px macOS hairline, a fourteen-ring faked shadow (amended 2026-10-22 from three layers) and a `Path`-drawn pointer on the card's trailing edge toward the tray — up on macOS and Linux, down on Windows |
| The application chrome is gone | `src/ui/main.slint` | no `METRICS` / `CRON` headings, no overhanging `Close` button, no daemon-identity line, no per-row `Edit` / `Remove` buttons; the header carries the summary and a compact `×`, and the *selected* host's actions are links |
| Placement on every path | `src/main.rs`, `src/placement.rs` | the first open is deferred into the event loop, the placement is retried for 20 ticks × 16 ms until the native window exists, and the size is clamped to the work area |
| The geometry survives a restart | `src/state.rs` (new) | `<config>.gui-state`, read bounded (4 KiB) and validated, written atomically |
| Rust owns the size | `src/placement.rs` | `placement::metrics` holds every dimension, `content_height()` computes the height from the model's row counts, and a test asserts the numbers against the `.slint` literals |
| The position is observable | `src/main.rs` | one machine-readable line: `gregg-gui: pulldown=placed x=… y=… width=… height=… area=l,t,wxh` |

The protocol is untouched: `crates/gregg/src/clientd/protocol.rs` gained no `DaemonRequest`
variant, the GUI still opens no HTTP client of its own, and the client daemon remains the only
poller.

### The macOS run

Run on 2026-10-22 on this host (Apple M4 Pro, 3840×1080), against the real
`~/Library/Application Support/gregg/gregg.toml` and the repository's own `gregg` client, with the
GUI started through its `--panel` path — the path that Plan 191 left unpositioned.

```text
gregg-gui: tray=available
gregg-gui: pulldown=placed x=3412 y=36 width=420 height=658 area=8,36,3824x1036
```

`work_area` now answers, the position is inside the work area, and the size is the content's own
size rather than a fixed 560×520. The window was then measured from the system's own window
list: **420×658 at (3412, 62)**, and the card's 1 px border was measured on a screen capture at
**y = 76**.

**The 26 px difference between the requested and the observed top is macOS's own window
placement, and it is recorded rather than hidden.** `NSScreen.visibleFrame` puts the real menu
bar's bottom edge at **30 pt** on this display, and the work area's top edge is 36 — the
documented 28 pt shell-strip approximation plus the 8 px edge margin. A window requested *flush
with that edge* is placed 26 px lower by AppKit. A **remembered** position, by contrast, is
honoured **exactly**: (2500, 120), (2300, 220), (3000, 200) and (3000, 50) were each requested
and each observed at the same coordinates. So the operator's own placement is precise, and the
default placement opens *clear of* the menu bar with a visible gap rather than under it. The
pulldown is inside the work area in every observation; nothing was clamped out of it.

### The geometry round trip

Both halves of the gate were observed, not inferred.

1. **The write.** The pulldown's header strip was dragged with synthesised pointer events, from
   (2600, 148) by (−200, +100). The window moved by exactly that delta, and the state file
   appeared beside the config:

   ```text
   position=2300,220
   size=500,600
   ```

2. **The read.** The GUI was stopped and started again with that file in place, and it opened at
   the remembered geometry rather than at the corner:

   ```text
   gregg-gui: pulldown=placed x=2300 y=220 width=500 height=600 area=8,36,3824x1036
   ```

The state file is written when the operator **moves or resizes** the pulldown — the drag and the
corner grip both raise `geometry-changed` — and not on the first placement, which is derived from
the content and the work area and so has nothing to remember. A corrupt or out-of-range file
falls back to the default; `state.rs` has 13 unit tests over that path.

> **Superseded by the sixth amendment.** This is the record of the run as it was measured, and the
> `position=` key it shows is what the file held then. The placement is now remembered as an
> **offset** against the display's anchor corner, the same file format's `offset=` key, because an
> absolute position names a place on *one* display — see the amendment at the top of this plan.

### The dropdown's shape

Measured on a screen capture of the running window: the card's corners are rounded (a 12 px
radius), the three shadow layers are visible around it, the pointer is drawn toward the menu
bar, and the pulldown is **transparent on macOS** — the window's own background is
`#00000000` there, and `Palette.background` everywhere else. The height is the content's:
**420×658** for a seven-host configuration, which is neither Plan 191's fixed 560×520 nor a
constant.

> **Amended 2026-10-22.** The transparency claim above was **wrong**, and the shadow claim is
> superseded.
>
> The window's *background* is `#00000000` on macOS, and the window really is a transparent
> `NSWindow` — but the **framebuffer** that carries its pixels was presented **opaque**:
> `softbuffer`'s macOS backend declares the pixel format as `CGImageAlphaInfo::NoneSkipFirst`,
> which means *no alpha channel*, so CoreGraphics discarded the alpha the software renderer had
> written. The shadow margin — transparent by design, so the desktop shows through it — was
> therefore presented as a **thick black frame**, and the card's rounded corners were black
> squares. The pulldown was never transparent in the sense this section claimed.
>
> The fix is one word in a **vendored copy** of `softbuffer` 0.4.8
> (`apps/gregg-gui/vendor/softbuffer`): `CGImageAlphaInfo::PremultipliedFirst` declares the
> same 32-bit layout with the alpha the buffer already carries. 0.4.8 is the latest release and
> the defect is still in it, which is why the copy exists. The change is inside Apple's platform
> only. See `apps/gregg-gui/vendor/VENDORED-PATCH.md` for the rationale and the measurements.
>
> The shadow is now **fourteen one-pixel rings** with a Gaussian alpha profile — about 17 % black
> at the card's edge, fading to nothing 14 px out, with no banding — and the card's outline is a
> measured macOS hairline, `NSColor.separatorColor` (`rgba(255,255,255,0.098)` dark,
> `rgba(0,0,0,0.098)` light), which Slint's `Palette.border` is not.

> **Amended 2026-10-22 (second) — the size, and what the pulldown carries.**
> The **420×658** above is superseded: the pulldown is now **606 px wide** (the fleet table's
> measured columns plus the card's chrome) and **264 px tall** for a seven-host fleet in the
> fleet view, because the fleet view is **one line per host** rather than a list of hosts plus
> every host's numbers. The detail view is taller, because it carries the numbers.
>
> A third renderer fact was found the hard way and is now in the `.slint`:
>
> - Slint **centres a child that has a size and no explicit `x`/`y`** inside a parent that is not
>   a layout (`maybe_center_in_parent` in the compiler's `default_geometry` pass). A usage bar's
>   fill without an `x` therefore grew out of the *middle* of its track and read as a dot. Every
>   element the pulldown places inside another element now carries its own `x` and `y`.
>
> And one layout fact, which is a **height** rule rather than a renderer one: a `Text` takes a
> line of its own **even when it is empty**, so an unwrapped line nobody asked for is a line the
> height calculation does not know about. The attribution is wrapped in a rectangle with an
> explicit height, like the status line and the message before it, and the panel's height is
> exact.

Two renderer facts were found the hard way and are now in the `.slint`:

- the software renderer's `draw_box_shadow` is an **empty stub** (`i-slint-renderer-software-1.18.1/lib.rs:3059`),
  so the shadow is drawn by hand — as three translucent rounded rectangles when this plan was
  implemented, and since 2026-10-22 as fourteen one-pixel rings with a Gaussian alpha profile;
- a `Text` keeps its unwrapped width as a minimum unless `overflow: elide` is set, so every
  wrapping text in the pulldown carries `overflow: elide` — without it a long status line
  overflowed the pulldown on **both** sides.

### The CI runs

Run **`39811582623`**, against the implementation commit `e26a273`, is **green on all ten jobs**:
the core `Linux`, `macOS`, `macOS Intel`, `Windows`, `Scripts`, `MSRV` and `FreeBSD` jobs, and
the three `GUI` legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The `GUI` legs are the only
authority for this workspace's Windows and Linux behaviour, because the workspace is not a member
of the root workspace and no root job compiles it.

**The Linux leg is the one that matters most here**, because it runs the native tray exercise
against a **real `StatusNotifierItem` host**. What it printed:

```text
sni-host: obs Title="Gregg"
sni-host: obs ToolTip.title="Gregg — 1 online, 0 pending, 0 offline — gui-fixture"
sni-host: obs Menu.labels=["Quit Gregg"]
sni-host: PASS tray-tooltip-carries-the-fleet-status the tooltip is "Gregg — 1 online, 0 pending, 0 offline — gui-fixture"
sni-host: PASS tray-tooltip-carries-the-fleet the tooltip mentions "gui-fixture"
sni-host: PASS tray-menu-is-the-context-menu a menu label mentions "Quit Gregg"
sni-host: PASS pulldown-is-placed-inside-the-work-area Placement { x: 852.0, y: 36.0, width: 420.0, height: 256.0, area_left: 8.0, area_top: 36.0, area_width: 1264.0, area_height: 980.0 }
sni-host: PASS gui-is-still-running the GUI is still running with a live tray
sni-host: PASS all-checks mode=WithHost
linux-tray: pass 2: no StatusNotifierItem host is present
sni-host: PASS gui-reported-tray-unavailable the GUI printed `gregg-gui: tray=unavailable reason="no StatusNotifierItem host on this desktop"`
sni-host: PASS gui-survives-without-a-tray the GUI is still running three seconds after reporting `tray=unavailable`
sni-host: PASS all-checks mode=NoHost
linux-tray: PASS both Linux tray passes
```

Three things in that transcript are this plan's own claims, observed natively:

- the menu is **`["Quit Gregg"]`** — the launcher is gone and the menu is the context menu;
- the **tooltip** carries the fleet status *and* the host name, so the tray states the fleet's
  health without being opened;
- the pulldown was **placed inside the work area it reported** — x=852, y=36, 420×256 in a
  1264×980 work area — which is the placement gate, proven on a real desktop rather than
  asserted.

The **Windows leg** proves the same placement claim on the platform where the anchor is the
**bottom**-trailing corner, and its own core launch path:

```text
windows-tray: the GUI printed: gregg-gui: pulldown=placed x=596 y=460 width=420 height=256 area=8,8,1008x708
windows-tray: the pulldown is inside the work area it reported
windows-tray: the GUI created 4 native top-level window(s)
windows-tray: the client daemon is running
windows-tray: the fleet fixture served 8 request(s): the daemon is polling
windows-tray: PASS every Windows tray check
```

`y=460` is `8 + 708 − 256` — the bottom of the work area, which is where the Windows taskbar is
— so the anchor is right on that platform too.

Neither leg claims that the **notification-area icon was rendered**: the Windows runner is
session 0 with no `explorer.exe`, and the Linux leg drives the SNI protocol rather than a
desktop shell. That is the interactive acceptance below, and it is still unchecked.

### Gate by gate

| Gate | State | Evidence |
| ---- | ----- | -------- |
| The tray's click opens the pulldown, the pulldown shows the panel's content, there is no launcher entry | **Met, except the gesture itself** | The content is in the pulldown and the launcher is gone — `the_tray_has_no_launcher_entry` and `the_pulldown_drops_the_application_chrome` assert it, and the macOS run above shows the panel's content in the dropdown. The *pointer click on the tray item* has not been performed on any platform; that is the interactive acceptance below. |
| The native menu is not the primary surface; on macOS it is detached and the pulldown carries `Reload`, `About` and `Quit` | **Met** | `platform::native_tray_menu()` is `false` on macOS and `true` elsewhere, and `the_native_menu_is_detached_only_where_it_would_swallow_the_click` asserts it. The `.slint`'s menu carries `Quit Gregg` and nothing else. |
| The pulldown is positioned and clamped within the work area on **every** path that opens it, and the native exercises assert the position | **Met** | The first open is deferred into the event loop and the placement is retried until the native window exists, so the work area is available on every path. The exercises parse the GUI's own `pulldown=placed` line and assert the position — and both legs **passed** in CI run `39811582623`: Linux placed it at (852, 36) inside a 1264×980 work area, Windows at (596, 460) inside 1008×708. |
| The pulldown's position and size survive a restart | **Met** | The drag round trip above. |
| The pulldown reads as a dropdown: rounded, shadowed, pointed, content-sized | **Met** | The measurements above. |
| The forms still add, remove, rename and re-point hosts through the CLI's own code, and a refused mutation leaves the config byte-identical and shows a visible reason | **Met** | Unchanged from Plan 191, and covered by the GUI's own `hosts.rs` tests. |
| The GUI still opens no HTTP client, still starts no polling plane, and the `DaemonRequest` set is unchanged | **Met** | `crates/gregg/src/clientd/protocol.rs` is untouched, and the counted harness in `evidence.rs` still proves the GUI adds no request of its own. |
| The GUI's own CI job still formats, lints, tests and release-builds on macOS, Windows and Linux, and the native tray exercises still pass | **Met** | `./scripts/check-gui.sh` passes locally (fmt, clippy `-D warnings`, **128 tests**, a release build of 11,929,712 bytes), and CI run **`39811582623`** is **green on all ten jobs** — including all three `GUI` legs and the native tray exercises on Linux and Windows. |
| The **interactive** native acceptance is recorded per target, or left unchecked and blocking shipment | **Left unchecked** | No tray icon was clicked on any platform. Nothing interactive was performed: no notification-area icon was seen, no pulldown was opened by a pointer, no GNOME session was used. This blocks shipment for Windows and Linux, and the GUI stays a source build. |

### Measurements

| Binary | Size |
| ------ | ---- |
| The GUI after Plan 191 | 11,896,608 bytes |
| **The GUI now** | **11,929,712 bytes** |
| The difference | **+33,104 bytes (+0.3%)** |

The pulldown carries more content than the panel did — the pointer, the shadow layers, the
footer links — and the cost of all of it is 33 KB on an 11.9 MB binary. No dependency was added
to the GUI workspace.

### Deviations from this plan, and why

- **The size is computed by Rust, not by Slint.** Slint 1.18 cannot expose a layout's preferred
  size to a `.slint` file: there is no `preferred-height` on a layout, and a nested id path such
  as `root.body.marker.y` does not compile. The height therefore comes from the model's row
  counts through `placement::metrics`, and a test asserts those numbers against the `.slint`
  literals so the two cannot drift. The detail area is a scroll view, so an estimate that is
  slightly wrong yields a scrollbar, never clipped content.
- **The native menu carries `Quit Gregg` on Windows and Linux**, not nothing. The plan said the
  menu would be "reduced to what a native menu is good at"; a menu with one entry is that, and
  it keeps a right-click on the tray icon useful on the two platforms where a right-click still
  reaches it. On macOS it is detached entirely, because an attached menu swallows the click that
  has to open the pulldown.
- **The tray's tooltip carries the fleet summary** — the counts, and the hosts that are not answering
  only when there are any — so the tray still states the fleet's health without being opened. The
  summary is also in the pulldown's header.
  > **Amended by the seventh amendment.** The summary no longer carries the host names: a line that
  > always carried three counts and a list of names was elided away on any fleet of any size. The
  > native exercise's `--expect-host-name` became `--host-name`, and it now asserts that the
  > tooltip does **not** name a host: the counts are what prove the tray carries the *fleet* and
  > not a placeholder.
- **The shadow is drawn by hand.** Slint's software renderer has no box shadow, and a vendored
  patch of Slint's runtime code is excluded by this plan.
- **The pulldown is dismissed by a click away, and this plan had excluded that.**
  > **Amended by the eighth amendment.** The exclusion rested on the claim that Slint 1.18
  > exposes no focus-lost event, which was never checked against the source and is wrong:
  > `FocusScope` carries `focus-lost(reason: FocusReason)`, and `FocusReason.window-activation`
  > is the desktop saying the window is no longer active — which is what a click away does.
  > The pulldown watches the keyboard focus, aggregates every element that can hold it (the scope
  > that handles the keys and the four fields of the two forms), and takes the focus back when
  > the view changes. The limit that is left is that the signal is *the focus was lost*, not *a
  > click landed elsewhere*: a desktop that takes the focus away for another reason dismisses
  > the pulldown too.
  > **Amended again by the same amendment.** That limit is what macOS could not live with: the
  > pulldown dismissed itself in **four opens out of five** on this machine, because an
  > application took the foreground for its own reasons. macOS now watches the **mouse**
  > (AppKit's global event monitor, safe API, no event tap, no accessibility permission) and
  > excludes the menu bar, because the tray's own click toggles the pulldown. Windows and Linux
  > keep the focus rule, and the limit above is theirs to carry.

### What was not performed

- **No interactive native run on any platform.** The tray item was never clicked, the pulldown was
  never opened by a pointer, and no notification area was seen. The CI legs drive the tray
  *protocol* — the `StatusNotifierItem` on Linux, the native window on Windows — not a desktop
  shell, so they prove the mechanism and the placement, not the gesture.
- The **macOS Intel build was not produced locally**, and there is still **no DPI measurement** at
  125 % or 150 %. The pulldown is placed in logical pixels against the documented shell-strip
  approximation, so the default placement's gap under the menu bar — the 26 px macOS places
  between the request and the window — has not been measured at either scaling.
- **Wayland was never executed.** The CI Linux leg runs under `xvfb-run`, which is X11, while
  `winit/x11` and `winit/wayland` are both compiled in.
- The **pre-existing core Windows handle-inheritance defect** is untouched: a command that
  spawns the daemon and pipes its output still hangs on Windows. It is recorded in
  `apps/gregg-gui/EVIDENCE.md` and is not a Plan 192 change.
- **The default local check does not pass on this host, and did not before this plan either.**
  `cargo test --workspace --all-targets --all-features` fails 13-16 tests here — the `eggpool`
  worker tests, the `scheduler` fleet-scaling tests and
  `clientd::ipc::tests::a_config_inside_the_temp_directory_yields_one_location`. Re-verified
  for this commit on a **pristine worktree at `ce25e35`**, with none of this plan's changes
  present, and the same tests fail there: the `ipc` test hardcodes `/tmp` while the code uses
  `std::env::temp_dir()`, so it fails wherever the temp directory is not `/tmp` (this host sets
  `TMPDIR`), and the `eggpool`/`scheduler` tests combine tokio's `start_paused` auto-advance
  with real loopback TCP I/O, so a loaded machine advances virtual time past the connect before
  the request is issued — the host's load average was 16-17 on 14 cores. This plan changed **no
  file under `crates/`**, so the root workspace's compiled code is byte-identical to `HEAD`.
  These were not "fixed" here: they are outside this line's scope, and changing the tests to
  pass would mask a real environment sensitivity. The same condition is already recorded in
  `plans/README.md`.

### What closing this plan would mean

The interactive acceptance gate above, recorded per target: a visible notification-area icon,
a left-click that opens the pulldown, and the pulldown's content being usable on a real
desktop — on macOS, on Windows and on a real GNOME session, recording which gesture opens it
there. Until that is done, the GUI stays a **source build** and is not shippable for Windows or
Linux.

### Fourth amendment — the detail view became the fleet view's own block (2026-10-23)

The plan's fleet view is a table of blocks, and the detail view was a
**second rendering** of the selected host: its own metric table, with its own
bar columns, its own column widths and its own headings, and after it the
drives, the interfaces and the `EggPool` plane as bare `name  value` lines.
Two things changed, and both are visible in the plan's own terms:

- **The detail view is now the fleet view's own block.** The selected host's
  block is drawn by the **same `HostBlock` component** the fleet view draws
  every host with, so there is **exactly one bar per used share in the panel**
  and the two views cannot drift apart. The geometry moved into that one
  component with them. The detail view's own metric table is gone.
- **The extra facts are one condensed list**, under quiet group headings: a
  two-column row — the name on the left, the value on the right — for the
  numbers behind the shares, for each drive's own facts and for each
  interface. The `EggPool` plane is a group of the same list rather than a
  pane of its own.

The pulldown's **window** is now built on the **first tray click** rather
than at start-up, and a hidden pulldown is not re-rendered. That is the plan's
"one surface, no second poller, no chrome" taken to its conclusion for the
background case: a menu-bar app that nobody has opened yet should not be
paying for a window it never shows. The measured effect, on macOS 15 (arm64)
against the real daemon and a seven-host fleet: the idle resident set fell
from **82 MB** to **45.5 MB**. The tray's click was exercised natively: the
pulldown did not exist before the click, and the click built it, placed it
and showed it. `--panel` and a desktop with no tray still open it at
start-up.

**What was not performed:** the pulldown's own footer control was not clicked
natively — the panel's window is on another macOS Space in this environment,
which is environmental and not the change's doing, so the previous binary was
started and clicked the same way and behaved the same. The detail view was
rendered natively from a build whose panel opened directly in the detail view
(a temporary start-up default, since reverted).

CI run **`39909338640`** on `62e74f9` is **green on all ten jobs**, including the
three `GUI` legs — the only authority for this workspace's Windows and Linux
behaviour.

### Fifth amendment — the pulldown reached over another application's full screen (2026-10-23)

This was a **defect, and it was not new**: since the pulldown existed,
clicking the tray icon while another application was in full screen built the
pulldown and showed the operator **nothing** — and dropped the full-screen
application out of its space on the way.

The cause is macOS's own rule, and it has two halves. A full-screen
application owns a **space** of its own, and a window that is not in that
space is not on the screen at all. And macOS keeps a **Dock** application's
windows out of another application's space, and that right is **fixed when the
window is created** — no window level and no collection behaviour changes it.
The GUI was a Dock application, because `winit` sets
`NSApplicationActivationPolicyRegular` for an unbundled binary.

Reproduced natively on macOS 15 (arm64), with TextEdit in full screen
(`0,33 1512x949`, on screen) and the tray icon clicked after the menu bar was
revealed:

```text
gregg-gui: pulldown=placed x=844.28125 y=36 width=606 height=478 area=8,36,1496x938
60184  layer=3    onscreen=false  gregg-gui  Gregg  844,65 606x478
frontmost: MiniMax Code
```

The click **was** delivered — the pulldown was built and placed — but its
window was `onscreen=false`: it was in the GUI's own Space, on one the
operator was not looking at. And the display had **left** the full-screen
space altogether.

The fix is three native things, all in `apps/gregg-gui/src/platform.rs` and
all inside `#[cfg(target_os = "macos")]`:

1. the application installs the **`Accessory`** activation policy — a
   menu-bar-only application — **before any window exists**, through winit's
   event-loop builder, which is where the desktop makes that decision itself;
2. the pulldown's **native window** is told to join every space
   (`CanJoinAllSpaces`, `FullScreenAuxiliary`, `Stationary`,
   `IgnoresCycle`), to stay visible when the application is deactivated, and
   to sit at **`NSPopUpMenuWindowLevel`** (101) — the level a native status
   item's own menu is drawn at. `always-on-top` reaches only the *floating*
   level (3), and the floating level is below full-screen content;
3. the native window is found by its **title**, and that title is **owned by the
   `.slint` file** — bound to a property the Rust side reads — so the two cannot
   drift apart. A title that lived in two places would be a title that could
   drift, and the overlay would silently stop working.

Verified natively on the same machine, with the same full-screen TextEdit and
the same click:

```text
60145  layer=0    onscreen=true   TextEdit  fs-note.txt  0,33 1512x949
60294  layer=101  onscreen=true   gregg-gui  Gregg       844,65 606x478
frontmost: gregg-gui
```

TextEdit is **still in full screen** — `1512x949`, on screen — and the
pulldown is on screen above it at the pop-up-menu level. The ordinary desktop
path is unchanged, `Esc` still dismisses the pulldown, `--panel` still opens
it at start-up, and the idle resident set is unchanged at **35.1–36.2 MB**
against the **35.8 MB** minimum measured before.

Two visible consequences are deliberate: the GUI has **no Dock icon**, and the
menu bar stays the other application's — which is what a native menu-bar extra
does. The menu bar itself is still macOS's business: it is hidden in full
screen, and the operator brings it back by moving the pointer to the top of
the screen.

**What was not performed:** this was exercised **natively on macOS only**. No
leg of the GUI CI job drives the macOS tray, so this behaviour is **not**
covered by CI; it is covered by the native exercise above. The Windows and
Linux legs are authority for those platforms, and the whole change is inside
macOS's own conditional.

**One flake was found and fixed on the way.** The `GUI (ubuntu-latest)` leg
failed once with *"the GUI never reported where it put the pulldown"*: the
Linux exercise read the GUI's transcript **once** and looked for the placement
line, and the line had not arrived yet — the pulldown's native window is
created on the event loop's own terms, so the line comes when it comes. The
re-run of the same commit passed, and the two runs before it passed, with
byte-identical code, so this was the harness's timing and not the GUI's. The
Linux exercise now **waits** for the line, within the budget it already has,
which is what the Windows exercise already did.

CI run **`39927427831`** on `22e1bce` is **green on all ten jobs**, including
the three `GUI` legs — the only authority for this workspace's Windows and
Linux behaviour. The implementation itself, `d0cd31c`, was green on all ten
jobs on run **`39922180617`**.

