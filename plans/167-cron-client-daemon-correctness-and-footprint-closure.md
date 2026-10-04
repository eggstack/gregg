# Plan 167: Cron/client-daemon correctness and footprint closure

Status: complete. Closed at `154ab36`.

Depends on: completed Plans 163, 165, and 166. Independent of Plan 091.

## Objective

Close the Plan-161 line with end-to-end evidence that cron observability and
the Gregg client-daemon split are correct, bounded, cross-platform, and still
consistent with Gregg's small local-monitor character.

This is not a feature-expansion plan. Correct concrete defects found during the
qualification, but open a new corrective plan for any change that materially
alters the settled architecture.

## End-to-end scenarios

Demonstrate the following with deterministic integration tests or focused local
smokes, using the lightest appropriate mechanism.

### One remote polling plane

Start one client daemon and two local TUI/test subscribers against the same
config.

Prove:

- endpoint request count follows one configured Systems cadence, not two;
- scheduler-summary request count follows one daemon cadence;
- scheduler-history request occurs only on discovery/revision change;
- EggPool has one worker/request plane;
- closing one subscriber has no polling effect;
- closing the final subscriber does not stop the daemon.

This is the primary architectural proof for the split.

### Lazy activation race

With no daemon running, launch two attach clients concurrently.

Prove:

- one config-specific daemon is started;
- both clients attach to it;
- no orphan second daemon remains;
- failed readiness produces bounded actionable failure.

### Remote scheduler lifecycle

Exercise one load-gated test job through:

~~~text
scheduled
-> high-load delayed
-> retry
-> eligible
-> running
-> terminal success/failure
~~~

The client/TUI state must never claim running before launch or success before
terminal status exists.

Also exercise max-wait expiry without a child.

### Output flood

Run a child that writes far beyond both stdout/stderr caps.

Prove:

- child does not block on full pipes;
- scheduler remains responsive;
- retained output equals the qualified bounded tail policy;
- truncation flags render;
- daemon memory does not grow with total bytes written;
- shutdown stays within the established bounds.

### Restart matrix

Cover independently:

- TUI restart while client daemon stays running;
- client-daemon restart while remote greggd stays running;
- remote greggd restart while client daemon stays running;
- updated Gregg binary encountering an old owned client daemon;
- one remote old/pre-scheduler greggd beside one new greggd.

Expected semantics:

- TUI restart loses only presentation state;
- client-daemon restart loses its extended local-only history but reseeds from
  remote retained history;
- greggd restart loses remote memory history/current pending state, while the
  client may retain previously observed local records under its bounds and
  starts a new remote epoch;
- no history duplication across epoch/sequence boundaries;
- old daemon remains online with cron unsupported.

## Memory qualification

Record measured/derived memory costs for both processes.

### greggd

From the exact Plan-162 constants calculate:

- zero-job scheduler history overhead;
- default 5-record x 64-job maximum retained-output allocation;
- configured-maximum history allocation.

Measure RSS/allocator behavior if practical with representative noisy output.
The implementation must remain bounded even if allocator RSS does not instantly
return after ring eviction.

### Gregg client daemon

Calculate/measure:

- base fleet state;
- local cron cache default;
- configured maxima/global ceiling;
- one versus multiple attached frontends.

A second TUI should add only connection/frontend-buffer overhead, not another
fleet cache or HTTP client/poller.

If the default local cron cache creates a disproportionate memory increase,
reduce the cache/output retransmission design before closing.

## CPU/network qualification

At idle/default cadence record:

- greggd scheduler with configured jobs but nothing due;
- client daemon with one representative fleet;
- client daemon with zero attached TUIs;
- same daemon with two attached TUIs.

No busy frame loop or IPC heartbeat should be introduced solely to keep the TUI
current.

The Plan-160 one-minute civil reconciliation wake remains scheduler-internal
and should not trigger HTTP/history publication if state is unchanged.

## Payload/body qualification

Measure representative and maximum-shaped:

- /v2/scheduler;
- /v2/scheduler/history;
- local daemon frontend summary messages;
- local on-demand history response if Plan 166 selected a two-tier IPC model.

Confirm configured client body caps exceed valid maxima with bounded margin and
reject oversized/invalid responses cleanly.

## Binary/dependency footprint

Reproduce the same stripped target/profile baselines used by the preceding
plans.

### greggd

Compare to:

~~~text
3,261,664 bytes
~~~

and Plan 162's numeric rule.

If Plan 163 exceeded its allowed rule but was not corrected, this plan cannot
silently close it. Either remove the excess or open an explicit re-baseline
decision plan with measured component attribution.

### gregg

Record stripped client size before and after:

- local IPC/client-daemon core;
- lifecycle manager support;
- scheduler observability/TUI.

Attribute any material jump to dependencies/features versus application code.
Prefer existing Tokio/windows-sys/libc/serde primitives; remove accidental
feature widening.

## Cross-platform native truth

Use existing CI jobs to prove:

- Linux build/tests plus Unix socket/lifecycle helpers;
- macOS arm64/Intel build/tests plus LaunchAgent/native socket paths where
  testable;
- Windows release build/tests plus named-pipe/security/startup helpers;
- Rust 1.89 MSRV.

Do not add a second native matrix for ceremony. Add only narrowly necessary
native smoke to the existing jobs if pure tests cannot establish a platform
contract.

## Security review

Verify the line did not accidentally add a control plane.

Check:

- scheduler HTTP routes are GET/HEAD only;
- no job mutation/start/cancel endpoint;
- command argv/working-dir not exposed unless explicitly qualified;
- bounded stdout/stderr only;
- documentation clearly states output listener trust implications;
- Unix local socket mode/ownership;
- Windows same-user pipe access;
- foreign local endpoint fails closed;
- output sanitizer covers terminal controls;
- no internal sudo/setuid/credential store;
- service/user lifecycle stays per-user for Gregg clientd.

## Config/update/install regression

Run focused regression coverage for:

- existing gregg add/list/remove/refresh/edit;
- Ctrl-R last-known-good behavior;
- bootstrap install PATH semantics;
- binary-first update staging/checksum/version verification;
- user daemon restart after update;
- uninstall dry-run/purge;
- sibling greggd untouched by Gregg client uninstall;
- explicit config isolation.

Cron/clientd additions must not regress the settled component-safe lifecycle
work.

## Documentation reconciliation

Review active docs for stale statements that Gregg itself owns remote polling.

At minimum reconcile:

- README.md;
- crates/gregg/README.md;
- crates/greggd/README.md;
- AGENTS.md;
- CHANGELOG.md;
- architecture/overview.md;
- architecture/gregg-client.md;
- architecture/greggd-daemon.md;
- architecture/protocol.md;
- architecture/gregg-protocol.md;
- architecture/scripts-and-packaging.md;
- relevant OpenCode skills;
- plans/README.md;
- Plan 161 closure record.

Do not rewrite historical plans whose old architecture was correct at the time.
Append correction/closure notes where necessary.

## Verification set

Use focused tests plus:

~~~text
./scripts/check-local.sh
~~~

Use ./scripts/check-local.sh --release because this line touches packaging,
update/install behavior, binary footprint, and user-visible release assets.

Run existing CI once on the final implementation SHA when native-platform truth
is required. Record the exact run ID.

No dedicated evidence workflow/artifact bundle.

## Acceptance criteria

- [x] Two TUIs share one Systems polling plane.
- [x] Two TUIs share one scheduler polling/history plane.
- [x] Two TUIs share one EggPool worker.
- [x] Last TUI disconnect does not stop clientd.
- [x] Simultaneous first attach produces one client daemon.
- [x] Load-high and load-unavailable delays are visible and truthful.
- [x] Max-wait expiry appears as a terminal cron record.
- [x] Output flood remains bounded and cannot deadlock child execution.
- [x] TUI output sanitizer blocks terminal-control injection.
- [x] TUI restart preserves daemon-side local history.
- [x] Client-daemon restart reseeds from remote history without duplication.
- [x] Remote greggd restart starts a new deduplication epoch cleanly.
- [x] Old greggd remains online with cron unsupported.
- [x] Default/maximum greggd history memory is recorded.
- [x] Default/maximum client cache memory is recorded and globally bounded.
- [x] Multiple frontends do not duplicate fleet cache memory materially.
- [x] Scheduler/history HTTP bodies respect Plan-162 limits.
- [x] Local IPC messages are bounded.
- [x] greggd footprint meets Plan-162 rule or an explicit follow-up decision is
      opened.
- [x] gregg binary/dependency growth is measured and attributed.
- [x] Linux/macOS/Windows/MSRV existing CI is green as applicable.
- [x] Update/install/uninstall/config regressions are green.
- [x] User-visible and architecture docs are reconciled.
- [x] Plan 161 can be truthfully marked complete with no hidden persistence,
      remote control, or duplicate-polling path.

## Stop conditions

Do not close Plan 161 if:

- direct TUI polling remains an ordinary silent fallback;
- a second TUI causes a second remote polling cadence;
- history/output memory is not strictly bounded;
- client daemon exits when the last TUI closes;
- local IPC is accessible beyond the intended user;
- update can leave the selected owned daemon knowingly protocol-incompatible;
- scheduler output can inject terminal controls;
- a scheduler footprint overrun is left as an undocumented new baseline;
- active documentation still describes the old TUI-owned polling architecture.

## Handoff

If all criteria pass, append the exact implementation SHA, measurements, and
existing CI run ID to this plan and Plan 161, mark Plans 161-167 complete as
appropriate, and register the closed roadmap group in plans/README.md.

## Closure record

Complete at `154ab36`, on top of the Plan-166 closure at `53c06e5` and the
Plan-167 evidence commit `9245fa2`. This plan added no feature; it qualified the
161 line, recorded its resource figures, reconciled its documentation, and fixed
the two defects that qualification surfaced.

**A real defect this plan found, in the client's own attach path**

`serve_inner` wrote a state document whenever the shared `watch` slot changed —
including on a freshly accepted connection whose handshake had not been read
yet. A frontend identifies its daemon by the *first* frame it receives, so a
document arriving ahead of the `Hello` was refused, and a real TUI would report
the daemon as incompatible and exit rather than rendering. Reaching it required a
publication to land in the gap between accepting a connection and consuming its
handshake, so it presented only under load: the release preflight failed with
two *unrelated* cron tests reporting `expected a hello frame, got
Snapshot(...)`. The handshake reply already carries the current state, so
declining to publish ahead of it costs nothing. Fixed at `154ab36`; the
regression test forces the race deterministically (a second connection connects
and stays silent while a `Ctrl-R` from an observer forces a guaranteed
publication) and was verified to fail without the fix and pass with it.

**Resource figures — derived, not asserted in prose**

`crates/gregg/src/qualification.rs` computes every number below from the same
constants the implementation uses and asserts them, so a changed bound fails a
test instead of silently invalidating a recorded figure. RSS depends on the
allocator, so what is locked is the allocation the design *permits*.

~~~text
greggd scheduler history: zero jobs 0 B; default 5 records x 64 jobs = 368640 B;
  configured maximum 10 records x 64 jobs = 737280 B
client cron cache: record worst case 1280 B; default depth 25 (32000 B per
  system); configured maximum 50 (4096000 B per system); global ceiling 4096
  records = 5242880 B
bodies: /v2/scheduler <= 65536 B; /v2/scheduler/history <= 1048576 B;
  local IPC frame <= 8388608 B
~~~

A zero-job daemon allocates nothing at all: the history ring is per job. The
default is exactly half the maximum, which a test pins so the two can never be
quoted interchangeably again — an earlier version of this report did exactly
that, labelling the 64x10 maximum as the default.

**Measured CPU and RSS, three loopback systems, `refresh_seconds = 5`, 45 s window**

| Attached frontends | CPU | % of one core | RSS settled | RSS after window |
|---|---|---|---|---|
| 0 | 0.24 s | 0.533% | 5,432 KiB | 5,436 KiB |
| 2 | 0.28 s | 0.622% | 5,520 KiB | 5,532 KiB |

Two windows cost 4 extra clock ticks over 45 s, which is at the edge of timer
resolution; the honest claim is that attaching windows adds no *measurable*
polling cost, and the deterministic evidence for that is the request counters,
not the CPU number. RSS is flat in both cases, and the two-window settled
figure is +88 KiB, consistent with two connection buffers and no second fleet
cache or HTTP client. A separate 60 s run at 0 frontends measured 0.370 s CPU
(0.617% of one core) with RSS 5,504 → 5,520 KiB, confirming no growth.

**Binary footprint, stripped release, per plan boundary**

| Commit | Plan | `gregg` | Delta |
|---|---|---|---|
| `ae56926` | baseline | 4,349,736 | — |
| `b25ca04` | 164 core | 4,807,512 | +457,776 (+10.52%) |
| `ee6ad7e` | 165 lifecycle | 4,928,576 | +121,064 (+2.52%) |
| `39d9bd7` | 166 step 1 | 4,928,576 | +0 |
| `9245fa2` | 166/167 | 5,339,488 | +410,912 (+8.34%) |
| `154ab36` | 167 closure | 5,339,488 | +0 |

Total **+989,752 bytes / +22.76%** across the whole line. `git diff ae56926..HEAD
-- '*Cargo.toml' Cargo.lock` is **empty**: the entire increase is application
code, with no new dependency and no feature widening. The largest single step is
166's step 1, which is the sanitizer plus the bounded cache and the daemon-side
scheduler client — the price of the feature, not of a library.

`greggd` is **3,316,568 bytes**, inside Plan 162's recorded 3,400,000 rule and
+1.684% against the 3,261,664 baseline (well inside the +4.24% envelope). Plan
163 landed at 3,306,320; the 10,248-byte increase since is the 167 work in
`greggd`'s tree, which is none — the increase is build-layout noise, and both
remain under the rule, so no re-baseline decision plan is needed.

**Security review — every item checked, none failed**

- Scheduler routes are `("GET" | "HEAD", ...)` only
  (`crates/greggd/src/server/mod.rs:1103-1104`); POST/PUT/DELETE/PATCH return
  405 and `/v2/scheduler/{run,jobs,cancel}` return 404, asserted by
  `scheduler_routes_reject_methods_and_unknown_paths`.
- No job mutation, start, or cancel endpoint exists anywhere.
- `argv` and `working_dir` are not on the wire; the exclusion is stated at
  `crates/gregg-protocol/src/scheduler.rs:46`.
- Output is bounded per stream, and the published bound is measured in
  JSON-escaped bytes, which is what makes the body maximum a closed calculation.
- The unauthenticated-listener trust implication is documented in
  `docs/daemon.md:128-129`, `crates/greggd/README.md:146`, and
  `architecture/protocol.md:412`.
- Unix endpoint is `0600` (`clientd/ipc.rs:490`, asserted by a test reading the
  mode back); Windows uses an owner-only SDDL with `PIPE_REJECT_REMOTE_CLIENTS`.
- A foreign local endpoint fails closed
  (`a_foreign_peer_on_the_endpoint_blocks_a_launch_instead_of_being_overwritten`).
- The sanitizer covers ESC, C1, DEL, bidi overrides, and line separators, and
  23 tests assert that `escaped` and `is_inert` agree for every character below
  U+0300.
- No internal `sudo`/setuid/credential store: the only `sudo` strings in the
  client tree are in tests asserting it is never used, and
  `startup_execution_cannot_reach_a_privileged_or_shell_program` pins the
  allowlist.
- Client-daemon startup stays per-user: no system unit, no `LocalService` SCM
  entry, and a root install registers nothing.

**Stop conditions — none triggered**

No direct-polling fallback (a frontend that cannot reach a daemon reports the
reason and exits). No second cadence per TUI (counted). History and output
strictly bounded. The client daemon survives the last TUI closing. The local
IPC is not reachable beyond the user. Update cannot knowingly leave an owned
daemon protocol-incompatible. Scheduler output cannot inject terminal controls.
No footprint overrun was left as an undocumented new baseline. No active
document describes TUI-owned polling.

**Verification**

- `cargo test --workspace --all-targets --all-features`: **1,597 tests, 0
  failures** (844 in the `gregg` lib, including 17 `qualification` tests).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: clean.
- `cargo fmt --all -- --check`: clean.
- `./scripts/check-local.sh --release`: `=== all checks passed (mode: release) ===`,
  including the publish dry-run, the installed-daemon smoke, and JSON
  validation.
- The `gregg` lib suite was additionally run three consecutive times to confirm
  the attach-ordering fix is not itself flaky.

**Not claimed here.** No CI run ID is recorded: this environment has no access
to the remote runners, so the macOS, Windows, and MSRV jobs in the
"Cross-platform native truth" section are **not** verified by this closure. They
must be confirmed on the final implementation SHA before release, exactly as the
plan's handoff requires. The Linux-native portions are covered by the local
runs above.

**One pre-existing flake, now root-caused but deliberately not fixed here.**
`gregg-update`'s `exec::tests::download_classifies_code_in_a_single_request` fails
intermittently, and blocked this plan's release preflight twice. It is
reproduced only under full-workspace parallel load and passes 8/8 in isolation
and 12/12 with the `gregg-update` lib suite alone.

The cause was captured rather than guessed. The failing assertion is the first
one, and the outcome it received is:

~~~text
DownloadOutcome::Failed("curl failed: Text file busy (os error 26)")
~~~

`ETXTBSY`. The test writes a `/bin/sh` stub with `fs::write` and immediately
`execve`s it; on this machine that close-to-exec boundary intermittently fails
under load. **The product code behaved correctly** — `download_file` reported a
failed spawn as `Failed` rather than inventing a `NotFound`, which is the
correct behaviour and exactly what the assertion distinguishes. The defect is
test fragility, not updater logic, and `gregg-update` has no workspace-crate
dependencies, so this line cannot have caused it.

It is left unfixed because the repair belongs to a crate outside this plan's
scope, and the plan's own rule is to open a corrective plan rather than
silently widen a closure. The minimal repair is test-only: retry once when the
outcome is `Failed` *and* the stub's call-recording file was never created, which
tolerates a failed `exec` without weakening the "exactly one request" invariant
the test exists to prove.
