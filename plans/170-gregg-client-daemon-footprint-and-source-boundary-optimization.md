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

## Closure record — RETAIN CURRENT

Status: complete (RETAIN CURRENT). No code change was made by this plan.

The +996 KiB the Plan-161 client-daemon line added to `gregg` is, measurement
says, the irreducible cost of one binary containing both a TUI and a background
polling daemon — and a meaningful part of it is not even new code: some of the
growth is code that *moved*.

### Post-169 baseline

Plan 169 is byte-neutral, so Plan 168's figures stand, and they reproduce bit
for bit (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`,
`panic = "abort"`, `cargo build --release -p gregg -p greggd`):

~~~text
gregg    5,345,864 bytes  md5 3dc20315fcbb4afd5d4b1974f7a76c8d
greggd   3,316,568 bytes  md5 86ec567e0b3d8f0d113e3b55dad50f3c
pre-line gregg (ae56926)  4,349,736 bytes
growth  +996,128 bytes / +22.90%
~~~

### Method

Temporary developer tooling only, nothing added to the repository: an unstripped
paired build of HEAD and of pre-line `ae56926` via
`CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=1` into throwaway
target directories, then `nm --print-size --size-sort --radix=d -C` aggregated
by crate and by Gregg module.

A plain "first path segment" split attributes only ~37% of a Rust binary,
because most text is generic instantiations whose demangled name opens with
`<Owner as Trait>` or `[<Owner as Trait>::…]`. The aggregator instead looks for
the first `gregg::` module path *anywhere* in the symbol, so monomorphized
serde glue for a Gregg DTO is charged to that DTO's module rather than to serde.

### Attribution

~~~text
total text  3,247,605 -> 4,121,152   delta +873,547

  bucket          before      after      delta
  gregg           435,459   1,298,675   +863,216
  deps            206,990     175,306    -31,684
  other           571,205     554,655    -16,550
  <unattributed> 2,033,951   2,092,516   +58,565
~~~

Text accounts for ~88% of the 996 KiB file delta; the remainder is rodata,
exception frames, and relocations, which `nm` symbol sizes do not cover.

New `clientd` text, by module:

~~~text
128,844  clientd::snapshot
128,340  clientd::daemon
 96,000  clientd::protocol
 45,953  clientd::cron
 30,032  clientd::startup
 25,187  clientd::launch
 19,701  clientd::frontend
 11,055  clientd::ipc
  1,873  clientd::identity
--------
486,985  clientd total
 38,662  cron            (client-side cache)
  3,799  sanitize
~~~

### Two findings that change how the growth should be read

**1. The line *removed* dependency surface.** Third-party text went **down**
31,684 bytes: `eggfetch_core` −24,617, `hyper_util` −4,912, `core` −13,121. The
pre-line binary contained `eggfetch_core::pipeline::lean::send_lean::{closure#0}`
twice, at 21,843 and 21,841 bytes; HEAD contains it once, at 30,093. And
`gregg::run_tui` went from 41,854 bytes across 11 symbols to 25,266 across 11.
The TUI event loop no longer owns a polling loop, so one copy of the HTTP send
path and the loop that drove it disappeared. This is a stronger result than
Plan 164-166's zero `Cargo.lock` delta: the line did not merely add no new
dependency graph, it shrank the monomorphized surface already there.

**2. A large part of the per-module growth is outlining, not new logic.**
`normalized` +107,078, `eggpool` +81,953, `config` +27,301, `state` +37,267,
`endpoint` +10,827. Pre-line, the TUI inlined these DTO decodes into its own
callers, so they had almost no standalone symbols (`normalized` had 3,238).
Reaching the same types through `FrontendSnapshot`'s `Deserialize` makes them
outlined and separately attributable. Reading those five lines as "the clientd
line added 264 KiB of new logic" would be wrong; the honest reading is that the
attribution boundary moved, and only `clientd::*` itself is new code.

### Duplicated codegen: 346,801 bytes, and not reachable from stable

326 symbol groups have more than one copy, totalling 346,801 bytes of wasted
text — over five times the retention threshold. The largest are serde derive
monomorphizations: `FrontendFrame::deserialize` 24,953; `OptionVisitor<EggpoolSnapshotDto>`
13,998; `OptionVisitor<NormalizedSnapshot>` 11,881; `PhantomData<Vec<SystemSnapshotDto>>`
11,150; `PhantomData<Vec<SystemCronDto>>` 10,509; and 25 copies of a 2,024-byte
`RawVecInner::finish_grow`.

Every one of the 326 groups was hashed: **all are byte-identical**. This is
LLVM not merging identical internal functions, not duplicated source. The source
confirms it — there is exactly one encode site (`encode_document`) and one decode
entry point (`Connection::try_read_frame::<T>` → `serde_json::from_slice`), with
one `T` instantiation, so there is nothing at the source level to collapse.

Reclaiming it needs LLVM's MergeFunctions, which stable rustc does not expose:
`-C llvm-args=-mergefunc`, `--mergefunc`, `-merge-functions`, and
`--enable-merge-functions` are all rejected as unknown LLVM command-line
arguments, and the real flag is the nightly-only `-Zmerge-functions=<aliases>`.
This workspace pins stable and MSRV 1.89, so the win is not reachable without
either a toolchain change (out of scope, and it would change `greggd` too, which
this plan must leave alone) or `#[inline(never)]` codegen hints on derive-generated
code, which is version-fragile churn for a size win outside this plan's
source-boundary scope. Recorded, not taken.

### Candidates

**A — platform lifecycle isolation: rejected on correctness, not on size.**
`clientd::startup` is 30,032 bytes, already under the threshold in isolation.
More importantly, it cannot be cfg-gated: `method_for(os: &str, ...)` takes an
OS *string*, and `render_instructions` matches all four `StartupMethod` variants.
A Linux binary must be able to print launchd and Startup-folder instructions,
because `gregg daemon startup install --method launchd` on Linux is documented
to print the exact command rather than fail, and
`method_selection_is_user_scoped_on_every_platform` locks that in. Splitting the
renderers behind `cfg` would break CLI behavior that the plan explicitly
forbids breaking ("preserve the CLI's current error behavior for unsupported
manager methods"). The module is cross-platform by design, not by accident.

**B — lifecycle/update/uninstall duplication: rejected.** `clientd::launch`
25,187 + `update` 7,199 + `uninstall` 5,977 = 38,363 bytes, below the threshold.
The three are genuinely different state machines — lock-based lazy launch,
prepare-then-quiesce self-replace, ownership-first teardown — and the repeated
shapes the plan asks about (daemon probe, identity handling, bounded wait,
owned/foreign/unknown branching) are small. Consolidating them risks the
ownership and security invariants the plan forbids weakening, in exchange for
bytes that do not clear the bar.

**C — protocol/DTO: no source dedup exists.** The 244,545 bytes in
`clientd::snapshot` + `clientd::protocol` + `clientd::frontend` are almost
entirely serde derive codegen, and the measured duplication is LLVM's, as above.
The one change that would shrink it substantially is switching `FrontendFrame`
from its current representation to adjacently/externally tagged — that is a wire
format change, which Plan 169 closed as a preserved invariant and which this
plan excludes. The fan-out already serializes one document once for all
frontends, which is the correct performance shape and is not duplicated work.

**D — cron presentation: rejected.** `clientd::cron` 45,953 + `cron` 38,662 +
`sanitize` 3,799 = 88,414 bytes, but the largest single item is
`<CronWorker>::observe::{closure#0}` at 26,997 bytes — a real async state
machine, not a formatter. Below threshold once formatters are excluded, and the
plan says not to trade clear formatter code for marginal bytes.

**Qualification and test-only code: contributes nothing.** `qualification.rs` is
525 lines and **zero** linked bytes — `nm` finds no `qualification` symbol in the
release binary, exactly as the plan suspected fat LTO would leave it. It is a
public library module and stays. `mixed_fleet_evidence` and `sustained_workload`
are `#[cfg(test)]` and contribute zero as well.

### Verdict

Cumulative safe reduction available against a 64 KiB bar: **0 bytes**. Every
candidate is either below the threshold, blocked by a correctness requirement, or
locked behind a toolchain change this plan may not make. Under this plan's own
stop conditions — "most growth is unavoidable code needed by the
same-binary architecture" and "a refactor saves less than the noise floor and
increases maintenance risk" — the correct outcome is RETAIN CURRENT.

`greggd` was rebuilt and is unchanged at 3,316,568 bytes with an identical md5.
No new dependency. No public API removed. Same-binary clientd/TUI architecture
preserved. CI run `37265973270` is green on all six jobs, including the
Windows-target Clippy gate Plan 169 added.

### Acceptance criteria

- [x] A post-169 stripped Gregg baseline is recorded. — 5,345,864 / 3,316,568,
      bit-reproducible, unchanged from Plan 168.
- [x] The ~996 KiB growth has symbol/module-level attribution sufficient to
      distinguish real linked code from source-line count. — per-module text
      table, plus the two corrections above: deps shrank, and five of the
      largest deltas are outlining rather than new logic.
- [x] Platform lifecycle code is checked for non-native code retention. — 30,032
      bytes, and it must stay: all four managers are reachable on every platform
      by design.
- [x] Lifecycle/update/uninstall duplication is measured before refactoring. —
      38,363 bytes across three modules, below threshold.
- [x] Protocol/DTO and cron code are optimized only when attribution supports it.
      — neither supports it.
- [x] No public API is removed merely to save bytes. — `qualification` is public
      and byte-neutral, so it stays.
- [x] No new runtime dependency is added.
- [x] Same-binary clientd/TUI architecture is preserved.
- [x] All Plan-161 polling, IPC, lifecycle, cron, and security invariants remain
      green.
- [x] Every retained change has a reproducible stripped-size delta. — no changes
      retained, so nothing to attribute.
- [x] Cumulative retained reduction is at least 64 KiB, or the plan closes
      RETAIN CURRENT with a measured rationale. — RETAIN CURRENT, 0 bytes
      available, with the 346,801-byte LLVM duplication recorded as reachable
      only through a nightly flag this workspace does not use.
- [x] Final `greggd` footprint is unchanged within ordinary linker
      reproducibility. — byte-identical, same md5.
- [x] Existing six-job CI is green on the final SHA.
