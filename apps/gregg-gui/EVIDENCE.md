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

The state file is written when the operator **moves or resizes** the pulldown —
the drag and the corner grip both raise `geometry-changed` — and not on the
first placement, which is derived from the content and the work area and so has
nothing to remember. `src/state.rs` reads it bounded (4 KiB) and validated and
writes it atomically, and falls back to the default on a corrupt or
out-of-range file; 13 unit tests cover that path.

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
