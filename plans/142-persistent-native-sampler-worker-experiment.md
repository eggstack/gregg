# Plan 142: persistent native sampler worker experiment

Status: planned experiment; may close with RETAIN SPAWN_BLOCKING.

Depends on: Plan 138 and completed Plan 141 native acquisition work, so native source cost is settled before runtime-handoff comparison.

## Objective

Determine whether `greggd` should replace one `tokio::task::spawn_blocking` call plus `Arc<Mutex<C>>` handoff per sample with one dedicated native collector thread, without changing public `Sampler` behavior, readiness, cadence, panic recovery, shutdown semantics, or collector identity/capability behavior.

This is a benchmark- and compatibility-gated experiment, analogous to Plan 126's reversible transport experiment. A clean RETAIN SPAWN_BLOCKING result is acceptable.

## Current baseline

`greggd` uses a Tokio current-thread runtime.

Each `Sampler::run` cycle:

1. clones the collector Arc;
2. creates a new `spawn_blocking` task;
3. locks the collector;
4. samples;
5. awaits the JoinHandle;
6. later locks the collector again during conversion to read identity/capability/support information.

This correctly isolates synchronous native I/O from the async server.

Tokio documents that started `spawn_blocking` work cannot be aborted and that dedicated threads are preferable for persistent/long-lived blocking loops. That supports evaluating a worker, but does not by itself justify adoption.

## Candidate design

### Dedicated collector owner

A single named `std::thread` owns the collector for the runtime sampling session.

The async sampler sends one request only when no sample is outstanding. The worker returns one ordered result through a bounded or one-shot response channel.

No unbounded request queue is permitted.

### Panic containment

Wrap only collector work in `catch_unwind(AssertUnwindSafe(...))` or an equivalent contained boundary.

A panic must:

- produce the same logical SourceUnavailable failure category as today;
- retain the collector object for later samples where possible;
- not kill the worker loop;
- not poison an async-visible mutex.

Do not catch panics around arbitrary daemon code.

### Identity/capability ownership

Avoid a second cross-thread lock.

A worker response may include:

- sample result;
- current identity result;
- current v1/v2 capabilities;
- `supports_v1_snapshot`.

Preserve the current possibility that identity can be re-read rather than freezing it at worker construction.

Do not move protocol conversion into `gregg-host` or the worker.

### Public Sampler compatibility

`Sampler::new`, `with_interval`, `snapshot`, `snapshot_v2`, `readiness`, `health_response`, `sample_once`, and `run` remain source-compatible.

Because `sample_once` is synchronous, the implementation must explicitly define and test collector ownership before, during, and after `run`.

Do not silently make `sample_once` unavailable after a normal bounded run.

If preserving this cleanly requires disproportionate state machinery, prefer RETAIN SPAWN_BLOCKING.

### Shutdown and blocked native calls

Current `spawn_blocking` calls cannot be abruptly cancelled once started. The experiment must not claim stronger cancellation than actually exists.

A worker must use bounded shutdown signaling:

- channel closure/request stop when idle;
- no unbounded join in the daemon's outer shutdown deadline;
- if a native call is uninterruptibly blocked, do not make the HTTP/current-thread runtime wait forever solely to join the worker.

Document whether a blocked worker is detached at process shutdown and compare that behavior to the current runtime/blocking-pool semantics.

Do not introduce unsafe thread termination.

## Baseline and candidate tests

Required compatibility tests:

- warming -> ready;
- counter reset -> recover;
- hard failure readiness/count behavior;
- collector panic -> later recovery;
- identity failure/conversion failure;
- callback ordering;
- sample cadence and first immediate sample;
- shutdown between samples;
- shutdown while worker result is pending;
- public `sample_once` before and after a normal bounded run;
- no more than one in-flight native sample;
- blocked synthetic collector does not stall unrelated HTTP/current-thread async work.

Retain all existing sampler tests.

## Measurement

Use a synthetic near-zero-cost collector to expose handoff overhead separately from native I/O, plus one ordinary native collector descriptive run after Plan 141.

Record:

- task/thread creation structure;
- mutex/channel operations per sample;
- release-mode descriptive CPU/time results;
- stripped `greggd` size;
- shutdown observations.

Do not install timing gates in CI.

## Retain/reject gate

Adopt the worker only if all are true:

1. public and lifecycle semantics are no more complex or weaker than baseline;
2. panic recovery remains robust;
3. blocked-call shutdown does not regress;
4. deterministic structural work per sample is reduced;
5. measured benefit is meaningful enough to justify the additional worker/channel state;
6. binary growth is acceptable and explained.

Otherwise revert the candidate completely and close as RETAIN SPAWN_BLOCKING with the evidence recorded.

## Verification

~~~text
cargo test -p greggd --all-targets --all-features -- sampler
cargo test -p greggd --all-targets --all-features -- run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use existing native CI only. No new soak or performance workflow is required.

## Acceptance criteria

- [ ] Baseline and candidate are compared against the same compatibility suite.
- [ ] At most one collector sample is in flight.
- [ ] Panic recovery and later sampling match baseline behavior.
- [ ] Identity/capability refresh semantics remain unchanged.
- [ ] Public Sampler methods remain source-compatible.
- [ ] Shutdown/blocking behavior is explicitly demonstrated, not inferred.
- [ ] Structural and descriptive performance evidence is recorded.
- [ ] Candidate is retained only if the retain/reject gate passes.
- [ ] A RETAIN SPAWN_BLOCKING closure is considered successful if the candidate fails the gate.
- [ ] No new runtime/dependency framework is introduced.

## Explicit non-goals

Do not include:

- changing sample interval semantics;
- multiple collector workers;
- parallel metric-family collection;
- unsafe thread cancellation;
- moving collection into async filesystem APIs;
- changing DriveRefreshCache policy;
- protocol/server/TUI changes;
- background benchmark infrastructure.

## Handoff note

Build the experiment so it can be reverted as one bounded diff. Do not entangle the worker candidate with Plan-141 source changes or unrelated sampler cleanup.
