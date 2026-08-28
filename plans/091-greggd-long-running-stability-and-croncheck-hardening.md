# Plan 091: greggd long-running stability and croncheck hardening

Status: implementation in progress; extended soak evidence remains before closure.

Depends on: completed Plans 080-090 and the 2026-08-28 `greggd` long-running stability review.

## Objective

Make `greggd` trustworthy as an unattended background daemon without expanding Gregg into a service framework or observability platform.

The daemon is expected to remain useful for weeks or months when left alone. Test deployments have instead needed to be started again after roughly one or two days. Current source review found two credible long-duration failure classes:

1. the Unix local-control task can cause the entire daemon to exit even though the control socket is auxiliary and no operator requested shutdown;
2. optional drive-capacity collection can block the sampler indefinitely on a native filesystem syscall while the HTTP listener remains alive, producing a process that is technically running but no longer publishing fresh system metrics.

The current `croncheck` behavior is also weaker than its intended watchdog role. It treats any accepting TCP listener on the configured port as evidence that `greggd` is running, so an unrelated process is a false positive and a live-but-stale `greggd` is indistinguishable from a healthy one.

This plan fixes those bounded issues while preserving the intended operator contract:

```text
greggd run       -> foreground daemon; should remain alive on its own
greggd stop      -> explicit local stop request
greggd croncheck -> if the configured greggd is running, do nothing;
                    if it is definitely absent, start `greggd run`;
                    if the port is occupied by something ambiguous, fail
                    diagnostically rather than starting blindly
```

The cron watchdog remains a safety net. It must not become the primary mechanism by which the daemon stays alive.

## Confirmed stability findings

### 1. Unix control-task failure can terminate the daemon as a successful shutdown

`crates/greggd/src/control.rs::stop_loop()` currently returns immediately on any `UnixListener::accept()` error:

```rust
let (mut stream, _) = match listener.accept().await {
    Ok(pair) => pair,
    Err(e) => return Err(e),
};
```

`spawn_stop_task()` forwards that result through a one-shot channel. `run.rs::shutdown_with_control()` converts a failed/closed control task into the synthetic shutdown reason `"control-error"`, and supervision classifies that reason as a normal signal shutdown.

That is the wrong ownership boundary. The control socket is an auxiliary local operator channel. Only receipt of a valid `STOP\n` request should ask the daemon to exit. Failure of that channel must not itself be interpreted as an operator stop request.

This also defeats the packaged systemd unit's `Restart=on-failure` policy because the resulting daemon exit can be classified as success.

### 2. One local control client can hold the control task indefinitely

The control loop accepts one connection and then awaits reads serially until newline, EOF, or the fixed byte limit. There is no per-client read deadline.

A local client that connects and then sends nothing can therefore monopolize the stop listener indefinitely. This does not stop the HTTP server, but it can make `greggd stop` unusable until process restart.

### 3. Optional drive collection is inside the critical sample path

The sampler correctly runs native collection on Tokio's blocking pool so `/proc`, Mach, Win32, and filesystem reads do not stall the current-thread HTTP runtime. However, `sample_on_blocking_pool()` awaits the blocking task without a deadline.

On Linux, every normal sample currently reaches drive collection, which parses `/proc/self/mountinfo` and calls synchronous `libc::statvfs()` for eligible mounts. At the default 1000 ms cadence, filesystem-capacity probing is therefore attempted every second.

A blocked `statvfs()` cannot be cancelled safely. The same general concern applies to platform-native drive-capacity calls on macOS/Windows: optional storage enumeration has a much larger blocking surface than CPU/memory counters and should not determine whether core metrics continue to advance.

### 4. A blocked `spawn_blocking` operation also weakens shutdown guarantees

Aborting the async sampler task does not terminate a `spawn_blocking` closure that has already started. Tokio runtime shutdown may wait for started blocking work.

Therefore the existing ten-second async task cleanup deadline is not a hard boundary around a native filesystem call. A single wedged drive syscall can both freeze fresh snapshots and complicate clean process shutdown.

### 5. Current `croncheck` verifies only TCP acceptance

Current `croncheck` performs a bounded `TcpStream::connect_timeout()` against the configured local address. If anything accepts the connection, it returns success without proving that the peer is `greggd` or that the HTTP API is responsive.

This gives two bad classifications:

- unrelated listener on the port -> false `greggd is running` result;
- stale/wedged `greggd` HTTP listener -> indistinguishable from a healthy daemon.

The watchdog should identify a responsive Gregg daemon, but it must preserve the stated action rule: start a daemon only when absence is reasonably proven. An ambiguous occupied or non-responsive port is not permission to launch another copy.

## Scope decisions

### In scope

- Correct Unix control-task supervision so auxiliary control failure cannot stop `greggd`.
- Make valid `STOP\n` receipt the only control-socket event that requests daemon shutdown.
- Add bounded per-control-client request I/O time so a silent client is dropped.
- Retry or safely degrade on transient Unix `accept()` errors without a busy loop.
- Isolate optional drive-capacity refresh from the critical CPU/memory/load sampling path.
- Stop performing synchronous drive-capacity collection once per normal sample.
- Keep at most one drive-refresh worker/in-flight operation per daemon; never spawn an unbounded sequence of threads/tasks when storage calls are slow.
- Preserve the most recent valid drive metrics between refreshes; absence/failure of drive metrics must not mark core collection failed.
- Reduce risky Linux mount probing for filesystems that are not meaningful local block-storage targets, including `autofs` and generic FUSE families where appropriate.
- Restore a tiny bounded HTTP identity/readiness probe inside `croncheck` without adding an HTTP client dependency.
- Make `croncheck` start a detached `greggd run` child only when the configured endpoint is definitely absent/refused.
- Add focused deterministic regression tests and one local sustained-runtime smoke/soak procedure.
- Update active daemon documentation and the plan index when implementation closes.

### Out of scope

- Self-daemonization, PID files, process-name scanning, `pkill`, `killall`, or arbitrary PID signalling.
- Reintroducing systemd/launchd command execution into Unix runtime code.
- Changing Windows SCM lifecycle architecture.
- A public HTTP shutdown endpoint.
- A generic supervisor framework, actor system, plugin system, or generalized worker pool.
- New crates solely for timers, HTTP probing, process supervision, or filesystem monitoring.
- A child-process-per-sample isolation architecture.
- Killing Rust threads or attempting unsafe cancellation of native syscalls.
- Prometheus/OpenTelemetry, historical metrics storage, tracing backends, dashboards, or new observability infrastructure.
- New CI jobs, matrices, soak workflows, self-hosted runners, evidence bundles, or release gates.
- Treating optional drive freshness as a reason to stop serving otherwise-current CPU/memory/load metrics.
- Unrelated TUI, protocol, release, or dependency cleanup.

## Expected implementation surface

Primary files likely to change:

```text
crates/greggd/src/control.rs
crates/greggd/src/run.rs
crates/greggd/src/cli.rs
crates/greggd/src/sampler.rs
crates/greggd/src/collector/mod.rs
crates/greggd/src/collector/linux/mod.rs
crates/greggd/src/collector/linux/drives.rs
crates/greggd/src/collector/macos/mod.rs
crates/greggd/src/collector/windows/...   # only as required for equivalent drive isolation
crates/greggd/README.md
architecture/greggd-daemon.md
architecture/collectors.md
plans/README.md
```

A small collector-internal helper module for optional drive refresh is acceptable if it materially reduces duplicated lifecycle/channel code across platforms. Do not create a generalized background-job framework.

## Implementation sequence

### Phase 1: fix control-socket ownership before touching collection

The daemon's shutdown contract must become explicit:

```text
SIGINT/SIGTERM                  -> daemon shutdown
valid local STOP request        -> daemon shutdown
control client timeout          -> drop client, keep daemon running
malformed control client        -> drop client, keep daemon running
control accept/transient error  -> retry/back off, keep daemon running
control channel becomes unusable-> log degradation, keep daemon running on signals
```

Remove the current semantic path where a failed control task becomes `"control-error"` and is classified as `RunOutcome::Signal`.

A suitable small design is either:

1. have the control task send only a `StopRequested` event and never send ordinary task failure through the shutdown channel; or
2. retain the existing result channel but make `shutdown_with_control()` continue waiting on SIGINT/SIGTERM when the control task returns `Err`/closes, rather than resolving the daemon shutdown future.

Prefer the smaller design after inspection. Do not add a broad event bus.

For `UnixListener::accept()`:

- transient errors must not immediately kill the daemon;
- retries must use a small bounded delay/backoff so a persistent failure cannot spin a CPU;
- if the listener becomes irrecoverably unusable, log one clear warning, allow the control task to end/disable itself, remove its socket path through the existing guard, and leave the HTTP/sampler daemon running until a real signal arrives.

Do not hide a tight infinite error loop behind `continue`.

### Phase 2: bound individual control clients

Keep the protocol exactly `STOP\n` -> `OK\n`.

Add a small request deadline around accepted-client reads. Approximately 750 ms to 2 s is reasonable; use one named constant and deterministic tests rather than scattering timeout literals.

Requirements:

- a silent connected client is dropped after the deadline;
- a partial request that never terminates is dropped;
- overlong/malformed input remains dropped;
- a later well-formed client can still issue `STOP\n`;
- timeout/malformed clients do not notify the daemon shutdown future;
- response writing remains bounded/best-effort and cannot hold shutdown indefinitely.

Do not add authentication or expand the protocol.

### Phase 3: remove drive-capacity work from the critical sample latency

This is the core longevity correction.

The invariant after this phase must be:

> A stuck or very slow drive-capacity operation cannot prevent new CPU, memory, swap/commit, and load snapshots from being published at the configured sampler cadence.

The preferred minimal implementation shape is a collector-owned optional drive refresh worker/cache rather than changing the daemon's top-level supervision model.

Recommended design constraints:

- one persistent drive-refresh worker per native collector/daemon, not one new thread per refresh;
- use a bounded request channel/capacity of one or equivalent coalescing so refresh requests cannot accumulate;
- the normal `SystemCollector::sample()` path checks for an already-completed drive result non-blockingly and never waits for one;
- refresh drive capacity at a much slower cadence, approximately every 30-60 seconds, with the first refresh requested immediately after collector construction or first sample;
- while no first result exists, publish `drives: None` rather than blocking daemon readiness;
- after a valid result, retain that latest result until a newer successful result arrives;
- a transient drive error is logged at debug/warn level as appropriate and does not transition core sampler readiness to `Failed`;
- if a drive worker blocks forever in a native syscall, it consumes only that one bounded worker and no additional refresh workers are created;
- daemon shutdown must not wait indefinitely to join a blocked optional drive worker. The process must still be able to exit after the existing critical-task shutdown path completes.

A dedicated `std::thread` for this optional worker is acceptable and may be preferable to `tokio::spawn_blocking`, because runtime teardown must not inherit Tokio's obligation to wait for an already-running blocking closure. Keep the worker bounded and private to collector implementation.

Do not attempt unsafe thread cancellation. Process termination is the hard boundary for a genuinely uninterruptible kernel/filesystem call.

### Phase 4: keep platform semantics truthful

Linux:

- core `/proc/stat`, `/proc/loadavg`, and `/proc/meminfo` sampling remains on the existing critical collector path;
- drive refresh continues to use truthful native filesystem capacity values;
- explicitly filter `autofs`;
- inspect current FUSE handling and exclude generic `fuse` / `fuse.*` filesystems unless a specific local filesystem is deliberately supported and demonstrated safe;
- retain existing exclusions for network/pseudo filesystems;
- drive mount errors remain optional and must never fail the core sample.

macOS:

- preserve the current CPU/Mach/memory/swap behavior;
- move mounted-filesystem capacity enumeration behind the same non-blocking refresh/cache boundary;
- retain the existing local + not-`MNT_DONTBROWSE` + non-`devfs`/`autofs` rules.

Windows:

- preserve existing SCM and core metric semantics;
- apply equivalent drive-refresh isolation if Windows drive capacity is currently collected synchronously in the core sample;
- do not redesign Windows service code for this plan.

If inspection proves one platform's drive implementation is already non-blocking with respect to core sampling, document that and avoid gratuitous churn.

### Phase 5: make `croncheck` identify greggd before deciding

The current TCP-only check is insufficient. Reintroduce the earlier tiny bounded HTTP probe style, but preserve the current user-required watchdog behavior that can start an absent daemon.

Use only `std::net::TcpStream` and the existing configuration. No `reqwest`/Hyper client dependency is needed.

Probe target:

```text
GET /v2/healthz HTTP/1.1
Host: localhost
Connection: close
```

or the equivalently minimal request already used historically in this crate.

Keep connect/read/write operations bounded. Read a fixed-size first status line and enough bounded body bytes to identify a valid Gregg health response if body validation is retained. Prefer the smallest robust discriminator.

Required classification:

#### Responsive greggd

A syntactically valid Gregg health response proves the daemon is running.

Both of these count as **running**, so `croncheck` does nothing and exits 0:

- HTTP 200 / Ready;
- HTTP 503 with a valid Gregg warming/failed health response.

`croncheck` is an existence watchdog, not a policy that restarts a live daemon merely because an optional collector is temporarily failed.

#### Definitely absent

Connection refusal / no listener at the configured local address means no daemon is accepting the configured endpoint.

Only this class should trigger the existing detached `greggd run` spawn path.

After spawning, `croncheck` may return success once process creation succeeds; it does not need to wait through full daemon warm-up. Kernel bind semantics still arbitrate races between simultaneous cron invocations.

#### Ambiguous/occupied

These must **not** cause a blind second start:

- TCP listener responds with non-Gregg HTTP;
- malformed HTTP;
- accepts TCP but times out;
- permission/unreachable/local networking errors that are not equivalent to connection refusal;
- an HTTP response that cannot be identified as Gregg.

Return nonzero with concise diagnostics. Starting another process would only race/bind-fail and could conceal a port collision.

This classification supersedes older Plan 076-080 statements that described `croncheck` as purely observational. The current product requirement is authoritative: `croncheck` is a small operator-managed watchdog that starts `greggd` only when it is absent.

### Phase 6: add deterministic stability regressions

Add the smallest tests that lock the failure boundaries rather than attempting a multi-day unit test.

At minimum cover:

#### Control supervision

- a valid STOP event still selects the graceful shutdown path;
- a simulated control-task failure/closed channel does **not** resolve daemon shutdown while signal input remains pending;
- SIGINT/SIGTERM-equivalent injected shutdown still exits normally after control degradation;
- malformed/silent control clients never produce a stop event;
- a client timeout is followed by successful handling of a later valid STOP request.

Refactor just enough to inject the relevant control outcome deterministically. Do not build a generic fake-listener framework solely to manufacture rare kernel `accept()` errors.

#### Drive isolation

Use a test worker/source whose drive refresh blocks on a barrier or pending condition while core metrics remain immediately available.

Prove that:

- at least several consecutive core samples/snapshots advance while the drive worker is blocked;
- sample timestamps continue changing;
- readiness remains `Ready` once core collection is ready;
- no second drive worker/request backlog is created while the first refresh is stuck;
- daemon/sampler shutdown completes without waiting for the blocked optional drive worker;
- latest valid drive data is retained across a later drive refresh failure.

Tests should run in seconds, not minutes.

#### croncheck classification

Cover with tiny local TCP fixtures:

- valid Gregg Ready response -> no spawn path;
- valid Gregg Failed/Warming response -> no spawn path;
- closed/refused port -> spawn path selected;
- unrelated HTTP 200 -> error/no spawn;
- accepted-but-silent peer -> bounded timeout/error/no spawn;
- malformed status/body -> error/no spawn;
- wildcard bind address still probes loopback.

Separate probe classification from actual process spawning enough that tests do not fork real detached daemons repeatedly.

### Phase 7: run focused local verification

Run at minimum:

```bash
cargo fmt --all -- --check
cargo test -p greggd control
cargo test -p greggd sampler
cargo test -p greggd cli
cargo test -p greggd collector
cargo test -p greggd --bin greggd
cargo test -p greggd
./scripts/check-local.sh
```

Run clippy if it is not already part of `check-local.sh`:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

No new workflow/job/matrix is required. Existing native CI may run normally after push, but Plan 091 does not add a CI requirement or soak job.

### Phase 8: perform a narrow local runtime smoke and sustained soak

Use a real release binary on the available Unix host. This is local operational validation, not permanent CI infrastructure.

#### Immediate lifecycle smoke

1. build `greggd --release`;
2. create a temporary user-owned config on an unused loopback port with the normal sample interval;
3. launch `target/release/greggd run --config <path>` directly;
4. confirm `/v2/status` produces advancing `observed_at_unix_ms` values;
5. run `greggd croncheck` while Ready and confirm it does nothing;
6. induce or use a fixture path for a Failed/Warming health response where practical and prove croncheck does not start a second process merely for non-Ready health;
7. connect to the Unix control socket and deliberately send no complete command; after the control-client deadline expires, verify HTTP snapshots continue advancing and a subsequent real `greggd stop` still works;
8. stop the daemon normally and verify the process exits promptly;
9. run `croncheck` against the now-refused endpoint and verify it starts one detached replacement;
10. verify the replacement becomes a valid Gregg endpoint and that a second `croncheck` does not start another daemon;
11. stop and clean up the replacement.

Use no `sudo`, `systemctl`, `launchctl`, `service`, `pkill`, or `killall` for this direct smoke.

#### Sustained local soak

Run one release `greggd` instance for at least several hours, preferably overnight/24 hours when the implementation environment permits normal unattended execution.

A tiny shell loop or existing local tooling may record at a coarse cadence (for example once per minute):

- daemon PID still present;
- `/v2/status` responds;
- `observed_at_unix_ms` continues advancing;
- RSS;
- open file descriptor count where the host exposes it.

Acceptance is based on qualitative boundedness, not a brittle exact memory threshold:

- PID remains stable unless intentionally restarted;
- snapshots never stop advancing for an unexplained extended interval;
- RSS/FD count does not show monotonic unbounded growth;
- no unexpected `greggd stopped` event occurs;
- control socket remains usable after idle time;
- croncheck continues to identify the running daemon and takes no action.

Do not add the soak script to CI or build an evidence framework. If a reusable script would materially simplify future manual reproduction, one short `scripts/` helper is acceptable; otherwise keep the commands in the Plan 091 closure record.

If a full overnight run cannot be completed in the implementation environment, do not fabricate it. Complete deterministic tests plus a shorter local stress run, mark the long soak as pending/manual evidence, and keep Plan 091 open until the requested longevity evidence is actually obtained or explicitly waived by the maintainer.

## Acceptance criteria

### Unix control-channel resilience

- [ ] Only a valid `STOP\n` request from the local control socket can request daemon shutdown through the control path.
- [ ] A control-task I/O error or closed result channel cannot be converted into `RunOutcome::Signal` or any successful daemon shutdown.
- [ ] Transient `accept()` failures are retried with bounded delay/backoff rather than terminating the daemon or spinning.
- [ ] An irrecoverable control-listener failure degrades local `greggd stop` capability but leaves HTTP serving/sampling alive until a real signal.
- [ ] A silent or partial local control client is dropped after a fixed request deadline.
- [ ] A malformed/timed-out client does not prevent a later valid `greggd stop`.
- [ ] Control socket cleanup remains safe on task termination and daemon shutdown.

### Critical sampling resilience

- [ ] CPU/memory/swap-or-commit/load collection does not synchronously wait for drive-capacity enumeration.
- [ ] Drive-capacity refresh runs at a slower explicit cadence than the core sample interval.
- [ ] At most one optional drive refresh can be in flight; requests/results are bounded/coalesced.
- [ ] If one native drive operation blocks indefinitely, core snapshots continue advancing.
- [ ] A blocked optional drive worker cannot hold Tokio runtime shutdown indefinitely.
- [ ] No unsafe thread cancellation is introduced.
- [ ] Last-known-good drive metrics are retained between refreshes and through transient drive refresh failure.
- [ ] Missing/failed drive metrics do not transition otherwise-valid core sampling to `Failed`.
- [ ] Linux no longer probes `autofs`, and generic FUSE mounts are excluded unless specifically justified.
- [ ] No per-refresh thread/task leak is possible.

### croncheck watchdog semantics

- [ ] `croncheck` uses a bounded HTTP probe to identify Gregg rather than treating any TCP accept as success.
- [ ] Valid Gregg Ready health means running -> exit 0, no spawn.
- [ ] Valid Gregg Warming/Failed health means running -> exit 0, no spawn.
- [ ] Connection refusal/no listener means absent -> spawn one detached `greggd run` child.
- [ ] Unrelated HTTP, malformed response, or accepted-but-silent peer is ambiguous -> nonzero, no blind spawn.
- [ ] Wildcard configured bind addresses continue to probe local loopback correctly.
- [ ] No HTTP client dependency, service-manager call, PID scanning, or public shutdown route is added.
- [ ] Concurrent croncheck races remain safe through ordinary process spawn + kernel bind semantics; no lock/PID-file subsystem is added.

### Regression and operational proof

- [ ] Deterministic tests cover control-task degradation without daemon shutdown.
- [ ] Deterministic tests cover silent-client timeout followed by a successful STOP.
- [ ] Deterministic tests prove core snapshots advance while drive refresh is blocked.
- [ ] Deterministic tests prove optional drive blockage does not prevent shutdown of the critical daemon runtime.
- [ ] Deterministic croncheck tests cover Ready, Failed/Warming, refused, unrelated HTTP, malformed, and silent-peer cases.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] Focused `greggd` tests pass.
- [ ] `./scripts/check-local.sh` passes.
- [ ] Clippy with warnings denied passes if not already included by the local check.
- [ ] A real release-binary direct lifecycle smoke passes without service-manager commands.
- [ ] A sustained local soak records stable PID, advancing snapshots, and no monotonic unbounded RSS/FD growth for the longest practical unattended window; target at least overnight/24 h before final closure unless explicitly waived.

### Scope control

- [ ] No new dependency is added unless the existing standard-library/Tokio primitives prove insufficient and the plan is amended first.
- [ ] No new CI workflow, soak job, matrix, evidence bundle, or release gate is added.
- [ ] No self-daemonization, PID files, process scanning, service-manager coupling, or generic supervisor architecture is introduced.
- [ ] No protocol schema change is required.
- [ ] No unrelated client/TUI/release cleanup is mixed into the implementation.

## Closure record requirements

Do not mark this plan complete merely because unit tests pass.

When closing Plan 091, record:

1. implementation commit SHA;
2. the exact control-failure semantic change;
3. the final drive-refresh isolation/cadence design;
4. the final croncheck classification contract;
5. focused/local check results;
6. release-binary lifecycle smoke result;
7. sustained soak duration and concise observations for PID, snapshot advancement, RSS, and FD behavior;
8. any platform-specific limitation that could not be directly exercised locally.

Update `plans/README.md` only after those acceptance criteria are truthful. Preserve prior Plans 076-090 as historical records; where their old `croncheck` wording conflicts with the current product requirement, Plan 091 is the authoritative superseding contract rather than rewriting old implementation history.
