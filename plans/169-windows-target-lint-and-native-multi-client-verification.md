# Plan 169: Windows target lint and native multi-client verification

Status: complete. Code at `cc4ea9f`; this closure record at `fe4993d`.

Depends on: completed Plan 168 and current main at
`06bd9f5e6340c58bdfe187e682abeb625a8c97a4`. Independent of the remaining
Plan 091 sustained-soak record.

## Objective

Close the two concrete Windows verification gaps Plan 168 deliberately left
open without reopening the client-daemon architecture:

1. Windows-only code is compiled and tested in CI but is not linted natively,
   so pedantic Clippy findings behind `cfg(windows)` can accumulate unnoticed.
2. The named-pipe transport is exercised natively, and multi-client fan-out is
   proven in shared tests, but two simultaneous native Windows frontends have
   not yet been demonstrated against one real client-daemon transport instance.

This is a bounded corrective/verification pass. It does not redesign IPC,
polling, startup, cron, or release architecture.

## Why this plan is needed

Plan 168 found that the Windows half of Plan 164 had been written but never
successfully compiled before closure. Native CI then exposed several real
transport defects. The repaired Windows job now compiles and executes the
named-pipe tests, but its toolchain step installs no Clippy component and its
job runs no Clippy command.

Plan 168 also records a pre-existing Windows-target lint backlog. Its prose says
"eleven" warnings, while the per-file attribution in the same paragraph sums to
a different count if read literally:

- `gregg-host/src/windows/source.rs`: recorded as seven;
- `greggd/src/startup/install.rs`: recorded as one;
- `greggd/src/update.rs`: recorded as two;
- `gregg/src/bin/lock_helper.rs`: recorded as one;
- `gregg/src/config/store.rs`: recorded as one.

Do not preserve that arithmetic ambiguity. The first implementation step is to
rerun the exact native/cross-target Clippy command and record the authoritative
lint/file/line inventory before editing.

## Scope

### 1. Establish an exact Windows Clippy baseline

Run, at minimum:

~~~text
cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

on the native Windows runner after installing the Clippy component.

A local cross-target `x86_64-pc-windows-gnu` Clippy run may be used as a fast
development aid, but it is not the closure authority for MSVC/Windows-specific
code.

Record:

- exact warning count;
- lint name;
- file and line;
- whether the finding is Windows-only or shared code reached from Windows;
- whether fixing it changes behavior or only expression/style.

If the exact count differs from Plan 168's prose, append a correction note to
Plan 168 at closure rather than rewriting its historical record.

### 2. Fix the existing Windows-target warning backlog

Correct the current findings with behavior-preserving Rust changes.

Expected starting files are the Plan-168 set:

- `crates/gregg-host/src/windows/source.rs`;
- `crates/greggd/src/startup/install.rs`;
- `crates/greggd/src/update.rs`;
- `crates/gregg/src/bin/lock_helper.rs`;
- `crates/gregg/src/config/store.rs`.

Do not add broad `#[allow(clippy::...)]` attributes merely to make the gate
green. A narrowly documented allow is acceptable only where the lint conflicts
with a native API safety/ABI requirement and rewriting would make the code less
correct.

Do not alter telemetry semantics, updater transaction ordering, startup
ownership, lock semantics, or config atomicity as part of lint cleanup.

### 3. Make native Windows Clippy permanent

Extend the existing Windows CI job; do not add another job or matrix.

The Windows toolchain step should install Clippy, then run:

~~~text
cargo clippy --workspace --all-targets --all-features -- -D warnings
~~~

before or alongside the existing test/build steps.

Preserve:

- the existing Windows workspace test;
- release builds of `greggd` and `gregg`;
- the existing SCM/component-safety smoke;
- Linux Clippy as the ordinary all-workspace lint gate;
- current six-job topology.

The purpose is platform visibility, not duplicated CI ceremony.

### 4. Add one native multi-frontend named-pipe proof

Add a deterministic Windows test that uses the real
`\\.\pipe\gregg-client-<id>` transport and one client-daemon instance with
two simultaneously connected frontend clients.

The lightest sufficient proof is:

- one daemon/listener;
- two independent Windows named-pipe frontend connections;
- both complete the version/config handshake;
- both receive the same published frontend generation;
- disconnecting one does not stop the daemon or the other frontend;
- a later publication reaches the remaining frontend.

If the existing daemon test harness can cheaply count remote mock requests,
also assert that adding the second frontend does not create a second Systems or
cron poll. If that would require a second bespoke harness, retain the existing
Plan-167 single-poll-plane proof and use this plan only to prove the native
Windows fan-out transport path.

Do not launch two interactive terminal windows in CI.

## Preserved invariants

This plan must not change:

- same-binary `gregg daemon run` architecture;
- one config-specific daemon per normalized config identity;
- owner-only named-pipe DACL;
- `PIPE_REJECT_REMOTE_CLIENTS`;
- blocking `ConnectNamedPipe` on the blocking pool;
- `PeekNamedPipe` nonblocking read strategy;
- local protocol framing/major version;
- Hello-before-document ordering;
- watch-slot latest-state fan-out;
- one remote polling plane;
- no direct-polling fallback;
- user-scoped client startup;
- cron history/output semantics.

## Verification

Local/development:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Where a Windows cross toolchain is available, also run the whole workspace
Windows target check/Clippy before pushing.

Closure requires one ordinary existing CI run on the implementation SHA with:

- Linux green;
- Windows native Clippy green;
- Windows workspace tests green, including the new multi-frontend test;
- Windows release builds/smoke green;
- both macOS jobs green;
- MSRV green;
- FreeBSD gregg-host green.

Record the exact run ID.

## Documentation

On closure update:

- this plan;
- `plans/README.md`;
- `AGENTS.md` build/verify section so future agents know Windows runs Clippy;
- the relevant Windows/clientd skill if it describes native verification;
- append a correction note to Plan 168 only if its recorded warning count or
  remaining-evidence statement is superseded.

No user-facing README or changelog entry is required for lint-only cleanup
unless the native multi-client test exposes and corrects user-visible behavior.

## Acceptance criteria

- [ ] The exact pre-fix Windows Clippy inventory is recorded.
- [ ] Plan 168's warning-count ambiguity is reconciled truthfully if needed.
- [ ] Existing Windows-target Clippy warnings are fixed or narrowly justified.
- [ ] Windows CI installs Clippy.
- [ ] The existing Windows job runs full workspace Clippy with `-D warnings`.
- [ ] No new workflow job/matrix is added.
- [ ] A native Windows two-frontend named-pipe test passes.
- [ ] One frontend disconnect does not affect the other or stop clientd.
- [ ] Existing Windows pipe/security/liveness tests remain green.
- [ ] Existing Systems/EggPool/cron single-poll-plane tests remain green.
- [ ] Linux/macOS/MSRV/FreeBSD existing CI remains green.
- [ ] No telemetry, updater, config, lifecycle, IPC, or cron behavior is changed
      merely to satisfy lint.

## Stop conditions

Open a narrow corrective follow-up rather than broadening this plan if:

- native Clippy exposes a behavior bug rather than a style/quality finding;
- the multi-frontend Windows test finds a transport/fan-out correctness defect;
- making Windows Clippy green requires a public API change;
- the added Clippy step materially destabilizes CI for infrastructure reasons
  unrelated to Gregg code.

## Handoff

Once complete, Windows-only Rust is continuously linted as well as compiled and
tested. Plan 170 can then measure client-daemon footprint from a clean
cross-platform correctness baseline.

## Closure record

Closure evidence is CI run `37265973270` on the code SHA `cc4ea9f`:
**all six jobs green** — Linux, macOS (arm64), macOS (Intel), Windows,
MSRV 1.89, FreeBSD (`gregg-host` native). `cc4ea9f` is the SHA the acceptance
criteria were demonstrated on; `fe4993d` adds only this record and its
documentation updates.

### The pre-fix inventory is twelve findings, not eleven

Plan 168 recorded "eleven pre-existing clippy warnings" and then attributed them
as seven in `gregg-host/src/windows/source.rs`, one each in
`greggd/src/startup/install.rs` and `gregg/src/bin/lock_helper.rs` and
`gregg/src/config/store.rs`, and **two** in `greggd/src/update.rs`. That sums to
twelve. The word and the arithmetic disagreed, and the ambiguity mattered: a
backlog described as a single number invites the reader to assume it was
enumerated. It was not. The exact inventory, from
`cargo clippy --workspace --target x86_64-pc-windows-gnu --all-targets
--all-features`, is:

~~~text
clippy::borrow_as_ptr            crates/gregg-host/src/windows/source.rs   x7
clippy::unnested_or_patterns     crates/greggd/src/update.rs              x1
clippy::needless_pass_by_value   crates/greggd/src/update.rs              x1
clippy::unnecessary_wraps        crates/greggd/src/startup/install.rs     x1
clippy::borrow_as_ptr            crates/gregg/src/config/store.rs         x1
clippy::borrow_as_ptr            crates/gregg/src/bin/lock_helper.rs      x1
                                                                          ---
                                                                          12
~~~

All twelve are fixed, and all twelve fixes are behavior-preserving:

- the FFI out-parameters in `gregg-host/src/windows/source.rs` and the two
  `LockFileEx` sites become explicit `std::ptr::from_mut` instead of an implicit
  coercion, so the pointer that crosses the FFI boundary is written down;
- `windows_ownership_error` borrows the `ServiceError` it only renders;
- the SCM disposition arms nest their or-pattern;
- `repair_system_config_permissions` keeps its shared `io::Result` signature
  under a documented allow, because `systemd.rs` and `launchd.rs` compile on
  every platform and both map the failure into `InstallError::Io`; making the
  return type platform-dependent would push `cfg` into shared callers.

### Removing two stale allows unmasked two real findings

`config/store.rs` and `bin/lock_helper.rs` each carried
`#[allow(clippy::ptr_as_ptr)]` above their `LockFileEx` call. That allow had
gone stale: the code no longer tripped the lint it named, it tripped
`clippy::borrow_as_ptr`, so the attribute was suppressing nothing and hiding the
fact that `handle as *mut _` was a genuine `ptr_as_ptr` finding. Both are now
`handle.cast()`, and the attributes are gone. A lint allow that outlives its
lint is worse than no allow, because it reads as a decision.

### CI now lints what it compiles

The Windows job installs the `clippy` component and runs
`cargo clippy --workspace --all-targets --all-features -- -D warnings` — the
same full-workspace gate Linux already runs. No new job and no new matrix: the
job count is still six, and the Windows job's own `Test` step still finished in
2m12s against a 2m29s pre-change baseline, so the extra gate did not
destabilize the job.

`AGENTS.md` now states plainly that a local `x86_64-pc-windows-gnu` Clippy run
is a development aid and **only a native Windows CI run is authority**. MSVC
needs the MSVC toolchain and named pipes need a real Windows host; this plan
could not be closed from Linux, and the record should not pretend otherwise.

### The multi-frontend proof is native, and it is what found the real defect

The clientd daemon test harness lived entirely behind
`#[cfg(all(test, unix))]`. It needed that for three lines: a raw-`UnixStream`
byte-ordering probe, a socket-file unlink assertion, and the `tokio::net::UnixStream`
import. The temporary config, the running daemon, the document reader, and the
counting remote are all transport-agnostic. They are now a platform-neutral
`test_support` module, and two Windows tests drive `run_daemon` and `attach` for
real over `\\.\pipe\gregg-client-<id>`:

- `two_frontends_share_one_daemon_and_one_publication_over_real_named_pipes` —
  the endpoint is asserted to be a named pipe in the `gregg-client` namespace;
  two independent connections complete the version and config handshake and see
  the same `daemon_id`; both observe the same publication generation; one
  `Ctrl-R` reaches both as one generation; closing one window leaves the daemon
  answering `status` and the surviving window still receiving; a later
  publication reaches the survivor. The config has no systems, so the only thing
  that can mint a generation is an explicit reload — the test is deterministic
  rather than a race against the poll cadence.
- `a_second_windows_frontend_does_not_add_a_second_polling_plane` — Plan 167's
  request-count proof repeated on the named-pipe transport, so "one polling
  plane" is not carried across platforms by argument alone. A second window,
  with the cron pane open, does not double the metrics cadence, does not re-read
  the scheduler summary, and does not refetch history; closing one of two
  windows does not stop the daemon polling for the other.

### The first native Windows `run_daemon` test found a shutdown defect

This is the substantive finding, and it is why the plan has a follow-up.

On Windows the accept loop parks a blocking-pool thread in `ConnectNamedPipe`,
and `run_daemon` stops it with `accept_task.abort()`. Aborting a task cannot
cancel a `spawn_blocking` job that is already running: the closure owns the pipe
handle and stays parked until a client arrives. `dispatch_daemon` then drops the
`Runtime`, whose shutdown waits for outstanding blocking jobs without a timeout.

So `gregg daemon run` on Windows acknowledges `STOP\n`, unwinds, unlinks the
endpoint — and then hangs instead of exiting. A later `gregg` finds a live pipe
held by a wedged process and attaches to a daemon that is no longer polling.
The Windows SCM lifecycle smoke does not catch it, because it exercises `greggd`
and not `gregg daemon run`.

Plan 168 could not have found this: no test had ever started `run_daemon` on
Windows. It is the same root cause as the lint backlog, in a place lint cannot
reach — **code behind a `cfg` the local loop never compiles is code nobody is
checking**, and this time the cost was not a pedantic warning but a daemon that
cannot be stopped.

The corrective work is **Plan 171**, `171-windows-client-daemon-accept-cancellation-and-clean-shutdown.md`.
It is deliberately not folded in here, per this plan's own stop conditions. What
*is* in this plan is the minimum needed to keep CI honest: `Running::shutdown`
opens one throwaway client so the parked `ConnectNamedPipe` returns, documented
as a test work-around that names Plan 171 and is a no-op on Unix, and every
direct document read in the daemon tests now goes through a bounded helper so a
regression fails a test instead of wedging a CI job. The first run of these
tests wedged the Windows job for over 21 minutes before it was cancelled; the
same step now takes 2m12s.

### Measurements

Plan 169 is byte-neutral, which is the correct outcome for lint and test-only
work, and it confirms the starting point Plan 170 needed. `cargo build
--release -p gregg -p greggd` reproduces bit for bit:

~~~text
gregg    5,345,864 bytes  md5 3dc20315fcbb4afd5d4b1974f7a76c8d
greggd   3,316,568 bytes  md5 86ec567e0b3d8f0d113e3b55dad50f3c
~~~

Both are exactly Plan 168's figures, so the +996,128-byte client growth that
Plan 170 was asked to attribute is unchanged by this plan.

### Acceptance criteria

- [x] The exact pre-fix Windows Clippy inventory is recorded. — twelve
      findings, enumerated with lint names and files.
- [x] Plan 168's warning-count ambiguity is reconciled truthfully. — "eleven"
      corrected to twelve, with the arithmetic shown.
- [x] Existing Windows-target Clippy warnings are fixed or narrowly justified. —
      all twelve fixed, one with a documented platform-signature allow.
- [x] Windows CI installs Clippy.
- [x] The existing Windows job runs full workspace Clippy with `-D warnings`.
- [x] No new workflow job/matrix is added. — still six jobs.
- [x] A native Windows two-frontend named-pipe test passes.
- [x] One frontend disconnect does not affect the other or stop clientd.
- [x] Existing Windows pipe/security/liveness tests remain green.
- [x] Existing Systems/EggPool/cron single-poll-plane tests remain green.
- [x] Linux/macOS/MSRV/FreeBSD existing CI remains green.
- [x] No telemetry, updater, config, lifecycle, IPC, or cron behavior is changed
      merely to satisfy lint. — the release binaries are byte-identical.
