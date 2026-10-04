# Plan 166: Cron cache and TUI observability

Status: complete.

Depends on: completed Plan 163 and Plan 164. Plan 165 should be complete before
this plan is declared closed so the background cache has the intended durable
process lifecycle. Independent of Plan 091.

## Objective

Consume greggd scheduler observability in the Gregg client daemon and expose it
through a compact keyboard-first cron view in the TUI.

The primary UX is:

- plain c behaves as the cron detail control beside d drives and n network;
- the selected system immediately shows configured jobs, schedule/next run,
  current state, last result, and obvious load delays;
- an individual job exposes the last five terminal records by default,
  including bounded stdout/stderr tails;
- the display count is configurable;
- the background client daemon retains a longer bounded in-memory history so
  closing/reopening the TUI does not reset what Gregg has observed.

## Remote scheduler polling

The client daemon, not each TUI, polls the new scheduler routes.

Use two planes:

1. compact /v2/scheduler summary;
2. larger /v2/scheduler/history fetched only when needed.

The summary's history revision controls history refresh. Do not download the
whole history body every Systems metrics cadence when nothing changed.

Cadence rules:

- scheduler summary may piggyback on the Systems endpoint cadence or use a
  bounded lower-frequency cadence if measurement demonstrates that is more
  appropriate;
- no per-second cron poll;
- a load-delayed state should become visible promptly enough for an operator
  watching Gregg;
- history fetch occurs on first support discovery and when revision changes;
- remote failures in scheduler routes do not mark the host's metrics offline.

Old daemon behavior:

- /v2/status success + scheduler route 404 => system stays online and cron
  capability is unsupported;
- scheduler route network/5xx failure => retain last known cron data with a
  stale/error marker distinct from system reachability;
- new daemon with empty jobs => supported/empty.

Do not fall back to parsing logs or config files remotely.

## Client-daemon cron state

Extend the local frontend snapshot with scheduler capability/state separate
from the normalized metrics payload.

Per system retain:

- capability: unknown/unsupported/supported;
- latest scheduler summary;
- scheduler fetch status/freshness;
- bounded local job histories.

The client daemon deduplicates remote records using the Plan-162 restart
discriminator + sequence identity. A repeated remote history response must not
append duplicate records.

When the remote scheduler restart discriminator changes:

- do not confuse reused sequence values with old records;
- retain previously observed local history if within local bounds, but mark the
  new epoch correctly;
- do not fabricate continuity such as claiming an old pending job is still
  pending.

## Longer local in-memory history

The client daemon is allowed to retain more than the remote default five
because it observes successive remote rings while always running.

Keep it bounded and memory-only.

Add client-side configuration with clear separation between:

- number of records displayed for the selected job, default exactly 5;
- longer per-job cache depth;
- a global total-record or byte bound preventing endpoints x jobs x depth from
  exploding memory.

Suggested names:

~~~toml
[cron]
display_history = 5
cache_history = 25
~~~

Plan implementation may choose equivalent names consistent with existing config
style. Both need hard maxima. A global total-record/byte ceiling is required
regardless of per-job maxima.

No client history database/file is added in this line.

Eviction should be deterministic: oldest retained terminal records first while
preserving the newest requested display window whenever possible.

## TUI action

Add Action::ToggleCron.

Map:

~~~text
c -> ToggleCron
Ctrl-C -> Quit
~~~

Plain c is currently unused and must not affect the existing Ctrl-C quit
mapping.

d, n, and c remain independent. Opening cron details must not silently close
drive/network details unless terminal-height degradation requires a documented
presentation choice.

Add cron_expanded, or equivalent presentation-only state, to the TUI side, not
the daemon fleet state.

## Cron detail layout

Use the selected-system expansion model as the top-level behavior so c feels
consistent with d and n.

The cron block must remain bounded when a system has many jobs or a job has
multiline output. Do not increase a system card without limit.

Recommended hierarchy:

~~~text
CRON  3 jobs
  cargo-cleanme  weekly Sun 03:00   DELAYED 17m   load15 9.24 > 8.00
  backup         daily  02:00       idle          next 11h
  rotate         hourly             ok 4m ago      820ms

selected job: cargo-cleanme
  2026-10-04 03:17  ok        delay 17m  42.1s
    stdout: ...
  2026-09-27 03:02  exit 1    delay 2m   3.8s
    stderr: ...
  ...
~~~

Exact date formatting should follow Gregg's compact width conventions.

At-a-glance job rows need:

- name;
- schedule/interval representation;
- state;
- next run or pending duration;
- last terminal result.

Load-delayed rows must show:

- delay duration;
- selected load window;
- observed load if available;
- configured threshold;
- next retry or max-wait context when width permits.

Load unavailable must say unavailable, not render 0.

## Individual job history navigation

A cron-expanded system may have many jobs, so only one job's multiline history
should be expanded at once.

Use a small dedicated selection control that does not steal unshifted j/k from
system navigation. One acceptable mapping is Shift-J/Shift-K while cron detail
is open; another compact mapping may be chosen if it is already free and
documented.

Requirements:

- current selected system remains controlled by ordinary j/k;
- selected cron job defaults deterministically when c opens;
- moving systems keeps or repairs cron sub-selection by stable job name where
  possible;
- closing cron detail does not mutate daemon state;
- history view shows display_history records, default five;
- no horizontal scrolling requirement.

## Output rendering

Treat remote command output as hostile terminal text.

Before Ratatui cells are constructed:

- strip or visibly escape ESC/CSI/OSC and C0/C1 terminal controls;
- normalize tabs/newlines under bounded rendering rules;
- never emit raw Esc, BEL, carriage-return overwrite, or OSC hyperlink/title
  sequences to the terminal;
- preserve printable Unicode;
- indicate remote truncation;
- locally truncate/wrap to the allocated cron-history viewport without changing
  stored data.

Test with malicious strings containing:

- ANSI color;
- cursor movement;
- clear-screen;
- OSC title/hyperlink;
- BEL;
- carriage returns/backspaces;
- invalid/replacement Unicode from daemon conversion.

The sanitizer should be a small pure helper with cell-width tests, not a new
terminal-emulation dependency.

## Viewport and height integration

Reuse the current dynamic selected-system detail accounting rather than adding
a second fleet scroll model.

Requirements:

- normal and condensed modes remain usable;
- selected system stays visible;
- drive/network detail row counts remain correct when cron detail coexists;
- cron block gets a bounded vertical budget based on terminal height;
- too-small terminal renders a clear degraded message/summary rather than
  indexing outside the buffer;
- renderer remains I/O-free.

Consider a scrollable inner history viewport if necessary, but keep one outer
system selection/viewport.

## Local IPC publication

The client daemon sends cron state/history through the existing latest-state
frontend snapshot/revision mechanism from Plan 164.

Do not create a second local socket or per-TUI remote scheduler poller.

Because history may be larger than metrics state, qualify whether local IPC
sends:

- complete bounded cron state on revision; or
- summary plus an on-demand selected-system/job history request.

Prefer the smaller design that keeps implementation simple and avoids
retransmitting a large fleet cache on every CPU poll. A two-tier local message
model, summary latest-state plus bounded history request, is allowed and may be
more appropriate.

Record the decision in architecture/gregg-client.md.

## Refresh/error semantics

Cron observability errors are independent of core system reachability.

Examples:

- system metrics online, scheduler unsupported -> cron unsupported;
- system metrics online, scheduler history request failed -> show stale cron
  marker and last known data;
- system offline -> existing offline row remains authoritative; cron data may be
  retained but clearly stale and not presented as current;
- client daemon restarted -> remote history can reseed the cache from retained
  greggd ring;
- greggd restarted -> local history may remain, but current scheduler state
  resets to the new remote epoch.

Do not erase useful recent history on one transient scheduler-route failure.

## Tests

### Client daemon

- summary 404 -> unsupported without system offline;
- empty supported scheduler;
- history revision suppresses redundant history fetch;
- revision change fetches once;
- duplicate history response does not duplicate cache;
- remote epoch reset handled correctly;
- local cache per-job/global bounds;
- local daemon restart reseeds last remote five;
- transient scheduler error retains stale data;
- multiple TUI subscribers do not create extra remote cron polls.

### State/action

- plain c maps to ToggleCron;
- Ctrl-C still quits;
- d/n/c independent;
- cron sub-selection repair across system changes;
- default display depth = 5;
- config validation for display/cache bounds.

### Renderer

Ratatui buffer tests at representative heights/widths for:

- no jobs;
- unsupported old daemon;
- idle jobs;
- running job;
- high-load delay;
- load unavailable;
- success/failure history;
- truncated output;
- malicious terminal-control output;
- many jobs/history requiring bounded viewport;
- coexistence with drive/network expansion;
- normal and condensed mode.

## Documentation

Update:

- top-level README hotkeys/example;
- crates/gregg/README.md;
- config example;
- architecture/gregg-client.md;
- .opencode/skills/gregg-client/SKILL.md;
- AGENTS.md;
- CHANGELOG.md.

Document that remote scheduler history is memory-only and client history is
also memory-only in this line.

## Acceptance criteria

- [x] Client daemon polls scheduler summary without duplicating per-TUI work.
- [x] History body is fetched only on support discovery/revision change.
- [x] Old daemons remain online with cron marked unsupported.
- [x] Scheduler-route failures do not change Systems reachability.
- [x] Remote record deduplication handles repeated polls and daemon restart.
- [x] Local cache is longer than remote default but strictly bounded globally.
- [x] No cron history is persisted to disk.
- [x] Display history defaults to exactly five and is configurable.
- [x] Plain c toggles selected-system cron detail.
- [x] Ctrl-C, d, n, system navigation, and pane controls retain existing
      semantics.
- [x] Current load delay/unavailable state is obvious and truthful.
- [x] Selected job shows the configured number of recent terminal records.
- [x] stdout/stderr truncation is visible.
- [x] Terminal control sequences cannot escape into the user's terminal.
- [x] Cron rendering has a bounded height and works in normal/condensed modes.
- [x] Multiple TUIs do not increase remote scheduler poll cadence.
- [x] Default local checks and relevant existing CI jobs pass.
- [x] User/client architecture docs match behavior.

## Stop conditions

Open a corrective plan if:

- full local snapshot fan-out makes cron history dominate every metrics update;
- renderer requires a second independent fleet scroll state;
- output sanitizer cannot be kept small/pure;
- cache bounds still permit disproportionate memory under maximum endpoint/job
  cardinality;
- scheduler route errors become coupled to Systems online/offline state.

## Handoff

Plan 167 performs measured end-to-end closure across greggd, the client daemon,
multiple TUIs, platform lifecycle, memory/body limits, and binary footprint.

## Closure record

Complete at `5624b48`, in three commits: the sanitizer and bounded cron cache, the
`[cron]` configuration and the client-daemon scheduler poller, the two-tier local
publication, and the `c` detail block with its renderer and docs.

**How the objectives are met**

- The client daemon, not any TUI, polls. `/v2/scheduler` on a 30s cadence;
  `/v2/scheduler/history` only on first support discovery and on a
  `history_revision` change. A loopback test asserts three summary reads produce
  exactly one history read, and a revision change produces exactly one more.
- The daemon keeps a memory-only cache deeper than the remote retains, bounded
  three ways: per-job depth (`cache_history`, max 50), a **global constant**
  record ceiling of 4096 across every system/job/epoch, and the remote's own
  per-stream output cap. Nothing is written to disk; a restart reseeds from the
  remote, which a test covers.
- `display_history` defaults to exactly five, is configurable to 20, and is
  published in the document because a TUI never opens the config file.
- Plain `c` toggles the selected system's cron block. `Shift-J`/`Shift-K` move
  the job selection. Unshifted `j`/`k` keep their meaning. `Ctrl-C`, `d`, `n`,
  navigation, and pane controls are unchanged; the three expansions are
  independent and share one vertical budget.

**Three design decisions worth recording**

1. *Two-tier local publication, not one.* The summary always rides along in the
   existing document; history records are included only for systems a frontend
   has open. The intent governs **transmission and never fetching**, which is why
   ten windows with the pane open cost the fleet what zero windows cost —
   asserted end to end against a counting loopback remote.
2. *`send_replace`, not `send`.* See below; this was a Plan 164 defect.
3. *A global ceiling that is a constant, not a setting.* A user who raises
   `cache_history` must not be able to turn a bounded cache into an unbounded
   one.

**Defects found and fixed while implementing this plan**

- **`Ctrl-R` was dead without an `EggPool` entry.** The reload request sat behind
  an `EggPool`-configured early return, so the one reload boundary the whole
  architecture is built on did nothing for those configurations. Now handled
  before any pane-specific work.
- **A TUI attaching to a settled daemon was shown stale state.**
  `watch::Sender::send` fails when there is no receiver and *discards the value*
  when it does, so every document published while no window was attached was
  dropped; a window opening later got the startup document and then never
  another, because a quiet fleet publishes nothing more. It would have shown
  `pending` forever against an already-polled system. Now `send_replace`.
- **A remote serving the summary but not its history was reported as a failed
  summary**, discarding the job list that had arrived. The two failure axes are
  now separate: the summary is applied first and unconditionally, and a history
  problem is recorded on top of it.
- **`compose` budgeted rows by `chars().count()`**, so a CJK or emoji job name
  overflowed the terminal by exactly the amount the plan asks to be guarded. Now
  display cells.
- **`civil_from_days` derived the month from `day_of_era`** instead of the
  day-of-year, producing `(4405, 307)`.
- **`format!("{hour:0>2}")` does not zero-pad a `&str`**; a daily schedule
  rendered as the visibly wrong `3:0`.
- **Condensed mode reserved cron rows it never drew**, because it has its own row
  renderer. Fixed and covered by a test.
- **The cache pruned against a summary that had not arrived yet**, so a history
  document that arrived first read as "the remote serves no jobs" and discarded
  everything.
- **The global record counter was never incremented**, so the ceiling bounded
  nothing.

**Verification**

- `cargo test --workspace --all-targets --all-features`: 1,573 tests, all green
  (824 `gregg` lib, including 28 `ui::cron` renderer tests, 23 sanitizer tests,
  18 cron-cache tests, 13 poller tests, 23 cron-intent/state tests, 11
  client-daemon boundary tests).
- `cargo clippy --workspace --all-targets --all-features`: zero warnings.
- `./scripts/check-local.sh`: `=== all checks passed (mode: default) ===`.
- `cargo fmt --all -- --check`: clean.

**Not claimed here.** Binary footprint and memory measurement belong to Plan 167.
