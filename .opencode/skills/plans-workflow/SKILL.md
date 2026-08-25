---
name: plans-workflow
description: Read, create, and close phase plans under plans/, including README index maintenance
---

## What I do

Guide agents through this repository's plan-driven workflow: finding the active
plan, registering new plans, recording closure truthfully, and keeping
`plans/README.md` accurate.

## When to use me

Use this before starting any implementation task that will be recorded as a
plan, when asked to create or close a phase plan, or when updating the plans
index.

## Where things live

| Path | Purpose |
|------|---------|
| `plans/README.md` | Index: current direction, roadmap status, per-plan table, dependency order, verification model, completion rule |
| `plans/000-roadmap-v1.md` | Original v1 sequencing and release gates |
| `plans/NNN-*.md` | Numbered phase plans (implementation + acceptance criteria + closure record) |
| `plans/archive/` | Retired staged-release/evidence plans (010-035); not current requirements |
| `plans/tui-manual-tests.md` | Manual TUI test checklist |

## Rules

### Creating a plan

1. Use the next sequential three-digit number (`ls plans/*.md | tail`).
2. Name the file `NNN-short-slug.md` with a kebab-case slug describing the scope.
3. State concrete scope decisions and preserved exclusions ("what not to redo").
4. Register the plan in `plans/README.md`: add a row to the per-plan status
   table, extend the dependency-order chain, and add a short status paragraph
   if the plan introduces new product behavior.

### Closing a plan

A phase is complete only when its explicit acceptance criteria are implemented
and demonstrated by the lightest appropriate mechanism:

- deterministic unit/integration tests;
- the default local check (`./scripts/check-local.sh`);
- the release preflight (`--release`) only for release-facing changes;
- native platform CI only where native-platform truth is actually required;
- direct local operational smoke where explicitly required;
- direct documentation inspection for scope and behavior claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier
commit that no longer matches HEAD. Record the implementation SHA and, when a
remote run was used, the exact CI run ID — never `gh run list --limit 1`
provenance.

A closure record must be truthful about what landed. If post-closure review
finds defects, open a narrow corrective plan (next number) rather than
rewriting the closed plan's history; append a short correction note instead.

### A plan does NOT require

- a dedicated qualification workflow;
- a second Windows job or matrix;
- a self-hosted or privileged runner;
- uploaded artifacts, logs, screenshots, or evidence bundles;
- immutable candidate SHAs or repeated green runs;
- crates.io publication, tags, or GitHub Releases.

### Scope discipline

Plans stay bounded. Preserve prior plans' historical records; do not reopen
closed architecture decisions without a separately justified corrective plan.
Corrective plans are legitimate product work, not ceremony — but they close
only the enumerated findings.

## Relationship to other docs

Architecture documents (`architecture/`) capture decisions that several phases
must respect together; plans capture sequencing and acceptance criteria for
one phase at a time. When a plan lands behavior users can see, update
`README.md`, the affected crate README, the matching architecture deep dive,
and the relevant skills in the same pass.
