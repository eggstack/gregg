# Phase 55: Phase 54 closure record correction

Status: planned; sole remaining closure-record correction for the completed drive-metrics and multi-view line of work.

## Objective

Close Phase 54 truthfully and minimally after its implementation landed correctly but its planning status was marked complete before the required ordinary hosted CI result was recorded.

This phase must:

1. preserve the completed Phase 54 implementation at baseline commit `561e398e42812933755168bc6488f72bd40ed767`;
2. obtain or identify one successful ordinary CI run covering the source-equivalent Phase 54 result on Linux, macOS, Windows, and the Rust 1.75 MSRV job;
3. correct the inconsistent Phase 54 and plan-index status wording;
4. record only the concise implementation SHA, tested SHA, CI run number, and local-check result needed to justify closure.

This is a documentation and ordinary-verification closure pass. It is not an implementation phase and must not modify production code, tests, manifests, dependencies, workflows, release tooling, or product behavior.

## Background

Phase 54 corrected the intended defects in two commits:

```text
07d46350c4f8a07832c95da8640149793af38721
    fix: close drive multiview corrective polish

561e398e42812933755168bc6488f72bd40ed767
    fix: validate windows drive string counts
```

The final implementation baseline is `561e398e42812933755168bc6488f72bd40ed767` because the second commit corrected the Windows return-value type flow while preserving the same planned behavior.

Post-implementation review found:

- the viewport correction is present and focused tests cover the four-row/five-row boundary, one-row offline behavior, and clipping of drive-detail rows only;
- macOS `getmntinfo` zero-return handling now maps to `SourceUnavailable`;
- Windows `GetLogicalDriveStringsW` zero-return handling now applies to both the initial and resized calls;
- collector tests preserve `drives: None` for top-level failure and `Some([])` for successful empty enumeration;
- no protocol, dependency, workflow, release, or architecture expansion occurred.

However, closure is not yet truthful because:

- `plans/054-drive-multiview-corrective-polish.md` says `Status: completed` while its acceptance checklist remains unchecked and no CI run is recorded;
- the plan explicitly required one ordinary CI run before completion;
- `plans/README.md` simultaneously describes Phase 54 as the remaining corrective pass and marks it completed in the table;
- no hosted status was attached to `561e398e42812933755168bc6488f72bd40ed767` at the time of review.

## Governing constraints

1. Do not change any file under `crates/`.
2. Do not change `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, scripts, fixtures, architecture documents, public READMEs, or `AGENTS.md`.
3. Do not add, edit, rename, or rerun through a modified GitHub Actions workflow.
4. Do not add a dedicated qualification workflow, evidence workflow, status script, or CI helper.
5. Do not create evidence bundles, logs, manifests, screenshots, artifact indexes, or a separate closure report file.
6. Do not add more tests; the missing item is hosted execution of the tests already added by Phase 54.
7. Do not perform crates.io publication, tagging, GitHub Release creation, package installation rehearsal, soak testing, or release preflight.
8. Do not require repeated green CI runs. One ordinary run is sufficient.
9. Do not rewrite completed Plans 048 through 053.
10. Keep edits limited to this plan, `plans/054-drive-multiview-corrective-polish.md`, and `plans/README.md`.

## Source-equivalence rule

The preferred tested SHA is the implementation baseline:

```text
561e398e42812933755168bc6488f72bd40ed767
```

A later descendant may be used for hosted closure only when all changes after `561e398e` are limited to files under `plans/` and the following remain byte-for-byte unchanged from the baseline:

```text
crates/**
Cargo.toml
Cargo.lock
rust-toolchain.toml
.github/workflows/**
scripts/**
```

Use a normal commit comparison to establish this. Record the later tested SHA and state that it is source-equivalent to `561e398e` through plan-only changes. Do not invent an empty code change merely to obtain a new SHA.

## Workstream A: establish one ordinary hosted CI result

### Preferred order

1. Check whether an ordinary CI run already exists for `561e398e42812933755168bc6488f72bd40ed767`.
2. If such a run exists and all required jobs succeeded, use it.
3. Otherwise, use the ordinary CI run produced by the current plan-only descendant, provided the source-equivalence rule above is satisfied.
4. If the ordinary workflow supports an existing manual dispatch or rerun mechanism, using that existing mechanism is acceptable.
5. Do not edit workflow triggers or workflow contents to force a run.

### Required job result

The single selected run must report success for the existing jobs representing:

- Linux;
- macOS Apple Silicon;
- macOS Intel;
- Windows;
- Rust 1.75 MSRV.

Use the actual current job names from the workflow. Do not add matrix entries or require architecture combinations that the existing workflow does not define.

### Failure handling

If the ordinary run fails:

- inspect the failing step;
- if the failure is transient infrastructure, rerun that same ordinary run once;
- if the failure indicates a real source defect, stop Phase 55 and create a separate implementation-correction request rather than modifying code under this closure plan;
- do not mark Phase 54 or Phase 55 complete while a substantive failure remains.

A second full green run is not required after one successful ordinary run.

## Workstream B: reconcile the local-check record

Phase 54 required the existing default local check. Use the lightest truthful path:

1. If the implementation handoff or commit record contains a specific successful `./scripts/check-local.sh` or `.\scripts\check-local.ps1` result for the final Phase 54 source, record that result concisely.
2. If no trustworthy result exists, run the appropriate existing local check once on `561e398e` or a source-equivalent descendant.
3. Record only the command, platform, and pass/fail result in Phase 54's closure note.

Do not create a transcript, log file, evidence directory, or command-output appendix. Do not run release preflight.

## Workstream C: correct Phase 54 status and closure metadata

Before hosted CI succeeds, Phase 54 must not claim full completion. Change its leading status to wording equivalent to:

```text
Status: implementation complete at `561e398e`; closure verification pending Phase 55.
```

After the selected ordinary CI run succeeds, update Phase 54 once more to:

```text
Status: completed; implementation `561e398e`; ordinary CI run `<run-number>` passed at `<tested-sha>`.
```

Immediately below the status or in a short `Closure` section, record:

- implementation SHA: `561e398e42812933755168bc6488f72bd40ed767`;
- tested SHA;
- whether the tested SHA is identical to or plan-only/source-equivalent to the implementation SHA;
- ordinary CI run number;
- successful existing job coverage;
- local-check command and platform;
- confirmation that no release action occurred.

Update the existing acceptance checklist to checked items only after the corresponding statements are true. Do not replace the checklist with a new evidence format.

## Workstream D: correct the plan index

Update `plans/README.md` in two stages or in one final closure edit, depending on CI timing.

While Phase 55 is open, the index must state:

- Roadmap 048 and Phases 49 through 53 are complete;
- Phase 54 implementation is complete at `561e398e`, but its closure record is being corrected by Phase 55;
- Phase 55 is the sole open closure-record correction;
- no product implementation, release, or CI-design work remains open.

The table should use statuses equivalent to:

```text
054 | implementation complete at `561e398e`; closure pending 055
055 | planned; sole open closure-record correction
```

After the selected CI run succeeds and Phase 55 is closed, the index must state:

- Phase 54 completed with its implementation SHA and CI run number;
- Phase 55 completed as the closure-record correction;
- no corrective phase remains open for the drive-metrics and multi-view line;
- the dependency summary ends with `54 -> 55` and identifies both as completed.

Remove all wording that calls Phase 54 a remaining corrective pass after it is recorded complete.

## Expected file footprint

Only these files may change during execution:

```text
plans/054-drive-multiview-corrective-polish.md
plans/055-phase-54-closure-record-correction.md
plans/README.md
```

The initial creation and registration of Phase 55 may occur in separate commits. Final closure should be one concise documentation-only commit after CI succeeds.

## Verification strategy

### Repository comparison

Confirm that the tested SHA is either:

- exactly `561e398e42812933755168bc6488f72bd40ed767`; or
- a descendant whose changes from `561e398e` are restricted to `plans/`.

### Local verification

Use one existing local check only if no trustworthy Phase 54 result is already available:

```text
./scripts/check-local.sh
```

or:

```text
.\scripts\check-local.ps1
```

### Hosted verification

Use one ordinary existing CI run. Verify the existing Linux, macOS, Windows, and MSRV jobs all succeeded.

### Documentation verification

Search the active plan files for contradictory phrases, including:

```text
sole remaining corrective pass
Status: completed
closure verification pending
planned; sole open
```

The final state must contain no contradiction about whether Phases 54 and 55 are open or complete.

No CI run is required after the final documentation-only closure commit, provided that commit changes only the three allowed plan files and the recorded tested SHA is source-equivalent to the final implementation.

## Acceptance criteria

Phase 55 is complete only when all of the following are true.

### Implementation baseline

- [ ] `561e398e42812933755168bc6488f72bd40ed767` is recorded as the final Phase 54 implementation baseline.
- [ ] No production code, tests, manifest, dependency, script, workflow, or release file changes under Phase 55.
- [ ] Any tested descendant is proven to differ from `561e398e` only under `plans/`.

### Verification

- [ ] One ordinary CI run succeeded for `561e398e` or a source-equivalent descendant.
- [ ] The run includes successful existing Linux, macOS Apple Silicon, macOS Intel, Windows, and Rust 1.75 MSRV jobs.
- [ ] The CI run number and tested SHA are recorded in Phase 54.
- [ ] One successful existing local check is recorded, either from the Phase 54 implementation handoff or from one new bounded run.
- [ ] No repeated qualification, release preflight, evidence bundle, or new workflow was introduced.

### Documentation consistency

- [ ] Phase 54 no longer claims completion without its required CI and local-check metadata.
- [ ] Phase 54's acceptance checklist reflects the verified final state.
- [ ] `plans/README.md` no longer describes Phase 54 as both remaining and completed.
- [ ] Phase 55 is registered as the sole open closure correction while work is pending.
- [ ] After closure, both Phase 54 and Phase 55 are marked completed and the index states that no drive/multi-view corrective phase remains open.
- [ ] The dependency summary includes `54 -> 55` without creating another roadmap.

### Scope

- [ ] No code fix, refactor, new test, new dependency, new CI job, or workflow modification was made.
- [ ] No tag, GitHub Release, crates.io publication, or packaging action was performed.
- [ ] Final edits are limited to the three expected plan files.

## Handoff sequence

A smaller execution model should follow this exact order:

1. Read this plan, Phase 54, `plans/README.md`, and the current CI workflow.
2. Confirm current main descends from `561e398e`.
3. Compare current main with `561e398e` and ensure intervening changes are plan-only before using a later CI run.
4. Change Phase 54 status to `implementation complete; closure pending Phase 55` if CI has not yet succeeded.
5. Register Phase 55 as the sole open closure-record correction in `plans/README.md`.
6. Locate an existing ordinary CI run for `561e398e` or use the ordinary run for a source-equivalent plan-only descendant.
7. Verify every existing Linux, macOS, Windows, and MSRV job succeeded.
8. Locate a trustworthy Phase 54 local-check result; otherwise run the existing local check once.
9. Update Phase 54 with the implementation SHA, tested SHA, CI run number, job coverage, and concise local-check result.
10. Check Phase 54's acceptance boxes that are now demonstrably true.
11. Mark Phase 55 completed and update `plans/README.md` to state that no corrective phase remains open.
12. Commit only the final plan-document updates.
13. Do not run CI again for that final plan-only closure commit.

## Stop conditions

Stop and request a separate corrective implementation plan if any of the following occurs:

- CI exposes a real compile, test, clippy, documentation, or platform-native failure;
- the tested descendant contains source, manifest, script, or workflow changes after `561e398e`;
- truthful closure appears to require modifying a workflow or adding a new CI mechanism;
- the local check fails for a substantive source reason;
- additional product behavior is proposed.

Do not absorb any such work into this closure-record pass.
