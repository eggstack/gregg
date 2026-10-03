# Plan 157: Load-aware maintenance scheduler implementation

Status: planned; blocked on Plan 156 qualification.

Depends on: completed Plan 156. Independent of the remaining Plan 091 soak record except that implementation must not alter Plan-091 croncheck/control semantics.

## Objective

Implement the bounded local scheduler defined by Plans 155-156: cron-like scheduled command execution inside greggd with optional cached-load gating, bounded deferral, coalescing, and one global execution slot.

The implementation must remain cheap while idle and must not create a new remote mutation surface.

## Configuration contract

Use the Plan-156-selected parser/process dependencies and the following logical schema.

Target example:

~~~toml
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000

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

Time-only job:

~~~toml
[[jobs]]
name = "refresh-local-index"
schedule = "15 */6 * * *"
command = ["/usr/local/bin/refresh-index"]
~~~

### Validation

Add config validation without weakening existing fields.

Required:

- jobs default to an empty list for backward-compatible existing config files;
- allow_privileged_jobs defaults false;
- job names are non-empty, bounded, control-free, and unique;
- schedule is bounded and parses under the exact five-field contract;
- command argv is non-empty and bounded per Plan 156;
- working_dir is optional and syntactically representable without canonicalizing or requiring it to exist at config-parse time;
- max_load is optional, finite, and non-negative;
- load_window defaults to 15m when max_load exists and accepts only 1m/5m/15m;
- retry_interval_ms and max_wait_ms default only for load-gated jobs and obey Plan-156 bounds;
- retry interval <= max wait;
- retry/max-wait fields without max_load are rejected rather than silently ignored;
- Windows rejects max_load because Gregg does not provide Windows load averages;
- Unix euid 0 + nonempty jobs + allow_privileged_jobs=false fails closed with a specific diagnostic.

Do not add environment variables, shell strings, run-as identities, command-retry counts, or runtime timeouts in this plan.

host/port mutation commands must preserve the jobs array and privileged flag during atomic config rewrite exactly as they preserve existing config fields.

## Scheduler module boundary

Add a dedicated greggd scheduler module rather than embedding the state machine in run.rs or sampler.rs.

Recommended ownership:

~~~text
crates/greggd/src/scheduler/
  mod.rs          # engine/state transitions
  schedule.rs     # selected cron wrapper / next occurrence
  process.rs      # selected bounded child adapter
  config tests may remain in config.rs
~~~

Keep public surface narrow. This is daemon-internal functionality unless a small pure scheduling type is useful to tests.

Do not put scheduler types in gregg-protocol. There is no wire/API change.

## Cached load handoff

The scheduler must not sample load independently and must not call greggd's HTTP endpoint.

Add a tiny sampler-owned publication seam, preferably a Tokio watch channel already available under the existing sync feature.

Logical state:

~~~text
LoadGateState {
    readiness,
    load: Option<LoadAverage>,
    sample_generation or monotonic update marker
}
~~~

Requirements:

- successful sampler publication updates the latest load state;
- failed/warming sampler state makes load gating unavailable/fail-closed;
- scheduler reads/borrows the latest value only when evaluating a due/pending load-gated job;
- scheduler does not await every one-second sampler update and therefore is not woken once per sample merely because load changed;
- no additional collector read;
- no HTTP request;
- no serialization/deserialization;
- Windows naturally publishes load unavailable.

Use the v2-capability-aware load value or equivalent collector-normalized value; never fabricate zero.

## Scheduler state machine

Represent each configured job with bounded runtime state.

Logical fields:

~~~text
next_due
pending_since: Option<Instant/civil occurrence>
next_retry: Option<Instant>
running: bool
coalesced_due: bool or equivalent one-bit state
~~~

Global state owns at most one running child.

### Due occurrence

When next_due is reached:

- advance the schedule to its next future occurrence;
- if the job is idle and no global child is running, evaluate it for launch;
- if the global slot is occupied, mark exactly one pending occurrence;
- if the same job is already pending, keep the existing pending_since/deadline and only record that another occurrence was coalesced if useful for debug logging;
- never enqueue an occurrence object per missed schedule.

### Load-gated launch

Before every launch of a load-gated job:

- borrow newest LoadGateState;
- require Ready plus the configured load window value;
- compare value <= max_load;
- if allowed, launch;
- if high/unavailable, keep pending and set next_retry;
- if max_wait elapsed since pending_since, expire it once.

When a running command exits and pending load-gated jobs exist, re-read current load before selecting the next one. Do not infer that the host remains idle because it was idle before the previous heavyweight job launched.

### Time-only launch

A time-only job does not evaluate load. It still respects the one-global-child rule and per-job coalescing.

### Command exit

Record exit code/signal classification and elapsed duration.

Do not automatically retry a command because it exits nonzero. The next cron occurrence is authoritative.

After exit:

- clear running state;
- reconcile any due/pending jobs;
- perform load checks anew;
- launch at most one next job.

## Event-loop/wakeup design

No short fixed ticker.

The scheduler should sleep/select until the earliest of:

- next cron occurrence across all jobs;
- earliest pending load retry;
- earliest pending max-wait expiry;
- active child completion;
- daemon shutdown.

For <=64 jobs, a simple O(N) scan to compute the earliest deadline is preferred over a heap/timing-wheel dependency. At minute-scale schedules this cost is negligible and keeps mutation logic simple.

Injected scheduler clocks must allow tests to advance time without production sleeps.

## Coalescing and max-wait invariants

Lock these invariants with tests:

1. one job has at most one pending occurrence;
2. pending_since is the first deferred occurrence and is not extended by later matches;
3. repeated matches while pending do not create a queue;
4. max_wait expiry drops that pending occurrence and future scheduling resumes from the next occurrence after the current scheduler time;
5. repeated matches while running coalesce to at most one pending occurrence;
6. global state contains at most one active child;
7. five deferred jobs cannot all launch when load becomes acceptable;
8. after each child exits, the next load-gated launch rechecks cached load;
9. daemon downtime is not replayed;
10. startup computes a next occurrence strictly after the startup reference instant.

Memory remains O(configured jobs).

## Selection/fairness

When multiple jobs are pending, choose deterministically:

- oldest pending_since first;
- stable config order as the tie breaker.

Do not add priorities.

A time-only job waiting behind a heavy running command may run before a younger load-gated job if its pending occurrence is older. This is acceptable under the single-slot first-release policy.

## Process execution

Use the exact process adapter selected by Plan 156.

Requirements:

- direct argv execution;
- optional working_dir;
- stdin null;
- bounded stdout/stderr policy exactly as qualified;
- no inherited secrets injected by Gregg;
- spawn errors log once and count as that occurrence's terminal command result;
- child wait never blocks the current-thread runtime;
- scheduler observes shutdown while child is active;
- shutdown follows Plan-156 direct-child termination semantics within the daemon's existing bounded cleanup architecture;
- no recursive process-tree guarantee for explicitly configured shell wrappers.

Do not call external timeout/nohup/nice/ionice utilities to implement lifecycle.

## Daemon supervision integration

Scheduler is a third supervised daemon subsystem only when jobs are configured.

run.rs must preserve existing server/sampler supervision and shutdown behavior.

Required:

- no jobs => no scheduler child task/process machinery beyond config validation;
- configured scheduler unexpected panic/exit is treated as a daemon runtime failure, not silently ignored;
- shutdown notifies scheduler together with sampler/server;
- common join deadline remains bounded;
- scheduler cleanup cannot multiply the shutdown deadline;
- server and monitoring remain fully functional while a maintenance child runs.

Do not change croncheck. croncheck remains only the daemon watchdog and must never execute configured maintenance jobs itself.

Do not change startup-manager detection, update ownership, uninstall ownership, control-socket identity, or HTTP readiness semantics.

## Logging contract

Use tracing fields with job name but avoid logging the complete argv at info level.

Info-level transitions:

~~~text
scheduled job pending: load gate
scheduled job started
scheduled job completed
scheduled job expired waiting for load
~~~

Include relevant structured fields:

- job;
- scheduled/pending age;
- load window and observed load when applicable;
- max load;
- exit status;
- elapsed duration.

Repeated retry observations while the same job remains above load threshold should be debug-only or suppressed entirely until the state changes. Do not emit one info/warn every five minutes forever.

Never log secret redaction claims; argv is simply not a secret-bearing supported surface.

## Tests

Create deterministic unit tests around an injected clock, fake load feed, and fake process runner.

Minimum matrix:

### Schedule/config

- old config with no jobs unchanged;
- one valid time-only job;
- one valid load-gated job;
- duplicate names rejected;
- bad cron expression rejected;
- six-field/seconds expression rejected;
- empty command rejected;
- bounds from Plan 156 enforced;
- invalid load window rejected;
- NaN/infinite/negative load threshold rejected;
- retry > max_wait rejected;
- retry/max-wait without max_load rejected;
- Windows load gate rejected;
- Unix root jobs rejected without explicit opt-in via an injectable privilege seam.

### Load gating

- load 7.9 <= threshold 8 starts;
- load 8.0 starts;
- load >8 defers;
- unavailable load defers;
- sampler Failed/Warming defers;
- 1m/5m/15m window chooses the exact configured scalar;
- successful fresh load after retry starts;
- high load until max_wait expires logs/drops once;
- later ordinary schedule can become pending after earlier expiry.

### Anti-herd/coalescing

- five due load-gated jobs with low load start exactly one;
- second does not start until first exits;
- load rises after first starts; second remains deferred after first exits;
- repeated occurrences while pending produce one pending record;
- repeated occurrences while running produce at most one pending follow-up;
- queue memory cardinality never exceeds configured job count;
- stable oldest-due/config-order selection.

### Command lifecycle

- argv preserved exactly;
- working directory applied;
- spawn failure terminal for occurrence;
- success and nonzero exit both release global slot;
- command failure is not retried immediately;
- shutdown active-child behavior matches Plan 156;
- server/sampler task supervision remains live while fake child runs.

No production-duration sleeps.

## Documentation and skills

Update in the same implementation pass:

- README.md essential daemon capability summary;
- crates/greggd/README.md;
- docs/daemon.md with config examples and service-identity warning;
- architecture/greggd-daemon.md with scheduler ownership/state machine/load feed;
- AGENTS.md daemon constraints;
- matching greggd daemon/config skill files;
- CHANGELOG.md;
- plans/155-157 and plans/README.md closure/status records.

Documentation must state clearly:

- commands execute as greggd's current OS principal;
- Linux system-service sandbox may not see developer home directories;
- macOS privileged/system jobs require explicit allow_privileged_jobs;
- Windows has no load gating while load averages are unsupported;
- command argv/config is not a secret store;
- no missed-job replay after daemon downtime;
- one global scheduled command at a time;
- load deferral defaults and max-wait behavior;
- shell use is explicit argv, not implicit Gregg behavior.

## Footprint/performance verification

Remeasure final implementation, not only Plan-156 prototypes.

Required:

~~~text
cargo tree -p greggd -e features
cargo build -p greggd --release
repository standard stripping/measurement
~~~

Record:

- baseline bytes from Plan 156;
- final bytes;
- absolute/percent delta;
- final new dependency/features;
- idle scheduler allocation/task count if easily measurable.

The final implementation must remain within the Plan-156 accepted budget. If integration causes the total to cross the gate, stop and reduce the implementation rather than silently accepting the drift.

## Verification

Focused:

~~~text
cargo test -p greggd --all-targets --all-features -- scheduler
cargo test -p greggd --all-targets --all-features -- config
cargo test -p greggd --all-targets --all-features -- run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
./scripts/check-local.sh --release
~~~

Run the existing native CI matrix; do not add a scheduler-specific workflow.

Manual Unix smoke after unit coverage:

1. use a temporary explicit config with a harmless command writing one marker file;
2. schedule it for the next eligible minute;
3. demonstrate low-load execution exactly once;
4. demonstrate a threshold below current load defers it;
5. shorten max_wait in the smoke config and demonstrate expiry without execution;
6. configure two immediately eligible load-gated jobs and demonstrate only one child starts, then load is re-evaluated before the second;
7. stop greggd during an active harmless long-running test child and demonstrate the Plan-156 shutdown contract.

Use temporary paths; do not run cargo-cleanme or destructive maintenance as qualification.

## Acceptance criteria

- [ ] Existing configs deserialize with jobs empty/default and unchanged monitoring behavior.
- [ ] Five-field cron schedule contract is enforced.
- [ ] No seconds/year/username/@reboot extension leaks in.
- [ ] Commands use direct argv with optional working_dir.
- [ ] No implicit shell, environment secret map, sudo, or run-as feature.
- [ ] Unix root job execution requires explicit privileged opt-in.
- [ ] Scheduler consumes cached sampler load and performs no new host load probes.
- [ ] Scheduler does not self-poll the HTTP API.
- [ ] 1m/5m/15m gates use the correct cached scalar.
- [ ] Missing/not-ready load fails closed.
- [ ] Windows load-gated jobs are rejected without fabricated load.
- [ ] Default load window/retry/max-wait behavior matches Plan 155.
- [ ] One configured job has at most one pending occurrence.
- [ ] Total pending-state cardinality is O(number of jobs).
- [ ] Exactly one Gregg-scheduled command may run globally.
- [ ] Deferred jobs do not herd when host load drops.
- [ ] Load is rechecked between sequential heavyweight jobs.
- [ ] Command nonzero exit is logged but not immediately retried.
- [ ] Pending max-wait expiry is bounded and does not extend on later coalesced occurrences.
- [ ] Daemon restart does not replay missed work/current minute.
- [ ] Child lifecycle/shutdown follows Plan 156.
- [ ] Repeated high-load retry logging is suppressed/debug-only.
- [ ] HTTP/protocol/croncheck/control/startup/update/uninstall behavior is unchanged.
- [ ] Final stripped-binary/dependency delta remains inside Plan-156 budget.
- [ ] Default/release local checks pass.
- [ ] Existing six-job native CI passes.
- [ ] Manual harmless scheduler smoke passes.

## Stop conditions

Stop and write a corrective follow-up instead of broadening this plan if implementation requires:

- persistent state to avoid correctness bugs;
- multiple simultaneous children to satisfy core behavior;
- process-tree/job-object/cgroup orchestration beyond the qualified direct-child lifecycle;
- a remote management API;
- weakening systemd/launchd/SCM sandbox policy;
- user impersonation;
- secret storage;
- a binary-footprint gate waiver;
- protocol changes.

## Handoff

Implement the scheduler as a small third daemon subsystem fed by existing sampler state. The highest-risk regressions are privilege expansion, accidental cron semantic drift, unbounded occurrence queues, blocking the current-thread runtime, and child/herd behavior. Preserve the single-slot/coalescing model even if a scheduling library offers a richer job engine.
