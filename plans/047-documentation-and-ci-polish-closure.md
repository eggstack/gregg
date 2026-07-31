# Phase 47: documentation and CI polish closure

> Completed on 2026-07-30. Local default validation passed; implementation
> commit `452f998` passed ordinary CI run `30599181232` across Linux, both
> macOS architectures, Windows, and MSRV. No release or publication operation
> was performed.

## Objective

Finish the Phase 46 simplification with one small polish pass that removes stale contributor instructions and trivial GitHub Actions boilerplate without reopening Gregg's CI, release, test, or platform-support architecture.

This phase exists because the product and verification model are already substantially complete, but the active contributor guide still advertises deleted `--full` / `-Full` modes. The current workflow also retains a one-entry Linux matrix and installs Rust components on native-platform jobs that no longer use them.

The intended result is not another verification redesign. It is a small consistency correction:

- active documentation describes exactly the two supported local modes;
- the ordinary workflow retains the Phase 46 responsibility split;
- unused workflow indirection and toolchain components are removed;
- no product behavior, release sequencing, or platform claim is expanded;
- closure requires only the lightest checks appropriate to documentation and YAML cleanup.

## Context

Phase 46 established the proportionate verification contract for this repository:

- one read-only `.github/workflows/ci.yml` workflow;
- Linux owns generic format, lint, workspace test, documentation, and Linux collector checks;
- hosted macOS and Windows jobs provide native-platform compilation and targeted native tests;
- one check-only MSRV job remains while the explicit Rust 1.75 compatibility promise remains;
- local validation has only `default` and `release` modes;
- CI does not publish, package, install release candidates, upload evidence, or manage releases;
- crates.io publication, Git tags, and GitHub Releases remain manual.

That implementation passed ordinary CI run `30598220062` at commit `ab97e37d21d31ef107a9c3316c2da9713f9679b7`. Phase 47 must preserve this architecture.

The remaining active inconsistency is in `CONTRIBUTING.md`, which still tells contributors to use commands that no longer exist:

```text
./scripts/check-local.sh --full
.\scripts\check-local.ps1 -Full
```

It also describes the removed full tier as running shellcheck, Python tests, and package checks. This guidance is now false and causes immediate command-line failures.

The workflow has two minor mechanical leftovers:

1. the Linux job uses a matrix containing only `stable`;
2. macOS and Windows request `rustfmt` and `clippy`, although those jobs run neither formatting nor Clippy.

These are not release blockers. They are appropriate for one final polish pass because correcting them reduces configuration noise without altering verification coverage.

## Scope

### Required work

1. Correct active contributor documentation to describe only the supported local commands.
2. Remove obsolete `--full`, `-Full`, and related full-tier wording from active documentation.
3. Simplify the one-entry Linux toolchain matrix into a direct stable Linux job.
4. Stop installing unused Rust components on macOS and Windows jobs.
5. Preserve every substantive Phase 46 check and platform responsibility.
6. Run only the minimal validation required for these changes.
7. Update the plan registry after the final ordinary CI run passes.

### Explicitly out of scope

Do not:

- modify Rust product code;
- change protocol behavior or schema;
- change collector, service-manager, daemon, client, or TUI behavior;
- add or remove supported operating systems or architectures;
- add another CI workflow;
- add reusable workflows, composite actions, scripts, Makefiles, task runners, or CI generators;
- add shellcheck, cargo-deny, Python tests, package checks, installation checks, or publish dry-runs to ordinary CI;
- add back a `full` local-check tier;
- add new test harnesses for the local scripts;
- add Windows SCM installation rehearsal to CI;
- add self-hosted runners, remote hosts, secrets, OIDC, environments, or write permissions;
- add artifacts, evidence bundles, manifests, checksums, reports, or qualification records;
- run a release preflight merely to validate documentation/YAML polish;
- publish crates, create tags, or create a GitHub Release;
- refactor the workflow beyond the exact mechanical cleanup described here;
- perform unrelated dependency, formatting, naming, or documentation cleanup.

If an unrelated defect is discovered, record it separately. Do not expand Phase 47 to absorb it.

## Target end state

### Local command contract

Active contributor-facing documentation should present two commands for normal development:

```text
./scripts/check-local.sh
.\scripts\check-local.ps1
```

Maintainer release preparation may be shown separately:

```text
./scripts/check-local.sh --release
.\scripts\check-local.ps1 -Release
```

The distinction must be explicit:

- normal contributors run the default check before opening a pull request;
- maintainers run the release preflight only when preparing a manual release;
- the release preflight is nonpublishing;
- dependent-crate publication sequencing remains documented in `RELEASING.md`, not duplicated in contribution guidance.

No active document may imply the existence of a third `full` tier.

### CI contract

The repository must still contain exactly one ordinary CI workflow with read-only contents permission.

Its responsibilities remain:

#### Linux

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-targets --all-features`
- `cargo doc --workspace --no-deps`
- targeted native Linux collector tests

The Linux job should run directly on `ubuntu-latest` with stable Rust. A one-entry matrix is not needed.

#### macOS

- native workspace compilation;
- targeted macOS collector/FFI native tests;
- both currently advertised hosted macOS architectures.

The macOS job must not install `rustfmt` or `clippy` unless it actually runs those tools.

#### Windows

- native workspace compilation;
- Windows client tests;
- Windows collector tests;
- Windows service-manager tests;
- bounded foreground daemon v2 smoke.

The Windows job must not install `rustfmt` or `clippy` unless it actually runs those tools.

#### MSRV

- one check-only Rust 1.75 job while `rust-version = "1.75"` remains an explicit supported contract.

Phase 47 must not alter the MSRV policy.

## Workstream A: correct `CONTRIBUTING.md`

### Required edits

Replace the stale pre-merge full-check section with accurate guidance.

Recommended structure:

1. Keep the existing normal local validation commands under the contribution steps.
2. State that these commands run the ordinary local developer checks appropriate to the current host.
3. Add a brief maintainer-only note for the release preflight, or link to `RELEASING.md` instead of duplicating detail.
4. Remove all claims that contributors should run shellcheck, Python tests, package checks, or an obsolete full tier.
5. Preserve the statement that releases are manual and CI never publishes.

A suitable contributor-facing shape is:

```text
Run the normal local validation command before opening a pull request:

./scripts/check-local.sh
.\scripts\check-local.ps1

Maintainers preparing a release should follow RELEASING.md and use the
nonpublishing --release / -Release preflight.
```

Exact prose may vary, but the behavioral contract may not.

### Acceptance criteria

- `CONTRIBUTING.md` contains no `--full` command.
- `CONTRIBUTING.md` contains no `-Full` command.
- `CONTRIBUTING.md` does not describe a full tier.
- normal contributor instructions point to the default Bash and PowerShell checks;
- any release-preflight reference is clearly maintainer-oriented and nonpublishing;
- manual release policy still points to `RELEASING.md`.

## Workstream B: search active documentation for stale mode references

Search active, nonarchived repository guidance for obsolete local-check flags and wording.

At minimum inspect:

- `README.md`
- `CONTRIBUTING.md`
- `AGENTS.md`
- `RELEASING.md`
- `architecture/`
- current plan registry text
- nonarchived plans 036 through 047 where their status text describes the current contract

Use direct searches such as:

```bash
git grep -n -- '--full' -- ':!plans/archive/**'
git grep -n -- '-Full' -- ':!plans/archive/**'
git grep -n -- 'SkipDeny' -- ':!plans/archive/**'
git grep -n -i -- 'full local check' -- ':!plans/archive/**'
```

Historical descriptions inside completed or superseded plan bodies may remain when they are clearly marked as historical. Do not rewrite hundreds of lines of old planning history merely to make broad searches empty.

The correction rule is:

- fix active instructions that a maintainer or contributor could reasonably follow today;
- preserve historical implementation context when clearly superseded;
- do not edit archived plans.

### Acceptance criteria

- no active user/contributor guide advertises `--full`, `-Full`, or `SkipDeny`;
- any surviving matches outside `plans/archive/` are clearly historical or superseded plan text, not current instructions;
- no archived plan is modified;
- no broad documentation rewrite is introduced.

## Workstream C: remove the one-entry Linux matrix

The current Linux job has a strategy matrix with one included toolchain, `stable`. Replace this with a direct job configuration.

Expected mechanical changes:

- change the displayed job name from `Linux (${{ matrix.toolchain }})` to `Linux`;
- remove `strategy`, `fail-fast`, and the one-entry `matrix` block;
- install stable Rust directly;
- retain `rustfmt` and `clippy` components on Linux because Linux runs both tools;
- leave all Linux commands and their ordering unchanged;
- do not split or combine Linux commands;
- do not add conditionals or path filters.

### Acceptance criteria

- the Linux job contains no matrix or strategy block;
- the Linux job still runs on `ubuntu-latest`;
- Linux still installs stable Rust with `rustfmt` and `clippy`;
- every existing Linux verification command remains present exactly once;
- no Linux verification responsibility moves to another platform.

## Workstream D: stop installing unused components on native jobs

The macOS and Windows jobs currently request `rustfmt` and `clippy` from `dtolnay/rust-toolchain`, but neither job runs those tools.

Simplify those setup steps by removing the unused component requests.

Acceptable forms include:

```yaml
- name: Install Rust stable
  uses: dtolnay/rust-toolchain@stable
```

or an equivalent explicit stable-toolchain configuration without components.

Do not replace the action, pin a new revision, introduce a setup wrapper, or change the toolchain channel as part of this phase.

### Acceptance criteria

- macOS Rust setup does not request `rustfmt`;
- macOS Rust setup does not request `clippy`;
- Windows Rust setup does not request `rustfmt`;
- Windows Rust setup does not request `clippy`;
- Linux continues to request both components;
- all macOS and Windows check/test steps remain unchanged;
- both macOS architectures remain in the matrix;
- the Windows foreground v2 smoke remains present.

## Workstream E: preserve the minimal verification boundary

Review the final diff specifically for accidental verification expansion.

The final `.github/workflows/ci.yml` must still have:

- one workflow file;
- triggers for `push` to `main`, pull requests, and manual dispatch;
- `permissions: contents: read`;
- no secrets or environment declarations;
- no artifact upload/download;
- no publishing commands;
- no package listing;
- no installed-binary release smoke;
- no cargo-deny or shellcheck;
- no scheduled event;
- no release event;
- no workflow chaining;
- no elevated Windows SCM operation.

### Acceptance criteria

Repository searches confirm that ordinary CI contains none of:

```text
cargo publish
cargo package --list
cargo install
upload-artifact
download-artifact
cargo deny
shellcheck
schedule:
release:
workflow_run:
```

A textual match in comments should also be removed if it creates ambiguity. Historical archived workflow files should not exist.

## Validation strategy

This is a documentation and workflow-polish phase. Validation must remain proportionate.

### Required local checks

Run:

```bash
./scripts/check-local.sh
```

On Windows, the equivalent default PowerShell check may be used instead:

```powershell
.\scripts\check-local.ps1
```

Only one host-local default check is required. Do not require both scripts to run locally.

Also inspect the workflow diff and perform the stale-reference searches from Workstream B.

### Required hosted check

Push the implementation and require one ordinary CI run at the implementation SHA.

The run must show success for:

- Linux;
- both macOS matrix entries;
- Windows;
- MSRV.

That single ordinary run is the authoritative validation of the workflow edit and native jobs.

### Explicitly unnecessary validation

Do not require:

- `./scripts/check-local.sh --release`;
- `.\scripts\check-local.ps1 -Release`;
- package-list review;
- installed-binary release smoke;
- protocol publish dry-run;
- dependent-crate publish dry-runs;
- a second CI run;
- a dedicated workflow-dispatch rerun after a successful push run;
- manual Windows service installation;
- a self-hosted or remote platform;
- evidence files or screenshots.

## Execution sequence

1. Confirm the branch is based on the latest `main` commit.
2. Edit `CONTRIBUTING.md` to remove obsolete full-tier instructions.
3. Search active documentation for other stale current-mode references and correct only genuine active guidance.
4. Remove the one-entry Linux matrix from `.github/workflows/ci.yml`.
5. Remove unused `rustfmt` and `clippy` component requests from macOS and Windows setup.
6. Review the complete diff for scope creep.
7. Run the normal local validation command once.
8. Run the focused repository searches in Workstreams B and E.
9. Commit and push the coherent polish change.
10. Observe one ordinary CI run at that implementation SHA.
11. If CI fails because of the polish change, correct only the specific defect and rerun the ordinary workflow.
12. Once green, add a concise completion note to this plan and mark Phase 47 complete in `plans/README.md`.
13. Stop. Do not start another verification cleanup pass without a concrete defect.

## Failure handling

### Documentation search finds many historical matches

Do not rewrite historical plans. Classify the match:

- active instruction: correct it;
- superseded plan body: leave it unless its status header incorrectly presents it as active;
- archived plan: leave it unchanged.

### CI YAML fails to parse

Correct the indentation or action configuration directly. Do not add a YAML linter or a workflow generator.

### Removing components breaks a native job

First verify that the job does not actually invoke the missing component. If an action or command unexpectedly requires it, restore only the specific required component and document why in the implementation commit. Do not restore both components speculatively.

### Hosted runner label changes or is unavailable

Do not redesign the workflow in this phase. Preserve the existing advertised runner labels unless GitHub rejects them. A runner-platform migration requires a separate concrete decision.

### Local default check fails in unrelated product code

Report the pre-existing failure. Do not expand Phase 47 into product debugging unless the failure is caused by the Phase 47 diff.

## Explicit acceptance criteria

Phase 47 is complete only when all of the following are true.

### Documentation

- [ ] `CONTRIBUTING.md` no longer advertises `--full` or `-Full`.
- [ ] `CONTRIBUTING.md` accurately presents the default Bash and PowerShell checks.
- [ ] release-preflight guidance, if present, is maintainer-only and points to `RELEASING.md`.
- [ ] active user/contributor documentation contains no obsolete full-tier instructions.
- [ ] archived planning history is not rewritten.

### Workflow simplification

- [ ] `.github/workflows/ci.yml` remains the only active workflow.
- [ ] workflow permissions remain read-only.
- [ ] the Linux one-entry matrix is removed.
- [ ] Linux still performs fmt, Clippy, workspace tests, docs, and native Linux collector checks.
- [ ] macOS no longer installs unused rustfmt/clippy components.
- [ ] Windows no longer installs unused rustfmt/clippy components.
- [ ] both macOS architectures remain covered.
- [ ] Windows client, collector, service-manager, and foreground v2 tests remain covered.
- [ ] the check-only MSRV job remains unchanged in responsibility.

### No verification expansion

- [ ] no new workflow, helper framework, or test harness is added.
- [ ] no package, install, publish, artifact, evidence, or release operation enters CI.
- [ ] no full local-check tier is reintroduced.
- [ ] no product code is modified.
- [ ] no platform-support claim is expanded.

### Validation

- [ ] one normal local default check passes on the implementation checkout.
- [ ] stale-reference searches identify no active obsolete instructions.
- [ ] one ordinary CI run passes at the implementation SHA.
- [ ] no release preflight or manual Windows service evidence is required.

### Registry closure

- [ ] this plan contains a concise completion note naming the implementation SHA and ordinary CI run.
- [ ] `plans/README.md` marks Phase 47 completed only after that CI run succeeds.
- [ ] the registry states that no verification/release corrective phase remains open.
- [ ] no separate evidence document is created.

## Handoff guidance for a smaller implementation model

Keep the change mechanical. The expected implementation diff should normally touch only:

```text
CONTRIBUTING.md
.github/workflows/ci.yml
plans/047-documentation-and-ci-polish-closure.md   # completion note after CI
plans/README.md                                    # final status after CI
```

A small additional active documentation edit is acceptable only when a direct search finds another stale current instruction.

Do not infer new work from old plan bodies. Phase 44 and Phase 45 contain superseded requirements; Phase 46 defines the active verification boundary. This plan only fixes consistency around that boundary.

Before changing the workflow, compare each job before and after:

- the same substantive commands must remain;
- only one-entry Linux matrix indirection and unused component installation should disappear;
- no job should gain new steps;
- no test command should become broader;
- no check should move between operating systems.

Before closing, report results in a few lines, for example:

```text
Phase 47 completed at <sha>.
Local default check: passed on <host>.
Ordinary CI run <run-id>: Linux, macOS arm64, macOS x86-64, Windows, and MSRV passed.
No release or publication operation was performed.
```

Do not create a closure report file, JSON result, checksum list, or artifact index.

## Stop condition

Stop when the stale contributor commands are corrected, the two trivial workflow leftovers are removed, the default local check passes, and one ordinary CI run is green.

At that point this line of work is complete. Further reductions should require a measured cost or a concrete defect, not another general verification review.
