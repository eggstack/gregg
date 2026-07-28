# Phase 37: remove release orchestration and archive historical release work

## Objective

Remove Gregg's active release-orchestration, qualification, evidence, provenance, and finalization system so the repository returns to a product-focused shape.

This phase is intentionally deletion-heavy. It must not preserve obsolete machinery behind new names, disabled workflows, compatibility wrappers, or a second-generation release abstraction. The desired state is that ordinary CI verifies source and an operator publishes manually using Phase 39's runbook.

## Dependency and execution position

This is the first phase of the roadmap in Plan 036.

It must complete before:

- Phase 38 simplifies local validation and CI;
- Phase 39 installs the final manual release runbook;
- Phase 40 adds Windows CI-sensitive portability work.

Do not implement Windows support inside this phase except for small compile fixes that are strictly necessary to leave the repository green after release-code deletion.

## Governing invariants

1. Release-only code is deleted, not migrated into another framework.
2. Product tests and product diagnostics remain.
3. GitHub Actions cannot publish crates, push tags, create releases, retrieve prior run artifacts, or finalize releases.
4. No active document requires candidate SHAs, run IDs, artifact IDs, ZIP digests, provenance indices, release selections, role materialization, disposition documents, or evidence ledgers.
5. Historical plans may remain for audit/history only if clearly separated from active execution.
6. A developer can understand the active repository without reading Plans 010 through 035.
7. Removing obsolete tests is correct when the behavior under test is also intentionally removed.
8. The resulting tree must pass normal source checks without release validators.

## Scope

### In scope

- release-related workflows under `.github/workflows/`;
- release workflow validators and tests;
- evidence schemas and qualification contracts under `plans/evidence/`;
- release selection, retrieval, provenance, materialization, aggregation, and finalizer scripts;
- release-only shell helpers;
- historical release evidence ledgers;
- active documentation references to staged release workflows;
- plan-index restructuring;
- GitHub release environments/secrets documentation, where represented in the repository;
- dependency cleanup caused by deleting release tooling.

### Out of scope

- changing the product protocol;
- changing collector behavior;
- adding Windows support;
- publishing any version;
- modifying GitHub repository secrets or environments through automation;
- deleting useful sustained-workload or installed-binary tests merely because they were previously called from release workflows;
- rewriting all historical plans to match the new philosophy;
- introducing a replacement release tool.

## Workstream A: inventory and classify release-related files

Before deletion, produce a short implementation-note section in the commit or plan status update classifying each candidate file as one of:

- **release-only**: delete;
- **product validation used by release**: retain and detach from release assumptions;
- **historical documentation**: archive or mark superseded;
- **ordinary CI**: retain for Phase 38 simplification.

At minimum inspect:

```text
.github/workflows/release-candidate.yml
.github/workflows/release-finalize.yml
.github/workflows/phase35-qualification.yml
.github/workflows/ci.yml
architecture/release-evidence.md
plans/evidence/**
plans/v1.0.1-final-evidence.md
scripts/validate-release-workflow.py
scripts/run-release-orchestration-qualification.py
scripts/validate-qualification-output.py
scripts/validate-release-evidence.py
scripts/github-artifact-retrieval.py
scripts/decode-release-selection.py
scripts/decode-release-disposition.py
scripts/prepare-final-release-inputs.py
scripts/materialize-release-evidence.py
scripts/write-package-provenance.py
scripts/merge-package-provenance.py
scripts/aggregate-release-evidence.sh
scripts/aggregate-candidate-evidence.sh
scripts/registry-reverify.py
scripts/tests/test_*release*
scripts/tests/test_phase*
README.md
CONTRIBUTING.md
AGENTS.md
SECURITY.md
CHANGELOG.md
plans/README.md
```

The list is not exhaustive. Search for these terms across the repository:

```text
release-candidate
release-finalize
phase35-qualification
candidate_sha
workflow_run_id
artifact_id
provenance
selection_base64
disposition_base64
release evidence
qualification contract
Boundary-2
postpublish-verify
```

### Product-validation retention test

Retain a script only if it can answer a product question without release metadata. Examples:

- Does an installed `greggd` binary start and serve a valid response? Retain.
- Does sustained polling stay bounded and preserve state semantics? Retain.
- Does a candidate artifact carry the exact selected GitHub run ID? Delete.
- Does an evidence manifest bind an archive digest across release boundaries? Delete.
- Does a packaged crate include required runtime files and compile? Retain as a simple local package check if still useful.

If a retained script currently accepts release-specific arguments, simplify it to product inputs rather than retaining release abstractions.

### Workstream A acceptance criteria

- [ ] Every release-related workflow, script, test, schema, and document is classified.
- [ ] The classification distinguishes product behavior from release evidence behavior.
- [ ] No file is retained solely because another obsolete release file imports it.
- [ ] The deletion set includes all three active release/qualification workflows.

## Workstream B: delete active release workflows and executable release machinery

Delete:

- staged release-candidate workflow;
- release-finalize workflow;
- Phase-35 qualification workflow;
- validators that exist only to validate those workflows;
- artifact retrieval and selection machinery;
- cross-run aggregation and evidence materialization;
- release provenance generation/merging;
- release qualification harnesses;
- negative contract suites for removed release behavior;
- machine-readable release dispatch/requirements/qualification contracts;
- immutable release evidence ledger files that are not product documentation.

Do not leave disabled copies under another directory. Git history already preserves them.

### Required code-search closure

After deletion, repository search must show:

- no executable `cargo publish` command;
- no GitHub Actions `contents: write` or release environment used for publication;
- no workflow-dispatch input for a candidate SHA, selection document, disposition document, mode, or tag;
- no script importing release-evidence modules;
- no Python test collecting removed release modules;
- no references from ordinary CI to deleted validators.

Documentation may contain the literal `cargo publish` only in Phase 39's manual operator runbook and any concise README link to that runbook.

### Workstream B acceptance criteria

- [ ] Release candidate, finalizer, and qualification workflows are deleted.
- [ ] Release-only scripts and Python tests are deleted.
- [ ] Release-only schemas/contracts are deleted.
- [ ] Ordinary CI does not call a release validator.
- [ ] `git grep` finds no executable publication or tagging path.
- [ ] The workspace still builds after deletion.

## Workstream C: preserve and simplify product validation assets

Review scripts previously called by release workflows. Retain only those with independent value, such as:

- installed daemon verification;
- short daemon HTTP smoke;
- mixed-fleet polling/state smoke;
- sustained workload/resource checks;
- package-content checks that are simple and version-neutral;
- platform-specific runtime diagnostics.

For retained scripts:

1. Remove candidate SHA, run ID, artifact ID, provenance, evidence directory, and release stage arguments unless they are independently meaningful.
2. Make output human-readable by default.
3. Use ordinary exit status as the pass/fail contract.
4. Avoid writing JSON manifests unless the product test itself consumes JSON.
5. Default to a short execution suitable for local use.
6. Support longer runs only through explicit duration flags.
7. Do not upload or archive their output in CI.

Example target shape:

```text
scripts/check-installed-daemon.sh /path/to/greggd
scripts/run-mixed-fleet-smoke.py --duration-seconds 3
scripts/measure-resources.sh --duration-seconds 10
```

Not acceptable:

```text
scripts/run-stage.py --candidate-sha ... --workflow-run-id ... --artifact-id ... --evidence-dir ...
```

### Workstream C acceptance criteria

- [ ] Each retained script has an explicit product-validation purpose.
- [ ] Retained scripts do not require release-stage metadata.
- [ ] Short defaults complete quickly on a development host.
- [ ] Deleted release modules are not imported by retained scripts.
- [ ] Product smoke scripts still pass locally where their platform applies.

## Workstream D: archive historical plans without making them active requirements

The repository currently has a long active chain of release plans. Preserve useful history without forcing future agents to execute it.

Preferred structure:

```text
plans/archive/v1.0.1-release/
  010-...
  011-...
  ...
  022-...
  030-...
  ...
  035-...
  v1.0.1-final-evidence.md
```

Plans 023 through 029 contain product/platform corrections rather than pure release orchestration. Do not archive them automatically. Reassess each:

- retain active if the underlying product defect still exists;
- mark superseded if later code already closed it;
- re-scope into a future product plan if still relevant but no longer a release gate.

If moving many historical files creates excessive churn, an acceptable alternative is:

- keep files in place;
- add a prominent `HISTORICAL — superseded by Plan 036` banner;
- move their rows to a clearly separated historical table in `plans/README.md`;
- remove all dependency language that makes them active release gates.

The implementation agent should prefer physical archival when straightforward because it makes the active plan directory easier to navigate.

### Plan index target

`plans/README.md` should begin with the current roadmap and active product plans. It must state:

- releases are manual;
- CI is source-only;
- Plans 010 through 022 and 030 through 035 describe a retired release model;
- Plan 036 and Phases 037 through 044 are authoritative for this line of work.

### Workstream D acceptance criteria

- [ ] Historical release plans are separated from active plans.
- [ ] No historical plan remains an active dependency or completion gate.
- [ ] Plans 023 through 029 are individually classified rather than blindly archived.
- [ ] `plans/README.md` identifies Plan 036 as the active umbrella roadmap.
- [ ] The active plan list is understandable without reading the archived release chain.

## Workstream E: remove active documentation for the retired model

Update:

- `README.md`;
- `CONTRIBUTING.md`;
- `AGENTS.md`;
- `architecture/README.md`;
- `packaging/README.md`;
- other documents found by search.

Delete explanations of:

- staged release candidate dispatch;
- Phase-35 qualification;
- immutable candidate evidence;
- cross-run artifact retrieval;
- evidence lineage;
- finalizer modes;
- selection and disposition base64 inputs;
- release manifests and role indices.

Replace them only with a short statement:

```text
Releases are performed manually. See RELEASING.md.
GitHub Actions verifies source changes and does not publish artifacts or releases.
```

`RELEASING.md` is created in Phase 39. During Phase 37, either add a temporary link to the Phase 39 plan or create a minimal placeholder that Phase 39 replaces. Do not leave a broken documentation link on `main`.

### Workstream E acceptance criteria

- [ ] User-facing docs no longer describe the retired release system.
- [ ] Contributor docs do not require release evidence.
- [ ] Agent instructions do not direct future agents to repair deleted workflows.
- [ ] Architecture docs focus on the product architecture.
- [ ] No broken links are introduced.

## Workstream F: dependency and test cleanup

After deletion:

1. Remove no-longer-used Python test configuration or dependencies, if any.
2. Remove shellcheck coverage for deleted shell files.
3. Remove workflow-specific fixtures.
4. Remove ignored or skipped tests that only awaited hosted release evidence.
5. Update test-count claims in documentation only if such claims remain useful; preferably remove exact test counts.
6. Run normal workspace checks.

Do not remove Python from the repository if retained product validation scripts still use it.

### Required validation commands

Run, at minimum:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
cargo deny check
```

Also run retained product-smoke scripts on the current native host where practical.

### Workstream F acceptance criteria

- [ ] No test references deleted release files.
- [ ] No CI step references deleted scripts.
- [ ] All normal Rust gates pass.
- [ ] `cargo deny check` passes under the repository's accepted policy.
- [ ] Retained native product smokes pass on the implementation host.

## Explicit deletion guardrails

Do not delete or weaken the following merely to reduce the release system:

- protocol validation and fixtures;
- collector unit/integration tests;
- daemon sampler/readiness tests;
- HTTP endpoint tests;
- config atomicity and transaction tests;
- polling/state/scheduler tests;
- TUI rendering/state tests;
- native service-manager state tests;
- short installed-binary tests;
- useful sustained workload or resource-bound tests.

A test may be relocated or simplified if it is coupled to release metadata, but its product assertion must be preserved.

## Examples of correct outcomes

### Correct

```text
.github/workflows/ci.yml
scripts/check-local.sh
scripts/check-installed-daemon.sh
scripts/run-mixed-fleet-smoke.py
RELEASING.md
```

### Incorrect

```text
.github/workflows/release-disabled.yml
scripts/release-v2.py
plans/evidence/simple-release-contract.json
scripts/write-lightweight-provenance.py
```

### Correct plan status language

```text
Plans 010-022 and 030-035 are historical records of a retired automated
release-evidence model. They are not active acceptance gates.
```

### Incorrect plan status language

```text
The old release gates are optional but should still be run where possible.
```

The latter preserves ambiguity and invites the complexity to return.

## Phase acceptance criteria

Phase 37 is complete only when:

- [ ] The active release-candidate, finalizer, and qualification workflows are gone.
- [ ] No executable repository path publishes crates, pushes tags, or creates GitHub Releases.
- [ ] Release-only evidence/provenance/selection/finalizer scripts and tests are gone.
- [ ] Product-validation scripts used by the old workflows are retained only where independently useful and stripped of release metadata.
- [ ] Release evidence schemas, contracts, and ledgers are removed or archived as inert history.
- [ ] Plans 010 through 022 and 030 through 035 are explicitly historical/superseded.
- [ ] Plans 023 through 029 are individually classified by current product relevance.
- [ ] README, contributor, agent, architecture, packaging, and plan-index documentation no longer describe the retired model as active.
- [ ] No ordinary CI step references removed validators or evidence scripts.
- [ ] The normal workspace checks pass.
- [ ] No replacement release framework is introduced.

## Evidence required for completion

Only lightweight repository evidence is required:

- the committed deletion/update diff;
- passing local command output summarized in the commit or handoff note;
- the resulting active workflow list;
- a repository search showing no automated publish/tag/release path.

Do not create an evidence bundle, manifest, artifact, checksum ledger, or hosted qualification run for this phase.

## Handoff notes for a smaller implementation model

1. Start with a repository-wide search and classification table.
2. Delete one coherent group at a time: workflows, imported scripts, tests, schemas, documentation.
3. Run tests after each group so accidental product-test deletion is caught early.
4. When uncertain whether a script is product validation, identify its inputs and final assertion. If the assertion is about GitHub run/artifact identity, delete it. If it is about Gregg runtime behavior, retain and simplify it.
5. Do not spend effort making old release tests pass after their target behavior is removed.
6. Update `plans/README.md` last, after the actual retained/deleted set is known.
7. Keep the final commit focused on removal and documentation; defer CI redesign to Phase 38 and the full release runbook to Phase 39.