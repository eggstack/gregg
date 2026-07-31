# Phase 60: EggPool pane integration and lightweight closure

Status: completed; implementation `1406c2b`; ordinary CI `30660744394` passed.

## Objective

Integrate the optional EggPool configuration, summary worker, application state, event loop, and renderer into one coherent runtime path; reconcile active documentation; and close the roadmap using the repository's existing lightweight local and ordinary CI checks.

This phase is an integration/correction phase, not a feature-expansion phase.

It ends when:

- the TUI creates EggPool runtime resources only when configured;
- pane activation, period changes, manual refresh, periodic refresh, config reload, and shutdown are wired correctly;
- existing greggd polling and system navigation remain unaffected;
- public docs accurately describe the CLI, configuration, authentication, route requirement, controls, and metric semantics;
- focused tests, the existing local check, and ordinary cross-platform CI pass;
- no new workflow, evidence system, EggPool server change, or adjacent feature is introduced.

## Dependencies and execution position

Depends on completed implementation of:

- Phase 57 configuration/CLI;
- Phase 58 summary client/worker;
- Phase 59 pane state/controls/rendering.

This is the final phase of Roadmap 56.

## Governing invariants

1. Runtime integration remains inside the `gregg` client crate.
2. `greggd` polling remains on its existing scheduler and cadence.
3. EggPool uses its own small optional worker and never enters the greggd `PollBatch`/protocol path.
4. No EggPool config means no worker, channel, interval, request, or pane.
5. Only one EggPool request may be in flight.
6. Event-loop drawing/input remains responsive during EggPool requests.
7. `Ctrl-R` refreshes the active pane only.
8. Pane/period changes trigger at most the required request and do not spawn unbounded tasks.
9. Config reload safely starts, updates, or stops the optional EggPool worker.
10. Shutdown restores the terminal and cancels both polling paths promptly.
11. EggPool failure cannot terminate or stall greggd monitoring.
12. Existing local-first/manual-release policy remains unchanged.
13. No new product scope is accepted during closure.

## Scope

### In scope

- `main.rs`/event-loop wiring;
- optional worker/channel lifecycle;
- active-pane activation/deactivation commands;
- period-change and Ctrl-R dispatch;
- result application and redraw;
- config-reload lifecycle reconciliation if the current runtime supports reload events;
- shutdown/cancellation behavior;
- integration tests with synthetic local servers/channels;
- README, crate README, AGENTS, config example, help text, and plan-index updates;
- final local/ordinary CI closure.

### Out of scope

- changing EggPool's API or route registration;
- supporting dashboard-disabled EggPool instances;
- multiple EggPool endpoints;
- additional metrics or panes;
- retries/backoff/caching/history;
- alerts, charts, costs, provider/model/account detail;
- generic scheduler/screen/plugin abstractions;
- packaging/release/version publication work unless a separate release decision is made;
- new CI jobs, services, matrices, artifacts, evidence documents, screenshots, or release automation.

## Workstream A: construct optional runtime resources

In `run_tui`, derive runtime setup from loaded config:

```text
config.eggpool == None
    -> do not construct EggpoolClient/worker/channel

config.eggpool == Some(entry)
    -> construct one EggpoolClient using request timeout
    -> create one bounded command channel
    -> create one bounded result channel
    -> spawn one worker tied to the TUI cancellation token
```

Do not add EggPool fields to the greggd scheduler, endpoint list, semaphore, `PollBatch`, or protocol types.

The worker should own a clone of the validated `EggpoolEntry`. Resolved credentials remain request-local.

Channel capacities should be small and documented. Latest desired period/activation may be coalesced; do not queue a long history of keypresses.

### Workstream A acceptance criteria

- [ ] Unconfigured startup creates no EggPool runtime object.
- [ ] Configured startup creates exactly one worker.
- [ ] Existing greggd scheduler construction is mechanically unchanged.
- [ ] Channel/task cardinality is bounded.

## Workstream B: wire initial pane activation

After terminal/state initialization:

```text
active pane Systems
    -> EggPool worker remains inactive

active pane EggPool
    -> send one Activate(current period)
```

This ensures EggPool-only configurations immediately request the default 1-hour summary while mixed configurations do not query EggPool until the user enters that pane.

Initial activation must not block the first frame. The first frame should render the EggPool pending state and update when the result arrives.

Do not prefetch all four periods.

### Workstream B acceptance criteria

- [ ] EggPool-only startup draws before network completion.
- [ ] EggPool-only startup sends exactly one 1-hour request.
- [ ] Mixed startup sends no EggPool request until pane entry.
- [ ] No prefetch/cache warming is added.

## Workstream C: dispatch pane and period transitions

The event loop must compare state before/after relevant actions or receive a small action effect from the reducer. Use the simpler repository-consistent method; do not introduce a general command/effect architecture solely for this feature.

Required runtime effects:

### Pane switch Systems -> EggPool

```text
apply action
send Activate(current period)
```

If same-period success is already present and the last attempt is recent, Phase 58's worker may still follow the required first-entry/immediate semantics. Preferred simple rule: activation requests immediately unless an identical request is already in flight; do not add freshness policy beyond the 60-second cadence.

### Pane switch EggPool -> Systems

```text
apply action
send Deactivate
```

An already in-flight request may complete. Its result may update retained EggPool state but must not switch panes or trigger additional periodic work.

### Period change on EggPool

Only when the period actually changes:

```text
apply action
send SetPeriod(new period)
```

The renderer immediately shows the new period and pending state without old-period metrics.

### Clamped period movement

No command is sent.

### Workstream C acceptance criteria

- [ ] Pane entry/exit commands correspond exactly to real transitions.
- [ ] Period changes issue one latest-period request.
- [ ] Clamped movement issues no request.
- [ ] Repeated rapid movement cannot create an unbounded request queue.
- [ ] No pane transition changes system selection/layout/viewport state.

## Workstream D: make Ctrl-R active-pane specific

Current `Ctrl-R` triggers a greggd poll cycle. Update event-loop handling:

```text
active Systems
    -> existing greggd refresh channel

active EggPool
    -> EggPool Refresh(current period)
```

Requirements:

- systems-only behavior is unchanged;
- EggPool manual refresh does not trigger greggd polling;
- Systems manual refresh does not activate/query EggPool;
- if EggPool is refreshing, duplicate refresh may coalesce rather than creating a second in-flight request;
- key translation remains `Action::RefreshNow`; the event loop selects the active backend.

### Workstream D acceptance criteria

- [ ] Ctrl-R refreshes only the visible pane.
- [ ] Existing greggd refresh tests remain valid.
- [ ] No second request runs concurrently for EggPool.

## Workstream E: receive and apply EggPool results

Add the optional result receiver to `tokio::select!` without biasing away terminal input or greggd batches.

Result handling:

1. receive `EggpoolResult`;
2. apply through `AppState::apply_eggpool_result`;
3. ignore stale/cancelled/mismatched results according to Phase 59;
4. redraw through the normal loop;
5. never return an error for an EggPool fetch outcome.

If no EggPool receiver exists, avoid awkward permanently pending branches. Use an optional receiver pattern or a small disabled future consistent with current style; do not create dummy background traffic.

A closed EggPool result channel while the TUI remains active should produce a bounded internal unavailable state if needed, but must not terminate greggd monitoring. Prefer worker/channel closure only during global cancellation.

### Workstream E acceptance criteria

- [ ] Results update state/redraw without blocking.
- [ ] Fetch failures remain data, not event-loop errors.
- [ ] Stale results cannot overwrite the current period.
- [ ] Worker failure cannot stop system monitoring.

## Workstream F: reconcile periodic active-pane refresh

Verify end-to-end behavior of the Phase 58 worker:

```text
Systems active for several ticks
    -> zero periodic EggPool requests

EggPool active
    -> one request at activation
    -> subsequent requests no faster than 60 seconds

EggPool -> Systems before tick
    -> no request at later tick

period changes
    -> immediate new-period request
    -> periodic cadence continues for the new period
```

Use paused/injected time in tests. Do not add wall-clock waits.

Do not expose the 60-second value as a new config option in this roadmap. A fixed constant is sufficient and avoids another configuration surface.

### Workstream F acceptance criteria

- [ ] Passive cadence is correctly gated by active pane.
- [ ] Period change resets/continues cadence without duplicate bursts.
- [ ] Tests complete quickly under controlled time.
- [ ] No new refresh configuration is added.

## Workstream G: reconcile configuration reload lifecycle

The repository contains `ConfigReloaded` state behavior even if current file-watcher wiring is limited. Ensure any active reload path is truthful.

Required lifecycle semantics:

### Add EggPool at runtime

- update `AppState`;
- construct/start worker if runtime supports dynamic config reload;
- do not activate it when Systems remains active;
- activate immediately if no systems and EggPool becomes active.

### Remove EggPool at runtime

- deactivate/cancel worker;
- drop channels after orderly shutdown;
- state switches to Systems when available;
- no later old worker result reintroduces state.

### Change EggPool endpoint/auth reference

- stop or reconfigure the single worker through the simplest safe path;
- clear old endpoint summary;
- preserve selected period as defined in Phase 59;
- fetch immediately only if EggPool pane is active.

If the current executable does not actually receive config-change events, keep the reducer behavior correct and avoid inventing a file-watcher subsystem. Runtime hot reload is not required merely because an action variant exists.

### Workstream G acceptance criteria

- [ ] Implemented reload paths have correct worker lifecycle.
- [ ] No unimplemented watcher/reload subsystem is added.
- [ ] Old endpoint results cannot reappear after replacement/removal.
- [ ] System state remains preserved.

## Workstream H: shutdown and terminal restoration

Global cancellation/quit must:

- stop accepting new EggPool commands;
- cancel/finish the one in-flight request promptly through the shared cancellation path;
- stop the periodic interval;
- close worker channels;
- preserve existing event-stream shutdown;
- restore terminal state on ordinary quit/error as before.

Do not wait for a production HTTP timeout after quit if cancellation can abort the request. Add a bounded test around worker shutdown.

No separate signal handler is required; reuse the existing cancellation token.

### Workstream H acceptance criteria

- [ ] Quit/Ctrl-C cancels EggPool work promptly.
- [ ] Terminal restoration path remains unchanged and reliable.
- [ ] No task/channel leak is observable in deterministic tests.

## Workstream I: end-to-end synthetic integration tests

Use local synthetic servers and existing TestBackend/state helpers. Do not require a real EggPool process.

Minimum scenarios:

1. **No EggPool configured**
   - startup systems pane;
   - no EggPool requests;
   - h/l no-op when it is the only pane;
   - existing system polling/refresh works.

2. **EggPool-only public success**
   - initial pending frame;
   - request path/query exactly `/api/stats/summary?period=1h`;
   - success frame shows four metrics.

3. **Mixed config pane transition**
   - no initial EggPool request;
   - l/Right enters EggPool and requests 1h;
   - h/Left returns Systems and stops passive requests;
   - system selection/layout retained.

4. **Period movement**
   - j sends 24h, then 7d, then 30d;
   - k reverses;
   - edges issue no extra request;
   - old-period result arriving late is ignored.

5. **Protected success and auth failure**
   - Bearer header present with injected env;
   - 401 renders bounded authentication state;
   - secret absent from buffers/loggable outcome.

6. **Stats unavailable**
   - 404 renders route/dashboard guidance;
   - TUI remains responsive and Systems remains usable.

7. **Transient same-period failure**
   - success stored;
   - refresh timeout/error;
   - prior metrics retained with warning.

8. **Manual refresh separation**
   - Ctrl-R on Systems hits only greggd refresh channel;
   - Ctrl-R on EggPool hits only EggPool worker.

9. **Shutdown during request**
   - cancellation completes promptly;
   - event loop exits cleanly.

Tests may target event-loop helper seams rather than driving real crossterm input if that is more deterministic. Do not introduce a full terminal-process harness.

### Workstream I acceptance criteria

- [ ] All core interaction paths have one deterministic integration test.
- [ ] Tests use synthetic local I/O only.
- [ ] No live EggPool/provider credentials or service are required.
- [ ] No long sleeps or screenshot artifacts are added.

## Workstream J: update active public and contributor documentation

Update active docs only.

### Root README

Document:

- optional EggPool pane purpose;
- nested add/list/remove examples;
- one-endpoint limitation;
- default HTTP port `11300` and `--https`;
- optional `--api-key-env` and environment setup;
- requirement that EggPool dashboard/statistics routes be enabled;
- public versus protected behavior;
- four metrics and exact semantics;
- period controls and API window mapping;
- h/l pane navigation, j/k context, v system layout, e system drives;
- no pane when unconfigured.

### `crates/gregg/README.md`

Keep client-specific CLI/config/TUI behavior aligned with root README.

### `AGENTS.md`

Update source-of-truth and TUI/config rules:

- optional one-endpoint EggPool integration belongs only to `gregg`;
- resolved credentials must not persist;
- rendering stays I/O-free;
- h/l now means pane; v means normal/condensed system view;
- EggPool uses summary only and a 60-second active cadence;
- no general dashboard expansion.

### Config example

Ensure Phase 57's example is present and comments explain public versus protected setup without including a key value.

### CLI help

Review generated help for nested commands and replacement key documentation where applicable.

Do not rewrite completed historical plans 048–055. Add truthful status/index updates only after implementation evidence exists.

### Workstream J acceptance criteria

- [ ] All active docs agree on commands, keys, periods, auth, and semantics.
- [ ] Docs do not imply a request-level cache hit rate.
- [ ] Docs do not imply wall-clock/request throughput.
- [ ] Docs clearly state dashboard/statistics route requirement and one-endpoint scope.
- [ ] No secret examples contain real/synthetic key values beyond environment-variable placeholders.

## Workstream K: preserve lightweight verification and close truthfully

During implementation use focused commands. Final closure uses:

```text
./scripts/check-local.sh
```

and the existing ordinary cross-platform CI workflow.

On Windows, the existing PowerShell equivalent remains the native local command:

```text
.\scripts\check-local.ps1
```

No additional CI job is needed because:

- EggPool client logic is platform-neutral Rust/HTTP/state/rendering code;
- existing Linux/macOS/Windows jobs already compile/test the client;
- synthetic HTTP tests do not need EggPool installed;
- environment-variable-name validation is deterministic;
- no native platform API is added.

Closure record should be concise:

- implementation commit SHA;
- focused/local command results;
- one ordinary CI run at the implementation SHA or source-equivalent plan-only descendant;
- no separate evidence file.

Update `plans/README.md` statuses only when criteria are actually met. Until then plans remain `planned` or `in progress`.

### Workstream K acceptance criteria

- [ ] Focused tests pass.
- [ ] Existing local check passes.
- [ ] Ordinary CI passes without workflow changes.
- [ ] No retained evidence, qualification workflow, service container, or manual platform record is added.
- [ ] Plan/index status is truthful.

## Expected files

Likely integration/documentation surface:

```text
crates/gregg/src/main.rs
crates/gregg/src/action.rs
crates/gregg/src/state.rs
crates/gregg/src/eggpool.rs
crates/gregg/src/ui/eggpool.rs
crates/gregg/src/ui/mod.rs
crates/gregg/src integration tests
README.md
crates/gregg/README.md
crates/gregg/config.example.toml
AGENTS.md
plans/README.md
plans/056-eggpool-summary-pane-roadmap.md
plans/057-eggpool-config-and-cli.md
plans/058-eggpool-summary-client-and-refresh.md
plans/059-eggpool-pane-state-controls-and-rendering.md
plans/060-eggpool-pane-integration-and-lightweight-closure.md
```

Do not touch EggPool, `greggd`, `gregg-protocol`, workflows, release scripts, or historical archived plans.

## Implementation sequence

1. Wire optional worker construction without changing greggd scheduler setup.
2. Trigger initial activation for EggPool-only config.
3. Wire pane transitions and period changes to worker commands.
4. Make Ctrl-R active-pane specific.
5. Receive/apply results in the event loop.
6. Verify active-only 60-second cadence under controlled time.
7. Reconcile implemented config reload behavior without adding a watcher.
8. Verify shutdown/cancellation/terminal restoration.
9. Add end-to-end synthetic integration scenarios.
10. Update root/client README, AGENTS, help, and config example.
11. Run focused tests and full existing local check.
12. Confirm ordinary CI without workflow edits.
13. Update plan/index statuses only from actual results.
14. Inspect the final diff for scope creep and remove unrelated changes.

## Required verification

Focused development commands:

```text
cargo fmt --all -- --check
cargo test -p gregg eggpool --all-features
cargo test -p gregg state --all-features
cargo test -p gregg ui --all-features
cargo test -p gregg --all-targets --all-features
cargo clippy -p gregg --all-targets --all-features -- -D warnings
```

Final existing repository check:

```text
./scripts/check-local.sh
```

Use ordinary CI as hosted Linux/macOS/Windows confirmation. Do not add a dedicated EggPool workflow or service.

A bounded manual smoke against a LAN EggPool may be useful:

```text
gregg eggpool add HOST[:PORT] [--api-key-env ENV]
gregg
```

but it is optional and must not become a checked-in evidence requirement.

## Phase acceptance criteria

Phase 60 is complete only when:

- [ ] EggPool runtime resources are created only when configured.
- [ ] EggPool-only startup asynchronously fetches the default 1-hour summary.
- [ ] Mixed startup performs no EggPool request before pane entry.
- [ ] Pane entry/exit activates/deactivates passive EggPool refresh.
- [ ] Period changes issue exactly the corresponding latest-period request.
- [ ] Clamped period movement issues no request.
- [ ] Ctrl-R refreshes only the active pane.
- [ ] EggPool results update state/redraw without blocking or terminating the event loop.
- [ ] Fetch failures never disrupt greggd polling/system interaction.
- [ ] Passive refresh is no faster than 60 seconds and occurs only while EggPool is active.
- [ ] Config replacement/removal cannot allow stale old-endpoint results to reappear on implemented reload paths.
- [ ] Shutdown cancels EggPool work promptly and preserves terminal restoration.
- [ ] No-config, EggPool-only, mixed, auth, 404, period, stale-result, transient-failure, manual-refresh, and shutdown integration scenarios pass deterministically.
- [ ] Existing system selection, viewport, normal/condensed rendering, drive expansion, and greggd polling remain correct.
- [ ] Root/client README, AGENTS, config example, CLI help, and plan index are accurate.
- [ ] Existing focused and full local checks pass.
- [ ] Ordinary cross-platform CI passes without workflow changes.
- [ ] No EggPool server change, extra endpoint/metric/pane, multiple-instance support, generalized abstraction, new dependency, new CI/evidence/release machinery, or adjacent refactor was added.

## Roadmap closure criteria

Roadmap 56 may be marked completed only when:

- [ ] Phases 57, 58, 59, and 60 are complete.
- [ ] All Roadmap 56 program acceptance criteria are satisfied.
- [ ] The final plan index records actual implementation and ordinary CI results.
- [ ] No unresolved correctness issue remains within the explicit optional EggPool summary-pane scope.

Do not keep the roadmap open for future multi-EggPool, drill-down, dashboard-disabled route registration, alerts, charts, or release work. Those are separate decisions.

## Handoff guidance for a smaller implementation model

- Treat this as wiring and regression closure, not a redesign.
- Keep greggd and EggPool work paths separate in the event loop.
- Compare state before/after actions rather than inventing a general effect system.
- Use one optional worker and small channels.
- Make Ctrl-R branch on active pane.
- Use synthetic servers and controlled time; do not install EggPool in CI.
- Update statuses only after commands/ordinary CI genuinely pass.
- Stop if the diff reaches workflows, protocol/daemon code, EggPool, multiple pages, or generalized dashboard infrastructure.

## Closure record

- Implementation commit: `1406c2b` (`Integrate EggPool pane runtime`).
- Local verification: `./scripts/check-local.sh` passed on Linux, including workspace formatting, clippy with `-D warnings`, all workspace tests, workspace docs, and native Linux collector tests.
- Focused integration coverage: active-pane command routing, clamped period behavior, request generation/stale-result handling, synthetic summary HTTP responses, authentication outcomes, bounded failures, rendering, and worker cancellation behavior are covered by the `gregg` test suite.
- Configuration reload: the reducer preserves systems and rejects old EggPool endpoint results; the executable has no file-watcher path, so no live reload subsystem was added.
- Ordinary cross-platform CI: run `30660744394` passed Linux, macOS arm64, macOS Intel, Windows, and Rust 1.75 MSRV jobs.
