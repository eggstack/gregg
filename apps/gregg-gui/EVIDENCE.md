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

**5. The footprint was re-measured against a rebuilt baseline.** The scroll
container comes from Slint's `std-widgets`, so both binaries grew: the GUI to
**10,803,360 bytes** and the matched Slint-only baseline to **9,956,032
bytes**. The difference — what this frontend adds over a pure-Slint build of
the same UI — is **847,328 bytes**, against 847,344 bytes before the change:
16 bytes, on a ten-megabyte binary. A baseline that was not rebuilt would have
reported the whole growth as the GUI's cost.

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
