# Plan 105: source-boundary, repository-hygiene, and MSRV review

Status: planned.

Depends on: Plan 103; preferably Plan 104 first so updater ownership is settled before module decomposition.

## Objective

Reduce maintenance cost without changing Gregg's product behavior by cleaning repository artifacts, splitting policy-heavy modules at natural boundaries, and making the Rust MSRV/dependency-pin policy explicit and evidence-based.

This plan is deliberately behavior-preserving.

## Baseline findings

The top-level three-crate architecture is sound, but several internal modules have accumulated multiple responsibilities and large test surfaces. The review also found one obvious repository artifact: `crates/gregg/src/bin/probe_top.rs` is auto-discovered and built as a normal binary despite being a diagnostic connectivity probe with a historical default LAN address.

The client manifest additionally contains multiple direct dependencies whose comments indicate they exist only to constrain transitive resolution for Rust 1.75. These constraints may still be justified, but each is now maintenance policy and should be treated as such.

## Scope

### 1. Remove `probe_top` from the production binary set

Preferred outcome: delete `crates/gregg/src/bin/probe_top.rs` if its investigation is complete.

If its functionality remains useful for maintainers, move it to one of:

```text
examples/
```

or an explicitly declared helper binary behind a non-default feature such as the existing test/helper boundary.

Required invariant: ordinary `cargo build -p gregg` and published `gregg` package builds must not produce a `probe_top` executable.

No hard-coded private/historical LAN address should remain in a distributed helper.

### 2. Split large policy-heavy modules mechanically

Use module decomposition only where ownership becomes materially clearer. Candidate boundaries include:

```text
crates/greggd/src/startup.rs
  -> startup/detect.rs
  -> startup/systemd.rs
  -> startup/launchd.rs
  -> startup/cron.rs
  -> startup/state.rs

crates/gregg/src/config.rs
  -> config/model.rs
  -> config/store.rs
  -> config/validation.rs
  -> config/lock.rs
```

Other modules may be split if source inspection shows similarly clean seams, but line-count reduction alone is not a criterion.

Do not introduce dynamic dispatch, generic frameworks, service locators, or trait layers solely to make files shorter.

Keep stable façade modules/re-exports where that avoids call-site churn.

### 3. Preserve subsystem ownership

Module moves must preserve the current architecture:

- protocol owns wire data/validation only;
- daemon collector owns platform collection;
- sampler owns cadence/publication;
- server owns HTTP serving of cached state;
- run owns runtime supervision;
- startup owns startup manager installation/state/restart;
- client scheduler owns polling cadence/concurrency;
- client state owns TUI reducer state;
- EggPool remains optional and isolated.

Do not use this plan to merge crates or redesign runtime boundaries.

### 4. Audit compatibility-only dependency pins

Inspect every direct dependency that exists primarily to constrain transitive resolution/MSRV compatibility, especially the client manifest compatibility block.

For each such dependency record one of:

```text
KEEP: required for Rust 1.75-compatible resolution; evidence = ...
REMOVE: no longer required; lockfile/resolution remains valid without it
REPLACE: a narrower direct constraint or upstream dependency change is preferable
```

Use actual resolution/build evidence, not assumptions.

Do not remove pins in bulk merely because they look unusual.

### 5. Make an explicit MSRV decision

Gregg currently declares Rust 1.75. Evaluate whether retaining that floor remains worthwhile now that prebuilt binaries exist for the primary supported targets.

Consider:

- whether Rust 1.75 still builds/tests all supported source paths;
- how many direct compatibility pins are required solely for that floor;
- whether those pins materially increase maintenance or security-update friction;
- source-build users such as ARMv7/unknown architectures that do not receive prebuilt binaries;
- crates.io consumers and downstream library usage.

The plan does not presume that MSRV should rise.

Acceptable outcomes:

A. retain Rust 1.75 and document why the compatibility surface remains justified; or

B. raise MSRV to the lowest evidence-supported version that materially simplifies dependency maintenance, with changelog/docs and CI adjusted deliberately.

Do not raise MSRV incidentally as a side effect of dependency updates.

### 6. Keep EggPool bounded

Do not remove the existing optional EggPool summary integration.

Do not add new provider/application-specific integrations in this pass. If source cleanup touches EggPool code, preserve the existing bounded worker/config/rendering contract.

## Implementation sequence

### Step 1: remove or quarantine `probe_top`

Verify Cargo metadata/bin discovery before and after the change.

Acceptance at this step requires that the normal package exposes only intended binaries/helpers according to explicit Cargo configuration.

### Step 2: characterize module seams

Before moving code, identify cohesive ownership blocks and their tests. Prefer moves that can be verified with no functional edits.

For each module split:

- preserve public/crate-private interfaces where practical;
- move tests alongside the implementation they cover;
- avoid circular dependencies between sibling modules;
- keep platform `cfg` boundaries obvious.

### Step 3: perform mechanical decomposition

Keep commits narrow enough that a reviewer can distinguish moves from behavioral changes.

If a proposed split creates more glue than clarity, do not retain it; record the no-change decision in the plan closure.

### Step 4: dependency/MSRV experiment

Use a clean branch/worktree and run controlled manifest experiments.

For each compatibility pin candidate:

1. remove only that pin or the smallest related group;
2. update/resolve the lockfile intentionally;
3. run Rust 1.75 check/build/tests where relevant;
4. inspect the resolved transitive version that causes success/failure;
5. record the result.

If evaluating a higher MSRV, repeat with candidate compiler versions and compare manifest simplification.

Do not commit lockfile churn unrelated to the chosen final policy.

### Step 5: docs cleanup

Update architecture documents to reflect module locations but avoid rewriting conceptual architecture that has not changed.

Reconcile stale planning/index statements encountered during this work, including outdated "in implementation" text for already completed plans where current repository evidence is clear.

Do not rewrite historical evidence inside old plan records; correct the active index/current-direction text instead.

## Verification

Mandatory:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
```

Additionally:

- inspect `cargo metadata` / build outputs to prove `probe_top` is not a normal production binary;
- run package checks (`cargo package --list` or equivalent non-publishing inspection) for `gregg` and `greggd`;
- if Rust 1.75 is retained, run the existing MSRV verification successfully after dependency cleanup;
- if MSRV changes, update the existing CI MSRV job rather than adding another permanent job;
- run the existing native macOS/Windows CI jobs after module moves affecting cross-platform code.

No new broad CI workflow is required.

## Acceptance criteria

Plan 105 is complete only when:

1. `probe_top` is deleted or explicitly non-production/feature-gated and no normal Gregg build emits it.
2. No historical/private default LAN address remains in a distributed diagnostic helper.
3. Policy-heavy modules are split only at natural ownership seams; no abstraction framework is added.
4. Existing runtime, polling, protocol, collector, startup, control-socket, update, and EggPool behavior remains unchanged.
5. Each compatibility-only direct dependency pin has a documented keep/remove/replace decision backed by resolution/build evidence.
6. The MSRV has an explicit recorded decision and is not changed accidentally.
7. If MSRV is retained, Rust 1.75 verification still passes; if raised, the change is documented and the existing MSRV CI job reflects the new floor.
8. Package contents contain only intended binaries/files.
9. Active architecture docs and `plans/README.md` reflect current module ownership/status without falsifying historical plan records.
10. Full local verification and applicable existing native CI jobs pass.
