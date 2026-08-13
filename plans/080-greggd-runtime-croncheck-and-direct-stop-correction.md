# Phase 080: greggd runtime/croncheck correction and direct stop command

Status: complete.

Depends on: Plans 076-079.

## Objective

Correct the current `greggd` operational gap exposed by a local `croncheck` failure and add a real `greggd stop` command on Unix without reintroducing the service-manager coupling removed by Plan 076.

The current observed failure is:

```text
error: health probe connection failed: Connection refused (os error 111)
```

Inspection of current `croncheck` shows that this diagnostic is emitted before any HTTP request is sent: `TcpStream::connect_timeout()` cannot connect to the configured probe target. Therefore this phase must not paper over the refusal by weakening `croncheck`, automatically starting a daemon, or treating a refused connection as healthy. The work must reproduce the current Ubuntu behavior with the release binary, determine why no listener is present at the expected target, correct only the actual runtime/config/packaging defect if one is reproduced, and then prove locally that a running daemon is reachable by `croncheck`.

This phase also restores one deliberately useful lifecycle operation on Unix: `greggd stop`. The command must stop the exact local `greggd` instance associated with the resolved daemon configuration without invoking `systemctl`, `launchctl`, `pkill`, `killall`, shell commands, process-name scanning, or a public HTTP shutdown route.

The preferred Unix mechanism is a tiny local Unix-domain control socket owned by the daemon. This is intentionally narrower and safer than adding PID-file management or reopening the removed Unix service-manager abstraction. The existing HTTP API remains read-only.

Windows already exposes `greggd stop` through the native SCM path. Preserve that behavior rather than replacing it.

## Baseline findings

### 1. `croncheck` is currently a genuine passive health probe

`crates/greggd/src/cli.rs` currently:

1. resolves and loads the same daemon config used by `run`;
2. maps wildcard bind addresses to loopback (`0.0.0.0 -> 127.0.0.1`, `:: -> ::1`);
3. preserves the configured port;
4. performs a bounded `TcpStream::connect_timeout()`;
5. sends `GET /v2/healthz` only after the TCP connection succeeds;
6. accepts only a complete HTTP/1.0 or HTTP/1.1 `200` status line.

The observed `Connection refused` therefore means there was no accepting TCP listener at the computed target at that moment. It is not an HTTP 503/readiness result.

Plan 080 must keep these semantics. A stopped daemon must continue to make `croncheck` fail nonzero.

### 2. `greggd run` is the authoritative Unix daemon entry point

Current `crates/greggd/src/main.rs` loads config, constructs the native collector, creates one current-thread Tokio runtime, and enters `greggd::run::run(...)` for `Command::Run`.

Current `run.rs` validates runtime config and sampler interval, binds the HTTP listener before spawning tasks, then supervises the sampler/server until SIGTERM/SIGINT or a task failure.

No Unix `systemctl`/`launchctl` runtime dispatch should be restored.

### 3. Linux packaging does not automatically start the service

`packaging/install-linux.sh` installs the binary, config, and unit and runs `systemctl daemon-reload`, but intentionally leaves enable/start as explicit operator action.

The installed unit runs:

```text
/usr/local/bin/greggd run --config /etc/gregg/greggd.toml
```

and uses:

```text
User=greggd
Group=greggd
RuntimeDirectory=gregg
PrivateTmp=true
Restart=on-failure
```

The local reproduction must distinguish these cases:

- daemon never started;
- daemon attempted to start but exited;
- daemon is alive but bound to a different address/port/config;
- daemon is healthy and the original refusal came from operator/service state rather than a product defect.

Do not invent a code change when local reproduction proves the binary is correct and merely not running. In that case, improve the diagnostic enough to show the exact target/config being probed and record the operational cause truthfully.

### 4. Unix `stop` was intentionally removed by Plan 076

Current `Command::Stop` is Windows-only and delegates to the Windows SCM manager. Native Linux/macOS parser tests explicitly reject `stop` today.

That is now an incomplete CLI contract for direct foreground Unix operation. The new requirement is:

```text
greggd stop
```

on Linux/macOS, while preserving the existing Windows SCM implementation.

### 5. Do not use a public HTTP shutdown endpoint

The daemon API commonly binds to `0.0.0.0` on a trusted LAN and intentionally has no authentication. Adding `POST /stop`, `/shutdown`, or equivalent to the HTTP API would let any reachable LAN client stop the daemon and would violate the read-only API boundary.

The stop control path must be local-only and separate from the metrics HTTP server.

### 6. Do not add PID-file/process-discovery machinery

Repository guidance explicitly excludes PID-file management and Plan 076 excluded process discovery. Do not reverse that architecture solely to implement one command.

Use a Unix-domain control socket on Linux/macOS. Tokio already has the required Unix socket primitives under the existing `net` feature; no new runtime dependency should be needed.

## Authoritative behavior after Plan 080

### `greggd run`

On Linux/macOS:

```text
greggd run [--config PATH]
```

must:

- remain the canonical foreground daemon command;
- load the resolved config and bind the configured HTTP listener exactly as today;
- establish one local Unix-domain control socket for the same daemon instance;
- remain in the foreground until SIGINT, SIGTERM, `greggd stop`, or a critical runtime failure;
- perform the same graceful shutdown path for signal-triggered and CLI-triggered stop;
- remove/clean its control socket on orderly shutdown and startup failure;
- never invoke a service manager or shell command.

The HTTP API remains unchanged and read-only.

### `greggd croncheck`

`croncheck` remains observational only:

```text
greggd croncheck [--config PATH]
```

Required semantics:

- load the same resolved configuration as `run`;
- normalize wildcard bind addresses to loopback;
- probe the configured TCP port;
- issue `GET /v2/healthz`;
- exit 0 only on HTTP 200;
- exit nonzero on connection refusal, timeout, malformed response, EOF, or non-200 status;
- never start or stop a process;
- never connect to the new Unix stop-control socket;
- include enough failure context to identify the target address and, at the binary boundary or error value, the config path used for the probe.

A useful failure shape is:

```text
error: health probe connection to 127.0.0.1:11310 failed: Connection refused (os error 111)
```

Do not make tests depend on the full string. The key requirement is that the target is visible in the diagnostic.

### `greggd stop` on Linux/macOS

`greggd stop [--config PATH]` must:

1. resolve/load the same daemon configuration identity used by `run`;
2. derive the local control-socket candidates for that config;
3. connect only to a local Unix-domain socket;
4. send one tiny bounded stop command, e.g. `STOP\n`;
5. require an explicit acknowledgement or clean protocol success before returning 0;
6. trigger the daemon's existing graceful shutdown path;
7. never invoke systemd, launchd, a shell, or process-discovery command;
8. never signal arbitrary PIDs;
9. never mutate the HTTP API or daemon TOML configuration.

Recommended user-visible outcomes:

```text
greggd stopped
```

for a successful accepted stop request, and:

```text
greggd not running
```

for an absent control socket / already-stopped daemon if idempotent success is retained.

The exact wording is less important than stable exit semantics.

Preferred idempotence:

- running daemon, request accepted -> exit 0;
- already stopped / no control socket -> exit 0 with concise `not running` output;
- stale socket candidate that cannot accept -> continue to any valid fallback candidate, then treat as not running if none are live;
- permission denied -> exit with the existing permission-denied taxonomy;
- malformed/incorrect local control response -> runtime error, do not claim success.

### `greggd stop` on Windows

Keep the current Windows SCM behavior:

```text
greggd stop
    -> Windows Service Control Manager stop
```

Do not route Windows through the Unix control-socket mechanism and do not redesign the existing SCM service worker.

## Unix control-socket design

Keep this mechanism deliberately tiny. It is not a generic IPC subsystem.

### Primary socket location

For a daemon using resolved config path `PATH`, prefer a socket adjacent to the config file when that directory is writable by the daemon, for example:

```text
/etc/gregg/greggd.control.sock
/Library/Application Support/gregg/greggd.control.sock
```

This is important on the packaged Linux service because `PrivateTmp=true` isolates the daemon's `/tmp`; an operator-side CLI could not reach a control socket created only in the service's private temporary namespace.

The Linux installer already makes `/etc/gregg` writable by the `greggd` service account, and the systemd unit explicitly allows writes there. The macOS LaunchDaemon runs in the system context and can use the existing config directory.

Do not add a new systemd service-manager command merely to discover the socket.

### Foreground fallback location

A direct foreground user may be able to read `/etc/gregg/greggd.toml` but not write `/etc/gregg`. Therefore `run` needs one deterministic fallback local socket location when the config directory is not writable.

Preferred fallback:

```text
std::env::temp_dir()/greggd-<sanitized-host>-<port>.control.sock
```

or an equally small deterministic equivalent.

Requirements for the fallback:

- derive only from validated local config data;
- keep the path below Unix-domain `sun_path` limits;
- avoid user-provided arbitrary path traversal;
- create with restrictive permissions (`0600` where supported);
- do not rely on a random per-process filename that `greggd stop` cannot rediscover;
- do not introduce a config field solely for the control path.

### Candidate resolution

`run` and `stop` must share one helper for socket candidate derivation so they cannot drift.

Recommended behavior:

```text
candidate 1: config-adjacent socket
candidate 2: deterministic fallback socket
```

`run`:

- try candidate 1;
- if the config parent is not writable / the path is unsuitable, use candidate 2;
- do not silently fall back on arbitrary unexpected errors;
- if a stale Unix socket exists, only remove it when inspection confirms it is actually a socket and no live peer accepts connections;
- never unlink an arbitrary regular file at the control path.

`stop`:

- try an existing config-adjacent socket first;
- if it is stale/unreachable, try the deterministic fallback;
- report not-running only after no valid candidate accepts the stop request.

A root/operator invocation should still be able to find a foreground fallback socket: candidate selection for `stop` should be based on existence/reachability, not merely whether the caller can write the config directory.

### Local protocol

Keep the protocol fixed and bounded. One line is enough:

```text
client -> STOP\n
daemon -> OK\n
```

Maximum command/response length should be a small constant. Reject any other command. Do not add JSON, serde types, framing libraries, version negotiation, or a general control API.

The daemon should not accept arbitrary metrics/config mutations over this socket in Plan 080.

### Shutdown integration

Reuse the existing daemon shutdown architecture.

Today `run_with_shutdown()` accepts a future that resolves to a shutdown reason and then uses the same broadcast/cleanup path for SIGINT/SIGTERM. Extend only the Unix foreground boundary so the shutdown future races:

```text
SIGINT / SIGTERM
        OR
valid local STOP command
```

Do not duplicate sampler/server teardown logic inside the control-socket code.

The preferred result is still one shared path:

```text
shutdown source resolves
    -> broadcast shutdown
    -> HTTP server + sampler terminate
    -> bounded join/abort cleanup
    -> greggd exits normally
```

The control-socket path/guard must also be cleaned when `run` returns because of bind failure, collector/runtime error, or ordinary signal shutdown.

## Corrective investigation for the current `croncheck` refusal

This phase is not complete from unit tests alone. Before editing behavior, reproduce the current failure on the available Ubuntu host using the current release build.

### Step A: establish the actual config and target

Run:

```bash
./target/release/greggd version
./target/release/greggd configprint
```

For the exact command/config path that produced the failure, also record:

```bash
./target/release/greggd --config <PATH> configprint
```

when an explicit config is involved.

Confirm the exact `croncheck` target after wildcard normalization.

### Step B: inspect listener/process state

Use ordinary host tools only for diagnosis:

```bash
ss -ltnp | grep 11310 || true
systemctl status greggd --no-pager || true
journalctl -u greggd -n 100 --no-pager || true
```

These commands are local verification tools, not production implementation dependencies.

Classify the refusal as one of:

1. service never started;
2. service exits immediately;
3. `greggd run` exits before binding;
4. daemon is bound to a different host/port/config;
5. daemon is listening and the refusal cannot be reproduced with the same binary/config.

### Step C: direct foreground reproduction independent of systemd

Create a temporary valid config on an unused high port and start the real release binary directly:

```bash
./target/release/greggd --config "$CFG" run >"$LOG" 2>&1 &
DAEMON_PID=$!
```

Use a bounded polling loop around:

```bash
./target/release/greggd --config "$CFG" croncheck
```

Do not use an arbitrary long sleep as the proof.

If direct `run` cannot become healthy, inspect the daemon log and correct the smallest actual runtime defect. If it becomes healthy, do not modify the probe just because a separately managed systemd service had not been started.

### Step D: correct only reproduced product defects

Potential fixes are limited to evidence from the reproduction, such as:

- wrong config path passed by a packaged service;
- startup failure caused by current unit hardening;
- daemon runtime exits before listener binding;
- bind/config mismatch between `run` and `croncheck`;
- incorrect local probe target derivation.

Do not speculate or broaden scope.

If the only cause is "daemon was not running", the code-side correction is limited to the improved target/config diagnostic plus the new direct `stop` control path; document the operational conclusion explicitly.

## Implementation sequence

### Step 1: reproduce and classify the current Ubuntu refusal

Run the baseline checks above before modifying `croncheck`.

Record in Plan 080 closure notes:

- binary version tested;
- resolved config path;
- configured bind address;
- normalized probe target;
- whether a listener existed;
- whether systemd service was active;
- whether direct foreground `run` became healthy.

Do not claim a root cause without this evidence.

### Step 2: make croncheck diagnostics identify the target

Keep the existing probe implementation and bounded parser unless reproduction finds an actual defect.

At minimum, connection/timeout/request errors should identify the target socket address. Preserve focused negative-path tests from Plan 077.

If config-path context is added, keep it at the binary/dispatch error boundary rather than embedding unrelated filesystem concerns inside the low-level probe helper.

### Step 3: expose Unix `Command::Stop`

Change CLI cfg-gating so:

Linux/macOS accept:

```text
run
stop
croncheck
configprint
host
port
version
```

and still reject Unix `start` and `restart`.

Windows continues to accept its existing lifecycle commands.

Update parser tests accordingly.

Do not resurrect Unix `ServiceManager` implementations.

### Step 4: add a tiny Unix control module/helper

Prefer one small module, e.g.:

```text
crates/greggd/src/control.rs
```

behind `#[cfg(unix)]`, or keep equivalent helpers in `run.rs`/`cli.rs` if that is materially smaller and clearer.

Responsibilities only:

- derive primary/fallback socket paths from config path + validated host/port;
- bind/guard the daemon socket;
- enforce restrictive permissions;
- reject/remove stale socket entries safely;
- receive one bounded `STOP\n` command and return `OK\n`;
- client-side connect/send/ack for `greggd stop`.

Do not make it a generic RPC framework.

### Step 5: race local stop with ordinary Unix shutdown signals

Integrate the control stop future into the same shutdown source passed to the existing `run_with_shutdown` core.

Required invariant:

```text
SIGTERM, SIGINT, and greggd stop all converge on the same graceful cleanup path.
```

Do not create a second server/sampler shutdown implementation.

### Step 6: preserve Windows SCM behavior

Keep existing Windows `Command::Stop` dispatch to `platform_service_manager().stop()`.

Windows compile/tests must remain green. Do not add a named-pipe control mechanism in this phase.

### Step 7: add focused tests

At minimum add deterministic coverage for:

1. native Unix parser accepts `stop` but rejects `start`/`restart`;
2. `croncheck` connection-error diagnostic contains the attempted target;
3. control-socket path derivation is deterministic for the same config;
4. config-adjacent path is preferred when viable;
5. fallback path is deterministic and bounded;
6. control listener accepts exact `STOP\n` and rejects malformed/overlong input;
7. client stop requires the expected acknowledgement;
8. missing control socket follows the chosen idempotent not-running semantics;
9. a stale socket does not cause arbitrary file deletion;
10. permission-denied control access maps to the existing permission-denied exit taxonomy;
11. a control stop causes the shared shutdown future to resolve;
12. existing SIGINT/SIGTERM shutdown tests remain green;
13. Windows SCM stop tests remain unchanged/green under Windows cfg.

Use short local Unix sockets/temp directories in tests. Do not add sleeps for production intervals.

### Step 8: run the existing local check

Run:

```bash
cargo fmt --all -- --check
cargo test -p greggd cli
cargo test -p greggd run
cargo test -p greggd --bin greggd
cargo test -p greggd
./scripts/check-local.sh
```

If exact filters do not map cleanly, run the nearest package-level command and record the actual invocation.

Do not add a new CI workflow or evidence harness for this phase.

### Step 9: mandatory real Ubuntu end-to-end smoke

Use the built release binary, not `cargo run`.

Create a temporary config on an unused high port. Then:

```bash
cargo build --release -p greggd

./target/release/greggd --config "$CFG" run >"$LOG" 2>&1 &
SHELL_PID=$!
```

Boundedly wait until:

```bash
./target/release/greggd --config "$CFG" croncheck
```

returns 0 and prints healthy output.

Then verify the HTTP listener exists:

```bash
ss -ltn | grep ":$PORT "
```

Run:

```bash
./target/release/greggd --config "$CFG" stop
```

Required result:

- `stop` exits 0;
- foreground `greggd run` exits normally;
- the shell `wait "$SHELL_PID"` completes with successful daemon exit;
- the HTTP listener disappears;
- the control socket is removed;
- a subsequent `croncheck` fails nonzero with connection refusal/unavailable semantics;
- a second `stop` follows the documented idempotent already-stopped behavior;
- no `systemctl`, `launchctl`, `pkill`, `killall`, shell, or public HTTP shutdown request is involved.

This smoke is mandatory for closure.

### Step 10: Linux packaged-service smoke on the current host

Because the original failure was observed around a Linux/systemd deployment, also verify the installed service path on the current Ubuntu host when the service/unit and required privileges are available.

Use the current built binary/config deliberately; do not publish anything.

Sequence:

1. ensure the installed unit points to the intended release binary and `/etc/gregg/greggd.toml`;
2. start the service explicitly with operator/systemd tooling;
3. verify `greggd croncheck` returns healthy;
4. verify the config-adjacent control socket is visible outside the service despite `PrivateTmp=true`;
5. invoke `sudo greggd stop` (or equivalent privileged invocation required by socket permissions), not `systemctl stop`, for the actual stop action under test;
6. verify `greggd` exits cleanly and `Restart=on-failure` does not immediately restart it;
7. verify `croncheck` then fails nonzero.

Using `systemctl` to arrange/inspect the test fixture is allowed. Production `greggd stop` itself must not execute it.

If this environment lacks the installed unit or privilege needed for this optional packaging-specific smoke, record that explicitly. The direct foreground Ubuntu smoke remains mandatory and cannot be skipped.

### Step 11: update user-facing docs narrowly

Update only directly affected documentation:

- README command list includes `greggd stop`;
- Unix stop is described as local direct daemon control, not service-manager invocation;
- `croncheck` remains passive and non-mutating;
- Windows stop remains SCM-backed;
- no HTTP shutdown endpoint exists;
- systemd/launchd remain optional external supervisors.

Update `AGENTS.md` only as needed to encode the new invariant and avoid future regression. Retain the prohibition on PID-file management and Unix service-manager coupling.

### Step 12: close the plan only after the local smoke

After implementation and verification:

1. mark Plan 080 complete;
2. record the actual root cause of the original connection refusal;
3. record the exact Ubuntu commands/results for `run -> croncheck -> stop -> dead croncheck`;
4. record packaged-service smoke results if performed;
5. update `plans/README.md` status/dependency chain;
6. do not create Plan 081 solely to record closure if every acceptance criterion passes.

## Expected files

Primary implementation surface:

```text
crates/greggd/src/cli.rs
crates/greggd/src/main.rs
crates/greggd/src/run.rs
crates/greggd/src/lib.rs
crates/greggd/src/control.rs              # only if a separate tiny Unix module is clearer
```

Potentially touched if the reproduced refusal proves a packaging defect:

```text
packaging/systemd/greggd.service
packaging/install-linux.sh
```

Documentation/planning:

```text
README.md
architecture/greggd-daemon.md             # only if current lifecycle description becomes inaccurate
architecture/scripts-and-packaging.md      # only if packaging behavior changes
AGENTS.md                                  # narrow invariant update
plans/080-greggd-runtime-croncheck-and-direct-stop-correction.md
plans/README.md
```

Do not create a new crate, generic IPC crate, supervisor abstraction, HTTP client dependency, PID manager, or CI workflow.

## Scope

### In scope

- reproduce the observed Ubuntu `croncheck` refusal with the current release binary/config;
- identify whether the cause is service state, runtime exit, config mismatch, bind mismatch, or a real probe defect;
- correct only a reproduced runtime/config/packaging defect;
- include target address in `croncheck` connection diagnostics;
- expose `greggd stop` on Linux/macOS;
- preserve Windows SCM `stop`;
- implement a small local-only Unix control socket;
- route CLI stop into the existing graceful shutdown path;
- secure/bound the control protocol;
- handle stale/missing control sockets safely;
- deterministic focused tests;
- mandatory direct Ubuntu release-binary E2E;
- optional-but-preferred current-host systemd packaging smoke when available;
- narrow documentation/planning updates.

### Out of scope

- restoring Unix `greggd start` or `restart`;
- `systemctl`/`launchctl` invocation from production `greggd`;
- self-daemonization/background mode;
- PID files or PID registries;
- `pkill`, `killall`, process-name matching, `/proc` process discovery, or equivalent;
- HTTP `/stop`/`/shutdown` endpoints;
- authentication/TLS redesign;
- config mutation through the control socket;
- status/metrics retrieval through the control socket;
- generic IPC/RPC framework;
- new configuration fields solely for stop/control;
- Windows named-pipe redesign;
- Windows SCM redesign;
- new dependencies unless inspection proves an existing standard-library/Tokio primitive is insufficient;
- new CI jobs, workflows, matrices, evidence bundles, or release gates;
- release automation/publication.

## Acceptance criteria

### Original refusal diagnosis/correction

- [ ] The exact current Ubuntu `croncheck` refusal is reproduced or explicitly shown non-reproducible using the current release binary.
- [ ] The resolved config path used by `croncheck` is recorded.
- [ ] The configured bind address and normalized probe target are recorded.
- [ ] Listener state is checked at the moment of failure.
- [ ] Service/process state is checked at the moment of failure.
- [ ] Direct foreground `greggd run` is tested independently of systemd.
- [ ] The root cause is recorded as service-not-running, runtime exit, config mismatch, bind mismatch, probe defect, or another directly demonstrated cause.
- [ ] Any code/packaging correction is limited to the demonstrated cause.
- [ ] `croncheck` still returns nonzero when the daemon is actually stopped.
- [ ] `croncheck` still returns nonzero for HTTP 503/unhealthy state.
- [ ] `croncheck` connection diagnostics identify the attempted socket address.
- [ ] `croncheck` never starts/stops/restarts a service or process.

### Unix `greggd stop`

- [ ] Linux/macOS CLI parsing accepts `greggd stop`.
- [ ] Linux/macOS CLI parsing continues to reject `greggd start` and `greggd restart`.
- [ ] Windows retains the existing SCM-backed `greggd stop` behavior.
- [ ] Unix `stop` targets the daemon associated with the same resolved config identity as `run`.
- [ ] Unix `stop` uses a local Unix-domain socket, not the public HTTP listener.
- [ ] No production Unix `systemctl`/`launchctl` invocation is added.
- [ ] No PID file or process-discovery mechanism is added.
- [ ] No `pkill`, `killall`, shell command, or name-based process termination is added.
- [ ] No HTTP shutdown route is added.
- [ ] Stop protocol input/output is fixed-size/bounded.
- [ ] Only the explicit stop command is accepted by the control socket.
- [ ] Control socket permissions are restrictive.
- [ ] Stale socket handling never unlinks arbitrary regular files.
- [ ] Missing socket/already-stopped semantics are documented and tested.
- [ ] Permission-denied stop requests use the existing permission exit taxonomy.
- [ ] Successful `stop` enters the same graceful cleanup path as SIGTERM/SIGINT.
- [ ] Successful `stop` does not cause systemd `Restart=on-failure` to relaunch a cleanly stopped service.

### Runtime/control integration

- [ ] Control-socket candidate derivation is shared by `run` and `stop`.
- [ ] Packaged Linux service uses a non-`/tmp` primary control path reachable despite `PrivateTmp=true`.
- [ ] Direct foreground run has a deterministic fallback when the config directory is not writable.
- [ ] Control socket is removed on ordinary CLI stop.
- [ ] Control socket is removed on SIGINT/SIGTERM shutdown.
- [ ] Control socket is cleaned on startup/runtime failure where it was created.
- [ ] Existing HTTP listener binding/readiness ordering remains correct.
- [ ] Existing server/sampler supervision and 10-second bounded shutdown cleanup remain the single teardown mechanism.

### Focused verification

- [ ] `cargo fmt --all -- --check` passes.
- [ ] Focused `greggd` CLI tests pass.
- [ ] Focused `greggd` run/control tests pass.
- [ ] `cargo test -p greggd --bin greggd` passes.
- [ ] `cargo test -p greggd` passes.
- [ ] `./scripts/check-local.sh` passes.
- [ ] No new CI workflow/job/matrix is added.

### Mandatory Ubuntu E2E

- [ ] Release `greggd` binary is built locally.
- [ ] A temporary valid config on an unused high port is used.
- [ ] `greggd run` remains alive in the foreground/backgrounded only by the shell test harness.
- [ ] `greggd croncheck` becomes healthy within a bounded wait.
- [ ] `ss` confirms the expected TCP listener while healthy.
- [ ] `greggd stop` exits 0.
- [ ] The daemon process exits cleanly after `stop`.
- [ ] The TCP listener disappears after `stop`.
- [ ] The Unix control socket disappears after `stop`.
- [ ] A subsequent `croncheck` fails nonzero.
- [ ] A second `stop` follows the documented already-stopped/idempotent behavior.
- [ ] The E2E uses no production systemd/launchd coupling.

### Packaged Linux smoke when available

- [ ] Existing/current-host systemd unit startup is verified when the environment permits it, or the missing privilege/unit limitation is recorded explicitly.
- [ ] When tested, service-managed `greggd` becomes healthy via ordinary `croncheck`.
- [ ] When tested, the config-adjacent control socket is reachable outside the service `PrivateTmp` namespace.
- [ ] When tested, privileged `greggd stop` stops the service process without `greggd` invoking `systemctl`.
- [ ] When tested, the clean stop does not trigger `Restart=on-failure`.

### Documentation and closure

- [ ] README documents `greggd stop` and preserves passive `croncheck` semantics.
- [ ] Architecture docs remain truthful about Unix foreground runtime and Windows SCM separation.
- [ ] AGENTS guidance continues to prohibit PID-file management and Unix service-manager coupling while allowing the narrow local control socket.
- [ ] Plan 080 closure record states the real cause of the original refusal.
- [ ] `plans/README.md` registers Plan 080 and marks it complete only after the mandatory local E2E passes.
- [ ] No Plan 081 is created solely for closure if all Plan 080 criteria pass.

## Closure standard

Plan 080 is complete only when both product-level statements are demonstrated on the current Ubuntu host:

```text
running greggd -> croncheck succeeds
```

and:

```text
greggd stop -> daemon exits -> croncheck fails because listener is gone
```

Compilation and unit tests alone are insufficient. The local release-binary smoke is the authoritative proof for this phase.

## Closure record

**Date:** 2026-08-13
**Binary version:** `greggd 1.0.5`
**Host:** Ubuntu 24.04.4 LTS (Noble Numbat, aarch64)

### Root cause of original croncheck refusal

The original `Connection refused (os error 111)` error was caused by the daemon
not running. No installed unit, no foreground process, no TCP listener at the
configured address. The `croncheck` probe correctly identified the target and
correctly refused the connection. The diagnostic lacked the target address but
the binary itself was functioning correctly.

### Implementation summary

1. **`croncheck` diagnostics:** Added the attempted socket address to all
   connection/timeout/request error messages. The format is now:
   `error: health probe connection to 127.0.0.1:11310 failed: Connection refused (os error 111)`

2. **Unix `Command::Stop`:** Added `stop` to the CLI parser on Linux/macOS
   (previously Windows-only). The parser now accepts `run`, `stop`, `croncheck`,
   `configprint`, `host`, `port`, `version` and rejects `start`/`restart`.

3. **Control socket module:** Added `control.rs` with:
   - Config-adjacent primary path (`greggd.control.sock` in config directory)
   - Deterministic temp-dir fallback path (`greggd-{host}-{port}.control.sock`)
   - Shared candidate derivation used by both `run` and `stop`
   - Socket binding with stale-socket detection and `0600` permissions
   - RAII cleanup guard (`ControlSocketGuard`) for socket removal on any exit path
   - Wire protocol: `STOP\n` -> `OK\n`
   - Client-side send_stop with candidate fallback and idempotent not-running

4. **Shutdown integration:** `run_with_control_path` wires the control listener
   into the same `select!` as SIGTERM/SIGINT. The shutdown future resolves when
   any source fires; the shared cleanup path runs identically. A dedicated
   tokio task owns the control listener for clean separation from the
   supervision loop.

5. **Windows SCM behavior preserved:** `Command::Stop` on Windows continues to
   delegate to `platform_service_manager().stop()`. No named-pipe control
   mechanism added.

### Local E2E smoke results

```
Binary version:   greggd 1.0.5
Config path:      /tmp/gregg-e2e/config.toml
Bind address:     127.0.0.1:11403
Control socket:   /tmp/gregg-e2e/greggd.control.sock

greggd run -> croncheck healthy:    OK (exit 0)
TCP listener present:               OK (ss confirms)
Control socket created (0600):      OK (ls -l confirms)
greggd stop exit:                   OK (exit 0, "greggd stopped")
Daemon process exited:              OK (wait returns 0)
TCP listener gone after stop:       OK (ss confirms)
Control socket removed after stop:  OK (ls confirms)
Subsequent croncheck:               OK (exit 3, "Connection refused")
Second stop (idempotent):           OK (exit 0, "greggd not running")
SIGTERM cleanup:                    OK (control socket removed)
```

### Packaged-service smoke

Not performed. The host lacks an installed systemd unit. This is explicitly
permitted by the plan: "If this environment lacks the installed unit or
privilege needed for this optional packaging-specific smoke, record that
explicitly. The direct foreground Ubuntu smoke remains mandatory and cannot
be skipped."

### Test coverage

- 14 control module tests (path derivation, bind/rebind, send/recv protocol, malformed response, cleanup)
- 13 CLI tests (parser accepts stop, rejects start/restart, croncheck target diagnostics, probe health)
- 172+ existing tests (run, sampler, server, config) remain green
- `cargo fmt --all -- --check` passes
- `./scripts/check-local.sh` passes