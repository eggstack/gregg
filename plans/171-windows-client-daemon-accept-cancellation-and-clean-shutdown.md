# Plan 171: Windows client-daemon accept cancellation and clean shutdown

Status: complete. Code at e3f0694.

Depends on: completed Plan 169, whose native two-frontend named-pipe test
discovered this. Independent of Plan 091.

Opened as the narrow corrective follow-up Plan 169's stop conditions require.
Plan 169's own test work-around is explicitly temporary and is named below.

## Objective

Make `gregg daemon run` terminate on Windows.

Today it does not: the daemon acknowledges a stop request (or a supervisor
signal), unwinds its engine loop, unlinks the endpoint, returns from
`run_daemon` — and then the process hangs instead of exiting.

## The defect, exactly

`clientd::ipc`'s Windows listener parks its wait for the next client in
`connect_instance`:

~~~rust
let Stream::Windows(pipe) = stream;
tokio::task::spawn_blocking(move || {
    let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut()) };
    ...
})
~~~

The accept loop is therefore suspended inside a `spawn_blocking` job the
overwhelming majority of the time — that is the whole point of Plan 168's
design, because `ConnectNamedPipe` on a synchronous handle has no timeout and
must not run on the current-thread runtime.

`run_daemon` stops the loop with:

~~~rust
tasks.cancel();
accept_task.abort();
~~~

`JoinHandle::abort` cancels the *task future*. It cannot cancel a
`spawn_blocking` job that is already running. The closure moved the `File` that
owns the server pipe handle into itself, so aborting drops the future while the
thread stays parked in `ConnectNamedPipe`, waiting for a client that will never
come.

`dispatch_daemon` in `cli.rs` then drops the `Runtime` at the end of the
function. Tokio's runtime shutdown waits for outstanding blocking-pool jobs, and
there is no timeout, so the drop — and therefore process exit — blocks forever.

The `Listener`'s own `pending` instance is *not* a way out: it is a different
handle, and the parked one is unreachable from outside the closure.

### Observable effect

- `gregg daemon stop` prints its acknowledgement; the daemon process stays.
- Ctrl-C and a supervisor stop signal behave the same way.
- The next `gregg` finds a live pipe held by a wedged process, so a TUI attaches
  to a daemon that is no longer polling, with no error to explain why.
- The Windows SCM lifecycle smoke does not catch it, because it exercises
  `greggd`, not `gregg daemon run`.

## Why it survived Plan 168

`mod tests` in `clientd/daemon.rs` was `#[cfg(all(test, unix))]`, so no test had
ever started `run_daemon` on Windows. The transport had tests; the daemon that
uses it did not. Plan 169 moved the harness into a platform-neutral
`test_support` module and added the first native Windows `run_daemon` tests, and
this surfaced immediately.

## Scope

Fix the accept path's cancellation and prove the process exits. Do not redesign
the local transport, change the handshake, change the frame format, or change
any polling, ownership, or security behavior.

Preserve:

- one accepted client per pipe instance, with the instance rotated on accept;
- the owner-only SDDL on every instance;
- `PIPE_REJECT_REMOTE_CLIENTS` staying a pipe-mode flag;
- the blocking-pool placement of the wait, so a current-thread runtime is never
  stalled by a client that never connects;
- the existing `WouldBlock` / `Disconnected` arms in `accept_loop`;
- Unix behavior, which is already correct.

## Options, in the order they are worth considering

1. **Bound the runtime shutdown.** Build the `daemon run` runtime with
   `Builder::shutdown_timeout(...)` so a blocking job that outlives the engine
   cannot hold process exit. Smallest change, but it abandons the thread rather
   than stopping the wait, and it does not help a daemon running under a
   supervisor that reaps it for other reasons.

2. **Cancel the wait explicitly.** Have the accept task retain a way to
   interrupt the parked thread: capture the blocking thread's id inside the
   closure and use `CancelSynchronousIo` at shutdown, or switch the instance to
   `FILE_FLAG_OVERLAPPED` and use `CancelIoEx` plus an event. This stops the
   wait rather than abandoning it. Plan 168 deliberately avoided
   `FILE_FLAG_OVERLAPPED` because the transport reads with `PeekNamedPipe`; that
   constraint must be re-checked, and overlapped `ReadFile` would be a larger
   change than this plan should make.

3. **Stop blocking on the accept at all.** Replace the parked
   `ConnectNamedPipe` with a poll that the accept loop drives from a cancellable
   async wait, so `tasks.cancel()` is observed normally. The cleanest end state
   and the one that removes the blocking-pool dependency, but it is a real
   transport change and must not regress the single-instance-at-a-time
   semantics or the ownership proofs Plan 168 added.

Pick one, justify the choice in the closure record, and keep the temporary
`release_parked_accept` test work-around only for as long as the fix does not
cover every test's teardown path.

## Acceptance criteria

- [x] `gregg daemon run` on Windows exits after `gregg daemon stop`, after a
      supervisor stop signal, and after Ctrl-C. All three now converge on one
      teardown, which required fixing the `select!` in `cli.rs` as well.
- [x] The exit is prompt, not "eventually after a client connects". The
      regression test never connects a client, so there is no "eventually" for
      it to recover on.
- [x] The parked blocking-pool job ends; no thread is abandoned at shutdown.
      `CancelSynchronousIo` ends the wait; the bounded `abort` is a last-resort
      that the accepted cancellation never needs.
- [x] A native Windows test drives `run_daemon` to completion and asserts the
      runtime is released, so this cannot regress silently.
- [x] The temporary `release_parked_accept` work-around in
      `clientd/daemon.rs`'s test harness is removed, or reduced to a case the
      fix genuinely does not cover. **Removed**; `Running::shutdown` now asserts
      the daemon exits.
- [x] Owner-only SDDL, `PIPE_REJECT_REMOTE_CLIENTS`, instance rotation, and the
      `Hello`-first contract stay green.
- [x] Two-frontend fan-out and single-poll-plane tests stay green.
- [x] Unix behavior and tests are unchanged.
- [x] `cargo fmt`, workspace Clippy, and `./scripts/check-local.sh` are clean, and
      the six-job matrix is green on the final SHA.

## Stop conditions

Open a further follow-up rather than broadening if:

- the only workable fix requires moving the transport to overlapped I/O *and*
  that changes `PeekNamedPipe` read semantics — that is a transport redesign,
  not a shutdown fix;
- a fix would let more than one client bind a single pipe instance;
- a fix would weaken the owner-only SDDL or drop
  `PIPE_REJECT_REMOTE_CLIENTS`.

## Handoff

Once complete, Windows client-daemon lifecycle is genuinely verifiable end to
end, and Plan 169's "code behind a `cfg` the local loop never compiles" argument
has a second, non-lint example behind it.

## Closure record

### Option chosen: option 2, stop the wait

`CancelSynchronousIo`, not `Builder::shutdown_timeout` and not an overlapped-I/O
rewrite.

- **Option 1 was rejected on its own acceptance criteria.** The plan requires
  that "the parked blocking-pool job ends; no thread is abandoned at shutdown". A
  shutdown timeout does the opposite by definition: it abandons the thread and
  lets the process exit anyway. It also would not have helped a daemon a
  supervisor reaps for an unrelated reason, which the plan called out.
- **Option 3 was out of scope by the plan's own stop conditions.** Making the
  accept non-blocking requires `FILE_FLAG_OVERLAPPED`, and the plan's first stop
  condition says that moving to overlapped I/O *and* changing `PeekNamedPipe`
  read semantics is a transport redesign, not a shutdown fix. The read path
  deliberately depends on a synchronous handle: `PeekNamedPipe` supplies the
  count and `ReadFile` is then asked for at most that many bytes so it has
  nothing left to wait for. Overlapped `ReadFile` would put a second
  cancellation surface on the hot read path of every attached frontend, for a
  defect that is entirely in the accept path.

`ConnectNamedPipe` on a synchronous handle is synchronous I/O, and it is **not**
one of the four functions Win32 lists as uncancellable
(`CopyFile`, `MoveFile`, `MoveFileEx`, `ReplaceFile`). `CancelSynchronousIo` is
the documented mechanism for interrupting exactly this.

Two facts about the API shaped the implementation, and both were checked against
the SDK documentation rather than assumed:

- It takes a **thread handle**, not a file handle and thread id. The parked
  closure duplicates a real `THREAD_TERMINATE` handle to itself and publishes
  that, because the cancellation applies to whatever synchronous I/O *that
  thread* currently has pending.
- It cancels only I/O that is **already pending**, and the request is **not
  remembered** for later. A cancel that races ahead of the thread entering the
  kernel finds nothing and is lost.

### Why the handle, and not the thread id

A thread id is recycled once a thread exits, so cancelling by id can interrupt
some unrelated thread that was handed the same number. A duplicated handle
refers to the thread object itself, so it can only ever name the parked accept.
This is also the only reason `ipc.rs` now carries the workspace's single
`unsafe impl Send`: a `HANDLE` is `*mut c_void` and so is not `Send`, while the
duplicate is created on the blocking-pool thread and read by the task that
requests the stop. Every access to it is serialized by the gate's lock, and the
handle is created, read, and closed exactly once each. AGENTS.md's unsafe
allowlist entry was updated to name it.

### The three races, and how each is closed

This was the part that made the fix more than "usually right", and each point is
a place a later edit could quietly reintroduce the hang.

1. **A wait that starts after the stop.** The request is recorded in an atomic
   *before* the parked record is inspected, and `begin` re-checks it **while
   holding the same lock** the requester takes. Exactly one ordering holds: either
   the store is visible and the wait declines to park, or it is not yet visible
   and the requester's lock acquisition is still to come, so it will find the
   record. There is no interleaving in which a wait parks with nothing left to
   cancel it.
2. **Cancelling a thread that has moved on.** `CancelSynchronousIo` cancels
   *whatever* that thread has pending, so aiming it at a thread already back on
   the blocking pool doing unrelated work would be a real bug. The lock is held
   across the call: the parked closure can only return by reaching `end`, which
   needs that same lock, so while the requester holds it the recorded thread is
   provably still the parked one. The lock is released *between* attempts, which
   is what lets the thread make progress instead of deadlocking against the
   canceller.
3. **A cancel that lost the race to enter the kernel.** Per the API's own
   semantics this is genuinely lossy, so `request` re-issues while the record is
   up, bounded at `ACCEPT_CANCEL_ATTEMPTS` (256) and yielding between attempts so
   a single-core runner is not starved out of the wait it is supposed to enter.
   The bound makes it impossible for this to become an unbounded spin, and the
   loop still terminates on the `None` case.

`ERROR_OPERATION_ABORTED` is mapped to `TransportError::Disconnected` rather than
a connection. No client attached, and handing the pipe on would give the accept
loop a dead connection to serve. The loop's pre-existing `Disconnected` arm
tolerates it without giving up, and its existing cancellation check ends it on
the next pass — so `WouldBlock` and `Disconnected` are preserved exactly as the
plan required.

### A second instance of the same defect, in `cli.rs`

Fixing the transport was not sufficient to satisfy the first acceptance
criterion, because the signal path never reached the transport fix. `select!`
cancels its losing branch, so on `shutdown_signal` the old code called
`cancel.cancel()`, returned `Ok(())`, and **dropped `run_daemon`'s future** —
skipping `tasks.cancel()`, the accept stop, and `ipc::cleanup` entirely. The
supervisor stop and Ctrl-C paths therefore hung for a second, independent reason,
and never even released the endpoint. The branch now cancels and then awaits the
daemon, so all three stop paths converge on one teardown.

This was found while implementing, not by the plan; it is recorded here because
the acceptance criteria are about behaviour and would have been only partly met
without it.

### Preserved

One accepted client per instance with rotation on accept, the owner-only SDDL on
every instance, `PIPE_REJECT_REMOTE_CLIENTS` as a pipe-mode flag, no
`FILE_FLAG_OVERLAPPED`, the blocking-pool placement of the wait, the existing
`WouldBlock`/`Disconnected` arms in `accept_loop`, the `Hello`-first contract,
the frame format, and every Unix code path. `AcceptStop` is a no-op on Unix and is
exported from both transports, so the daemon's shutdown sequence carries no
`#[cfg]` — consistent with the rest of `ipc.rs` and with the module's stated
rule that neither the daemon nor the frontend carries a `#[cfg]` of its own.

One deliberate consequence: on Unix the accept loop now unwinds by observing its
cancellation token and returning, rather than being aborted. It reaches that
check within the existing 10 ms `WouldBlock` sleep, so the two are
indistinguishable from outside, and it avoids a second abort path in the daemon.

### Evidence

Native Windows tests, all with **no client in sight**, which is what makes the
accept genuinely parked rather than incidentally so:

- `a_stop_request_ends_a_parked_accept_wait` — the "stop before the wait starts"
  ordering, which the recorded request settles on its own.
- `a_stop_request_interrupts_an_already_parked_accept` — the ordering the defect
  actually lived in: the accept is already blocked in `ConnectNamedPipe` when the
  stop lands, so the cancellation is the only thing that can release the thread.
  It sleeps 250 ms first so it cannot pass by the path the first test covers.
- `a_stopped_daemon_releases_its_runtime` — the plan's regression test. It
  builds the same current-thread runtime `dispatch_daemon` builds, drives a real
  daemon over a real named pipe, stops it the way an operator does, and then
  drops **that runtime** on its own thread under a deadline. The drop is the
  assertion, because the drop is where an uncancellable accept wait parked
  forever. It cannot simply be awaited: a drop that never returns is the defect,
  and waiting on one in-line is indistinguishable from a hung test, which is
  exactly how Plan 169's first run wedged the Windows job for over 21 minutes.
  Handing it to a thread turns that into a named assertion.

`Running::shutdown` now asserts the daemon really exits instead of discarding a
timeout, and `release_parked_accept` is deleted rather than reduced — with the
wait cancellable there is no case left for it to cover, and leaving it would
re-introduce a way to make a regression look like a pass.

Preserved green: the two-frontend fan-out and single-poll-plane tests, the
transport's existing Windows tests, and all 94 `clientd` tests on Linux.
`./scripts/check-local.sh` passes, and `cargo clippy --workspace --all-targets
--all-features` is clean both natively and cross-checked for
`x86_64-pc-windows-gnu`.

Per the repository's own rule, that cross-target Clippy run is a development aid
and not closure evidence; the six-job matrix on the final SHA is, and only that.

### CI

**Closure evidence: run `37271258556` on `d402c72`, all six jobs green.** The
code is unchanged from `e3f0694`; `d402c72` adds only this closure record and the
follow-up plan.

Windows is the job that matters, and the three new tests passed natively:

~~~
test clientd::ipc::windows_tests::a_stop_request_ends_a_parked_accept_wait ... ok
test clientd::ipc::windows_tests::a_stop_request_interrupts_an_already_parked_accept ... ok
test clientd::daemon::windows_tests::a_stopped_daemon_releases_its_runtime ... ok
test clientd::daemon::windows_tests::two_frontends_share_one_daemon_and_one_publication_over_real_named_pipes ... ok
test clientd::daemon::windows_tests::a_second_windows_frontend_does_not_add_a_second_polling_plane ... ok
~~~

alongside the four pre-existing transport tests, so the owner-only SDDL,
`PIPE_REJECT_REMOTE_CLIENTS`, instance rotation, and the `Hello`-first contract
are demonstrated still green on the native MSVC runner. The Windows `Test` step
took 2m15s against a 2m12s pre-change baseline: the new tests cost nothing
measurable, and the step finished rather than wedging the way Plan 169's first run
did at over 21 minutes. `Clippy`, both release builds, and the SCM lifecycle smoke
were green too.

#### An unrelated intermittent, recorded rather than glossed over

The first run of this SHA, `37270317172` on `e3f0694`, was green on five of six
jobs — including Windows — and failed MSRV at
`exec::tests::download_classifies_code_in_a_single_request` in `gregg-update`,
panicking at `exec.rs:785` with `ENOENT` on a `calls` file. It passed on the next
run of the identical code, so it is intermittent and **not** a Plan 171
regression:

- `gregg-update` is a separate crate; Plan 171 changed `gregg` and one
  `windows-sys` feature. Nothing in `gregg-update` depends on `clientd`.
- The same commit ran the identical test green on Linux, both macOS jobs,
  Windows, and FreeBSD.
- Root cause is a test-quality defect, not product behavior: the assertion is
  `DownloadOutcome::Failed(_)`, which also matches the `Err(e)` **spawn
  failure** arm at `exec.rs:402`. A transient `EAGAIN` fork failure under
  loaded-runner parallelism therefore satisfies the HTTP-500 assertion and then
  panics `ENOENT` reading a file the stub never wrote. The two other `exec`
  tests that spawn real children — one deliberately keeping a hanging child
  alive — appear in the log immediately before the failure.
- The 100-second `DOWNLOAD_WALL_TIMEOUT` is not involved; the stub is a
  two-line `/bin/sh` script.

The diagnosis and the fix are registered as **Plan 172**, exactly the narrow
follow-up this plan's stop conditions call for, rather than a change to
`download_file`'s classification smuggled in here. Locally, `./scripts/check-local.sh`
passes and the full workspace suite is 1,598 tests green on both stable and the
MSRV toolchain.

### Stop conditions

None triggered. The fix does not move the transport to overlapped I/O, does not
let more than one client bind an instance (rotation and the single-client-at-a-
time invariant are untouched), and weakens neither the owner-only SDDL nor
`PIPE_REJECT_REMOTE_CLIENTS`.

### Dependency note

The only dependency surface added is the `Win32_System_Threading` feature on the
existing `windows-sys`, for `THREAD_TERMINATE`. `CancelSynchronousIo` itself is
already in `Win32_System_IO` and needed nothing, and its windows-sys 0.59
binding is correct for the one-argument thread-handle form. No new crate, no
version change, and the lockfile is otherwise untouched.
