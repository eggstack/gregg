# Plan 171: Windows client-daemon accept cancellation and clean shutdown

Status: planned.

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

- [ ] `gregg daemon run` on Windows exits after `gregg daemon stop`, after a
      supervisor stop signal, and after Ctrl-C.
- [ ] The exit is prompt, not "eventually after a client connects".
- [ ] The parked blocking-pool job ends; no thread is abandoned at shutdown.
- [ ] A native Windows test drives `run_daemon` to completion and asserts the
      runtime is released, so this cannot regress silently.
- [ ] The temporary `release_parked_accept` work-around in
      `clientd/daemon.rs`'s test harness is removed, or reduced to a case the
      fix genuinely does not cover.
- [ ] Owner-only SDDL, `PIPE_REJECT_REMOTE_CLIENTS`, instance rotation, and the
      `Hello`-first contract stay green.
- [ ] Two-frontend fan-out and single-poll-plane tests stay green.
- [ ] Unix behavior and tests are unchanged.
- [ ] `cargo fmt`, workspace Clippy, and `./scripts/check-local.sh` are clean, and
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
