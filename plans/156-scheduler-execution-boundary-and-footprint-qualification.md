# Plan 156: Scheduler execution-boundary and footprint qualification

Status: planned.

Depends on: Plan 155 and current main. Independent of the remaining Plan 091 soak record.

## Objective

Resolve the small number of architectural decisions that can make a load-aware scheduler either safe and lean or accidentally turn greggd into a privileged, dependency-heavy process runner.

This is a qualification plan. It should produce an implementation-ready decision record and, where useful, small reversible measurement branches/patches. It must not land partially wired scheduled-command behavior in production.

## Current constraints to preserve

The qualification starts from these current Gregg facts:

- stripped release greggd is intentionally small and has repeatedly rejected large dependency growth;
- the runtime is Tokio current-thread with sync/time/net/fs/signal support but no process feature today;
- systemd uses User=greggd, Group=greggd, ProtectHome=true, ProtectSystem=strict, and a narrow ReadWritePaths set;
- macOS uses a system LaunchDaemon with no UserName override;
- Windows SCM uses NT AUTHORITY\LocalService;
- config is readable by unprivileged diagnostic commands and must remain usable by croncheck/status/configprint;
- the HTTP API is read-only;
- load telemetry already exists in the sampler on supported Unix hosts.

Do not weaken any of those constraints as a shortcut.

## Decision A: execution authority

Lock the first-release rule:

Scheduled commands execute only as the already-running greggd OS principal.

No:

- sudo;
- su;
- setuid/setgid transition;
- Windows token impersonation;
- password/key storage;
- run_as_user field;
- service-manager rewrite.

This means the canonical Linux system service generally cannot run maintenance over protected developer home directories. That is expected. A developer maintenance scheduler should use a user-owned/rootless greggd process or commands/paths accessible to the greggd account.

### Privileged Unix guard

Because the canonical macOS LaunchDaemon is privileged and a foreground greggd may also be started by root, configured jobs must not silently convert the monitoring daemon into a root command scheduler.

Qualify and adopt this guard unless a stronger equally small mechanism is demonstrated:

~~~toml
allow_privileged_jobs = false
~~~

Rules:

- if no jobs are configured, the field has no runtime effect;
- on Unix, when effective UID is 0 and one or more jobs exist, greggd refuses scheduler startup unless allow_privileged_jobs = true;
- the error names the risk and the exact config field; no interactive prompt;
- enabling the flag does not relax sandboxing or change user identity;
- Linux systemd's normal greggd user does not require the flag;
- macOS system LaunchDaemon jobs require the explicit opt-in;
- Windows keeps same-principal semantics without adding an administrator-token detector in this line.

Use the existing Unix libc dependency/euid boundary; do not add a privilege-detection crate.

## Decision B: command representation

First release command representation is argv-only:

~~~toml
command = ["/usr/local/bin/cargo-cleanme", "scan", "--deep"]
~~~

Requirements:

- non-empty executable element;
- bounded argument count and bounded string sizes;
- reject NUL/control data that cannot be passed safely;
- optional working_dir;
- no environment map in v1;
- no command-string parser in Gregg;
- no implicit /bin/sh -c or cmd.exe /C.

Operators who need shell features may explicitly select a shell as argv. That keeps quoting semantics outside Gregg and makes process ownership/test fixtures deterministic.

The daemon config remains non-secret. Document that argv values are visible anywhere the config is readable and must not contain credentials/tokens.

## Decision C: cron parser and civil-time backend

The public contract is five-field cron in local civil time, not a six/seven-field scheduler language.

Research on 2026-10-03 found these current candidates:

- cron-parser 0.12.x: specifically targets five-field expressions and depends on chrono;
- croner 4.0.x: richer POSIX/Vixie-style parser with chrono or optional jiff backend, but brings a broader feature/dependency surface;
- cron 0.17.x: mature schedule explorer but uses a broader expression/dependency stack than this feature needs.

Do not select a crate from source-package size or popularity alone. Build actual Gregg candidates.

### Candidate 1: cron-parser + chrono

Prototype only enough to prove:

- five fields only;
- lists/ranges/wildcards/steps;
- correct DOM/DOW behavior;
- local civil-time next-occurrence calculation;
- DST gap/overlap behavior;
- @hourly/@daily/@weekly/@monthly support if available or a tiny Gregg alias normalization if not.

### Candidate 2: croner with the smallest viable backend

Prototype with default features disabled and only the required time backend if possible. Do not enable seconds/year extensions, serde, descriptions, or timezone databases Gregg does not need.

### Candidate 3: narrow Gregg-owned parser

Consider only if library candidates fail the footprint/MSRV/semantic gate.

An internal parser may use compact field bitsets, but it must not casually introduce new unsafe local-time conversion code on every platform. If reliable local civil-time/DST handling would require substantial platform FFI, prefer a measured library even if the field parser itself is trivial.

### Footprint gate

Measure from the same clean release baseline:

~~~text
cargo build -p greggd --release
strip using the repository's normal release measurement method
record binary bytes
record cargo tree -p greggd feature/dependency delta
~~~

Then measure each viable candidate.

Adoption target: no more than 5% stripped greggd growth and no more than 128 KiB absolute growth from the baseline. If a candidate crosses either bound, record it as rejected unless it demonstrably removes enough existing code/dependency surface to offset the growth.

This mirrors Gregg's established footprint discipline; do not waive the gate merely because scheduler functionality is optional at runtime.

Record MSRV Rust 1.89 compatibility explicitly.

## Decision D: process supervision mechanism

Compare two small implementation shapes.

### Option 1: Tokio process feature

Enable only the Tokio process feature needed for async Child wait/kill and measure the stripped delta/feature graph.

Advantages:

- natural child completion future;
- clean select with scheduler deadlines and shutdown;
- straightforward bounded termination.

### Option 2: std::process with one bounded reaper

Because Plan 155 fixes global scheduler concurrency at one, a std::process::Command child plus one dedicated blocking wait/reaper is sufficient if it keeps the daemon responsive.

Requirements if selected:

- never wait synchronously on the current-thread Tokio executor;
- at most one reaper thread/blocking task because only one scheduled child may run;
- scheduler receives completion through a bounded one-shot/channel;
- no per-retry/per-job thread creation while deferred.

Do not add a process-management crate.

Choose the smaller design that preserves deterministic shutdown and testability. Record the stripped delta separately from the cron/time candidate.

## Decision E: child stdio and output bounds

First-release default should avoid turning maintenance output into an unbounded greggd log stream.

Qualify this policy:

- stdin = null;
- stdout = null;
- stderr = null;
- greggd itself logs start, completion status, elapsed duration, and state transitions.

Do not capture complete output in memory. Do not add log files per job.

If implementation evidence shows that discarding stderr makes failures unreasonably opaque, the allowed alternative is a small fixed-size tail capture with a hard byte cap and truncation marker. Any tail-capture proposal must be measured and specified before Plan 157; inherited unbounded output is not the fallback.

## Decision F: daemon shutdown while a job is running

Lock a deterministic first-release lifecycle.

Preferred contract:

- shutdown stops new launches immediately;
- scheduler requests termination of the direct child it owns;
- wait for child exit under a small bound inside the existing daemon shutdown budget;
- if the direct child does not exit, proceed with daemon shutdown and log the outcome;
- do not promise recursive process-tree termination for an operator-supplied shell/pipeline in v1.

If the selected process API can cheaply provide kill-on-drop/kill-and-wait semantics, use them. Do not change systemd KillMode, launchd policy, or SCM service policy to manage scheduled children.

Document that direct argv executables are the supported predictable lifecycle path; explicit shell wrappers may create descendant-process behavior Gregg does not own.

## Decision G: config bounds

Lock concrete upper bounds before implementation. Recommended:

~~~text
MAX_JOBS                    64
MAX_JOB_NAME_CHARS          96
MAX_SCHEDULE_BYTES          128
MAX_COMMAND_ARGS            64
MAX_COMMAND_ARG_BYTES       4096
MIN_RETRY_INTERVAL_MS       10000
MAX_RETRY_INTERVAL_MS       3600000
DEFAULT_RETRY_INTERVAL_MS   300000
DEFAULT_MAX_WAIT_MS         86400000
MAX_MAX_WAIT_MS             604800000
~~~

max_load must be finite and non-negative. load_window is one of 1m, 5m, 15m. retry_interval_ms must not exceed max_wait_ms for load-gated jobs.

These are configuration-validation bounds, not wire protocol.

## Required semantic fixtures

Before closing Plan 156, create isolated tests/prototypes proving the selected schedule backend can express the contract:

- every minute;
- Sunday weekly schedule;
- numeric and named weekday if names are supported;
- month boundary;
- leap-day next occurrence;
- DOM-only;
- DOW-only;
- both DOM and DOW restricted using the selected traditional-cron rule;
- DST spring-forward skipped local time;
- DST fall-back repeated local time with no accidental duplicate beyond the documented cron semantics;
- next occurrence strictly after a supplied instant.

The final Plan-157 contract must state the observed DST behavior rather than assuming it.

## Expected repository surface

Likely:

~~~text
plans/155-load-aware-maintenance-scheduler-roadmap.md
plans/156-scheduler-execution-boundary-and-footprint-qualification.md
plans/157-load-aware-maintenance-scheduler-implementation.md
plans/README.md

temporary/reversible Cargo.toml and Cargo.lock changes for measurement
temporary scheduler/time prototype module/tests if needed
~~~

Permanent production source changes should be limited to reusable seams or tests only if they are clearly part of the selected Plan-157 design. Otherwise revert prototypes before closing Plan 156.

## Verification

For every candidate that reaches measurement:

~~~text
cargo check -p greggd --all-targets --all-features
cargo test -p greggd --all-targets --all-features
cargo +1.89 test -p greggd --all-targets --all-features
cargo tree -p greggd -e features
cargo build -p greggd --release
~~~

Use the repository's established stripped-binary measurement procedure and record exact before/after byte counts.

If temporary candidate patches are committed for reproducibility, keep them on a dedicated qualification branch or revert them before Plan-156 closure on main. Main should end Plan 156 with the decision record and only the dependencies/features selected for Plan 157 if adoption itself is intentionally part of the closure.

## Acceptance criteria

- [ ] Same-principal execution is the locked first-release authority model.
- [ ] No sudo/user switching/credential mechanism is introduced.
- [ ] Root/euid-0 Unix jobs require an explicit allow_privileged_jobs opt-in.
- [ ] Canonical Linux/macOS service sandboxing is unchanged.
- [ ] argv-only command semantics are locked; no implicit shell.
- [ ] Config remains non-secret; no per-job environment/secret map is added.
- [ ] Five-field local cron semantics are tested and documented.
- [ ] DST gap/overlap behavior is explicitly recorded.
- [ ] cron/time candidates are measured against actual stripped greggd.
- [ ] Selected cron/time approach stays within the 5% and 128 KiB footprint gates, or the plan stops for redesign.
- [ ] Rust 1.89 MSRV is demonstrated for the selected approach.
- [ ] Tokio-process versus bounded std-process supervision is measured/compared.
- [ ] The selected process mechanism never blocks the current-thread runtime while a child runs.
- [ ] stdin/stdout/stderr policy is bounded and explicit.
- [ ] In-flight shutdown behavior is explicit and testable.
- [ ] Config cardinality/string/duration bounds are fixed for Plan 157.
- [ ] No remote API, persistence, workflow engine, service-manager weakening, or product scheduler behavior lands accidentally.

## Stop conditions

Stop and revise Plan 155/157 rather than forcing implementation if:

- every viable local-time cron implementation exceeds the footprint gate materially;
- correct DST/local-time semantics require broad unsafe platform code;
- process supervision cannot be made bounded without a material runtime/dependency expansion;
- the privilege model would require weakening the canonical service sandbox;
- a requirement emerges for secrets, user impersonation, persistent replay, or remote control.

## Handoff

The output of this plan is a precise implementation choice, not a generalized scheduler framework. Record exact crate versions/features, binary deltas, dependency deltas, shutdown semantics, and cron/DST behavior in the closure section so Plan 157 can be executed mechanically.
