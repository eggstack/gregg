# Phase 076: native runtime, croncheck, and version correction

Status: implemented.

Depends on: Plan 075.

## Objective

Correct the Unix daemon lifecycle boundary without reopening Gregg's broader service architecture, convert `croncheck` into a genuine supervisor-independent HTTP health probe, add explicit `version` subcommands to both `greggd` and `gregg`, and demonstrate the resulting daemon behavior with one local end-to-end run on the current Ubuntu host.

The intended Unix model after this phase is deliberately simple:

```text
operator / init system / cron
            |
            v
       greggd run
            |
            v
   native greggd HTTP server
```

`greggd` must not invoke systemd or launchd in order to run, check health, or mutate Unix configuration. systemd and launchd remain optional deployment mechanisms supplied by packaging assets and operated explicitly by the user or administrator.

Windows SCM support completed in Plans 072-075 is not being redesigned by this phase. Windows service lifecycle behavior may retain its native SCM path. The defect owned here is the Unix runtime coupling plus the misleading `croncheck` semantics.

This is a narrow correction. Do not add self-daemonization, PID files, process discovery, privilege escalation, a generic supervisor framework, a new HTTP client dependency, or new CI coverage.

## Current defects

1. On Linux, `greggd start`, `stop`, `restart`, and the inactive branch of `croncheck` delegate to `SystemdManager`, which directly executes `systemctl`.
2. `croncheck` currently means "query the native service manager and start the service if inactive" rather than "check whether greggd is healthy".
3. `host` and `port` mutations are coupled to `service.restart()`, causing an otherwise local configuration edit to require system service privileges on Unix.
4. The Unix executable therefore carries process-supervision policy that belongs in external systemd/launchd packaging.
5. `greggd` and `gregg` expose Clap's generated `--version` flag, but neither has the requested explicit `version` subcommand.
6. The repository's ordinary unit checks do not prove on a real Linux host that the built daemon can run directly, bind a socket, become healthy, serve status JSON, and be checked by `croncheck` without sudo or systemd.

## Authoritative behavior after this phase

### `greggd run`

`greggd run [--config PATH]` remains the canonical foreground daemon entry point. It must:

- load and validate configuration exactly as today;
- construct the native collector;
- bind the configured TCP listener;
- sample and serve the existing API;
- remain in the foreground until normal shutdown;
- require no systemd, launchd, `sudo`, PID file, fork, or backgrounding mechanism;
- never invoke a service manager.

The existing Windows hidden `greggd service` entry point remains separate and is not converted into `run`.

### Unix `start`, `stop`, and `restart`

On Linux and macOS, remove the executable-owned lifecycle commands rather than replacing them with another supervisor implementation.

The Unix CLI should direct users to:

```text
greggd run
```

for direct foreground execution, or to the externally installed supervisor when they intentionally deploy one:

```text
systemctl ...
launchctl ...
```

Do not implement `start` as a background fork, do not synthesize `stop` through process matching, and do not retain no-op or always-failing Unix lifecycle subcommands merely for compatibility.

Windows may retain `start`, `stop`, and `restart` where they map to the already-implemented native SCM lifecycle. Keep the recently completed Windows dispatcher, service worker, readiness, shutdown, and smoke architecture unchanged.

### `greggd croncheck`

`croncheck` becomes a read-only health probe suitable for cron, shell scripts, non-systemd Linux, containers, and other supervisors.

Required semantics:

- load the same resolved daemon configuration used by `run`;
- derive a connectable local probe address from the configured bind address;
- issue a bounded HTTP request to the existing `GET /v2/healthz` endpoint;
- exit `0` only for HTTP `200`;
- exit nonzero for connection refusal, timeout, malformed HTTP, premature EOF, or any non-200 response including `503`;
- never start, stop, restart, repair, install, enable, or otherwise mutate a process or service;
- never invoke `systemctl`, `launchctl`, SCM control, `sudo`, or a shell;
- remain useful when no service manager exists.

Probe target normalization must handle wildcard listeners:

```text
0.0.0.0 -> 127.0.0.1
::      -> ::1
```

Specific configured addresses are probed directly. Preserve the configured TCP port.

Use a small fixed connect/read/write timeout appropriate for a local health check. Do not add a new configuration field solely for `croncheck`.

The health probe only needs the HTTP status line. Do not deserialize the full health schema merely to decide success, and do not add a dependency such as `reqwest` or another HTTP client crate. Prefer a small bounded `std::net::TcpStream` implementation or an already-linked primitive if inspection proves it is smaller and equally clear.

Successful output should be concise and script-friendly, for example:

```text
greggd healthy
```

Failure should produce one concise diagnostic on stderr and a nonzero exit status. Exact wording may follow existing error conventions; tests should assert semantics rather than brittle full strings.

### `greggd host` and `greggd port`

On Unix, these commands must only mutate and atomically persist configuration. They must not call a service manager or restart a daemon.

A short message may state that the new value applies on the next daemon start/restart, but do not introduce signaling, PID discovery, hot reload, or background process management.

Do not reopen Windows SCM behavior solely to make this correction. If the existing Windows implementation needs a small compile-time branch to preserve its current post-mutation restart behavior, retain it. The required invariant for this phase is that Linux/macOS config mutation performs no service-manager operation.

### Explicit `version` commands

Add:

```text
greggd version
gregg version
```

Both commands must:

- print the binary name and package/workspace version derived at compile time from Cargo metadata;
- exit `0`;
- require no config file;
- perform no filesystem mutation;
- perform no networking;
- perform no service-manager action;
- for `gregg`, not initialize or enter the TUI;
- preserve the existing Clap-generated `--version` behavior unless removal is required for a concrete conflict.

Expected output shape:

```text
greggd <version>
gregg <version>
```

Use `env!("CARGO_PKG_VERSION")` or the equivalent compile-time package version. Do not add generated version files, build scripts, Git SHA embedding, dirty-tree detection, or release metadata machinery.

## Scope

### In scope

- Correct Linux/macOS runtime separation from systemd/launchd.
- Remove Unix service-manager dispatch from normal executable commands.
- Remove dead Unix service-manager source if it becomes unreferenced.
- Preserve optional systemd and launchd packaging assets.
- Convert `croncheck` to an HTTP health probe.
- Normalize wildcard bind addresses to loopback for local probing.
- Make Unix `host`/`port` config-only operations.
- Preserve completed Windows SCM runtime architecture.
- Add `version` subcommands to `greggd` and `gregg`.
- Add focused unit tests.
- Run the existing local check.
- Run one real local Ubuntu end-to-end daemon/server smoke using the built binary.
- Update directly affected README/architecture/help text so service ownership and `croncheck` semantics are truthful.

### Out of scope

- Self-daemonization, double-forking, background modes, PID files, lock files, or process registries.
- Generic init-system detection.
- OpenRC, runit, s6, supervisord, Docker, Kubernetes, or other supervisor integrations.
- New systemd or launchd installation behavior.
- Automatic privilege escalation or embedded `sudo`.
- Hot reload, signals for config reload, or daemon IPC control.
- A new health protocol or endpoint.
- Authentication, TLS, public-internet hardening, or API schema changes.
- Reworking Windows SCM dispatcher/readiness/shutdown logic.
- New runtime dependencies for HTTP probing unless existing standard-library primitives are demonstrably insufficient.
- New CI jobs, workflow steps, matrices, artifacts, evidence bundles, or release gates.
- Moving the Ubuntu end-to-end check into CI.

## Expected files

Likely implementation surface:

```text
crates/greggd/src/main.rs
crates/greggd/src/cli.rs
crates/greggd/src/service/mod.rs        # reduce/gate to Windows-only ownership as appropriate
crates/greggd/src/service/systemd.rs    # delete if no longer referenced
crates/greggd/src/service/launchd.rs    # delete if no longer referenced
crates/greggd/src/service/windows.rs    # only if compile-time dispatch requires a minimal adjustment
crates/gregg/src/cli.rs
crates/gregg/src/main.rs                # only if needed to keep version independent of TUI/config setup
crates/greggd/README.md
packaging/README.md
architecture/greggd-daemon.md
plans/076-native-runtime-croncheck-and-version-correction.md
plans/README.md                         # closure/status update only after implementation
```

Do not create a new crate, service-control crate, HTTP-client module hierarchy, integration harness, or evidence directory for this phase.

## Implementation sequence

### Step 1: lock the corrected CLI contract with parsing tests

Before removing service dispatch, add focused parser tests for the intended platform-visible commands.

For `greggd`:

- `run` remains accepted.
- `croncheck` remains accepted.
- `host` and `port` remain accepted.
- `version` is accepted.
- Linux/macOS builds do not expose `start`, `stop`, or `restart` if those commands are gated to Windows.
- Windows continues to expose `start`, `stop`, and `restart`.
- existing hidden Windows `service` command remains hidden and Windows-only in behavior.

For `gregg`:

- `version` is accepted alongside the current endpoint/config commands.
- no-subcommand behavior still enters the TUI.

Keep Clap's existing derive model. Do not introduce a second parser or command layer.

### Step 2: sever Unix runtime dispatch from service managers

At the `greggd` binary/CLI boundary, make Linux and macOS ordinary commands independent of `platform_service_manager()`.

Preferred shape:

```text
run        -> daemon runtime
croncheck  -> local HTTP health probe
host/port  -> config mutation only on Unix
version    -> print compile-time version
```

Windows service lifecycle commands may continue to construct/use the existing Windows SCM manager.

Once no Linux/macOS runtime path needs the service abstraction:

- remove `SystemdManager` from runtime selection;
- remove `LaunchdManager` from runtime selection;
- delete the Unix manager modules if they have no remaining production purpose;
- gate retained service abstractions to Windows where practical rather than leaving unused Unix code compiled into the executable.

A source search under `crates/greggd/src` must show no production `Command::new("systemctl")` or `Command::new("launchctl")` after this phase.

Do not modify `packaging/systemd/greggd.service`, the launchd plist, or installer scripts merely because runtime wrappers are removed. Those assets are the proper place for optional supervisor integration.

### Step 3: separate config mutation from Unix restart

Refactor `mutate_and_restart` only as much as necessary.

Prefer a small primitive such as:

```text
mutate_config(...)
```

that:

1. loads/defaults configuration using the existing explicit-path rules;
2. applies one mutation;
3. validates the complete config;
4. atomically writes it;
5. returns success without process-management side effects.

Linux/macOS `host` and `port` use this path directly.

If Windows must preserve its existing automatic SCM restart for compatibility, layer that action at the Windows command boundary after successful persistence. Do not keep a generic cross-platform service dependency inside the mutation helper.

Regression tests must prove Unix config mutation succeeds with a fake/temp config and performs no service call.

### Step 4: implement the bounded `croncheck` health probe

Keep the health check small and synchronous unless existing code structure makes a tiny async implementation materially simpler.

Recommended decomposition:

```text
probe_address(config.host) -> connectable IpAddr
croncheck_target(config)    -> SocketAddr
probe_health(target)        -> Result<Healthy, CroncheckError>
```

The implementation should:

1. map wildcard v4/v6 bind addresses to loopback;
2. connect with a short timeout;
3. set bounded read/write timeouts;
4. send a minimal HTTP/1.1 `GET /v2/healthz` request with `Connection: close`;
5. read enough bytes to obtain the first response line;
6. accept only a syntactically valid `HTTP/1.x 200` response;
7. return a typed/small error for timeout, connection failure, malformed status, or unhealthy status.

Do not read an unbounded response body. A small fixed buffer or bounded line read is sufficient.

Do not implement retries. Cron or the caller already supplies repetition semantics.

Required focused tests:

- wildcard IPv4 maps to `127.0.0.1`;
- wildcard IPv6 maps to `::1`;
- specific v4/v6 addresses remain unchanged;
- local test listener returning HTTP 200 makes `croncheck` succeed;
- HTTP 503 makes it fail;
- malformed status line makes it fail;
- closed/refused port makes it fail within the bounded timeout;
- failure does not call any start/restart path;
- success performs no config mutation.

Use a tiny in-test `TcpListener` where needed. Do not add a mock HTTP framework.

### Step 5: add explicit version commands

For each binary, keep version rendering trivial and testable.

A small helper is sufficient:

```rust
fn version_string() -> String {
    format!("greggd {}", env!("CARGO_PKG_VERSION"))
}
```

and the corresponding `gregg` helper.

Do not couple version output to config resolution or runtime initialization.

Required tests:

- parser accepts `greggd version`;
- parser accepts `gregg version`;
- rendered string begins with the correct binary name;
- rendered version equals `env!("CARGO_PKG_VERSION")`;
- `version` exits successfully without requiring an existing config path.

Preserve `greggd --version` and `gregg --version` as generated by Clap unless a direct conflict is observed.

### Step 6: update documentation narrowly

Correct only statements made false by this phase.

At minimum document:

- `greggd run` is the normal foreground/native daemon command;
- Unix systemd/launchd lifecycle is externally operated and optional;
- Linux/macOS `greggd` does not invoke `systemctl`/`launchctl`;
- `croncheck` probes `/v2/healthz` and never starts a daemon;
- `host`/`port` on Unix update config but do not restart the process;
- `greggd version` and `gregg version` exist;
- Windows SCM lifecycle remains supported separately.

Do not rewrite unrelated architecture sections or expand deployment documentation into a supervisor guide.

## Verification

### Focused tests

Run at minimum:

```bash
cargo fmt --all -- --check
cargo test -p greggd cli
cargo test -p greggd --bin greggd
cargo test -p greggd
cargo test -p gregg cli
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

If a package-specific test selector does not match the repository's exact test naming, use the nearest existing package-level invocation rather than adding a harness just to preserve these command strings.

### Runtime-coupling source check

On the Ubuntu implementation host, verify no Unix service-manager command remains in daemon production source:

```bash
rg 'Command::new\("systemctl"\)|Command::new\("launchctl"\)' crates/greggd/src
```

Expected result: no Linux/macOS production runtime invocation. Windows SCM APIs are outside this check.

A broader documentation/package search may still find `systemctl` and `launchctl` in `packaging/` and README files; that is expected and correct.

## Required local Ubuntu end-to-end run

This phase is not complete from unit tests alone. Run the real built daemon on the current Ubuntu host and prove that it serves the API without systemd or sudo.

Do not move this check into GitHub Actions. Do not add a persistent smoke-test script unless the existing repository already has an appropriate local script that can be extended with less code than a one-off shell sequence.

### 1. Build real binaries

```bash
cargo build --release -p greggd -p gregg
```

Use `target/release/greggd` and `target/release/gregg` for the rest of the smoke.

### 2. Verify explicit version commands

```bash
./target/release/greggd version
./target/release/gregg version
```

Both must print the current Cargo package/workspace version and exit `0` without requiring config files or service-manager access.

### 3. Create an isolated temporary config

Use a temporary directory owned by the current user. Select an unused loopback TCP port on the current Ubuntu host and write a minimal valid config using:

```toml
name = "greggd-e2e"
host = "127.0.0.1"
port = <unused-port>
sample_interval_ms = 250
stale_after_ms = 5000
```

Do not write `/etc/gregg`, install a unit, call `sudo`, or alter the host's existing greggd installation.

### 4. Start the daemon directly

Run:

```text
target/release/greggd run --config <temp-config>
```

as the current user in the background only for purposes of this shell smoke, capturing its PID and stdout/stderr so cleanup is deterministic.

The command itself must remain the normal foreground daemon; the shell is responsible for backgrounding during the test.

### 5. Prove the live server is functional

Poll for a short bounded startup window, then verify:

```text
GET /v2/healthz -> HTTP 200
GET /v2/status  -> HTTP 200 with valid JSON
```

The status JSON must at minimum show:

- configured `system.name == "greggd-e2e"`;
- nonempty native hostname;
- numeric CPU/memory fields in the existing schema;
- no service-manager error in daemon logs.

Use `curl` and an existing JSON utility if present. Do not add a repository dependency for the smoke.

### 6. Prove `croncheck` is a real health probe

While the daemon is live:

```bash
./target/release/greggd --config <temp-config> croncheck
```

must exit `0`.

Then terminate the daemon normally, wait for the process to exit, and run the same command again.

It must:

- exit nonzero;
- report the failed/unhealthy probe;
- return promptly within the bounded timeout;
- not start a new daemon;
- leave the endpoint unreachable afterward.

This live/dead transition is the operational proof that `croncheck` is observational rather than supervisory.

### 7. Prove no systemd dependency was exercised

The entire E2E sequence must complete without:

```text
sudo
systemctl
service
loginctl
pkexec
```

and without installing/enabling a systemd unit.

The presence of systemd on the Ubuntu host is irrelevant; the test passes only because greggd does not use it.

### 8. Cleanup

Always terminate the spawned daemon and remove the temporary directory/config. If any assertion fails, cleanup must still run before reporting the failure.

Record the commands and concise pass/fail result in the implementation handoff or this plan's completion section. Do not create an evidence bundle or commit generated logs.

## No-CI rule for this phase

The local Ubuntu E2E run is sufficient operational verification for the Unix correction.

Do not:

- add a GitHub Actions step for the daemon E2E;
- add a privileged runner;
- start systemd inside CI;
- add containerized init-system tests;
- add an artifact upload;
- require a green workflow run solely to close this plan.

Ordinary existing CI may run naturally after implementation, but it is not an acceptance gate for Plan 076 and must not be modified for this work.

## Acceptance criteria

### Unix runtime boundary

- [ ] `greggd run` on Linux starts the daemon directly and has no service-manager dependency.
- [ ] Linux/macOS production source no longer invokes `systemctl` or `launchctl`.
- [ ] Linux/macOS `start`, `stop`, and `restart` are removed from the executable-owned lifecycle contract rather than replaced by self-daemonization or process discovery.
- [ ] Optional systemd and launchd packaging assets remain available for manual/operator-controlled installation.
- [ ] No `sudo`, privilege escalation, PID file, fork/background daemon mode, or generic supervisor abstraction is added.
- [ ] Completed Windows SCM dispatcher/runtime/readiness/shutdown architecture remains intact.

### `croncheck`

- [ ] `greggd croncheck` performs a bounded HTTP probe of `/v2/healthz`.
- [ ] HTTP 200 returns exit `0`.
- [ ] HTTP 503 and other non-200 statuses return nonzero.
- [ ] Connection refusal, timeout, malformed HTTP, and premature EOF return nonzero with one useful diagnostic.
- [ ] `0.0.0.0` is probed through `127.0.0.1`; `::` is probed through `::1`.
- [ ] Specific configured bind addresses remain unchanged for the probe target.
- [ ] `croncheck` never starts, stops, restarts, repairs, installs, or enables greggd.
- [ ] `croncheck` works without systemd/launchd and requires no administrator privileges for an ordinary unprivileged-port deployment.
- [ ] No new HTTP client/runtime dependency is introduced solely for the probe.

### Configuration mutation

- [ ] Linux/macOS `greggd host` atomically updates config without invoking a service manager.
- [ ] Linux/macOS `greggd port` atomically updates config without invoking a service manager.
- [ ] Existing validation, explicit-path handling, and atomic-write guarantees remain intact.
- [ ] No hot reload, signal control, or process discovery is added.

### Version commands

- [ ] `greggd version` exists, prints `greggd <Cargo version>`, and exits `0`.
- [ ] `gregg version` exists, prints `gregg <Cargo version>`, and exits `0`.
- [ ] Both commands work without an existing config file.
- [ ] `gregg version` does not enter the TUI.
- [ ] Neither version command performs networking or service management.
- [ ] Existing `--version` behavior remains functional unless a concrete Clap conflict requires adjustment.
- [ ] No build script, Git metadata embedding, or generated version file is introduced.

### Focused verification

- [ ] Formatting passes.
- [ ] Focused `greggd` CLI/runtime tests pass.
- [ ] Focused `gregg` CLI tests pass.
- [ ] Full package tests for directly affected crates pass.
- [ ] `./scripts/check-local.sh` passes.
- [ ] Source search confirms no production Linux/macOS `systemctl`/`launchctl` invocation under `crates/greggd/src`.

### Required Ubuntu E2E

- [ ] Release `greggd` and `gregg` binaries build on the current Ubuntu host.
- [ ] Both explicit `version` commands execute successfully from the built binaries.
- [ ] A temporary user-owned config with a free loopback port is created without touching `/etc`.
- [ ] `target/release/greggd run --config <temp-config>` starts directly without `sudo` or systemd.
- [ ] The live daemon reaches HTTP 200 on `/v2/healthz`.
- [ ] The live daemon serves valid HTTP 200 JSON from `/v2/status` with `system.name == "greggd-e2e"`.
- [ ] `greggd croncheck` returns `0` while that daemon is healthy.
- [ ] After the daemon is terminated, `greggd croncheck` returns nonzero promptly.
- [ ] The failed `croncheck` does not start a replacement process and the endpoint remains unreachable.
- [ ] The complete E2E uses no `sudo`, `systemctl`, `service`, `loginctl`, or `pkexec`.
- [ ] The temporary process/config are cleaned up.
- [ ] This E2E is executed locally only; no CI workflow is added or modified for it.

### Scope closure

- [ ] Documentation describes `run`, `croncheck`, Unix supervisor ownership, config mutation, and both version commands truthfully.
- [ ] No unrelated collector, protocol, TUI, EggPool, scheduler, drive, release, or CI work is included.
- [ ] No evidence file or follow-up closure-only plan is created.

## Handoff

Implementation handoff should report only the material outcome:

```text
Implementation SHA: <sha>
Unix runtime boundary: <systemd/launchd removed from greggd runtime; packaging retained>
croncheck: <live HTTP health probe semantics and timeout>
Config mutation: <Unix host/port no longer restart services>
Version commands: <greggd version / gregg version outputs>
Focused verification: <tests + check-local result>
Ubuntu E2E: <run command, health/status result, live croncheck result, stopped croncheck result>
CI changes: none
Remaining work: none / concrete defect only
```

## Implementation record

Implementation SHA is recorded after the implementation commit. Unix runtime
ownership is now native foreground execution; systemd and launchd source
adapters were removed while packaging assets remain. `croncheck` probes
`/v2/healthz` over a bounded TCP connection and never starts a process. Unix
`host` and `port` persist atomically without service-manager calls; Windows
retains SCM lifecycle and post-mutation restart behavior. `greggd version` and
`gregg version` print their compile-time package versions.

Verification completed locally:

- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `./scripts/check-local.sh`
- `cargo build --release -p greggd -p gregg`
- Ubuntu end-to-end run with a temporary user-owned config: live `/v2/healthz`
  and `/v2/status`, live `croncheck`, and nonzero stopped-daemon `croncheck`
- Source search found no `systemctl` or `launchctl` command invocation under
  `crates/greggd/src`.

CI changes: none.

Do not create a separate evidence document. After implementation and the required local Ubuntu E2E pass, mark this plan complete and update `plans/README.md` directly.
