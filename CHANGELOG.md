# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and
this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Cron observability in the TUI (Plan 166):** `c` expands a read-only cron
  block inside the selected system, and `Shift-J` / `Shift-K` move between jobs.
  The block lists every job a `greggd` schedules with its state, schedule, and
  last result, plus the run history of the selected job including its output.

  The client daemon reads a compact summary on a 30-second cadence and
  downloads the larger history document only on first support discovery and
  when `history_revision` changes, so the largest response in the system is
  never on the hot path to draw five job rows. Opening the pane in ten windows
  costs the fleet exactly what zero windows cost: the cron intent governs what is
  **published** and never what is **fetched**.

  The daemon keeps a deeper memory-only cache than the remote retains, so
  closing and reopening the TUI does not reset what has been observed. It is
  bounded by per-job depth, a global record ceiling across every system and job,
  and the remote's own output cap, so a pathological fleet cannot turn a bounded
  cache into an unbounded one. Nothing is written to disk; a daemon restart
  reseeds from the remote.

  New `[cron]` settings: `display_history` (default 5, max 20) is a viewport,
  `cache_history` (default 25, max 50) is retention. The table is optional, so
  existing configurations keep working unchanged.

  A `greggd` that does not serve the scheduler routes is reported as
  *unsupported* — the normal state for an older daemon in a mixed fleet — and is
  never reported as a system failure. A transient scheduler read failure keeps
  the last known data and is labelled stale.

  **Remote command output and remote job names are now escaped, not stripped.**
  Control sequences are rendered in `cat -v` caret notation, so a hostile build
  script printing `ESC [ 2 J` displays as `^[[2J` instead of clearing your
  screen, and an OSC title or hyperlink sequence cannot retitle your terminal.
  Printable Unicode, including wide CJK, is left intact.

### Fixed

- **Third audit corrective pass (Plan 182, 2026-10-07):** all eight findings
  from the logic/robustness audit of commit `adc948d` are fixed, and both
  recorded optimizations are applied. No mechanical gate was failing before
  these changes — `cargo test`, `clippy -D warnings`, `cargo fmt`, and
  `shellcheck` were all green — so every finding was logical. Every behavioral
  fix carries a regression test that was executed against the pre-fix logic and
  observed failing.

  Daemon: a load-blocked occurrence coalescing into the next civil occurrence
  no longer loses its gate reading (`state.last_gate = None` ran on the
  coalesce path, not just the fresh-occurrence one), so a job the gate is
  holding is published as `load_high` with its reading and a real
  `next_retry_unix_ms` countdown instead of as `waiting_for_slot` with
  `load: null`; nothing downstream would have restored it, because
  `defer_blocked_before` skips a pending whose `retry_at` is still in the
  future. The status and health handlers now read the clock *under* the
  published read guard rather than taking it as an argument evaluated before
  the await, so a read that parked behind a publication can no longer compare
  a pre-publication instant against a post-publication snapshot and serve a
  fresh sample as `503` (a client read that as `offline (http) HTTP 503`);
  the health retry attempt re-reads for the same reason. `Engine::new` now
  refuses an empty job set, so `next_deadline`'s assertion is backed by the
  module that makes it true rather than by a caller's guard elsewhere.

  Client: the per-system retained-job ceiling (`MAX_CRON_JOBS_PER_SYSTEM`, 64)
  is enforced unconditionally. It previously sat behind an early return taken
  whenever every cached job was still live, which is the normal steady state,
  so a remote configured with more than 64 jobs grew the cache without bound —
  proven by an executed test that retained 84 histories against a 64 ceiling.
  The sweep also uses a borrowed `HashSet` instead of a `Vec<String>` of cloned
  names with two linear scans. The EggPool footer's `Updated for Nm` age is
  part of the render-visible comparison, since that age *is* rendered and the
  footer's purpose is to state when a summary went stale; attempt ages remain
  excluded because no renderer reads them. The condensed view's offline status
  row now appends the same stable failure category the normal view renders
  (`offline (refused)`, `offline (http) HTTP 503`) inside the existing width
  budget; pressing `v` no longer drops the only statement of why a system is
  offline. A pending row still carries no reason, since it has no poll result.

  Test/CI: the sustained-runner smoke test's hardcoded 90-second timeout
  wrapped a subprocess that runs `cargo test --no-run`, so it reported build
  time as a workload failure on any cold or invalidated cache; it is now
  scaled to the build (1800s, overridable with
  `GREGG_TEST_BUILD_TIMEOUT_SECONDS`). A new blocking CI job runs `shellcheck`
  over every shell script and `pytest scripts/tests`, both of which previously
  ran nowhere — shellcheck existed only in the release workflow, against one
  file, with findings discarded by `|| echo`. The release workflow's installer
  check is now fatal rather than an echoed warning.

  Optimization: the scheduler observer compares its last publication field by
  field instead of deep-cloning the whole job vector to build a comparison key,
  so the common "nothing changed" wake no longer allocates and copies every
  configured job.

  One existing test changed rather than being added to:
  `empty_job_list_builds_no_engine_state` asserted that an empty job set
  produces a state-free engine, which is precisely the state `next_deadline`
  cannot serve a deadline from. It now asserts the refusal, and keeps its
  original point — a jobless daemon still pays no reconciliation cost, because
  `run.rs` never spawns the scheduler task.

- **Release builds could not recover from a panic, despite three code paths that
  exist to do exactly that.** The workspace release profile set `panic = "abort"`
  with no per-package override, so every crate and dependency was compiled
  abort-on-panic. `std::panic::catch_unwind` cannot intercept a panic under that
  strategy and a panicking task aborts the process instead of yielding a
  `JoinError`. That silently disabled the gregg-host drive-refresh worker's
  collector containment and backoff, `greggd`'s `run_with_shutdown` supervision
  of a dead server or sampler task, and `gregg`'s conversion of a panicked poll
  task into a synthetic `Cancelled` result — so instead of degrading, a panic
  anywhere in a collector or poll took down the whole TUI or the daemon and
  every HTTP consumer behind it. Plan 071 retained `panic = "abort"` on the
  recorded finding that "no unwind-dependent production behavior was found";
  `slow_probe.rs` was added afterwards and reintroduced the dependency that
  ruling ruled out, and the conclusion was never re-validated. No test could
  catch it, because Cargo forces `panic = "unwind"` for test and bench profiles,
  so the suite asserted a behaviour the shipped binary cannot exhibit.
  Unwinding is restored; binary size grows by the unwind tables this gives back.

- **A load-gated maintenance job made the cron pane show a permanent scheduler
  error on a healthy daemon.** `greggd` deliberately retains the gate decision
  that *admitted* a job, so a running row carries the reading it started under
  and an idle or slot-waiting row carries the *last* one — which the client
  renders as `start … <=` and `last gate …` precisely so an old reading is not
  presented as current load. The wire validator rejected exactly that shape for
  `idle` while permitting the identical meaning for `running` and
  `waiting_for_slot`. Because the client validates every scheduler summary and
  treats a failure as `Invalid`, any daemon with a `max_load`-gated job and
  available load telemetry — the common case — failed that system's cron fetch
  on every poll. The validator now accepts a retained gate in every non-deferred
  state; `load_unavailable` is still the only state where an observed reading
  contradicts the row.

- **A pending job that outlived its own `max_wait` could pin the daemon at 100%
  CPU.** The scheduler's `next_deadline` guards the `retry_at` candidate with
  `> now` precisely so an elapsed instant cannot park `sleep_until` on the past,
  but the `max_wait` expiry candidate twelve lines later had no such guard.
  `take_expired` only drops an expired pending while the load gate is still
  blocking, so a pending whose gate recovered keeps an expiry already in the
  past; `bounded_wake_deadline` only caps *above*, so it reached `sleep_until`
  ready, and the loop re-ticked and recomputed the same past deadline until the
  retry interval elapsed — stalling metrics, and every client reading them, on
  the single-threaded runtime that also serves HTTP. The candidate is now folded
  under the same `> now` rule.

- **`greggd croncheck` reported success when the daemon it started never came
  up.** The readiness wait's result was discarded and the arm returned `Ok(())`
  unconditionally. `croncheck` is the watchdog entry point that the managed cron
  block schedules every minute, and the child is spawned with null stdio, so on
  a system where the daemon cannot start — a config parse error, a bind
  conflict, a slow first start — every run exited `0` and the watchdog looked
  healthy forever without ever restarting the daemon. Reaching the deadline
  without a healthy answer is now a failure that names the last probe, matching
  the readiness contract other paths already honour.

- **Installing one config's cron watchdog deleted another config's.** The
  install path removed *any* Gregg managed block by marker text before writing
  its own, while uninstall correctly proved config ownership first. Two
  `greggd` configs on one account installed with `--method cron` meant that
  `startup install --config <B>` silently destroyed config A's watchdog. Install
  now proves ownership of the existing block with the same config-aware
  classifier uninstall uses, and preserves a `Foreign` or `Unknown` block
  exactly.

- **The `sudo` remediation hint for a privileged `startup install` emitted an
  unquoted executable path.** A path containing whitespace, `$`, or a quote
  re-parsed into different arguments — or a different binary — when an operator
  copied the hint. It now uses the same POSIX single-quote helper `gregg-update`
  already applies to its own hints. Display-only: `greggd` never runs it.

- **The EggPool endpoint skipped the sanitization chokepoint.** `AppState`
  adoption is documented as the single point every renderer path passes through,
  and every other document string is cleaned there — but the EggPool endpoint
  was copied raw, and its `name` is the one operator-supplied string in the
  config model with no control-character rejection. A config value like
  `"[2Jwiped"` therefore reached a cell unescaped and was silently stripped
  rather than shown in caret notation. It now goes through `clean` like the
  rest of the document.

- **A test asserting the cron pane-budget invariant never ran.** The function
  had no `#[test]` attribute and `#![allow(dead_code)]` on the `ui` subtree
  silenced the unused-function warning that would otherwise have caught it, so
  the rule that remote-depth and viewport truncation are reported *separately*
  had no coverage at all.

- **`gregg daemon startup install` could erase your entire crontab.** On Linux
  without user systemd — the documented `auto` → cron fallback — the bounded
  manager runner read a child's stdout and stderr only *after* the child exited.
  A pipe holds about 64 KiB, so a `crontab -l` whose output exceeded that blocked
  in `write(2)`, never exited, and was killed at the 20s timeout. That timeout
  returned the same `None` a spawn failure would, which the crontab reader mapped
  to `String::new()` — "you have no jobs". The ownership gate then classified the
  empty table as `Absent`, the merge produced a table containing only the Gregg
  block, and `crontab -` **replaced the whole file**. The command reported
  success. Any hang reached the same path, so a stuck `crontab` (an NFS home, a
  wedged PAM lookup) was enough with no large output at all.

  Both halves are fixed. The shared runner now drains both pipes *while the
  child runs*, on the same discipline `gregg-update` uses for its downloads.
  And the crontab read **fails closed**: only a successful exit, or a non-zero
  exit carrying the standard "no crontab" diagnostic, is trusted as a table.
  A timeout, a spawn failure, non-UTF-8 output, or an over-cap read is now an
  error naming `gregg daemon startup instructions`, so the crontab is never
  rewritten from an unverified read. `uninstall` shares the reader and reports an
  unreadable crontab as *unknown* rather than *already absent*.

- **Windows disk telemetry asked for the wrong thing and opened the handle with
  no rights.** `DeviceIoControl` was issued `0x00070020`, which decodes as
  `FILE_DEVICE_DISK`, function `0x08`, `FILE_ANY_ACCESS` —
  `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`, not `IOCTL_DISK_PERFORMANCE` — so no
  performance counters ever came back. Separately, the `\\.\PhysicalDriveN`
  handle was opened with `dwDesiredAccess = 0`, and the I/O manager compares the
  control code's required access against the access already granted *before* the
  driver sees the request, so it failed `ERROR_ACCESS_DENIED`. Either mistake
  alone left `disk_io` empty on every Windows host, and the collector publishes
  `disk_io: None` for an empty set — so `R/s`, `W/s` and the `IO` row were
  silently missing with no error and no capability flag. The control code is now
  composed from its published parts as `CTL_CODE(FILE_DEVICE_DISK, 0x20,
  METHOD_BUFFERED, FILE_READ_ACCESS)` = `0x00074080`, and the handle carries
  `FILE_READ_ACCESS`. Both had to land together; verified by tests that recompute
  the code from first principles, with native Windows CI as the only runtime
  authority.

- **macOS leaked the whole `getifaddrs` list on every sample.** The interface
  walker took ownership of the kernel-allocated chain and released it once, at
  the end — but a non-UTF-8 interface name returned early through `?`, skipping
  that release. Because the collection runs once per sample, a long-running
  `greggd` leaked unboundedly until restart. The list is now owned by a scope
  guard installed immediately after the success check, so every exit path
  releases it exactly once, and a non-UTF-8 name skips just that interface
  instead of aborting the walk.

- **One non-UTF-8 macOS mount point discarded every drive.** `getmntinfo` walk
  aborted the whole collection on the first mount whose name or filesystem type
  was not valid UTF-8, so `drives` was permanently empty while that mount
  existed. Malformed records are now skipped individually, matching Linux's
  `parse_mountinfo_line`.

- **Tunnel and virtual adapters were counted twice on Windows, macOS and
  FreeBSD.** All three treated every non-loopback interface as an aggregate
  member, so a VPN adapter's counters (the encapsulated frames) and its physical
  underlay's counters (the same frames) were both summed — approaching 2× on a
  saturated tunnel. Because the directional capacities were summed from the same
  rows, the utilization denominator inflated with the numerator. Linux already
  excluded this class through a slave interface's `master` symlink; the other
  three now decide by interface type (`IF_TYPE_TUNNEL`, `IFT_TUNNEL`/`IFT_GIF`,
  WireGuard, `utun*`'s `IFT_OTHER`, and the pseudo types), by exclusion over a
  closed set so an unrecognised type still counts.

- **`gregg daemon restart` reported success with nothing running.** It discarded
  the result of its own readiness wait, so a child that exited at startup — which
  `run_daemon` does on any config validation violation, since it begins with
  `store.load_or_default()?` — still printed the success line and exited 0. It
  now propagates the failure, which also means `gregg update` reports
  `RelaunchFailed` instead of claiming `Relaunched`.

- **A concurrent `restart` could kill the daemon a launching TUI had just
  started.** Its destructive stop ran outside the config's launch lock,
  breaking the invariant the module states about itself. It could observe a
  daemon that `ensure_running` had just spawned and was still waiting to become
  ready, stop it, and leave that TUI to exhaust its 15s budget and exit
  `NotReady`. The lock is now taken before the stop is even considered, and the
  running-or-not decision is re-made from inside it.

- **A dead poll or EggPool task was retired instead of stopping the daemon.**
  `SchedulerStopped` and `EggpoolWorkerStopped` existed and `is_fatal` already
  called them fatal, but nothing ever constructed them: the engine dropped the
  receiver and kept serving documents whose numbers could never change again —
  indistinguishable, to an operator, from a healthy fleet that is merely quiet.
  Both variants are now raised. A closed channel unambiguously means the task
  ended, because the scheduler deliberately stays alive with zero endpoints so
  `Ctrl-R` can add systems.

- **`greggd` config writes deleted a concurrent writer's live temp file.** The
  stale-temp sweep matched `.greggd-*.toml.tmp` with no age check, so a second
  writer — a daemon bootstrap racing `startup install` or `uninstall --purge` —
  could unlink a temp file between its creation and its `rename`, failing a write
  that had already succeeded. The client side of the workspace already gated this
  on age for exactly this reason; the daemon now does the same.

- **`greggd startup install` widened the permissions it promises to preserve.**
  The repair added missing bits by *replacing* the mode, so an operator's
  deliberate `0750` directory and `0640` config — the very modes its own comment
  names as preserved — were silently loosened to world-readable on every install
  and restart. It now ORs the missing bits and leaves everything else alone.

- **A link capacity measured in only one direction was rendered as a capacity
  for both.** The aggregate collapsed a one-sided payload into a bare number, so
  the pane asserted a capacity the daemon never measured — while rendering
  `800Mb/s/1.60Gb/s` two lines later when it did know both, and scoring
  utilization per direction precisely because capacity is directional. A
  one-sided capacity is now labelled `rx` or `tx`. Windows can produce this:
  each direction comes from an independent MIB value.

- **The EggPool pane measured fit in characters, not terminal cells.** An
  identity containing wide glyphs passed the fit test while overflowing the
  one-row header rect, and the clipped surplus was the `Window:` indicator — the
  one thing the pane is supposed to keep. Fit now counts display cells, like
  `truncate_width` and the sanitiser already did.

- **An EggPool worker that had never once succeeded claimed to be "updated".**
  The failure footer rendered absence of a timestamp as the word `updated`. It
  now says `never updated`.

- **A cron-history depth change alone never repainted.** `adopt_snapshot`
  decided visibility from the pre-update values, but compared
  `cron_display_history` against itself — it had already been overwritten two
  lines earlier, so the term was always false. A document that changed nothing
  but the depth (exactly what a `Ctrl-R` reload produces) was adopted as
  unchanged and the cron pane kept its old height.

- **Connect failures that were not "nothing there" authorized a daemon
  spawn.** Any `TransportError::Io` classified as plain absence, so fd
  exhaustion (`EMFILE`/`ENFILE`) or `EACCES` on the socket started a daemon
  nobody asked for, which then failed readiness for 15 seconds. Only
  `NotFound` and `ConnectionRefused` are absence now.

- **`gregg daemon status` named an endpoint the daemon was not using.** It
  reported the first candidate path even when the daemon had bound the temp
  fallback — printing a path that does not exist for the case an operator
  reaches for exactly when "running, but there is nothing at the path it
  printed". A connect now reports the candidate it reached.

- **A skipped socket candidate recorded no reason.** `bind` noted an occupied
  endpoint and a bind failure but silently `continue`d past a candidate whose
  parent directory did not exist, so a typo'd config directory bound the temp
  fallback with no mention of the directory — while the launch lock, derived
  from that same path, failed separately for the same cause.

- **Operator remediation commands were emitted unquoted on Unix.** A `sudo`
  rerun hint or a `cargo uninstall --root` handoff printed a path with a space
  unquoted, so the documented recovery path ran the wrong binary. Paths are now
  POSIX single-quoted, matching what the Windows branch already did with the same
  argument.

- **Staging paths were lossily converted before being passed to external
  commands.** `TMPDIR`/`HOME` with non-UTF-8 bytes produced a U+FFFD-substituted
  path naming a different — normally nonexistent — file, so `curl -o` failed as a
  download error and, worse, `cargo uninstall --root` could have targeted a
  different installation than the ownership check proved. Paths are now passed
  as `OsStr` arguments with no conversion.

- **The normal-view cron pane painted over the system card it belongs under.**
  `c` in the default view put the `CRON` header, the selected job's header, and
  its record rows directly on top of `name@host:port`, CPU, MEM, SWP/COMMIT and
  DISK, leaving the rows `layout.rs` had reserved for the pane blank. The cron
  block draws relative to the rect it is handed and does no rebasing of its own;
  the drive and network renderers rebase by offset and the condensed view rebased
  inline, but the normal view passed the unrebased card rect. It now starts where
  the previous detail ended. `cron::render` additionally clips to the rect it was
  given, so an over-generous budget can never reach the next system or the
  footer.

- **A second `gregg` launch could kill the client daemon the first one had just
  installed.** The launcher took its advisory launch lock only after an initial
  probe, and that probe was not a pure query: when it classified the endpoint as
  an older *owned* daemon, it went ahead and stopped it. Two launches racing
  against one stale daemon therefore meant the loser waited on the lock and then
  acted on a classification it had taken before waiting — tearing down the
  current, healthy daemon the winner had just spawned and disconnecting every
  attached TUI. The probe is now side-effect-free and returns a classification;
  the destructive rotate happens only with the lock held, on a classification
  redone at that moment. The rule the launcher already documented — only plain
  absence authorizes a spawn, and the classification is redone under the lock —
  now covers the stop, which is at least as destructive as the spawn.

- **`gregg update` could hang instead of failing.** The shared child runner's
  deadline bounded the child's *lifetime* only. Once the child exited — and on
  the timeout path itself — it joined the pipe reader threads with no bound, and
  a descendant that inherited a write end keeps that pipe open forever. The
  result was a silent hang rather than an error. Pipe bytes now arrive over a
  channel (a `JoinHandle::join()` cannot be given a deadline) and every drain gets
  one fixed 250 ms post-exit settle bound. An expired bound is reported as an
  error, not as short output: an incomplete capture that looked complete would
  let a caller act on a version line or status code it never received.

- **A newline in a remote name silently merged the two halves into a name that
  does not exist.** Control characters in remote text are rendered in `cat -v`
  caret notation, but a `\n` was deliberately left inert because job output is
  multi-line and splits on it. A drive name, job name, interface name, or
  hostname occupies exactly one row, and ratatui *drops* a control grapheme
  rather than breaking the row — so `/dev/sda1\nroot` rendered as
  `/dev/sda1root`, a device that is not on the machine, with no `^J` and no
  marker. Sanitizing now has two modes for a newline (separator for a body,
  anomaly `^J` for a one-row field), and the one-row mode is applied at the
  single chokepoint every single-row field already passes through.

- **An overdue cron schedule rendered a confident `next 0ms`.** A due instant
  hours in the past saturated to zero, which on a pane whose purpose is to say
  when a job runs reads as "starting now" for a job that has not started. A
  passed instant now reads `due`. Elapsed ages are unchanged: there, a future
  stamp is clock skew and `0ms` correctly means "just began".

- **The EggPool footer always claimed the result was recent.** It read
  `Updated recently` regardless of when the last summary refresh actually
  succeeded, so an hours-old snapshot read the same as a fresh one. The footer
  now reports the real age using the cron pane's unit ladder, which keeps the two
  panes from disagreeing about how long three minutes is.

- **The cron pane's local line-drop marker reused the remote truncation label.**
  When both facts held, the row read `stdout+: more lines not shown`, so the `+`
  that means "the remote cut this stream's tail" also labelled a purely local
  budget drop. The two are different facts and now read differently: the remote
  keeps `stdout+`, the local drop says only `more lines not shown`.

- **A failed atomic config write could not say why it failed.**
  `AtomicWriteError::VerificationFailed` was a unit variant that discarded the
  underlying error, so a re-read that could not open the temporary file and a
  re-read that got *different* configuration were one opaque label — and an
  intermittent `write_atomic` failure under parallel tests was undiagnosable from
  its own report. The two modes are now named separately.

- **The CI Clippy gate failed on `main`.** `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` exited 101 with 17
  `large_futures` errors: the scheduler's `await_child_completion` had grown to
  ~17 KB because each `drain_step` future carried its 4 KiB read scratch buffer
  *inline*, and the scheduler keeps several of those futures alive at once. The
  scratch now lives in the tail it was folded into, so a drain future's size no
  longer scales with the read size, at one allocation per stream instead of one
  per read.

- **Test temp directories were keyed by label alone, and the helper deletes
  before it creates.** A duplicate label — from a renamed test, say — would have
  one test remove another's in-flight config temp file, which surfaces as an
  intermittent verification failure naming a directory nobody else can see. The
  pid and a process-lifetime counter now make each directory unique, so the
  labels are the readability aid they were always meant to be.

- **`greggd` could be held past its own shutdown bound by an orphaned
  collection.** A collection cycle runs on the blocking pool and
  `spawn_blocking` cannot be aborted. When shutdown won the select the sampler
  dropped the cycle's handle without awaiting it, which *detaches* the task
  rather than cancelling it: the blocking read kept running and `Runtime::drop`
  then blocked on the pool drain with no bound and no diagnostic — after the
  daemon had already logged a clean sampler shutdown. A read that was merely
  slow cost extra exit latency; a genuinely hung one (macOS registry traversal,
  a Windows CPU/power query, FreeBSD `devstat`, a wedged `/proc` read) held
  process exit indefinitely. A cycle that outlives the loop is now joined under
  an explicit one-second bound and detached with a warning only past it, so the
  shutdown path stays bounded and the pathological case says why.

- **A collection that finished in the same poll a stop signal arrived in was
  silently discarded.** The sampler's select was not `biased`, so the winning
  branch was pseudo-random. If the cycle completed and shutdown became ready in
  the same poll, a real sample was dropped without being applied or published —
  and if that sample was a *failure*, it was never counted, so the daemon exited
  reporting readiness the collector had contradicted. This is the ordinary case
  for a fast collector on the one-second default interval. The collection branch
  is now polled first, matching the ordering discipline already used in the
  scheduler and the EggPool worker.

- **A Linux host with readable `/proc/net/dev` but unreadable
  `/sys/class/net/<if>/flags` counted loopback in the fleet network
  aggregate.** The `IFF_LOOPBACK` bit came from a sysfs read whose failure mode
  was `0`, i.e. "not loopback". Because `lo` has no `master` symlink, the
  interface became an aggregate member and its traffic was summed into
  `aggregate_rx_bytes_per_sec` / `aggregate_tx_bytes_per_sec`, while also
  publishing `is_loopback: false` — a claim the client cannot contradict and the
  validator cannot catch. Triggered by `/sys` masked or unmounted (some container
  and chroot setups) or `EACCES` on a hardened host. An unreadable or
  unparseable flag set is now *unknown*, and an unknown interface stays out of
  the aggregate. macOS and Windows derive loopback from native flags and could
  not fail open this way.

- **`logical_cores` reported the process's CPU allotment, not the kernel's core
  count.** The field is documented as the number of logical cores available to
  the kernel, but it was sourced from `std::thread::available_parallelism`,
  which honours a `taskset` mask or cpuset. A restricted daemon published
  `logical_cores: 1` on a sixteen-core machine and disagreed with the Windows
  collector's kernel total. The Linux collector now reads the kernel's own
  online-CPU list, falling back to the `processor` entries in `/proc/cpuinfo`
  when sysfs is masked. Both are affinity-independent; the process-allotment
  probe is now only a last resort behind them.

- **An absent `SwapTotal` was indistinguishable from "this machine has no
  swap".** A `/proc/meminfo` that parsed but lacked the key produced a confident
  `swap = {0 bytes, 0.0% used}` *with the swap capability asserted* — which the
  protocol requires to be accompanied by a swap sample, so nothing could tell a
  truncated read from a genuinely swap-free host. Swap now meets the same bar
  memory already set for an absent `MemTotal` and fails closed. A real
  `SwapTotal: 0` is still the legitimate zero sample.

- **Closing the EggPool pane left the document claiming `Refreshing`.**
  `worker_state = Refreshing` means a current desired request was published and
  its result is awaited, but the same code ran for *deactivations*, where an
  inactive desired state aborts in-flight work and emits no synthetic result.
  Nothing could ever resolve it — the reducer's only escape for "a request that
  will never arrive" is `EggPoolFetchOutcome::Cancelled`, which is never
  constructed. Both ordinary deactivations hit this: a fresh client whose pane
  is closed, and the pane closing on the last attached frontend. A deactivation
  now returns the field to `Idle` while still superseding any in-flight result.
  Activation now reads the *converged* intent rather than one frontend's
  request, so a second frontend holding the pane open is still honoured, and an
  unreachable duplicate of the convergence block was removed.

- **An all-digit cron hour above `u32::MAX` rendered as a fabricated `00:00`.**
  `schedule_label` checked that the field was ASCII digits, but an all-digit
  string can still overflow `u32`, and the overflow was silently rewritten to
  midnight — a plausible-looking wrong time on a pane whose entire purpose is
  to say when a job actually runs, and the opposite of the function's promise to
  keep the raw schedule verbatim where it does not fit. A field that does not
  parse as a time is now kept verbatim. A current `greggd` rejects such a
  schedule at config validation, so this needed an older, faulted, or hostile
  remote — but remote scheduler data is untrusted input.

- **Re-installing or uninstalling Gregg could delete the user's own crontab
  jobs.** A managed cron block is exactly two lines and is appended last, so a
  job the operator later added with `crontab -e` landed *after* it with no blank
  line to scan for. The rewrite consumed lines until the next blank one, so that
  job was silently dropped and the truncated table written back — on the Linux
  path where systemd is unavailable. Merge and removal now consume exactly the
  marker and the one command line it announced, which is all a managed block
  contains. This was the only place Gregg could destroy data outside its own
  ownership.

- **A `greggd` job that ran longer than a minute published an empty output tail
  while claiming nothing was truncated.** The wake is capped at the 60-second
  civil-clock reconciliation bound, so a long-running job had its drain future
  cancelled and rebuilt every wake — and the bounded tail was a local inside that
  future, so every byte read before the last wake was dropped. A job that logged
  its line and then slept published nothing at all. The tail now belongs to the
  running child, so what a wake cancels is the read, not the capture, and
  `truncated` again means what it says. History capture is on by default, so this
  affected ordinary long jobs rather than an opt-in edge.

- **A refused `j`/`k` on the EggPool pane was lost permanently.** The intent
  request is a bounded `try_send`, so it is refused whenever the daemon is busy,
  and the pending request was then erased by the next document: the next key
  press re-derived the old converged window and was suppressed as unchanged. The
  request now survives until a document *confirms* the window, and an arriving
  document re-offers it — so the pane converges without a second key press.

- **Five reloads in one poll generation could freeze every attached TUI.**
  `Ctrl-R` and every mutating CLI nudge ask the poll task for an immediate poll
  through a 4-slot channel that is only read *between* generations, so a few in
  a row filled it. The ask was a blocking send on the single engine task that
  owns the fleet and writes every frontend's document, parking it for the rest of
  the generation and leaving a stop request unread. It is now a `try_send`: a
  refused ask costs the early poll, which the fixed cadence provides anyway, and
  never the fan-out.

- **Adding an `[eggpool]` entry with `Ctrl-R` left the pane permanently
  `Refreshing`.** The worker was spawned once from the startup config, so a
  config that gained an entry published a pane no worker could answer; any
  request moved it to `Refreshing` and nothing ever resolved it. The worker is
  now wired from the current config, so adding, removing, or repointing the
  entry all work. Removing it already did.

- **A native read that hung longer than the collection timeout accumulated one
  parked OS thread every sampling interval.** A timed-out `spawn_blocking` cannot
  be cancelled, so the cycle kept running and kept holding the collector mutex
  while the loop queued another closure behind it every interval — exhausting the
  blocking pool in under an hour. At most one collection cycle is now
  outstanding, and a hung one is picked back up rather than joined by a second.

- **A departing frontend's cron intent kept being transmitted.** The removal
  happens on the connection task, which has no engine handle and so raised no
  publication trigger: attached windows kept receiving that history, and a newly
  attached one was handed history it never asked for, until some unrelated change
  happened to republish. The intent set now carries a revision, so any change
  republishes. Reopening the cron pane still shows the daemon's retained history
  without refetching.

- **A dead EggPool worker republished a full fleet document every 250 ms.** A
  failed publication left the converged memo unset, so each reduce tick
  re-detected the same failure and republished forever. It is recorded once; a
  different intent still retries.

- **The per-connection read buffer had no ceiling.** One announced frame is
  capped, but the number of *pipelined* frames was not, and the serve loops drain
  at most one request per tick — so a peer on the private endpoint that sent
  faster than it was served grew the buffer without bound. It is now cut off at
  one frame plus a read chunk of headroom.

- **An EggPool worker could not be cancelled while the daemon was not reading
  results.** Delivering a finished fetch awaited the send inside the completion
  branch, so a full result channel left cancellation, a newer desired state, and
  the passive refresh deadline unpolled. Delivery now waits for a slot inside a
  `select!` biased toward cancellation.

- **A window change cost two fetches instead of one.** The converged memo was
  recorded before the worker generation was bumped, so the next tick saw a
  difference and republished the same window one generation later — which the
  worker treats as superseding, aborting the fetch it had just started. The memo
  now records the generation the worker was actually driven with.

- **A raw NUL byte in `gregg-protocol`'s v2 validation tests** made that source
  file classify as binary to `grep` and `diff`. It is spelled as an escape, like
  the rest of the codebase.

- **The `main` clippy gate was red on four sites** (`double_must_use`,
  `doc_markdown`, `needless_borrows_for_generic_args`, `large_stack_arrays`)
  after a commit whose stated verification was the fmt-and-tests loop. The gate is
  green again; `./scripts/check-local.sh --release` runs it.

- **A `greggd` child that wrote more than one pipe buffer could wedge the
  scheduler forever.** The wait for a running job awaited the child's exit
  *before* draining its output, so a job that filled the 64 KiB stdout pipe
  blocked in `write(2)` while nothing was reading: the wait never resolved, the
  one global child slot never freed, and no further job could start on a
  daemon that was otherwise healthy. The wait and both drains are now polled as
  one join, which is what makes a flooding child's writes and this process's
  reads progress together. The flood test drives that production future instead
  of reimplementing the join, so it can no longer pass while production wedges.

- **A frontend disconnect could freeze the EggPool pane of every window still
  open.** Converging the worker's window happened on the request path only, but
  a disconnect removes an intent with no request of its own — so the worker
  switched windows while fleet state kept the old one. The reducer accepts a
  result only on a window match, so from then on every fetch was discarded: the
  surviving pane stayed on its pre-disconnect summary and the worker kept
  burning a request per interval. The converged window now lands in the fleet on
  every path that changes it, disconnect included.

- **A stalled TUI could be disconnected for being slow.** The local protocol's
  write path treated a full socket buffer as a dead peer, so a frontend that
  stopped reading for a moment lost its connection. It is backpressure: the
  unsent tail of the frame in flight is now kept and pushed as the peer drains,
  and a document arriving meanwhile is queued behind it, with the newest
  superseding the older queued one. The frame in flight is never dropped —
  a length-prefixed frame with a hole in it desynchronises the peer's parser
  permanently.

- **The `PERFORMANCE_INFORMATION` mirror did not match the Windows ABI.** Its
  members were all declared `usize`, which misreports the buffer size the API
  validates `cb` against and moves the four `DWORD` counters by four bytes
  each, and `PhysicalTotal` / `PhysicalAvailable` were in the wrong order. The
  member types and order are now ABI-faithful, the struct's size and every
  offset this crate reads are asserted at compile time, and the commit-charge
  read seeds a zeroed buffer so nothing is read as uninitialised memory. A
  `hw.memsize`-style total of zero is no longer published as `0 / 0` on Windows,
  macOS, or FreeBSD: it is an error, so the memory family is omitted rather than
  rendered as a confident `0.0%` used on a machine that could not be measured.

- **A conforming daemon could be shown as offline for being well-formed.** The
  client's 64 KiB response-body cap was verified against drives alone, but a
  maximum-bound v2 payload — 32 drives, 32 disk-io devices and 32 network
  interfaces, each with a name and id of up to 512 bytes — measures 111,465
  bytes, which the cap refused as a decode failure. The cap is 256 KiB: a
  hostile-input ceiling, not a wire-format limit, and one that clears what the
  protocol allows. The measurement now covers every capped collection at once.

- **A control byte in a config-reload diagnostic reached the terminal.** The
  diagnostics pane renders that text verbatim, and it is the daemon's
  config-parser output quoted from a local file. It now goes through the same
  sanitising chokepoint as the rest of a document, so an escape is rendered in
  caret notation instead of being able to repaint the pane.

- **A job waiting for the child slot was recorded as expired by load.** Expiry
  ran before the global child-slot check and was gated only on the job having a
  `max_load`, so a job whose threshold the host was comfortably under was
  dropped with a `load_expired` record blaming load for a slot the gate never
  saw. An occurrence can only expire while the load gate is actually refusing
  it; one that is merely waiting for the slot stays pending and runs when the
  slot frees.

- **The cron pane labelled `stderr` as `stdout`.** A failed job's error output
  — and the `+` marking that the remote truncated it — were both reported on
  the wrong stream, which sends an operator to the wrong place to look for the
  failure. Each stream now carries its own name, truncated or not.

- **A download could grow the updater's heap without bound.** The release-asset
  and checksum downloads piped `curl`'s stdout and stderr into unbounded
  buffers while every other child was capped. The asset body goes to a file, so
  these pipes carry only the status line and diagnostics; they are capped now
  too, and a mirror that floods them fails the download.

- **A refused pane intent was remembered as sent.** The event loop's
  last-intent memories suppressed a re-send, but they were written whether or
  not the request reached the daemon, so one full request channel left the pane
  silently on a window the daemon had never been told about. An intent is only
  remembered once it is actually queued, so the next drain re-offers it.

- **A frontend could spin a core on a partial frame.** After reading part of a
  frame, the read loop had no await point, so the loop condition depended only
  on bytes already in hand and nothing was left to wait for. The loop now
  yields between the halves of a frame.

- **`gregg remove`'s IPv6 advice told the operator to type something it
  rejects.** The malformed-bracket message suggested `[ipv6]:port`, but the same
  error comes from `remove`, which takes a host on its own. The advice is now
  about the bracketing it is actually complaining about.

- **`greggd` busy-looped a core while a job waited behind a load gate.** A
  job that became due and was held back by `max_load` in the same tick kept an
  already-elapsed `retry_at`: the reschedule pass that pushes such an occurrence
  forward was reachable only *after* a winner was chosen, and it explicitly
  skipped the winner. Nothing was eligible, so it never ran. With the global
  child slot idle, that elapsed `retry_at` became the `sleep_until` deadline, so
  the loop woke, republished, and re-checked immediately — pegging one core and
  republishing a fresh civil timestamp several times a second until the gate
  opened, which a `retry_interval` of up to 24 hours makes a long wait.

  A gated occurrence with a free slot is now rescheduled like any other, and a
  retry that is already in the past is no longer used as a wake deadline. A
  load-delayed job is also published as load-delayed for its whole wait, with a
  real `next_retry_unix_ms` countdown instead of the second the tick began in,
  and the job actually holding the child slot reports `running_since_unix_ms`
  for its whole run rather than only at the instant it started.

- **Windows `Tx/s` was a receive counter on every host.** The `MIB_IF_ROW2`
  mirror declared in `gregg-host` omitted the six `*Octets` members the SDK
  places inline, so `out_octets` read the kernel's `InUcastOctets` — transmit
  bytes rendered as unicast *receive* bytes — and the 48-byte short struct
  stepped `from_raw_parts` through `GetIfTable2`'s allocation with the wrong
  stride, so every interface after the first was read from the wrong offset. The
  size and the offsets of the members this collector reads are now asserted at
  compile time, so a future mismatch fails the build instead of the numbers.

- **Two frontends with different EggPool windows left both panes stuck.** Fleet
  state stored the *window one frontend asked for* while the worker was driven
  with the converged shortest active window. The reducer accepts a result only
  when the window matches, so as soon as two frontends disagreed, every result
  the worker actually fetched was dropped: both panes sat on `Refreshing` with no
  data and no error, and the next key press from either one re-minted the
  generation. Fleet state now carries the converged window the worker is driven
  with, which is also the window the pane is allowed to show.

- **The cron pane's vertical budget was miscounted and the job table uncapped.**
  A record's row cost was estimated flat, so multi-line output under-counted its
  own height and pushed the newest record out of the budget; the count is now
  derived from the same function that renders the lines. The job table is also
  bounded by the constant the budget is built from, so a wide table can no
  longer consume the block and hide the selected job's history.

- **A swapless host reported `SWP 0.0%`.** Linux reports swap with a zero total
  when there is none, which means there is nothing to measure rather than a swap
  measurably at zero. The row suppressed `0 B / 0 B` and then fell through to
  the percentage, so the one row that had no measurement behind it was the row
  that claimed a measured zero. It now renders `—`, as a host that omits swap
  entirely already did.

- **A stored IPv6 host could not be removed by the form stored in the config.**
  `gregg add [2001:db8::1:2]:11310` stores the host `2001:db8::1:2`, but
  `gregg remove 2001:db8::1:2` was refused, because that string really does read
  as both a host and a host plus the port `2` — the ambiguity `add` exists to
  reject. `remove` takes a host on its own and has no such guess to protect, so
  it now reads a bare IPv6 literal as that host; the bracketed forms are
  unchanged.

- **FreeBSD read each interface row from a possibly unaligned address.** The row
  was dereferenced in place out of a `Vec<u8>`, whose alignment is 1, while
  `IfmibData` embeds `u64` counters. It is now copied out with `read_unaligned`,
  matching the macOS collector. Two page counters that silently became `0` when
  `sysctl` failed now propagate their error like their two neighbours, instead
  of understating available memory and overstating usage.

- **A Windows daemon rotation never waited for the old daemon to release the
  pipe.** The wait checked that the endpoint path existed, and a named pipe is
  never a filesystem entry, so it returned immediately and a replacement could
  race a daemon that was still unwinding. It now asks the transport directly,
  the same way the readiness probe does.

- **Cron record eviction was not oldest-first.** The derived ordering that picks
  the victim led with the system and job name, so the alphabetically first
  `(system, job)` pair was dropped rather than the oldest record. The documented
  policy is now what the code does, ties included.

- **`gregg daemon stop` reported success for a non-answer.** A state document
  the handshake left buffered, a protocol violation, or a timeout was all read
  as a completed stop. Only the acknowledgement or a clean disconnect is now a
  success; a buffered document is skipped, since it is legitimate traffic rather
  than an answer.

- **The TUI redrew on every poll for a host whose data `sanitize` rewrites.**
  The visible-change check compared the sanitized stored state against the raw
  document, so a string containing a tab or a bare carriage return differed on
  every document. Both sides are now cleaned the same way, as the cron branch
  beside it already was.

- **A `gregg-update` test reported a spawn failure as a missing file (Plan 172).
  Test-only; no update behavior changed.** The download-classification tests
  asserted `DownloadOutcome::Failed(_)`, which is also exactly what a failed
  `Command::spawn` produces. Under a loaded CI runner a transient `fork` `EAGAIN`
  therefore satisfied the "HTTP 500 is a hard failure" assertion, and the next
  line then read a `calls` file the stub never had the chance to write — so the
  build went red with `ENOENT` on a file that has nothing to do with HTTP
  classification, pointing at the wrong code entirely.

  Every stub `curl` now records itself in a `calls` log as its first act, so a
  test can tell "curl never started" from "curl started and was classified"
  using the child's own side effect rather than a production error string. A
  spawn failure is retried a bounded number of times, and if it never succeeds
  the failure is a named assertion about the spawn that carries the underlying
  error — never a missing `calls` file.

  The four sibling download tests that shared the over-wide assumption are
  corrected in the same pass, and the real-`curl` 500 baseline now records
  whether a client actually reached the fixture before accepting its result.

- **`gregg daemon run` never exited on Windows (Plan 171).** A stop request, a
  supervisor stop signal, and Ctrl-C were all acknowledged — the engine loop
  unwound and the endpoint was released — and then the process hung instead of
  exiting. The accept wait is parked on the blocking pool, and aborting the accept
  task cannot cancel a `spawn_blocking` job that is already running, so the
  blocking thread stayed in `ConnectNamedPipe` waiting for a client that was
  never coming. `gregg daemon run` then dropped its runtime, and Tokio's shutdown
  waits for outstanding blocking jobs without a timeout. A later `gregg` found a
  live pipe held by a wedged process and attached to a daemon that was no longer
  polling, with nothing to explain why.

  The wait is now **stopped** rather than abandoned: the parked thread publishes
  a real `THREAD_TERMINATE` handle to itself, and the stop request cancels the
  pending `ConnectNamedPipe` with `CancelSynchronousIo` — the documented
  mechanism for interrupting synchronous I/O from another thread. The handle is a
  real handle and not a thread id, because ids are recycled when a thread exits
  and cancelling by id could interrupt an unrelated thread.

  The signal path had the same defect independently: it returned from its
  `select!`, which dropped the daemon's future and skipped the whole teardown.
  It now awaits the daemon after cancelling it, so a supervisor stop and Ctrl-C
  take the same path as a stop request.

  The handshake, frame format, owner-only SDDL, `PIPE_REJECT_REMOTE_CLIENTS`,
  instance rotation, and the `Hello`-first contract are unchanged, as is every
  Unix code path. Windows-native tests now cover the interrupt in both orderings
  — before the wait starts and while it is already parked — and assert that a
  stopped daemon actually releases its runtime.

- **A TUI that was refused could not be told why.** `FrontendFrame` is
  internally tagged, and serde cannot represent an internally tagged newtype
  whose payload is a bare string. The refusal frame was written as
  `ProtocolError(String)`: it compiled, and the receiving side matched it
  correctly, but it could never be *serialized*. Both places that tried to send
  it — the daemon's one attempt to explain a protocol failure before closing a
  connection, and the endpoint-classification squatter's refusal — discarded the
  encode error with a `let _ =`, so the frontend saw a bare disconnect instead
  of the reason. The variant is now a struct variant, and a test enumerates
  every `FrontendFrame` variant and asserts each one encodes and decodes, which
  is the guard that was missing.

- **The `gregg` client daemon did not build or run on Windows at all.** The
  Windows half of the local IPC transport (Plan 164) and the Windows branch of
  the config lock had been written but never compiled: the `gregg` client crate
  did not build for `x86_64-pc-windows-msvc`, so the Windows CI job had been red
  since the client daemon landed. The defects were not cosmetic. The pipe stream
  type did not exist, `ConnectNamedPipe` was called with five arguments instead
  of two, the owner-only SDDL out-parameter was confused with a caller-supplied
  string buffer so the pipe would have been created with a garbage security
  descriptor, the handle was closed and then reused, and
  `PIPE_REJECT_REMOTE_CLIENTS` was OR-ed into the open-mode argument instead of
  the pipe-mode argument, which would have left the pipe reachable off-box
  instead of restricting it. Three `windows-sys` feature gates the code depends
  on were not enabled, and `LOCKFILE_EXCLUSIVE_LOCK` was imported from a module
  that does not contain it.

  Two further faults would have kept the daemon from working even once compiled.
  The endpoint name was a Unix socket path, which `CreateNamedPipeW` rejects: a
  named pipe requires a `\\.\pipe\` name, and that namespace is flat, so there is
  no config-adjacent location and no temp-directory fallback on Windows. And the
  accept loop required a non-blocking accept that cannot exist for a synchronous
  pipe on a current-thread runtime, where a blocking wait would freeze polling
  and every attached window at once.

  The transport now names the pipe `\\.\pipe\gregg-client-<id>`, creates each
  instance with the owner-only DACL and a correct descriptor lifetime, parks the
  blocking `ConnectNamedPipe` on the blocking pool, and reads with
  `PeekNamedPipe` so a poll costs no context switch. Endpoint liveness is a
  `WaitNamedPipeW` probe rather than a file-existence check, which would have
  answered "not running" for a daemon that was running. Four Windows tests
  exercise the pipe end to end on CI, including a client that connects *after*
  the accept is already parked and a closed pipe that must report a disconnect.

- **A TUI could fail to attach to a healthy client daemon.** The daemon's
  publication loop wrote a state document as soon as the shared slot changed,
  including before it had answered a newly accepted connection's handshake. A
  frontend identifies its daemon by the *first* frame it receives, so a
  document arriving ahead of the `Hello` was rejected as a refusing daemon and
  the window exited instead of rendering. It needed a publication to land in
  the gap between accepting a connection and reading its handshake, so it
  appeared only under load — as two unrelated cron tests failing in a release
  preflight. The handshake reply already carries the current state, so nothing
  is lost by declining to publish ahead of it.

- **`Ctrl-R` did nothing without an `EggPool` entry.** The config-reload request
  sat behind an `EggPool`-configured early return, so for every configuration
  without an `EggPool` table the one reload boundary the client-daemon
  architecture is built on was silently dead.
- **A TUI attaching to an already-settled daemon was shown stale state.** State
  documents published while no window was attached were discarded by the local
  watch channel, so a window opening later was handed the document from startup
  and then never told anything again, because a quiet fleet publishes no further
  documents. Publications are now stored unconditionally.

### Changed

- **Windows-only Rust is now linted in CI (Plan 169):** the Windows job installs
  the `clippy` component and runs the same full-workspace
  `--all-targets --all-features -- -D warnings` gate that Linux already runs.
  There is still no extra CI job and no extra matrix entry.

  The Windows job had been compiling and testing Windows-only code without ever
  linting it, which is how a pedantic backlog built up unnoticed and how
  two stale `#[allow]` attributes ended up suppressing nothing while hiding two
  real findings underneath them. All twelve backlog findings are now fixed. The
  release binaries are byte-identical, so this is a verification change only.

  The Windows job also now runs the first native tests that start
  `gregg daemon run` at all. They immediately exposed a real defect: on Windows
  the accept wait is parked on the blocking pool, and aborting the accept task
  cannot cancel a blocking job that is already running, so the daemon
  acknowledges a stop request and then hangs instead of exiting. That defect is
  **not fixed here**; it is tracked in Plan 171. The test harness works around
  it explicitly rather than hiding it, and every document wait in the daemon
  tests is now bounded so a future regression fails a test instead of wedging a
  CI job.

- **The ~996 KiB the client-daemon line added to `gregg` is measured and kept
  (Plan 170):** attribution over unstripped paired builds attributes the growth
  to 486,985 bytes of new `clientd` code plus serde surface reached through the
  new fan-out document. Two findings correct how the growth reads: the line
  *shrank* third-party text by 31,684 bytes and cut `run_tui` from 41,854 to
  25,266 bytes, because the TUI no longer owns a polling loop; and the largest
  single module deltas are serde impls that were previously inlined and are now
  outlined, not new logic. No candidate cleared a 64 KiB bar without weakening
  something the architecture depends on, so nothing was changed.

- **`gregg` is now a frontend (Plan 164):** all remote polling for a
  configuration file moved out of the TUI and into a new per-config
  **client daemon**, `gregg daemon run`. Each config gets its own daemon,
  identified by a digest of the normalized config path, reachable on a `0600`
  Unix socket beside that config (a Windows named pipe with an owner-only DACL
  and remote clients rejected). Two configurations never share a daemon, and one
  daemon never serves another config's fleet.

  Opening a second TUI window now costs a socket instead of a second copy of the
  fleet's polling, and closing the last window no longer stops observation: the
  daemon keeps polling with no frontend attached. If the daemon is not running,
  the TUI **reports that and exits** rather than falling back to polling
  directly — a silent fallback would double the request budget exactly when the
  daemon is unhealthy and would hide the failure from you.

  `Ctrl-R` is unchanged as the only config-reload boundary, and there is still
  no filesystem watcher. The reload itself is now performed by the daemon, which
  republishes; an invalid file leaves the last-known-good fleet active and
  surfaces a diagnostic. `gregg add`, `gregg remove`, and `gregg refresh` nudge
  a running daemon to reload, best-effort and silently. The EggPool pane's
  window and activation are now a request the daemon reduces across all
  attached frontends, so two windows converge on one worker state instead of
  racing it.

  New commands: `gregg daemon run`, `gregg daemon status`, `gregg daemon stop`.
  `status` and `stop` are read-only and bounded, and identify the daemon by
  completing the local protocol handshake rather than by looking for a process
  name or a PID file, so they cannot act on something that merely looks
  similar. `run` never forks or self-daemonizes.

  Bare `gregg` now starts the daemon itself when the endpoint is genuinely
  empty, under a per-config launch lock, so a second simultaneous launch still
  produces one daemon. It refuses to spawn over a peer it cannot positively
  identify. An owned daemon older than this `gregg` is rotated to the current
  binary on attach; a daemon *newer* than this one is reported with upgrade
  guidance rather than stopped, because it may be serving a newer window
  elsewhere.

### Added

- **User-scoped client-daemon startup (Plan 165):** `gregg daemon startup
  install|instructions|status|remove`, plus `gregg daemon restart`. `install`
  writes one artifact inside the calling user's own home — a `systemctl --user`
  unit, a `~/Library/LaunchAgents` agent, a current-user Startup-folder entry, or
  a managed user crontab watchdog when user systemd is unavailable. It never
  creates a system service, a `LocalService` SCM entry, or a `sudo` call, and a
  root/Administrator install registers nothing on anyone's behalf.

  Every artifact is named for the config's identity so two configs never
  collide, and Gregg keeps no global registry of active configs. Ownership is
  proven by parsing the entry back — a foreign entry, or one that cannot be
  read at all, is preserved.

- **Client-daemon-aware update and uninstall (Plan 165):** `gregg update`
  identifies a running daemon, prepares and verifies the replacement, and only
  then stops, replaces, and relaunches — reporting partial success with the exact
  retry command rather than hiding it. `gregg uninstall --dry-run` now names the
  startup entry and whether a daemon is running; execution stops only an
  identified owned daemon, removes only a provably owned startup entry, and
  refuses to delete the executable if it cannot confidently stop a running
  daemon.

- **Installer startup registration:** a user-local `gregg` install registers the
  default-config startup entry after the binary is fully acquired and verified,
  and a same-scope replacement transitions a running client daemon the way it
  already transitions `greggd`. A registration failure leaves the binary
  installed and prints `gregg daemon startup instructions`; it is never fatal,
  because bare `gregg` works either way. A system-wide install registers
  nothing.

### Added

- **Scheduler observability API (Plans 162/163):** `greggd` now exposes its
  maintenance scheduler over two additive read-only routes,
  `GET/HEAD /v2/scheduler` and `GET/HEAD /v2/scheduler/history`, kept separate
  from `/v2/status` so an ordinary metrics poll never carries command output.
  The summary reports each configured job's name, schedule, next civil
  occurrence, an explicit live state (`idle`, `waiting_for_slot`, `load_high`,
  `load_unavailable`, `running`), its load-gate decision, pending-since, next
  retry while load-deferred, running-since, and the most recent terminal
  result. The history document returns retained terminal records with
  scheduler-lifetime sequence, timing, delay, coalescing, exit status, and
  bounded stdout/stderr tails. Occurrences that never created a child are
  first-class records: `spawn_failed` and `load_expired` appear with no exit
  code, start time, or duration rather than being silently omitted. A daemon
  with no configured jobs answers `200` with a valid empty document, and a
  pre-feature daemon's `404` means "observability unsupported", not "host
  offline". There is no control plane: these routes are `GET`/`HEAD` only, and
  nothing anywhere can create, edit, start, or cancel a job.

- **Bounded scheduler history and output capture (Plans 162/163):** child
  stdout and stderr are now piped and drained concurrently with the child wait
  instead of being sent to `/dev/null`, so a failing job's diagnostic is
  visible. Only fixed-size tails are retained — 1024 raw bytes per stream,
  published as at most 512 JSON-escaped bytes with independent `truncated`
  flags — so a job that floods output can neither block on a full pipe nor grow
  daemon memory with the bytes it writes. History is entirely memory-only:
  `scheduler_history_limit` (default 5, hard maximum 10, `0` to retain nothing)
  is validated before the listener binds, and a daemon restart clears it with
  no file, database, or replay. `argv` and working directory are never
  published. The listener remains unauthenticated, so documentation now states
  that any principal able to reach it can read scheduler output.

### Fixed

- **Scheduler wall-clock reconciliation (Plan 160):** the maintenance
  scheduler no longer trusts a long civil-time deadline as a single monotonic
  sleep. The event loop now wakes at `min(semantic deadline, now + 60s)` and
  re-reads civil time, so a large forward system-clock adjustment is observed
  within about a minute and skipped occurrences coalesce into at most one
  pending occurrence per job instead of sleeping past them; a backward jump
  cannot launch before the stored civil occurrence is due and never recreates
  an already-consumed occurrence. Load retry, max-wait expiry, and child
  lifecycle stay purely monotonic. Reconciliation wakes are silent and
  perform no telemetry, HTTP, filesystem, or process work; a daemon with no
  configured jobs still spawns no scheduler task.

- **Calendar-impossible maintenance schedules (Plan 158):** a syntactically
  valid cron expression that no Gregorian date can satisfy — for example
  `0 0 31 2 *` — is now rejected as an `InvalidJobs` configuration violation
  during config loading, before the daemon binds its listener or launches
  runtime tasks. Satisfiability is a pure calendar walk over one 400-year
  cycle from a fixed epoch, using the same day-matching semantics as runtime
  scheduling, so it never reads the current clock or the host timezone and
  preserves traditional day-of-month/day-of-week OR behavior (`0 0 31 2 1`
  stays valid because a Monday in February satisfies it). A later
  schedule-arithmetic failure is an internal time-domain error: it now
  propagates through the existing scheduler fatal task boundary instead of
  silently rescheduling the job 366 days out.

- **Scheduler selection and launch cost (Plan 158):** pending selection is an
  allocation-free bounded scan over at most 64 jobs instead of building and
  sorting a candidate vector on every launch decision, the engine borrows
  validated job configuration instead of cloning it per job and per launch,
  and the daemon hands the job list to the scheduler task by move rather than
  a deep copy. Oldest-pending ordering with config-order tie breaking,
  load-gate deferral, one global child slot, max-wait expiry, coalescing, and
  direct-child shutdown are unchanged.

- **EggPool schema-v1 status wire contract (Plan 153):** the optional
  EggPool pane's health reader now matches the JSON EggPool actually
  serializes (`rust/src/operations/status.rs::ProxyStatusSnapshot` at
  `eggstack/eggpool` `299a0b36`). Account counts are read from
  `proxy.routable_accounts` / `proxy.enabled_accounts` instead of the document
  root, and provider identity and observation come from
  `providers[].provider_id` and `providers[].last_observation` instead of the
  never-upstream `id` / `observation`. Plan 152's synthetic fixture validated a
  Gregg-invented shape, so a real provider-bearing status response failed to
  decode while CI stayed green. The bounded contract is now aligned with
  EggPool's own producer limits — 256 provider rows, 96-byte provider IDs, and
  64-byte reason codes (previously 256/64/128) — with exact-limit and
  one-over regressions for each. The status matrix, ceiling split (1 MiB
  status, 16 KiB summary), dual-plane worker, freshness, authentication, and
  rendering are unchanged, and no compatibility alias is added for the
  Gregg-local shape.

### Changed

- **EggFetch 0.2 patch refresh (Plan 147):** `Cargo.lock` now resolves
  published `eggfetch-core 0.2.2` while the `gregg` manifest keeps the lean
  `version = "0.2"`, `default-features = false`, `standard-http1` +
  `tls-rustls` contract. The diff is one package version (three shared
  `windows-sys` requirement edges re-resolved between already-present
  versions; no package added or removed), no application source change was
  needed, the resolved feature graph is unchanged, redirect/retry/proxy/H2/H3
  and the other excluded capabilities stay absent, all transport regressions
  (3xx passthrough, absolute total deadline, body ceilings, typed DNS/refused/
  timeout, Bearer redaction) stay green, and the stripped fat-LTO release
  `gregg` is byte-identical before and after the refresh. `gregg-update`
  remains on external `curl`; Plan 126 is not reopened.

- **EggPool desired-state delivery and worker-state correction (Plan 151):**
  the client no longer publishes bounded `EggPool` commands with `try_send`.
  `AppState` now owns one latest desired state
  (`EggpoolDesiredState`: `active`/`period`/`generation`) published
  synchronously and capacity-free through `EggpoolControl`, so activation,
  period changes, manual refreshes, and deactivation converge on the worker
  without ever blocking terminal input or Systems poll-result handling, and
  without a full queue silently discarding a transition. The worker converges
  on the newest desired state, coalesces states it did not observe
  individually, aborts obsolete in-flight requests, arms the request-relative
  60-second passive deadline only after a request completes, reuses the
  reducer generation for passive refreshes, and stops on `CancellationToken`
  with no queued shutdown command.

### Added

- **Load-aware maintenance scheduler (Plans 156-157):** `greggd` can run
  optional five-field local cron jobs as direct argv commands, with cached
  1m/5m/15m load gates, bounded deferral/coalescing, one global child slot,
  same-principal execution, and bounded direct-child shutdown. Existing configs
  remain job-free by default; no remote control API or persistent replay is
  added.

- **EggPool service-health status plane (Plan 152):** the optional `EggPool`
  pane now also reads EggPool's schema-version-1 authenticated
  `GET /api/status` beside the existing `/api/stats/summary` metrics. The pane
  header gains a bounded `Health: ready | degraded | unready | unknown` token
  and the footer may show a compact provider count summary such as
  `Providers: 2 ready · 1 degraded · 1 unavailable`. The four periodized
  metrics, their meaning, and the 16 KiB summary body limit are unchanged; the
  status route has its own 1 MiB ceiling aligned with EggPool's bounded status
  client. Both reads happen concurrently inside the existing worker request, so
  a slow or unavailable status route never delays a valid summary, and
  `AppState` keeps health freshness (`health`, `last_health_success_at`,
  `last_health_attempt_at`, `last_health_error`) separate from summary state —
  a failed health refresh keeps the previous snapshot but marks it stale, and a
  summary failure never hides valid service health. An older `EggPool` without
  the route keeps working and reports health as unsupported, a public dashboard
  with no configured key reports health as auth required, and an unknown future
  schema version degrades to an explicit unsupported health state. No provider
  probe, quota use, account/model drill-down, new credential field, or
  `EggPool`/`greggd`/`gregg-protocol` change is introduced.

### Removed

- **EggPool `Busy` worker state (Plan 151):** the `EggpoolStatus` reducer/UI
  enum is renamed to `EggpoolWorkerState` (`Idle`, `Refreshing`,
  `WorkerUnavailable`) so local worker lifecycle cannot be confused with
  `EggPool` service health. The `Busy` variant and its "worker busy" pane
  text existed only because a full command queue dropped the requested
  transition, and are removed. Summary transport, authentication, periods,
  body limit, metric semantics, and rendering are unchanged.

## [1.0.15] - 2026-10-01

### Changed

- **EggServe 0.4 direct-server adoption (Plan 148):** `greggd` now resolves
  `eggserve-server 0.4.0` with `eggserve-primitives 0.2.2`. Existing routes,
  response framing, shared cached bodies, split server supervision, and
  explicit runtime limits (including the 300-second connection lifetime and
  1000-request cap) remain unchanged; no protocol behavior changed.

### Fixed

- **Second audit corrective pass (bugs.md, 2026-09-30):** every actionable
  finding from the workspace-wide logic/robustness audit is fixed.
  Protocol: health envelopes enforce a total state/category allowlist
  (`warming` carries only `warming`; `failed` carries only
  `collector_failure`/`not_serving`; `ready` carries neither category nor
  message), health `message` is bounded by `MAX_HEALTH_MESSAGE_BYTES` and
  NUL-free, `HealthResponse::try_ready`/`HealthResponseV2::try_ready`
  validate the snapshot before advertising `Ready` (the daemon serves a
  `failed` envelope instead of a `200` for a snapshot that fails
  validation), `drives: Some([])` makes any disk-I/O `drive_name` dangling
  (only `drives: None` skips the check), drive names reject blank and
  NUL-padded labels, duplicate detection is hash-set based (linear over an
  attacker-sized collection) for drives, disk-I/O devices, and network
  interfaces, disk/net IDs and names reject whitespace-only values, link
  capacity and CPU frequency are upper-bounded (new
  `CapacityExceedsMaximum`/`CpuFrequencyExceedsMaximum` kinds, 36
  `ViolationKindV2` total), an all-zero `drives[].total_bytes` is a valid
  empty/placeholder volume unless it also claims non-zero content, and
  `test_support` gains a `MacosSnapshotV2Builder`. Host: Linux memory
  clamps `MemAvailable` to `MemTotal` (matching macOS/Windows/FreeBSD) so a
  transient kernel counter race no longer fails the whole `HostSample`, the
  Windows `RawCpuTimes::total()` doc records that `GetSystemTimes` kernel
  time already includes idle (a double-count "fix" would collapse
  `usage_pct`), the FreeBSD `ifmib_record` production decoder is now unit
  tested (the divergent dead `normalize_ifmib_row` helper is gone), and the
  Windows collector collapses case-variant volume roots so `C:\` and `c:\`
  cannot reach the wire as two drives. Daemon: the control-socket cleanup
  guard is owned by the caller so the signal-driven shutdown path removes
  the socket instead of relying on stale detection, control-socket identity
  is canonicalized once per `run`/`stop` (removing a TOCTOU flip and the
  `cwd`/symlink `<id>` divergence), `greggd stop` no longer loads the
  config (identity is path-only, so a corrupt config cannot block stopping
  a running daemon), systemd/launchd installs render the selected
  `--config` path into `ExecStart`/`ProgramArguments` instead of a
  hardcoded standard path, `uninstall`'s post-teardown endpoint re-probe
  fails closed when the config cannot be loaded, Windows update re-queries
  the SCM registration inside `quiesce_windows_service_if_needed` so an
  owned-to-foreign transition cannot stop a foreign service, the ready-health
  memo revalidates the published state after its awaited init so a
  concurrent failure transition cannot produce a transient 200/503
  divergence, the bounded `/v2/healthz` fetch has a total deadline (a
  slow-loris peer could otherwise hold `status`/`croncheck`/`update`/
  `uninstall` for ~48s), the uninstall writability probe uses a per-process
  counter so two threads cannot collide on the same probe name, and
  `greggd::cli::dispatch` is deprecated with an explicit contract (it cannot
  distinguish an explicit `--config` from the implicit default).
  Client: config system `host` rejects credentials, fragments, whitespace,
  and control bytes, and system `name` matches `endpoint::validate_name`
  (no surrounding whitespace, no control bytes, no `@ : /`) via the new
  `InvalidName` violation, so a hand-edited config fails validation instead
  of surfacing as a later `NetworkError`; the normal and condensed drive
  detail heights share `valid_drive_detail_count` (a legal `drives: None` +
  `disk_io: Some(..)` rendered header and I/O total in condensed but
  nothing in normal); `CondensedRenderKey` carries the port so a port-only
  config edit cannot reuse a stale memoized row, with the host-only online
  label documented as a deliberate choice; EggPool endpoint parsing accepts
  IPv6 zone IDs exactly like `gregg add`; the Unix `flock` path inspects
  `errno` so `EBADF`/`EINVAL`/`ENOTSUP` surface as `Io` instead of a
  5-second `LockTimeout`; and public `ConfigStore::write` takes the same
  in-process mutex and file lock as `mutate` instead of bypassing the
  documented cross-process protocol. Updater: `curl`/`cargo` discovery
  probes run under the shared bounded child runner (5s) so an earlier-`PATH`
  shim that hangs cannot block `resolve_plan`/`prepare_candidate` forever,
  and a 2xx download is rejected unless the staged file is within
  `MAX_DOWNLOAD_BYTES`. Installers: `--version` now accepts only strict
  `X.Y.Z` (prerelease/build metadata is rejected at argument parse instead
  of after download and staging), and the Cargo fallback staging root is
  removed by an `EXIT` trap so a `die` in candidate verification no longer
  leaks `/tmp/tmp.XXX/cargo-root`.

- **Audit corrective pass (bugs.md, 2026-09-30):** minimal behavior-preserving
  fixes across workspace drift, validation, daemon, client, updater, and
  installers. Workspace/docs now name all five crates
  (`gregg-protocol → gregg-update → gregg-host → greggd → gregg`), the
  `gregg-host` native-telemetry role/boundary, FreeBSD backend, corrected
  unsafe allowlist, and `gregg-host` coverage in version/package/release
  checks plus macOS CI. Protocol v2 validation now caps per-device
  disk-I/O and per-interface network rates at `MAX_RATE_BYTES_PER_SEC`,
  rejects duplicate drive names, and rejects dangling `drive_name`
  associations (new `DuplicateDriveName`/`UnknownDriveAssociation` kinds;
  36 `ViolationKindV2` total; see the second audit entry below for the
  capacity/frequency upper bounds). Daemon: `systemctl is-system-running` probe
  requires exit success, control-socket client timeout raised above the
  server 1s window with timeout errors surfaced, and dir-sync
  `PermissionDenied` now warns on all platforms. Client: endpoint names
  reject surrounding whitespace and `@`/`:`/`/` (closing the
  `user:pass@host:port` nickname bypass), `PATH` lookup skips empty
  components instead of probing `.`, EggPool `--replace` preserves the
  stable ID, stale-temp cleanup is age-gated (5 min) and best-effort, and
  the empty-config hint requires an explicit port. Updater: `curl`/`cargo`
  discovery requires exit success, `probe_http_code` uses `-fsSL`,
  `prepare_candidate` returns an error instead of `expect`, staging docs
  match canonicalization, and checksum tests use exclusive `TempDir`s.
  Installers verify lowercase 64-hex checksums and exact
  `"<program> X.Y.Z"` versions, bound `curl` with `--max-time`/
  `--max-filesize`/User-Agent, follow redirects on 404 probes, expand the
  cleanup trap immediately, and use unpredictable temp names with
  `-TimeoutSec` on Windows. Flaky drive-cache tests use a 10s deadline;
  `check-local.ps1` runs `--all-targets --all-features`.

- **Linux CPU-set cardinality boundary** (`gregg-host`, Plan 146): the
  CPU-list parser now bounds the number of *distinct CPU identities* Gregg
  materializes (8192, the same value as its logical-core safety target)
  instead of clamping numeric CPU IDs to 8192. Linux CPU numbers are
  identities and need not be contiguous, so sparse lists such as
  `0,2,10000` keep their exact members; a list that would exceed the
  member bound — including a single oversized range — now fails the whole
  list instead of silently truncating to a partial set, and reversed
  ranges, values beyond the CPU-index width, and empty/malformed input
  still fail without panic or unbounded allocation. The legacy
  `affected_cpus -> related_cpus -> bounded default` CPUFreq fallback
  derives its weight from the same parsed identity set instead of
  reinterpreting ranges as dense IDs bounded by the host core count. The
  Plan-145 identity-set type and intersection calculation are now private
  to the Linux source module, the unused `live_weight()` helper and the
  public `last_online()` inspection accessor are removed, and in-module
  tests read cache state directly. `CpuFreqStructuralCache`,
  `ProcSource::cpu_frequency_hz_with_cache`, the Plan-141/145 steady-state
  read pattern, weighted-average arithmetic, sample cadence, protocol and
  capability surfaces, and Rust 1.89 MSRV are unchanged. This is
  correctness hardening of an internal boundary; no user-visible
  telemetry change is claimed, and no dependency, unsafe code, or new
  topology read is introduced.

- **Native telemetry closure and FreeBSD network-evidence correction (Plan
  137):** the FreeBSD `native_loopback_traffic_advances_lo_counters`
  qualification is now fail-closed — base-system ping must execute and
  succeed, the same stable loopback identity is required before and after
  traffic, counters must be monotonic, and both RX and TX must advance
  strictly (one bounded visibility re-read only). Loopback evidence is
  described truthfully as counter-activity, not RX/TX field-order proof:
  the smoke proves the mapped ifmib byte counters are live under known
  traffic while ordering rests on the field-for-field `struct if_data`
  ABI mapping. `gregg-host` package metadata now names FreeBSD explicitly
  (NetBSD/OpenBSD remain deferred), all 67 Plans 132-136 acceptance boxes
  are reconciled with closure evidence, and the duplicated registry entry
  is removed. No collector, protocol, readiness, cadence, or
  production-subprocess changes.

- **Installer profile detection (Plan 131):** the user-local bootstrap now
  suppresses its managed PATH block only for a recognizable active PATH
  integration (`$HOME/.local/bin`, `${HOME}/.local/bin`, `~/.local/bin`, or
  the expanded home as a discrete `PATH`/`path` entry) or an intact
  Gregg-managed block (exact marker plus an active functional PATH line).
  Comments, commented-out assignments, prose, `echo`/`printf`, aliases,
  functions, non-`PATH` variables, subpaths, and longer names no longer
  suppress persistence; exotic shell constructs fail closed by appending the
  safe managed block. Destinations, profile selection, opt-out,
  system-install, activation, `both`, update/uninstall, and platform behavior
  from Plan 130 are unchanged.

### Added

- **Native host telemetry crate and FreeBSD foundation (Plans 132-136):**
  native Linux/macOS/Windows acquisition moved without semantic change into
  the protocol-neutral, runtime-neutral `gregg-host` workspace crate
  (`HostSample`/`HostIdentity`/`HostCapabilities`/`CollectionLimits`,
  shared counter baselines, drive normalization, slow-probe isolation, and
  the per-OS collectors with their source seams and mocks; only `std` plus
  target-scoped `libc`, `thiserror`, and `tracing`). `greggd` consumes it
  through a `greggd::collector` compatibility facade preserving all public
  paths, `CollectedMetrics`/v1/v2 conversion, readiness mapping, Windows
  v2-only behavior, and exact collection limits; the stripped release
  `greggd` binary is unchanged (2,629,008 bytes before/after). FreeBSD
  joins as the first post-extraction backend (`kern.cp_time` CPU,
  `getloadavg`, `hw.physmem`/VM-counter memory, `getmntinfo` filesystems,
  `libdevstat` disk I/O, `ifmib` network; swap/frequency truthfully
  unsupported with recorded follow-ups; no generic Unix/BSD abstraction).
  No protocol, cadence, readiness, privilege, or external-command changes;
  NetBSD/OpenBSD remain explicit future backends.


- **User-local installer PATH activation (Plan 130):** non-root Unix
  bootstrap installs whose `$HOME/.local/bin` destination is absent from the
  current `PATH` now persist it to the user shell profile for future shells
  by default (zsh `${ZDOTDIR:-$HOME}/.zshrc`, bash `~/.bashrc` on Linux and
  login-aware `~/.bash_profile` / `~/.bash_login` / `~/.profile` on macOS;
  idempotent append-only with a recognizable marker, never evaluated or
  sourced, `--no-shell-profile` opts out, `both` integrates once). Output
  distinguishes current-shell availability from future-shell persistence and
  always prints the exact `export PATH="$HOME/.local/bin:$PATH"` for the
  current shell; the documented rootless client quick-install now carries a
  trailing parent-shell `export` (a piped child installer cannot change its
  parent's environment). System installs never touch shell profiles, profile
  failures never roll back a successful binary install, and uninstall never
  removes the generic `$HOME/.local/bin` PATH entry. Same-scope replacement,
  foreign-destination refusal, staged Cargo fallback, daemon finalization,
  update, and uninstall ownership from Plans 112-116 are unchanged.

### Changed

- **Daemon HTTP runtime consolidation (Plan 127):** `greggd` now uses EggServe 0.2.1's direct HTTP/1 server and canonical request/response types. The existing routes, schemas, staleness decisions, cached status bytes, and wire headers remain covered by transport-neutral and raw-wire tests. EggServe's independent server-completion signal remains under daemon supervision, and total connection lifetime is disabled for pooled clients. No protocol change.

- **eggfetch 0.2 lean client adoption (Plan 125):** the `gregg` client now
  builds on published `eggfetch-core 0.2.0` with the unchanged lean
  `standard-http1` + `tls-rustls` profile. No polling, redirect, timeout,
  body-limit, auth, or outcome behavior changes; the stripped fat-LTO
  release `gregg` binary is unchanged at 3,740,592 bytes.

- **Updater transport experiment closed (Plan 126, RETAIN CURL):** an
  implementation-quality in-process eggfetch 0.2 updater candidate was
  built, fixture-tested, measured, and reverted — the parity-complete
  feature set doubled the stripped `greggd` binary, so external `curl`
  remains the update transport with no behavior change. Kept from the
  experiment: a unit-tested crates.io response parser and hermetic
  end-to-end `curl` regression fixtures (redirect, exact-404 fallback,
  hard failures, metadata capture/bounds).

### Performance

- **Bounded runtime optimization campaign (Plans 121-123):** daemon samples
  now move v2-only telemetry through one conversion and preserve `Arc`
  publication; the client moves owned poll payloads through normalization,
  uses O(1)-average renderer memo lookup and O(N) ordered reducer matching,
  redraws only after visible changes, and preformats condensed rows once per
  frame. Daemon v1/v2 status JSON is serialized once per successful
  publication and served from shared immutable bytes while staleness and
  health decisions remain request-time.

- **Ready-health single-flight memoization** (`greggd`, Plan 144): the
  Plan-139 per-publication ready-health memo now uses
  `Arc<tokio::sync::OnceCell<Bytes>>` instead of `Option<Bytes>`, so
  concurrent first requests serialize the ready-health body exactly once
  per immutable publication (previously up to N serializations for N
  concurrent first requests). The `PublishedState` read guard is dropped
  before the cell is initialized, a failed serialization leaves the cell
  retryable rather than memoizing the error, fresh cells replace any
  prior cell on every publication/warming/failure transition, and an
  in-flight old-publication init cannot populate the new publication's
  cell. Plan-124 stale/failure envelope, `BorrowedReadyHealthV1`/`V2`,
  Plan-123 status caching, public `ServerState` API, and `EggServe`
  runtime policy are unchanged. No protocol, route, or wire change.

- **Linux CPUFreq online-membership freshness** (`gregg-host`, Plan 145):
  the Plan-141 `affected_cpus` weight cache now stores structural
  `related_cpus` identity sets per policy, refreshed only when the
  policy directory set changes. Each sample reads the global
  `/sys/devices/system/cpu/online` set once and intersects it with each
  cached `related_cpus` to derive the live policy weight, so
  same-cardinality online membership swaps and zero-online policies are
  visible immediately without re-reading structural membership files.
  The global online source fails closed to the pre-Plan-141 live
  `affected_cpus -> related_cpus` path when unreadable or malformed, and
  the same legacy fallback is used per-policy when one policy's
  `related_cpus` cannot be read/parsed. Plan-141 topology-change
  behavior, current-frequency live reads, public `ProcSource` API,
  macOS/Windows/FreeBSD collectors, protocol/capability surfaces, and
  Rust 1.89 MSRV are unchanged. No new dependencies, no arbitrary TTL
  or periodic refresh, no protocol or wire change.

### Fixed

- **macOS route-message parser correction (Plan 129):** the `NET_RT_IFLIST2`
  walker now validates the common four-byte route-message prefix
  (`msglen`/`version`/`type`) before type discrimination and applies the full
  `if_msghdr2` size requirement only to `RTM_IFINFO2`, so valid shorter
  `RTM_NEWADDR` / `RTM_NEWMADDR2` records no longer truncate enumeration
  before later interfaces. Malformed tails still truncate safely without
  over-reads, and native macOS CI now requires a non-loopback interface in
  both the raw enumeration and the complete v2 payload. No protocol, TUI,
  cadence, readiness, fallback, or IOKit changes.

- **macOS Intel drive/network collector correctness (Plan 128):** filesystem
  enumeration now uses `libc::getmntinfo` with `libc::statfs` so Intel hosts
  receive the correct `INODE64` ABI instead of a private unsuffixed layout;
  network counters prefer `NET_RT_IFLIST2` / `if_msghdr2` 64-bit statistics
  with a correctly typed `getifaddrs` / `if_data` fallback (never an
  `if_data64` cast). Legacy wraps re-baseline without spikes, optional
  drive/network/disk-I/O failures use bounded transition logging, and native
  macOS CI now proves nonempty v2 drive/network telemetry on arm64 and Intel.
  No protocol, TUI, cadence, or readiness changes.

- **Stale status compatibility (Plan 124):** v1/v2 status responses that
  cross the collector-failure threshold now preserve the latest stored failure
  diagnostic, matching the health endpoints; ready snapshots that become stale
  only by age retain the explicit `cached snapshot is stale` message.

## [1.0.14] - 2026-09-19

### Added

- **Per-system network rows and drive-key mnemonic** (Plan 114): normal-view
  NET rows now appear only for systems whose current snapshot includes network
  telemetry, preserving valid zero-throughput and unknown-capacity data while
  omitting phantom rows for legacy systems. Drive details are now toggled with
  `d`; `n` and `v` retain their existing meanings.

- **Installer upgrade semantics and cross-platform uninstall** (Plan 112):
  rerunning `packaging/install.sh` / `packaging/install.ps1` at the same
  scope now classifies the destination (`absent` / `replace` / `foreign`)
  via the existing binary's `version` command, reports first install vs
  identified replacement with versions, and refuses to overwrite a foreign
  executable instead of silently treating it as an upgrade. Bootstrap Cargo
  fallback builds into a private staging root and copies only the verified
  binary to the destination, leaving no Cargo ownership metadata behind.
  New `gregg uninstall [--dry-run] [--purge]` and
  `greggd uninstall [--dry-run] [--purge]` remove only the exact invoked
  binary plus the Gregg-owned startup integration actually present
  (systemd unit, launchd plist, managed cron block, or `greggd` SCM
  registration, each discovered independently), preserve configuration by
  default, and never prompt, escalate, recurse into directories, or keep an
  install receipt. Cargo-owned installs delegate on Unix and print the
  exact `cargo uninstall` handoff on Windows. The legacy
  `packaging/uninstall-windows.ps1` is now a thin `greggd uninstall`
  wrapper with no recursive directory deletion.

### Fixed

- **Plan 116 update-lifecycle ownership corrections**: `greggd update` no
  longer uses host-global `startup_state()` for pre-replacement mutation. It
  observes an exact-executable `UpdateLifecycle` after candidate preparation:
  Unix combines systemd/launchd ownership with selected-config health (foreign
  inactive no longer masks a running direct daemon; foreign active using the
  selected config is preserved), Windows revalidates `query_registration()`
  immediately before quiescence (only owned running/start-pending may stop,
  owned stop-pending waits stopped without restart, foreign/unknown and
  `NotInstalled` do zero SCM mutation, owned-to-foreign fails before
  replacement). Only `ManagedRunning`/`DirectRunning` restart via
  `restart_daemon()`; stopped/foreign stay stopped/preserved without
  fabricated restart claims.

- **Plan 115 restart, activation, and elevation corrections**: `greggd restart`
  now mutates only an exact-executable-owned systemd, launchd, or SCM
  registration and fails closed for foreign/unknown ownership. Same-scope
  non-root Unix daemon bootstrap replacements reactivate only a daemon that was
  healthy before replacement, using config-specific `stop` + `croncheck` for
  both prebuilt and staged-Cargo candidates. Shared update/uninstall permission
  hints now say to use an Administrator terminal/PowerShell on Windows instead
  of emitting Unix `sudo` commands.

- **Plan 113 install/uninstall ownership corrections**: daemon startup
  teardown is now bound to the exact invoked executable across systemd,
  launchd, cron, and Windows SCM. Windows SCM discovery preserves full
  service state and parses its launch command fail-closed for ownership;
  unknown registration state blocks mutation. Unix Cargo-owned uninstall now
  completes owned lifecycle work before Cargo removes the package and applies
  `--purge` only after successful Cargo removal. Staged Cargo daemon fallback
  now shares the prebuilt install finalization path on Unix and Windows.

- **Daemon config readability** (`greggd`): system configs such as
  `/etc/gregg/greggd.toml` are now written `0644` (temp file stays `0600`
  during the write) and `startup install --method systemd|launchd` repairs
  older `0600` installs to `0644`/`0755`. Previously an unprivileged
  `greggd croncheck`/`status`/`configprint` failed with
  `Permission denied (os error 13)` for anyone except the daemon user and
  root. The Unix control socket stays `0600`, so `stop` still requires the
  daemon owner or root.
- **Update permission exit code** (`greggd`): `greggd update` permission
  failures now exit `4` (`PermissionDenied`) with the exact
  `sudo <exe> update` rerun hint instead of falling through to exit `3`.
  Already-current invocations need no privilege and still exit `0` for any
  user; verified for both `greggd update` and `gregg update`.

### Changed

- **Client HTTP transport consolidation** (Plan 118): the `gregg` client
  replaces `reqwest 0.12` with feature-minimal `eggfetch-core 0.1.5`
  (`http1` + `tls-rustls`). Systems polling and the EggPool summary client
  keep their separate pools, explicit whole-request deadlines, body caps,
  redirect rejection, protocol negotiation, and stable outcomes while
  deleting Gregg's duplicated transport error and body-limit machinery.
  No polling, scheduler, TUI, protocol, or CLI behavior changes.

- **Lean eggfetch client profile** (Plan 119): the `gregg` client adopts
  published `eggfetch-core 0.1.7` with the lean `standard-http1` +
  `tls-rustls` profile instead of the broad `http1` compatibility profile,
  removes the now-unavailable runtime `follow_redirects(false)`
  configuration while preserving 3xx passthrough, and adopts 0.1.7's
  corrected absolute `Timeout.total` through response-body EOF by mapping
  body-stage typed timeout errors to the existing `Timeout` outcomes.
  Advanced-routing/retry/redirect/Basic/proxy features are absent.
  Stripped release `gregg` shrinks 4,264,912 to 3,740,592 bytes
  (-12.3% versus the Plan 118 record). No polling, scheduler, TUI,
  protocol, or CLI behavior changes.

- **MSRV raised to Rust 1.89** (Plan 117): the workspace `rust-version` moves
  from 1.75 to 1.89 and the CI MSRV job moves with it. The ten
  transitive-only resolver pins (`indexmap`, `instability`,
  `unicode-segmentation`, `idna`, `idna_adapter`, `hyper-rustls`,
  `quinn-proto`, `rustc-hash`, `zeroize`, `thiserror-compat`) are removed;
  genuine direct dependencies keep ordinary ranges (`uuid 1`, `url 2`,
  `reqwest 0.12`). Prebuilt binary users are unaffected; direct
  `cargo install` / Cargo-fallback source builds now require Rust 1.89+.
  No runtime, protocol, TUI, service, or update behavior changes.

## [1.0.13] - 2026-09-12

### Added

- **TUI live-metrics presentation** (Plan 110): the client now shows optional
  CPU clock detail, fleet-aligned NET utilization, disk R/s/W/s and daemon
  aggregate throughput in `e` detail, and independent network/interface detail
  in `n`. Condensed view adds a width-aware NET column; legacy and unsupported
  telemetry remains absent or unavailable without fabricated zeroes.

- **Native live-metrics collection** (Plan 109): greggd now samples current
  CPU frequency where supported, de-duplicated disk throughput, and
  per-interface network throughput/capacity using native Linux, macOS, and
  Windows APIs. Rates use actual monotonic elapsed time and re-warm on reset,
  restart, disappearance, or hotplug; optional source failures do not make
  core daemon readiness fail. Loopback and layered storage are kept out of
  aggregate capacity/accounting where required.
- **Additive live-metrics v2 payloads and client normalization** (Plan 108):
  schema-v2 status responses may now carry optional raw CPU frequency in Hz,
  daemon-selected disk read/write bytes per second, and directional network
  throughput/capacity with bounded device/interface detail. Validation rejects
  zero frequencies/capacities, duplicate or oversized identities, and
  loopback aggregate membership. v1 and pre-feature v2 payloads remain
  compatible; normalized network utilization is full-duplex safe and returns
  unavailable when capacity is absent.
- **Read-only `greggd status`** (Plan 106): composes version, config path,
  canonical bind `host:port`, the bounded `/v2/healthz` classification
  (`ready`/`warming`/`failed`/`unreachable`/`not-gregg`, same probe
  authority as `croncheck`), and detected startup-manager state. Exit `0`
  only when a valid Gregg endpoint answered; never starts, stops,
  restarts, installs, mutates config, or invokes `sudo`.
- **Client offline provenance** (Plan 106): accepted poll failures
  normalize to `OfflineKind`/`OfflineReason` at the poller boundary, travel
  into `AppState`, and render as a stable suffix (`offline (refused)`,
  `offline (http) HTTP 503`) inside the existing row width; recovery
  clears them in the same generation and stale generations never overwrite
  newer state.
- **Release-policy scripts** (Plan 104): `scripts/release-targets.txt`
  (single target table), `scripts/release-preflight.sh` (version/tag/
  registry checks, also runnable locally), `scripts/release-check-assets.sh`
  (staged-asset validation), `scripts/release-install-zig.sh` (shared Zig
  setup); the release workflow now calls them instead of embedding the
  logic, and a unit test fails loudly if the Rust updater constants drift
  from the target table.

### Changed

- **Shared updater crate** (Plan 104): new internal `gregg-update` member
  owns version/target/asset/download/checksum/staging/replacement
  mechanics for both `gregg update` and `greggd update`
  (`UpdateSpec`-parameterized; no service-manager, TUI, EggPool, or
  protocol concepts). Both application updaters are thin adapters;
  `greggd` keeps activation/restart coordination and the
  prepare-before-quiesce rule. All update/install/release user-visible
  behavior is unchanged. Publication order is now
  `gregg-protocol` → `gregg-update` → `greggd` → `gregg`.
- **Module decomposition** (Plan 105): `greggd` startup logic split into
  `src/startup/` (`method`, `process`, `systemd`, `launchd`, `cron`,
  `state`, `install`) and client config split into `src/config/`
  (`model`, `store`, `validation`, `lock`), both behind façades that
  preserve every `crate::startup::X` / `crate::config::X` path.
  Behavior-preserving; no new abstractions.
- **MSRV retained at Rust 1.75** (Plan 105): `cargo check --workspace
  --all-features` passes under 1.75, and a relax experiment proved fresh
  resolution without the compatibility bounds pulls rust-version 1.77–1.88.
  Every compatibility-only pin now has a documented KEEP reason in
  `architecture/workspace.md`.

### Removed

- **`probe_top` diagnostic binary** (Plan 105): the standalone
  connectivity probe with a historical default LAN address is deleted; a
  normal `gregg` build no longer emits it.

### Fixed

- **Bug-audit hardening, fifth pass** (no behavior additions): reject
  unparseable `%{http_code}` on the download success path as `Failed` and
  remove the partial file; delete dead `UpdateError::UnsupportedHost` /
  `ReleaseAssetAbsent` variants; reject absurd aggregate disk/network
  throughput above `MAX_RATE_BYTES_PER_SEC` (1 TiB/s) as
  `RateExceedsMaximum` instead of displaying it; remove the all-false
  `MetricCapabilitiesV2::default()` footgun in favor of explicit `new()`;
  disambiguate the update permission-probe name with a per-process sequence
  plus attempt index; use `u64::try_from` for the bounded pipe cap;
  document the hung-`statvfs` drive-refresh and `sample_once` non-publish
  behavior; enforce `gregg refresh` 1..=3600 at clap parse time; run
  `cargo test --workspace --all-targets --all-features` in default
  `check-local.sh`; use 104-byte `UNIX_PATH_MAX` on macOS.
- **Bug-audit hardening, fourth pass** (no behavior additions): fix
  `install-linux.sh` rejecting correct ARM64 binaries (`file` matched bare
  `ARM` before `aarch64`); check `sc.exe config obj=` / `failure` exit codes
  in both Windows installers and wrap `WaitForStatus` timeouts in
  gregg-style errors; match any `-pc-windows-msvc` target for the `.exe`
  asset suffix; avoid double `VersionLookup` nesting on crates.io fetch
  failures; document the scheduler batch bound as intentional backpressure,
  the `page_size` zero path, the drive vs disk-I/O NUL policy, the
  `HealthResponse` constructor-only contract, the bounded-command poll
  interval, the EggPool two-clock coupling, and the `base_height` fallback
  rule; share the oversized-body fragment const; assert HTTP 2xx on the
  download success path; disambiguate atomic-write temp names with a
  process-wide counter; run explicit cleanup before releasing the
  check-local trap; name the drive-refresh worker thread; return a
  timeout/reset-classified detail with `StopOutcome::Uncertain`; hoist the
  redundant `current_exe()` call, the eager `expected` alloc, the
  `unwrap_or_default` invariant, and the sampler lock scope.
- **Bug-audit hardening, third pass** (no behavior additions): reject `ready`
  v1 health responses that carry a `category` (matching v2); propagate
  staged-candidate chmod failures instead of discarding them; bound curl
  metadata/probe/download children with wall-clock timeouts, stream the
  crates.io body through a 256 KiB cap, and cap release downloads with
  `--max-filesize`; reject ambiguous bare `::1:8080` (use `[::1]:8080`)
  instead of silently taking the default port; skip (rather than abort on)
  overflowing disk-sector lines; probe Windows admin via SCM
  create-service access; verify the pinned Zig tarball SHA-256 before
  extraction; check all four crates in release preflight; run the full test
  suite in the MSRV job; lexically normalize `..` in the current-exe
  symlink fallback.
- **Bug-audit hardening, second pass** (no behavior additions): saturate
  instead of dropping the disk/network payload on aggregate overflow (all
  three collectors); return an error instead of panicking in endpoint
  colon-splitting and render an explicit `<invalid>` display placeholder
  for hosts that fail normalization; reject SemVer-illegal leading zeroes
  in stable-version parsing; map curl `000` probe codes to `None` and
  remove partial download residue on failure; record attempt time and
  provenance for cancelled polls; retry the update permission probe on
  name collision; require exact candidate `version` identity up to one
  trailing newline.

- **Bug-audit hardening** (no behavior additions): reject `ready` v2 health
  responses that carry a `category` (`gregg-protocol`); treat
  from-the-future snapshots as stale when the clock jumps backward
  (`greggd` server); create `startup install` temp files with `0600` and
  propagate file/dir sync errors; classify asset-download failures from the
  HTTP code captured in the same `curl` invocation instead of a second
  probe request plus `"404"` substring sniffing (also fixes the
  Unix-only `/dev/null` probe target on Windows); use `saturating_add`
  for the client response cap; bound `id`/`useradd`/`chown` in systemd
  install with the existing 10s manager runner; remove the
  `path_exists`+`read_dir` TOCTOU in the Linux disk-slave check; check the
  croncheck response cap before buffering; replace `expect` on Windows
  FFI size conversions and the macOS `CString` literal with fallible
  `SourceUnavailable`/`Parse` paths; route Windows memory/commit
  percentages through the shared clamped helper.

- **Linux CPUFreq policy-list compatibility** (`greggd`, Plan 111): accept
  the whitespace-separated CPU membership form emitted by some kernels in
  addition to ranges and comma-separated lists, so readable
  `scaling_cur_freq` values are not omitted on those hosts. Current CPU
  frequency remains optional and is reported only when a supported source is
  readable.

- **Sustained workload v2 accounting** (`gregg`, Plan 111): count validated
  `OnlineV2` results alongside v1 results so the compatibility smoke accepts
  current schema-v2 fixtures.

- **Bootstrap installer requires bash, docs use it everywhere** (`install.sh`,
  packaging, workflows, docs): every documented pipe-to-shell installer
  command used `sh -s --`, but `install.sh` is a bash script (`set -o
  pipefail`, `[[ ]]`) and fails on Debian/Ubuntu where `sh` is dash (`set:
  Illegal option -o pipefail`, verified end-to-end against the published
  `v1.0.12` assets). All live commands now pipe to `bash -s --`
  (`packaging/install.sh` header/usage output, the unprivileged-`greggd`
  system-wide follow-up hint, `packaging/README.md`,
  `packaging/install-linux.sh`, `packaging/install-macos.sh`,
  `release-binaries.yml` draft-release notes, `RELEASING.md`, the
  `release-process` skill, both crate READMEs, and the root README).
  `install.sh` now fails fast with a clear rerun command when executed
  without bash instead of a cryptic `set` error. The already-published
  `v1.0.12` `install.sh` asset runs unchanged under `bash`, so the corrected
  commands work against the current release.
- **README is a quickstart again** (root `README.md`, `docs/`): removed
  planning artifacts (forward-looking "source-only until a binary-bearing
  release" wording now that `v1.0.12` ships all ten binaries plus both
  installers, release-architecture internals, and scheduler/control-socket
  implementation detail). The README now covers install → startup → add
  endpoints → launch with the verified installer commands first and Cargo
  as the documented fallback, keeps a compact supported-targets table, and
  links new `docs/installation.md`, `docs/daemon.md`, `docs/client.md`,
  `docs/display.md`, `docs/api.md`, and `docs/development.md` (local-build
  and operator-managed paths moved there).

## [1.0.12] - 2026-09-03

### Added

- **Prebuilt binaries and bootstrap installers** (`gregg`, `greggd`, packaging): release-only workflow builds both binaries for Linux x86_64/aarch64 (glibc 2.17 via `cargo-zigbuild` + Zig), macOS Intel/ARM64, and Windows x86_64, verifies `version`/`--help` and a loopback daemon smoke before hashing, and assembles a draft GitHub Release with stable `gregg-<target>`/`greggd-<target>[.exe]` assets plus `<asset>.sha256`, `install.sh`, and `install.ps1` (Plan 099). Unix `packaging/install.sh` and Windows `packaging/install.ps1` are binary-first with Cargo fallback for `armv7l`/unknown hosts, detect the current OS/arch, download the matching asset and checksum from `https://github.com/eggstack/gregg/releases/...`, verify SHA-256 and candidate version, install to `/usr/local/bin` (root) or `$HOME/.local/bin` (`%ProgramFiles%\Gregg` vs `%LOCALAPPDATA%\Gregg`), warn when the destination is not on `PATH`, and never silently invoke `sudo`. Linux ARM64 explicitly covers ordinary 64-bit Raspberry Pi/Le Potato images; ARMv7 remains source-build only. macOS binaries are unsigned.

- **greggd startup installation and restart** (`greggd`, packaging, `install.sh`, `install.ps1`): `greggd startup install` (`auto` default; `--method systemd|launchd|cron` explicit) installs the canonical systemd unit (`/usr/local/bin/greggd` + `/etc/gregg/greggd.toml` + `greggd` user/group + `/etc/systemd/system/greggd.service`, atomic, `daemon-reload` + `enable` + `start`/`restart`) or launchd plist (`/Library/LaunchDaemons/com.eggstack.greggd.plist`) or an idempotent `# greggd managed watchdog` crontab block (`@reboot` + `* * * * *` `croncheck`, shell-quoted, preserves unrelated entries, never edits `/var/spool/cron`). `startup instructions` is read-only and prints exact commands/paths for the detected or specified method. `greggd restart` is manager-aware (Windows SCM, systemd `systemctl restart greggd`, launchd `launchctl kickstart -k`, otherwise `stop` + detached `run` via the same primitive as `croncheck`) and factored for `update` reuse via `startup_state()`. An identified systemd/launchd host never silently falls back to cron on permission failure; the exact `sudo <exe> startup install --method <...>` is printed and `PermissionDenied` is returned. No internal `sudo`, no competing supervisor, no PID-file or process-scan fallback. Unix `install.sh` now delegates to `greggd startup install` after placing the binary; Windows `install.ps1` remains the single canonical SCM registration (Plan 100).

- **Binary-first self-update** (`gregg`, `greggd`, `startup`): `gregg update` and `greggd update` query the latest stable `gregg`/`greggd` crate on crates.io (`max_stable_version` via `curl` with Gregg User-Agent, SemVer-safe `MAJOR.MINOR.PATCH` compare, `env!("CARGO_PKG_VERSION")` is local version, GitHub `latest` never authoritative), download the exact `vX.Y.Z` GitHub Release asset for the current host (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc[.exe]`) plus `.sha256`, verify SHA-256 via `sha2` crate and candidate `version` before any replacement, stage to a private temp dir, then atomically replace the current executable (`self-replace` 1.5.0 / Rust 1.63 via `tempfile`, same-filesystem rename on Unix, running-image on Windows, preserves symlink target, never overwrites symlink file, `current_exe()` derived destination, permission probe before any `greggd` shutdown with `sudo <exe> update` message). Only HTTP 404 permits `cargo install --locked --version "=X.Y.Z" --root <temp>` staged then verified then same replacement path; checksum/version mismatch, transport failure, or 5xx never fall back. `greggd` preserves config/registration and restarts only when running/managed (systemd active, launchd loaded, SCM running, or direct/cron running via health probe), stopped services remain stopped, not-running not started, successful replacement + failed restart is `UpdatedButRestartFailed` with installed version and exact restart command and nonzero exit; no background checks, TUI notifications, package-manager, or internal `sudo` (Plan 101).

### Fixed

- **Update/restart release-readiness corrections** (`gregg`, `greggd`): updater
  candidates now use exclusive private staging and real child-process timeout
  cleanup; `greggd` prepares and verifies candidates before stopping a running
  Windows service, direct/cron restart proves endpoint absence and health
  readiness, and systemd/launchd commands are bounded with preserved stderr.
  Installation docs now identify Cargo as the working path until the first
  binary-bearing release (Plan 102).

- **Supplied bugs audit corrections** (`gregg`, `greggd`): production client,
  input, and TOML setup failures now propagate as errors; Unix temporary config
  files reject symlinks; stale temporary files are reaped and reported;
  scheduler reload delivery no longer blocks the TUI event loop; endpoint,
  state, display truncation, and collector percentage handling are consistent;
  and control-socket publication cannot replace a concurrent path.

- **Supplied bugs audit corrections** (`gregg`, `greggd`): daemon config
  directory creation is race-safe and preserves existing modes, drive refresh
  panics retry with bounded backoff, endpoint equivalence and fallback latency
  are canonical and complete, temporary resolver failures and EggPool URL
  errors are classified accurately, and Unix editor discovery honors the
  effective execute check.

- **Remaining supplied bugs audit findings** (`gregg-protocol`, `gregg`,
  `greggd`): non-ready health envelopes require categories in both schema
  versions, whitespace-only identities and empty IPv6 zones are rejected,
  invalid hosts no longer create duplicate diagnostics, scheduler and sampler
  failure paths remain typed, and pre-epoch clocks cannot publish invalid
  timestamps or serve age-uncheckable cached data as fresh. The flaky drive
  refresh regression test now waits with a bounded wall-clock deadline.

- **Remaining supplied bugs audit findings** (`gregg`, `greggd`): listener and
  lock-directory failures propagate through library boundaries, bracketed
  endpoint hosts are validated as IPv6, URL construction no longer masks host
  normalization errors, DNS classification uses typed connection evidence,
  drive-refresh panics are contained, and synchronous CLI config mutations run
  outside the Tokio runtime.

- **Remaining 2026-08-31 audit findings** (`gregg`, `greggd`): direct polling
  intervals and drive refresh delivery are bounded, endpoint/config host
  normalization and DNS diagnostics are consistent, malformed warm samples
  fail safely, config renames sync their parent directory on Windows, and
  large byte values format without unit-boundary rounding artifacts.

- **Remaining 2026-08-31 audit findings** (`gregg`, `greggd`): IPv6 zone IDs
  are persisted in URL-safe form, resolver lookup errors are classified as DNS
  failures in both client request paths, zero-port validation reflects the
  actual unsigned boundary, and backward clock movement no longer makes a
  future-dated snapshot stale.

- **Long-running daemon stability** (`greggd`): Unix control-client requests are
  bounded, transient listener errors retry with backoff, and control-channel
  degradation no longer becomes a successful daemon shutdown. Optional drive
  capacity refresh now runs in one bounded collector-owned worker at a slower
  cadence, retaining last-known-good data without blocking core snapshots or
  runtime shutdown. Linux excludes `autofs` and generic FUSE filesystems from
  drive probing.
- **Conservative `croncheck` identity** (`greggd`): the watchdog now validates a
  bounded `/v2/healthz` response, treats Ready/Warming/Failed Gregg health as
  running, starts only after connection refusal, and reports ambiguous occupied
  endpoints without spawning a competing daemon.

### Fixed

- **Remaining 2026-08-27 audit findings** (`gregg-protocol`, `gregg`,
  `greggd`): configuration metadata errors are no longer treated as missing,
  client request timeouts are bounded to 100–60,000 ms, v2 capability objects
  require all four explicit flags, identity fields are bounded to 512 UTF-8
  bytes, failed v1 health responses require a category, DNS classification no
  longer relies on display strings, EggPool deadlines honor its clock seam,
  endpoint deduplication uses ASCII case folding, and existing daemon config
  directory permissions are preserved.

- **Remaining audit findings** (`gregg`, `greggd`): identity failures no longer
  publish blank snapshots, malformed IPv6 zone IDs are rejected consistently,
  failed Systems config reloads remain last-known-good while showing a TUI
  diagnostic, pre-epoch clocks do not falsely stale cached snapshots, large
  byte ratios use widened arithmetic, daemon names reject control characters,
  and CI-blocking clippy diagnostics are resolved.

- **Audit corrections** (`gregg`, `greggd`): macOS byte percentages now use
  the shared collector normalization helper, non-Unix Ctrl-C listener errors
  return through the runtime error boundary instead of panicking, and a
  duplicate EggPool endpoint reports a dedicated configuration conflict.
- **Stale-snapshot 503 bodies no longer claim `ready`** (`greggd`): when a
  cached snapshot ages past `max_snapshot_age` (or the failure threshold is
  met) while the stored health state still says `ready`, the status and
  health handlers substitute a `CollectorFailure` failure ("cached snapshot
  is stale"), so the JSON body always agrees with the 503 status code.
- **One sampling-task panic no longer permanently degrades the daemon**
  (`greggd`): the collector is shared with the blocking pool behind a
  poisoning-tolerant mutex, so a single panic fails only that cycle and
  later ticks recover the lock and resume collection instead of losing all
  metrics until restart.
- **EggPool TLS errors are no longer misreported as DNS failures** (`gregg`):
  the overly broad `"name"` substring was removed from the fetch-error
  classifier, realigning it with the main poller's DNS classification.
- **Truncated mountinfo escapes keep the mount entry** (`greggd`): an octal
  escape cut off at end of input now contributes its raw characters instead
  of silently dropping the whole drive, and a statvfs result with zero block
  size or overflowing capacity logs a diagnostic before the drive is skipped.
- **Pre-epoch clocks pause publication** (`greggd`): a system clock behind
  the Unix epoch pauses snapshot publication; age-based status checks remain
  conservative without warning on every polling request.
- **Client config writes fsync on every platform** (`gregg`): the temp-file
  `sync_all()` durability barrier is no longer Unix-only; builds on targets
  without a cross-process lock implementation (neither unix nor windows)
  now fail to compile rather than silently locking in-process only.
- **Terminal wrapper fixes** (`gregg`): `into_inner()` returns the wrapped
  ratatui terminal instead of fabricating a fresh one, and `restore()`
  flushes buffered frame state before tearing down global terminal mode.
- **Protocol identity docs match validation** (`gregg-protocol`):
  `SystemIdentity` docs now state that empty values are rejected for every
  field (including `name` and `hostname`) instead of claiming they are
  permitted.

- **Windows transient commit over-commit no longer fails the sample**
  (`greggd`): a commit charge momentarily above the commit limit (pagefile
  resize windows, kernel over-commit before expansion) is clamped to the
  limit with `usage_pct` saturating at 100 % instead of aborting the whole
  collection cycle and losing CPU, memory, and drive metrics.
- **EggPool command dispatch never blocks the TUI event loop** (`gregg`):
  pane commands are queued with `try_send`; a momentarily full bounded
  channel drops the command and surfaces a "worker busy" pane state instead
  of stalling key handling and poll batches behind a slow fetch. A closed
  channel still marks the worker unavailable.
- **Zero-total validation reports the root cause** (`gregg-protocol`): v1/v2
  memory, swap, and v2 commit payloads with zero capacity but nonzero used
  bytes now also report `ZeroNotAllowed` for the total/limit field (alongside
  `UsedExceedsTotal`) so consumers matching on violation kinds see that the
  total must be positive; all-zero metrics remain valid.
- **macOS VM counters widen consistently** (`greggd`): `vm_info64` now uses
  `widen_natural()` like `cpu_load_info`, so unsigned 32-bit Mach counters
  can never sign-extend into huge values.
- **Control-socket read errors are logged** (`greggd`): unexpected client
  read failures in the stop listener warn instead of being silently treated
  as EOF.
- **Unknown mountinfo escape sequences are logged** (`greggd`): a mount entry
  containing an octal escape outside `{040, 011, 012, 134}` is still skipped,
  but the skip is now visible in the log.
- **Control-socket startup permission window closed** (`greggd`): the Unix
  control socket is bound inside a process-private `0700` staging directory
  and atomically renamed into its final path only after the `0600` mode is
  verified, so the inode never exists publicly with umask-derived
  permissions.
- **`greggd stop` distinguishes unexpected failures from "not running"**
  (`greggd`): unexpected I/O conditions (for example a daemon that accepts
  `STOP\n` but never replies) now report an uncertain outcome with a
  warning and exit code `3` instead of silently printing "greggd not
  running" with exit code `0`. Missing and refused sockets remain an
  idempotent not-running success; permission errors still map to exit `4`.
- **Sampler no longer blocks the daemon runtime** (`greggd`): each collection
  cycle now runs on tokio's blocking thread pool, so hosts with many mounts or
  slow network filesystems can stretch `statvfs()` without stalling
  `/v1/status`, `/v2/status`, or `/healthz`.
- **Drive validation covers excess entries** (`gregg-protocol`): payloads above
  `MAX_DRIVE_ENTRIES` still report `TooManyDrives`, but individual violations
  in entries beyond the bound are now also reported for diagnostics.
- **Control-socket cleanup race narrowed** (`greggd`): stale-socket removal
  treats a concurrent unlink between the metadata check and the delete as
  success instead of surfacing a spurious error.
- **Config directory permission failures are logged** (`greggd`): a failed
  `0700` chmod on a freshly created config directory warns instead of being
  silently ignored.
- **EggPool deactivation aborts in-flight fetches** (`gregg`): leaving the pane
  promptly releases the pending request task instead of letting it run to a
  result that would be discarded anyway.
- **Dead EggPool generation assignment removed** (`gregg`):
  `apply_eggpool_result` no longer rewrites a generation that the guard just
  proved equal.

### Changed

- **Drive detail rendering allocates less** (`gregg`): expanded drive rows
  are built directly from drive references instead of cloning each
  eligible drive every frame. Rendering behavior is unchanged.
- **Shared percentage normalization** (`greggd`): v1/v2 swap percentages derive
  from one collector helper, preventing future v1/v2 divergence.
- **Client render/poll allocation reductions** (`gregg`): display order is
  computed once per action and render; normal-view metric rows are memoized
  per system and rebuilt only when a snapshot's content or fleet membership
  changes. Rendering behavior is unchanged.
- **Protocol docs** (`gregg-protocol`): documented the capability-flag
  absence-vs-`false` ambiguity, the accepted absence of a `usage_pct`
  vs byte-count cross-check, and the validate-after-decode requirement for
  health-response snapshots.
- **IPv6 zone-ID endpoints** (`gregg`): `status_url`/`v2_status_url` now bracket
  any colon-containing host per RFC 2732, so IPv6 zone IDs such as
  `fe80::1%eth0` produce valid request URLs.
- **Protocol validation hardening** (`gregg-protocol`): correctness pass over
  v1/v2 violation checks, health constructors, and test-support builders from a
  workspace bug audit.
- **Client and daemon hardening** (`gregg`, `greggd`): workspace audit
  corrections across poller outcome handling, endpoint parsing, state/UI text,
  the EggPool client, daemon configuration validation, control-socket stale
  entry handling, sampler accounting, and collector sources.

## [1.0.11] - 2026-08-19

### Changed

- Bumped all crate versions to 1.0.11.

### Added

- **Dynamic compact metric suffix** (`gregg`): when the longest natural metric
  suffix across the entire online fleet exceeds one quarter of the terminal
  width, every normal-view metric row renders bar-only fleet-wide until the
  terminal widens again; resizing wider restores suffixes dynamically
  (Plan 087).
- **Transient selection highlight** (`gregg`): the reverse-video highlight arms
  on Systems navigation and clears roughly ten seconds later via
  `Action::ClearSelectionHighlight`; persistent logical selection (`selected_id`)
  and `e` drive expansion are unaffected (Plan 087).
- **Header I/O-wait omission** (`gregg`): the normal-header `IO` token is
  omitted entirely when CPU I/O-wait is unsupported or has no real value,
  instead of rendering a placeholder (Plan 087).

### Fixed

- **Fleet-wide metric-row geometry** (`gregg`): one fleet-wide layout keeps the
  opening `[` and closing `]` columns aligned across every online system,
  including while scrolling; the DISK aggregate suffix became
  `<used bytes> / <total bytes>` so the slash denominator matches the
  percentage; expanded drive details share one selected-system table layout and
  condensed headings/values share one column layout (Plan 085).
- **Renderer boundary corrections** (`gregg`): condensed offline/pending rows
  keep their configured nickname or endpoint identity; expanded-drive fit math
  shares structural width constants with the renderer and degrades Compact via
  truncated names before Minimal; mixed `SWP`/`COMMIT` fleets budget suffixes
  against the same structural prefix width (Plan 086).

## [1.0.10] - 2026-08-18

### Changed

- Bumped all crate versions to 1.0.10.
- **`greggd croncheck` is now a watchdog** (`greggd`): the subcommand no
  longer performs a read-only HTTP probe of `/v2/healthz`. It opens a
  bounded TCP connect to the configured local bind address (with wildcards
  normalized to loopback). If a listener accepts the connection, it exits
  silently with status `0`. If nothing is listening, it spawns
  `greggd run` as a detached child (stdin/stdout/stderr closed; on Unix,
  in a new process group so signals sent to croncheck's group do not
  reach the daemon) and exits `0`. This restores the intended semantics
  for cron, Task Scheduler, and other supervisors that have no built-in
  readiness monitoring and need `croncheck` to actually start the daemon
  when it is not running.
- **`greggd croncheck --target HOST:PORT` removed** (`greggd`): the new
  watchdog operates only on the configured local bind. There is no remote
  probe mode; existing callers must drop the flag.
- **`greggd configprint` wildcard resolution** (`greggd`): a wildcard bind host
  resolves to the host's primary local IP (transient UDP `connect()` route
  lookup only) so the printed address is dialable; the wildcard is preserved
  verbatim if resolution fails.
- Crate metadata polish and docs.rs build fixes.

### Fixed

- **Compact TUI geometry and endpoint ergonomics** (`gregg`): shared
  normal-view metric-row geometry with aligned brackets, concise disk aggregate
  text, fresh-launch viewport snap on the first accepted poll batch only,
  explicit-port `gregg add` accepting `nickname@host:port` and HTTP URL forms,
  named versus unnamed offline rendering, and regression tests locking in
  offline-endpoint polling across generations (Plan 083), plus corrective
  closure of `--name` validation parity, renderer-level geometry proof,
  Unicode-aware offline padding, and `default_port` documentation (Plan 084).
- Workspace bug-audit findings across protocol validation, collectors, and
  client code.

## [1.0.9] - 2026-08-17

### Added

- **`probe_top` helper binary** (`gregg`): standalone TCP-connectivity probe
  driven by `PROBE_HOST`/`PROBE_PORT` environment variables; a development
  diagnostic, not part of the product CLI.

### Fixed

- Poller live-probe test coverage against a local fixture server.

## [1.0.8] - 2026-08-15

### Changed

- Bumped all crate versions to 1.0.8.

### Fixed

- **Croncheck target** (`greggd`): `greggd croncheck` now accepts a `--host` and
  `--port` flag to target a remote daemon, instead of only probing the local
  instance.

## [1.0.7] - 2026-08-15

### Changed

- Bumped all crate versions to 1.0.7.

## [1.0.6] - 2026-08-14

### Changed

- Bumped all crate versions to 1.0.6.

## [1.0.5] - 2026-08-12

### Fixed

- **Scheduler endpoint replacement** (`gregg`): Ctrl-R now reliably delivers the
  replacement endpoint through the bounded scheduler command channel and polls
  it immediately, instead of silently diverging state.
- **Client endpoint reload** (`gregg`): Reloaded configs reconcile stable system
  IDs and deliver replacements without losing pending state.
- **README** (`greggd`): Corrected `greggd host` description — it sets the bind
  address, not the display name.

## [1.0.3] - 2026-08-11

### Added

- Bounded mounted-local-filesystem capacity metrics in v2 status responses,
  with aggregate disk usage in the normal TUI and selected-system details in
  both normal and condensed views.
- Condensed fleet view with `h`/`l` (and arrow) view cycling plus `e` drive
  expansion while preserving mixed v1/v2 compatibility.

## [1.0.1] - 2026-07-23

### Fixed

- **launchd stop idempotency** (`greggd`): `greggd stop` now returns success
  when the service is already unloaded, instead of failing with a launchd
  not-found error. This makes stop safe to call unconditionally in scripts
  and automation.
- **Client config permissions** (`gregg`): All atomic configuration writes now
  enforce `0600` (owner read/write only) permissions on the final config file,
  preventing other users from reading endpoint credentials or host lists.
- **Lock-file truncation** (`gregg`): The advisory lock file is no longer
  truncated during acquisition. Previous behavior could silently drop lock
  content on concurrent access; the fix preserves existing file content.
- **Installed daemon loopback verification** (`scripts/verify-installed-daemon.sh`):
  The verifier now accepts an explicit executable, writes the flat daemon TOML
  schema, validates bounded health/status responses, and checks the reaped
  child exit status.

- **macOS FFI** (`greggd`): `mach_host_self()` and `mach_task_self()` are now
  declared as foreign functions instead of being assumed to exist. `HostPort::current()`
  returns `Result` and rejects `MACH_PORT_NULL`. `Drop` releases via
  `mach_task_self()` rather than `MACH_PORT_NULL`. Swap-info length is validated
  before field access.
- **macOS collector** (`greggd`): Added `complete_production_collector_smoke`
  native test verifying CPU iowait is reported as unsupported/null and memory/
  swap metrics are sane.
- **CI** (`.github/workflows/ci.yml`): Explicit `macos-15` (arm64) and
  `macos-15-intel` (x86_64) matrix entries with architecture verification.
- **Resolved port storage** (`gregg`): `cmd_add` now stores the resolved port
  from `EndpointSpec` instead of the parser default, fixing the case where a
  non-default port from a previous config entry was overwritten.
- **Cross-process locking** (`gregg`): Replaced in-process `AdvisoryLock` with
  OS-level `flock` (`FileLockGuard`) so concurrent CLI invocations across
  processes cannot corrupt the config file. Lock timeout is configurable.
- **`port_was_explicit` removal** (`gregg`): Removed `port_was_explicit` from
  `SystemEntry` (the persistence struct). `EndpointSpec` retains it for CLI
  disambiguation.
- **Scheduler** (`gregg`): One trigger (manual refresh or periodic tick) now
  produces exactly one generation — the old fall-through caused double polls on
  Ctrl-R. Closing the refresh channel no longer causes a busy loop. Timer uses
  `tokio::time::interval` with `MissedTickBehavior::Skip` for fixed cadence;
  manual refresh does not reset the periodic schedule.
- **Response size cap** (`gregg`): The body-size check now happens before
  `extend_from_slice`, preventing a single oversized chunk from allocating
  beyond `MAX_RESPONSE_BYTES` (64 KiB).
- **Daemon supervision** (`greggd`): Unexpected clean exit from the HTTP server
  or sampler (without a shutdown signal) is now treated as a failure. State
  updates from the sampler callback are awaited inline (no detached spawns).
  After a shutdown signal, both tasks are joined with a 10-second timeout.
- **launchd state semantics** (`greggd`): `start()` now bootstraps if the
  service is not loaded, kickstarts if loaded but not running, and is a no-op
  if already running. `is_active()` returns true only when the service is
  actually running (not just loaded). Added `ServiceState` enum and `state()`
  method using `launchctl print`.
- **Installer root resolution** (`packaging/`): `install-linux.sh` and
  `install-macos.sh` now resolve the default binary path one level up
  (`$(dirname "$0")/..`) instead of two, matching the `packaging/` directory
  layout.
- **systemd non-root identity** (`packaging/`): The unit file now runs as a
  dedicated `greggd` user/group with `RuntimeDirectory=gregg`. The Linux
  installer creates the system user and sets config ownership.
- **CI package checks** (`.github/workflows/ci.yml`): Removed `--allow-dirty
  --no-verify` from all `cargo package` invocations so packages are verified
  and the working tree must be clean. Added shellcheck step for installer
  scripts.
- **Rust 1.75 dependency resolution** (`gregg`, `greggd`): Added documented
  compatibility-only bounds for the existing CLI, HTTP, URL, UUID, terminal,
  TOML, and crypto dependency graphs where current transitive releases exceed
  the declared MSRV. Fresh package and workspace resolution now stays below
  the edition-2024 and Rust-1.85-only dependency lines while retaining current
  active TLS fixes.

## [1.0.0] - 2026-07-23

### Added

- `gregg-protocol` crate: versioned JSON wire types, metric capabilities,
  identity structures, and snapshot validation for schema version 1.
- `greggd` crate: lightweight Linux and macOS metrics daemon with read-only
  HTTP API (`/`, `/v1/status`, `/healthz`), periodic sampling, graceful
  shutdown, TOML configuration, and native service integration (systemd,
  launchd).
- `gregg` crate: compact keyboard-first terminal monitor with endpoint
  management (`add`, `list`, `remove`, `refresh`, `edit`), bounded concurrent
  polling, application state engine, and Ratatui-based four-row-per-system TUI.
- Native Linux metrics collection from `/proc` (CPU, memory, swap, load,
  identity).
- macOS metrics collection from Mach host statistics and sysctl (CPU, memory,
  swap, load, identity).
- Protocol compatibility fixtures for Linux, macOS, and health responses.
- Supply-chain policy via `cargo-deny`.
- CI pipeline: formatting, clippy, tests, docs, and package validation on
  Linux and macOS.

### Known limitations

- macOS does not expose a Linux-equivalent aggregate CPU I/O-wait state.
  This is reported as unsupported (`iowait_pct: null`) rather than
  fabricated as zero.
- The daemon is designed for private-network use only. It does not provide
  TLS, authentication, rate limiting, or other public-internet hardening.
- Per-process inspection, historical telemetry, alerting, and web dashboards
  are explicitly out of scope for version 1.
