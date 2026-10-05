# Plan 170: Gregg client-daemon footprint and source-boundary optimization

Status: planned.

Depends on: completed Plan 169 so size measurements begin from one clean
cross-platform verification baseline. Independent of Plan 091.

## Objective

Investigate and, only where measurement justifies it, reduce the client-side
binary/code footprint added by the Plan-161 client-daemon/cron line without
changing its process architecture, polling semantics, lifecycle ownership, or
user-visible capability.

This plan is deliberately reversible. It may close with RETAIN CURRENT if the
measured size is the irreducible cost of one binary containing both the TUI and
the background client daemon.

## Current measured baseline

Plan 167 measured the pre-client-daemon Gregg baseline at:

~~~text
4,349,736 bytes stripped
~~~

and the Plan-167 closure at:

~~~text
5,339,488 bytes stripped
+989,752 bytes / +22.76%
~~~

Plan 168 then moved the current stripped Gregg binary to:

~~~text
5,345,864 bytes
+996,128 bytes / +22.90% vs the pre-line baseline
~~~

while leaving `greggd` at 3,316,568 bytes.

The key observation is that Plans 164-166 added no new Gregg dependency graph:
their closure recorded a zero `Cargo.toml` / `Cargo.lock` dependency delta.
The large client increase is therefore primarily application code and
monomorphized/runtime surface, not a newly imported framework.

The implementation comparison from pre-line `ae56926` to the current line
shows most source growth in:

- `state.rs`;
- `clientd/daemon.rs`;
- `clientd/startup.rs`;
- `ui/cron.rs`;
- `clientd/cron.rs`;
- `cron.rs`;
- `clientd/ipc.rs`;
- `clientd/launch.rs`;
- update/uninstall/lifecycle support.

Source-line count is not binary attribution. Do not optimize those files simply
because they are large.

## Product boundaries that are not negotiable

Do not reduce size by undoing the architecture that motivated the line.

Retain:

- one existing `gregg` executable serving both TUI and client-daemon modes;
- one config-specific background polling plane shared by all frontends;
- no direct-polling fallback;
- Unix socket / Windows named-pipe local IPC;
- same-user IPC security;
- client-daemon persistence after the last TUI exits;
- lazy activation and user-scoped startup;
- update/uninstall lifecycle ownership;
- scheduler summary/history polling;
- memory-only bounded cron cache;
- plain `c` cron UI;
- hostile-output sanitization;
- EggPool semantics;
- Rust 1.89 MSRV.

Do not create a separately distributed client-daemon binary to win bytes.
Do not remove Windows/macOS lifecycle support from the release binary.
Do not add compression, protobuf, async frameworks, or other dependencies in
the name of reducing code.

## Phase 1: attribute the binary growth

Build the current `gregg` with the same release profile used by Plans
167-168:

~~~text
lto = "fat"
codegen-units = 1
strip = "symbols"
panic = "abort"
~~~

Keep the stripped 5,345,864-byte figure as the current comparison point unless
Plan 169 changes it; if 169 does, record the new post-169 baseline first.

For attribution, use temporary developer tooling only. Acceptable methods
include:

- an unstripped paired build plus `nm` / platform symbol-size tooling;
- `cargo bloat` installed locally, not added to the repository;
- compiler/linker map output;
- controlled feature/source ablations on a throwaway branch.

Do not add a release dependency or permanent CI job for the profiler.

Record at least:

- top functions/modules by text contribution where attribution is reliable;
- how much is Tokio/runtime glue versus application functions;
- platform-lifecycle contribution;
- local IPC/serde DTO contribution;
- cron cache/renderer/sanitizer contribution;
- update/uninstall contribution;
- any duplicated codegen that can be tied to concrete generic instantiations.

The plan must distinguish linked production code from test-only source.

## Candidate A: platform-specific lifecycle isolation

Research the current `clientd/startup.rs` boundary first.

The file contains render/parse/ownership code for systemd-user, crontab,
LaunchAgent, and Windows Startup-folder artifacts in one module. Determine which
non-native paths remain linked into each release binary and why.

If measurement shows material cross-platform code retention:

- split platform-specific implementations behind explicit `cfg` modules;
- keep shared ownership types/contracts in a small common module;
- compile only the platform's manager renderer/parser/executor in production;
- keep deterministic cross-platform fixture/parser tests using test-only helper
  modules or pure fixtures where appropriate;
- preserve the CLI's current error behavior for unsupported manager methods.

Do not make startup artifact ownership less strict to save code.

## Candidate B: lifecycle/update/uninstall duplication

Inspect:

- `clientd/launch.rs`;
- `clientd/startup.rs`;
- `startup_support.rs`;
- `update.rs`;
- `uninstall.rs`.

Look for repeated:

- daemon probe/status mapping;
- exact-config identity handling;
- bounded wait/readiness loops;
- lifecycle diagnostic formatting;
- owned/foreign/unknown branching.

Retain a consolidation only if it both:

1. removes a real duplicated runtime path rather than merely moving lines; and
2. is byte-reducing or clearly simpler with no size regression.

Do not build a generic service-manager framework.

## Candidate C: local protocol/DTO serialization

The current fan-out architecture already serializes one document once for all
frontends, which is the correct performance shape.

Measure whether binary growth is materially attributable to duplicated serde
models or conversion paths among:

- `FleetState`;
- `FrontendSnapshot`;
- clientd protocol frames;
- cron DTO/history projection.

Possible changes are limited to source/codegen deduplication. Do not:

- send remote greggd wire structs directly to the TUI;
- remove the normalization boundary;
- change latest-state watch semantics;
- widen frame/body limits;
- introduce binary serialization merely for size.

## Candidate D: cron presentation code

Only optimize cron cache/UI/sanitizer code if attribution says it is material.

Retain exact semantics for:

- `(epoch, sequence)` deduplication;
- global 4096-record bound;
- display/cache depth validation;
- scheduler errors independent of Systems reachability;
- load-unavailable truthfulness;
- output truncation markers;
- terminal-control escaping;
- `c`, `J`, and `K` behavior.

Avoid replacing clear formatter code with opaque tables/macros for marginal
bytes.

## Qualification and test-only code

`qualification.rs` was added to make Plan-167 memory/body bounds executable,
and it is currently a public library module.

Determine whether any of it contributes to the final binary. With fat LTO,
unused public library functions may already be eliminated, so do not assume the
525-line source file costs runtime bytes.

Do not remove a stable public module/API solely for footprint. If it is byte
neutral, leave it. If a genuine production reference unexpectedly keeps
qualification-only code alive, remove that reference while preserving the
tests/derived bounds.

## Retention threshold

The optimization campaign needs a meaningful win rather than churn.

Use the post-169 stripped `gregg` size as baseline.

Keep a candidate only if:

- it is behavior-preserving under the full existing test suite; and
- it produces a reproducible size reduction under the exact release profile.

The campaign should retain changes only if cumulative reduction is at least
64 KiB, approximately 6% of the ~996 KiB Plan-161 growth and about 1.2% of the
current binary, unless a smaller reduction also removes a concrete duplicated
maintenance boundary with essentially zero risk.

If no safe candidate reaches that threshold, close the plan as RETAIN CURRENT
with attribution explaining where the cost lives.

Do not silently establish a new architecture to meet the threshold.

## Performance and correctness guardrails

For each retained candidate:

- no new dependency;
- no remote request/cadence change;
- no local IPC frame-count increase;
- no extra daemon wake loop;
- no loss of config/lifecycle ownership checks;
- no regression in Plan-167 cache/body bounds;
- no Windows named-pipe/security regression;
- no startup/update/uninstall behavior change;
- no TUI feature loss.

Run the existing multi-client request-count tests after each structural
candidate rather than waiting until the end.

## Measurements

Record before/after:

- stripped `gregg` bytes;
- percentage versus the post-169 baseline;
- percentage of the Plan-161 growth recovered;
- dependency/feature graph diff;
- release build wall time only as informational data;
- optional symbol/module attribution for retained changes.

Also rebuild `greggd` once at closure to prove this client-only pass did not
accidentally widen the daemon.

No RSS target is introduced: Plan 167 already bounded cache memory by design,
and this plan is about linked code footprint/source boundaries.

## Verification

At minimum:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
./scripts/check-local.sh --release
~~~

Because Plan 169 makes Windows Clippy part of the existing job, final CI should
exercise the retained code under the complete six-job existing matrix.

If a candidate is platform-specific, the corresponding native job is required
for closure.

## Documentation

On closure update:

- this plan with exact attribution and byte measurements;
- `plans/README.md`;
- `architecture/gregg-client.md` only if a retained source-boundary
  simplification changes module ownership;
- `.opencode/skills/gregg-client/SKILL.md` and `AGENTS.md` only for new
  enduring boundaries;
- `CHANGELOG.md` only if a user-observable behavior or packaging characteristic
  changes.

Do not rewrite Plans 161-168. If their footprint numbers are superseded, append
a short forward reference where needed.

## Acceptance criteria

- [ ] A post-169 stripped Gregg baseline is recorded.
- [ ] The ~996 KiB growth has symbol/module-level attribution sufficient to
      distinguish real linked code from source-line count.
- [ ] Platform lifecycle code is checked for non-native code retention.
- [ ] Lifecycle/update/uninstall duplication is measured before refactoring.
- [ ] Protocol/DTO and cron code are optimized only when attribution supports it.
- [ ] No public API is removed merely to save bytes.
- [ ] No new runtime dependency is added.
- [ ] Same-binary clientd/TUI architecture is preserved.
- [ ] All Plan-161 polling, IPC, lifecycle, cron, and security invariants remain
      green.
- [ ] Every retained change has a reproducible stripped-size delta.
- [ ] Cumulative retained reduction is at least 64 KiB, or the plan closes
      RETAIN CURRENT with a measured rationale.
- [ ] Final `greggd` footprint is unchanged within ordinary linker
      reproducibility.
- [ ] Existing six-job CI is green on the final SHA.

## Stop conditions

Close RETAIN CURRENT rather than escalating if:

- most growth is unavoidable code needed by the same-binary architecture;
- the only material win requires splitting the release into another executable;
- a candidate weakens ownership/security/polling semantics;
- a candidate needs a new dependency whose own code cancels the reduction;
- a refactor saves less than the noise floor and increases maintenance risk.

Open a separate corrective plan if measurement exposes an actual correctness or
security defect.

## Handoff

If this plan retains a meaningful reduction, the new stripped byte figure
becomes the client baseline for future client-daemon work. If it closes RETAIN
CURRENT, the Plan-161 growth is considered measured and justified, and future
work should not repeatedly reopen it without a new concrete regression.
