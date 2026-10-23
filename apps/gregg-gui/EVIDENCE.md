# Plan 190 evidence: the counted multi-frontend proof

This is the recorded artifact for the Plan 190 blocking gate: a **counted**
proof that attaching more frontends to one config does not change what the
client daemon asks the remotes for, and that the GUI never adds a remote
fetch of its own.

Every number below was produced by an actual run of the harness in
`apps/gregg-gui/src/evidence.rs`. Nothing here is an expectation, a design
intent, or a comment.

## The corrective pass (2026-10-21)

The counted proof above is unchanged. This section records what a later
corrective pass added, and — as importantly — what it did **not** manage to
observe.

**1. The Windows build was broken, and nothing noticed.** The `#[cfg(windows)]`
branch of `is_executable_file` in `crates/gregg/src/clientd/launch.rs` asked
`std::fs::Metadata` for its `extensions()`, a method that does not exist, so
the whole `gregg` crate failed to build on Windows and the Windows CI job was
red for every commit of the GUI line (run `39628897811`). The GUI could not
have been built for Windows at all, because it depends on that crate. The
branch is fixed — the extension lives on the **path**, and the rule is the one
Windows applies when it is handed a path to run — and locked by Windows-gated
tests that run on the native Windows job.

**2. The monitor window's lists did not scroll.** The host list and the
scheduler job list were unbounded, so a large fleet pushed the metrics, cron
and `EggPool` panes out of the fixed-size window. Both are now in scroll
containers with bounded heights.

*Observed* (macOS, this host): 25 fixture instances, one port each — the
config validator rejects duplicate endpoints — a config with 25 `[[systems]]`
(UUID v4 ids), and the real GUI against the real client daemon. The GUI
printed `gregg-gui: tray=available`, attached, and the monitor window showed
the fleet summary (`25 online, 0 pending, 0 offline — fixture-host-01, …`),
the host list **clipped to about three and a half rows with the vertical
scrollbar drawn at the right edge** and the row text clear of it, the first row
highlighted as selected, and the METRICS, CRON and EGGPOOL panes and the
Reload / EggPool / About / Quit buttons **all still inside the 560×520
window**.

*Not observed*: the **macOS menu bar** could not be photographed. Another
session's full-screen application covered the bar for the whole run, the
Gregg window sat on a non-active Space whenever another application held
focus, and `CGWindowListCreateImage` is unavailable on this macOS release.
The only tray evidence from that run is the machine-readable line above and
Slint's unchanged `NSStatusItem` path. No tray-icon visual proof is claimed.

**3. The no-tray fallback did not exist.** The native matrix requires, for a
desktop without a `StatusNotifierItem` host, that the application "must provide
a visible/failsafe window and clear guidance, not hide its only control
surface". The GUI hid its window and had no fallback at all. It now probes the
desktop (on Linux, the `org.kde.StatusNotifierWatcher` name on the session
bus — exactly what Slint's `ksni` backend requires), reports the answer on one
machine-readable line, shows the operator what is going on, and **closes by
quitting rather than hiding** when there is no tray. The decision is a pure,
unit-tested function, and the probe is bounded and re-run at close time.

**4. The GUI workspace now has its own CI.** The `gui` job formats, lints,
tests and release-builds this workspace on macOS, Windows and Linux natively,
and runs native tray exercises on the Windows and Linux legs.

*The Linux leg, natively, in CI run `39650126619`.* The exercise implements the
real StatusNotifierItem protocol: `apps/gregg-gui/native/sni-host` owns
`org.kde.StatusNotifierWatcher`, serves the watcher interface, starts the real
GUI against the repository's own fleet fixture, and reads the item the GUI
registers. What it observed:

```text
linux-tray: pass 1: a StatusNotifierItem host is present
sni-host: PASS watcher-host-owned this helper owns org.kde.StatusNotifierWatcher at /StatusNotifierWatcher
sni-host: PASS gui-reported-tray-available the GUI printed `gregg-gui: tray=available` with a tray host present
sni-host: PASS gui-registered-an-item the GUI registered org.kde.StatusNotifierItem-22071-1
sni-host: PASS watcher-lists-the-item RegisteredStatusNotifierItems is ["org.kde.StatusNotifierItem-22071-1"]
sni-host: obs ToolTip.title="Gregg — 1 online"
sni-host: obs Menu.labels=["1 online, 0 pending, 0 offline — gui-fixture", "Open Monitor", "Reload", "About Gregg", "Quit Gregg"]
sni-host: PASS tray-tooltip-carries-the-fleet-status the tooltip is "Gregg — 1 online"
sni-host: PASS tray-menu-carries-the-fleet a menu label mentions "gui-fixture"
sni-host: PASS tray-item-has-an-icon IconName="", IconPixmap.entries=1
sni-host: PASS tray-item-has-a-menu the menu is at /MenuBar
sni-host: PASS gui-is-still-running the GUI is still running with a live tray
sni-host: PASS all-checks mode=WithHost
linux-tray: pass 2: no StatusNotifierItem host is present
sni-host: PASS no-tray-host-present nobody owns org.kde.StatusNotifierWatcher
sni-host: PASS gui-reported-tray-unavailable the GUI printed `gregg-gui: tray=unavailable reason="no StatusNotifierItem host on this desktop"`
sni-host: PASS gui-survives-without-a-tray the GUI is still running three seconds after reporting `tray=unavailable`
sni-host: PASS all-checks mode=NoHost
linux-tray: PASS both Linux tray passes
```

So the Linux tray is proven natively — the item registers with a real host,
its tooltip carries the live fleet status, and its menu carries the fleet
summary and the five items — and so is the **no-tray fallback**, on a desktop
with no host at all: the GUI reports the reason and keeps running instead of
dying.

The transcript above is what run `39650126619` actually printed, and it is
kept verbatim. The menu has changed twice since: the entry that opened the
monitor window was renamed **Open Gregg** when there was no separate monitor
window to open, and **Plan 192 removed the launcher entirely** — the native
menu now carries `Quit Gregg` and nothing else, the summary lives in the tray's
**tooltip**, and the tray's **click** opens the pulldown. The exercise
**records** the menu labels it observes rather than asserting on them, so a
fresh transcript appears in the GUI job's log on every push.

*The Windows leg* starts the real GUI, waits for its `tray=` line, and proves
with `EnumWindows` that it created a **native top-level window** for its own
process, and that the client daemon it launched is running. It does **not**
claim the notification-area icon was rendered: the runner is session 0 with no
`explorer.exe`, so there is no notification area to observe. The
**interactive** native runs — a visible icon in a real shell, a real GNOME
session — remain outstanding and are not claimed.

The first run of the job (`39647073131`) is also worth reading: it caught two
defects no root job could see. The GUI's Unix-only test helpers were dead
code on Windows, which is a build failure there because the job compiles the
tests with `-D warnings`; and the Linux runner was missing
`libxkbcommon-x11`, so the GUI panicked as it opened its window — after the
D-Bus half of the exercise had already passed.

**5. The client daemon could not be launched on Windows at all.** The next
Windows run failed with a *real* failure, not a flake: the GUI started,
printed `gregg-gui: tray=available` and created a native window, but the
client daemon it launched never came up. The GUI's own reason named the
cause:

```text
gregg-gui: cannot attach to the client daemon: failed to read
\\.\pipe\gregg-client-a38b32a809d9a7d4.launch.lock:
The system cannot find the file specified. (os error 2)
```

The launch lock's path was derived from the endpoint, and on Windows the
endpoint is a **named pipe**: the whole pipe namespace is a *device*
namespace, not a filesystem, so a path inside it can never exist. Opening one
asks for a pipe instance with that name and fails with
`ERROR_FILE_NOT_FOUND`, so the lock could never be taken and the launcher
failed **before it ever got as far as spawning a daemon**. This was never a
GUI defect: the **terminal client** takes the same lock, so it could not
launch a daemon on Windows either — the GUI merely reported it first.

Two things follow from that, and both are now true:

- the Windows lock is a **real file beside the config**, named after the same
  identity digest (the Unix path is unchanged), so both platforms keep the
  same properties: one lock per config, never the config mutation lock, and a
  path a file can actually be created at;
- the launcher's Windows rules now live in their own
  `#[cfg(all(test, windows))]` module. The old module was
  `#[cfg(all(test, unix))]`, so the `#[cfg(windows)]` tests written inside it
  — including the ones added for the first defect above — **never compiled on
  Windows and never ran**. Six Windows tests now exist and run natively: the
  three executable rules, and three that pin the launch lock — that it is not
  in the pipe namespace, that it is a file which can really be locked, and
  that it stays distinct per config and from the config lock.

The Windows exercise also now proves the **core** launch path on its own,
before the GUI is started at all: `gregg daemon restart` takes the lock,
spawns a daemon and waits for it to answer, `gregg daemon status` identifies
it through the daemon's own handshake, and `gregg daemon stop` releases it.
That is the path the terminal client takes, and it was the path the defect
broke. The daemon is stopped again afterwards, so the GUI that follows still
has to launch a daemon of its own, and the "the fleet fixture served a
request" check stays a proof about the GUI's own daemon rather than about
this one.

After the fixes, the exercise passes on the native Windows runner (CI run
`39667817523`, job `GUI (windows-2022)`):

```text
windows-tray: proving the core client-daemon launch path
windows-tray: daemon restart: client daemon d45402fdf71b3bd9 restarted
windows-tray: daemon status after the core launch: client daemon d45402fdf71b3bd9 running (protocol 1, version 1.0.15) at \\.\pipe\gregg-client-d45402fdf71b3bd9
windows-tray: the core launch, the status handshake and the stop all work on Windows
windows-tray: starting the GUI
windows-tray: the GUI printed: gregg-gui: tray=available
windows-tray: top-level window: 589996|hidden|Gregg
windows-tray: top-level window: 262650|visible|
windows-tray: top-level window: 327998|hidden|Default IME
windows-tray: the GUI created 3 native top-level window(s)
windows-tray: the GUI process 5692 is still running
windows-tray: daemon status: client daemon d45402fdf71b3bd9 running (protocol 1, version 1.0.15) at \\.\pipe\gregg-client-d45402fdf71b3bd9
windows-tray: the client daemon is running
windows-tray: the fleet fixture served 8 request(s): the daemon is polling
windows-tray: PASS every Windows tray check
```

Read it as four separate claims, because they are four separate claims: the
**core** launch path works on Windows (the first three lines, with no GUI
involved); the GUI starts and creates a **native** window; the client daemon
**the GUI launched** is running, identified through the daemon's own local
protocol handshake on the named pipe; and that daemon is really **polling**,
because the fleet fixture served eight requests.

The six Windows launcher tests also ran natively in the same run, in the core
`Windows` job:

```text
test clientd::launch::windows_tests::a_windows_executable_is_recognised_by_its_extension ... ok
test clientd::launch::windows_tests::a_windows_name_without_an_extension_is_runnable ... ok
test clientd::launch::windows_tests::the_runnable_windows_client_wins_over_the_bare_name ... ok
test clientd::launch::windows_tests::the_windows_launch_lock_is_distinct_per_config_and_from_the_config_lock ... ok
test clientd::launch::windows_tests::the_windows_launch_lock_is_not_in_the_pipe_namespace ... ok
test clientd::launch::windows_tests::the_windows_launch_lock_is_a_file_that_can_actually_be_locked ... ok
```

**6. An outstanding Windows defect in the launcher, found by that exercise and
not fixed.** The exercise hung the first time it ran the new core-launch
check, and the hang was not the check's fault: it was a real property of the
Windows spawn.

`gregg daemon restart` finished — every step of it is bounded — but the
PowerShell pipeline it ran in never returned. The reason is that on Windows a
detached child inherits **every inheritable handle** the parent holds, not
only the three standard ones: Rust has to pass the null stdio handles to the
child, so it creates the process with `bInheritHandles = TRUE`, and Windows
hands over the write end of the launcher's stdout pipe as well. The daemon
kept that pipe open for as long as it lived, so the pipe never reached EOF.
The daemon outlives the launcher by design, so "for as long as it lived" is
forever.

On Unix this cannot happen: `Stdio::null()` there really is `/dev/null`, and
no descriptor of the launcher's is inherited. The launcher's own comment
claimed the daemon "must not inherit a pipe that would keep a parent
waiting" — true on Unix, false on Windows. The comment now says so.

It is **not fixed here**, and it is recorded as an outstanding defect rather
than quietly worked around. Closing it needs a handle *list* —
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` through `STARTUPINFOEX` — which the
standard library does not expose, so it means `windows-sys` plus `unsafe`
code in `crates/gregg/src/clientd/launch.rs`, which the repository's unsafe
allowlist does not currently cover. That is a core change of its own, not a
corrective pass on the GUI line.

What it does and does not affect:

- the **GUI** is not affected: its standard streams are a console or a file,
  never a pipe, so there is no EOF to withhold;
- the **terminal client** is not affected in normal use, where stdout is the
  terminal;
- a **CLI command that spawns the daemon, whose stdout is a pipe**, is
  affected: `gregg daemon restart | tee log` waits for a reader that will
  never be satisfied.

The exercise starts that command with `Start-Process` and redirects its
output to **files**, never through a pipeline. PowerShell's `*>` redirection is
not enough — it still hands the child a pipe and copies it into the file, so
the daemon's inherited write end keeps *that* pipe open; `Start-Process
-RedirectStandardOutput` passes a real file handle, which has no EOF to
withhold. That is the same way the GUI itself is started in this exercise,
and the GUI also spawns a daemon. The exercise also waits for the command
with an explicit deadline, so a check that cannot fail reports itself in one
line instead of hanging the step for fifteen minutes. None of this is a
workaround for the defect: the defect is in the launcher, and the exercise
only stops tripping over it.

**7. The footprint was re-measured against a rebuilt baseline.** The scroll
container comes from Slint's `std-widgets`, so both binaries grew: the GUI to
**10,803,360 bytes** and the matched Slint-only baseline to **9,956,032
bytes**. The difference — what this frontend adds over a pure-Slint build of
the same UI — is **847,328 bytes**, against 847,344 bytes before the change:
16 bytes, on a ten-megabyte binary. The earlier pair was **9,776,560 bytes**
against **8,929,216 bytes**, so both binaries grew by roughly a megabyte —
1,026,800 and 1,026,816 bytes respectively — and the growth was the style's
own assets, on both sides. A baseline that was not rebuilt would have
reported the whole growth as the GUI's cost.

**8. Plan 191 re-measured the footprint for the pulldown, and the measurement
is per crate.** The GUI is now **11,896,608 bytes** against **10,803,360**
before the plan: **+1,093,248 bytes (+10.1%)** for the pulldown and the
host-management forms together. `cargo bloat --release --crates` attributes
it: the GUI's own crate grew **418.5 KiB** (285.1 → 703.6 KiB), which is the
generated code for the `Button` and the `LineEdit` the forms need, plus
`toml_edit` (+114.7 KiB), `std` (+110.9 KiB), `i_slint-core` (+74.8 KiB),
`gregg` (+48.7 KiB) and `serde_core` (+24.7 KiB).

Two facts in that measurement matter more than the total.

- **The style's own assets did not grow.** The font stack (`harfrust`,
  `skrifa`, `zeno`, `parley`, `read_fonts`, `fontique`) is 1,356.9 KiB before
  and 1,358.0 KiB after; the SVG stack (`usvg`, `resvg`, `tiny_skia`,
  `svgtypes`, `roxmltree`, `simplecss`) is 949.4 KiB before and 947.3 KiB
  after. The scroll container had already paid for them, so keeping the
  `std-widgets` import cost 418.5 KiB of widget code rather than a megabyte of
  style.
- **The HTTP and TLS stack is absent from the binary.** An earlier draft of
  the host-management path called `gregg::cli::dispatch`, which also reaches
  the daemon commands, and the daemon reaches the HTTP client and rustls:
  that binary was **14,761,600 bytes**, **2,864,992 bytes** more, with
  `rustls`, `ring`, `hyper`, `http`, `webpki` and `eggfetch_core` all linked
  in. The GUI now calls `gregg::cli::add_host` and `gregg::cli::remove_host`
  — the same code paths `gregg add` and `gregg remove` take — and those
  crates are not in the binary at all.

The **runtime** figures in the README are the monitor window's, and Plan 191
did **not** re-measure them on the 25-host fleet, because the harness that
produced them is not in the repository. The pulldown was measured separately
against a one-host configuration on the same host, and the ranges overlap
(68–104 MB with the panel open, 65–76 MB with it hidden), so the honest
reading is that the pulldown is **not** lighter in memory than the monitor
window was. The **request-budget** result above is unaffected: the GUI still
attaches to the same daemon and still adds no request of its own.

The CI that carries Plan 191 is run **`39727675355`**, green on **all ten
jobs**: the core `Linux`, `macOS`, `macOS Intel`, `Windows`, `Scripts`, `MSRV`
and `FreeBSD` jobs, and the three `GUI` legs (`macos-15`, `windows-2022`,
`ubuntu-latest`). The `GUI` legs are the only authority for this workspace's
Windows and Linux behaviour, because this workspace is not a member of the
root workspace and no root job compiles it.

### The footprint figure was `ps` RSS, and the physical footprint is a quarter of it

Re-measured on 2026-10-22. The "72–83 MB resident" and "68–104 MB" figures
above are `ps` RSS. On macOS that counts shared, clean, reclaimable pages —
the dyld shared cache, `icudt78l.dat`, the system font collections and the
macOS appearance resources — which every app on the machine maps, and which
`footprint` correctly reports as reclaimable. The **physical footprint**, which
is what the machine actually pays and what Activity Monitor shows:

| configuration | `ps` RSS | physical footprint | Rust heap (`MALLOC_SMALL`) |
| ------------- | -------- | ------------------ | ------------------------- |
| The panel open | 71–92 MB | **18 MB** | 9 MB |
| The panel hidden | 67–76 MB | **11 MB** | 7 MB |

Three further measured facts, and one lever that is real but unreachable from
inside the process:

- **The memory does not scale with the fleet.** 1 host on a 5 s poll: 18 MB
  footprint, 9 MB heap. 25 hosts on a 3 s poll: 19 MB footprint, 9 MB heap.
  The fleet projection is not what the memory is made of.
- **There is no leak.** 150 s of continuous polling left the footprint flat at
  22–24 MB.
- **The system-font scan is the one sizeable, addressable cost.** Slint's
  software renderer asks CoreText for every font descriptor and then mmaps and
  parses every font file on the system — 645 families on this host. Measured
  in isolation against `fontique` directly: **+6,880 KB RSS, +3,152 KB
  footprint and 182 ms of blocking startup**, paid at `SlintContext::new`,
  before the panel's first paint. Slint 1.18.1 gives **no** supported way to
  turn it off: the software renderer's `std` feature includes `systemfonts`
  unconditionally, so Slint's own `software-renderer-systemfonts` feature is a
  no-op — removing it produced a **byte-identical** binary. Removing it would
  need a vendored patch of Slint's runtime code, which is not worth 3.2 MB
  and 180 ms.
- **`MallocSpaceEfficient=1`, set before the process starts**, takes the heap
  from 14 MB to 8 MB and the footprint from 26–28 MB to **14–15 MB**, with the
  CPU unchanged (0.10 % of one core either way). macOS's malloc reads the
  variable in `malloc_init()`, before `main()` runs: setting it from inside the
  process was measured and does **not** work, so it would have to come from a
  launcher's environment, an `LSEnvironment` plist entry, or a re-exec.
  `mimalloc` as a global allocator was measured and gives **no** gain
  (13–16 MB heap); a smaller panel is worth about 1 MB; and a GPU renderer
  would add a graphics dependency the plan deliberately avoided without
  touching the font scan.

Two earlier runs on the same branch failed, and both are recorded here because
the failures are what the native Windows job is for. `39726097298` failed on
the Windows Clippy gate — a `dead_code` on an enum variant that only one
platform constructs, and a `clippy::result_large_err` on a closure over the
store's error type, which is larger on Windows. The second was **not** silenced:
the test was changed to write the whole configuration instead of mutating it.
`39727156228` failed on the Windows test gate, because a test asserted the
desktop shell's strip is always at the top; it is at the top on macOS and Linux
and at the bottom on Windows. The fix removed the platform conditional from the
arithmetic rather than correcting the assertion, so both cases are now tested
on every platform.

Neither failure was visible on the author's macOS host. The first two were
caught by a local `--target x86_64-pc-windows-gnu` Clippy cross-check, which is
a development aid and **not** closure evidence; the third was caught by the
native Windows job, which is the authority. The Linux leg could not be
cross-checked at all from macOS, because the fontconfig system library is not
available for cross-compiling.

### The interaction model was replaced by Plan 192, and two claims did not hold

Running the committed build on 2026-10-22 showed that the tray's native menu
was a **launcher** for a separate window, so the content was not in the
dropdown, and the window carrying it read as a small application rather than a
popover. **Plan 192 replaced that interaction model**: the tray's *click* opens
the pulldown, and the pulldown carries the content. The mechanism is Slint's
`SystemTrayIcon.clicked`, and a native menu with no entries detaches itself so
the platform routes the click there instead of popping the menu — on macOS
(`i-slint-core-1.18.1/items/system_tray/appkit.rs:248`), on Windows
(`windows.rs:290-298`, where a right-click still opens the native menu) and on
Linux (`ksni.rs:284`). The `if cond : Menu { … }` form was **verified by
compiling it** in this workspace.

Two claims in Plan 191's record did not hold, and are corrected there:

- **The panel was never positioned on the startup path.** Slint's
  `with_winit_window` "will only succeed when the event loop is active", and
  the startup path calls it before the event loop runs, so `work_area()`
  returned `None`: no `set_position` at all, no clamping, and the window at
  x=1640 on a 3840-wide display instead of under the menu bar. Deferring the
  first open into the event loop gives `work_area=Some(left 8, top 36,
  3824×1036)` and the window at **(3272, 36)**.
- **The position and size were not kept across a restart.** They live in
  `UiState`, in memory, and the GUI writes no state file.

And the **Wayland** path has never been executed: the CI Linux leg runs under
`xvfb-run`, which is X11, while `winit/x11` *and* `winit/wayland` are both
compiled in. Wayland is the modern GNOME default, so the pulldown's behaviour
there is untested.

### Plan 192's own run, on macOS

Run on 2026-10-22 against the real `~/Library/Application Support/gregg/gregg.toml`
and the repository's own `gregg` client, started through the `--panel` path —
the path Plan 191 left unpositioned.

```text
gregg-gui: tray=available
gregg-gui: pulldown=placed x=3412 y=36 width=420 height=658 area=8,36,3824x1036
```

That is the machine-readable placement line, and it says three things at once:
the work area **is** available now, the position is **inside** it, and the size
is the **content's** size (420×658 for a seven-host configuration) rather than
a fixed rectangle. The window was then measured from the system's own window
list — **420×658 at (3412, 62)** — and the card's 1 px border was measured on
a screen capture at **y = 76**.

**The 26 px between the requested top and the observed one is macOS's own
window placement, and it is recorded rather than hidden.** `NSScreen.visibleFrame`
puts the real menu bar's bottom edge at **30 pt** on this display; the work
area's top edge is 36, which is the documented 28 pt shell-strip approximation
plus the 8 px edge margin. A window requested *flush with that edge* is placed
26 px lower by AppKit. A **remembered** position is honoured **exactly**:
(2500, 120), (2300, 220), (3000, 200) and (3000, 50) were each requested and
each observed at the same coordinates. So the operator's own placement is
precise, and the default placement opens *clear of* the menu bar with a visible
gap rather than under it. The pulldown was inside the work area in every
observation.

**The geometry round trip was observed end to end, not inferred.** The pulldown's
header strip was dragged with synthesised pointer events, from (2600, 148) by
(−200, +100); the window moved by exactly that delta and the state file appeared
beside the config:

```text
position=2300,220
size=500,600
```

The GUI was then stopped and started again with that file in place, and it
opened at the remembered geometry rather than at the corner:

```text
gregg-gui: pulldown=placed x=2300 y=220 width=500 height=600 area=8,36,3824x1036
```

The state file is written when the operator **moves** the pulldown — the drag
raises `geometry-changed` — and not on the
first placement, which is derived from the content and the work area and so has
nothing to remember. `src/state.rs` reads it bounded (4 KiB) and validated and
writes it atomically, and falls back to the default on a corrupt or
out-of-range file; 13 unit tests cover that path.

> **Superseded.** The corner grip is gone — see *"The panel had a band of dead
> space above the fleet"* below — so the state file holds a **placement and
> nothing else**, and the size is the content's size.

**What was measured about the dropdown's shape:** the card's corners are rounded
(a 12 px radius), the three shadow layers are visible around it, the pointer is
drawn toward the menu bar, and the pulldown is **transparent on macOS** — the
window's own background is `#00000000` there and `Palette.background` everywhere
else. Two renderer facts are now in the `.slint`: the software renderer's
`draw_box_shadow` is an **empty stub**
(`i-slint-renderer-software-1.18.1/lib.rs:3059`), so the shadow is drawn by
hand as three translucent rounded rectangles; and a `Text` keeps its unwrapped
width as a minimum unless `overflow: elide` is set, so every wrapping text in
the pulldown carries `overflow: elide` — without it a long status line
overflowed the pulldown on **both** sides.

**What was not done:** no tray icon was clicked on any platform, so the
**interactive** native acceptance is still outstanding. The macOS Intel build
was not produced locally, there is still no DPI measurement at 125 % or 150 %,
and Wayland was never executed. The GUI's release binary is **11,929,712
bytes**, **+33,104 (+0.3%)** on Plan 191's 11,896,608, with no new dependency in
this workspace.

### The pulldown's chrome was revised, and the "transparent on macOS" claim did not hold (2026-10-22)

The section above records the dropdown's shape as it was measured for Plan
192: a 12 px corner radius, "the three shadow layers", and a pulldown that is
**transparent on macOS**. Two of those claims are now superseded, and the
first one hid a real defect.

**The pulldown was never transparent, and the reason was not a platform
limit.** On macOS a frameless Slint window *is* a transparent `NSWindow` —
`winit` asks for exactly that (`NSWindow.opaque = false`, `backgroundColor =
clear`) and `i-slint-backend-winit` asks for transparency whenever the window's
background is not opaque or the window has no frame. The window server
honours it: `CGWindowListCreateImage` reports the unpainted region as alpha 0.

The **framebuffer** was the problem. `softbuffer`'s macOS backend hands
CoreGraphics a `CGImage` whose pixel format is declared as
`CGImageAlphaInfo::NoneSkipFirst` — **no alpha channel at all**, the first byte
of each pixel being an unused filler. The software renderer writes a correct
premultiplied alpha into that byte and CoreGraphics discards it, so every
transparent pixel of the window was presented **opaque**.

The visible consequence was the defect this revision fixes: the pulldown's
14 px shadow margin is transparent by design, so the desktop should show
through it — and instead it was presented as a **thick black frame**, and the
card's rounded corners were black squares.

**How the defect was established.** A purpose-built AppKit probe presented the
*same* premultiplied RGBA bytes under both declarations and read the result
back with `CGWindowListCreateImage`:

| Declared alpha          | Border pixel       | Meaning                          |
| ----------------------- | ------------------ | -------------------------------- |
| `noneSkipFirst` (6)     | `(0, 0, 0, 255)`   | the alpha was dropped — opaque    |
| `premultipliedFirst` (2)| `(0, 0, 0, 0)`     | the alpha was honoured            |

The pulldown itself was then captured with `screencapture -l`, which reports
the window's own alpha. **Before** the fix the 14 px margin read
`(0, 0, 0, 255)`; **after** it, `(0, 0, 0, 0)`, and the shadow rings became
genuinely translucent — a shadow ring measured `(61, 61, 61, 245)` where the
margin beside it is `(0, 0, 0, 4)`.

**The fix** is one word in a **vendored copy** of `softbuffer` 0.4.8
(`apps/gregg-gui/vendor/softbuffer`): `CGImageAlphaInfo::PremultipliedFirst`
declares the same 32-bit layout with the alpha the buffer already carries.
0.4.8 is the latest release and the defect is still in it, which is why the
copy exists. `softbuffer` was already in the dependency graph through Slint, so
**no crate was added to the build**, and the change is inside Apple's platform
only — the X11, Wayland, KMS, Win32, Android, Orbital and web backends are
byte-identical to upstream. The full rationale, the alternatives that were
rejected and the removal condition are in `apps/gregg-gui/vendor/VENDORED-PATCH.md`.

**What the pulldown's chrome is now**, measured:

- The **shadow** is fourteen one-pixel rings with a Gaussian alpha profile
  (`exp(-x²/50)`), not three hard-edged layers: the three-layer version read
  as a visible three-step band. Over a known light background the composited
  profile ramps smoothly from 17 % darkening at the card's edge to nothing
  14 px out — `249, 248, 246, 243, 239, 235, 230, 225, 219, 214, 210, 208,
  207` — with no banding.
- The **outline** is a measured macOS hairline, `NSColor.separatorColor`:
  `rgba(255, 255, 255, 0.098)` in the dark scheme and `rgba(0, 0, 0, 0.098)`
  in the light one. Slint's own `Palette.border` is `#00000073` in the light
  scheme — 45 % black in a single pixel, which is a hard dark line rather than
  a separator.
- The **fill** is `Palette.background` at 94 % opacity, the closest the
  software renderer comes to a vibrant material.
- The **pointer** moved to the card's **trailing edge**, the edge the tray is
  on: the card anchors to the trailing corner, so a pointer at the card's
  centre pointed at nothing.

Both colour schemes were checked. The dark scheme was captured natively; the
light scheme was captured with the application temporarily forced to the Aqua
appearance, so the light palette and the light hairline could be inspected on
a dark-mode desktop.

**CI:** run **`39833577564`** on `ee83e43` is **green on all ten jobs** — the
core `Linux`, `macOS`, `macOS Intel`, `Windows`, `Scripts`, `MSRV` and
`FreeBSD` jobs, and the three `GUI` legs. The GUI workspace is not a member
of the root workspace, so no root job compiles it: the three `GUI` legs are
the **only** authority for the vendored `softbuffer` copy on macOS, Windows
and Linux, and all three are green. The `GUI (macos-15)` leg is the one that
builds the patched macOS backend; the `GUI (ubuntu-latest)` and
`GUI (windows-2022)` legs build the copy's untouched backends.

**The cost of the revision:** the release binary is **11,962,832 bytes**,
**+66,224 (+0.6%)** on Plan 192's 11,929,712. The vendored `softbuffer` copy
costs **nothing measurable**: it changes one word in a pixel-format
declaration that was already in the binary. The growth is the revised chrome
and the metric bars.

**What was not done:** the interactive native acceptance is still outstanding,
as it was for Plan 192 — no tray icon was clicked and no pulldown was opened
by a pointer. The macOS Intel build was not produced locally, there is still
no DPI measurement at 125 % or 150 %, and Wayland was never executed. The
Windows and Linux legs are **not** affected by the vendored patch, and their
behaviour is authority in the GUI CI job, not here.

### The pulldown became a compact fleet table, and the detail moved behind a control (2026-10-22)

The section above describes the pulldown as it was for Plan 192: one panel
carrying the fleet summary, the host list **and** the selected host's own
numbers. That is **superseded**. The pulldown now has **two views**, and the
fleet view is the TUI's condensed fleet row with bars in it.

**What the fleet view is**, measured on a window capture of the running build
(`screencapture -x -o -l<window id>`, macOS 15, arm64):

```text
 HOST              CPU             MEM        SWP       DISK       NET     LOAD
 Deadpool IO 61.5%  8% 4.03GHz     39%        17%      82%       0%      26.38
 MacbookPro        offline (invalid)
 Raspi 10  IO 73.9%  2% 2.40GHz     22%        0%       48%       0%      11.68
 Plex              offline (invalid)
 Le Potato20  IO 0.6%  4%            54%        34%      49%       —        0.19
 Le Potato21  IO 8.0%  5%            20%        4%       24%       —        0.77
```

- **One line per host**, with a **bar for each of the five used shares** and
  the shares written compactly (`8%`, not `8.3%`) — the way the TUI's
  condensed table writes them.
- The **I/O wait follows the host's name** and the **frequency follows the CPU
  percentage**, in the TUI's own places. The **endpoint is gone** from the
  row: the fleet view names the host and not where it is.
- The **LOAD** column carries the one-minute average, and an **offline host
  has no columns at all** — its row says why it is offline and nothing else.

**The bar's origin was a real defect, and it is fixed.** The bars grew out of
the **middle** of their tracks. The cause is Slint's own layout rule, not the
GUI's: the compiler's `default_geometry` pass centres a child that has a size
and no explicit `x`/`y` inside a parent that is not a layout
(`maybe_center_in_parent`). A `Rectangle` with an explicit `width` and no
`x` is such a child. The measurement, on the build before the fix — the track
spans x=72..147, and every fill was **centred** on it, whatever its share:

| row | fill before | fill after |
| --- | --- | --- |
| CPU, 9.4 % | x=106..113 (centred) | x=73..77 (from the left edge) |
| MEM, 22.9 % | x=101..118 (centred) | x=73..98 (from the left edge) |
| SWP, 9.5 % | x=106..113 (centred) | x=73..77 (from the left edge) |
| DISK, 93.1 % | x=75..144 (centred) | x=73..142 (from the left edge) |

Every element the pulldown places inside another element now carries its own
`x` and `y`.

**The panel clipped its last host row, and that is fixed too.** The
attribution text took a line of its own **even when it was empty** — Slint
measures a `Text` as one line high whatever it says — and a line nobody asked
for is a line the height calculation does not know about, so the panel was a
line too short. The attribution is now wrapped in a rectangle with an explicit
height, like the status line and the message before it. The row that was cut
in half is now whole, and the fleet view's height is exact: six whole rows of
19 px, a heading row of 14 px, and the panel's own chrome.

**Both colour schemes were captured natively.** The light scheme was captured
by forcing the Aqua appearance with a temporary `NSAppearance` diagnostic,
which was **reverted** before the commit; the dark scheme is the desktop's
own. In both, the card's margin is transparent (`(0, 0, 0, 0)` at the corner,
`(0, 0, 0, 15)` in the shadow's falloff) and the card is `Palette.background`
at 94 % (`(241, 241, 241, 244)` light, `(44, 44, 44, 244)` dark).

**The detail view** is behind the pulldown's own footer control and carries
the TUI's metric rows unchanged — the same label, bar, share and value shape,
with the same aggregates and the same "no reading" rules. Clicking the
control re-sizes the panel to what that view carries.

**The cost:** the release binary is **12,062,128 bytes**, **+99,296 (+0.8%)**
on the 11,962,832 of the chrome revision above.

**What was not done:** the interactive native acceptance is still outstanding,
as it was for Plan 192 — no tray icon was clicked and no pulldown was opened
by a pointer. The light scheme was captured with a temporary diagnostic that
is not in the tree. The macOS Intel build was not produced locally, there is
still no DPI measurement at 125 % or 150 %, and Wayland was never executed.
The Windows and Linux legs are authority in the GUI CI job, not here.

**The CI run that covers this work** is **`39846098748`**, against
`7e56a3c`, and it is **green on all ten jobs**: the core `Linux`,
`macOS (macos-15)`, `macOS (macos-15-intel)`, `Windows`, `Scripts`, `MSRV`
and `FreeBSD` jobs, and the three `GUI` legs (`macos-15`, `windows-2022`,
`ubuntu-latest`). The `GUI` legs are the only authority for this workspace's
Windows and Linux behaviour, because the workspace is not a member of the
root workspace and no root job compiles it. The `GUI (macos-15)` leg is the
one that builds the patched macOS backend.

### The fleet view became a table of blocks, and the selection became a quiet tint (2026-10-22)

The section above describes the fleet view as it was after the second revision:
**one line per host** under a row of column headings. That is **superseded**.
The fleet view is now the TUI's condensed fleet row **turned on its side**: every
host is a **block of its own**.

**What the fleet view is**, measured on a window capture of the running build
(`screencapture -x -R<x> <y> 606 478`, macOS 15, arm64, the desktop's own dark
appearance):

```text
Deadpool  IO 40.7%  9.87/11.42/12.26  16C  912.96MHz
CPU   [████████████████████░░░░░░░░░░░░░░]   7%
MEM   [██████████████████████████░░░░░░░░░░]  91%
SWP   [███████░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  24%
DISK  [███████████████████████████████░░░░░░]  93%
NET   [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]   0%
MacbookPro  5.22/6.58/8.24  14C
CPU   [██████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░]  45%
```

- **The header line names the host and carries what is not a used share**: the
  I/O wait, the load, the core count and the frequency, in the TUI's own words
  and the TUI's own order. A host that reports no I/O wait and no frequency
  carries neither (`MacbookPro  5.22/6.58/8.24  14C`): a missing reading is
  omitted, never shown as a zero.
- **The five rows under it are the five used shares**, and each row carries
  **only its share** — the bar and the percentage, nothing else.
- A host that is **not online has no rows at all**: its header says why it is
  offline and nothing else.

**The geometry, measured on the capture** (the panel's own chrome is at
x=14..592, the content starts at x=26):

| thing | measured | designed |
| --- | --- | --- |
| the label column | x=26..90 | 64 px |
| the bar's left edge | x=96 | 26 + 64 + 6 |
| the bar's fill | x=97..287 (7 %), x=97..411 (26 %), x=97..500 (41 %) | the left edge, by share |
| the row pitch | 13 px | 13 px |
| the block | 81 px | 16 + 5 × 13 |
| the panel | 606 × 478 | 606 wide, the host list bounded at 348 px |

**The selection was a real defect, and it is fixed.** The selected block was
tinted with `Palette.selection-background`, and its contents were drawn in
`Palette.selection-foreground`. Measured on macOS, that foreground is
**black** — the selection background is the accent blue `(0, 127, 214)` — so the
selected host's name, its labels, its percentages and **every bar fill** were
drawn black on blue: a bar drawn in it reads as a hole rather than as a
measurement. The selection is now a **quiet tint** — the foreground at 7 %,
`(53, 53, 53)` against the card's `(39, 39, 39)` — with the host's **name in the
accent colour**, and **the bars are the same on a selected block as on any
other**: the fill is `Palette.accent-background` `(104, 208, 255)` and the track
is the foreground's own 15 %.

**Both colour schemes were captured natively.** The light scheme was captured by
turning the system's dark mode off and back on again (`osascript`, appearance
preferences), which was **reverted** before the commit; the dark scheme is the
desktop's own. The light scheme's card is `(224, 224, 224)` and the bars are
the same accent blue on a light track.

**The `offline (invalid)` hosts are fixed, and the fix was verified against the
live daemon.** Two macOS hosts reported `offline (invalid)` while the TUI showed
them correctly. The cause was in the *producer*: the deployed `greggd` 1.0.13
reads `ifa_data` through the 64-bit `if_data64` layout, so the 32-bit
`ifi_baudrate` is picked up from the low half and `ifi_ipackets` from the high
half — `en0` arrives with a capacity of `1326367260874488128` bits per second
instead of its real `304200000`. The v2 protocol bounds a capacity, and the
client's validator rejected the **whole snapshot** for it.

The client now **omits** an unrepresentable optional capacity rather than
failing the snapshot, and accepts the payload only if the *repaired* payload
validates. After restarting the client daemon with the fixed binary, the
daemon's own published snapshot reports all seven hosts `Online`:

```text
Deadpool     192.168.182.143  Online   net agg cap rx=11000000000 tx=11000000000
MacbookPro   127.0.0.1        Online   net agg cap rx=None tx=None
Raspi 10     192.168.182.10   Online   net agg cap rx=11000000000 tx=11000000000
Plex         192.168.182.4    Online   net agg cap rx=None tx=None
Le Potato20  192.168.182.20   Online   net agg cap rx=None tx=None
Le Potato21  192.168.182.21   Online   net agg cap rx=None tx=None
Raspi 8      192.168.182.8    Online   net agg cap rx=None tx=None
```

The two hosts' network **shares** now read as *not measured* (`—`) in the fleet
view, and every other reading on them is shown.

**A host that is not online was observed natively too.** The same build was run
against a configuration carrying a host that answers nothing
(`192.168.182.250:11310`, a timeout of 1500 ms), and the block drew its header
line and nothing else:

```text
Deadpool  IO 35.5%  12.45/12.29/12.27  16C  3.54GHz
CPU   [██████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  14%
MEM   [██████████████████████████████░░░░░░░░░░]  79%
SWP   [████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  21%
DISK  [██████████████████████████████████████░░░]  94%
NET   [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]   0%
Ghost  offline (network)
```

The summary line above it read `2 online, 0 pending, 1 offline`, and the
pulldown's height followed the content: **220 px** for two online blocks and one
offline header, against 478 px for four online blocks.

**The cost:** the release binary is **12,012,560 bytes**, **−49,568 (−0.4 %)** on
the 12,062,128 of the revision above.

**What was not done:** the interactive native acceptance is still outstanding, as
it was for Plan 192 — no tray icon was clicked and no pulldown was opened by a
pointer. The macOS Intel build was not produced locally, there is still no DPI
measurement at 125 % or 150 %, and Wayland was never executed. The Windows and
Linux legs are authority in the GUI CI job, not here.

**The CI run that covers this work** is **`39863031513`**, against `9932b57`, and
it is **green on all ten jobs**: the core `Linux`, `macOS (macos-15)`,
`macOS (macos-15-intel)`, `Windows`, `Scripts`, `MSRV` and `FreeBSD` jobs, and
the three `GUI` legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The `GUI`
legs are the only authority for this workspace's Windows and Linux behaviour,
because the workspace is not a member of the root workspace and no root job
compiles it.

**The root workspace's `cargo clippy --workspace --all-targets --all-features
-- -D warnings` does not pass on this macOS host**, and did not before this
work either: the local toolchain is Rust 1.99.1, whose clippy has new lints
(`borrow_as_ptr` and `clippy::panic`) that fire in `gregg-host`'s **macOS-only**
collector, which no CI job lints — CI's clippy job runs on Linux, where that
module is not compiled, and the macOS CI job runs `cargo check`, not clippy.
Re-verified on a pristine worktree at `cf05ad0` with none of this work present:
the same lints fire there. Nothing under `crates/gregg-host/` was changed here.

### The detail view became the fleet view's own block, and the idle GUI lost its window (2026-10-23)

The detail view used to be a **second rendering** of the selected host: its
own metric table, with its own bar columns, its own column widths and its own
headings, and after it the drives, the interfaces and the `EggPool` plane as
bare `name  value` lines. It is now **the fleet view's own block, opened up**
— the selected host's block, drawn by the very same `HostBlock` component the
fleet view draws every host with — and one condensed list of groups under it.

**There is exactly one bar per used share in the panel.** The `HostBlock`
component carries the geometry (the label column, the share column, the gap,
the bar's height and the row's height), so the two views *cannot* drift apart:
a host that is shown twice is shown twice the same way. The detail view has no
second set of bars, and the old metric table — a second `MetricRow` struct,
a second set of `metric-*` properties and a second set of headings — is gone.

**The groups are one condensed list.** A quiet heading and a two-column row
under it: the name on the left, the value on the right, in the quiet colour.

```text
Deadpool  IO 0.1%  1.73/2.22/4.72  16C  3.05GHz
CPU   [████████░░░░░░░░░░░░░░░░░░░░]  11%
MEM   [██████████████░░░░░░░░░░░░░░]  39%
SWP   [███░░░░░░░░░░░░░░░░░░░░░░░░░░]   9%
DISK  [█████████████████████████░░░░░]  86%
NET   [░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]   0%

RESOURCES
  MEM         6.0 / 15.5 GiB
  SWP         7.3 / 84.0 GiB

DRIVES
  /              791.2 GiB / 914.8 GiB · 86.5% · 77.1 GiB free
  /boot/efi        6.1 MiB / 1.0 GiB · 0.6% · 1.0 GiB free
```

`RESOURCES` carries the numbers **behind the shares** the block already shows,
under the block's own row labels. `DRIVES` carries each mount's own facts in
the TUI's own words, and the free space falls back to what is left of the
total when the platform reported none — the TUI's own rule. `NETWORK` carries
the aggregate first (the block's network share is the aggregate's share of its
link) and then each interface, and a link capacity that was **not measured** is
**not stated**: the row says `RX` and `TX` and no `LINK`.

**The detail view was rendered natively**, on macOS 15 (arm64), against the
live daemon and the real seven-host fleet — in the dark scheme and in the
light scheme (the light capture was taken with the appearance switched and the
switch reverted afterwards). The dark capture is the one reproduced above; the
light one shows the same layout in the light palette, with the card at
`(224,224,224)`.

**The idle GUI lost its window.** The pulldown's component is now **built when
the operator first opens it**, not at start-up: a window costs about thirty
megabytes before it draws anything, and a background GUI nobody has opened yet
should not be paying for it. The measurement, on this machine, against the real
daemon:

| | resident set | CPU |
| --- | --- | --- |
| **idle, the pulldown never opened** | **45.5 MB** (35.8 min, 45.5 max over 60 s) | about 1.6 % of one core |
| the pulldown open | 107 MB | about 8 % of one core |
| the same binary with the panel built at start-up | 82 MB | about 1.6 % of one core |

**The floor is Slint's, not ours.** The same measurements on a *trivial* Slint
app with the same features give **27–35 MB** for an event loop with no
component at all, **44 MB** with a tray icon and no window, and **62–74 MB**
with a window that is never shown. So the idle GUI is now within about ten
megabytes of a Slint app with **no window at all** on this desktop.

**The model is not the cost.** The hidden-panel rule — a hidden pulldown is
not re-rendered; the tray keeps its two facts straight from the document — was
measured **before and after** on the same machine, in the same minute: the
resident set differs by **0.6 MB** (82.0 → 82.6) and the CPU by **0.06 s over
sixty seconds**. That is noise, and it is reported as noise: the cost of an
idle GUI is the daemon's document arriving and being decoded, which the TUI
pays too.

**Destroying the window does not give the memory back.** A trivial Slint app
whose window component was dropped was watched for ninety-six seconds: the
resident set stayed flat at **75,168 KB**. So the deferred window is a
**one-off** saving — worth about thirty megabytes of resident set — **twelve**
of physical footprint — to a GUI that has never been opened, and nothing to one
that has — and that is stated rather than implied.

**There is no leak.** `leaks` on the running GUI reports **287 leaks for
14 KB**, every one of them in Apple's own XPC and Objective-C machinery. And
macOS reclaims most of the rest over hours: a five-hour-old instance of the
same binary measured **14–30 MB**.

**The tray's click was exercised natively.** The GUI was started **without**
`--panel`, so the pulldown did not exist: the window list showed **no**
`gregg-gui` window and the resident set was **46,320 KB**. The tray icon was
clicked in the menu bar, and the pulldown appeared — the window list showed it
at `844,36 606x478` and the machine-readable line `pulldown=placed x=844.28125
y=36 width=606 height=478 area=8,36,1496x938` was printed. A second click hid
it and a third showed it again, so the panel is built **once** and toggled
after that.

**What was not done:** the pulldown's footer control was **not** clicked
natively. The panel's window is on another macOS Space in this environment —
which is environmental and not the change's doing: the **previous** binary was
started and clicked the same way and its window was on another Space too — so
a global click cannot reach the panel's own controls. The detail view was
therefore rendered from a build whose panel opened **directly** in the detail
view (a temporary start-up default, since reverted); the rendering path is the
pulldown's own and is the same one the footer control reaches. The Windows and
Linux legs are authority in the GUI CI job, not here, and the macOS Intel
build was not produced locally.

**The CI run that covers this work** is **`39909338640`**, against `62e74f9`, and
it is **green on all ten jobs**: the core `Linux`, `macOS (macos-15)`,
`macOS (macos-15-intel)`, `Windows`, `Scripts`, `MSRV` and `FreeBSD` jobs, and
the three `GUI` legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The `GUI`
legs are the only authority for this workspace's Windows and Linux behaviour,
because the workspace is not a member of the root workspace and no root job
compiles it.

**The root workspace's `cargo test -p gregg --lib` does not pass on this macOS
host**, and did not before this work either: the run fails **13 to 17** tests
depending on the run, in the `eggpool` worker races, the `clientd::ipc`
temp-directory path and the timing-sensitive `scheduler::fleet_scaling_*` tests,
and the **set varies from run to run**. The branch's state before this work
(`99de6b8`) fails **13** on the same host in the same session. Nothing under
`crates/gregg/src/` was changed by this work except the one shared wording
helper, whose own tests (`cargo test -p gregg --lib ui::text`, **26 passed**)
are green; the GUI workspace's own suite is **150 passed / 0 failed** and its
clippy is clean under `-D warnings`.

### The tray's click did nothing over another application's full screen (2026-10-23)

A defect that was **not new**: on macOS, clicking the tray icon while another
application was in full screen did nothing at all. The operator's expectation —
"a tray icon overlays on top natively" — is right, and it is what every native
menu-bar extra does.

**What macOS actually does, and why the click did nothing.** A full-screen
application owns a **space** of its own, and the operator is looking at that
space. A window that is not in that space is not on the screen at all. macOS
keeps a **Dock** application's windows out of another application's space, and
that is not a matter of level or collection behaviour: the right to reach
another application's space is fixed when the window is created. The GUI was a
Dock application (`winit` sets `NSApplicationActivationPolicyRegular` for an
unbundled binary), so its pulldown could never be in that space.

**Reproduced natively, before the fix.** TextEdit was put into full screen
(`0,33 1512x949`, on screen), the menu bar was revealed by moving the pointer
to the top of the screen, and the tray icon at `(940,16)` was clicked:

```text
gregg-gui: pulldown=placed x=844.28125 y=36 width=606 height=478 area=8,36,1496x938
60184  layer=3    onscreen=false  gregg-gui  Gregg  844,65 606x478
frontmost: MiniMax Code
```

The click **was** delivered — the pulldown was built and placed — but its
window was `onscreen=false`: it was in the GUI's own space, on a Space the
operator was not looking at. And the display had **left** the full-screen
space altogether: the frontmost application was no longer TextEdit. So the
operator clicked the tray icon and saw nothing, and the full-screen
application was dropped out of full screen on the way.

**What was changed.** Three native things, all in `src/platform.rs`:

1. the application installs the **`Accessory`** activation policy — a
   menu-bar-only application — before any window exists, through winit's
   event-loop builder (`install_backend`), which is where the desktop makes
   that decision itself;
2. the pulldown's **native window** is told to join every space, including a
   full-screen one (`CanJoinAllSpaces`, `FullScreenAuxiliary`, `Stationary`,
   `IgnoresCycle`), to stay visible when the application is deactivated, and to
   sit at **`NSPopUpMenuWindowLevel` (101)** — the level a native status item's
   own menu is drawn at, because `always-on-top` only reaches the *floating*
   level (3) and the floating level is below full-screen content
   (`prepare_overlay`);
3. the native window is found by its **title**, and that title is **owned by
   the `.slint` file** — bound to a property the Rust side reads — so the two
   cannot drift apart. A title that lived in two places would be a title that
   could drift, and the overlay would silently stop working.

**Verified natively, after the fix** — same machine, same full-screen
TextEdit, same click:

```text
60145  layer=0    onscreen=true   TextEdit  fs-note.txt  0,33 1512x949
60294  layer=101  onscreen=true   gregg-gui  Gregg       844,65 606x478
frontmost: gregg-gui
```

TextEdit's window is still `1512x949` **on screen** — it is still in full
screen — and the pulldown is on screen above it, at the pop-up-menu level. The
screenshot `apps/gregg-gui/target/gui-evidence/pulldown-over-fullscreen.png`
shows the pulldown floating over the full-screen document, with the document
intact behind it and no menu bar in the way.

The rest of the behaviour was checked on the same build:

- the **ordinary desktop path** is unchanged — the pulldown opens on the
  desktop, on screen, at `844,65 606x478`;
- **`Esc` still dismisses it** (the window went `onscreen=false`) and the
  tray's click brings it back;
- **`--panel`** still opens the pulldown at start-up, on screen;
- the **idle resident set is unchanged**: **35.1–36.2 MB** over sixty seconds
  with the pulldown never opened, against the **35.8 MB** minimum measured
  before this change. The accessory policy costs nothing.

**Two visible consequences, both deliberate.** The GUI has **no Dock icon**
now — an accessory application has none, and a tray-only GUI has no business
having one. `lsappinfo` says so directly: `gregg-gui` is
`"ApplicationType"="UIElement"`, where TextEdit is `"Foreground"`. And the
**menu bar stays the other application's**: activating a menu-bar-only
application does not take the menu bar away from the application the
operator is using, so the full-screen application keeps its menus — exactly
as a native menu-bar extra behaves.

**What is macOS's own business and was not changed:** in full screen the
menu bar is hidden, and the operator brings it back by moving the pointer to
the top of the screen (unless "Automatically hide and show the menu bar in
full screen" is set to Never). That is true of every menu-bar extra, and
nothing in this GUI can change it. The tray icon itself is in the menu bar,
so it is reachable only while the menu bar is showing.

**What was not done:** this was exercised **natively on macOS only**, on the
machine that has the real daemon and a seven-host fleet. The Windows and
Linux legs are authority in the GUI CI job, and they are unaffected — the
whole change is inside `#[cfg(target_os = "macos")]`. No macOS leg of the GUI
CI job exercises the tray at all, so this behaviour is **not** covered by CI;
it is covered by the native exercise above.

**The CI run that covers this work** is **`39927427831`**, against `22e1bce`,
and it is **green on all ten jobs**: the core `Linux`, `macOS (macos-15)`,
`macOS (macos-15-intel)`, `Windows`, `Scripts`, `MSRV` and `FreeBSD` jobs, and
the three `GUI` legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The
implementation itself, `d0cd31c`, was green on all ten jobs on run
**`39922180617`**. The `GUI` legs are the only authority for this workspace's
Windows and Linux behaviour, because the workspace is not a member of the root
workspace and no root job compiles it.

The GUI workspace's own suite is **153 passed / 0 failed** and its clippy is
clean under `-D warnings`. The root workspace's `cargo test -p gregg --lib`
does not pass on this macOS host, and did not before this work either: the run
fails **13 to 17** tests depending on the run, in the `eggpool` worker races,
the `clientd::ipc` temp-directory path and the timing-sensitive
`scheduler::fleet_scaling_*` tests, and the **set varies from run to run**.
Nothing under `crates/gregg/src/` was changed by this work at all.

### The pulldown opened in the middle of the desk (2026-10-23)

The full-screen defect above was fixed, and the operator reported the next one
in the same breath: the pulldown now works over a full-screen window, but
**it appears far to the left of where it should be**. The question asked was
whether the GUI hardcodes a pixel or a percentage for its placement, or
accounts for the display's size. The answer is: **the anchor accounted for the
display and the remembered placement did not**, and the remembered placement
won.

**The machine.** The active display is **3840×1080**, at the origin, the main
display, not built in:

```text
display 2 origin 0 0 size 3840 x 1080 pixels 3840 x 1080 builtin false main true
```

The **1512×982** built-in laptop display is **not online** — the machine is
closed. So the pulldown was placed on the laptop display and the operator moved
to the external one.

**The measurement.** The state file held an **absolute position**:

```text
position=844,65
size=606,478
```

`844` is where the pulldown's trailing corner sits on the **1512-point**
laptop display (`1512 − 606 = 906`, and 844 is 62 points in from it). The
3840-point display's trailing corner is at `3226` (`8 + 3824 − 606`). The
pulldown opened at **844** — **2382 points from the tray**: on screen, the
right size, and nowhere near the icon whose click opened it.

**The anchor was never wrong.** `placement::work_area()` divides the monitor's
**physical** bounds by the window's scale factor, so the work area it computes
is in logical pixels and is aware of the display's size and scale. What was
wrong was `open_position`: a **remembered position won over the anchor**, and
the remembered position had been measured on a display that was no longer
there.

**Reproduced natively, before the fix** — the same binary, the same file, the
3840-point display:

```text
gregg-gui: pulldown=placed x=844 y=65 width=606 height=478 area=8,36,3824x1036
60184  layer=101  onscreen=true  gregg-gui  Gregg  844,65 606x478
```

The work area was correctly the **3840×1080** display — `area=8,36,3824x1036`
— and the anchor for it is `x = 8 + 3824 − 606 = 3226`. The panel was placed
at **844** anyway, because that was what the file said.

**What was changed.** The remembered placement is now an **offset** against the
display's anchor corner — `placement::Offset`, measured to the panel's
**edges** — and the file holds `offset=…` instead of `position=…`:

- `placement::place(offset, size, anchor)` and `placement::offset_of(position,
  size, anchor)` are inverses, for either anchor, and a test asserts the round
  trip is exact;
- `open_position` places the remembered **offset** and clamps the result;
- `state::PanelGeometry` holds an `offset`, and the file's older `position=`
  key is **not read** — reading it would reproduce the very defect the offset
  exists to fix, because a place on the display it was measured on cannot be
  carried to a display it was never measured against;
- the drag's base is the position the GUI **asked for**
  (`UiState::panel_position`), never the position the window reports back.

**Verified natively, after the fix** — the same file, unchanged, on the same
display:

```text
gregg-gui: pulldown=placed x=3226 y=36 width=606 height=478 area=8,36,3824x1036
61034  layer=101  onscreen=true  gregg-gui  Gregg  3226,36 606x478
```

The pulldown now opens against the **trailing corner of the display it is on**.
The screenshot of the panel at that position shows the fleet — seven hosts
online, each with its five used shares — hanging off the right-hand edge,
which is where the tray is.

**The drag, and what it remembers.** The pulldown's header strip was dragged
200 points to the left with a synthesized mouse drag:

```text
before:  3226,36 606x478
after:   3026,36 606x478
file:    offset=200,0
```

The file holds the **gap at the trailing edge** (`200`) and the **gap at the
anchor edge** (`0`), not the absolute place. A second drag, and then a
restart:

```text
drag:    3026,36 -> 2826,36
file:    offset=400,0
restart: gregg-gui: pulldown=placed x=3026 y=36 width=606 height=478 area=8,36,3824x1036
```

The restart restored the same **relative** place — 400 points in from the
trailing edge of *this* display — and not the corner and not the stale
`844`.

**In a full-screen space.** The same drag was exercised with TextEdit in full
screen (`0,0 3840x1080`), with the pulldown drawn over it at the pop-up-menu
level:

```text
61195  layer=0    onscreen=true  TextEdit  Untitled 2  0,0 3840x1080
61485  layer=101  onscreen=true  gregg-gui  Gregg      2626,36 606x478
```

The drag moved the pulldown 200 points to the left and the file recorded
`offset=799.96094,-0.67578125` — the trailing gap and **an anchor gap of
essentially zero**. The fractions are the window server's own rounding of the
frame; the *vertical* offset stays at zero even where macOS draws the window
lower than it was asked to be, because the offset is measured against the
requested position and never against the window's own report. On the laptop's
full-screen space that shift is 29 points, and the pre-fix file carried it:
`position=844,65` — the `65` is the full-screen value, the `36` is the
desktop one.

**What was not done, and what is not covered by CI.** The display **change**
itself was not reproduced natively: the laptop display is offline and the
machine has one display attached, so "the same placement on a different
display" is proved by the **unit test** — `the_same_offset_means_the_same_thing_on_a_wider_display`
— and by the measurement above, not by a native run on two displays at once.
This was exercised **natively on macOS only**; the Windows and Linux legs are
authority in the GUI CI job, and the change is in the platform-neutral
placement code, so both legs run it.

The GUI workspace's own suite is **160 passed / 0 failed** (153 before this
work, plus seven new tests) and its clippy is clean under `-D warnings`.

One test in that suite failed on **one of eleven** local runs:
`evidence::the_eggpool_view_converges_one_daemon_owned_worker`, whose
assertion is that the **daemon** fetched the `EggPool` window the GUI asked
for. It drives the daemon's own EggPool worker, and the core's `eggpool`
tests fail **13 tests on this host right now** — three runs in a row — so the
condition is the daemon's and not this line's. The commit it is recorded
against was green on all ten CI jobs.

**The CI runs.** Run **`39968561479`** against `fd11201` is **green on all ten
jobs** — the core `Linux`, `macOS (macos-15)`, `macOS (macos-15-intel)`,
`Windows`, `Scripts (shellcheck + pytest)`, `MSRV (Rust 1.89)` and
`FreeBSD (gregg-host native)` jobs, and the three `GUI` legs (`macos-15`,
`windows-2022`, `ubuntu-latest`). The implementation itself, `7c19581`, was
green on all ten jobs on run **`39963924905`**. The `GUI` legs are the only
authority for this workspace's Windows and Linux behaviour, because the
workspace is not a member of the root workspace and no root job compiles it.

One run in between **failed**, and it is recorded here rather than forgotten:
run **`39966757278`** against `4e7c073` failed the `GUI (windows-2022)` leg,
because the two tests that commit added assumed the **top** anchor — the
macOS and Linux one — while Windows hangs the pulldown from the **bottom**,
where the taskbar is:

```text
a_drag_remembers_a_relative_placement
  left: Some(Offset { x: 200.0, y: 560.0 })
 right: Some(Offset { x: 200.0, y: 0.0 })
```

The trailing gap — the `x` — is right on both; the vertical gap is measured
from the edge the pulldown hangs from, and that edge is the bottom on
Windows. `fd11201` states what is true on every platform instead, and the
anchor was checked the only way it can be without a Windows host: the
`Anchor::for_platform()` branch was flipped to the Windows one locally and
the suite was run, and both tests pass under it.

### The panel had a band of dead space above the fleet (2026-10-23)

The placement defect above is fixed. The next report was about the panel's
**shape**: there was a **large gap between the fleet's title line and the first
host**, and the title line itself said far too much.

**The gap was dead space, and the cause was the layout.** A Slint layout adds
its spacing between **every** cell, and a cell that is empty still takes part
in the layout. The body's first three cells were the attachment's state, the
last action's message and the tray's warning — all empty on a healthy fleet —
and each of them still took a six-point gap. Eighteen points of nothing, on
every panel, every time.

**Measured, before.** The panel was captured off the screen by its window id,
so the numbers are the panel's own pixels:

```text
title line's ink          rows 45..56
first host's name         rows 101..113
gap                       45 points
panel height              478
```

**Measured, after.** The same fleet, the same display, the same capture:

```text
title line's ink          rows 41..50
first host's name         rows 69..81
gap                       19 points
panel height              446
```

```text
gregg-gui: pulldown=placed x=3226 y=36 width=606 height=446 area=8,36,3824x1036
61870  layer=101  onscreen=true  gregg-gui  Gregg  3226,62 606x446
```

The panel's height is **446** and the height the Rust computes for that fleet
is **446** as well — the `.slint` file's own layout and the computed height
agree exactly, with no slack at the bottom and no band at the top.

**What changed.**

- The three notices are **one cell** now, and it takes **no room at all** when
  it is empty. It carries its own gaps: two notices are six points apart, and
  the gap that follows them is there only when there is something to separate.
- The body's own layout spacing is **zero**, and each group carries the gap
  that follows it. The attribution is the body's **last** group in every view,
  so the gap above it is the only gap it needs.
- The title strip is **28 points** high rather than 36, and the body's top
  padding is **six** rather than twelve. The space between the title and the
  first host is a *gap*, not a margin, and a margin the size of the side
  padding reads as an empty band.
- The footer's links sit **six points off the card's bottom edge**, so a row of
  links does not read as cropped: the visible gap below them went from **six
  points to twelve** (the ink of the links ends at row 438 and the card's
  hairline is at row 450; before, the ink ended at 476 and the hairline was at
  482).

**The title line says only what matters.** It used to read:

```text
7 online, 0 pending, 0 offline — Deadpool, MacbookPro, Raspi 10, Plex, Le Potato20 (+2 mo…
```

which is elided away on any fleet of any size — the host names were never
readable. It now reads:

```text
7 online
```

and names the hosts that are **not** answering **only when there are any**:
`7 online, 1 offline`, `7 online, 1 pending`. A fleet with no hosts at all
says `no systems configured`, which is a different statement from `0 online`
among some.

**The corner square is gone.** There was a hairline square in the bottom-right
corner: the **resize grip**. It looked like a button, it did nothing anybody
could see, and the size it controlled was a size the operator could set once
and never change again — so the pulldown's size is now its content's size,
always, and the remembered size is gone with it. The file beside the
configuration now holds a placement and nothing else:

```text
offset=0,0
```

An older file's `size=` key is ignored, like its `position=` key: the size
follows the content now.

**Verified natively, on the other views too.** The detail view, the About block
and the add-host form were each opened on the running panel and captured. The
detail view is 424 points tall with the attribution hidden and 486 with it
shown — the difference is the attribution's 56 points plus the 6-point gap that
now follows it — and the form is 380 with it shown. The attribution sits
*below* the form now, which is where it belongs: it is the last thing in the
panel in every view.

**What was not done.** This was exercised **natively on macOS only**, on the
3840×1080 display; the Windows and Linux legs are the authority for those
platforms, and they run in the GUI CI job. The change is in the platform-neutral
`.slint` file and the placement module, so both legs compile and test it, but
nobody has *looked* at the panel on either of them.

The GUI workspace's own suite is **166 passed / 0 failed** (160 before this
work, plus six new tests), its clippy is clean under `-D warnings`, and its
formatting is clean.

**The CI runs that cover this work, and the two that failed.** Two runs failed
and both are recorded here rather than forgotten.

Run **`39982133374`** against `054fffc` was **green on nine jobs** — every core
job and both of the other two `GUI` legs — and **failed the `GUI (ubuntu-latest)`
leg** on one test:

```text
bridge::tests::an_explicit_client_must_identify_itself
a genuine Gregg client must be accepted: the Gregg client at
/tmp/gregg-gui-bridge-explicit-3538-ThreadId(8)/gregg did not identify itself:
could not run /tmp/gregg-gui-bridge-explicit-3538-ThreadId(8)/gregg version:
Text file busy (os error 26)
```

`ETXTBSY`. It is **not** this work's defect and not a defect in the product:
the test writes a `/bin/sh` stub with `fs::write` and the product's identity
probe `execve`s it a moment later, and on Linux that close-to-exec boundary
intermittently fails under load. This repository has root-caused the very same
transient twice already — in `plans/161` and `plans/167`, where the repair was
left to a corrective plan because it belonged to another crate — and the
minimal repair it names is the one applied here: **retry the probe**.

The repair is **test-only** and it is in the GUI's own crate, which is where
the flake lives: `bridge.rs`'s test helper `resolve_daemon_retrying` retries a
probe whose diagnostic says the executable **could not be run** — three times,
fifty milliseconds apart — and a retry that still fails still fails the test.
Nothing in the product changed: the GUI reported the failed spawn as a
diagnostic, which is the correct behaviour, and it still does. The sibling test
now also asserts that the tool was **run and refused** rather than merely
failed to start, so a transient cannot satisfy it by accident.

Run **`39985312878`** against `7823cfe` was **green on seven jobs** and failed
**both** of the two remaining `GUI` legs, and **both** failures were this
work's:

```text
GUI (windows-2022)
error: constant `PROBE_RETRY_PAUSE` is never used
error: constant `PROBE_RETRIES` is never used
error: function `resolve_daemon_retrying` is never used
```

The retry helper is only used by the two `#[cfg(unix)]` tests, so on Windows it
is dead code — and the CI runs with `-D warnings`, so dead code is a failure.
The helper and its two constants are `#[cfg(unix)]` now, like the tests that
use them.

```text
GUI (ubuntu-latest)
sni-host: FAIL the tray never carried the live fleet state: the tooltip was
"Gregg — 1 online" (expected it to contain "1 online" and "gui-fixture") and
the menu labels were ["Quit Gregg"] (expected one to mention "Quit Gregg")
```

The **native exercise** was asserting the *old* summary: it waited for the
tooltip to name the fixture's host, which is exactly what this work removed.
The tooltip carries `1 online` and nothing else now, so the exercise's premise
was wrong, not the GUI.

The exercise now asserts what the summary **is** — the counts — and what it
**is not**: the tooltip must not name a host. The counts are still the proof
that the tray carries the *fleet* and not a placeholder, because they only
appear once the first fleet document has arrived. The exercise's
`--expect-host-name` became `--host-name`, and the pass it prints is now
`tray-tooltip-carries-the-counts-not-the-names`.

**The CI run that closes this work is green.** Run **`39989322526`** against
`e303d05` is **green on all ten jobs** — the core `Linux`, `macOS (macos-15)`,
`macOS (macos-15-intel)`, `Windows`, `Scripts (shellcheck + pytest)`,
`MSRV (Rust 1.89)` and `FreeBSD (gregg-host native)` jobs, and the three `GUI`
legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The `GUI` legs are the
only authority for this workspace's Windows and Linux behaviour, because the
workspace is not a member of the root workspace and no root job compiles it —
and the `ubuntu-latest` leg is where the **native** Linux tray exercise runs,
against a real `StatusNotifierItem` host, with the *new* summary.

The GUI workspace's own suite is **166 passed / 0 failed** (160 before this
work, plus six new tests), its clippy is clean under `-D warnings`, its
formatting is clean, and its release binary is **11,962,992 bytes**.

### The pulldown closes when the operator clicks away (2026-10-23)

The next report was about the pulldown's **behaviour**: a menu closes when the
operator clicks somewhere else, and the pulldown did not. It stayed over
whatever was behind it, which is not what a menu does.

**The stated limit was wrong.** The documentation said, in four places, that
Slint 1.18 "exposes no focus-lost event". That was never checked against the
source, and it is not true. Slint 1.18's `FocusScope` carries:

```text
callback focus-changed-event(reason: FocusReason);
callback focus-gained(reason: FocusReason);
callback focus-lost(reason: FocusReason);
```

and `FocusReason` has five variants — `programmatic`, `tab-navigation`,
`pointer-click`, `popup-activation` and **`window-activation`**. The last one
is the desktop saying *this window is no longer the active one*, which is what
a click away does. The winit backend delivers it: `WinitWindowEvent::Focused`
becomes `activation_changed(have_focus)`, which dispatches
`WindowEvent::WindowActiveChanged`, which is `WindowInner::set_active`, which
sends `FocusEvent::FocusOut(FocusReason::WindowActivation)` to the focused
item.

So the signal exists, and it is cross-platform. It was built first, and it was
**measured** before it was trusted.

**The focus is not a reliable click-away signal on macOS.** With the focus rule
in place, the pulldown was opened five times and it **dismissed itself four
times** — every time because something else had taken the foreground. The
frontmost application during every sample was `Rio.app`
(`com.raphaelamorim.rio`), a mouse-automation tool that was installed and
running on this machine and that clicks the screen and re-asserts itself on its
own. A pulldown that closes itself while the operator is reading it is worse
than one that stays up, so the focus rule was **not** kept on macOS.

**What macOS uses instead: the mouse.** A click that lands outside the pulldown
belongs to the application that was clicked, so it **cannot** be seen from
inside the pulldown's own window. AppKit's global event monitor reports exactly
those events — the ones dispatched to *other* applications:

```rust
NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
    NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
    &handler,
)
```

Both that call and `NSEvent::mouseLocation()` are **safe** objc2-app-kit API,
so the GUI crate still contains **no `unsafe` at all**, and the watch needs
**no event tap and no accessibility permission**.

**The menu bar is excluded from "away".** The tray's own click **toggles** the
pulldown, so a watch that also dismissed on it would hide the pulldown and
open it again in the same gesture. The exclusion is a **band** and not a line,
because the pointer's position is read when the click is *delivered* rather
than out of the click itself, and a pointer that moves in between would
otherwise put the tray's own click outside the band. The band is 40 points.

**The geometry is one pure function**, so it is unit-tested on every platform,
including the ones that do not use the mouse watch:

```text
a click inside the pulldown            -> not a click away
a click in the menu bar (or its band)  -> not a click away
a click anywhere else                  -> a click away
```

**What was built.**

- The pulldown aggregates **everything that can hold the keyboard focus**: the
  scope that handles the keys, and the four fields of the two forms. A text
  field takes the focus away from the scope when it is clicked, so a click away
  with a form open has to be caught too.
- The form's fields are **named**, which is why the form's pane is always in
  the tree and only its visibility changes: a field inside a conditional block
  cannot be named from the top of the component, and the aggregate lives at the
  top.
- When the **view changes**, the pulldown **takes the focus back**: a form that
  goes away takes the focus with it, and the pulldown must not mistake that for
  a click away.
- Every dismissal — `Esc`, the ×, the tray's click and the click away — now
  goes through **one** function, so they leave the same thing behind: the
  fleet view, and no half-typed form.
- The **focus** rule is kept where the mouse cannot be watched: **Windows and
  Linux** use it, and macOS uses the mouse.

**Measured, natively**, on macOS 15 arm64, the **release** build
(11,979,536 bytes), against the live window list:

| what the operator did | the pulldown |
| --- | --- |
| clicked the **tray** (the menu bar) | `onscreen=true` — opened, and **not** dismissed by the watch |
| clicked the **tray** again | `onscreen=false` — closed by the toggle, and **not** dismissed by the watch |
| clicked the **tray** a third time | `onscreen=true` — opened again: the toggle still works with the watch installed |
| clicked **inside** (a host row) | `onscreen=true` — still there |
| clicked **away** (the work area) | `onscreen=false` — dismissed |

```text
gregg-gui: a click landed away from the pulldown at (800, 380)
gregg-gui: the pulldown was dismissed: the operator clicked away
```

The three tray clicks in a row produced **no** "landed away" line at all: the
menu bar's exclusion is what keeps the tray's own click from dismissing the
pulldown, which would otherwise hide it and open it again in the same gesture.
Those are the rows the focus rule could not pass — the tray's click is in the
menu bar, and the watch has to leave it alone.

The "away" coordinates in the log are AppKit's — the **bottom-left** origin —
so `(800, 380)` is the same click the driver made at `(800, 700)` from the
top-left.

**The limits that are left are stated rather than papered over.**

- On **Windows and Linux** the signal is *the focus was lost*, not *a click
  landed elsewhere*. A desktop that takes the focus away for another reason —
  another application activating on its own, a full-screen space being left —
  dismisses the pulldown too. A native status-item menu does not have that
  problem, because it is **modal**: it grabs the mouse and the keyboard, so
  nothing else can take the focus while it is open. A Slint window cannot be
  modal, so those desktops watch the focus instead of the mouse.
- On **macOS** the watch sees the clicks that are dispatched to *other*
  applications, so a click that another application swallows before dispatch is
  not seen; and the pointer's position is read at delivery time rather than out
  of the click itself, which is why the menu bar's exclusion is a band.

**The CI run that covers this work is green.** Run **`40017131833`** against
`481cda0` is **green on all ten jobs** — the core `Linux`, `macOS (macos-15)`,
`macOS (macos-15-intel)`, `Windows`, `Scripts (shellcheck + pytest)`,
`MSRV (Rust 1.89)` and `FreeBSD (gregg-host native)` jobs, and the three `GUI`
legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The `GUI` legs are the
only authority for this workspace's Windows and Linux behaviour, because the
workspace is not a member of the root workspace and no root job compiles it —
and the `ubuntu-latest` leg is where the **native** Linux tray exercise runs,
against a real `StatusNotifierItem` host.

The GUI workspace's own suite is **174 passed / 0 failed** (166 before this
work, plus eight new tests), its clippy is clean under `-D warnings`, its
formatting is clean, and its release binary is **11,979,536 bytes**.

### The scroll box stopped short of the bottom, and the detail was too short to read (2026-10-23)

The next report was about the pulldown's **layout**: the fleet's scroll box was
much shorter than it had been and stopped short of the bottom, the pulldown was
too wide for what it carries, the detail button did not do its job, and there
was no way back to the fleet view once the detail was open.

**What was measured, before anything was changed.** The panel was **606 × 446**
and the fleet's content was clipped at **y = 249**, while the body's own space
ran on to **y = 396** — **147 points of dead space** between the last host and
the footer's strip. The detail view's content was clipped at **y = 98**: the
host's block, three rows, and nothing else.

**The cause was two invisible panes.** A temporary diagnostic build printed the
layout's own numbers from inside the panel:

```text
DIAG list=348 svh=155.33333 vis=155.33333 cont=603 lay=201.33333
```

The box was asked for **348** points and got **201.33** — exactly
`348 − 2 × 73.33`. The body's cells were the notices, the fleet, the two forms
and the attribution, and the two forms claimed **220 points each** whether they
were showing or not: **440 points** out of a body that had **348**, so the
deficit was shared out and the fleet's box was left with 201.

**Why a hidden pane still had a height.** A Slint layout lays out the
**invisible** items as if they were visible. They are not painted, but they
still claim their height, and the layout squeezes the items around them to
make room. The two forms are in the tree only so that their fields can be
named — the focus aggregate names them, so that a click away with a form open
is caught — and they had to say so in their **size** as well.

**What changed.**

- A hidden pane declares **zero height**: its preferred height, its maximum
  height *and* its minimum height, because the fluent scroll container has a
  fifty-point floor of its own and a floor is a floor whether the pane is
  visible or not.
- The fleet view and the detail view scroll in **one** box, so the panel's
  height does not change when the operator opens the detail, and the footer's
  control that brings the fleet back does not move under the pointer.
- The pulldown is **194 points narrower**: **412 px**.

**What was measured, after.** The same fleet, the same window height:

| | before | after |
| --- | --- | --- |
| the panel's width | 606 px | **412 px** |
| the fleet's scroll box | 201 points | **348 points** — the whole body |
| the detail's scroll box | 51 points | **348 points** |
| the fleet's content ran to | y = 249 (clipped) | **y = 387**, the body's edge |
| the panel's height, fleet → detail | 446 → 424 | **446 → 446** |

The last row is the "no way back": the panel used to shrink by twenty-two
points when the detail opened, which moved the footer's links up by the same
twenty-two points, so the control that brings the fleet back was no longer
where the operator had clicked. It is in the same place now, and it says
`Fleet`.

**The scroll was exercised too**: a wheel event over the list moved it down to
the last four hosts — `Plex`, `Le Potato20`, `Le Potato21` and `Raspi 8` — and
the box still reached the footer's strip.

**The click away still works with the narrower panel.** A click on the desktop
at (800, 900) was reported by the watch at (800, 180) in AppKit's
bottom-left coordinates, and the pulldown was dismissed:

```text
gregg-gui: a click landed away from the pulldown at (800, 180)
gregg-gui: the pulldown was dismissed: the operator clicked away
```

**The suite is 177 passed / 0 failed** (174 before this work, plus three new
tests: `a_hidden_form_takes_no_room`, `the_fleet_and_the_detail_share_one_box`
and `the_detail_does_not_resize_the_panel`).

**The CI run that covers this work is green.** Run **`40029395421`** against
`f7c3fb7` is **green on all ten jobs** — the core `Linux`, `macOS (macos-15)`,
`macOS (macos-15-intel)`, `Windows`, `Scripts (shellcheck + pytest)`,
`MSRV (Rust 1.89)` and `FreeBSD (gregg-host native)` jobs, and the three `GUI`
legs (`macos-15`, `windows-2022`, `ubuntu-latest`). The `GUI` legs are the only
authority for this workspace's Windows and Linux behaviour, because the
workspace is not a member of the root workspace and no root job compiles it.

The release binary is **11,979,552 bytes** — **+16 bytes** on the build before
this work, because a narrower panel is the same code drawing fewer pixels.

### What the GUI costs, and what it polls (2026-10-23)

The same session asked two questions that are worth answering with numbers
rather than with assurances: **how does the GUI's memory compare with the
terminal client's**, and **is the GUI polling the fleet a second time**.

**Memory.** The same fleet, the same **final** release build, sampled every two
seconds for twenty-four seconds on this machine (macOS 15, arm64), with
`footprint`'s physical footprint beside the resident set:

| | resident set (min / median / max) | physical footprint |
| --- | --- | --- |
| **the TUI** (`gregg`) | 3,168 / **3,168** / 3,168 KB | 2,352 KB (2.3 MB) |
| **the client daemon** — the poller, shared by both frontends | 5,184 / **5,184** / 5,232 KB | 4,128 KB (4.0 MB) |
| **the GUI**, the pulldown never opened | 46,912 / **46,928** / 47,056 KB | 14 MB |
| **the GUI**, the pulldown built and hidden | 24,464 / **24,464** / 24,496 KB | 22 MB |
| **the GUI**, the pulldown open | 38,512 / **41,856** / 41,936 KB | 26 MB |

The **physical footprint** is the honest number on macOS and it is monotonic:
**14 MB** before the pulldown has ever been opened, **22 MB** once the window
exists but is hidden, **26 MB** with it on screen, against **2.3 MB** for the
TUI and **4.0 MB** for the daemon. The GUI is therefore about **six times** the
TUI before it is opened and about **eleven times** with its window on screen.

The **resident set** is not monotonic, and the reason is worth stating: on
macOS `ps` counts shared, clean, reclaimable pages — the dyld shared cache, the
system font collections — which the system hands back at its own pace, and a
*fresh* process reports its high-water mark. That is why the never-opened GUI,
which had just started, reads higher than the one that had been open and
dismissed. The resident set says the GUI is roughly **fifteen times** the
TUI's before it is opened and **thirteen times** with its window open.

Either way the difference is Slint's software renderer and the window server,
not the model: the TUI is a terminal renderer with no window, and the table
above says where the difference comes from.

**Polling.** Measured on the live processes, with the GUI attached to a
seven-host fleet:

| | remote sockets | local sockets |
| --- | --- | --- |
| the client daemon (the poller) | **14 established TCP connections** to the fleet's hosts | the endpoint the frontends attach to |
| the TUI | **none** | 7 Unix sockets (the daemon's endpoint) |
| the GUI | **none** | 4 Unix sockets (the daemon's endpoint) |

`lsof -nP -a -p <pid> -i` on the GUI's process lists **no network socket at
all**. The GUI opens no TCP connection, never opens an HTTP client, and never
polls a host: it is a frontend, and the daemon is the poller. The GUI's own
dependency list is checked by a test as well — `bridge::tests::
the_gui_crate_depends_on_no_http_client` reads the manifest and refuses
`reqwest`, `axum`, `ureq`, `isahc`, `surf`, `attohttpc`, `ehttp`, `curl` and
`hyper`.

**One stale process was found while measuring this**, and it is worth naming:
`~/.local/bin/gregg`, version **1.0.14**, has been running since 2026-10-20 and
**does** hold TCP connections to the fleet's hosts — **seven** of them, on a
re-measurement. It is a build from before
the client daemon existed — the current release is 1.0.15, and it is a
frontend. It is a leftover, not the current client, and it is the one process
on this machine that polls the fleet outside the client daemon.

**What the dismissal cost.** The release binary is **11,979,536 bytes**, which
is **+16,544 bytes (+0.1%)** on top of the previous build: a focus callback, an
aggregate over five elements, one function and the macOS mouse watch. It adds
**no new crate** to the build — `block2`, the crate that carries an Objective-C
block, was **already in this workspace's graph** through `objc2`, and the
lockfile still holds the same **604 packages** — and it adds no event tap and
no accessibility permission. The GUI workspace's suite is **174 passed / 0
failed** (166 before this work, plus eight new tests), with clippy clean under
`-D warnings` and the formatting clean.

**The non-macOS branch was checked too.** The macOS build never compiles the
`not(target_os = "macos")` path, and a branch that no local job compiles is
exactly how a broken Windows build ships, so it was checked **cross-wise**:
`cargo clippy --target x86_64-pc-windows-gnu --all-targets --all-features --
-D warnings` is **clean**, which compiles the stub and the geometry tests on a
Windows target. A Linux cross-run was **not** possible on this machine: the
build needs a Linux C toolchain for `ring` and `yeslogic-fontconfig-sys`, and
there is none here. The Linux leg of the GUI CI job is the authority for that,
and it compiles the same `not(macos)` stub that the Windows cross-run did.

### The native exercises, on CI run `39811582623`

The GUI workspace is not a member of the root workspace, so no root job
compiles it; the three `GUI` legs are the only authority for its Windows and
Linux behaviour. Run **`39811582623`** is **green on all ten jobs** — the core
`Linux`, `macOS`, `macOS Intel`, `Windows`, `Scripts`, `MSRV` and `FreeBSD`
jobs, and the three `GUI` legs.

The **Linux** leg runs the native tray exercise against a **real
`StatusNotifierItem` host**, and what it printed is this plan's own claims,
observed rather than asserted:

```text
sni-host: obs ToolTip.title="Gregg — 1 online, 0 pending, 0 offline — gui-fixture"
sni-host: obs Menu.labels=["Quit Gregg"]
sni-host: PASS tray-tooltip-carries-the-fleet-status the tooltip is "Gregg — 1 online, 0 pending, 0 offline — gui-fixture"
sni-host: PASS tray-tooltip-carries-the-fleet the tooltip mentions "gui-fixture"
sni-host: PASS tray-menu-is-the-context-menu a menu label mentions "Quit Gregg"
sni-host: PASS pulldown-is-placed-inside-the-work-area Placement { x: 852.0, y: 36.0, width: 420.0, height: 256.0, area_left: 8.0, area_top: 36.0, area_width: 1264.0, area_height: 980.0 }
sni-host: PASS gui-survives-without-a-tray the GUI is still running three seconds after reporting `tray=unavailable`
linux-tray: PASS both Linux tray passes
```

The menu is **`["Quit Gregg"]`** — the launcher is gone and the menu is the
context menu; the **tooltip** carries the fleet status and the host name; and
the pulldown was **placed inside the work area it reported**, which is the
placement gate proven on a real desktop.

The **Windows** leg proves the same placement claim on the platform where the
anchor is the **bottom**-trailing corner, and its own core launch path:

```text
windows-tray: the GUI printed: gregg-gui: pulldown=placed x=596 y=460 width=420 height=256 area=8,8,1008x708
windows-tray: the pulldown is inside the work area it reported
windows-tray: the GUI created 4 native top-level window(s)
windows-tray: the client daemon is running
windows-tray: the fleet fixture served 8 request(s): the daemon is polling
windows-tray: PASS every Windows tray check
```

`y=460` is `8 + 708 − 256` — the bottom of the work area, where the Windows
taskbar is — so the anchor is right on that platform too.

Neither leg claims that the **notification-area icon was rendered**: the
Windows runner is session 0 with no `explorer.exe`, and the Linux leg drives
the SNI protocol rather than a desktop shell. That is the interactive
acceptance, and it is still outstanding.

**One flake was found and fixed.** The `GUI (ubuntu-latest)` leg failed once
on run `39924998544` with *"the GUI never reported where it put the
pulldown"*, and the GUI's whole output was `gregg-gui: tray=available`. The
Linux exercise read the GUI's transcript **once** and looked for the
placement line, and the line had not arrived yet: the pulldown's native
window is created on the event loop's own terms, so the line comes when it
comes. The re-run of the same commit passed, and the two runs before it
passed — and the code the leg exercises was byte-identical in all of them, so
this was the harness's timing and not the GUI's.

The Linux exercise now **waits** for the placement line, within the budget it
already has — which is what the **Windows** exercise already did, so the two
now behave the same way.




## The fixture

| piece | what it is |
| --- | --- |
| remote **modern** | an in-process HTTP server that serves `/v2/status`, `/v2/scheduler` and `/v2/scheduler/history`, counting each route separately |
| remote **old** | the same server with the scheduler routes answering `404` — a healthy `greggd` from before the scheduler plane |
| the daemon | the **real** `gregg::clientd::daemon::run_daemon`, not a stub |
| frontend **TUI-like** | a second attachment through the same public client-daemon API the terminal client uses (`gregg::clientd::daemon::attach` + `FrontendSender` intents) |
| frontend **GUI** | the real `Bridge` from `apps/gregg-gui/src/bridge.rs` |
| config | `config_version = 1`, `refresh_seconds = 1`, two systems (one per remote) |

The measurement window is **4 seconds** — four refresh intervals, so a count
over it is a rate rather than a single sample.

## The request-count table

Observed, from one run of `evidence::two_frontends_do_not_multiply_the_fleet_request_budget`:

| remote | route | daemon alone | with TUI | with TUI + GUI | after both frontends closed |
| --- | --- | --- | --- | --- | --- |
| modern | `/v2/status` | 4 | 4 | 4 | 4 |
| modern | `/v2/scheduler` | 0 | 0 | 0 | 0 |
| modern | `/v2/scheduler/history` | 0 | 0 | 0 | 0 |
| old | `/v2/status` | 4 | 4 | 4 | 4 |
| old | `/v2/scheduler` | 0 | 0 | 0 | 0 |
| old | `/v2/scheduler/history` | 0 | 0 | 0 | 0 |

Whole-life totals for that daemon, after all four windows:

```
modern  Hits { status: 17, scheduler: 1, history: 1 }
old     Hits { status: 17, scheduler: 1, history: 0 }
```

What the table says, in order of how sharply it is asserted:

- **`/v2/status` is flat at 4 polls per window in every phase** — one poll per
  refresh interval with zero frontends, with one, with two, and with none
  again. A frontend that opened its own HTTP client would have shown 8 or
  more in the two-frontend column.
- **`/v2/scheduler/history` is 0 in every window and 1 in the whole life of
  the daemon.** The history body is fetched once per `(epoch, revision)`, at
  discovery; nothing in the fixture changes either, so no frontend — GUI
  included — can make it fetched again.
- **`/v2/scheduler` is 0 per window and 1 in the whole life** on both remotes:
  the summary is polled on the daemon's own thirty-second cadence, and the
  four-second windows are shorter than that. The old remote's single hit is
  the `404` that marks it as an older daemon, not an error.
- The old remote's `history` count is **0 for the whole life**: a remote with
  no scheduler routes is never asked for history at all.

The two remotes are told apart by *capability*, not by failure: the modern
system is reported `CronCapability::Supported` and the old one
`CronCapability::Unsupported`, with both systems `Online`.

### The cron intent

Observed, from `evidence::an_opened_cron_job_grows_the_local_document_but_not_the_remote_budget`:

```
local document: 1903 -> 2276 bytes   (+373 bytes)
remote counters after the whole test: Hits { status: 1, scheduler: 1, history: 1 }
```

Opening a cron job in the GUI grew the **local IPC document** by 373 bytes —
the job's records rode along — while the remote's history counter stayed at
exactly the one discovery fetch. The GUI's cron intent governs transmission,
never fetching.

### The `EggPool` pane

Observed, from `evidence::the_eggpool_view_converges_one_daemon_owned_worker`:

```
windows the remote was asked for: ["7d", "7d", "7d"]
summary requests: 3   status requests: 3
```

The GUI asked for the **week** window and the terminal-style frontend asked
for the **month** window. The remote was asked for `7d` three times and for
`30d` **never**. A worker per frontend would have asked for both windows;
one converged worker only ever asks for the shortest window any active
frontend asked for. The three `7d` requests are the three daemon-side
transitions (the GUI opening the pane, the second frontend joining, and that
frontend releasing), each of which drives the same single worker. When the
last pane closed the worker converged to idle and the request stream stopped.

## Observable process identities

From the run that produced the table above:

```
test process pid: 82112
daemon id:        fafc8828777ceaec
daemon endpoint:  /private/var/folders/2j/dlwhrpps66scv9bw8f7vdfg40000gq/T/gregg-client-fafc8828777ceaec.sock
```

Honest notes on these values:

- The daemon under test runs as a **task inside the test process**, so the
  socket's owning PID *is* the test process's PID. There is no separate
  daemon PID in this harness, and the record does not invent one.
- The daemon's own identity is the config-derived digest (`fafc8828777ceaec`)
  and the socket it bound. Both change from run to run because the fixture's
  config path contains the test process's PID and thread id; the *shape* is
  what matters, and it is stable.
- The socket is the **fallback** endpoint, not the primary one beside the
  config. The primary path would be 125 bytes, over the 104-byte
  `sun_path` limit on macOS, so the daemon bound
  `<system temp>/gregg-client-<id>.sock` (91 bytes) instead. The client
  daemon tries both candidates, so the frontend still reached it.
- The isolation test records two distinct daemon identities for two distinct
  config paths, and the **same** identity for the same file spelled
  `…/gregg.toml` and `…/./gregg.toml`.

## What each test proves

| test | the one-sentence claim |
| --- | --- |
| `two_frontends_do_not_multiply_the_fleet_request_budget` | attaching a terminal-style frontend and a GUI to the same config does not change the daemon's request rate on either remote, and closing them again does not stop the polling |
| `an_opened_cron_job_grows_the_local_document_but_not_the_remote_budget` | an opened cron job enlarges the local IPC document and leaves the remote's history fetch count untouched |
| `the_eggpool_view_converges_one_daemon_owned_worker` | the `EggPool` pane is driven by one daemon-owned worker on the converged window, not one worker per frontend, and it stops asking the remote when the last pane closes |
| `the_daemon_outlives_its_frontends_and_reattaches_with_the_newest_document` | the daemon survives its frontends closing, keeps its cadence, and hands a reattaching frontend the newest document without refetching the scheduler history |
| `distinct_config_paths_reach_distinct_daemons_and_one_spelling_reaches_one` | two config paths are two daemons with two fleets, and two spellings of one config path are one daemon |
| `the_gui_refuses_an_incompatible_daemon_without_stopping_it` | the GUI reports an incompatible daemon instead of rendering it, and does not stop or replace the peer it collided with |

## Not proved here, and why

1. **The daemon restart path, the foreign-peer refusal, the directional
   version rotation, and "never stop an unowned process" are not re-proved
   here.** They are already covered by the client daemon's own launcher tests
   in `crates/gregg/src/clientd/launch.rs` (25 tests), among them
   `a_foreign_peer_on_the_endpoint_blocks_a_launch_instead_of_being_overwritten`,
   `an_older_owned_daemon_rotates_but_a_newer_one_is_left_alone`,
   `a_rotation_stops_the_stale_daemon_and_starts_a_replacement`,
   `a_daemon_started_for_one_config_is_invisible_to_another` and
   `two_concurrent_launches_produce_exactly_one_daemon`. Duplicating them
   here would have proved nothing new about the GUI line.
2. **The launcher-side refusal to replace an incompatible daemon is not proved
   end to end.** The harness's stand-in completes the handshake and *then*
   reports a newer protocol version, which exercises the GUI's own
   `Incompatible` reporting and the "no second connection" property. The
   launcher's `EnsureError::IncompatibleOwned` refusal belongs to
   `launch.rs`.
3. **Windows was not covered here, and the reason was structural.** The GUI
   crate is not a root workspace member, so no CI job ran its tests; the
   stand-in-daemon test is `#[cfg(unix)]` because it speaks the Unix socket
   protocol directly. The named-pipe transport on Windows was untested here.
   **That is no longer true of the build**: the `gui` CI job builds, lints and
   tests this workspace on a native Windows runner, and the native Windows
   exercise starts the GUI and proves it creates a native top-level window.
   The named-pipe transport *within* the GUI's own tests is still
   `#[cfg(unix)]`, and the **interactive** Windows run — a visible
   notification-area icon in a real shell — is still outstanding and still not
   claimed.
4. **The scheduler summary's rate over a long horizon is not measured.** The
   summary cadence is 30 seconds and the window is 4 seconds, so a window
   contains zero or one summary request. The comparison therefore uses a
   tolerance of one, and the absolute claim is "at most two over the whole
   test" — not a measured per-minute rate.
5. **The `EggPool` worker's 60-second passive refresh is not observed.** The
   test is shorter than the refresh interval, so "one worker, not one per
   frontend" is proven by *which window* the remote is asked for and by the
   request stream stopping when the last pane closes — not by counting
   refreshes across a minute.
6. **The remotes are fixtures, not real `greggd` instances.** The status
   document is a fixed `LinuxSnapshotV2` payload and the scheduler documents
   are hand-built for the fixture. What is counted is the daemon's *request*
   behaviour; the fixture's *answers* are not the subject.
7. **The Slint UI itself is not exercised.** The harness drives `Bridge` — the
   attachment, the snapshot stream and the desired-state intents. The
   snapshot-to-view-model projection is covered by the GUI's own model tests,
   not here.
8. **The remote's `EggPool` health plane is a minimal valid snapshot.** The
   harness asserts that the health route is fetched alongside the summary
   (3 status requests against 3 summary requests), not that the health
   reduction is correct.

## Where a claim was weakened, and why

- **The metrics comparison is a difference of differences with a tolerance of
  two polls**, not an exact equality. The daemon's poll cadence is anchored,
  so a four-second window holds four polls, except when a poll lands within
  a millisecond of a window edge — it then falls on either side of the cut,
  and a loaded machine can skip a tick. Two polls of slack covers that while
  staying far below the eight-or-more polls a self-polling frontend produces.
- **The scheduler summary comparison uses a tolerance of one**, because the
  summary cadence (30 s) is longer than the window (4 s).
- **"The daemon keeps polling after the frontends close" is asserted as "at
  least two polls in a four-second window"**, not as an exact count, for the
  same edge reason.
- **The cron document growth is asserted as "at least 64 bytes larger"**,
  not as an exact size: the metrics payload around it is a fixed document
  whose only length jitter is a latency digit, while the history record is a
  few hundred bytes. The observed growth was 373 bytes.
- **The `EggPool` "one worker" claim is proven by the window the remote is
  asked for, not by counting concurrent requests.** The two frontends publish
  their intents at different moments, so a concurrency measurement could not
  have distinguished one worker from two.
- **The "the GUI did not stop the peer" claim counts the peer's connections
  after a 300 ms settle**, because a stop attempt would be asynchronous
  relative to the incompatibility report. The count was exactly 1.
- **One test in this workspace is sensitive to a heavily loaded host, and it
  is pre-existing.** `bridge::tests::an_explicit_client_must_identify_itself`
  asks the launcher to identify a candidate client executable, and the
  launcher's identity probe gives that child **ten seconds** to answer. On a
  heavily loaded host that budget was exceeded and the test failed with `gregg
  version did not answer within 10000 ms` — the same message the core's own
  three identity tests produced in the same window. The probe, its budget and
  the test are all untouched by this line; the test passes in about half a
  second when the machine is idle, and the whole GUI suite passes 63/63 when
  it is. The failure is recorded here rather than "fixed", because raising
  the budget or weakening the assertion would hide the environment
  sensitivity instead of removing it, and the CI runs of this branch are
  green on all three desktop targets.

## Reproducing

```bash
cargo test --manifest-path apps/gregg-gui/Cargo.toml            # the whole suite
cargo test --manifest-path apps/gregg-gui/Cargo.toml -- --nocapture evidence::
cargo clippy --manifest-path apps/gregg-gui/Cargo.toml --all-targets --all-features -- -D warnings
cargo fmt --manifest-path apps/gregg-gui/Cargo.toml -- --check
```

Three consecutive full-suite runs, all green:

```
run 1: test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
run 2: test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
run 3: test result: ok. 54 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

54 = the 48 pre-existing GUI tests + the 6 harness tests above. The budget
table was re-run three more times separately and came out identical every
time (`4 / 4 / 4 / 4` on both remotes, `0` on both scheduler routes in every
window).
