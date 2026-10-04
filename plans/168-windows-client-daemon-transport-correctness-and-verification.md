# Plan 168: Windows client-daemon transport correctness and cross-platform verification

Status: complete. Closed at `eb2bc62`.

CI run `37243685819` on `eb2bc62`: **all six jobs green** — Linux, macOS
(arm64), macOS (Intel), Windows, MSRV 1.89, FreeBSD (`gregg-host` native).

Follows the completed Plan 161 line (Plans 162-167). Independent of Plan 091.

## Why this plan exists

Plans 164 and 167 closed with an explicit, honest gap: the Windows job had not
been run, so the Windows half of the client daemon was **unverified**. That gap
turned out to hide a much larger fact.

The Windows half of the local IPC transport and the Windows branch of the config
lock had been written but never compiled. The `gregg` client crate did not build
for `x86_64-pc-windows-msvc`, so the Windows CI job had been red continuously
since the client daemon landed. Two pushes on the Plan-161 line were red for
this reason alone, and the earlier closure's "the macOS/Windows/MSRV jobs were
not run" note was, in effect, the only thing standing between that state and a
release.

The defect was not a version skew or a stale lockfile. It was a Windows
implementation that had never been type-checked, in a repository whose local
verification loop is Linux-only and whose Windows verification is a remote job
nobody was watching.

## What was wrong

Compile-level, in `crates/gregg/src/clientd/ipc.rs`,
`crates/gregg/src/config/lock.rs`, and `crates/greggd/src/scheduler.rs`:

- `Stream::Windows` named `std::io::ReadWrite`, which does not exist.
- `ConnectNamedPipe` was called with five arguments; it takes two.
- The owner-only SDDL out-parameter was confused with a caller-supplied string
  buffer, and the buffer's own address was then passed off as the security
  descriptor, so the pipe would have been created with a garbage descriptor.
- The handle was adopted into a `File`, dropped at the end of the `unsafe`
  block, and then reused.
- `PIPE_REJECT_REMOTE_CLIENTS` was OR-ed into the open-mode argument instead of
  the pipe-mode argument, which would have left the pipe reachable off-box
  rather than restricting it.
- `LOCKFILE_EXCLUSIVE_LOCK` and `LockFileEx` were imported from
  `Win32::System::IO`; windows-sys 0.59 keeps both in `Win32_Storage_FileSystem`.
- `SDDL_REVISION_1` was read from `Win32::Foundation`; it lives in
  `Win32::Security::Authorization`.
- Four `windows-sys` feature gates the code depends on were not enabled.
- Two Unix-only test helpers in `greggd/src/scheduler.rs` were dead on Windows,
  which CI's `-D warnings` turns into a build failure.

Behaviour-level, which no compiler would have found:

- The endpoint name was a Unix socket path. `CreateNamedPipeW` requires the
  `\\.\pipe\` prefix and rejects anything else, and that namespace is flat, so
  there is no config-adjacent location and no temp-directory fallback.
- The accept loop requires a non-blocking accept, which cannot exist for a
  synchronous named pipe. `gregg daemon run` uses a **current-thread** runtime,
  so a blocking accept would freeze polling, cron, and every attached window at
  once.
- Endpoint liveness was a `path.exists()` check, which is always false for a
  pipe. The `stop` confirmation and every startup readiness wait would have
  returned immediately, reporting success without observing anything.

## What was done

- Added the missing `windows-sys` feature gates and corrected every import
  against the vendored windows-sys 0.59 source rather than from memory.
- Rewrote the Windows transport: `Stream::Windows` owns a `std::fs::File` so the
  raw `HANDLE` has exactly one owner; the SDDL descriptor is taken from the
  API's own out-parameter and released with `LocalFree` in the same block;
  `PIPE_REJECT_REMOTE_CLIENTS` goes in the pipe-mode argument; no
  `FILE_FLAG_OVERLAPPED`.
- Parked the blocking `ConnectNamedPipe` on the blocking pool with the pipe
  instance moved into the closure, so the current-thread runtime is never
  parked and the handle keeps one owner for the whole wait.
- Reads use `PeekNamedPipe` for the queued byte count and then `ReadFile` for at
  most that many bytes and at most what the caller's buffer holds, so a poll
  costs no context switch. Asking for less than the queued count keeps
  `ERROR_MORE_DATA` out of the picture; the remainder is read next poll.
- Identity produces `\\.\pipe\gregg-client-<id>` on Windows: one name, from the
  digest alone, with the `sun_path` length limit correctly treated as a
  Unix-only constraint.
- Added `ipc::endpoint_is_live` — `path.exists()` on Unix,
  `WaitNamedPipeW(name, 0)` on Windows — and routed the `stop` confirmation, the
  test-harness readiness wait, and the launcher's endpoint check through it.
- `Listener::accept` is now `async` on both platforms so one signature serves
  both; the Unix arm is otherwise unchanged.

## Second defect found by the new tests

`FrontendFrame` is internally tagged, and serde cannot represent an internally
tagged newtype whose payload is a bare string. The refusal frame was written as
`ProtocolError(String)`: it compiled, the receiving side matched it correctly,
and it could never be serialized. Both senders discarded the encode error with
`let _ =`, so a refused frontend saw a bare disconnect instead of the reason.

This is not Windows-specific and predates this line. It was found because the
new round-trip test picked that variant to assert the write path, and CI
reported `cannot serialize tagged newtype variant`. The variant is now a struct
variant, and `every_frame_variant_survives_an_encode` enumerates every variant
and asserts each encodes and decodes.

The lesson is recorded rather than the fix: **an enum variant that cannot be
serialized is not caught by any compiler, and only by a test that enumerates
the variants.**

## Verification

### Local

- `cargo check --workspace --target x86_64-pc-windows-gnu --all-targets
  --all-features` — clean, zero warnings. This is the loop that made the fix
  converge; see "Process change" below.
- `cargo test --workspace --all-targets --all-features` — 1598 passed, 0 failed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.

### Remote

CI run `37243685819` on `eb2bc62` — **all six jobs green**. The first attempt at
this work, run `37243214189` on the transport fix `01a1405`, had five green and
one red: the Windows job compiled and ran 801 tests, and the single failure was
the new round-trip test reporting
`cannot serialize tagged newtype variant FrontendFrame::ProtocolError containing
a string`. That is where the second defect came from.

On the final SHA the Windows job reports **803 passed, 0 failed, 2 ignored** in
the `gregg` client lib, and the whole Windows suite green. The six tests this
plan added all executed and passed on the real runner:

| Test | Proves |
| --- | --- |
| `clientd::ipc::windows_tests::a_client_and_daemon_exchange_frames_over_a_real_named_pipe` | the SDDL pipe, the accept, and both the write and the peek/read path, in one exchange |
| `clientd::ipc::windows_tests::an_accept_waits_for_a_client_that_connects_later` | the blocking `ConnectNamedPipe` on the blocking pool, and that the current-thread runtime is not parked by it |
| `clientd::ipc::windows_tests::a_closed_pipe_reports_a_disconnect_not_an_error` | a vanished peer becomes `Disconnected`, so a TUI exit is not logged as a fault |
| `clientd::ipc::windows_tests::connecting_reports_that_no_daemon_is_running` | an absent pipe is a refusal, not a hang |
| `clientd::identity::tests::the_windows_candidate_is_one_pipe_name_in_the_flat_namespace` | the endpoint is one `\\.\pipe\gregg-client-<id>` name |
| `clientd::identity::tests::a_very_long_config_path_still_yields_one_usable_pipe_name` | the Unix `sun_path` limit does not suppress a Windows endpoint |

The pre-existing client-daemon tests that use the daemon harness also pass on
Windows, which is the indirect but substantial evidence that binding, the
`WaitNamedPipeW` liveness probe, and `FrontendLink::connect` all work against a
real daemon process on that platform.

The Windows job is the load-bearing one for this plan. It compiles the client
crate for `x86_64-pc-windows-msvc` and then **runs** the suite, which now
includes four `clientd::ipc::windows_tests` cases that exercise the pipe end to
end:

- a client and daemon exchanging frames over a real named pipe, including the
  reply travelling back;
- an accept that waits for a client which connects *after* the accept is
  already parked, which is the ordering that exercises the blocking
  `ConnectNamedPipe` rather than the `ERROR_PIPE_CONNECTED` shortcut;
- a closed pipe reporting a disconnect rather than an error;
- connecting to an absent pipe being reported as a refusal.

## Process change

The reason this defect survived four plans is a verification gap, not a coding
error, so the gap is closed rather than only the symptom:

- **A cross-target type-check is now part of the local loop.** With
  `gcc-mingw-w64-x86-64` and the `x86_64-pc-windows-gnu` target, the whole
  workspace type-checks for Windows in about a minute. This is what turned a
  26-error unknown into a short fix list. It is a local developer aid, not a CI
  change: CI keeps `windows-2022` and the real MSVC target as the authority for
  both compilation and execution.
- **`clientd::ipc` now has a Windows test module.** The Unix suite could never
  have found any of this, because none of the code was reachable on Linux.

## Not claimed here

- **No claim about Windows behaviour outside what CI executed.** The four new
  tests plus the existing Windows suite are real evidence. The SCM lifecycle
  smoke in `scripts/smoke-windows.ps1` covers the daemon service, not the client
  daemon's multi-window behaviour, so "two TUIs on one Windows daemon" is still
  argued from the shared code path rather than demonstrated on Windows.
- **No macOS or FreeBSD re-verification was needed.** Those jobs were already
  green and their platform-gated code was not touched.
- **The pre-existing `gregg-update` `ETXTBSY` preflight flake is untouched.** It
  is root-caused in Plan 167's closure and remains out of scope here.
- **Eleven pre-existing clippy warnings remain on the Windows target.** CI runs
  clippy on Linux only, so Windows-only code has never been linted. Running
  `cargo clippy --workspace --target x86_64-pc-windows-gnu --all-targets
  --all-features` shows eleven pedantic warnings in files this plan did not
  touch: seven in `gregg-host/src/windows/source.rs`, one each in
  `greggd/src/startup/install.rs`, `greggd/src/update.rs` (two),
  `gregg/src/bin/lock_helper.rs`, and `gregg/src/config/store.rs`. They do not
  fail CI and they are not regressions from this line, so they are reported
  rather than fixed here. The warnings in `clientd/ipc.rs` and
  `config/lock.rs` — the code this plan wrote and edited — are all fixed, so
  both files are clean on the Windows target. This is the same root cause as the
  original defect: **code behind a `cfg` that the local loop never compiles is
  code nobody is checking.** A follow-up that lints the Windows target in CI
  would close it permanently; that is a workflow change and is deliberately not
  made here.
