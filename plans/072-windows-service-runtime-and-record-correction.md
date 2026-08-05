# Phase 072: Windows service runtime and planning-record correction

Status: complete.

Depends on: Plans 066-071.

## Objective

Correct the Windows SCM service execution path so it owns exactly one Tokio runtime and never blocks that runtime's sole worker thread while waiting for an SCM control event. Then reconcile the existing Roadmap 066 closure records with the implementation that actually landed.

This is a narrow release-blocking correction. It does not redesign Windows service management, add a new runtime abstraction, expand CI, or reopen the completed drive, server-state, scheduler, EggPool, or footprint work.

## Why this phase exists

The ordinary foreground daemon path is working, but current Windows service mode has two incompatible runtime boundaries:

1. `crates/greggd/src/main.rs` uses `#[tokio::main(flavor = "current_thread")]`, so every command begins inside a Tokio runtime.
2. `service::windows::run_service()` is synchronous and creates a second current-thread runtime, then calls `Runtime::block_on()`.

Calling `block_on()` from code already executing inside another Tokio runtime can panic. The service shutdown future also performs blocking `std::sync::mpsc::Receiver::recv()` while being polled by the service's current-thread runtime. Once polled, that receive can occupy the only runtime thread and prevent the HTTP server and sampler tasks from progressing.

The same service function prints an error before returning it to `main`, where the binary prints the error again. This violates the intended one-diagnostic binary boundary from Plan 069.

The implementation records also contain direct factual inconsistencies:

- Plan 069 is marked complete while its acceptance checklist remains unchecked.
- Plan 071 is marked complete while final policy/documentation criteria remain unchecked.
- Plan 071 records an implementation SHA that does not match the actual footprint commit.
- Plan 071 says no Reqwest manifest change was required even though the `json` feature was removed from the planning baseline.

These are record corrections, not a reason to create another verification framework or evidence file.

## Scope

### In scope

- Make top-level `greggd` command dispatch synchronous before any Tokio runtime is created.
- Create a runtime only for commands that require async execution.
- Invoke Windows `service` mode outside an existing Tokio runtime.
- Replace blocking SCM shutdown reception with a nonblocking async signal.
- Preserve the current SCM control-handler behavior for Stop, Shutdown, Interrogate, and unsupported controls.
- Preserve the shared `run_with_shutdown()` daemon core.
- Remove duplicate service-path diagnostics.
- Add focused Windows-compilable tests for runtime ownership and shutdown signaling without installing a real service.
- Correct Plans 066, 069, and 071 plus `plans/README.md` to describe actual implementation and verification truth.
- Run the existing focused checks, default local check, and one ordinary CI run.

### Out of scope

- Rewriting the SCM adapter or service lifecycle state machine.
- Changing the service name, installation format, account, permissions, recovery policy, or packaging.
- Replacing Tokio, `windows-service`, Axum, the sampler, or service managers.
- Adding a second daemon core or a Windows-specific server implementation.
- Installing or starting a real Windows service in CI.
- Adding VM orchestration, privileged runners, workflow artifacts, evidence bundles, or a dedicated qualification workflow.
- Adding a permanent release-build matrix or binary-size gate.
- Reopening Plans 067, 068, or 070 unless implementation reveals a direct regression caused by this phase.
- Re-measuring binary size unless the runtime correction changes release profile or dependencies, which it should not.
- Creating Plan 073 merely to record closure.

## Product invariants

The implementation must preserve all of the following:

1. `greggd run` remains a foreground async daemon on Linux, macOS, and Windows.
2. `greggd service` remains hidden and Windows-only.
3. Linux/macOS service-manager commands remain synchronous.
4. Windows SCM Stop and Shutdown controls request graceful daemon shutdown.
5. The sampler and HTTP server continue sharing `run_with_shutdown()`.
6. The daemon still uses a current-thread runtime unless measured behavior requires otherwise; do not switch to a multithread runtime as a workaround.
7. Errors are printed once at the executable boundary.
8. Exit-code values from Plan 069 remain unchanged.
9. CI remains one read-only, nonpublishing workflow.
10. Release publication remains manual.

## Expected files

Primary implementation files:

```text
crates/greggd/src/main.rs
crates/greggd/src/service/windows.rs
crates/greggd/src/run.rs                 # only if a tiny generic bound/test seam is required
```

Focused documentation and record files:

```text
README.md                                # only if Windows runtime wording is currently inaccurate
architecture/greggd-daemon.md            # directly affected runtime ownership
architecture/error-conventions.md        # only if one-diagnostic wording needs correction
plans/066-bounded-correctness-and-maintainability-roadmap.md
plans/069-daemon-cli-runtime-and-test-correctness.md
plans/071-measured-footprint-and-lightweight-closure.md
plans/072-windows-service-runtime-and-record-correction.md
plans/README.md
```

Do not edit unrelated architecture documents, skills, packaging scripts, or workflow files unless a focused test cannot compile without a directly justified change.

## Implementation sequence for GPT-5.6 Luna

### Step 1: establish command-mode runtime ownership

Inspect `crates/greggd/src/main.rs` and remove the unconditional `#[tokio::main]` boundary.

Prefer a direct synchronous structure:

```rust
fn main() {
    init_logging();
    let code = match run_main() {
        Ok(()) => ExitCode::Success,
        Err(error) => {
            eprintln!("error: {error}");
            classify_error(error.as_ref())
        }
    };
    std::process::exit(code as i32);
}

fn run_main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run => run_foreground(...),
        Command::Service => run_windows_service(),
        command => dispatch_sync(command, ...),
    }
}
```

For `Command::Run`, construct one current-thread Tokio runtime and use it to execute `greggd::run::run(...)`.

For Windows `Command::Service`, call `service::windows::run_service()` directly from synchronous code. `run_service()` may construct the one runtime used for the daemon service core.

For non-Windows `Command::Service`, retain the current unsupported-command error without constructing a runtime.

Do not add an executor trait, runtime factory framework, global runtime, or command context object.

### Step 2: keep runtime construction small and fallible

Use one small helper if it improves testing, for example:

```rust
fn build_runtime() -> Result<tokio::runtime::Runtime, std::io::Error>
```

or:

```rust
fn block_on_current_thread<F: Future>(future: F) -> Result<F::Output, std::io::Error>
```

Keep ownership obvious:

- foreground `run` creates one runtime at the binary boundary;
- Windows `service` creates one runtime inside the service entry path;
- no path creates a runtime while another runtime is entered.

Do not silently fall back to another runtime flavor.

### Step 3: replace blocking SCM shutdown reception

Replace the `std::sync::mpsc` shutdown channel used by the control handler with a signal whose receiver can be awaited without blocking a runtime thread.

Preferred minimal design:

- create a `tokio::sync::oneshot::channel::<&'static str>()` before registering the SCM handler;
- hold the sender in `Arc<Mutex<Option<oneshot::Sender<&'static str>>>>` or an equivalently small one-shot container because the control handler is `FnMut`/`Fn`-like and SCM may deliver more than one control;
- on Stop, send `"SCM_STOP"` once;
- on Shutdown, send `"SCM_SHUTDOWN"` once;
- ignore later duplicate stop/shutdown sends after the sender is consumed;
- let Interrogate return success without sending shutdown;
- await the receiver in the shutdown future passed to `run_with_shutdown()`;
- map sender loss to one stable fallback reason such as `"SCM_CHANNEL_CLOSED"`.

The handler callback itself must remain nonblocking. Do not call `recv()`, `block_on()`, sleep, or wait for daemon termination inside the SCM callback.

A dedicated receiver thread plus async oneshot forwarding is acceptable only if `windows-service` callback constraints make the direct oneshot sender unusable. Prefer the direct sender because `oneshot::Sender::send` is synchronous and nonblocking.

### Step 4: preserve service status behavior

Keep the current status sequence unless a direct correctness issue must be fixed to make the runtime change work:

```text
START_PENDING
RUNNING
STOPPED
```

Do not expand this phase into checkpoint management, pause/continue support, service recovery configuration, or delayed RUNNING publication architecture.

Ensure every return path after SCM registration reports STOPPED where the current code already does so.

### Step 5: remove duplicate diagnostics

`run_service()` must return its error without printing it when `main` will report it.

Requirements:

- no `eprintln!("service exited with error...")` inside the reusable service runtime path;
- SCM status still receives a nonzero service exit code on failure;
- the executable emits one human-readable diagnostic;
- cleanup-only diagnostics already emitted by `run_with_shutdown()` are not broadened in this phase.

Do not build a logging or diagnostic framework.

### Step 6: add focused regression coverage

Add tests at the smallest stable seams.

Required coverage:

1. **Command/runtime separation**
   - service mode is selected before a foreground Tokio runtime is entered;
   - foreground mode constructs and uses one runtime;
   - synchronous lifecycle/config commands do not construct a runtime.

   This may be tested through a small pure command-classification helper or injected closures. Do not expose private internals publicly merely for tests.

2. **Nonblocking shutdown signal**
   - Stop sends `SCM_STOP` and completes the async receiver;
   - Shutdown sends `SCM_SHUTDOWN` and completes the async receiver;
   - Interrogate does not complete shutdown;
   - duplicate Stop/Shutdown controls do not panic or block;
   - dropped sender/handler produces the stable channel-closed reason if that branch is retained.

3. **Runtime helper**
   - a plain synchronous test can create the current-thread runtime and complete an immediately-ready future;
   - no production test intentionally enters one runtime and starts another merely to assert Tokio's panic behavior.

4. **Diagnostic boundary**
   - errors remain classifiable by the existing exit-code tests;
   - no second service-path print is present.

Tests must not install a service, require Administrator privileges, wait for the 30-second service transition timeout, or bind a fixed network port.

### Step 7: run focused verification

Run locally where supported:

```bash
cargo fmt --all -- --check
cargo test -p greggd --bin greggd
cargo test -p greggd service::windows
cargo test -p greggd run
cargo clippy -p greggd --all-targets --all-features -- -D warnings
./scripts/check-local.sh
```

On non-Windows hosts, Windows-only production branches may only compile in hosted CI. Keep platform-neutral signaling helpers under `cfg(any(test, target_os = "windows"))` when practical so deterministic unit tests run locally.

Push once the focused and default local checks pass, then require one ordinary existing CI run. The existing Windows job is sufficient when it compiles the production Windows branch and runs the focused unit tests.

Do not add a real-SCM CI job or a second workflow.

### Step 8: correct the planning records directly

After implementation and verification, update existing records in place.

#### Plan 069

- Check only acceptance criteria demonstrated by the landed implementation.
- Record the actual implementation commit for the Plan 069 work.
- Clarify that the unconditional async `main` left a Windows service runtime defect later corrected by Plan 072.
- Do not rewrite the original plan as though the defect was never present.

#### Plan 071

- Replace the incorrect implementation SHA with the actual footprint implementation commit:

```text
a53542b7bd5e74b68726191074500aa1ceb6a6d9
```

- State truthfully that the Reqwest `json` feature was removed from the planning baseline.
- Preserve the recorded baseline/final byte counts unless reinspection proves they are wrong.
- Check the final documentation, CI-policy, manual-release, and no-evidence-follow-up criteria only after direct inspection.
- Keep workflow run `31020619216` as the hosted closure for the footprint/source-equivalent state if it remains accurate.

#### Roadmap 066

- Record Plan 072 as the narrow correction for Windows service runtime ownership and closure truth.
- Mark Roadmap 066 fully complete only after Plan 072 implementation and ordinary CI pass.
- Do not reopen drive semantics, coherent server state, scheduler/EggPool decisions, or size measurement.

#### Plan index

- List Plan 072 as active while implementation is pending.
- After completion, move Roadmap 066 and Plans 067-072 to completed status.
- Keep the dependency summary concise:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072
```

Do not create an evidence file, closure manifest, Plan 073, or archived duplicate.

### Step 9: reconcile affected architecture text

Update only statements that describe runtime ownership or service shutdown signaling.

The final documentation should state:

- the executable dispatches command mode before creating a Tokio runtime;
- foreground mode and Windows SCM mode each own exactly one current-thread runtime;
- SCM control callbacks send a nonblocking one-shot shutdown signal;
- the shared daemon core remains `run_with_shutdown()`;
- errors are printed once by the executable boundary.

Avoid implementation-history prose in user-facing README sections unless necessary. Detailed rationale belongs in this plan or architecture notes.

## Acceptance criteria

### Windows service runtime

- [x] `greggd` no longer enters a Tokio runtime before deciding between foreground and SCM service modes.
- [x] `greggd run` owns exactly one current-thread runtime.
- [x] Windows `greggd service` owns exactly one current-thread runtime.
- [x] No production path calls `Runtime::block_on()` while already entered into another Tokio runtime.
- [x] The SCM control handler performs no blocking receive, sleep, runtime entry, or daemon join.
- [x] Stop and Shutdown complete the async service shutdown future with distinct stable reasons.
- [x] Interrogate and duplicate controls remain safe and nonblocking.
- [x] `run_with_shutdown()` remains the single shared daemon core.
- [x] Service errors are printed once and retain the existing exit-code classification.

### Verification

- [x] Focused command/runtime and shutdown-signal tests pass.
- [x] `cargo test -p greggd --bin greggd` passes.
- [x] `cargo test -p greggd service::windows` passes.
- [x] Focused Clippy passes with warnings denied.
- [x] `./scripts/check-local.sh` passes.
- [x] One ordinary existing CI run passes, including Windows production compilation/tests.
- [x] No new workflow, job class, artifact, evidence bundle, or privileged test environment is added.

### Planning-record truth

- [x] Plan 069's checklist and completion record reflect what landed and identify Plan 072 as the service-runtime correction.
- [x] Plan 071 records the correct footprint implementation SHA.
- [x] Plan 071 records the Reqwest `json` feature removal truthfully.
- [x] Plan 071's remaining policy/documentation criteria are checked only after inspection.
- [x] Roadmap 066 and `plans/README.md` include Plan 072 and accurately describe final status.
- [x] No Plan 073 or evidence-only closure document is created.

### Scope

- [x] No feature, protocol field, supported platform, service command, or user-visible monitoring behavior is removed.
- [x] No SCM lifecycle redesign, executor framework, runtime abstraction layer, or dependency is introduced.
- [x] Manual release and the single read-only CI workflow remain unchanged.

## Handoff format

Report:

```text
Implementation SHA:
Runtime ownership change:
SCM shutdown signal change:
Duplicate diagnostic removal:
Focused tests:
Default local check:
Ordinary CI run:
Plan 069 correction:
Plan 071 SHA/Reqwest correction:
Roadmap/index correction:
```

Keep the handoff concise. Do not create a separate evidence file.

## Completion

Implementation SHA: `bfc49f166f2962bfcb2723ceaf0807531000017d`
Runtime ownership change: synchronous command dispatch; one current-thread
runtime for foreground mode and one inside Windows SCM service mode.
SCM shutdown signal change: blocking `mpsc::recv()` replaced by a nonblocking
one-shot sender with stable Stop, Shutdown, and channel-closed reasons.
Duplicate diagnostic removal: service errors return to the executable, which
prints the single diagnostic and preserves exit-code classification.
Focused tests: `greggd` binary, `service::windows`, `run`, workspace tests,
focused/workspace Clippy, and formatting passed locally.
Default local check: passed (`./scripts/check-local.sh`).
Ordinary CI run: passed, workflow `31032208878` at `bfc49f1`, including Windows.
Plan 069 correction: acceptance checklist checked; the original unconditional
async-main service defect is recorded as corrected by Plan 072.
Plan 071 SHA/Reqwest correction: footprint SHA corrected to
`a53542b7bd5e74b68726191074500aa1ceb6a6d9`; the unused `json` feature removal
is recorded truthfully.
Roadmap/index correction: Plans 066-072 are complete in the roadmap and index;
no Plan 073 or evidence-only closure record was created.
