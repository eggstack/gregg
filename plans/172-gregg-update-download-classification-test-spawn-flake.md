# Plan 172: `gregg-update` download-classification test reports a spawn failure as a missing file

Status: planned.

Depends on: nothing. Found while validating Plan 171; independent of Plans
091, 169, 170, and 171.

Opened as the narrow corrective follow-up Plan 171's stop conditions require.
This defect is in `gregg-update`, which Plan 171 did not touch and does not
depend on, and it is **not** a Plan 171 regression.

## The failure, exactly

CI run `37270317172` on `e3f0694` (the Plan 171 code SHA) had five of six jobs
green — including Windows, which is the job Plan 171 exists to satisfy. The MSRV
job failed at `exec::tests::download_classifies_code_in_a_single_request`:

~~~
thread 'exec::tests::download_classifies_code_in_a_single_request' panicked at
crates/gregg-update/src/exec.rs:785:52:
called `Result::unwrap()` on an `Err` value:
  Os { code: 2, kind: NotFound, message: "No such file or directory" }
~~~

It is intermittent: the same commit's Linux job, both macOS jobs, the Windows
job, and the FreeBSD job all ran the same test green, and MSRV was green on
`39017b1`.

## Root cause

`crates/gregg-update/src/exec.rs`, the second half of the test:

~~~rust
// A body/message mentioning 404 must not be sniffed as NotFound
// when the captured status code is 500.
let curl = stub_curl_with_code(&dir, "500");
assert!(matches!(
    download_file(&curl, "https://example.invalid/404-docs", &dest),
    DownloadOutcome::Failed(_)
));
assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\n");
~~~

`Failed(_)` is far wider than the assertion intends. `download_file` maps a
**spawn failure** to the same `DownloadOutcome::Failed` as a genuine HTTP 500
(`exec.rs:402`, the `Err(e)` arm of `run_child_with_timeout`), so when
`Command::spawn` fails transiently the first assertion still passes — and the
second one then reads a `calls` file the stub never got the chance to write,
reporting `ENOENT` for a file that has nothing to do with the HTTP
classification the test is about.

The transient spawn failure is ordinary loaded-runner contention, not a
mystery. In the failing run this test executed alongside the two other
`exec` tests that spawn real child processes (`cargo_timeout_kills_and_reaps_child`
and `discovery_probe_is_bounded_and_kills_a_hanging_binary`, the latter keeping a
deliberately hanging child alive until its discovery timeout), and both are in
the log immediately before the failure. A `fork` under that load can fail
`EAGAIN`.

The 100-second `DOWNLOAD_WALL_TIMEOUT` is not involved: the stub is a two-line
`/bin/sh` script, so a timeout is not what killed anything.

## Why it is not Plan 171's

- `gregg-update` is a separate crate with its own `src/`; Plan 171 touched
  `gregg` only, plus one `windows-sys` feature in `gregg`'s manifest.
- Nothing in `gregg-update` depends on `clientd::ipc`, `clientd::daemon`, or
  `dispatch_daemon`.
- The failure reproduces in a code path Plan 171 does not reach: it is a
  `curl` stub spawn, and Plan 171 changed no process-spawning code outside
  `gregg`'s client-daemon shutdown path.
- It is intermittent, and the Linux job runs the identical test green in the
  same commit.

## Scope

Make this test report the truth instead of a misleading `ENOENT`, and make it
stop failing on an unrelated transient. Nothing in `gregg-update`'s production
behavior changes: `download_file`'s classification is correct as written, and
that is worth keeping.

## Acceptance criteria

- [ ] The HTTP-500 half of the test asserts the **specific** classification
      (an `unexpected HTTP 500` failure), not `Failed(_)`, so a spawn failure
      can no longer satisfy it.
- [ ] A spawn failure surfaces as a named assertion about the spawn, with the
      underlying `io::Error` in the message, not as a missing `calls` file.
- [ ] The test is not sensitive to a transient `EAGAIN`/fork failure under
      loaded-runner parallelism, or failing that it is retried a bounded number
      of times and reports how many attempts it needed.
- [ ] The sibling `download_*` tests that share `stub_curl_with_code` are checked
      for the same over-wide `Failed(_)` assumption, and the ones that make the
      same mistake are corrected in the same pass.
- [ ] `gregg-update`'s own behavior is unchanged: `download_file` still maps
      spawn errors, 5xx, unparseable status, and oversize to distinct hard
      failures, and only an exact 404 to `DownloadOutcome::NotFound`.
- [ ] `cargo fmt`, workspace Clippy, and `./scripts/check-local.sh` are clean,
      and MSRV is green across repeated runs rather than once by luck.

## Stop conditions

Open a further follow-up rather than broadening if the fix would require
changing `download_file`'s classification or its `DownloadOutcome` contract,
since that is updater behavior rather than test honesty, and Plan 126 measured
the external-`curl` design deliberately.

## Handoff

CI stops reporting an unrelated `ENOENT` in `gregg-update` as though it were a
classification bug, and a loaded runner stops being able to turn a fork failure
into a red build for a reason that has nothing to do with the code under test.
