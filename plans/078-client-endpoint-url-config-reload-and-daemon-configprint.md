# Phase 078: client endpoint URL input, runtime config reload, and daemon `configprint`

Status: complete.

Depends on: Plan 077 complete at repository baseline `32144c9393eaff567ca083d069b77ee8cc9a64b4`.

## Objective

Correct the concrete client endpoint-staleness failure observed during local Gregg testing, accept ordinary HTTP URL input as a convenience for `gregg add`, and add one minimal read-only `greggd configprint` command.

This phase is intentionally small. It does not add discovery, a file-watcher service, a general URL-routing model, TLS, daemon reload semantics, or another configuration framework.

The required product behavior after this phase is:

1. a running Gregg TUI must not remain permanently pinned to a stale startup endpoint after the persisted system entry is corrected; `Ctrl-R` becomes the explicit bounded boundary that reloads the current client config, reconciles system endpoints, and immediately polls the reloaded targets;
2. `gregg add` accepts either the existing endpoint forms or an HTTP URL such as `http://192.168.183.143:11310/`, extracts only the host and optional explicit port, and persists the existing canonical `host` + `port` representation;
3. `greggd configprint` loads the same daemon config selected by the rest of the CLI and writes exactly the configured bind `host:port` to stdout, with no `http://` prefix and no process/network side effects.

The originating live report identified `192.168.183.143:11310` as the
verified working `greggd` endpoint while the running client displayed/polled
the stale `192.168.182.143:11310` address. The later closure environment had
changed: `.183` was unavailable and `.182` returned ready health, so the final
operational smoke exercised the reverse `.183` -> `.182` replacement direction.
These are separate environment observations; the later smoke does not rewrite
the original report or prove that `.183` was never working.

## Baseline findings

### 1. Gregg freezes its polling endpoint list at TUI startup

Current `crates/gregg/src/main.rs` does this once in `run_tui()`:

```text
ConfigStore::load_or_default()
    -> AppState::from_config(&config)
    -> config.systems -> Vec<Endpoint>
    -> PollScheduler::run(endpoints, ...)
```

The scheduler then owns that fixed `Vec<Endpoint>` for the life of the TUI.

`Ctrl-R` currently sends only an empty refresh signal. It starts another generation against the same startup endpoint vector. It does not reload the config file and cannot replace the scheduler's endpoint set.

This explains the observed local symptom:

```text
192.168.182.143@192.168.182.143:11310 offline ...
```

while the inspected client config had already been corrected to:

```text
192.168.183.143:11310
```

The renderer itself is not changing `.183` into `.182`. `SystemEntry::to_endpoint()` copies the configured host directly, and the offline row renders `SystemState.endpoint.host` / `display_address()` directly. Therefore the displayed `.182` proves that the running TUI still held the old endpoint in memory.

The narrow correction is to make the existing manual refresh boundary reload system configuration before polling. Do not add an automatic filesystem watcher just to solve this.

### 2. `gregg add` deliberately rejects URL syntax

Current `EndpointSpec::parse()` treats `://` as `EndpointError::HasScheme` and rejects paths. This is appropriate for the canonical persisted endpoint representation, but unnecessarily strict for CLI input.

Users commonly have a daemon address in URL form, for example:

```text
http://192.168.183.143:11310/
http://greggd.local:11310/
http://[fd00::10]:11310/
```

The CLI should accept that input without changing the configuration schema. URL syntax is an input convenience only; configuration remains a normalized host and port.

### 3. There is no one-line daemon bind-address inspection command

`greggd croncheck` answers whether the configured daemon is healthy, and `host` / `port` mutate configuration, but there is no command that simply reports the configured server address.

For local deployment and debugging, the required operation is deliberately simpler than a config dump:

```text
greggd configprint
```

should print one canonical address such as:

```text
0.0.0.0:11310
192.168.183.143:11310
[::]:11310
[fd00::10]:11310
```

and exit.

It must print the configured bind address, not `croncheck_target()`'s wildcard-to-loopback probe address.

## Authoritative behavior after Plan 078

### Gregg TUI runtime configuration behavior

Startup remains unchanged: Gregg resolves one config path, loads it, builds state, and starts the polling scheduler.

`Ctrl-R` changes from:

```text
refresh existing startup endpoints now
```

to:

```text
reload the current client config from the already-resolved ConfigStore
    -> reconcile system entries into AppState
    -> replace the scheduler's active endpoint list
    -> immediately poll the reloaded endpoint list
```

This is an explicit manual reload boundary, not continuous hot reload.

Normal periodic polling may continue using the current in-memory endpoint set until `Ctrl-R` or process restart. Do not add `notify`, inotify/kqueue/ReadDirectoryChangesW integration, a metadata-polling loop, or a second timer merely to watch configuration.

If reload fails because the config is missing, unreadable, malformed, or invalid, keep the last known-good in-memory systems and polling endpoints. The TUI must not partially apply an invalid config and must not crash merely because an external edit is temporarily invalid. A plain refresh of the last known-good endpoints is acceptable on that failure path. Do not build a new persistent error-pane subsystem in this phase.

The reload applies to the configured **system endpoint list**. Do not turn this phase into dynamic reconfiguration of every TUI subsystem. In particular, live EggPool worker replacement, arbitrary refresh-cadence changes, and runtime replacement of every global config field remain out of scope unless a very small implementation falls out naturally from the same code path without expanding the architecture.

### System reconciliation rules

When a valid config is reloaded, reconcile `config.systems` into `AppState.systems` by stable system ID.

For an unchanged system ID whose host, port, and configured name are unchanged:

- preserve reachability;
- preserve the latest normalized snapshot;
- preserve success/attempt timestamps and latency;
- preserve the last poll error until the replacement poll result arrives.

For an existing system ID whose host or port changed:

- replace `SystemState.endpoint` with the newly configured endpoint;
- update `configured_name`;
- set reachability to `Pending` before the new target is polled;
- clear the old target's snapshot, latency, timestamps, and last error so metrics from the previous machine cannot be displayed under the new address.

For an existing ID whose only change is the configured display name:

- update the display name;
- retain the endpoint and current metrics/reachability.

For a newly added system:

- add it in config order;
- initialize it as `Pending` with no snapshot/error/timestamps.

For a removed system:

- remove it from state;
- do not retain it merely because an old poll result exists.

Selection and viewport behavior:

- preserve the selected ID if it still exists;
- if the selected system was removed, select the first remaining system using the existing stable semantics;
- repair `viewport_top_id` through the existing visibility helper rather than inventing separate viewport rules;
- do not disturb the active top-level pane, view mode, or drive-expansion preference solely because systems were reconciled.

Generation safety:

- a result from a poll generation that belonged to the superseded endpoint set must not overwrite state for the replacement target;
- endpoint replacement must therefore either establish a fresh scheduler generation domain or otherwise ensure old in-flight results are ignored;
- do not rely only on matching `system_id` when a manually edited config can keep the same UUID while changing host/port.

A simple and acceptable design is to cancel/restart only the system poll scheduler on successful endpoint replacement and reset the system poll generation boundary. Another acceptable design is a scheduler command that atomically replaces endpoints and starts a new generation while preventing superseded results from being applied. Choose the smaller design after implementation inspection.

Do not reuse the process-wide cancellation token in a way that also shuts down the TUI or EggPool worker when only the systems scheduler is being replaced. If scheduler restart is chosen, use a child/scheduler-local cancellation token or equivalent small ownership boundary.

### `gregg add` endpoint input behavior

Keep `EndpointSpec` / persisted endpoint semantics centered on canonical host + port. URL parsing is a CLI input adapter, not a new persisted endpoint type.

Required accepted forms for `gregg add`:

```text
192.168.183.143
192.168.183.143:11310
greggd.local
greggd.local:11310
[fd00::10]:11310
fd00::10
http://192.168.183.143:11310/
http://greggd.local:11310/
http://[fd00::10]:11310/
```

For an HTTP URL:

1. require scheme `http` exactly, case-insensitively if the chosen standards-compliant parser already normalizes schemes;
2. require a host;
3. reject credentials/userinfo rather than silently discarding them;
4. extract the host and optional **explicitly supplied** port;
5. ignore URL path/query/fragment after syntactic validation because Gregg always polls its own fixed status routes and persists only the authority;
6. normalize the extracted host through the same canonical host validation used by ordinary endpoint input;
7. do not persist `http://`, `/`, query text, fragments, credentials, or any URL path.

Port semantics are important:

- `http://host:11310/` -> explicit port `11310`;
- `http://host/` -> no explicit port, so `cmd_add()` continues to use Gregg's configured `default_port` rather than HTTP's conventional port 80;
- if the user explicitly writes `http://host:80/`, preserve `80` as an explicit port even if a URL library would otherwise normalize the default HTTP port away.

This means the implementation must retain `port_was_explicit` correctly for URL input. Add a regression test for explicit `:80` so this does not depend on URL-library normalization behavior.

Unsupported schemes:

```text
https://host:11310/
ftp://host:11310/
```

must fail with a clear unsupported-scheme diagnostic. Gregg's system poller is plain HTTP today. Silently accepting `https://` and then polling it over HTTP would be incorrect and potentially surprising.

Malformed URL input and URLs without a host must fail with the existing endpoint-error exit-code class.

Do not add TLS support in this phase.

### Parser placement and dependency rule

Prefer a small adapter such as:

```text
parse_add_endpoint_input(input) -> EndpointSpec
```

that:

- recognizes an HTTP URL input;
- extracts authority information safely;
- then routes the host through existing endpoint normalization;
- otherwise delegates directly to the current `EndpointSpec::parse()`.

This keeps strict canonical endpoint parsing reusable for persisted config and exact matching while allowing `gregg add` to be more ergonomic.

Do not introduce a new URL dependency solely for this change. `gregg` already depends on `reqwest`; using its existing URL facilities is acceptable if they preserve the required explicit-port semantics. A small bounded authority parser is also acceptable. Do not write a broad hand-rolled RFC 3986 implementation.

Only `gregg add` is required to accept URL form in this phase. Do not expand `remove`, persisted TOML syntax, EggPool configuration, or unrelated CLI commands unless sharing a tiny helper makes the behavior unavoidable and tests explicitly lock the result.

### `greggd configprint` behavior

Add an explicit `configprint` subcommand to the `greggd` CLI.

Successful output must be exactly one line containing the configured bind socket address:

```text
<host>:<port>\n
```

IPv6 must use canonical socket-address brackets so output is unambiguous:

```text
[::]:11310
[fd00::10]:11310
```

Implementation should reuse standard `SocketAddr` formatting or one existing canonical formatter rather than duplicating IPv6 bracket logic.

Configuration semantics must match existing daemon commands:

- explicit `--config PATH` -> load that exact file; missing explicit path is an error;
- implicit/default config path exists -> load it;
- implicit/default config path missing -> use `Config::default()` under the current `load_config()` contract;
- validation errors propagate through the existing exit-code mapping.

`configprint` must:

- not call `probe_address()`;
- not turn wildcard addresses into loopback;
- not call `croncheck` or any HTTP endpoint;
- not bind a socket;
- not start/stop/restart the daemon or Windows service;
- not mutate or write the config;
- not print labels such as `host=`, `port=`, `greggd`, or `http://`;
- not print the config path or other fields.

Examples:

```text
$ greggd --config /tmp/greggd.toml configprint
192.168.183.143:11310
```

```text
$ greggd --config /tmp/greggd-v6.toml configprint
[fd00::10]:11310
```

## Scope

### In scope

- Fix the demonstrated stale Gregg system-endpoint behavior at the existing `Ctrl-R` refresh boundary.
- Reload the already-resolved client ConfigStore on `Ctrl-R`.
- Reconcile system entries by stable ID without carrying metrics across a changed host/port.
- Ensure superseded in-flight poll generations cannot overwrite a replacement endpoint.
- Replace/reconfigure the scheduler's system endpoint list and immediately poll it after successful reload.
- Keep last-known-good system state if external config reload fails.
- Add URL-form convenience parsing for `gregg add` using HTTP URLs.
- Preserve URL explicit-port semantics and Gregg's own default-port semantics.
- Keep the persisted system schema as `host` + `port` only.
- Add `greggd configprint` with exact one-line host:port output.
- Add focused tests for all three behaviors.
- Retain the originating `.183`-working/`.182`-stale report and separately record
  the later local Ubuntu smoke against the reachable `.182` daemon.
- Update only directly affected user/architecture documentation and planning records.

### Out of scope

- Automatic filesystem watchers or continuous hot reload.
- New background config-monitor threads/tasks.
- inotify, kqueue, FSEvents, or ReadDirectoryChangesW integration.
- A generalized dynamic-config control plane.
- Daemon SIGHUP/reload support.
- Dynamic replacement of the EggPool worker unless a trivial shared reload falls out with no extra architecture.
- TLS or HTTPS polling.
- Accepting `https://` by silently downgrading to HTTP.
- Discovery, mDNS browsing, service registries, or endpoint probing during `gregg add`.
- Persisting URL schemes or paths in Gregg config.
- Changing the config schema version.
- Adding a new URL-parsing dependency when existing facilities suffice.
- Turning `configprint` into a full TOML/JSON config dump.
- Process discovery or service-manager behavior in `configprint`.
- Changes to `greggd croncheck` semantics from Plans 076-077.
- Changes to Unix/Windows daemon runtime ownership or Windows SCM behavior.
- New GitHub Actions workflows/jobs/matrices/artifacts/evidence bundles.
- Release automation or publication work.
- Broad TUI redesign or unrelated diagnostics/UI polishing.

## Expected files

Primary implementation surface:

```text
crates/gregg/src/main.rs
crates/gregg/src/state.rs
crates/gregg/src/scheduler.rs
crates/gregg/src/endpoint.rs
crates/gregg/src/cli.rs
crates/greggd/src/cli.rs
```

Likely focused documentation updates:

```text
README.md
crates/gregg/README.md
crates/greggd/README.md
architecture/gregg-client.md
architecture/greggd-daemon.md
plans/078-client-endpoint-url-config-reload-and-daemon-configprint.md
plans/README.md
```

Touch `crates/gregg/src/action.rs`, `event.rs`, or UI files only if required to accurately rename/document existing `Ctrl-R` semantics. There should be no new key binding.

Do not create a new crate or a generalized config/runtime module for this phase. One small scheduler command/helper and one state reconciliation helper are preferable to a new abstraction hierarchy.

## Implementation sequence

### Step 1: lock the observed stale-endpoint failure with focused tests

Before changing runtime behavior, add deterministic tests that prove the present source-of-truth contract and the required correction.

At minimum cover:

1. `AppState::from_config()` with host `192.168.183.143` produces a `SystemState.endpoint.host` of exactly `192.168.183.143`.
2. The offline renderer for that state contains `.183`, proving the renderer does not transform the address.
3. A state initially configured for `192.168.182.143:11310` can be reconciled with the same system ID changed to `192.168.183.143:11310`.
4. After reconciliation the state endpoint is `.183`, reachability is `Pending`, and old `.182` snapshot/error/latency/timestamps are cleared.
5. An unchanged endpoint retains its current successful snapshot/reachability across reconciliation.
6. A name-only change updates the configured name without discarding the snapshot.
7. Added and removed IDs are reflected correctly.
8. Selection is retained when possible and repaired when the selected entry is removed.

These tests belong primarily in `state.rs` and existing UI buffer tests. Do not add a separate integration harness for state reconciliation.

### Step 2: add a narrow AppState system-reconciliation operation

Add one explicit reducer/helper, for example:

```text
AppState::reconcile_systems(&Config)
```

or a systems-only equivalent.

Implementation rules:

- reconcile by stable UUID, not host string;
- compare old/new host and port before deciding whether metrics are reusable;
- never retain a snapshot from an old target after host/port change;
- preserve unchanged target state;
- preserve non-system TUI settings;
- call existing selection/viewport repair helpers rather than duplicating navigation logic;
- keep ordering equal to the new config's system order within each current online/offline grouping rule.

Do not make `SystemState` independently load configuration.

### Step 3: make the scheduler accept an atomic endpoint replacement at refresh

Replace the current `mpsc::Receiver<()>` refresh signal with the smallest internal command shape that can distinguish ordinary refresh from replacement/reload, for example:

```text
SchedulerCommand::Refresh
SchedulerCommand::ReplaceEndpointsAndRefresh(Vec<Endpoint>)
```

The exact names are not important.

Required scheduler behavior:

- ordinary refresh keeps current endpoint list;
- replacement command swaps the active endpoint list before starting the next generation;
- replacement immediately produces a poll generation; it must not wait for the next periodic tick;
- no old endpoint remains in subsequent generations after successful replacement;
- bounded concurrency remains unchanged;
- current fixed-cadence periodic behavior remains unchanged for ordinary refreshes;
- cancellation remains prompt;
- no extra worker thread/task per endpoint is introduced beyond the existing scheduler design.

Generation correctness is mandatory. If an old generation can still complete after replacement, its results must be rejected. Prefer cancelling/awaiting the old generation or using a fresh scheduler-generation identity rather than layering ad hoc host comparisons into `AppState::apply_batch()`.

If it is smaller and clearer to replace the whole system scheduler instead of adding a replacement command, do that. In that case, isolate scheduler cancellation from process-wide cancellation and retain the same visible semantics.

Add scheduler tests proving that a replacement command changes the actual address requested. A pair of loopback test servers with distinct counters/responses is sufficient; no TUI is needed for this layer.

### Step 4: wire `Ctrl-R` to config reload + endpoint replacement

Keep the resolved `ConfigStore` available to the TUI event loop instead of using it only during startup.

When `Action::RefreshNow` is received while the Systems pane is active:

1. load the current config from that exact store/path;
2. if load/validation succeeds:
   - reconcile `AppState.systems`;
   - derive the replacement `Vec<Endpoint>` from the reloaded systems;
   - send the scheduler replacement-and-refresh command;
3. if load/validation fails:
   - preserve the current last-known-good state and endpoint list;
   - do not partially apply entries from the bad file;
   - an ordinary refresh of the existing endpoint list is acceptable;
4. continue to use the existing EggPool-specific Ctrl-R behavior when the EggPool pane is active.

Do not reload the config into a second independently resolved path. `--config`, `XDG_CONFIG_HOME`, `HOME`, and platform-default resolution must remain decided once by the existing CLI/config-store path.

Add a focused `main.rs`/dispatch test around a temporary config store where `.182` is loaded initially, the file is atomically replaced with `.183`, and the refresh path produces a replacement endpoint vector for `.183`.

### Step 5: add HTTP URL input support only at `gregg add`

Add failing parser/CLI tests before implementation.

Required positive tests:

```text
gregg add http://192.168.183.143:11310/
gregg add http://greggd.local:11310/
gregg add http://[fd00::10]:11310/
gregg add http://192.168.183.143/
gregg add http://192.168.183.143:80/
gregg add http://192.168.183.143:11310/v2/status
```

Assertions:

- persisted host contains only the normalized host;
- persisted port is the explicit URL port when supplied;
- missing URL port uses config `default_port`, not 80;
- explicit `:80` remains 80;
- path is not persisted;
- UUID/name/duplicate behavior remains the same as ordinary `add`.

Required negative tests:

```text
https://192.168.183.143:11310/
ftp://192.168.183.143:11310/
http://user:password@192.168.183.143:11310/
http:///missing-host
http://192.168.183.143:0/
http://192.168.183.143:70000/
```

Keep ordinary host/port/IPv6 parser regression tests green.

Implementation must not probe the address during `add`.

### Step 6: add `greggd configprint`

Add the new clap command to `crates/greggd/src/cli.rs` and route it through the existing `dispatch_with_config_intent()` configuration-loading path.

Prefer a tiny formatter/helper such as:

```text
config_address(&Config) -> SocketAddr/String
```

using:

```text
SocketAddr::new(config.host, config.port)
```

for canonical formatting.

Add tests for:

1. IPv4 exact stdout value;
2. IPv6 bracketed exact value;
3. wildcard address is printed as wildcard, not loopback;
4. custom port is respected;
5. missing explicit config path fails;
6. implicit missing config uses current defaults;
7. command does not invoke health probing or service management;
8. command does not mutate the config file.

Avoid brittle testing of global stdout where a pure formatter plus one dispatch-level smoke is smaller. The CLI binary smoke can validate exact stdout.

### Step 7: update directly affected documentation

Update only statements that become inaccurate.

At minimum:

- Gregg CLI examples/documentation should show both `host:port` and HTTP URL input for `add`.
- Endpoint architecture docs must no longer claim that every CLI add input with a scheme/path is rejected; distinguish canonical endpoint parsing from add-command URL convenience.
- TUI key documentation should describe `Ctrl-R` as reloading system config and refreshing the Systems pane.
- `greggd` command documentation should include `configprint` and its one-line output contract.
- Daemon architecture should state that `configprint` is read-only config inspection and does not probe or mutate runtime state.

Do not expand documentation into a general troubleshooting guide or configuration framework rewrite.

### Step 8: focused local verification

Run at minimum:

```bash
cargo fmt --all -- --check
cargo test -p gregg endpoint
cargo test -p gregg state
cargo test -p gregg scheduler
cargo test -p gregg cli
cargo test -p gregg --bin gregg
cargo test -p greggd cli
cargo test -p greggd --bin greggd
./scripts/check-local.sh
```

If exact test filters differ from module naming, use the closest package-local focused commands and record the actual commands in this plan on completion.

Also run source checks demonstrating scope control:

```bash
rg 'notify|inotify|kqueue|ReadDirectoryChanges' crates/gregg Cargo.toml crates/*/Cargo.toml
rg 'https://' crates/gregg/src
```

Interpret matches; documentation/tests may contain `https://` negative cases. There must be no new watcher dependency/runtime and no HTTPS polling implementation.

No new GitHub Actions work is required.

### Step 9: local Ubuntu end-to-end smoke against the real daemon

The final closure smoke used the later environment's reachable daemon at:

```text
192.168.182.143:11310
```

This smoke is useful evidence because the originating bug was observed with a
real Gregg/greggd pair, but its reachable address is a later environment fact,
not a replacement for the originating `.183`-working report.

#### A. Verify the daemon is genuinely reachable

From the same host that will run Gregg, confirm the live endpoint responds before blaming the client:

```bash
curl -fsS http://192.168.182.143:11310/v2/healthz
```

If `/v2/healthz` is temporarily unavailable for a concrete compatibility reason, use the current supported status/health route and record why. Do not change product code merely to make the smoke convenient.

#### B. Prove URL-form `add` persistence

Use an isolated temporary client config, not the user's normal config:

```bash
tmpdir="$(mktemp -d)"
client_cfg="$tmpdir/gregg.toml"

target/debug/gregg --config "$client_cfg" add http://192.168.182.143:11310/
target/debug/gregg --config "$client_cfg" list --json
```

Require the persisted/listed entry to contain exactly:

```text
host = 192.168.182.143
port = 11310
```

with no scheme/path retained.

#### C. Reproduce and close the stale `.183` -> `.182` TUI failure

Use a temporary config with a stable UUID and intentionally dead/wrong initial host:

```text
192.168.183.143:11310
```

Start Gregg against that explicit config and confirm the TUI shows that target offline.

While the TUI remains running, atomically correct the same config entry to:

```text
192.168.182.143:11310
```

Preserve the same system ID so this tests host replacement rather than remove/add identity replacement.

Press `Ctrl-R` once.

Acceptance requires, without restarting Gregg:

1. the rendered address changes from `.183` to `.182`;
2. no metrics from `.183` remain displayed under `.182` before the new poll succeeds;
3. the immediate replacement poll targets `.182`;
4. the system becomes online using the verified real daemon;
5. subsequent periodic generations continue polling `.182`, not `.183`.

This is the decisive operational proof for the original defect.

#### D. Prove `greggd configprint`

Create a temporary daemon config on the local Ubuntu host and run the built binary directly:

```bash
target/debug/greggd --config "$daemon_cfg" configprint
```

Require exact stdout equal to the configured `host:port` plus one newline.

Run one IPv4 case and one IPv6 formatting case. No daemon process needs to be started for `configprint`.

The smoke must not use `sudo`, `systemctl`, service installation, or CI.

### Step 10: close planning records directly

After implementation and verification:

1. set Plan 078 status to `complete`;
2. mark acceptance criteria only from final source/tests and the real Ubuntu smoke;
3. record the implementation SHA and concise local verification result in this file;
4. update `plans/README.md` to show 078 complete;
5. extend the dependency chain from `... -> 076 -> 077` to `... -> 076 -> 077 -> 078`;
6. retain 078 as new product work rather than misclassifying it as a closure-only plan;
7. do not create Plan 079 solely to mark 078 complete if all criteria below pass.

## Acceptance criteria

### Original stale-endpoint defect

- [x] A regression test proves the renderer/state preserve the exact configured IP and do not transform `.183` into `.182`.
- [x] `Ctrl-R` on the Systems pane reloads the current client config from the same resolved ConfigStore used at startup.
- [x] A valid host/port edit is reconciled into `AppState` without restarting Gregg.
- [x] A host/port change for an existing ID clears the old target's snapshot, reachability, latency, timestamps, and last error before the replacement poll result is applied.
- [x] Unchanged endpoint state is preserved across reload.
- [x] Name-only changes do not discard valid metrics.
- [x] Added/removed system IDs reconcile correctly.
- [x] Selection/viewport remain valid after reconciliation.
- [x] The scheduler polls only the replacement endpoint after reconfiguration.
- [x] Superseded in-flight results cannot overwrite the replacement target's state.
- [x] Invalid/unreadable reloaded config is not partially applied and does not crash the running TUI.
- [x] No filesystem watcher, new config-monitor task, or background hot-reload subsystem is added.

### URL-form `gregg add`

- [x] Existing bare IPv4, DNS, IPv6, and host:port input remains supported.
- [x] `gregg add http://192.168.183.143:11310/` succeeds.
- [x] URL input persists only canonical host + port.
- [x] URL path/query/fragment are never persisted.
- [x] URL without an explicit port uses Gregg's configured `default_port`, not HTTP port 80.
- [x] Explicit `:80` remains explicit port 80.
- [x] Bracketed IPv6 HTTP URL input is parsed correctly and persisted without URL brackets in the host field.
- [x] Credentials/userinfo are rejected.
- [x] `https://` and non-HTTP schemes are rejected rather than silently downgraded.
- [x] Invalid/missing host and invalid port fail with the endpoint-error class.
- [x] `gregg add` does not perform a network probe.
- [x] No new URL parsing dependency is added solely for this feature.
- [x] Config schema/version remains unchanged.

### `greggd configprint`

- [x] `greggd configprint` exists on Linux, macOS, and Windows builds through the common CLI.
- [x] Successful output is exactly one configured `host:port` line with no label and no `http://`.
- [x] IPv4 output is canonical.
- [x] IPv6 output uses unambiguous brackets.
- [x] Wildcard configured addresses remain wildcard in output and are not mapped to loopback.
- [x] Explicit `--config` path semantics match `run`/`croncheck` loading behavior.
- [x] Missing explicit config fails nonzero.
- [x] Default missing config follows the existing default-config contract.
- [x] `configprint` does not probe HTTP, bind a socket, start/stop/restart services, or write config.
- [x] Existing `croncheck`, `host`, `port`, `run`, `version`, and Windows SCM behavior remains unchanged.

### Verification and scope

- [x] Focused Gregg endpoint/state/scheduler/CLI tests pass.
- [x] Focused greggd CLI tests pass.
- [x] `cargo fmt --all -- --check` passes.
- [x] `./scripts/check-local.sh` passes.
- [x] No new watcher/runtime dependency is introduced.
- [x] No new CI workflow/job/matrix/artifact/evidence mechanism is introduced.
- [x] Local Ubuntu URL-add persistence smoke against the `192.168.182.143:11310` target passes without network probing.
- [x] Local Ubuntu running-TUI `.183` -> `.182` Ctrl-R address reconciliation smoke passes without restarting Gregg.
- [x] The reloaded TUI becomes online against the external `192.168.182.143:11310` daemon.
- [x] Local Ubuntu `greggd configprint` IPv4 and IPv6 output smoke passes.
- [x] Directly affected README/architecture text matches final behavior.
- [x] Plan 078 and `plans/README.md` are reconciled after implementation without a closure-only follow-up plan.

## Implementation and verification record

Implementation commit `1867d22` contains the original code and documentation
changes. The originating report was `.183` working / `.182` stale; during the
later closure run `.182` was reachable and `.183` was not, so the live smoke
used `.183` -> `.182`. This demonstrates address replacement without rewriting
the original observation. Plan 079 subsequently corrected the bounded
scheduler replacement-delivery edge found during post-implementation review.
Deterministic
verification passed with `cargo fmt --all -- --check`, the full workspace
default check, `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo doc --workspace --no-deps`, the exact CI Linux test command with
`RUSTFLAGS=-Dwarnings`, and `cargo +1.75 check --workspace --all-features`.
The isolated smoke persisted the HTTP URL correctly, printed exact IPv4 and
IPv6 `configprint` output, and switched a running TUI from `.183` to `.182` on
`Ctrl-R`. The live probe of `http://192.168.182.143:11310/v2/healthz` returned a
ready v2 response, and the reloaded TUI displayed live metrics from that
daemon without restarting Gregg.

## Handoff notes

Treat the `.182`/`.183` symptom as a stale runtime endpoint-set problem, not as an IP parsing arithmetic bug. The existing renderer already proves what address the running process believes it is polling.

Keep the three changes independent at the code level:

- state/scheduler/main changes solve runtime endpoint replacement;
- add-command URL adaptation solves input ergonomics;
- daemon CLI formatting solves config inspection.

Do not couple `configprint` to the client reload work or use URL-form input as a reason to change the persisted configuration schema.

The most important later-environment closure evidence is the local real-host
reproduction: start with `.183`, correct the same stable-ID entry to `.182`,
press `Ctrl-R`, and observe Gregg switch to the reachable live
`192.168.182.143:11310` daemon without a client restart. The originating report
remains that `.183` was the verified working endpoint and `.182` was the stale
address displayed by the running client.
