# Plan 106: bounded daemon status and client offline provenance

Status: planned.

Depends on: Plan 103; preferably after Plans 104-105 so diagnostics are added on the consolidated structure.

## Objective

Improve day-to-day operational diagnosis using information Gregg already computes, without adding a new monitoring subsystem or any remote-control capability.

This plan adds two bounded user-visible improvements:

1. a read-only local `greggd status` command;
2. concise client-side provenance for why an endpoint is offline/unavailable.

## Product boundary

This plan must not add:

- persistent telemetry/history;
- alerts or notification rules;
- process/service inspection beyond Gregg's own startup registration/running state;
- remote start/stop/restart controls;
- TLS/authentication changes;
- a new daemon API schema merely for the CLI status command;
- generalized diagnostics plugins;
- external command scraping such as `ps`/`netstat` for process discovery.

## Baseline

Operational information is currently fragmented across existing surfaces:

```text
greggd version
greggd configprint
greggd croncheck
greggd startup instructions / startup-state helpers
/v2/healthz
client PollOutcome/error classes
```

Operators can determine what is wrong, but often need several commands and must infer whether a failure is configuration, DNS, connection refusal, timeout, invalid Gregg response, warming/failed health, or startup-manager state.

The implementation already contains typed/bounded logic for most of these distinctions. This plan should compose that existing logic rather than add parallel probes.

## Part A: `greggd status`

### Required semantics

Add:

```text
greggd status
```

It is a read-only local diagnostic command. It must not start, stop, restart, install, modify configuration, or write service-manager state.

The command should report, in a stable human-readable format, enough information to answer:

```text
Which greggd version/config is this invocation using?
What address is configured?
Is that configured Gregg endpoint reachable?
If reachable, what health/readiness state does it report?
What startup method/state is detected for this installation?
```

Suggested fields, adjusted to existing terminology:

```text
version: 1.0.x
config: /path/to/greggd.toml
listen: 127.0.0.1:11310
health: ready | warming | failed | unreachable | occupied-by-non-gregg | invalid-response
startup: systemd active | systemd inactive | launchd ... | cron configured | direct/unmanaged | scm ... | unknown
```

Do not promise exact wording in code unless tests intentionally establish it. The important part is deterministic categorization.

### Exit semantics

Choose and document a simple contract.

Recommended:

- exit 0 when configuration can be read and a valid Gregg health endpoint is present in an accepted running state;
- nonzero when configuration is invalid/unreadable, the endpoint is absent/unreachable/ambiguous, or a manager probe itself fails in a way that prevents truthful classification.

A stopped-but-valid installation may be reported clearly as stopped/inactive with a nonzero status result. Do not make `status` mutate state to "fix" it.

If existing CLI exit-code categories can represent these outcomes, reuse them. Do not create a large new exit taxonomy.

### Probe reuse

The status command must reuse the authoritative bounded Gregg health probe used by `croncheck`/restart rather than implement another parser/HTTP client.

The following distinction is important:

```text
configured endpoint absent/unreachable
configured endpoint occupied by something that is not a valid Gregg health endpoint
valid Gregg endpoint warming/ready/failed
```

Do not infer process ownership from port occupancy.

### Startup-state reuse

Reuse `startup_state()` / existing manager detection where possible.

Status must remain read-only. Service-manager queries must stay bounded and preserve existing permission/error behavior. If manager state cannot be determined, report `unknown`/diagnostic context rather than falling back to another manager or changing state.

## Part B: client offline provenance

### Required semantics

The client already has typed polling outcomes. Preserve the current compact offline rendering but make the cause inspectable/visible without turning every row into verbose logs.

Acceptable presentation options include:

- a concise reason suffix in the selected system detail/header;
- a dedicated one-line diagnostic field for the selected offline system;
- a terse stable category in condensed/normal rendering only where it fits without breaking established geometry.

Prefer the least disruptive TUI change.

At minimum distinguish categories that the current poller can already identify reliably, such as:

```text
timeout
DNS/resolve failure
connection refused/unreachable
HTTP error
invalid Gregg response / validation failure
unsupported/incompatible endpoint
```

Do not expose raw unbounded error chains directly into the TUI.

Preserve a bounded, sanitized detail string where useful, but make the stable category primary.

### State model

Offline provenance should travel with the poll result into `AppState` rather than being recomputed by the renderer.

The renderer should consume a small normalized diagnostic representation such as conceptually:

```rust
OfflineReason {
    kind: OfflineKind,
    detail: Option<String>,
}
```

Exact naming is flexible.

Do not make the UI depend directly on `reqwest::Error` or platform DNS error types.

### Recovery behavior

When an endpoint returns to a valid online state, stale offline provenance must be cleared in the same accepted poll generation.

When a newer failure replaces an older one, the newest accepted generation owns the displayed reason.

Existing generation/stale-result protections must remain intact.

## Implementation sequence

### Step 1: identify/share the daemon health-probe primitive

Locate the final authoritative probe used by `croncheck` and restart/update logic.

If necessary, move a small helper into an appropriate daemon module so both CLI commands can call it.

Required properties remain:

- finite connection/read deadline;
- bounded response body;
- strict Gregg health identity/JSON validation;
- no service-manager invocation;
- no mutation.

### Step 2: implement status model and renderer

Keep diagnostic gathering separate from stdout formatting enough to unit-test it deterministically.

Use injected/fake probe and startup-state results where practical.

Do not create a generalized diagnostics framework.

### Step 3: add client normalized offline reason

Map existing poll outcomes/errors into stable application-level categories.

Keep transport-specific details at the poller boundary.

Update state reducer logic so accepted failures store reason provenance and accepted successes clear it.

### Step 4: render minimally

Integrate provenance in the least invasive existing UI location.

Do not reopen Plans 083-087 geometry unless a concrete terminal-width regression is found. Existing compact/fleet alignment invariants remain authoritative.

### Step 5: docs

Update applicable docs:

```text
README.md only if status belongs in quickstart-level command examples
docs/daemon.md
docs/client.md
architecture/greggd-daemon.md
architecture/gregg-client.md
.opencode/skills relevant to daemon/client CLI
CHANGELOG.md
plans/README.md
```

Document that `status` is read-only and local.

## Tests

### Daemon status tests

Cover at minimum:

1. valid Ready endpoint -> running/ready classification;
2. valid Warming endpoint -> valid Gregg but warming classification;
3. valid Failed health -> failed classification without leaking internals;
4. connection refused/absent -> unreachable/stopped classification;
5. occupied endpoint returning non-Gregg data -> non-Gregg/invalid classification;
6. malformed/oversized health body -> bounded invalid response;
7. invalid local config -> configuration error and no mutation;
8. startup manager active/inactive/unknown is rendered from injected state;
9. status never calls install/start/stop/restart paths.

Use loopback/injected fixtures rather than privileged manager setup in ordinary tests.

### Client provenance tests

Cover at minimum:

1. DNS failure maps to DNS category;
2. timeout maps to timeout;
3. connection refusal/unreachable maps correctly;
4. HTTP non-success maps predictably;
5. invalid v2/v1 Gregg response maps to protocol/validation category;
6. fallback behavior still follows existing v2-first/v1-on-404 contract;
7. accepted success clears the previous failure reason;
8. stale generation failure cannot overwrite newer online state;
9. repeated offline polling continues normally;
10. renderer truncates/sanitizes detail without breaking established width invariants.

## Local verification

Mandatory:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
```

On the Ubuntu host run a real local smoke:

```text
1. launch greggd directly with a temporary loopback config;
2. wait for /v2/healthz to become a valid Gregg response;
3. run `greggd ... status` against the same config and verify it reports the endpoint as valid/running;
4. stop the daemon through the existing direct control path;
5. run status again and verify it reports stopped/unreachable without spawning anything;
6. confirm no systemd command is required for this direct lifecycle.
```

Also exercise `gregg` against one valid local endpoint and one deterministic unavailable endpoint to verify offline reason display and recovery.

Use existing native macOS/Windows CI for compatibility; no new matrix is required.

## Acceptance criteria

Plan 106 is complete only when:

1. `greggd status` exists and is strictly read-only.
2. Status composes existing config, version, bounded health probe, and startup-state logic rather than duplicating those mechanisms.
3. Status distinguishes a valid Gregg endpoint from absent/unreachable and non-Gregg/invalid occupancy.
4. Status never starts/stops/restarts/installs a daemon and never invokes internal `sudo`/authorization.
5. Client poll failures are normalized into stable application-level offline categories.
6. The TUI exposes the selected endpoint's failure cause without raw transport types or unbounded error text.
7. Successful recovery clears stale failure provenance and generation ordering remains correct.
8. Existing polling cadence, v2/v1 fallback, TUI geometry, EggPool behavior, and daemon API schemas remain unchanged.
9. Ubuntu direct-runtime smoke proves status works both while running and after stop without systemd coupling.
10. Full local verification and existing applicable native CI jobs pass.
