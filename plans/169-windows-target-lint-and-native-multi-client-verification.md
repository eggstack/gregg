# Plan 169: Windows target lint and native multi-client verification

Status: planned.

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
