# Gregg plan index

This directory contains Gregg's implementation roadmaps and execution-ready plans.

## Current direction

Gregg remains a small local/LAN system monitor:

- native Linux, macOS, and Windows metric collection;
- a cached read-only JSON daemon API;
- a compact terminal client for a small fleet;
- optional bounded EggPool summary integration;
- local-first verification and manual releases;
- no generalized observability platform, public-internet service, release orchestration, or evidence system.

Local tests remain the primary development path. The `ci.yml` workflow provides Linux checks and native macOS/Windows truth; the tagged `release-binaries.yml` workflow builds five prebuilt targets and assembles a draft GitHub Release from verified binaries. Ordinary CI does not publish crates or publish a release; the release workflow never publishes crates, bumps versions, or creates tags.

## Roadmap status

Plans 066-079 are complete. Plan 076 implemented the Unix runtime/service-manager separation, HTTP `croncheck`, config-only Unix mutation, and explicit version commands. Plan 077 completed the strict bounded status-line correction, negative-path coverage, stale test cleanup, and planning reconciliation.

Plan 078 implemented the stale client endpoint correction at the existing `Ctrl-R` boundary, HTTP URL input convenience for `gregg add`, and read-only `greggd configprint`. Plan 079 then made replacement delivery reliable under bounded command pressure and corrected Plan 078's live-host record without rewriting the original `.183`/`.182` observation.

Plan 080 implementation landed and its mandatory Ubuntu direct lifecycle smoke passed. Post-closure review then found two product defects: Windows foreground `run` referenced a Unix-only control wrapper and the Unix primary control socket was directory-scoped, allowing configs in the same directory to cross-stop. Plan 081 closed those defects plus permission/stale-socket hardening, preserved the valid Plan 080 historical record, demonstrated the corrected Unix one-daemon and two-daemon same-directory stop-isolation smokes, and passed the existing native CI workflow at implementation SHA `59e17551c211df382c6f0219d0d465ef1c198a8a` in run `31813136597`. Current `main` at the subsequent Plan 081 record commit `6fb005b4a469cdd1ea4baf498fe4a18f5858f3be` also passed the existing workflow in run `31813615708`.

Plan 082 completed the final polish pass. It corrected the remaining Unix control-identity edge so different ordinary path spellings of the same existing explicit config file converge, reconciled the remaining Plan 080/081 status/checklist/provenance wording, and passed existing CI run `31841994426` across all five jobs. It did not reopen the daemon lifecycle architecture or add verification infrastructure.

Plan 083 implemented six bounded client UI/CLI corrections: a shared normal-view metric-row geometry (aligned `[` and `]` across CPU/MEM/SWP-or-COMMIT/DISK with one common `bar_width`), concise disk aggregate text without `used` / `avail` words, fresh-launch viewport snap to `display_order[0]` on the first accepted poll batch only, an explicit-port requirement on `gregg add` accepting the ergonomic `nickname@host:port` form, named versus unnamed offline rendering without duplicate host printing, and a regression test locking in continued polling of offline endpoints across generations. The default local check and remote CI run `32094925174` both passed all five jobs (Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.75). Post-closure review identified four narrow corrective items; completed Plan 084 closes them without reopening the client architecture, with CI run `32100189772` green across all five jobs.

Plan 085 corrects four narrow client-rendering defects that survived the Plan 083/084 follow-ups without reopening the daemon, protocol, scheduler, or release architecture: a fleet-wide (not block-local) normal-view metric geometry so opening `[` and closing `]` columns line up across devices, the DISK slash denominator switched from `available_bytes` to `total_bytes` so the percentage and the slash use the same number, a shared selected-system drive-detail table layout so expanded mount/used/total/remaining/percent columns stop drifting between rows, and one shared `CondensedTableLayout` so condensed headings and value columns always sit in the same terminal cell. Plan 086 then closes three narrow boundary defects found in post-implementation review — condensed offline/pending identity was erased by online-only HOST width, expanded-drive fit math did not share the indent/gap/separator constants with the renderer, and per-system suffix resolution used a local label width instead of the fleet `COMMIT` label width — without reopening the daemon, protocol, scheduler, or release architecture. Plan 086 reconciliation is recorded at the end of the plan. The default local check passes; one existing remote CI run is recorded below as evidence.

Plan 087 is a bounded client polish pass that keeps Plans 085/086 geometry and storage corrections intact while adding two compact-pane behaviors and one transient-selection polish. It introduces a strict integer-safe compact suffix policy: when the longest natural metric suffix across the entire online fleet exceeds one quarter of the terminal width, the entire suffix region disappears fleet-wide and the metric rows render as bar-only until the terminal widens again. It separates persistent logical selection from the transient reverse-video highlight: startup leaves the highlight `false`, Systems navigation activates it, and a resettable ten-second event-loop deadline dispatches `Action::ClearSelectionHighlight` while leaving `selected_id` (and `e` drive expansion) untouched. It omits the normal-header `IO` token entirely on unsupported or missing I/O-wait data instead of rendering a placeholder. No daemon, protocol, collector, normalized-capacity, scheduler, endpoint, configuration, dependency, CI, or release behavior changes.

Plan 088 is complete at implementation `58b332b51021e3950fa14d8888a46ed6d069a687`. It is a narrow corrective pass for the three confirmed low-priority findings in the 2026-08-26 workspace bug audit: route macOS byte percentages through the shared normalization helper, return non-Unix Ctrl-C listener failures through the reusable daemon runtime error boundary, and report duplicate EggPool configuration with a dedicated violation kind. The audit's informational observations and accepted optimizations remain out of scope.

Plan 089 is the completed follow-up corrective pass for the remaining actionable audit
findings: blank sampler identities, IPv6 zone parsing diagnostics, rejected
Systems reload feedback, pre-epoch staleness, large byte-ratio arithmetic,
daemon-name control characters, and CI-blocking clippy diagnostics.

Plan 090 is the completed follow-up for the remaining 2026-08-27 audit
findings: configuration metadata errors, bounded client timeouts, complete v2
capability objects, bounded protocol identities, failed-health categories,
typed DNS classification, injected EggPool deadlines, endpoint normalization,
and preservation of existing daemon config-directory permissions. It closed
in implementation `8193643`.

Plan 091 is in implementation: it hardens long-running `greggd` control and
optional drive collection, and makes `croncheck` identify a responsive Gregg
health endpoint before deciding whether to spawn. Deterministic regressions and
local lifecycle evidence are required before closure; the extended soak remains
manual evidence rather than CI infrastructure.

Plans 155-159 define the load-aware maintenance scheduler line for `greggd`,
and the line is now complete. Plan 156 qualified the boundary; Plan 157
implemented the scheduler with all six existing CI jobs green; Plan 158
corrected the schedule-validation defects and trimmed the allocation surface
at `7a466f8` (CI run `37172425056`); and Plan 159 closed the line with an
explicit budget decision (outcome 1): the scheduler line ships at the measured
3,261,664 stripped bytes (+164,464 / +5.309% over the 3,097,200-byte
pre-scheduler baseline), superseding the Plan-156 5%/128 KiB gate for this
line only. The general no-regression stance is unchanged, and any future
scheduler-line growth beyond 3,261,664 bytes re-opens review under a new plan.
Plan 156's recorded headroom correction stands as noted in Plan 156 rather
than rewritten. The line is independent of Plan
091 and preserves the read-only HTTP boundary, cached sampler load,
same-principal execution, service sandboxing, one pending occurrence per job,
and one global child slot.

Plan 160 is complete at implementation `d650c59` (with CI run `37177061388`
green across all six jobs) as the separate post-closure correctness hardening
pass for the scheduler timing layer. It addressed the discovered wall-clock
discontinuity edge: a long civil-time deadline was converted once into a
monotonic Tokio sleep, so a large forward system-clock adjustment could leave
an already-due cron occurrence asleep until the old monotonic deadline. The
event loop now sleeps at `min(semantic deadline, now + 60s)` and re-reads
civil time, while retry/max-wait/child lifecycle stay monotonic and
`Engine::next_deadline` stays truthful. Forward jumps coalesce, backward
jumps never launch early, nine deterministic clock-domain tests prove the
matrix, and stripped `greggd` is byte-neutral at 3,261,664 bytes (delta 0).
It did not reopen Plans 155-159, their footprint decision, or Plan 091, and
being terminal it unblocks no remaining plan.

Plan 161 was the coordination roadmap for cron observability and the
Gregg client-daemon split, and is now **closed** at `154ab36` with every child
plan complete. It keeps greggd scheduler execution local and
read-only over HTTP while adding bounded in-memory recent-history/output
observability, and moves Systems/EggPool polling out of each TUI into one
config-specific per-user Gregg background daemon. Plans 162-163 own the remote
scheduler contract and greggd implementation; Plans 164-165 own the local
client-daemon/IPC and user lifecycle; Plan 166 owns the bounded local cron cache
and plain c TUI; Plan 167 owns multi-client, restart, memory, payload, security,
native-lifecycle, and footprint closure. All six closed; the measured figures
and the two defects found during closure are in Plan 167's closure record. Plan
168 then closed the line's one remaining verification gap and found that the
Windows half of the client daemon had never been compiled at all; it is
described below. The
line is independent of the remaining Plan 091 soak record. It adds no remote
scheduler mutation, persistent cron database, generalized RPC/workflow system,
or separately distributed client-daemon binary.

Plans 169-172 are the active post-168 follow-up line, and all four are now
complete. Plan 169 was the concrete Windows verification corrective: establish the
exact native Clippy backlog, make full workspace Windows Clippy a permanent step in
the existing Windows job, and add one real named-pipe two-frontend proof without
adding a job or matrix. Its first native Windows `run_daemon` test then found that
`gregg daemon run` never exited on Windows at all, because aborting the accept
task cannot cancel the `spawn_blocking` `ConnectNamedPipe` it was parked in. Plan
170 performed a reversible client-daemon footprint/source-boundary campaign from
that clean baseline — attributing the 5,345,864-byte stripped Gregg binary — and
closed RETAIN CURRENT with zero bytes retained, every candidate having been
rejected on the merits. Plan 171 is the narrow corrective that Plan 169's stop
conditions required: it makes the parked accept cancellable, so the wait is
stopped rather than abandoned, and it also fixes the `select!` in `cli.rs` that
dropped the daemon's future on the signal path and so skipped the teardown
entirely. Plan 172 is a test-only corrective found while validating 171: the
MSRV job's failure there was a `gregg-update` download test whose `Failed(_)`
assertion was satisfied by a transient `fork` failure, so it then reported
`ENOENT` on a file the stub never wrote — it changed no update behavior, and
every affected test now proves its child actually ran before asserting a
classification. All four remain independent of Plan 091.

Plans 173-175 are the completed post-closure cron correctness line opened from the
2026-10-05 review of main at `1aac89f`. Plan 173 corrects greggd's
direct-child/output lifetime so an inherited stdout/stderr writer cannot retain
the one global scheduler slot after the scheduled child exits. Plan 174 corrects
the Gregg client daemon's live-summary dirty predicate, summary/history
epoch+revision coherence, endpoint-repoint identity, startup cadence, and
fleet-serial cron polling. Plan 175 then corrects the cron TUI's load relation,
elapsed-time wording, UTC timestamp labelling, stale/truncation row accounting,
and selected-job visibility on large/short layouts. The three plans preserve the
settled read-only scheduler boundary, memory-only bounded history, one remote
polling plane, and default-five display history. They are independent of Plan
091 and do not include the unrelated current Windows EggPool test failure.

Plans 176-178 are the completed narrow follow-up correctives from the subsequent
review of main at `892a771`; all three are independent of Plan 091 and of each
other, and each was closed against its own acceptance criteria.

Plans 179-181 are the completed post-closure bounded-liveness correctives from
the 2026-10-06 review of main at `fcbe2df`; all three are independent of Plan 091
and of each other, and each was closed against its own acceptance criteria.

Plan 179 is complete. The 250 ms post-exit settle deadline is now the *first*
branch of the biased select in `settle_output`, which is what makes it
authoritative rather than decorative. Plan 177 had removed stdout/stderr mutual
starvation but left the timer last, so a descendant that inherits a descriptor and
keeps writing left one `drain_step` ready on every poll and kept winning every
biased poll — the fixed bound could be read past indefinitely and one inherited
writer could hold the one global child slot forever. Before the deadline the timer
is pending, so Plan 177's alternating preference alone arbitrates and two ready
streams still make fair bounded progress; at or after it the timer wins over
either stream and no byte is read. An `Instant::now()` guard was deliberately
*not* added as a substitute: it can pass and then lose the next poll to a drain
that completed in between, so the priority has to live in the ordering — recorded
in the function docs and `AGENTS.md` so it is not "simplified" away. Two new
helper-level regressions use an already-expired deadline plus a scripted
continuously-ready reader, and both were mutation-tested: restoring the
drain-first ordering consumes 262,144 bytes (64 × `DRAIN_CHUNK`) past the bound
and fails them in 0.00 s. The retained silent-writer and
cancellation/rebuild cases still prove the bound without an EOF and that a
rebuilt future cannot restart it. Paired stripped `greggd` grew 16 bytes
(3,312,688 → 3,312,704, +0.0005%), so Plan 159 is not re-baselined and Plan
162's 3,400,000 ceiling holds with 87,296 bytes of headroom. No new dependency,
workflow, or job.

Plan 180 is complete. Cron reload/cancellation preemption now covers the *whole*
round, not just its HTTP fetches. Plan 178 made the fetches preemptible and the
history gate transactional with delivery, but the delivery itself still ran as a
bare `updates.send(observation).await` into the bounded 64-slot worker-to-engine
channel — the one await a round could park on unboundedly, and while parked the
round polled neither reload nor cancellation. `deliver_observation` now selects
cancellation, then reload, then the send, and the round returns that outcome as
the existing typed `RoundOutcome`, so only `Ok(())` ever reaches
`gate.commit(commit)`. Signals precede the send so a slot freeing alongside a
stored signal cannot let an obsolete observation and its pending commit win. This
is bounded loss of *superseded* work only: with no signal the channel still
backpressures and the observation is delivered. Eight new tests drive the real
bounded channel — `CRON_CHANNEL_CAPACITY` became `pub(crate)` so the
full-channel precondition is filled directly rather than synthesized through a
live fleet — and cron tests went 29 → 37. Reverting the send to a bare `await`
fails the full-channel reload, cancellation, and round-level tests in 0.03 s.
`CRON_CHANNEL_CAPACITY = 64` and `CRON_MAX_IN_FLIGHT = 4` are unchanged, as are
cadence, the one startup round, and revision/epoch-driven history.

Plan 181 is complete. EggPool's latest-desired-state convergence now stays live
*while a completed result is backpressured*, not just while an HTTP request is in
flight. Plan 176 had proved cancellation could interrupt a blocked result send,
but that helper selected only cancellation and the send, so the worker stopped
polling `control_rx.changed()` and a newer `(active, period, generation)` intent
waited for a result slot — the exact pressure under which leaving the pane should
be fastest. `deliver_result_or_interrupt` selects cancellation, then the control
watch, then the reservation, and returns a typed `ResultDelivery`.
`borrow_and_update()` keeps latest-value coalescing intact; `reserve()` rather
than `send()` is what makes the retry-after-equivalent-publication path possible;
and an *equivalent* publication is consumed rather than treated as supersession,
so a redundant repaint cannot silently lose a valid result. `Superseded` reuses
the existing `EggpoolWorkerState::converge`, so the abandoned result arms no
passive refresh deadline and at most one new request starts. Six new tests plus
the retained Plan-176 case drive the real channels; EggPool tests went 82 → 87.
Removing the `control_rx.changed()` branch fails deactivation, supersession, and
the worker-level test, while Plan 176's cancellation regression correctly still
passes — proving the branch was added rather than substituted. Latest-value
coalescing, one-request-at-a-time, the four-slot bound, and all EggPool
wire/auth/body-cap/health semantics are unchanged.

At the time of closure all three plans deliberately left their "existing six-job
CI completes green" boxes unchecked rather than claim an unobserved run: none adds
platform-specific code, protocol/wire changes, or CI infrastructure, so the
default local check plus the workspace Clippy/test gate was the lightest
appropriate mechanism under this repository's completion rule. CI run
`37414987171` on `7853743` has since gone green across all six existing jobs —
Linux, macOS arm64, macOS Intel, Windows (full-workspace Clippy and Test, both
release builds, SCM lifecycle smoke), MSRV Rust 1.89, and FreeBSD `gregg-host`
native — so those boxes are now checked and each closure record carries a CI
evidence note. The Windows steps are precisely the native authority the earlier
withholding was waiting for, and the workflow itself is unmodified. The
intermediate commits for Plans 179 and 180 were superseded by later pushes and
are covered by this same green tree, so no separate run is claimed for them.
Because all three are terminal, they unblock no remaining plan; Plan 091 is
still the only in-progress plan, gated solely on its own extended soak record.

Plan 176 is complete: the Windows EggPool full-result-channel cancellation
regression no longer synthesizes its own precondition. The worker already
delivered inside a cancellation-biased select; the test asked four requests to
closed loopback port 1 to fail, advanced paused time, and asserted the bounded
channel was full, which never happened on the Windows runner. The production
completion branch now calls one private `deliver_result_or_cancel` primitive
holding the same biased select, and the regression drives that exact function: it
fills all four slots of the real bounded channel first, proves the fifth delivery
stays pending while the token is live, cancels without freeing receiver capacity,
and requires the result to be abandoned. No fifth result enters the channel, the
buffered four come back intact and in order, and dropping the last sender closes
the receiver. Reverting the primitive to a bare `send().await` fails it, so the
lock is real. It is test/evidence only: no EggPool wire, config, API, capacity, or
retry behavior changed. The Windows half is closed by native CI run `37407151408`
on `2e14a83`, green across all six jobs with no new workflow, job, matrix, or
retry wrapper. Plans 173-175 name the removed
`worker_cancellation_wins_over_a_full_result_channel` in their historical records;
those are preserved and the replacement is
`a_full_result_channel_never_delays_cancellation`.

Plan 177 is complete: the post-exit settle now advances stdout and stderr
independently inside one fixed 250 ms budget. The old per-iteration
`tokio::join!` meant that with stdout holding several ready chunks and stderr
inherited by an idle descendant, one chunk was consumed and then the iteration
parked on the idle read — so the slot freed on time (Plan 173's liveness fix
stands) while bytes already available on the busy stream were dropped from the
retained tail. `settle_output` selects per iteration among one `drain_step` per
unfinished stream and the frozen deadline, with finished streams excluded by
precondition rather than by return value so a completed stream cannot win every
`biased` poll, and an alternating preferred branch so neither stream starves the
other. The deadline is passed in, never recomputed, so a wake or per-stream read
cannot extend it; nothing is spawned and only bytes from a completed read are
folded into the `RunningChild`-owned tails. Four deterministic helper-level tests
use scripted and permanently-idle readers under a zero-timeout poll — no
`test-util`, no paused clock, no wall-clock wait — and both mirror cases fail
against the old joined implementation. The process-level descendant, flood,
cancellation/rebuild, shutdown, and carried-index regressions remain green.
Paired stripped `greggd` grew 232 bytes (3,312,456 -> 3,312,688, +0.007%), so Plan
159 is not re-baselined and Plan 162's 3,400,000 ceiling still holds with 87,312
bytes of headroom. No new dependency.

Plan 178 is complete: an accepted config reload and cancellation are now selected
*inside* an active cron round, so reaction time no longer scales with the
superseded fleet. The round returns a typed `RoundOutcome`; on reload it drops the
in-flight futures — which are the requests, so nothing is left to abort — and the
outer loop re-reads endpoints, prunes absent gate keys, and starts a fresh round
immediately rather than parking on the 30-second cadence. `CRON_MAX_IN_FLIGHT` is
still 4, nothing is spawned, and rounds never overlap. The history gate is now
transactional: `settle` returns the observation plus a `PendingGateCommit`, and
the commit lands only after the bounded worker-to-engine channel accepts the
observation, so a document dropped by a preempted round or a closed channel can no
longer claim "already fetched" and suppress every later fetch. Five deterministic
tests cover both invariants and each was mutation-tested. 29 cron tests pass and
Plan 174's target binding, epoch+revision coherence, and startup/cadence budgets
remain authoritative. No new configuration field, dependency, or job.

CI run `37409557600` on `3b2a94a` is green across all six jobs, covering the
Windows Clippy and Test steps. Its predecessor `37408868235` on `515c106` failed
only on a `-D dead-code` gate over Plan 177's test-only predicate, recorded in that
plan's closure record.

None of Plans 176-181 reopens Plan 091. Their historical closures remain valid,
and the later review's Plans 179-181 closed three narrower liveness edges without
rewriting the 176-178 closure records.

Plan 175 is complete: the cron block is truthful about what it can actually
know. Load rows state the relation they mean — only a load-delayed job prints
`load15m 9.24 > 8.00`, a running job names the gate it started under, an idle job
names the previous one, and a missing reading is `unavailable` with no comparison
at all, never `0.00`. Elapsed states read `for 3m`/`pending 17m`/`queued 2m` and
countdowns read `next 11h`/`retry 20s`, with clock skew saturating at `0ms`, and
record clocks are labelled `Z` because they are UTC instants beside a remote-local
schedule; reconstructing scheduler-local civil time remains a protocol change and
was not invented here. `block_rows` now builds the block once, so the requested
height and the emitted height cannot drift and the stale-scheduler row is counted
by construction; the selected job's header and newest record are reserved ahead
of the job table, the table is a window around the selection clamped at both ends
with hidden jobs counted above and below, and anything left out is marked
`… more cron rows not shown` — a third marker, distinct from remote `stdout+`
truncation. 47 renderer tests (16 new) and the 894-test gregg suite pass with
Clippy and the default local check clean. This closes the 173-175 post-closure
cron correctness line with no protocol, execution, or retention change.

Plan 174 is complete: a frontend document is now republished whenever the
operator-visible scheduler state changes (capability, job rows,
`(epoch, revision)`, stale marker) instead of on `history_revision` alone, so a
live transition and a cleared error can no longer sit unpublished in the cache.
Summary and history are applied as one pair — a straddling response is never
merged, never advances the history gate, and is reported as a scheduler-scoped
`Incoherent` diagnostic with the summary kept. Cron observations and gate entries
are target-bound like metrics results, so a `Ctrl-R` repoint drops the old
target's answer and its retained history, and an equivalent endpoint spelling
preserves both. Startup now runs exactly one round (`interval_at`, not
`interval`), and each round observes the fleet with a fixed four reads in flight
instead of sequentially, removing single-endpoint head-of-line blocking without
a burst; nothing is spawned. 24 cron tests (11 new) and the full 878-test gregg
suite pass with Clippy and the default local check clean. No dependency was
added; `futures-util`'s `async-await` feature is now declared explicitly instead
of relying on transitive feature unification. Plan 175 is therefore unblocked.

Plan 173 is complete: the scheduler now treats the direct child's wait result as
the terminal execution event, freezes finish time and duration there, and bounds
post-exit capture with one fixed non-configurable 250 ms settle before closing
the read handles and freeing the one global child slot, so an inherited
descendant writer can no longer block every later maintenance job. Terminal
records are attributed through the index the child was launched with, and the
job-name search plus index-zero fallback are gone. 76 focused scheduler tests,
the `greggd` Clippy gate, and `./scripts/check-local.sh` are green; stripped
release `greggd` grew 5,104 bytes (3,307,360 -> 3,312,464, +0.154%), leaving the
scheduler line 50,800 bytes over the Plan-159 baseline and under Plan 162's
3,400,000 ceiling. Plans 174 and 175 remain open and are unblocked independently
of 173, so the next implementation step is 174.

Plans 107-111 coordinated the additive live-metrics work and are now complete.
Plan 108 owns
the protocol/client normalization boundary first: optional schema-v2 CPU
frequency, disk-I/O, and network fields, bounded validation, and mixed-version
fixtures. Plan 109 now owns native daemon collection and monotonic rate
publication. Plan 110 is complete at implementation `af9b9bd` with remote CI
run `34640870291`. Plan 111 closed final compatibility verification and
documentation after correcting Linux CPUFreq whitespace-list parsing in
`efb18dc` and `29a2633`; final documentation closure is `fa4b022`.
Daemon-version transport remains deferred.

Plan 112 is complete as the historical implementation record. It codified the existing
same-scope installer-rerun behavior as an explicit upgrade contract, stages
bootstrap Cargo fallbacks so the installer owns the final binary cleanly, and
adds component-safe `gregg uninstall` / `greggd uninstall` commands with
`--dry-run` and opt-in `--purge`. Post-closure review found four lifecycle
correctness gaps in exact-executable startup ownership, Windows SCM discovery,
Cargo-owned uninstall sequencing, and staged-Cargo daemon finalization; Plan 113
owns those corrections without rewriting Plan 112's original closure evidence.

Plan 113 is complete at implementation `7295c6e9a6b266f6da38eaf3d31558ce55d7cafb`
with CI run `34711999742` green across Linux, macOS arm64, macOS Intel,
Windows SCM smoke, and MSRV. It binds systemd/launchd/cron/SCM teardown to
the exact invoked `greggd` executable, preserves full Windows SCM state and
registration identity, sequences Unix Cargo-owned uninstall through the
complete Gregg lifecycle, and makes prebuilt/Cargo bootstrap acquisition
converge before daemon finalization. A follow-up Windows correction added a
fail-closed parser for the full SCM `lpBinaryPathName` launch command.

Plan 114 is complete at implementation `f794c962557e6af43fd12cff6d39226f2e3bd14d`
with remote CI run `34713929966` green across Linux, macOS arm64, macOS Intel,
Windows SCM smoke, and MSRV Rust 1.75. Drive details use `d`, plain `e` is
unmapped, and normal-view NET rows follow each online snapshot's network
availability while the fleet-wide horizontal geometry and condensed NET-column
policy remain intact.

Plan 115 is complete as a narrow post-Plan-113 corrective pass. It makes
`greggd restart` honor exact-executable manager ownership, preserves the running
state of a user-local direct/cron daemon across same-scope Unix bootstrap
replacement, fixes Windows shared update/uninstall elevation guidance so it
never emits Unix `sudo` instructions, and reconciles Plan 113's stale index
status without rewriting its historical implementation evidence. Implementation
commits are `d7cba02` and `17079e5`; remote CI run `34734612707` is green.
Post-closure review found that `greggd update` still used host-global manager
state for its pre-replacement lifecycle/quiescence decision; Plan 116 owns that
update-specific correction without reopening the valid Plan 115 work.

Plan 116 is complete as a narrow update-lifecycle ownership corrective pass. It
moves `greggd update` off host-global `startup_state()` for mutation authority,
requires exact SCM ownership before Windows quiescence, and ensures foreign or
inactive Unix manager state cannot mask the selected direct daemon's running
intent. Candidate preparation, exact-binary replacement, Plan 115 restart
ownership, and partial-success semantics remain intact. Implementation
`d0b9231` with remote CI run `34739895730` green.

Plan 117 is complete at implementation `ee485cc` with remote CI run
`35179950199` green across Linux, macOS arm64, macOS Intel, Windows SCM
smoke, and MSRV Rust 1.89. It supersedes only Plan 105's active decision
to retain Rust 1.75, keeps Plan 105's historical record intact, removes
the ten transitive-only resolver pins (genuine `uuid`/`url`/`reqwest`
deps keep ordinary ranges), moves the existing MSRV CI job in place,
and makes source/Cargo fallback requirements truthful without changing
runtime behavior.

Plan 118 is complete at implementation `66a0102` (plus Windows test-deadline
fix `cda51a4`) with remote CI run `35184430460` green across Linux, macOS
arm64, macOS Intel, Windows, and MSRV Rust 1.89. It replaces production
client-side reqwest usage with feature-minimal `eggfetch-core` 0.1.5,
preserving Systems polling and EggPool deadlines, body caps, stable
outcomes, protocol negotiation, and worker/scheduler ownership while
deleting Gregg's duplicated transport error and body-limit machinery.
Binary/dependency footprint changes are measured rather than assumed
(release `gregg` grows materially; see the Plan 118 closure).

Plan 119 is complete at implementation `8d7dfc2` with remote CI run
`35393388956` green across Linux, macOS arm64, macOS Intel, Windows, and
MSRV Rust 1.89. It adopts the
published `eggfetch-core 0.1.7` lean client profile
(`standard-http1 + tls-rustls`) instead of the broad `http1` compatibility
profile, removes the now-unavailable runtime `follow_redirects(false)`
configuration while preserving 3xx passthrough, and adopts 0.1.7's corrected
absolute `Timeout.total` through response-body EOF by mapping body-stage
typed timeout errors to Gregg's existing `Timeout` outcomes. It preserves
all other Systems/EggPool protocol, auth, body-limit, pool, scheduler and
worker semantics, proves that advanced-routing/retry/redirect/Basic/proxy
features are absent, and remeasures Gregg's actual fat-LTO release footprint
(stripped `gregg` 3,740,592 bytes: -524,320 / -12.3% versus the 4,264,912-byte
Plan-118 record). See
`119-eggfetch-0-1-7-lean-client-adoption.md`.

Plans 120-123 define and implement the bounded runtime-performance campaign.
Plan 120 coordinates the work without reopening scheduler, protocol, cadence,
or release architecture. Plans 121-123 landed at implementation `45582ce`:
Plan 121 owns mechanical allocation/copy reduction, Arc-preserving daemon
publication, owned client normalization, O(N) ordinary batch reduction, and
stable counter-baseline keys; Plan 122 owns TUI redraw/render preparation
efficiency; and Plan 123 owns daemon status-publication JSON caching and removal
of unused ready-health cloning. Post-closure review found one narrow observable
compatibility regression in the stale-after-failure 503 message plus
overstated timing-evidence wording. Plan 124 closed only those corrections at
implementation `b8d72b2`; the performance architecture remains intact. Current
main CI is green, including run `35541246292`.

Plan 125 is complete as the narrow adoption of published `eggfetch-core 0.2.0`
for the existing Gregg Systems/EggPool client. It kept Plan 119's
`standard-http1 + tls-rustls` feature boundary with a dependency-only change
(no application source change), preserved the single-dispatch/no-redirect/
no-retry transport contract, re-ran the timeout/body/network classification
regressions, proved the lean feature graph did not widen, and remeasured the
stripped `gregg` baseline at 3,740,592 bytes (delta 0). It did not change
`gregg-update`. See `125-eggfetch-0-2-lean-client-adoption.md`.

Plan 126 is complete with result RETAIN CURL. The benchmark-gated updater
transport consolidation experiment built an implementation-quality private
eggfetch 0.2 adapter (sync API, absolute deadlines, redirects, native roots,
explicit environment-proxy routing, streaming 64 MiB cap, partial cleanup,
typed error mapping) and exercised it against deterministic local fixtures
alongside the curl baseline (23 adapter + 6 baseline tests green). The
parity-complete feature set (`redirects` + `tls-native-roots` + `proxy`,
which pulls the broad `http1` alias) grew stripped `greggd` 2,432,408 →
4,989,488 bytes (+105%, ~20× the 5%/128 KiB gate) and `gregg` +47%, so the
candidate was reverted cleanly. External `curl` remains the update
transport; the kept transport-neutral parse/fixture regressions and the
measured rationale are recorded in
`126-eggfetch-updater-transport-consolidation-experiment.md`.

Plan 127 completed at implementation `1861bbc` after a 2026-09-23 review of published
`eggserve-server 0.2.1` and its required `eggserve-primitives 0.2.0`.
EggServe 0.2.1 now exposes separate cloneable shutdown and awaitable typed
completion authorities, and supports disabling the total connection lifetime
while preserving independent bounds. The EggServe transport passed the wire,
supervision, cached-body, feature-graph, footprint review, and loopback checks;
existing CI run `35871682878` passed all five jobs. See
`127-eggserve-0-2-daemon-http-transport-adoption.md`.

Plan 128 is complete at implementation `0f134b0` with remote CI run
`36170917401` green across Linux, macOS arm64, macOS Intel, Windows SCM
smoke, and MSRV Rust 1.89. It is the bounded macOS collector correctness pass
for the observed older-Intel loss of drive capacity and network telemetry: it
removes Gregg-owned Darwin ABI assumptions around `getmntinfo`/`statfs` and
AF_LINK `ifa_data`, moves the preferred network counter source to
`NET_RT_IFLIST2`/`if_msghdr2`/`if_data64` with a typed `if_data` fallback,
and extends the existing arm64+Intel native CI proof to the complete v2
optional metric payload. Protocol, TUI, cadence, readiness, and
Linux/Windows sources stay unchanged. See
`128-macos-intel-disk-network-collector-corrective-pass.md`.

Post-closure review found one narrow Plan-128 network-parser defect: Darwin's
`NET_RT_IFLIST2` stream interleaves `RTM_IFINFO2` with shorter heterogeneous
routing messages, but the landed parser applied the full `if_msghdr2` size
requirement before type discrimination and could stop before later interfaces.
Plan 129 is complete at implementation `30df587` with remote CI run
`36176134555` green across Linux, macOS arm64, macOS Intel, Windows SCM
smoke, and MSRV Rust 1.89. It corrects only that message-walk boundary
(common-prefix framing first, `if_msghdr2` size only for `RTM_IFINFO2`),
strengthens the native non-loopback-interface proof, and reconciles Plan 128's
closure record via its preserved post-closure correction note. Plan 128
remains complete with a Plan-129 corrective follow-up.

Plan 130 is complete at implementation `d4f2843` with remote CI run
`36186493797` green across Linux, macOS arm64, macOS Intel, Windows SCM
smoke, and MSRV Rust 1.89. It preserves the settled root/system versus
user-local destination contract from Plans 112-116, but replaces
advisory-only user-local PATH handling with idempotent supported-shell
persistence, a narrow opt-out, and truthful distinction between
future-shell persistence and current-parent-shell activation. The documented
rootless client quick-install path includes a trailing parent-shell
`export PATH="$HOME/.local/bin:$PATH"` so `gregg` is immediately resolvable
without a terminal restart; system installs, arbitrary PATH destinations,
uninstall ownership, Windows PATH policy, and release workflows remain
unchanged. See `130-user-local-installer-path-activation.md`.

Post-closure review found one narrow Plan-130 classifier defect:
`profile_contains_local_bin()` treats any textual `.local/bin` occurrence
as proof of active PATH integration, so comments or unrelated references can
suppress the managed block. Plan 131 is complete at implementation `3aade95`
with remote CI run `36193308083` green across Linux, macOS arm64, macOS
Intel, Windows SCM smoke, and MSRV Rust 1.89. It corrects only that static
classification boundary (recognizable active PATH integration or intact
Gregg-managed block suppresses a duplicate; comments and unrelated mentions
do not) and its deterministic tests while preserving the settled Plan-130
install/profile/activation/ownership behavior. Plan 130 remains complete
with a Plan-131 corrective follow-up.

Plan 132 opened the native host telemetry extraction and BSD-portability campaign
and is complete with Plans 133-136. The Linux/macOS/Windows move stayed
strictly behavior-preserving: Plan 133 froze/characterized the collector,
wire, readiness, public-path, rate/reset, and slow-probe contracts; Plan 134
extracted those native primitives into the protocol-neutral working
`gregg-host` workspace crate; Plan 135 cut `greggd` over through a
compatibility facade and qualified exact v1/v2/native/MSRV behavior
(stripped release `greggd` byte-identical at 2,629,008 bytes); Plan 136
added FreeBSD as the first post-extraction backend without introducing a
generic Unix collector. NetBSD/OpenBSD remain explicit later ports, and
complete FreeBSD `greggd` service/release support remains separately
deferred. Post-closure review found a narrow evidence/record defect: the
FreeBSD loopback qualification could pass without proving traffic-driven
counter advancement, Plan 136 overstated symmetric loopback traffic as an
RX/TX-direction proof, and Plans 132-136 retained unchecked acceptance boxes
despite complete closure records. Completed Plan 137 closed only those
corrections plus the precise FreeBSD package metadata and registry cleanup;
the extraction architecture remains closed. Plans 132-137 are independent
of the remaining Plan 091 soak record.

Plan 138 coordinated the second bounded runtime-performance campaign on the
post-Plan-137 architecture and is complete with Plans 139-143. Plan 139
memoized ready-health JSON per publication with borrowed serialization and
borrowed dispatch; Plan 140 prepares client poll targets once per installed
list with owned polling and index-based panic recovery; Plan 141 added
freshness-safe `CPUFreq`/macOS caching with deterministic source-call
accounting and explicitly retained current network metadata, disk topology,
and baseline scratch; Plan 142 ran the reversible dedicated worker
experiment and closed with RETAIN SPAWN_BLOCKING and zero production diff;
Plan 143 suppressed provable no-op redraws and added cross-render condensed
memo plus single-aggregate normal misses. The campaign did not reopen
protocol, cadence, supported capabilities, scheduler architecture, release
profile, or the completed native extraction. Plans 138-143 are independent
of the remaining Plan 091 soak record.


Post-closure review of completed Plans 138-143 found two narrow corrective items.
Plan 144 owns true concurrent single-flight ready-health memo initialization after
Plan 139; Plan 145 owns Linux CPUFreq online-membership freshness after Plan 141.
Plans 139/141 remain complete historical records. Plans 144-145 are independent
of Plan 091 and of each other.

Plan 144 is complete at implementation `808f44e4dd27de1c3ae2eb3238ce627696bb95f9`: it
replaces Plan-139's `Option<Bytes>` memos with per-publication
`Arc<tokio::sync::OnceCell<Bytes>>` cells, drops the `PublishedState` read
guard before awaiting `get_or_try_init`, installs fresh cells on every
publication/warming/failure transition, preserves retry-after-serialization-error
semantics, and proves eight-way concurrent v1/v2 first requests serialize
exactly once with byte-identical bodies through a `cfg(test)` `Barrier`-backed
deterministic gate.

Plan 145 is complete at implementation `e7f6256`: it replaces the
Plan-141 cached `affected_cpus` weights with structural `related_cpus`
identity sets per policy, reads the global
`/sys/devices/system/cpu/online` set once per sample to drive cached-policy
live weights, fails closed to the pre-Plan-141 live membership path when
the global online source is unreadable, and proves same-cardinality
online membership swaps change the weighted average immediately while
steady-state structural reads stay flat. The Plan 141 closure record is
preserved; its tests are updated to the new fixture shape (online +
`related_cpus`). Plans 144-145 are independent of the remaining Plan 091
soak record and of each other.

Plan 146 is complete at implementation `9651b68` with CI run
`36457308977` green across Linux, both macOS jobs, Windows, MSRV Rust 1.89,
and FreeBSD native. It is the final post-campaign reconciliation/hardening
pass and does not reopen Plans 144-145: Plans 144 and 145 now carry
truthful `complete` status lines plus the shared all-platform CI run
`36338426733`, the Plan-145 CPU-set helper surface is narrowed (private
`CpuIdSet`, private intersection count, no unused `live_weight()`, no
public cache inspection accessor), and the `MAX_CPU_ID` clamp is replaced
by an exact 8192-distinct-identity bound that preserves sparse Linux CPU
numbers and fails atomically instead of truncating.
`CpuFreqStructuralCache` and `ProcSource::cpu_frequency_hz_with_cache`
stay source-compatible and the Plan-145 steady-state read pattern is
unchanged. Plans 138-146 are independent of the remaining Plan 091 soak
record; with Plans 144-146 closed, Plan 091 is the only in-progress plan
in the 091-146 line. (The retired Plan 064 file retains a pre-existing
stale `planned` header; the 063-065 group is recorded complete in this
index, and reconciling that unenumerated artifact is noted in Plan 146's
closure rather than remade here.)

Plans 147-148 are the dependency-currentness follow-ups for the
already-adopted EggFetch/EggServe transports. Plan 147 advances the existing
lean `eggfetch-core 0.2` client from lockfile resolution 0.2.0 to the published
0.2.1 patch without changing the manifest feature contract. Plan 148 advances
`greggd` from `eggserve-server 0.2.1` / `eggserve-primitives 0.2.0` to the
published `eggserve-server 0.4.0` / `eggserve-primitives 0.2.2` pair while
preserving current wire behavior, split control/completion supervision,
known-length cached-`Bytes` responses, explicit runtime limits, and the
direct-H1 feature boundary. Plan 148 is complete at implementation
`2830498409885d6424905b7702b4f604e2399088`, with existing CI run `36872317548`
green across all six jobs. The plans are
semantically independent of each other and of the remaining Plan 091 soak
record; if implemented on parallel branches, the second merge must re-resolve
the shared `Cargo.lock` against current main.

Plan 149 is complete at implementation `8de5c12` with CI run
`36908815046` green across all six jobs (Linux, both macOS jobs, Windows
SCM smoke, MSRV Rust 1.89, and FreeBSD 14.2 native `gregg-host` under the
upgraded action in about five minutes). The native FreeBSD collector
remains complete under Plans 136-137; the defect
was the legacy `vmactions/freebsd-vm@v1.1.9` CI wrapper intermittently reaching
`First boot` and then polling `VM is booting` until GitHub's six-hour limit.
Plan 149 upgraded that existing job to maintained `v1.5.8` while retaining
the exact FreeBSD 14.2 floor, added a twenty-minute repository-owned outer
timeout, cancelled superseded same-ref FreeBSD jobs, and disabled unused guest
build-tree copyback. It is CI-infrastructure corrective work only and was
independent of Plan 091 and Plans 147-148; Plans 091 and 147 keep their
existing statuses.

Plan 150 is complete at implementation `2ceafcd3` with CI run
`36919813734` green across all six jobs. It is the Windows-only foreground-smoke
reliability corrective pass for failed CI run `36915516145`, where
`foreground_daemon_serves_v2_status` did not observe readiness within thirty
seconds on the documentation-only Plan-149 closure commit while the immediately
preceding implementation run `36908815046` was green with identical product
code. The smoke now takes the binary from Cargo's `CARGO_BIN_EXE_greggd` instead
of running a nested `cargo build -p greggd`, selects the loopback port from an OS
`127.0.0.1:0` allocation, captures child stdout/stderr in files inside its
temporary directory instead of unread pipes, detects early child exit during
readiness polling, and guarantees child termination and reaping on every path
with bounded failure diagnostics. The thirty-second deadline, 200-ms cadence,
and existing status/capability/identity/metric assertions are unchanged, and no
daemon, collector, SCM, HTTP, or CI behavior changes. The old harness's failure
cause remains unproven because it discarded that evidence; the corrected smoke
did not reproduce it. Plans 091 and 147 were already independent and kept their
statuses as of this closure (Plan 147 is since complete; see its record), and
Plan 150 is terminal in the dependency order.


Plan 147 is complete as a lockfile-only refresh of the already-selected lean
`eggfetch-core 0.2` client, retargeted from its recorded 0.2.1 to the then
published 0.2.2: one package version moved, the resolved feature graph and the
excluded-capability set are identical, every transport regression stayed green,
and the stripped fat-LTO `gregg` binary is byte-identical before and after. It
was never blocked by Plan 091 or by Plans 151-153. Plan 147 remains closed.
Plan 091 is still gated only on its own extended soak record. Completed Plan
153 fixed the EggPool schema-v1 production wire contract; post-closure evidence
review has opened independent Plan 154 to make its canonical fixture's ignored
runtime/provider fields exactly match EggPool's serialized structs.

Plans 151-153 are the completed functional implementation baseline for the post-closure EggPool corrective line. Plan 154 is a narrow evidence-only fixture cleanup and does not reopen that production behavior. Plan 151
is complete at implementation `4f9debe` with existing CI run `37029539823`
green across all six jobs: it replaces the August-25 `d31d72f`
drop-on-full worker-control contract without blocking terminal input with one
retained latest desired active/period/generation state published synchronously
through `EggpoolControl`, explicit `EggpoolWorkerState` naming
(`Idle`/`Refreshing`/`WorkerUnavailable`, no `Busy`), and deterministic
pressure/convergence coverage. Plan 152 is complete at implementation `7b88e43` with existing CI run
`37034974349` green across all six jobs, and current `main` at `9d2fcff` is
green in run `37035252848`. It adds
EggPool's newer schema-version-1 authenticated GET /api/status as an
independent health plane alongside the existing /api/stats/summary metrics, with
a per-route 1 MiB status ceiling, concurrent per-cycle reads, reducer-owned
health freshness, a bounded `Health:` header token plus provider counts, and the
full compatibility matrix.
The original Plans 056-062 remain a completed historical summary-pane
baseline; their closure records are not rewritten, but now carry supersession
notes where later behavior changed, and Plan 062's status line marks it a
historical record whose pressure contract was replaced. Both plans are
client-only, add no dependency/workflow/config schema, and are independent of
the remaining Plan 091 soak record and completed Plan 147.

Post-closure cross-repository review found that Plan 152's local schema-v1
fixture does not match EggPool's actual serialized `ProxyStatusSnapshot`:
provider rows use `provider_id` / `last_observation`, proxy account counts
are nested under `proxy`, and EggPool's producer bounds are 96-byte provider
IDs / 64-byte reason codes rather than Gregg's 64 / 128. Plan 153 closed that
narrow wire-contract and test-provenance defect at implementation `195724c`:
the private decoder now consumes the canonical field names and placement, the
bounded contract matches EggPool's own 256/96/64 limits with exact-limit and
one-over tests, one provenance-recorded canonical fixture (upstream repo,
commit, and `ProxyStatusSnapshot` type) replaced the self-authored matrix
payload, and a negative regression proves the never-upstream
`id`/`observation`/root-count shape is not a supported schema. No serde alias
was added. A later evidence review found that some fields included only as
ignored examples in the so-called canonical fixture (`runtime.pid`,
`runtime.started_at`, `account_count`, and nested `last_probe`) are not the
actual fields serialized by EggPool's `RuntimeHealthSummary` /
`ProviderHealthSummary`. Plan 154 completed that evidence-only cleanup with a
structurally canonical synthetic fixture. Plan 091 is the only in-progress
plan and remains gated on its independent extended soak record.

Plan 098 is the coordination roadmap for binary distribution (Plans 099-101);
it narrowly supersedes the former Plans 036-039 statement that GitHub releases
never contain binary attachments and Actions never creates release artifacts,
while preserving manual crates.io publication and tag creation.

Plan 099 implements the release binary matrix and bootstrap installers:
a dedicated release-only workflow builds the five required targets with a
documented glibc 2.17 floor, verifies each executable, hashes with SHA-256,
and assembles a draft release; Unix `install.sh` and Windows `install.ps1`
are binary-first with Cargo fallback for `armv7l`/unknown hosts. A 64-bit
Raspberry Pi/Le Potato uses the ordinary AArch64 Linux asset; ARMv7 remains
source-build only. macOS binaries are unsigned. No `cargo publish`, tag
creation, or auto-publication is added.

Plan 103 is the coordination roadmap for maintenance consolidation and
bounded diagnostics (Plans 104-106) and is complete with them. Plan 104
extracted the shared `gregg-update` mechanism (both application updaters
are thin adapters; `greggd` keeps activation/restart coordination),
moved reusable release checks into locally runnable scripts behind one
target table with a drift test, and preserved the exact update/install/
release contract. Plan 105 deleted `probe_top`, split `greggd` startup
and client config modules at ownership seams behind path-preserving
façades, audited all 13 compatibility pins (documented KEEP), and
retained MSRV 1.75 with a passing 1.75 workspace check. Plan 106 added
read-only `greggd status` (same probe authority as `croncheck`) and
stable client offline provenance rendered inside the existing row width.
Implementations `27ec978` / `2e4c4a1` / `a243162` (plus follow-up fix `30f7efe` for platform-conditional imports); each verified by the
full local checks plus Ubuntu direct-lifecycle smokes, and existing CI run `34293771380` green across all five jobs (Linux, macOS arm64, macOS Intel, Windows incl. SCM smoke, MSRV Rust 1.75).

Plan 092 is complete: it closed the actionable findings from the
2026-08-31 bugs audit around IPv6 zone-ID URL normalization, DNS error
 classification, zero-port validation clarity, and backward-clock snapshot
 staleness, with one low-risk endpoint precondition assertion.

Plan 093 is complete: it closes the remaining actionable findings from that
audit with bounded scheduler, state, drive-cache, endpoint/config, durability,
sampler, and byte-format corrections. Performance-only notes and the two
explicitly non-correctness observations remain excluded.

Plan 094 is complete: it closes the actionable findings in the
supplied `bugs.md` audit around reusable error propagation, bracketed endpoint
validation, DNS classification, drive-worker panic handling, synchronous
configuration locking, and generation-wrap documentation.

Plan 095 is complete at implementation `cfc1c84`: it closed the supplied audit's remaining
protocol-envelope, identity, endpoint, scheduler, sampler, clock, test
reliability, and aggregate-overflow findings without changing product scope.

Plan 096 is complete at implementation `de7ef0d`: it closes the supplied audit's remaining
current-code findings around atomic directory creation, drive refresh retry,
cross-platform durability, endpoint equivalence, fallback timing, DNS and
EggPool URL classification, worker cleanup, and Unix editor discovery without
adding product scope.

Plan 097 is complete at implementation `2601fb5`: it addresses the actionable
findings in the supplied `bugs.md` audit with bounded control-socket,
panic-boundary, config durability, TUI responsiveness, endpoint, state,
rendering, and collector corrections. Optimization-only suggestions remain
excluded.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) | Correct concrete cross-platform defects, retain only justified simplification, and close through bounded verification | complete through 074; CI-backed Windows SCM verification passed |
| [`067-truthful-drive-capacity-semantics.md`](067-truthful-drive-capacity-semantics.md) | Preserve truthful used, free, and caller-available drive capacity | complete |
| [`068-coherent-daemon-state-and-health.md`](068-coherent-daemon-state-and-health.md) | Publish one coherent daemon state and truthful health responses | complete |
| [`069-daemon-cli-runtime-and-test-correctness.md`](069-daemon-cli-runtime-and-test-correctness.md) | Correct config intent, runtime/error boundaries, exit codes, and omitted tests | complete |
| [`070-bounded-client-async-simplification.md`](070-bounded-client-async-simplification.md) | Retain scheduler or EggPool simplification only when smaller and behavior-preserving | complete; no change |
| [`071-measured-footprint-and-lightweight-closure.md`](071-measured-footprint-and-lightweight-closure.md) | Measure safe manifest/profile reductions and retain only verified improvements | complete |
| [`072-windows-service-runtime-and-record-correction.md`](072-windows-service-runtime-and-record-correction.md) | Correct Windows service runtime ownership and nonblocking shutdown | complete |
| [`073-native-windows-scm-entry-and-readiness-correction.md`](073-native-windows-service-runtime-and-record-correction.md) | Add native SCM dispatcher/`ServiceMain`, config handoff, and post-bind readiness | complete; operationally verified by 074 |
| [`074-ci-backed-windows-scm-closure.md`](074-ci-backed-windows-scm-closure.md) | Correct the Windows lifecycle smoke and run it in the existing Windows CI job | complete; run `31040689848` passed |
| [`075-configured-name-and-windows-hostname-correction.md`](075-configured-name-and-windows-hostname-correction.md) | Remove the native Windows hostname NUL and honor configured daemon names in foreground and SCM modes | complete; CI run `31189587467` |
| [`076-native-runtime-croncheck-and-version-correction.md`](076-native-runtime-croncheck-and-version-correction.md) | Separate Unix foreground runtime, health probing, config mutation, and version commands | complete; implementation and corrected strict-parser verification recorded |
| [`077-croncheck-strictness-test-cleanup-and-plan076-closure.md`](077-croncheck-strictness-test-cleanup-and-plan076-closure.md) | Bound and tighten `croncheck` status parsing, remove stale disabled tests, and close Plan 076 truthfully | complete |
| [`078-client-endpoint-url-config-reload-and-daemon-configprint.md`](078-client-endpoint-url-config-reload-and-daemon-configprint.md) | Reload stale client endpoints on `Ctrl-R`, accept HTTP URL input convenience for `gregg add`, and add read-only daemon bind-address printing | complete; historical wording corrected by 079 |
| [`079-scheduler-replacement-delivery-and-plan078-record-correction.md`](079-scheduler-replacement-delivery-and-plan078-record-correction.md) | Guarantee scheduler endpoint replacement delivery under bounded command pressure and correct Plan 078's environment record | complete; implementation `49c4c7d` |
| [`080-greggd-runtime-croncheck-and-direct-stop-correction.md`](080-greggd-runtime-croncheck-and-direct-stop-correction.md) | Diagnose/correct the daemon refusal and add direct local Unix `greggd stop` without restoring service-manager coupling | implemented and corrected by completed Plan 081; original Ubuntu lifecycle evidence preserved |
| [`081-plan080-cross-platform-stop-corrective-pass.md`](081-plan080-cross-platform-stop-corrective-pass.md) | Restore Windows foreground compatibility and make Unix stop identity/permissions/stale-socket handling safe | complete; implementation `59e17551`; CI run `31813136597`; Ubuntu one-daemon + two-daemon stop-isolation smokes passed |
| [`082-plan081-control-identity-and-record-polish.md`](082-plan081-control-identity-and-record-polish.md) | Normalize equivalent explicit config path spellings for Unix stop identity and reconcile Plan 080/081 records | complete; focused tests and relative/absolute release smoke passed |
| [`083-compact-tui-endpoint-nicknames-and-polling-invariant.md`](083-compact-tui-endpoint-nicknames-and-polling-invariant.md) | Six bounded client UI/CLI corrections: shared normal-view metric geometry, concise disk text, fresh-launch viewport snap, explicit-port `gregg add` with `nickname@host:port`, named versus unnamed offline rendering, offline-endpoint polling invariant | complete; corrective follow-up 084 closed |
| [`084-plan083-corrective-closure.md`](084-plan083-corrective-closure.md) | Close `--name` validation parity, renderer-level geometry proof, Unicode-aware offline padding, and stale `default_port` documentation | complete; implementation `020188f`; CI run `32100189772` |
| [`085-fleet-wide-tui-column-and-storage-display-correction.md`](085-fleet-wide-tui-column-and-storage-display-correction.md) | Fleet-wide normal-view metric geometry, `<used>/<total>` DISK slash denominator, shared expanded drive-detail table layout, shared condensed-view column layout | complete; closed through Plan 086 |
| [`086-plan085-renderer-boundary-corrective-pass.md`](086-plan085-renderer-boundary-corrective-pass.md) | Condensed offline/pending identity preservation, expanded-drive structural width constants and Compact-before-Minimal degradation, fleet-wide suffix budget | complete |
| [`087-dynamic-compact-metric-suffix-and-transient-selection-polish.md`](087-dynamic-compact-metric-suffix-and-transient-selection-polish.md) | Fleet-wide compact metric suffix when longest natural metric suffix exceeds terminal-width quarter; logical-vs-visual selection separation with resettable ten-second event-loop deadline; normal-header I/O-wait omission | complete |
| [`088-bugs-audit-corrective-pass.md`](088-bugs-audit-corrective-pass.md) | Correct shared macOS percentage normalization, non-Unix shutdown error propagation, and EggPool duplicate violation semantics | complete; implementation `58b332b` |
| [`089-bugs-audit-corrective-pass.md`](089-bugs-audit-corrective-pass.md) | Correct remaining actionable audit findings and CI-blocking clippy diagnostics | complete; implementation `7f245cc` |
| [`090-bugs-audit-corrective-pass.md`](090-bugs-audit-corrective-pass.md) | Correct remaining 2026-08-27 audit findings with minimal bounded changes | complete; implementation `8193643` |
| [`091-greggd-long-running-stability-and-croncheck-hardening.md`](091-greggd-long-running-stability-and-croncheck-hardening.md) | Harden long-running daemon control, optional drive refresh, and croncheck identity | implementation in progress |
| [`092-bugs-audit-corrective-pass.md`](092-bugs-audit-corrective-pass.md) | Correct IPv6 zone-ID transport, DNS classification, zero-port validation clarity, and future-snapshot staleness | complete; implementation `6efa52c` |
| [`093-bugs-audit-corrective-pass.md`](093-bugs-audit-corrective-pass.md) | Correct remaining actionable findings from the 2026-08-31 bugs audit with bounded behavior-preserving changes | complete; implementation `ede60b8` |
| [`094-bugs-audit-corrective-pass.md`](094-bugs-audit-corrective-pass.md) | Correct actionable findings from the supplied bugs audit with bounded behavior-preserving changes | complete; implementation `4930377` |
| [`095-bugs-audit-corrective-pass.md`](095-bugs-audit-corrective-pass.md) | Correct remaining actionable findings from the supplied audit with bounded behavior-preserving changes | complete; implementation `cfc1c84` |
| [`096-supplied-bugs-corrective-pass.md`](096-supplied-bugs-corrective-pass.md) | Correct remaining current-code findings from the supplied audit with bounded behavior-preserving changes | complete; implementation `de7ef0d` |
| [`097-supplied-bugs-corrective-pass.md`](097-supplied-bugs-corrective-pass.md) | Correct actionable findings from the supplied audit with bounded behavior-preserving changes | complete; implementation `2601fb5` |
| [`098-binary-distribution-install-update-roadmap.md`](098-binary-distribution-install-update-roadmap.md) | Binary distribution, bootstrap installation, and update roadmap for Plans 099-101 | complete; Plans 099-101 verified at `2eb0577` / CI `33683771778`; first real `vX.Y.Z` release after `1.0.11` will be the live binary proof |
| [`099-release-binary-matrix-and-bootstrap-installers.md`](099-release-binary-matrix-and-bootstrap-installers.md) | Release binary matrix and bootstrap installers (five targets, glibc 2.17, bootstrap `install.sh`/`install.ps1`, draft release assembly) | complete; implementation `19ee03a` (feat `dc31276` + fix `19ee03a`); CI `33672525397` green (Linux, macOS Intel/ARM64, Windows, MSRV) |
| [`100-greggd-startup-installation-and-restart.md`](100-greggd-startup-installation-and-restart.md) | greggd startup installation and restart (systemd/launchd/cron, `startup install`/`instructions`, `restart`) | complete; implementation `2271b9e` (feat `a73a6a3` + fix `2078924` + fix `2271b9e`); CI `33680301250` green (Linux, macOS Intel/ARM64, Windows, MSRV) |
| [`101-binary-first-self-update-and-release-integration.md`](101-binary-first-self-update-and-release-integration.md) | Binary-first self-update and release integration (`gregg update`/`greggd update`, crates.io authority, exact-tag assets, Cargo fallback) | complete; implementation `2eb0577` (feat) verified by CI `33683771778` green (Linux, macOS Intel/ARM64, Windows, MSRV) |
| [`102-update-restart-release-readiness-corrective-pass.md`](102-update-restart-release-readiness-corrective-pass.md) | Update/restart lifecycle corrections, bounded manager and Cargo processes, private staging, and truthful pre-binary-release installation docs | complete; implementation `008092c`; CI run `33695133206` green |
| [`103-maintenance-consolidation-and-bounded-diagnostics-roadmap.md`](103-maintenance-consolidation-and-bounded-diagnostics-roadmap.md) | Maintenance consolidation and bounded diagnostics roadmap for Plans 104-106 | complete; see Plans 104-106 closures below |
| [`104-shared-updater-and-release-policy-consolidation.md`](104-shared-updater-and-release-policy-consolidation.md) | One shared internal updater (`gregg-update`), thin application adapters, release-policy scripts, single target table with drift test | complete; implementation `27ec978`; CI `34293771380` green |
| [`105-source-boundary-repository-hygiene-and-msrv-review.md`](105-source-boundary-repository-hygiene-and-msrv-review.md) | Remove `probe_top`, split startup/config modules, audit compatibility pins, retain MSRV 1.75 | complete; implementation `2e4c4a1`; CI `34293771380` green |
| [`106-bounded-daemon-status-and-client-offline-provenance.md`](106-bounded-daemon-status-and-client-offline-provenance.md) | Read-only `greggd status` and client offline provenance | complete; implementation `a243162`; CI `34293771380` green |
| [`107-live-throughput-and-clock-metrics-roadmap.md`](107-live-throughput-and-clock-metrics-roadmap.md) | Live CPU clock, disk-I/O, and network roadmap | complete; closed by Plan 111 |
| [`108-additive-live-metrics-protocol-and-normalization.md`](108-additive-live-metrics-protocol-and-normalization.md) | Additive v2 live-metrics wire types, validation, fixtures, and client normalization | complete; implementation `94f01c5`; CI run `34606521857` green |
| [`109-native-clock-disk-io-and-network-collectors.md`](109-native-clock-disk-io-and-network-collectors.md) | Native CPU clock, disk-I/O, and network collectors | complete; implementation `0028843`; CI `34636802865` green |
| [`110-tui-clock-disk-io-and-network-integration.md`](110-tui-clock-disk-io-and-network-integration.md) | TUI clock, disk-I/O, and network presentation | complete; implementation `af9b9bd`; CI `34640870291` green |
| [`111-live-metrics-compatibility-verification-and-docs.md`](111-live-metrics-compatibility-verification-and-docs.md) | Mixed-version/live-metrics verification and final documentation closure | complete; implementation `efb18dc`, `29a2633`; docs `fa4b022`; CI `34644786245` green |
| [`112-installer-upgrade-and-cross-platform-uninstall.md`](112-installer-upgrade-and-cross-platform-uninstall.md) | Same-scope installer upgrade semantics and component-safe cross-platform uninstall | complete historical record; post-closure lifecycle corrections are Plan 113 |
| [`113-plan112-install-uninstall-ownership-corrective-pass.md`](113-plan112-install-uninstall-ownership-corrective-pass.md) | Correct Plan 112 exact-executable startup ownership, SCM discovery, Cargo uninstall sequencing, and fallback finalization | complete; implementation `7295c6e`; CI `34711999742` green; post-closure follow-up is Plan 115 |
| [`114-drive-key-and-per-system-network-row.md`](114-drive-key-and-per-system-network-row.md) | Remap drive details to `d` and omit the normal NET row per-system when network telemetry is unavailable | complete; implementation `f794c96`; CI `34713929966` green |
| [`115-plan113-restart-activation-and-elevation-corrective-pass.md`](115-plan113-restart-activation-and-elevation-corrective-pass.md) | Make restart manager dispatch exact-executable-aware, reactivate running user-local Unix daemons after same-scope replacement, and fix Windows elevation diagnostics | complete; implementation `17079e5`; CI `34734612707` green; post-closure follow-up is Plan 116 |
| [`116-update-lifecycle-ownership-corrective-pass.md`](116-update-lifecycle-ownership-corrective-pass.md) | Make `greggd update` pre-replacement lifecycle/quiescence exact-executable-aware across Windows SCM and Unix managed/direct cases | complete; implementation `d0b9231`; CI `34739895730` green |
| [`117-rust-1-89-msrv-and-dependency-modernization.md`](117-rust-1-89-msrv-and-dependency-modernization.md) | Raise the workspace MSRV to Rust 1.89, retire Rust-1.75-only resolver pins, move the existing MSRV CI job, and document source-install requirements | complete; implementation `ee485cc`; CI `35179950199` green |
| [`118-eggfetch-client-http-consolidation.md`](118-eggfetch-client-http-consolidation.md) | Replace client-side reqwest with feature-minimal eggfetch-core 0.1.5 while preserving Systems/EggPool transport contracts and measuring footprint | complete; implementation `66a0102` + fix `cda51a4`; CI `35184430460` green |
| [`119-eggfetch-0-1-7-lean-client-adoption.md`](119-eggfetch-0-1-7-lean-client-adoption.md) | Adopt published eggfetch-core 0.1.7 lean `standard-http1` profile, drop redirect configuration, map body-stage timeouts to existing `Timeout` outcomes, remeasure release footprint | complete; implementation `8d7dfc2`; CI `35393388956` green |
| [`120-bounded-runtime-performance-optimization-roadmap.md`](120-bounded-runtime-performance-optimization-roadmap.md) | Coordinate behavior-preserving daemon/client/TUI runtime optimization after the Plan-119 lean transport baseline | complete; campaign implementation `45582ce`; CI `35538999184` green |
| [`121-allocation-ownership-and-reducer-optimization.md`](121-allocation-ownership-and-reducer-optimization.md) | Remove avoidable daemon/client deep copies, use Arc-preserving publication, O(N) ordinary batch matching, and stable counter keys | complete; implementation `45582ce`; CI `35538999184` green |
| [`122-tui-render-path-and-redraw-optimization.md`](122-tui-render-path-and-redraw-optimization.md) | Reduce no-op redraws, fleet cache lookup/copy work, repeated suffix formatting, and duplicate condensed preformatting without visual changes | complete; implementation `45582ce`; CI `35538999184` green |
| [`123-daemon-status-publication-and-http-serialization-optimization.md`](123-daemon-status-publication-and-http-serialization-optimization.md) | Cache immutable status JSON per publication and remove unused ready-health cloning while preserving typed APIs and stale/health semantics | complete; implementation `45582ce`; CI `35538999184` green |
| [`124-runtime-optimization-compatibility-and-evidence-corrective-pass.md`](124-runtime-optimization-compatibility-and-evidence-corrective-pass.md) | Restore exact stale-after-failure response messages and reconcile unrecorded performance-evidence wording without reopening the optimization architecture | complete; implementation `b8d72b2`; CI `35541246292` green |
| [`125-eggfetch-0-2-lean-client-adoption.md`](125-eggfetch-0-2-lean-client-adoption.md) | Upgrade the existing lean Gregg polling transport to published eggfetch-core 0.2.0 without widening capabilities or changing observable behavior | complete; dependency-only change, lockfile 0.2.0, stripped `gregg` 3,740,592 bytes (delta 0) |
| [`126-eggfetch-updater-transport-consolidation-experiment.md`](126-eggfetch-updater-transport-consolidation-experiment.md) | Benchmark-gated experiment to replace gregg-update's external curl transport with eggfetch 0.2 while preserving update semantics and small-binary goals | complete with RETAIN CURL; parity candidate doubled stripped `greggd` (+105%), reverted cleanly |
| [`127-eggserve-0-2-daemon-http-transport-adoption.md`](127-eggserve-0-2-daemon-http-transport-adoption.md) | Replace greggd's Axum HTTP facade with direct EggServe 0.2.x while preserving wire, cached-body, supervision, keep-alive, and footprint contracts | complete at `1861bbc`; CI `35871682878` green across five jobs; EggServe 0.2.1 gates, wire tests, size review, and loopback check recorded |
| [`128-macos-intel-disk-network-collector-corrective-pass.md`](128-macos-intel-disk-network-collector-corrective-pass.md) | Correct Darwin filesystem/network ABI handling and prove macOS Intel+arm64 v2 drive/network telemetry | complete; implementation `0f134b0`; CI `36170917401` green; route-message parser follow-up owned by Plan 129 |
| [`129-macos-route-message-parser-corrective-pass.md`](129-macos-route-message-parser-corrective-pass.md) | Correct heterogeneous `NET_RT_IFLIST2` message walking, strengthen native interface-completeness proof, and reconcile Plan 128 closure | complete; implementation `30df587`; CI `36176134555` green |
| [`130-user-local-installer-path-activation.md`](130-user-local-installer-path-activation.md) | Persist user-local `~/.local/bin` for supported shells and document same-shell activation without changing install ownership | complete; implementation `d4f2843`; CI `36186493797` green |
| [`131-installer-path-profile-detection-corrective-pass.md`](131-installer-path-profile-detection-corrective-pass.md) | Correct false-positive existing-profile detection so comments/unrelated `.local/bin` text cannot suppress user-local PATH persistence | complete; implementation `3aade95`; CI `36193308083` green |
| [`132-native-host-telemetry-extraction-and-bsd-portability-roadmap.md`](132-native-host-telemetry-extraction-and-bsd-portability-roadmap.md) | Coordinate regression-controlled native telemetry extraction and FreeBSD-first portability | complete; coordinated 133-136 at `a9dab65`+`a5624a9`+`43b5cf3`+`d928950`+`a2d8bd2`+`c01389b`; CI `36220930632` green |
| [`133-native-collector-compatibility-characterization-and-boundary-freeze.md`](133-native-collector-compatibility-characterization-and-boundary-freeze.md) | Freeze collector/public-path/wire/readiness/rate/slow-probe behavior before moving source | complete; implementation `a9dab65`+`a5624a9`+`43b5cf3`+`d928950`+`a2d8bd2`+`c01389b`; CI `36220930632` green |
| [`134-gregg-host-native-telemetry-crate-extraction.md`](134-gregg-host-native-telemetry-crate-extraction.md) | Extract protocol-neutral Linux/macOS/Windows native telemetry into working `gregg-host` crate | complete; implementation `a9dab65`+`a5624a9`+`43b5cf3`+`d928950`+`a2d8bd2`+`c01389b`; CI `36220930632` green |
| [`135-greggd-gregg-host-adapter-cutover-and-qualification.md`](135-greggd-gregg-host-adapter-cutover-and-qualification.md) | Cut greggd over through a compatibility facade and prove exact native/v1/v2/MSRV equivalence | complete; implementation `a9dab65`+`a5624a9`+`43b5cf3`+`d928950`+`a2d8bd2`+`c01389b`; CI `36220930632` green |
| [`136-freebsd-first-native-telemetry-and-bsd-portability-foundation.md`](136-freebsd-first-native-telemetry-and-bsd-portability-foundation.md) | Add FreeBSD as the first post-extraction native backend and establish BSD portability seams | complete; implementation `a9dab65`+`a5624a9`+`43b5cf3`+`d928950`+`a2d8bd2`+`c01389b`; CI `36220930632` green; post-closure evidence/record follow-up is Plan 137 |
| [`137-native-telemetry-closure-and-freebsd-network-evidence-corrective-pass.md`](137-native-telemetry-closure-and-freebsd-network-evidence-corrective-pass.md) | Reconcile Plans 132-136 closure records and make FreeBSD loopback counter-activity qualification fail closed and truthful | complete; implementation `f5c2c4c`; CI `36223217199` green across Linux, both macOS jobs, Windows SCM smoke, MSRV Rust 1.89, and the strengthened FreeBSD native job; independent of Plan 091 |
| [`138-bounded-runtime-performance-follow-up-roadmap.md`](138-bounded-runtime-performance-follow-up-roadmap.md) | Coordinate the post-gregg-host behavior-preserving runtime optimization follow-up across daemon HTTP, client polling, native acquisition, sampler execution, and TUI/state | complete; campaign implementation `83df89e` + fix `e9ca180`; CI `36261210288` green across Linux, both macOS jobs, Windows SCM smoke, MSRV Rust 1.89, and FreeBSD native; coordinates 139-143; independent of Plan 091 |
| [`139-daemon-health-response-and-request-dispatch-optimization.md`](139-daemon-health-response-and-request-dispatch-optimization.md) | Memoize ready health JSON per publication and remove successful-route request-string ownership without changing stale/health semantics | complete; implementation `83df89e` (+ fix `e9ca180` docs-clean); campaign CI `36261210288` (see Plan 138); concurrent-single-flight follow-up is Plan 144 |
| [`140-prepared-client-poll-targets-and-endpoint-ownership-optimization.md`](140-prepared-client-poll-targets-and-endpoint-ownership-optimization.md) | Prepare v1/v2 poll targets once per installed endpoint list and reduce per-generation Endpoint cloning while preserving scheduler behavior | complete; implementation `83df89e` (+ fix `e9ca180` docs-clean); campaign CI `36261210288` (see Plan 138) |
| [`141-native-telemetry-acquisition-work-reduction.md`](141-native-telemetry-acquisition-work-reduction.md) | Reduce gregg-host native source work with freshness-safe CPUFreq/macOS caching and correctness-gated Linux network/disk consolidation | complete; implementation `83df89e` (+ fix `e9ca180` macOS `-D warnings`); unblocks 142 (closed RETAIN); campaign CI `36261210288` (see Plan 138); CPUFreq membership follow-up is Plan 145 |
| [`142-persistent-native-sampler-worker-experiment.md`](142-persistent-native-sampler-worker-experiment.md) | Compare per-tick spawn_blocking against a dedicated collector worker under strict lifecycle/panic/shutdown gates | complete with RETAIN SPAWN_BLOCKING; implementation `83df89e` (test-only, zero production diff); depends on 141; campaign CI `36261210288` (see Plan 138) |
| [`143-tui-state-noop-and-cross-render-optimization.md`](143-tui-state-noop-and-cross-render-optimization.md) | Suppress provable no-op redraws and reuse condensed/aggregate render preparation across unchanged frames | complete; implementation `83df89e` (+ fix `e9ca180` docs-clean); campaign CI `36261210288` (see Plan 138) |
| [`144-ready-health-single-flight-serialization-corrective-pass.md`](144-ready-health-single-flight-serialization-corrective-pass.md) | Make Plan-139 ready-health memoization truly single-flight under concurrent first requests while preserving retry/stale/failure semantics | complete at `808f44e`; per-publication `Arc<tokio::sync::OnceCell<Bytes>>` cells with dropped-guard `get_or_try_init` and deterministic concurrent-first-request gate; CI `36338426733` green across all six jobs |
| [`145-linux-cpufreq-online-membership-freshness-corrective-pass.md`](145-linux-cpufreq-online-membership-freshness-corrective-pass.md) | Make Plan-141 CPUFreq policy weighting respond to same-cardinality online CPU membership changes without restoring steady per-policy dynamic reads | complete at `e7f6256`; structural `related_cpus` cache + live `cpu/online` reads with fail-closed legacy fallback; preserves Plan-141 topology-change behavior; CI `36338426733` green across all six jobs; CPU-set visibility/cardinality boundary hardened by Plan 146 |
| [`146-post-campaign-record-and-cpu-set-boundary-corrective-pass.md`](146-post-campaign-record-and-cpu-set-boundary-corrective-pass.md) | Reconcile Plans 144-145 closure evidence/status and harden Plan-145 CPU-set visibility/cardinality semantics without reopening the runtime corrections | complete at `9651b68`; truthful 144/145 status + CI `36338426733`, private CPU-set identity helpers, exact 8192-distinct-identity cardinality bound preserving sparse CPU numbers; CI `36457308977` green across all six jobs; terminal for the 138-146 campaign |
| [`147-eggfetch-0-2-1-patch-adoption.md`](147-eggfetch-0-2-1-patch-adoption.md) | Advance the existing lean EggFetch client resolution from 0.2.0 to the published 0.2 patch without widening features or changing Systems/EggPool behavior | complete, retargeted to published 0.2.2 at `50aedac`, CI `37038881606` green across all six jobs; lockfile-only, feature graph identical, transport regressions green, stripped `gregg` byte-identical (delta 0); depends on completed 125/current main; independent of 091, 148, and 151-152 |
| [`148-eggserve-0-4-direct-server-adoption.md`](148-eggserve-0-4-direct-server-adoption.md) | Advance greggd to eggserve-server 0.4.0 + eggserve-primitives 0.2.2 while preserving current wire, lifecycle, runtime-limit, cached-body, and direct-H1 contracts | complete at `2830498409885d6424905b7702b4f604e2399088`; CI `36872317548` green across six jobs; depends on completed 127/current main; independent of 091 and 147 |
| [`149-freebsd-ci-vm-bootstrap-reliability-corrective-pass.md`](149-freebsd-ci-vm-bootstrap-reliability-corrective-pass.md) | Bound and modernize the existing FreeBSD 14.2 native CI VM bootstrap without changing collector semantics or qualification strength | complete at `8de5c12`; CI `36908815046` green across all six jobs; terminal after 137, independent of 091 and 147-148 |
| [`150-windows-foreground-smoke-reliability-corrective-pass.md`](150-windows-foreground-smoke-reliability-corrective-pass.md) | Make the native Windows foreground daemon smoke deterministic, fail-fast, diagnostic, and process-clean without changing product behavior | complete at `2ceafcd3`; CI `36919813734` green across all six jobs; Cargo-provided binary path, OS-selected loopback port, file-backed bounded child diagnostics, early child-exit detection, guaranteed reaping; terminal after 149, independent of 091 and 147-149 |

| [`151-eggpool-desired-state-delivery-and-worker-state-corrective-pass.md`](151-eggpool-desired-state-delivery-and-worker-state-corrective-pass.md) | Replace lossy EggPool try_send/Busy command pressure with nonblocking latest-desired-state convergence and clarify local worker-state naming | complete at `4f9debe`; CI `37029539823` green across all six jobs; watch-based `EggpoolDesiredState` + `EggpoolWorkerState` (no `Busy`), deterministic pressure/convergence tests, local checks green; post-closure correction to 056-062/d31d72f; independent of 091 and 147 |
| [`152-eggpool-service-health-status-plane-integration.md`](152-eggpool-service-health-status-plane-integration.md) | Add EggPool schema-v1 /api/status proxy/provider health alongside the existing four-metric summary without conflating worker lifecycle or endpoint failures | complete at `7b88e43`; CI `37034974349` green across all six jobs, current `main` `9d2fcff` green in `37035252848`; typed health model, per-route 1 MiB status ceiling, concurrent dual-plane worker read, separate health freshness in AppState, compact `Health:` token and provider counts, full compatibility matrix, local checks green; depends on completed 151; post-closure wire-contract defect corrected by completed Plan 153; independent of 091 and 147 |
| [`153-eggpool-schema-v1-wire-contract-corrective-pass.md`](153-eggpool-schema-v1-wire-contract-corrective-pass.md) | Correct Plan-152 status JSON field names/placement and producer bounds against EggPool's canonical schema-v1 `ProxyStatusSnapshot`, with upstream-provenance regression fixture | complete at `195724c`; `proxy`-nested account counts, `provider_id`/`last_observation`, producer-aligned 256/96/64 bounds with exact/one-over tests, one canonical provenance-recorded fixture replacing the synthetic matrix, negative regression locking out the never-upstream `id`/`observation`/root-count shape; CI `37046499078` green across all six jobs, local checks green; functional correction complete; fixture-evidence cleanup owned by Plan 154; independent of 091 |
| [`154-eggpool-canonical-status-fixture-evidence-cleanup.md`](154-eggpool-canonical-status-fixture-evidence-cleanup.md) | Make Plan-153's canonical status fixture structurally identical to EggPool schema-v1 for ignored runtime/provider fields, without changing production decoding or behavior | complete at `fd24918`; CI `37053403192` green across all six jobs; focused/local checks green; independent of 091; unblocks no remaining plan |
| [`155-load-aware-maintenance-scheduler-roadmap.md`](155-load-aware-maintenance-scheduler-roadmap.md) | Coordinate an optional bounded local cron-like maintenance scheduler for greggd with cached-load deferral, coalescing, anti-herd serialization, and no remote execution surface | complete at the Plan-159 decision (outcome 1: line re-baselined to the measured 3,261,664 bytes); independent of 091 |
| [`156-scheduler-execution-boundary-and-footprint-qualification.md`](156-scheduler-execution-boundary-and-footprint-qualification.md) | Qualify same-principal execution, privileged-Unix opt-in, five-field local cron semantics, parser/time footprint, process supervision, stdio bounds, and shutdown behavior before scheduler code lands | complete at `f35e1369c36cc38de5bd5e9e42f4a27d338db1e1`; the cron-parser + chrono + Tokio-process linked candidate passed its prototype gate and unblocked 157; Plan 158 appended a correction note recording that its 38,424-byte headroom omitted the 67,584-byte `jobs` field Plan 157 added; independent of 091 |
| [`157-load-aware-maintenance-scheduler-implementation.md`](157-load-aware-maintenance-scheduler-implementation.md) | Implement bounded cron scheduling, cached 1m/5m/15m load gates, fixed retry/max-wait deferral, one pending occurrence per job, one global child slot, transition logging, and deterministic qualification | complete at the Plan-159 decision; functional criteria demonstrated at `7a466f8` / CI `37172425056`, footprint criterion met under the re-baselined 3,261,664-byte budget; independent of 091 |
| [`158-scheduler-footprint-and-schedule-validation-corrective-pass.md`](158-scheduler-footprint-and-schedule-validation-corrective-pass.md) | Close the remaining Plan-157 footprint gate and reject calendar-impossible cron expressions before daemon startup, while preserving scheduler semantics/security/lifecycle | complete at the Plan-159 decision; implemented at `7a466f8`, CI `37172425056` green across all six jobs, and its stop-condition escalation resolved by the 159 re-baseline; independent of 091 |
| [`159-scheduler-footprint-budget-qualification-and-re-baseline-decision.md`](159-scheduler-footprint-budget-qualification-and-re-baseline-decision.md) | Decide explicitly whether the scheduler line ships on a re-baselined measured footprint budget, on one named approved architectural reduction, or not at all | complete (outcome 1): scheduler line re-baselined to the measured 3,261,664 bytes for this line only, general stance unchanged, growth beyond it re-opens review; qualifying CI `37172425056`, focused/default/release checks green; closes 155/157/158; independent of 091 |
| [`160-scheduler-civil-clock-reconciliation-hardening.md`](160-scheduler-civil-clock-reconciliation-hardening.md) | Bound the scheduler's trust in long monotonic sleeps so forward/backward wall-clock discontinuities are reconciled at minute-scale without changing cron/load/lifecycle semantics | complete at `d650c59` (implementation `6bb5dcf` + Windows-only `#[cfg(unix)]` test-attribute correction); CI `37177061388` green across all six jobs; one-minute `MAX_CIVIL_RECHECK` wake cap with truthful semantic deadlines, nine deterministic clock-domain tests, stripped `greggd` byte-neutral at 3,261,664 bytes (delta 0 vs Plan-159 baseline); independent of 091; terminal, unblocks no remaining plan |
| [`161-cron-observability-and-client-daemon-roadmap.md`](161-cron-observability-and-client-daemon-roadmap.md) | Coordinate bounded greggd scheduler history/output observability plus a config-specific per-user Gregg client daemon so multiple TUIs share one polling plane and plain c can inspect cron health | **complete** at `154ab36`; all six child plans closed (162 `42e2fc2`, 163 `4fd0158`, 164 `b25ca04`, 165 `c8f2542`+`ee6ad7e`, 166 `53c06e5`, 167 `154ab36`); all 18 roadmap criteria met; one polling plane counted across N frontends, memory-only history with no disk persistence, no remote control plane; greggd 3,316,568 B inside 162's rule and gregg +989,752 B (+22.76%) with a **zero** `Cargo.toml`/`Cargo.lock` diff; macOS/Windows/MSRV CI not run here (no remote runner access) and must be confirmed before release; independent of 091 |
| [`162-scheduler-observability-contract-and-resource-qualification.md`](162-scheduler-observability-contract-and-resource-qualification.md) | Freeze scheduler summary/history wire types, live/outcome vocabulary, dedupe identity, default-5 history bounds, stdout/stderr caps, body/memory limits, security policy, and a numeric greggd footprint rule before integration | **complete** at `42e2fc2`; contract record in the plan; `scheduler_history_limit` default 5 / hard max 10 / 0 disables; 1024 raw + 512 JSON-escaped published bytes per stream; measured worst-case history body 832,022 bytes vs a 1 MiB client cap; handed 163 a 3,400,000-byte `greggd` ceiling (met at 3,306,320); no dependency added; independent of 091 |
| [`163-greggd-scheduler-history-and-read-only-api.md`](163-greggd-scheduler-history-and-read-only-api.md) | Implement memory-only per-job terminal history, bounded concurrent output draining, live load/pending/running publication, and read-only scheduler summary/history routes without changing execution semantics | **complete** at `4fd0158`; `GET/HEAD /v2/scheduler` + `/v2/scheduler/history`, separate publication cell from metrics `PublishedState`, publication only on visible state change, `spawn_failed`/`load_expired` are terminal records, concurrent bounded drain by borrowing the streams, stripped `greggd` 3,306,320 (+1.369% vs baseline, within 162's ceiling); verified against a live daemon; 480 `greggd` tests green; **unblocks the scheduler half of 166**; independent of 091 |
| [`164-gregg-client-daemon-core-and-local-ipc.md`](164-gregg-client-daemon-core-and-local-ipc.md) | Move Systems/EggPool polling and normalized fleet state behind one same-binary config-specific client daemon with same-user Unix-socket/Windows-pipe IPC, versioned handshake, latest-state fan-out, and daemon-owned config reconciliation | **complete** at `b25ca04` (transport foundation at `c887094`); `gregg daemon run`/`status`/`stop` exist and `run` never self-daemonizes; `FleetState` is the daemon-owned reducer and `AppState::adopt_snapshot` is a TUI's only fleet writer; fan-out is a `watch` slot with each document serialized once for all frontends; the handshake carries protocol version **and** config identity; the EggPool worker is daemon-owned with order-independent intent reduction; `Ctrl-R` crosses the boundary and `gregg add`/`remove`/`refresh` nudge a running daemon; no direct-polling fallback; workspace tests green (669 `gregg` lib incl. 8 new boundary tests), clippy 0, `check-local.sh` pass; **unblocks 165 and contributes to 166**; independent of 091 |
| [`165-client-daemon-lifecycle-install-update-and-uninstall.md`](165-client-daemon-lifecycle-install-update-and-uninstall.md) | Add lazy bare-gregg activation, single-launch coordination, per-user startup ownership, protocol-version reconciliation, and client-daemon-aware install/update/uninstall without creating a privileged client service | **complete** at `c8f2542` (lazy activation, config-specific launch lock with no PID file, endpoint classification, directional version rotation, user-scoped startup) and `ee6ad7e` (prepare-then-quiesce update, ownership-first uninstall, user-local installer registration); only plain absence authorizes a spawn and the classification is redone under the lock; startup is always user-scoped (`systemctl --user` / LaunchAgent / Startup folder / user crontab), never a system service, `LocalService` SCM, or `sudo`; 710 `gregg` lib tests green, clippy 0, `check-local.sh` pass; **unblocks 166**; independent of 091 |
| [`166-cron-cache-and-tui-observability.md`](166-cron-cache-and-tui-observability.md) | Poll scheduler summary/history once in clientd, retain a longer globally bounded memory-only local history, add plain c cron detail with default-five job history/output, load-delay state, and terminal-control sanitization | **complete** at `39d9bd7` (sanitizer + bounded `(epoch, sequence)`-deduplicated cache), `818922b` (`[cron]` config + the client-daemon scheduler poller), `dce9cc1` (two-tier local publication over the existing socket), `5624b48` (the `c` block, adoption-boundary sanitization, and docs); summary on a 30s cadence with history only on discovery/revision change; cache bounded by per-job depth, a **global constant** 4096-record ceiling, and the remote output cap; the cron intent governs **transmission and never fetching**, so N windows cost the fleet what 0 windows cost; `Unsupported` (404) is a healthy older daemon and never touches `Reachability`; remote text is escaped in `cat -v` caret notation; 1,573 workspace tests green, clippy 0, `check-local.sh` pass; **unblocks 167**; independent of 091 |
| [`167-cron-client-daemon-correctness-and-footprint-closure.md`](167-cron-client-daemon-correctness-and-footprint-closure.md) | Prove one polling plane across multiple TUIs, lazy-start/restart behavior, bounded output/history memory and payloads, old/new daemon compatibility, IPC security/native lifecycle, and measured greggd/gregg footprint | **complete** at `154ab36`, on top of the evidence commit `9245fa2`; all 24 acceptance criteria met; `qualification.rs` derives every memory/payload bound from the real constants and asserts them (daemon history default 368,640 B / max 737,280 B, client cache ceiling 5,242,880 B, frame cap 8,388,608 B); idle 0.533% of one core with 0 frontends vs 0.622% with 2; **found and fixed a real attach bug** — a state document could be published ahead of the handshake `Hello`, so a healthy daemon looked incompatible and a TUI would exit; also corrected the report labelling the daemon maximum as the default; security checklist all clear; 1,597 workspace tests green, clippy 0, `check-local.sh --release` pass; **closes the 161 line** |
| [`168-windows-client-daemon-transport-correctness-and-verification.md`](168-windows-client-daemon-transport-correctness-and-verification.md) | Close the 161 line's unverified-platform gap: make the Windows client-daemon transport build and run, name the endpoint in the `\\.\pipe\` namespace, keep the current-thread runtime unblocked, and execute the pipe path in CI for the first time | **complete** at `eb2bc62` (on top of the transport fix `01a1405`); CI run `37243685819` on `eb2bc62` green across all six jobs with 803 passed / 0 failed in the Windows client lib and all six added Windows tests executed on the real runner; the Windows half of Plan 164 had been written but never compiled, so the Windows job had been red since 164 landed; corrected stream handle ownership, the owner-only SDDL descriptor, `PIPE_REJECT_REMOTE_CLIENTS` placement, four missing `windows-sys` feature gates, `LOCKFILE_EXCLUSIVE_LOCK`/`SDDL_REVISION_1` module paths, and two Unix-only greggd test helpers that `-D warnings` rejected; the endpoint is now `\\.\pipe\gregg-client-<id>`, the blocking accept is parked on the blocking pool, reads use `PeekNamedPipe`, and liveness is `WaitNamedPipeW` rather than `path.exists()`; four new Windows tests execute the pipe end to end and found that `FrontendFrame::ProtocolError(String)` can never be serialized under an internally tagged enum, so every refusal reason had always been silently discarded; 1,598 workspace tests green, clippy 0, and a windows-gnu type-check of the whole workspace clean; independent of 091 |
| [`169-windows-target-lint-and-native-multi-client-verification.md`](169-windows-target-lint-and-native-multi-client-verification.md) | Close the remaining Windows verification hygiene gap by inventorying/fixing the native Clippy backlog, adding full Windows workspace Clippy to the existing job, and proving two real named-pipe frontends can share one clientd instance | **complete** at `cc4ea9f`; CI run `37265973270` on `cc4ea9f` green across all six jobs, with the Windows `Clippy` step green on the native MSVC runner for the first time; the pre-fix backlog is **twelve** findings, not the "eleven" Plan 168's prose claimed (its own per-file attribution sums to twelve), all fixed behavior-preservingly, and removing two stale `#[allow(clippy::ptr_as_ptr)]` attributes unmasked two real `ptr_as_ptr` findings they were hiding; the Windows job installs Clippy and runs the same full-workspace `-D warnings` gate as Linux with **no new job or matrix**; the clientd daemon test harness moved out from behind `#[cfg(all(test, unix))]` into a platform-neutral `test_support` module so two new native Windows tests drive `run_daemon` and `attach` over the real `\\.\pipe\gregg-client-<id>`; **that first native Windows `run_daemon` test found a real defect** — `accept_task.abort()` cannot cancel a `spawn_blocking` `ConnectNamedPipe`, so `gregg daemon run` acknowledges a stop and then hangs instead of exiting; corrective work is registered as **171**; release binaries are byte-identical (`gregg` 5,345,864 / `greggd` 3,316,568), confirming a clean baseline for 170; independent of 091 |
| [`170-gregg-client-daemon-footprint-and-source-boundary-optimization.md`](170-gregg-client-daemon-footprint-and-source-boundary-optimization.md) | Attribute the ~996 KiB same-binary clientd/TUI growth and retain only measured behavior-preserving source-boundary/footprint reductions; close RETAIN CURRENT if no safe >=64 KiB cumulative win exists | **complete (RETAIN CURRENT)** on code SHA `cc4ea9f`, record at `fe4993d`, CI run `37267973159` green across all six jobs; **zero bytes retained**; unstripped paired builds of HEAD vs `ae56926` plus `nm` attribute +873,547 bytes of text to the clientd line (of a +996,128-byte file delta), with 486,985 in the new clientd modules; the narrative needed two corrections — the line **shrank** third-party text (`eggfetch_core` −24,617, `hyper_util` −4,912) and `run_tui` fell 41,854→25,266 because the TUI no longer owns polling; 346,801 bytes sit in 326 duplicated-but-byte-identical symbol groups whose reclaim needs nightly-only `-Zmerge-functions` (all four stable rustc spellings rejected), measured and recorded rather than gamed; every candidate was rejected on the merits — A on **correctness** (all four startup renderers must stay on every platform), B below threshold at 38,363 bytes, C a wire-format change, D dominated by one real async state machine; `qualification.rs` contributes 0 linked bytes and stays; independent of 091 |
| [`171-windows-client-daemon-accept-cancellation-and-clean-shutdown.md`](171-windows-client-daemon-accept-cancellation-and-clean-shutdown.md) | Make `gregg daemon run` terminate on Windows: the accept wait must be cancellable so `accept_task.abort()` plus runtime shutdown cannot wedge process exit after a stop request or supervisor signal | **complete** at `e3f0694`; **closure evidence run `37271258556` on `d402c72` green across all six jobs**, with all three new tests passing natively on the MSVC runner and the Windows `Test` step at 2m15s against a 2m12s baseline (i.e. finished, not wedged as Plan 169's first run was for >21 min); **option 2** (stop the wait, not abandon the thread) — option 1 (`shutdown_timeout`) is excluded by the plan's own "no thread is abandoned" criterion and option 3 by the overlapped-I/O stop condition, and `ConnectNamedPipe` is not one of the four functions Win32 lists as non-cancellable; the parked thread publishes a real `THREAD_TERMINATE` handle to itself (**not a thread id** — ids are recycled on exit) and `AcceptStop::request` cancels the wait with `CancelSynchronousIo`; three races are closed explicitly — the stop is recorded before the record is inspected and re-checked under the same lock, the lock is held across the cancel (so the thread is provably still the parked one) and released between attempts, and a cancel that lost the race to enter the kernel is re-issued while the record is up; `ERROR_OPERATION_ABORTED` maps to `Disconnected`, preserving both existing `accept_loop` arms; **a second instance of the defect was found in `cli.rs`** — returning from `select!` dropped `run_daemon`'s future and skipped the whole teardown on the signal path, so it now cancels **and awaits**; `release_parked_accept` is **deleted**, not reduced, and `Running::shutdown` asserts the daemon exits; three new native Windows tests cover both stop orderings and assert a stopped daemon releases its runtime (the drop itself is the assertion, on its own thread under a deadline, because a drop that never returns cannot be awaited in-line without wedging CI as Plan 169's first run did); only new dependency surface is the `Win32_System_Threading` feature on existing `windows-sys`; the handshake, frame format, owner-only SDDL, `PIPE_REJECT_REMOTE_CLIENTS`, instance rotation, `Hello`-first contract, and every Unix path are unchanged; independent of 091 |
| [`172-gregg-update-download-classification-test-spawn-flake.md`](172-gregg-update-download-classification-test-spawn-flake.md) | Stop `gregg-update`'s download-classification test from reporting a transient `Command::spawn` failure as a missing `calls` file, and from failing on it at all | **complete** at `be1aee0`; **closure evidence run `37308641245` on `2dee87c` green across all six jobs**, with MSRV — the job that failed in `37270317172` — green; `download_file` and the `DownloadOutcome` contract are **byte-for-byte unchanged**, so the plan's stop condition was not triggered; "the child ran" is proven from the **child's own side effect** — every stub `curl` now appends to a `calls` log as its first act, so `download_file_with_started_stub` can separate "curl never started" from "curl started and was classified" without parsing a production error string (the `"curl failed: "`-prefix alternative was rejected as a string coupling that would silently reopen the defect on any reword); a spawn failure is retried 4× with 50 ms backoff and then fails as a **named spawn assertion** carrying the reason `download_file` reported and the attempt count; `a_stub_that_never_started_is_reported_as_a_spawn_failure` reproduces the runner's `fork` failure deterministically every run rather than waiting for one; **all four** siblings made the same mistake and were corrected — the oversize and unparseable stubs now assert their specific rejection reason (a spawn failure previously satisfied *both* of their assertions, exercising nothing), and the real-`curl` 500 baseline gained `spawn_recording_response`, which records that a client reached the fixture; **a correction to this plan's own analysis:** it predicted the assertion would check `unexpected HTTP 500`, but production always passes `-f`, so a 5xx exits non-zero and arrives via the non-success arm as `curl exit Some(22)` — both arms are now covered instead, and the new exit-0-on-5xx `stub_curl_success_with_code` closes a previously untested defense-in-depth branch; **the windows-gnu cross-check caught a real break** — the new constants are dead on Windows because the stub cluster is `#[cfg(unix)]`, which `-D warnings` in the Windows job would have failed, so both are now `#[cfg(unix)]`, and that job's green `Clippy` step is the confirmation; 44 `gregg-update` tests (was 42), 1,600 workspace tests, clippy 0, `check-local.sh` pass, **five consecutive MSRV 1.89 runs** green, and 8/8 concurrent test-binary runs as a loaded-runner proxy; independent of 091 |
| [`173-greggd-scheduler-direct-child-output-lifecycle-corrective.md`](173-greggd-scheduler-direct-child-output-lifecycle-corrective.md) | Correct the greggd scheduler/output lifetime so direct-child exit, not inherited pipe EOF, releases the one global scheduled-child slot; keep bounded concurrent output capture and direct-child timing/attribution truthful | complete; two-phase child/output state machine with a fixed 250 ms post-exit settle, frozen direct-child timing, carried-index attribution; 76 scheduler tests + default local check green; stripped greggd +5,104 bytes; independent of 091 and 174-175 |
| [`174-gregg-cron-client-daemon-coherence-and-polling-corrective.md`](174-gregg-cron-client-daemon-coherence-and-polling-corrective.md) | Publish live scheduler-summary/error transitions even with unchanged history revision; require coherent epoch+revision history, bind observations/gates to endpoint identity, eliminate the duplicate startup round, and use a small bounded cron request concurrency | complete; operator-visible publication predicate, coherent summary+history pair, target-bound observations/gates, one startup round, four reads in flight; 24 cron + 878 workspace tests + default local check green; **unblocks 175** |
| [`175-gregg-cron-tui-truthfulness-and-bounded-layout-corrective.md`](175-gregg-cron-tui-truthfulness-and-bounded-layout-corrective.md) | Correct cron load/elapsed/timestamp wording, account for stale/truncated rows truthfully, and keep the selected job/history visible with large job sets and constrained terminal height | complete; relation-by-meaning load labels, elapsed-vs-countdown grammar, `Z`-labelled record clocks, one row builder feeding both heights, reserved selected-job section, selection-centred job window, explicit viewport truncation; 47 renderer + 894 workspace tests + default local check green; no protocol change |
| [`176-windows-eggpool-full-result-channel-test-determinism-corrective.md`](176-windows-eggpool-full-result-channel-test-determinism-corrective.md) | Replace the Windows-sensitive closed-port timing fixture with a deterministic proof that cancellation wins over a blocked send to the full bounded EggPool result channel | **complete**; production corrected only by extracting one `deliver_result_or_cancel` primitive (same biased select) and rewriting the regression to fill the real bounded channel deterministically; 81 `eggpool` tests green; mutation-tested; CI `37407151408` green across all six jobs on `2e14a83`; independent of 091/177/178; no EggPool behavior change |
| [`177-greggd-post-exit-dual-stream-drain-progress-corrective.md`](177-greggd-post-exit-dual-stream-drain-progress-corrective.md) | Preserve Plan 173's 250 ms direct-child settle while allowing ready stdout/stderr streams to make independent post-exit progress when the other inherited stream remains open and idle | **complete**; `settle_output` selects per iteration among one `drain_step` per unfinished stream plus the frozen deadline, with finished streams excluded by precondition and an alternating preferred branch; 20 observation tests green, both mirror cases mutation-tested; stripped `greggd` +232 bytes (3,312,456 -> 3,312,688), Plan 162 ceiling retained; depends on completed 173; independent of 091/176/178; no protocol/process-group change |
| [`178-gregg-cron-worker-reload-preemption-corrective.md`](178-gregg-cron-worker-reload-preemption-corrective.md) | Let accepted config reload/cancellation interrupt an active bounded cron fleet round, and commit the history gate only after a coherent observation is handed to the engine | **complete**; reload/cancellation selected inside the active round with a typed `RoundOutcome` and immediate re-entry; `settle` returns a `PendingGateCommit` applied only after channel delivery; 29 cron tests green, both invariants mutation-tested; depends on completed 174; independent of 091/176/177; four-read bound and 30s steady cadence preserved |
| [`179-greggd-post-exit-settle-deadline-authority-corrective.md`](179-greggd-post-exit-settle-deadline-authority-corrective.md) | Make the frozen 250 ms post-exit settle deadline outrank continuously-ready descendant stdout/stderr so output readiness can never retain the global scheduler slot beyond the bounded settle | **complete** at `5442dd2`; **closure evidence run `37414987171` on `7853743` green across all six jobs**, including the native Windows Clippy/Test steps; the deadline is the first `biased` branch in both `prefer_stdout` arms, with the optional `Instant::now()` guard deliberately rejected (it can pass and then lose the next poll to a drain that completed in between); 22 observation tests green, both ordering cases mutation-tested — restoring drain-first ordering consumed 262,144 bytes past the deadline in 0.00 s; stripped `greggd` +16 bytes (3,312,688 -> 3,312,704), Plan 162 ceiling retained with 87,296 bytes of headroom; depends on completed 177; independent of 091/180/181; no protocol/process-group change |
| [`180-gregg-cron-observation-delivery-preemption-corrective.md`](180-gregg-cron-observation-delivery-preemption-corrective.md) | Let reload/cancellation preempt a cron observation blocked on the full bounded worker-to-engine channel, with history-gate commit only after successful delivery | **complete** at `64676b0`; **closure evidence run `37414987171` on `7853743` green across all six jobs**, including the native Windows Test step that executes the round-selection regressions; `deliver_observation` selects cancel, then reload, then the send, so the gate commit stays reachable only on `Ok(())`; 37 cron tests green (was 29), mutation-tested — a bare `send().await` failed the reload, cancellation, and round-level regressions in 0.03 s; `CRON_CHANNEL_CAPACITY` made `pub(crate)` so tests fill the real bounded channel; depends on completed 178; independent of 091/179/181; four-read bound, 30s cadence, and the 64-slot channel capacity all preserved |
| [`181-eggpool-desired-state-result-backpressure-corrective.md`](181-eggpool-desired-state-result-backpressure-corrective.md) | Keep EggPool latest-desired-state convergence live while a completed result is blocked on the full bounded result channel, abandoning only superseded work | **complete** at `7853743`; **closure evidence run `37414987171` on `7853743` green across all six jobs**, including the native Windows Clippy/Test steps that are the platform authority Plan 176 waited on; `deliver_result_or_interrupt` selects cancel, then `control_rx.changed()`, then `sender.reserve()`, returning a typed `ResultDelivery`; an equivalent publication is consumed and the wait continues, so a redundant repaint cannot become a lost result; `Superseded` reuses the existing `EggpoolWorkerState::converge` for at most one request; 87 `eggpool` tests green (was 82), mutation-tested — removing the control branch failed deactivation and supersession in 0.00 s while Plan 176's cancellation regression still passed, proving addition not substitution; depends on completed 151+176; independent of 091/179/180; four-slot bounded backpressure preserved and never made lossy |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073 -> 074 -> 075 -> 076 -> 077 -> 078 -> 079 -> 080 -> 081 -> 082 -> 083 -> 084 -> 085 -> 086 -> 087 -> 088 -> 089 -> 090 -> 091 -> 092 -> 093 -> 094 -> 095 -> 096 -> 097 -> 098 -> 099 -> 100 -> 101 -> 102 -> 103 -> 104 -> 105 -> 106 -> 107 -> 108 -> 109 -> 110 -> 111 -> 112 -> 113 -> 114 -> 115 -> 116 -> 117 -> 118 -> 119 -> 120 -> 121 -> 122 -> 123 -> 124 -> 125 -> 126 -> 127 -> 128 -> 129 -> 130 -> 131 -> 132 -> 133 -> 134 -> 135 -> 136 -> 137 -> 138 -> 139 -> 140 -> 141 -> 142 -> 143
139 -> 144
141 -> 145
144 -> 146
145 -> 146
125 -> 147
127 -> 148
137 -> 149
149 -> 150
062 + 070 + current main -> 151
151 -> 152
152 -> 153
153 -> 154
current post-154 main -> 155
155 -> 156
156 -> 157
157 -> 158
158 -> 159
159 -> 160
160 -> 161
161 -> 162 -> 163
161 -> 164 -> 165
163 + 164 -> 166
163 + 165 + 166 -> 167
161-167 -> 168
168 -> 169 -> 170
169 -> 171
163 + 167 + current post-172 main -> 173
166 + 167 + current post-172 main -> 174
174 -> 175
current 892a771 -> 176
173 + current 892a771 -> 177
174 + current 892a771 -> 178
177 + current fcbe2df -> 179
178 + current fcbe2df -> 180
151 + 176 + current fcbe2df -> 181
155 is the coordination roadmap for the load-aware maintenance scheduler and is independent of the remaining Plan 091 soak record; 156 qualified same-principal execution, local five-field cron semantics, process lifecycle, and prototype footprint; 157 implemented the scheduler and 158 reconciled its functional criteria and corrected validation/allocation; 159 closed the original line with the explicit re-baseline decision, so 155-159 remain complete. Plan 160 is a separate post-closure correctness hardening pass for civil-clock discontinuities and does not reopen those historical closures. None of 155-160 depends on the remaining Plan 091 soak record, and none adds a workflow, job, matrix, or artifact requirement.
Plan 161 coordinated the cron-observability/client-daemon line and is now closed with all of 162-167 complete: 162 `42e2fc2` settled the remote scheduler wire/output/memory/footprint contract before 163 changed greggd; 164 `b25ca04` built the client-daemon split, independently justified by duplicate TUI polling; 165 `c8f2542`/`ee6ad7e` added per-user lifecycle on the Plan-164 boundary; 166 `53c06e5` added the bounded local cron cache and plain `c` view on top of the remote scheduler plus clientd core; 167 `154ab36` closed the line on measured multi-client, restart, memory, payload, security, and footprint evidence. Plans 161-167 were and remain independent of Plan 091, and added no workflow, job, or matrix requirement.
Plan 168 is the post-closure corrective pass for the one gap Plans 161 and 167 both flagged honestly: the Windows job had not been run. Running it showed the Windows half of the client daemon had been written but never compiled, so the Windows CI job had been red since Plan 164 landed. 168 corrected the transport (stream ownership, the owner-only SDDL descriptor, `PIPE_REJECT_REMOTE_CLIENTS` placement, the `windows-sys` feature gates, the `\\.\pipe\` endpoint name, a blocking-pool accept, and a `PeekNamedPipe` read), added a `WaitNamedPipeW` liveness probe so the `stop` confirmation stops answering "not running" for a running daemon, and added four Windows tests that execute the pipe path for the first time. Those tests then found a second, pre-existing defect: `FrontendFrame::ProtocolError(String)` can never be serialized because the enum is internally tagged, so every "here is why you were refused" frame had always been silently discarded. Plan 168 depends on 161-167 and remains independent of Plan 091, and adds no workflow, job, or matrix requirement.
Plan 169 follows 168 because 168 explicitly recorded a Windows-only Clippy backlog and the existing Windows job still does not run Clippy. It also closes the one remaining native evidence gap with a two-frontend named-pipe test while preserving the shared Plan-167 one-poll-plane proof. The first step must re-enumerate the warnings because Plan 168's prose says eleven while its per-file attribution is arithmetically ambiguous; closure should correct the historical note rather than assume either count. Plan 170 then depends on 169 so its stripped-size baseline includes all correctness/lint cleanup. It is a reversible optimization pass, not a new architecture: the same-binary clientd/TUI boundary is fixed, no dependency may be added for size, and the plan closes RETAIN CURRENT if attribution finds no safe cumulative reduction of at least 64 KiB. Plan 171 depends on 169 alone, not on 170: it is a correctness fix for a defect 169's own test found, and it was implemented and closed while 170 was already closing, so 171 does not inherit any 170 decision. Plan 172 **depends on nothing** and is placed last only for provenance: it was found while validating 171, when the MSRV job failed on a `gregg-update` download test whose `Failed(_)` assertion was satisfied by a transient `fork` failure, leaving an `ENOENT` on a file the stub never wrote. It changed no update behavior, and all four are independent of Plan 091. Plan 173 is independently justified by a post-Plan-163
execution-boundary defect: the joined pipe drains can wait for descendant-held
EOF after the scheduled direct child has exited, retaining the one global child
slot. Plan 174 is independently justified by five client-daemon correctness
defects in the completed Plan-166 path: live state can fail to republish,
summary/history can cross epochs/revisions, observations can cross endpoint
repoints, startup polls twice immediately, and one slow endpoint serializes the
whole cron fleet. Plan 175 follows 174 only so it is verified against corrected
state delivery; it owns renderer truthfulness/layout defects and makes no
protocol change. Plans 173 and 174 may be implemented in parallel; both are
now complete, and Plan 175 followed 174 and is now complete too, so the
173-175 line is closed. Plans 176-178 do not reopen that line's architecture: 176 is a
deterministic CI/test correction for the independently introduced EggPool
backpressure test, 177 is a bounded output-completeness correction to 173's
post-exit settle, and 178 is a reload/cancellation responsiveness correction to
174's worker loop with the existing fleet-side target check retained as final
authority. All three are complete; 176 closes on native Windows CI run
`37407151408` on `2e14a83`, green across all six jobs, and the Windows EggPool
backpressure regression is no longer a red CI signal. Plans 179-181 are later,
independently justified follow-ups: 179 owns deadline priority after 177, 180
owns blocked observation-send preemption after 178, and 181 owns EggPool
desired-state preemption during result-channel backpressure after 151/176.
066 ... 097 complete or in-progress as above; 098 is the coordination roadmap for 099-101;
099 may proceed independently of the remaining Plan 091 soak record;
100 requires 099's binary/bootstrap contract and Plan 091's final croncheck semantics;
101 requires 099's asset contract and 100's restart contract;
102 requires 098-101's implementation and corrects their release-readiness boundaries without rewriting their historical closure records;
103 coordinates 104-106 and completes with them; 104 removes duplicated updater/release policy first, 105 cleans module/repository/dependency boundaries on the settled ownership, 106 adds bounded diagnostics on the simplified structure; 107 coordinates live metrics, 108 owns the additive protocol and normalization boundary, 109 owns native collection, 110 owns TUI integration, and 111 owns final compatibility/verification closure;
112 depends on the settled installer/update/startup ownership from 099-105, can proceed independently of the remaining Plan 091 soak record, and owns the original install-rerun/uninstall lifecycle implementation;
113 depends on Plan 112's implementation and closes only the post-closure lifecycle findings;
114 depends on the settled Plans 110-111 TUI/live-metrics baseline and is independent of Plan 113, so it may proceed in parallel despite being the next numbered plan;
115 depends on Plan 113 plus the settled restart/update contracts from Plans 100-102, is independent of completed Plan 114, and does not depend on the remaining Plan 091 soak record;
116 depends on Plan 115 plus the settled self-update ownership/restart contracts from Plans 101-104 and 113, is independent of completed Plan 114, and does not depend on the remaining Plan 091 soak record;
117 depends on the settled Plan 105 workspace/dependency baseline, deliberately supersedes only its active Rust-1.75 decision, and is independent of the remaining Plan 091 soak record;
118 depends on Plan 117's Rust 1.89/dependency baseline and the published `eggfetch-core` 0.1.5 API, and is independent of the remaining Plan 091 soak record;
119 depends on Plan 118's eggfetch transport baseline and the published `eggfetch-core` 0.1.7 API, and is independent of the remaining Plan 091 soak record;
120 coordinates the bounded runtime-performance campaign after Plan 119 and does not reopen Plan 091;
121 depends on 120 and owns the mechanical ownership/allocation/reducer changes that settle the publication boundary;
122 depends on 120 and may proceed after or alongside 121 once reducer-internal helper names are settled;
123 depends on 120 and 121 because it builds cached HTTP publication on the Arc-preserving server handoff.
124 depends on the completed 120-123 implementation and owns only stale/failure response-envelope compatibility plus truthful evidence-record reconciliation.
125 depends on Plan 119's settled lean eggfetch client contract and the current post-124 main state; it upgrades only the client-side eggfetch dependency to published 0.2.0 and remains independent of the remaining Plan 091 soak record.
126 depends on completed Plan 125 plus the settled self-update ownership/lifecycle contracts from 101-104, 115, and 116; it is a reversible updater-transport experiment and may close with RETAIN CURL if behavioral parity or footprint gates do not justify adoption.
127 depended on the settled daemon publication/HTTP compatibility baseline from Plans 123-124 and the current post-126 workspace state. EggServe 0.2.1 satisfied the lifecycle/error-observation and unlimited healthy total connection lifetime gates; implementation closed at `1861bbc` with existing CI run `35871682878` green across all five jobs. The plan records the runtime policy, wire parity, footprint review, and lightweight loopback evidence.
128 depends on the settled live-metrics collector/protocol/client baseline from Plans 109-111 and 114 plus the current post-127 daemon state. It is a macOS-only collector correctness pass, independent of the remaining Plan 091 soak record, and must close against the existing native arm64+Intel CI matrix without adding a new workflow.
129 depends on completed Plan 128 and corrects only its post-closure `NET_RT_IFLIST2` heterogeneous-message parsing defect plus closure-record inconsistency. It is independent of Plan 091 and must use the existing native macOS arm64+Intel CI jobs without adding new infrastructure.
130 depends on the settled installer/update/uninstall ownership contracts from Plans 112-116 and the current post-129 baseline. It changes only Unix bootstrap PATH persistence/activation UX, remains independent of Plan 091, and must use the existing deterministic installer harness plus ordinary CI without adding a workflow or privileged dotfile smoke.
131 depends on completed Plan 130 and corrects only its over-broad existing-profile `.local/bin` substring classifier. It remains independent of Plan 091 and must use the existing isolated installer harness plus ordinary CI without adding shell-evaluation, workflow, or privileged-dotfile infrastructure.
132 coordinates the native host telemetry extraction and BSD-portability campaign after the settled Plans 109-111, 117, 120-124, and 128-129 collector/runtime baseline; it is independent of the remaining Plan 091 soak record.
133 depends on 132 and freezes the current collector public paths, sequence semantics, slow-probe behavior, readiness mapping, and canonical v1/v2 output before source movement; it blocks the extraction/cutover work.
134 depends on completed 133 and creates the protocol-neutral working `gregg-host` workspace crate, moving shared/native Linux/macOS/Windows telemetry implementation without changing timing, worker, metric, or platform semantics; it blocks final greggd cutover and FreeBSD expansion.
135 depends on completed 133-134 and makes `greggd` consume the extracted implementation through a compatibility facade, preserving current `greggd::collector` paths and exact v1/v2/native/MSRV behavior; it is the qualification gate before new platform work.
136 depends on completed 132-135 and adds an explicit FreeBSD backend to the reusable telemetry crate using native FreeBSD interfaces, while leaving full FreeBSD greggd service/release support and NetBSD/OpenBSD implementation to later separately researched plans.
137 depends on completed 132-136 and corrects only the post-closure telemetry evidence/record defects: fail-open FreeBSD loopback qualification, overstated RX/TX-direction wording, unchecked acceptance records, precise FreeBSD package metadata, and the duplicated implementation-list entry. It is independent of Plan 091 and does not reopen collector formulas, protocol behavior, daemon runtime, or the extraction architecture.
138 coordinates the second bounded runtime-performance campaign after completed 120-124 and 132-137, and remains independent of Plan 091.
139 depends on 138 plus the settled 123-124 status/health publication contract and owns only health-response memoization plus request-dispatch allocation cleanup.
140 depends on 138 plus the settled eggfetch polling contract from 118-125; it prepares targets and reduces ownership churn without redesigning PollScheduler.
141 depends on 138 plus the completed gregg-host extraction/FreeBSD qualification; it may retain only native acquisition changes that preserve immediate metric/topology semantics and blocks 142.
142 depends on completed 141 and is explicitly reversible: it compares a dedicated collector worker with the current spawn_blocking model and may close RETAIN SPAWN_BLOCKING.
143 depends on 138 plus the settled Plan-122 TUI baseline and may proceed independently; it owns only provable no-op redraw/state and cross-render formatting work.
144 depends on completed 139/138-143 and corrects only the ready-health concurrent-first-request memo initialization gap; it is independent of Plan 091 and Plan 145 and does not reopen HTTP wire/stale/runtime policy.
145 depends on completed 141/138-143 and corrects only Linux CPUFreq online-membership freshness/source-call qualification; it is independent of Plan 091 and Plan 144 and does not reopen Plan 141's macOS/network/disk/baseline decisions.
146 depends on completed 144-145 and owns only their post-closure status/evidence reconciliation plus the Plan-145 CPU-set helper-visibility and cardinality-bound cleanup; it is independent of Plan 091 and does not reopen ready-health single-flight or CPUFreq online-membership behavior. 144/145/146 are now complete, so the 091-146 line has no remaining corrective follow-up; the remaining Plan 091 extended soak record is independent.
147 depends on completed 125 plus current main and advances only the already-selected lean EggFetch 0.2 client to the published patch line; it is independent of Plan 091, Plan 148, and Plans 151-152. It was retargeted from the recorded 0.2.1 to published 0.2.2 and is complete at `50aedac` as a lockfile-only refresh: one package version moved, the resolved feature graph is identical, the excluded capability set is unchanged, every transport regression stayed green, and the stripped fat-LTO `gregg` is byte-identical before and after. Existing CI run `37038881606` at `e5c3231` is green across all six jobs. It is terminal and unblocks no remaining plan.
148 depends on completed 127 plus current main and advances only the already-selected direct EggServe H1 server boundary to eggserve-server 0.4.0 / eggserve-primitives 0.2.2 with compatibility qualification; it is complete at `2830498409885d6424905b7702b4f604e2399088`, with existing CI run `36872317548` green across all six jobs. It is independent of Plan 091 and Plan 147. Plans 147 and 148 may be implemented in parallel semantically, but both touch Cargo.lock, so the second integration must re-resolve against the first if their branches overlap.
149 depends on completed Plans 136-137 plus current main and corrects only the existing FreeBSD native CI VM bootstrap boundary: upgrade the maintained VM action while retaining FreeBSD 14.2, add a twenty-minute outer job timeout, cancel superseded same-ref FreeBSD jobs, and disable unused VM copyback. It is independent of Plan 091 and Plans 147-148 and does not reopen collector implementation or native qualification semantics.
151 depends on completed Plans 056-062, the Plan-070 async-state-machine review, and the current main state after the intentional `d31d72f` nonblocking change. It owns only the EggPool worker-control contract: one retained latest desired active/period/generation state published without waiting for capacity, local worker-state naming separate from EggPool service health, and deterministic pressure/convergence coverage. It is complete at `4f9debe`, is independent of Plan 091 and Plan 147, changes no daemon/protocol/collector/config/workflow surface, and unblocks Plan 152 only.
152 depends on completed Plan 151's convergent worker and adds EggPool's schema-version-1 authenticated GET /api/status as an independent health plane beside the existing four-metric summary. It is complete at `7b88e43` with existing CI run `37034974349` green across all six jobs. Post-closure review found a local wire-fixture/schema mismatch; Plan 153 owned that correction without reopening Plan 151 or the valid Plan-152 dual-plane architecture, and is now complete. Plan 152 remains independent of Plan 091 and Plan 147, changes no `greggd`/`gregg-protocol`/`EggPool`/config surface, and does not reopen the four summary metric meanings.
153 depends on completed Plan 152/current main and corrects only the schema-v1 consumer contract: canonical nested proxy account counts, `provider_id`, `last_observation`, producer-aligned 96-byte provider IDs / 64-byte reason codes, and an upstream-provenance fixture that prevents a Gregg-local mock shape from masquerading as cross-repo compatibility. It is independent of Plan 091 and adds no dependency, configuration, endpoint, cadence, worker, pane, daemon/protocol, workflow, or release surface. It is complete at `195724c` with `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-targets --all-features`, the focused EggPool/UI/state suites, and `./scripts/check-local.sh` all green, plus existing CI run `37046499078` green across all six jobs. Plan 154 later corrected only the ignored-field examples in the fixture; the Plan-153 production correction remains valid.
154 depends on completed Plan 153/current main and corrects only the canonical fixture's ignored runtime/provider examples to match EggPool's serialized schema-v1 structs. It changes no production decoder/model/behavior and adds no dependency, API, workflow, or release surface. It is complete at implementation `fd2491838eae5b2f168abc0633b10586c690bf23`, with focused EggPool tests, formatting, workspace clippy, full workspace tests, and `./scripts/check-local.sh` green; existing CI run `37053403192` passed all six jobs. It is independent of Plan 091, unblocks no remaining plan, and leaves Plan 091 as the only in-progress plan, gated on its own extended soak record.
150 depends on the current post-149 main state only as its source/CI baseline. It corrected the Windows integration smoke harness exposed by failed CI run `36915516145`: Cargo-provided binary discovery, OS-selected loopback port allocation, bounded file-backed child diagnostics, early child-exit detection, and guaranteed process cleanup. It is complete at `2ceafcd3` with existing CI run `36919813734` green across all six jobs, is terminal in the dependency order (`149 -> 150`), and unblocks no remaining plan. It is independent of Plan 091 and Plans 147-149 semantics and did not change daemon, collector, SCM, protocol, or workflow architecture.
```

Plan 076 is concrete product-correctness work, not a closure-only record. Plan 077 corrected the remaining bounded `croncheck` issues. Plan 078 added separate live-tested product functionality. Plan 079 is justified by a concrete runtime divergence edge found in source review. Plan 080 is separately justified by the observed daemon refusal and direct-stop product requirement. Plan 081 is separately justified by native Windows breakage and a reproducible cross-config Unix stop-targeting defect. Plan 082 is separately justified by a remaining same-file path-spelling identity edge plus contradictory closure/provenance wording; it is not a closure-only record. Plan 083 is separately justified by six concrete client UI/CLI correctness defects enumerated in its own scope decisions; Plan 084 is separately justified by four concrete post-closure findings and is now closed. Plan 085 is separately justified by four narrow client-renderer defects enumerated in its own scope decisions; it is not a closure-only record. Plan 086 is separately justified by three narrow boundary defects found in Plan 085 post-implementation review and is not a closure-only record. Plan 087 is separately justified by the three bounded client-only visual polish behaviors enumerated in its own scope decisions and is not a closure-only record. Plan 112 is separately justified by the installer/uninstaller ownership gap it originally addressed. Plan 113 is separately justified by four concrete post-closure lifecycle defects in Plan 112's implementation: host-global startup ownership, lossy Windows SCM discovery, Cargo-owned early returns, and fallback finalization bypass. Plan 114 is separately justified by the current drive-key mnemonic mismatch and the fleet-wide normal NET-row policy that renders an unavailable bar for hosts whose own snapshot has no network telemetry. Plan 115 is separately justified by the residual host-global restart dispatch, the same-scope user-local Unix daemon activation gap after executable replacement, and platform-wrong Windows elevation guidance discovered after Plan 113 closure. Plan 116 is separately justified by the remaining host-global `greggd update` pre-replacement lifecycle decision, which can stop a foreign Windows SCM service and can suppress restart of a running selected Unix direct daemon. Plan 117 is separately justified by the explicit decision to adopt Rust 1.89 and retire the compatibility-only dependency policy that Plan 105 intentionally retained for Rust 1.75. Plan 118 is separately justified by eggfetch 0.1.5 now providing typed HTTP/HTTPS DNS/refused provenance and bounded body/auth/timeout primitives sufficient to remove Gregg's duplicated reqwest transport machinery without changing application policy. Plan 119 is separately justified by eggfetch-core 0.1.7's lean standard-http1 profile and corrected whole-request timeout behavior. Plan 120 is separately justified by the current source audit identifying repeated deep copies, O(N squared) reducer lookup, repeated render formatting, and per-request immutable snapshot serialization after the transport footprint work settled. Plans 121-123 divide those concrete costs by ownership/risk boundary rather than creating closure-only phases. Plan 124 is separately justified by the post-Plan-123 regression that replaces an already-failed collector message with `cached snapshot is stale` on the status 503 path, plus the need to correct closure wording that claims numerical timing evidence not actually recorded; it does not reopen the valid performance implementation.
Plan 125 is separately justified by the newly published eggfetch-core 0.2.0 line: it preserves the Plan-119 public/feature contract while incorporating upstream private core maintenance and ownership improvements, and the pre-1.0 semver boundary means Gregg's current 0.1.7 requirement will not adopt it automatically. Plan 126 is separately justified by the remaining external-curl maintenance/runtime dependency in shared update mechanics; because moving HTTPS/TLS/proxy transport in-process can materially increase the small `greggd` binary, the work is explicitly evidence-gated and does not presume replacement is beneficial. Plan 127 is separately justified by EggServe 0.2.0's new direct embeddable H1 service/runtime boundary, which now matches greggd's five-route read-only server closely enough to replace an otherwise framework-only Axum layer; the plan remains upstream-gated because Gregg must not lose critical HTTP-task supervision or silently add a hard lifetime to healthy pooled connections. Plan 128 is separately justified by the observed older-Intel loss of drive capacity and network telemetry: Gregg's private `getmntinfo`/`statfs` layout bypassed libc's architecture-sensitive `INODE64` symbol selection and its `AF_LINK` data was cast to the wrong (`if_data64`) structure, so the corrective pass lets libc own the Darwin ABI and proves the v2 optional families on both native macOS architectures. Plan 129 is separately justified by post-closure source review showing that the Plan-128 `NET_RT_IFLIST2` parser applies the full `if_msghdr2` minimum size before discriminating heterogeneous route-message types, so a valid shorter `RTM_NEWADDR`/multicast record can truncate enumeration before later interfaces. Plan 130 is separately justified by the reproducible user-local bootstrap gap: the documented non-root `gregg` path installs to `$HOME/.local/bin` but only advises when that directory is absent from PATH, while the documented privileged `greggd` path normally lands in already-visible `/usr/local/bin`; the corrective pass adds bounded persistent shell integration and a truthful parent-shell activation path without reopening installer ownership. Plan 131 is separately justified by post-closure review showing that Plan 130's broad `grep -F ".local/bin"` classifier can mistake comments or unrelated text for effective PATH integration and therefore skip persistence; the correction is limited to static active-integration recognition and regressions.
Plan 132 is separately justified by the current collector already forming a cohesive native telemetry subsystem whose reusable value is independent of greggd's transport/service layers, while a direct file move would otherwise entangle protocol types, readiness, public paths, and slow-probe semantics. Plan 133 is separately justified because stateful warmup/reset/rate/optional-family behavior must be frozen before extraction to make regressions observable. Plan 134 is separately justified by the protocol/runtime coupling currently preventing the native collectors from being reused outside greggd. Plan 135 is separately justified because source extraction is not complete until greggd consumes the new crate with exact wire/native compatibility and dead duplicate production code is removed. Plan 136 is separately justified by the longer-term BSD goal: FreeBSD's Tier-2 Rust targets plus native sysctl/devstat/ifmib facilities provide a concrete fourth-platform proof without falsely treating Linux/Darwin/FreeBSD as one Unix implementation. Plan 137 is separately justified by post-closure review showing that the FreeBSD loopback smoke can return success when traffic generation or loopback matching fails and only checks non-decreasing counters, while the Plan-136 record calls it a direction proof; the same review found 67 unchecked acceptance boxes across otherwise-complete Plans 132-136 and generic BSD package wording despite only FreeBSD being implemented. Plan 138 is separately justified by the post-137 source review finding residual repeated health serialization, endpoint ownership/URL preparation, native metadata acquisition, sampler handoff, and TUI formatting work after the completed 120-124 campaign. Plan 139 is justified by ready health still deep-cloning/serializing immutable snapshots per request while status already uses publication-time cached Bytes. Plan 140 is justified by URL normalization and multiple Endpoint clones still occurring every generation despite stable configured targets. Plan 141 is justified by repeated Linux CPUFreq/sysfs topology reads and the macOS immutable page-size query, with dynamic network/disk candidates explicitly correctness-gated. Plan 142 is justified as an experiment by Gregg's current-thread runtime plus Tokio's documented spawn_blocking lifecycle characteristics, but may retain the current design. Plan 143 is justified by remaining reducer no-op redraw and cross-render formatting work that can be removed without presentation change. Plan 144 is separately justified by post-closure concurrency review showing that the landed Option-based ready-health memo can execute duplicate serializers when multiple first requests race before memo installation, despite correct sequential behavior. Plan 145 is separately justified by Linux CPUFreq documentation distinguishing dynamic `affected_cpus` (online members) from structural `related_cpus` (online+offline policy membership), which exposes a same-cardinality hotplug invalidation gap in Plan 141's cached weight. Plan 146 is separately justified by post-closure review finding stale `Status: planned` headers/evidence in completed Plans 144-145 plus Plan 145's unnecessary CPU-set helper visibility and an off-by-one/silent-clamp parser policy that treats an 8192-member safety limit as a maximum numeric CPU ID even though Linux CPU numbers need not be contiguous.

Plan 147 is separately justified by dependency currentness on the published
EggFetch 0.2 patch line: Gregg's `version = "0.2"` requirement already admits
it, but the committed lockfile still resolved 0.2.0. It was written against
0.2.1 and retargeted to the then-current 0.2.2 before implementation, so the
plan is deliberately lockfile-first and retains Plan 125's lean
`standard-http1 + tls-rustls` feature boundary rather than reopening client
transport architecture. It closed as a lockfile-only refresh: identical
feature graph, green transport regressions, and a byte-identical stripped
`gregg` binary.

Plan 151 is separately justified by the 2026-10-02 review of the `d31d72f` design: `try_send` plus `Busy` guarantees the input path never waits on a slow worker but can drop a state-changing command, so a visible pane/period/generation can advance while the worker never sees the transition, and a dropped `Deactivate` can leave passive EggPool polling armed after returning to Systems. Plan 152 is separately justified because Gregg consumes only `/api/stats/summary` and therefore cannot distinguish its own local worker lifecycle from whether the configured EggPool proxy and its providers are ready, degraded, or unready.

Plan 155 is separately justified as a coordination roadmap for an operator
need that no existing Gregg surface covers: bounded, local, same-principal
maintenance commands with cron-like schedules and cheap load deferral, without
any remote execution plane. Plans 156-157 are separately justified by the
same-principal/argv/no-shell execution boundary, the five-field local-civil
cron language with DST gap/overlap semantics, the qualified direct-child
lifecycle, and the fixed 5%/128 KiB footprint gate the roadmap set. Plan 158 is
separately justified by two concrete post-implementation defects, not by
ceremony: a per-tick candidate `Vec`+sort and a per-launch job-config clone
that were removable, a fabricated `wall_now + 366 days` schedule fallback that
fabricated semantics for a broken schedule, and a source-review finding that a
syntactically valid expression such as `0 0 31 2 *` can never occur and
therefore failed only after configuration validation had already accepted the
config. Plan 159 is separately justified by the result: Plan 158 measured that
the 131,072-byte gate is 129,104 bytes of irreducible dependency and
configuration-schema cost before any scheduler code, and by Plan 156's own
recorded correction that its 38,424 bytes of headroom omitted the 67,584-byte
`jobs` field. Leaving that undecided would keep a finished feature line open
indefinitely, so the decision is made an explicit, narrow plan with exactly
three recorded outcomes instead of being absorbed by silently checking boxes.
Plan 159 is now complete with outcome 1, and Plans 155, 157, and 158 are
closed against it.

Plan 161 is separately justified by two product-level gaps that the completed
scheduler line intentionally left out: scheduler execution state/output is not
remotely observable, and every Gregg TUI currently owns duplicate Systems and
EggPool polling. The new line keeps remote scheduler control out of scope while
making status/history read-only and bounded, and makes the client daemon a mode
of the existing gregg binary rather than a new distributed component. Plan 162
is a real qualification gate because Plan 159 requires renewed measurement for
any scheduler-line growth beyond 3,261,664 stripped bytes and because bounded
child output introduces concrete pipe/memory/body/security decisions. Plans
163-167 then separate daemon execution/publication, local IPC/state ownership,
user lifecycle, TUI/cache behavior, and measured closure so implementation can
proceed without one cross-cutting mega-plan.

Plan 169 is separately justified by a concrete verification defect, not general
cleanup: Windows-only code is not Clippy-gated in CI, and Plan 168 recorded a
live backlog behind those cfg paths after native verification had already found
serious Windows transport faults. Adding one Clippy step to the existing job is
the smallest durable control. Plan 170 is separately justified by Plan 168's
measured stripped Gregg size of 5,345,864 bytes, +996,128 / +22.90% over the
pre-clientd baseline despite no new dependency graph in Plans 164-166. It is
measurement-first and explicitly allowed to retain the current design if the
cost is intrinsic; source-line count alone is not evidence for a refactor.

Plan 148 is separately justified by the published EggServe 0.4.0 server line:
Gregg's direct `eggserve-server = "0.2"` requirement prevents Cargo from
adopting it, while the published 0.4.0 source still exports the exact direct-H1
surfaces Gregg uses (pre-bound listener builder, service adapter,
split `ServerControl`/`ServerCompletion`, canonical response stream, and
current runtime-limit setters). The minor-line move therefore warrants a
bounded wire/lifecycle/feature/footprint qualification pass without reopening
Plan 127's settled server architecture.

## Execution record for Plan 075

Execution completed:

1. Inspected the existing Windows source, startup paths, foreground smoke, SCM smoke, and relevant documentation.
2. Kept the completed SCM dispatcher, runtime ownership, readiness, and CI architecture unchanged.
3. Truncated `GetComputerNameExW` output using the successful call's returned UTF-16 length.
4. Passed `Some(config.name.as_str())` into native collector construction in foreground and SCM modes.
5. Strengthened the existing Windows foreground and SCM smoke assertions without adding a test harness.
6. Ran focused tests, the default and release local checks, exact Linux CI gates, Rust 1.75 compilation, and one ordinary existing CI run.
7. Recorded the green implementation SHA and workflow run in Plan 075; no corrective phase remained for the Plan 075 scope.

## Verification model

Routine development:

```bash
./scripts/check-local.sh
```

Manual release preflight remains:

```bash
./scripts/check-local.sh --release
```

For Plan 079, focused deterministic local verification was run in addition to the default local check:

```text
cargo fmt --all -- --check
cargo test -p gregg main
cargo test -p gregg scheduler
cargo test -p gregg state
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

The key Plan 079 proof is a bounded scheduler-command channel test that fills capacity, performs a valid endpoint reload, and demonstrates that `ReplaceEndpoints` is delivered rather than silently dropped. A second A -> B -> C test proves convergence to the latest accepted replacement under bounded capacity.

A second external private-LAN smoke was optional for Plan 079 and did not determine completion. Plan 078 already demonstrated the address-replacement path against a live daemon in the environment available at that time. Plan 079 corrected command-delivery semantics deterministically and corrected the record to preserve both the originating `.183`-working/`.182`-stale report and the later `.182`-reachable smoke environment.

Plan 080's original direct lifecycle proof remains valid historical evidence:

```text
greggd run -> croncheck succeeds -> greggd stop -> daemon exits -> croncheck fails
```

Plan 081 closed the post-080 defects: the Ubuntu one-daemon lifecycle smoke and the two-config same-directory stop-isolation smoke both passed against the corrected config-specific control identity. Existing CI run `31813136597` passed Linux, both macOS jobs, Rust 1.75, and Windows; the Windows job completed workspace tests, release `greggd` build, and SCM lifecycle smoke. No new CI job was added. Later run `31813615708` confirms the documentation-only follow-up commit also left current `main` green; repeated green runs are not a standing requirement.

Plan 082 required focused Unix identity tests, the default local check, and one narrow Ubuntu release-binary smoke proving that a daemon started with one ordinary spelling of an existing config path can be stopped with another spelling of the same file. Those checks passed, and existing CI run `31841994426` passed all five jobs. Plan 082 added no workflow/job/matrix requirement.

A plan does not require:

- a dedicated qualification workflow;
- a second Windows job or matrix;
- a self-hosted or privileged runner;
- uploaded artifacts, logs, screenshots, or evidence bundles;
- immutable candidate SHAs or repeated green runs;
- crates.io publication, tags, or GitHub Releases.

## Completion rule

A phase is complete only when its explicit acceptance criteria are implemented and demonstrated by the lightest appropriate mechanism:

- deterministic unit/integration tests;
- the default local check;
- the release preflight only for release-facing changes;
- native platform CI only where native-platform truth is actually required;
- direct local operational smoke where explicitly required;
- direct documentation inspection for scope and behavior claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier commit that no longer matches HEAD.

Plans 081 and 082 are complete because Plan 081's Ubuntu one-daemon lifecycle smoke, Ubuntu two-config stop-isolation smoke, and native CI run `31813136597` all passed, and Plan 082's same-file identity tests, local checks, release-binary relative/absolute smoke, and record reconciliation all passed. Plan 083 is complete because its client behavior and focused tests passed under the default local check and CI run `32094925174`; Plan 084 is complete because its four corrective findings passed the exact local CI-equivalent checks and CI run `32100189772`.

## Closed scope record for Plan 085

Completed:

- compute one fleet-wide `MetricFleetLayout` from every online system with a current normalized snapshot so the normal-view opening `[` and closing `]` columns line up across devices and survive viewport scrolling;
- switch the normal DISK slash denominator from `aggregate.available_bytes` to `aggregate.total_bytes` while keeping the percentage at `used / total` and preserving explicit `available_bytes` for the expanded remaining-space field;
- replace the per-row drive-detail formatter with one selected-system `DriveTableLayout` so expanded mount/used/total/remaining/percent columns stop drifting between rows, with a documented degradation path for narrow terminals;
- introduce one shared `CondensedTableLayout` so condensed headings and value columns always sit in the same terminal cell and `HOST` is the only flexible/truncatable column;
- update active documentation (`README.md`, `crates/gregg/README.md`, `architecture/gregg-client.md`, `.opencode/skills/gregg-client/SKILL.md`, this index) to describe the fleet geometry, the `<used>/<total>` DISK shape, and the shared expanded drive and condensed layouts;
- run focused renderer tests plus the default local check.

Implementation landed in `f8be3cf2` with clippy cleanup in `29945c3`. Post-implementation review found three narrow boundary defects; they are corrected by completed Plan 086 without reopening the daemon, protocol, scheduler, or release architecture.

Preserved exclusions:

- daemon, protocol, scheduler, state/viewport, drive collector, or release-architecture redesign;
- Plan 067 caller-available semantics, normalized drive model, or KiB/MiB/GiB/TiB unit conversion;
- new dependencies, workflows, jobs, matrices, evidence bundles, or self-daemonization;
- rewriting Plan 067, Plan 083, or Plan 084 historical records;
- horizontal scrolling, mouse controls, themes, snapshot/golden tests, or table-framework dependencies;
- unrelated cleanup or scope expansion beyond the four documented display defects.

## Closed scope record for Plan 086

Completed:

- include all visible system names (online/offline/pending) in the condensed `HOST` width budget and decouple `status_line()` width budgeting from the online numeric table so offline/pending rows always retain a recognizable configured nickname or endpoint host alongside `offline`/`pending`;
- centralize the drive-table structural width constants (`DRIVE_INDENT_CELLS`, `DRIVE_GAP_CELLS`, `DRIVE_SLASH_CELLS`) so the fit calculation and the renderer share the same structural cells, and rewrite `compute_drive_table_layout` so Compact considers a truncated name before falling to Minimal;
- thread the fleet `MetricFleetLayout` through `resolve_system_suffixes` (via the shared `metric_prefix_width` helper) so mixed `SWP`/`COMMIT` fleets budget and render suffixes against the same structural prefix width;
- add deterministic condensed/drive/suffix boundary tests covering the three defects, the aligned-position helper-level drive test, and the mixed-platform fleet budget test;
- reconcile Plan 085's status and acceptance checklist once the corrected behavior is demonstrated;
- update this index to reflect Plan 085 closed through Plan 086 and Plan 086 complete.

Preserved exclusions:

- daemon, protocol, scheduler, state/viewport, normalized-capacity, drive collector, CLI, endpoint, dependency, workflow, or release-process redesign;
- Plan 067 caller-available semantics, normalized drive model, or KiB/MiB/GiB/TiB unit conversion;
- new dependencies, workflows, jobs, matrices, evidence bundles, or self-daemonization;
- horizontal scrolling, mouse controls, themes, snapshot/golden tests, or table-framework dependencies;
- rewriting Plan 067, Plan 083, or Plan 084 historical records;
- a closure-only Plan 087.

## Closed scope record for Plan 084

Completed:

- restore `--name` validation parity with inline `nickname@host:port` before config mutation;
- prove final Ratatui `TestBackend` metric-row indentation, bracket alignment, COMMIT geometry, unavailable DISK truthfulness, and width bounds at representative widths;
- calculate offline dot padding from terminal display width and cover a Unicode nickname;
- make live `default_port` comments and documentation describe compatibility-only state for `gregg add` while retaining the field;
- reconcile Plan 083's follow-up wording and close this plan with implementation `020188f` and CI run `32100189772`.
- remove the Rust 1.75-incompatible lint-reason attribute found during the CI-equivalent MSRV check without changing behavior.

Preserved exclusions:

- endpoint parser, scheduler, state/viewport, daemon, protocol, CI, or release-process redesign;
- schema removal or implicit-port `gregg add` behavior;
- new dependencies, workflows, jobs, matrices, or test infrastructure;
- rewriting historical plan records that accurately describe their former behavior.

## Active scope record for Plan 082

Required:

- normalize the Unix control identity for the same existing config file across relative/absolute and symlink/target spellings where supported;
- preserve deterministic distinct identities for genuinely different config files in the same directory;
- preserve missing implicit default-config behavior without requiring the TOML file to exist;
- correct misleading `canonical` naming/comments if raw path bytes remain anywhere in the identity helper;
- add focused identity tests and one narrow release-binary explicit-path lifecycle smoke;
- replace ambiguous Plan 081 `gh run list --limit 1` provenance with exact run `31813136597`;
- reconcile Plan 081 checkboxes only where closure evidence exists;
- keep Plan 080's valid historical Ubuntu record intact;
- keep CI and release machinery unchanged.

Preserved exclusions:

- control-protocol redesign;
- persistent control registry;
- service-manager integration;
- PID/process discovery;
- new dependencies;
- new workflows/jobs/matrices/evidence bundles;
- unrelated refactoring.

## Closed scope record for Plan 083

Completed:

- introduce `crates/gregg/src/ui/bar.rs` width primitives (`truncate_to_cells`, `render_text_line`) and rewrite `crates/gregg/src/ui/system_block.rs` around `MetricRow`, `build_metric_rows`, `MetricGroupLayout`, `compute_metric_group_layout`, and `render_metric_row` so the four normal metric rows share one label width and one bar width with brackets aligned at the same terminal column, plus `make_bar_string`, `render_drive_details`, and a named-versus-unnamed `render_offline` that never duplicates the host after `name@`;
- shorten the aggregate disk suffix to `used / avail` (no `used` or `avail` words) and emit the unavailable `—` marker instead of a fabricated `0.0%`;
- snap `selected_id` and `viewport_top_id` to `display_order()[0]` only on the first accepted poll batch (`last_applied_generation == 0` snapshot) and preserve ordinary selection/viewport behavior thereafter; `Ctrl-R` does not re-snap;
- require an explicit port on `gregg add` and accept `nickname@host:port`, `http://host:port/` URL form, `[ipv6]:port`, and bare `host:port`; reject host-only, URL-without-port, `nickname@host`, `@host:port`, and the ambiguous combination of inline nickname with `--name`;
- keep `--name` as an alternate explicit form, retain HTTP URL credential/userinfo rejection, keep HTTPS downgraded/never accepted, keep host-only `gregg remove HOST` semantics unchanged, and reuse the existing `SystemEntry.name` schema field (no configuration migration);
- add two `crates/gregg/src/scheduler.rs` regression tests proving offline endpoints are polled again on the next generation and recover automatically when the mock becomes reachable, plus a two-generation failure-only assertion that demonstrates the configured endpoint is never silently suppressed;
- update the user-facing documentation surface (`README.md`, `AGENTS.md`, `architecture/gregg-client.md`, `crates/gregg/config.example.toml`, `.opencode/skills/gregg-client/SKILL.md`) to use explicit-port examples and `nickname@host:port`, show the aligned four-space metric block, and forbid future agents from reintroducing implicit-port `gregg add` examples;
- run focused local checks (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`) and the existing ordinary remote CI workflow once.

Preserved exclusions:

- configuration schema additions or migrations;
- daemon, protocol, scheduler architecture, or polling cadence changes;
- offline backoff, retry queue, or exponential-backoff state machine;
- HTTPS acceptance, credential/userinfo reuse, or generalized URL forms;
- new dependencies, workflows, jobs, matrices, or evidence bundles;
- unrelated TUI redesign or test-suite restructuring.

## Closed scope record for Plan 081

Completed:

- restore Windows foreground `greggd run` compilation by introducing a tiny cfg-aware `run_with_control_path_or_default` dispatch helper that uses the Unix-only control wrapper on Unix and the ordinary `run` on Windows;
- replace the directory-scoped Unix control identity with a config-path-scoped FNV-1a digest so two configs in the same directory cannot cross-stop;
- enforce restrictive `0600` control-socket permissions: a failed `chmod` discards the candidate, the foreground entry point returns `ControlSetupError::NoSecureControl` when no secure candidate succeeds;
- narrow stale-socket cleanup to a tiny `stale_connect_error` helper that only `ConnectionRefused` and `NotFound` authorize, after metadata confirms a socket entry;
- add a deterministic A/B cross-stop regression test plus identity, primary/fallback, and permission-path tests;
- preserve Plan 080's valid Ubuntu root-cause and lifecycle record and append a short correction note rather than rewriting history;
- rerun focused local checks, the Ubuntu one-daemon release-binary lifecycle smoke, and a two-config same-directory stop-isolation smoke;
- pass native CI run `31813136597`, including Windows workspace tests, release `greggd` build, and SCM lifecycle smoke.

Preserved exclusions:

- permanent legacy directory-scoped stop fallback;
- Windows named-pipe redesign;
- Unix service-manager coupling;
- new CI infrastructure.

## Closed scope record for Plan 079

Completed:

- make successful Systems endpoint replacement delivery reliable through the existing bounded scheduler command channel;
- use bounded async backpressure or an equivalently small latest-replacement mechanism;
- explicitly handle a closed scheduler command receiver;
- retain the existing state reconciliation, endpoint host/port stale-result guard, scheduler generation model, and immediate replacement poll;
- add deterministic capacity-pressure tests and an A -> B -> C convergence test;
- correct Plan 078 so it separately records the originating `.183`-working/`.182`-stale report and the later `.182`-reachable closure environment;
- focused local checks and direct planning-record updates.

Preserved exclusions:

- an unbounded scheduler command channel;
- filesystem watcher libraries or continuous hot reload;
- a new background config-monitor subsystem;
- scheduler/actor rewrite, generic priority queue, or watch-channel architecture unless a tiny equivalent is strictly smaller than awaited delivery;
- changes to poll concurrency or endpoint schemas;
- TLS/HTTPS polling or changes to URL-form `gregg add`;
- changes to `greggd configprint` or `croncheck`;
- EggPool redesign;
- broad TUI redesign, test-suite restructuring, or unrelated cleanup;
- new workflows, jobs, matrices, artifacts, evidence bundles, or CI gates;
- release automation or publication work;
- a Plan 080 created only to record Plan 079 closure.

## Completed roadmap groups

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed historical baseline; post-closure command/status corrections tracked by Plans 151-152 |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and runtime cleanup | completed; CI run `30964819950` passed |
| [`161-cron-observability-and-client-daemon-roadmap.md`](161-cron-observability-and-client-daemon-roadmap.md) with Plans 162-168 | Bounded read-only `greggd` scheduler observability, the per-config per-user Gregg client daemon that owns all remote polling for multiple TUIs, the plain `c` cron view, terminal-output sanitization, the measured footprint closure, and the cross-platform verification of the Windows transport | closed at `154ab36` with Plans 162-167; **the macOS/Windows/MSRV jobs were not run by that closure**, and running them exposed that the Windows client daemon had never compiled, which Plan 168 corrected and verified |

Plans 010-035 describing retired staged release/evidence work remain archived under `plans/archive/v1.0.1-release/` and are not current requirements.
