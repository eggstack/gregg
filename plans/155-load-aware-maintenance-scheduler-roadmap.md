# Plan 155: Load-aware maintenance scheduler roadmap

Status: planned.

Depends on: current post-Plan-154 main. Independent of the remaining Plan 091 soak record.

## Objective

Add an optional, bounded local maintenance scheduler to greggd so operator-defined commands can run on cron-like schedules while optionally waiting for the monitored host to become sufficiently idle.

The primary use case is heavyweight periodic maintenance such as a weekly deep cargo-cleanme scan. A scheduled occurrence may be due at a specific local time, but if the configured load threshold is exceeded, greggd should defer that occurrence, retry cheaply, expire it after a bounded wait, and prevent several deferred heavyweight jobs from all launching together when the host becomes idle.

This line must preserve Gregg's core character: small binary, low steady-state CPU/memory cost, local configuration, native telemetry, no generalized orchestration platform, and no new remotely writable control plane.

## Existing Gregg properties this line should reuse

Current greggd already provides the important inputs and runtime primitives:

- Linux and macOS collectors publish native 1/5/15-minute load averages on the existing sample cadence.
- FreeBSD gregg-host also supports load averages, although the full daemon remains outside the supported FreeBSD product boundary.
- Windows intentionally reports load average unsupported.
- The sampler already runs independently of HTTP request handling and publishes immutable cached state.
- Tokio sync/time support and tracing are already present.
- The daemon uses a current-thread runtime and bounds blocking native collection separately.
- The canonical Linux systemd service runs as greggd:greggd with ProtectHome=true and ProtectSystem=strict.
- The canonical macOS LaunchDaemon has no UserName override and therefore runs with the system LaunchDaemon identity.
- Windows SCM runs greggd as NT AUTHORITY\LocalService.
- The daemon HTTP API is read-only and must remain unrelated to command execution.

The scheduler should consume cached sampler state. A deferred job must not invoke uptime, sysctl, PowerShell, shell probes, or a second host-telemetry collector merely to inspect load.

## Product boundary

The scheduler is local-only configuration interpreted by the local greggd process. It does not add:

- an HTTP endpoint for creating, editing, starting, cancelling, or querying jobs;
- a remote command API;
- distributed scheduling;
- cross-host dependencies;
- DAGs/workflows;
- persistence/replay of missed jobs across daemon downtime;
- shell-script storage or secret management;
- automatic privilege escalation;
- sudo invocation;
- setuid/user switching;
- containers or sandbox runtimes;
- a database;
- a generalized queue.

The first release is intentionally a maintenance scheduler, not a replacement for systemd, launchd, cron, Task Scheduler, or a workflow engine.

## Governing scheduling semantics

Use a familiar five-field cron schedule for time selection while keeping the command and Gregg-specific controls as structured TOML.

Target shape:

~~~toml
allow_privileged_jobs = false

[[jobs]]
name = "cargo-cleanme-deep"
schedule = "0 3 * * 0"
command = ["/usr/local/bin/cargo-cleanme", "scan", "--deep"]
working_dir = "/home/user/projects"

max_load = 8.0
load_window = "15m"
retry_interval_ms = 300000
max_wait_ms = 86400000
~~~

The exact schema is owned by Plan 157 after Plan 156 qualifies the parser/process approach, but preserve these concepts.

Cron compatibility target:

- exactly five ordinary fields: minute, hour, day-of-month, month, day-of-week;
- normal lists, ranges, wildcard, and step syntax;
- local civil time;
- traditional cron day-of-month/day-of-week matching semantics;
- @hourly, @daily, @weekly, and @monthly aliases if the selected parser supports them without semantic ambiguity;
- no seconds field;
- no year field;
- no crontab username field;
- no environment-assignment syntax;
- no implicit shell command field;
- no @reboot in the first release because daemon startup/restart is not a scheduled civil-time occurrence.

The command is an argv array. greggd launches the executable directly. Users who deliberately require shell expansion or pipelines can explicitly configure an argv such as ["/bin/sh", "-c", "..."] on Unix or the appropriate shell on Windows. greggd itself should not silently add a shell.

## Load-gating semantics

Load gating is optional. A job without max_load is purely time-gated.

A load-gated job:

1. becomes pending when its cron occurrence is due;
2. reads the latest cached sampler load state;
3. launches only when the chosen 1m/5m/15m value is available, fresh/ready, finite, and <= max_load;
4. otherwise remains pending and becomes eligible for another check after retry_interval_ms;
5. expires when max_wait_ms elapses from its original pending occurrence;
6. logs expiry once and returns to the next ordinary cron occurrence.

Default load window: 15m.

Recommended first-release defaults when max_load is configured:

~~~text
load_window        = 15m
retry_interval_ms  = 300000     # 5 minutes
max_wait_ms        = 86400000   # 24 hours
~~~

Do not add exponential backoff or a retry-count state machine initially. The load check is a cached scalar read, so fixed sparse retries plus a wall-clock max-wait bound are simpler and cheaper. max_wait is the authoritative anti-pileup/staleness bound.

If current load is unavailable or sampler readiness is not Ready, a load-gated job fails closed and remains pending. It must never interpret missing load as an idle machine.

Windows may support time-only jobs, but load-gated configuration must be rejected clearly while Windows load averages remain unsupported.

## Concurrency and anti-herd policy

The first scheduler should execute at most one Gregg-scheduled command at a time globally.

This is intentionally stricter than ordinary cron and directly solves the deferred-job herd problem without another semaphore/configuration surface.

Consequences:

- there is one global execution slot;
- each configured job has at most one pending occurrence;
- later occurrences while the same job is pending are coalesced and do not extend the existing max-wait deadline;
- later occurrences while the job is running may coalesce to one pending occurrence, never an unbounded backlog;
- multiple load-gated jobs that become eligible together are launched one at a time;
- after one job exits, the next load-gated candidate must re-read the newest cached load before launch;
- command failure does not trigger load retry or immediate command retry; it is logged and the next cron occurrence is authoritative.

This gives O(number of configured jobs) scheduler state with no event backlog proportional to downtime or system load duration.

A future plan may add an explicit concurrency policy only if real workloads justify it. Do not pre-build that abstraction.

## State and persistence

No durable scheduler state in this line.

Daemon restart:

- drops pending deferrals;
- does not replay occurrences that were missed while greggd was stopped;
- computes the next schedule strictly after the startup reference instant;
- must not immediately replay the current minute merely because greggd restarted inside that minute.

This matches the deliberately basic cron-driver scope and avoids introducing a writable scheduler-state directory, crash reconciliation, migration format, or duplicate-running-job registry.

Plan 156 must settle and document in-flight child behavior during daemon shutdown before Plan 157 implements process supervision.

## Logging

Use existing tracing only.

Required lifecycle events:

- configuration accepted;
- occurrence becomes pending because load is above threshold or unavailable;
- pending occurrence becomes eligible and starts;
- command exits with success/failure status and elapsed duration;
- pending occurrence expires after max_wait;
- scheduler cannot execute because of privilege/platform validation;
- daemon shutdown while a scheduled command is active.

Repeated high-load retries must not emit an info/warn line every retry. Log state transitions, not polling noise. Debug-level repeated-load detail is acceptable if bounded.

Do not add a scheduler log database or retain unbounded stdout/stderr in memory.

## Security and execution identity

A scheduled command executes as the existing greggd process identity. The scheduler must not alter service-manager ownership or weaken service sandboxing merely to make a command reachable.

In particular:

- Linux system service jobs run as greggd and remain subject to ProtectHome/ProtectSystem restrictions.
- Developer-home maintenance should use a user-owned/rootless greggd invocation if the system service cannot see the relevant paths.
- macOS system LaunchDaemon execution is privileged and requires an explicit privileged-job opt-in defined by Plan 156 before configured jobs may run.
- Windows SCM jobs execute as LocalService and receive no Gregg-provided elevation.
- no sudo, su, setuid, credential storage, or run_as_user feature is included.

The current daemon config is intentionally readable for croncheck/status/configprint and historically contains no secrets. The first scheduler therefore must not add per-job secret/environment-value storage. Documentation must state that command arguments are not a secret store.

## Plan sequence

### Plan 156 — execution boundary, cron/process footprint qualification

Before product implementation:

- lock the same-principal privilege contract;
- define the privileged Unix opt-in;
- qualify five-field cron parser/time-library candidates against Gregg's stripped-binary budget and MSRV;
- qualify tokio process support versus a smaller bounded std-process/reaper design;
- lock local-time/DST semantics;
- lock daemon-shutdown behavior for an in-flight command;
- record the chosen dependencies/features or the decision to implement a narrow internal parser.

No scheduler product behavior should land before these decisions are recorded.

### Plan 157 — load-aware scheduler implementation and qualification

Implement:

- config schema/validation;
- cron occurrence calculation;
- sampler-to-scheduler cached load handoff;
- bounded pending state machine;
- single global execution slot;
- command spawn/reap/lifecycle logging;
- shutdown integration;
- deterministic tests with injected clocks/load state;
- documentation/skills/changelog updates;
- binary/dependency remeasurement;
- existing cross-platform CI qualification.

### Plan 158 — scheduler footprint and schedule-validation corrective pass

The first integrated Plan-157 implementation is functionally strong and all-platform CI is green, but its stripped release binary remains 44,184 bytes above the Plan-156 128 KiB cap. Source review also found that syntactically valid but calendar-impossible cron expressions can pass config validation and fail only when the scheduler initializes.

Plan 158 therefore owns the remaining closure work:

- attribute and remove at least 44,184 bytes of scheduler-linked/code surface without weakening local-time/DST/process semantics;
- eliminate avoidable per-decision allocation/cloning before considering a time-backend change;
- reject calendar-impossible schedules during pure config validation;
- remove the fabricated +366-day runtime schedule fallback;
- rerun scheduler smoke, release-size qualification, Rust 1.89, and the existing six-job CI matrix;
- reconcile and close Plan 157 and this roadmap only after the footprint and correctness gates are satisfied.

## Performance budget

Idle cost should be approximately zero beyond existing greggd sampling:

- no second telemetry sampler;
- no per-second scheduler ticker;
- sleep until the next cron occurrence, retry deadline, child completion, or shutdown;
- no heap growth with missed occurrences;
- no process spawning while load-gated work is deferred;
- no HTTP self-polling from inside greggd.

The scheduler should hold only parsed schedule/config state, one pending record per job, and at most one child execution record.

## Acceptance criteria for the line

The roadmap is complete only when Plans 156-158 demonstrate:

- five-field cron-like scheduling with deterministic semantics;
- optional 1m/5m/15m cached-load gating;
- default 15-minute load window suitable for the motivating weekly maintenance case;
- bounded deferral with configurable retry interval and max wait;
- at most one pending occurrence per job;
- one global scheduled-command execution slot;
- load recheck between deferred heavyweight jobs;
- fail-closed behavior when load telemetry is unavailable;
- no Windows load-gate fabrication;
- no remote execution/control API;
- no privilege escalation or service-sandbox weakening;
- no persistent scheduler queue;
- no unbounded child output capture;
- transition-level logging only;
- dependency/binary footprint within the qualified Plan-156 budget;
- no regression to current monitoring, HTTP, startup, update, uninstall, croncheck, or protocol contracts.

## Explicit non-goals

Do not include:

- remote job submission;
- web/TUI scheduler management;
- distributed locks;
- dependency graphs;
- retries after command exit failure;
- exponential load backoff;
- job history database;
- persistent missed-job replay;
- catch-up storms;
- per-user impersonation;
- secret injection;
- cgroups/resource quotas;
- container execution;
- shell parsing implemented by Gregg;
- arbitrary second-level scheduling;
- a new scheduler crate/repository unless the implementation proves broadly reusable after the feature is complete.

## Handoff

Plan 156 is complete and Plan 157's implementation has landed, but the line remains open through corrective Plan 158. Continue with Plan 158; do not weaken the systemd unit, add sudo, relax the footprint gate, or replace correct DST/local-time behavior with unsafe platform code. The critical remaining goals are closing the measured binary deficit and moving impossible-calendar rejection into configuration validation.
