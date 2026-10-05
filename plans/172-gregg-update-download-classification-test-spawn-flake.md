# Plan 172: `gregg-update` download-classification test reports a spawn failure as a missing file

Status: complete. Code at `be1aee0`.

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

- [x] The HTTP-500 half of the test asserts the **specific** classification
      (an `unexpected HTTP 500` failure), not `Failed(_)`, so a spawn failure
      can no longer satisfy it.
- [x] A spawn failure surfaces as a named assertion about the spawn, with the
      underlying `io::Error` in the message, not as a missing `calls` file.
- [x] The test is not sensitive to a transient `EAGAIN`/fork failure under
      loaded-runner parallelism, or failing that it is retried a bounded number
      of times and reports how many attempts it needed.
- [x] The sibling `download_*` tests that share `stub_curl_with_code` are checked
      for the same over-wide `Failed(_)` assumption, and the ones that make the
      same mistake are corrected in the same pass.
- [x] `gregg-update`'s own behavior is unchanged: `download_file` still maps
      spawn errors, 5xx, unparseable status, and oversize to distinct hard
      failures, and only an exact 404 to `DownloadOutcome::NotFound`.
- [x] `cargo fmt`, workspace Clippy, and `./scripts/check-local.sh` are clean,
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

## Closure record

### The stop condition was not triggered

`download_file` is byte-for-byte unchanged, as is the `DownloadOutcome` enum.
The plan's stop condition existed to stop a fix that "improves" the test by
changing what production does; this fix moved the *evidence*, not the
classification, so no follow-up is required.

### How "the child ran" is proven

`download_file` genuinely cannot distinguish a refused server from a child that
never started — `run_child_with_timeout` returns `Err` for a failed
`Command::spawn`, and `download_file` maps that to the same
`DownloadOutcome::Failed(String)` a genuine 500 produces
(`exec.rs:402`). That behavior is correct, so the test had to supply the
distinction itself.

The distinction is taken from **the child's own side effect**, not from parsing
a production error string. Every stub `curl` in the module now appends to a
`calls` log as its first act — `stub_curl_oversized_asset` was extended to do so,
since it was the only one that did not — and `download_file_with_started_stub`
clears that log, calls `download_file`, and accepts the outcome only if the log
came back. A missing log is therefore direct evidence that no child started.

The alternative (matching the `"curl failed: "` reason prefix) was rejected: it
duplicates a production string inside the test, so any reword of that message
would silently turn a spawn failure back into a classification failure. The
side-effect approach cannot drift that way.

### The retry, and what happens when it runs out

A child that never started is retried up to `SPAWN_RETRY_ATTEMPTS` (4) with a
50 ms backoff, and every skipped attempt is reported. If all attempts fail, the
helper panics with a message that names the spawn as the subject, carries the
reason `download_file` reported, and states the attempt count. That is the
"retried a bounded number of times and reports how many attempts it needed"
branch of the third acceptance criterion, and it is deliberately *not* an
`ENOENT` on a file the stub never wrote.

`a_stub_that_never_started_is_reported_as_a_spawn_failure` locks this
deterministically by pointing `download_file` at a `curl` path that does not
exist — the same failure the runner produced, reproduced on demand rather than
waited for. It asserts the panic names the spawn, carries the reason taken from
`download_file` itself (so no fixed `strerror` is baked in), and states the
attempt count. That test is the actual proof of the second and third
acceptance criteria: it exercises the flake's exact mechanism every run, instead
of hoping a `fork` fails.

### A correction to this plan's own analysis

This plan predicted the fix would assert the message `unexpected HTTP 500`, and
named it in acceptance criterion 1. **That prediction was wrong**, and the code
says why: production always passes `-f`, so a 5xx makes curl exit non-zero and
the code never reaches the `Ok(out) if out.status.success()` arm that produces
`unexpected HTTP {code}`. A 500 arrives through the non-success arm as
`curl exit Some(22): …`.

Rather than assert a message the production path cannot produce, both arms are
now covered, which is strictly more than the plan asked for:

- the `-f` stub (the production shape) asserts the specific `curl exit`
  classification, and
- a new `stub_curl_success_with_code` models a curl *without* `-f` that exits 0
  on a 5xx, which asserts `unexpected HTTP 500`.

The second case is the defense in depth `download_file`'s own doc comment
describes — "assert the captured `%{http_code}` is 2xx anyway so a future curl
without `-f` cannot accept an error body as success" — and it was previously
untested. It is also the only case where the body's mention of 404 could
plausibly be laundered, so it is where the plan's original invariant is actually
testable.

### The four siblings

Criterion 4 asked for the siblings to be checked. All four made the same
mistake, in the same way:

1. `download_rejects_an_oversized_asset_even_on_http_200` — a spawn failure
   reports `Failed` *and* leaves no `dest`, so **both** its assertions passed
   while the size cap was never exercised. It now goes through the helper and
   asserts `exceeding the {MAX_DOWNLOAD_BYTES} byte maximum`.
2. `download_rejects_unparseable_status_on_success` — its pre-seeded `dest` is
   removed by the unparseable-status arm *and* by the spawn-failure arm, so the
   removal check could not tell them apart either. It now asserts
   `unparseable HTTP status`.
3. `download_classifies_code_in_a_single_request` — the reported defect.
4. `baseline_download_500_is_hard_failure` (real `curl`, so it cannot use a
   `calls` log) — added `spawn_recording_response`, which sets a flag when a
   client is accepted. The test now asserts that a client reached the fixture
   before accepting its `Failed`, so a curl that never spawned fails loudly
   instead of passing. The child exits only after the request completes, so the
   flag is settled by the time `download_file` returns — no race.

`serve_once` was split so both fixtures share one `respond` implementation
rather than duplicating the read/write loop.

### One thing the cross-check caught

`cargo clippy -p gregg-update --target x86_64-pc-windows-gnu` reported
`SPAWN_RETRY_ATTEMPTS` and `SPAWN_RETRY_DELAY` as never used: the whole stub
cluster is `#[cfg(unix)]`, so on Windows the new constants had no users. CI runs
Clippy with `-D warnings`, so this would have **failed the Windows job** — a
`gregg-update` test fix that breaks Windows lint. Both constants are now
`#[cfg(unix)]`. This is a development aid, not closure evidence; native Windows
CI is the only authority, and it is green.

### Evidence

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --all-features` — clean, no warnings.
- `cargo clippy -p gregg-update --target x86_64-pc-windows-gnu --all-targets
  --all-features` — clean after the `#[cfg(unix)]` fix.
- `cargo test -p gregg-update --all-features` — 44 passed, 0 failed (was 42;
  the two new cases are `a_stub_that_never_started_is_reported_as_a_spawn_failure`
  and the exit-0-on-5xx leg of the classification test).
- `cargo test --workspace --all-targets --all-features` — 1,600 tests, 0 failed.
- `./scripts/check-local.sh` — `=== all checks passed (mode: default) ===`.
- **MSRV, five consecutive runs** of `rustup run 1.89 cargo test -p gregg-update
  --all-features -- exec::tests` — 20 passed, 0 failed each time. The criterion
  asked for green across repeated runs rather than once by luck.
- **Loaded-runner proxy:** eight concurrent invocations of the built
  `gregg-update` test binary (20 tests each, `--test-threads=8`) on a 16-core
  host — 8/8 green. This is a proxy for CI's six-job parallelism, not a
  reproduction of the original `EAGAIN`; the deterministic
  `a_stub_that_never_started_…` test is the real evidence, because it produces
  the same spawn failure on every run instead of waiting for the runner to.

### CI

**Run `37308641245` on `2dee87c`: all six jobs green** — Linux, both macOS
jobs, Windows, MSRV Rust 1.89, and FreeBSD. MSRV is the job that failed in
`37270317172` and is green here; it is not a claim that a `fork` never fails,
but the assertion it was making is now incapable of being satisfied by one.

The Windows job's `Clippy` step is green, which is the direct confirmation that
the `#[cfg(unix)]` gate on the new constants was required: without it the
Windows job would have failed on `-D warnings` for two dead-code constants, and
the local cross-run was the only thing that caught it in time. The Windows log
also shows the six `curl_baseline` tests executing — the stub-cluster tests are
correctly excluded there, which is the gating working as intended.

