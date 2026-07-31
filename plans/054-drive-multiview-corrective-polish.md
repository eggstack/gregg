# Phase 54: drive and multi-view corrective polish

Status: completed; corrective closure for the completed Phase 48 roadmap.

## Objective

Correct three narrow defects discovered during post-implementation review of the completed drive-metrics and multi-view work:

1. prevent the normal view from selecting and clipping an online entry when only four rows are available even though the renderer requires five base rows;
2. preserve the protocol distinction between drive enumeration failure (`None`) and successful enumeration with no eligible volumes (`Some([])`) when the native macOS or Windows enumeration API returns its documented failure sentinel;
3. reconcile stale plan-index wording that still calls the completed Phase 48 roadmap active.

This is a bounded correctness and documentation pass. It must not reopen the drive protocol, collector architecture, TUI design, CI matrix, release process, or product scope.

## Background and verified defects

### Normal-view four-row blank state

The normal renderer requires five rows for an online system: header, CPU, memory, swap/commit, and disk. The current viewport guard rejects heights below four rather than below five. At exactly four available rows, `visible_range` includes the online entry, `compute_viewport` clips its rectangle to four rows, and `render_online` returns without drawing because its minimum is five rows.

The corrective behavior is:

- a normal-view online base block is not considered renderable below five rows;
- offline and pending one-row entries remain renderable in one row;
- a selected expanded online entry may still be included when at least five rows exist, with only as many detail rows as fit;
- condensed entries remain renderable with one content row after their existing two-row header.

### macOS enumeration failure mislabeled as successful empty enumeration

The native macOS `getmntinfo` API uses a zero return to indicate failure. The current wrapper accepts zero as an empty successful mount table. That converts a top-level enumeration failure into `Some([])` and loses the intentional unavailable-versus-empty distinction.

The wrapper must reject a zero return before constructing the owned mount list. A successful positive count still requires a non-null pointer. No retry loop, fallback command, or additional mount API is required.

### Windows enumeration failure mislabeled as successful empty enumeration

The native Windows `GetLogicalDriveStringsW` API uses a zero return to indicate failure. The current wrapper can accept zero as an empty successful drive list on either the initial call or the resized retry call.

The wrapper must reject zero immediately after each native call. Existing buffer-resize handling remains otherwise unchanged. No additional Windows dependency or alternate enumeration API is required.

### Plan index calls a completed roadmap active

`plans/048-drive-metrics-and-multiview-tui-roadmap.md` and Phases 49 through 53 are complete. The plan index table reflects that, but introductory and historical wording still calls Phase 48 active. Phase 54 is the sole open corrective plan and must be described as such.

## Governing constraints

1. Do not change the v1 or v2 JSON schema.
2. Do not change `StatusPayloadV2`, `DriveMetrics`, drive bounds, or client aggregation semantics.
3. Do not add a v3 route or a separate drive endpoint.
4. Do not add collector tasks, mount watchers, refresh intervals, retries, caches, configuration, or external-command fallbacks.
5. Do not add a dependency or replace the contained native FFI seams.
6. Do not redesign viewport/layout state or introduce a generic widget/table framework.
7. Do not alter view controls, visual design, fleet ordering, or expansion semantics.
8. Do not add or modify GitHub Actions workflows.
9. Do not create evidence bundles, qualification scripts, release gates, or automated publication.
10. Keep implementation changes within the files named by this plan unless a directly required test fixture or comment must be adjusted.

## Expected file footprint

Primary implementation files:

```text
crates/gregg/src/state.rs
crates/gregg/src/ui/layout.rs                 # only if a comment/test needs alignment
crates/greggd/src/collector/macos/ffi.rs
crates/greggd/src/collector/windows/source.rs
plans/README.md
```

Expected tests remain colocated in the existing modules. No new production module is expected.

`AGENTS.md`, protocol documentation, public READMEs, manifests, lockfiles, server code, sampler code, and CI workflows should remain unchanged unless implementation proves a factual statement is now inaccurate. Such a change must be narrowly justified in the commit.

## Workstream A: make minimum render height authoritative

### Required implementation

Refine `visible_range` so its decision for the first candidate entry is based on that entry's minimum renderable base height rather than a global `has_online` heuristic.

A small private helper is acceptable, for example:

```rust
fn minimum_render_height(state: &AppState, system_index: usize) -> u16 {
    match (state.view_mode, state.systems[system_index].reachability) {
        (ViewMode::Normal, Reachability::Online) => 5,
        _ => 1,
    }
}
```

Equivalent code is acceptable if it remains local and obvious.

The iteration contract should be:

- if `height == 0`, return an empty range;
- for the first candidate, return an empty range when `height` is below that entry's minimum render height;
- when the first entry's full dynamic height exceeds the viewport but its base is renderable, include it and allow layout to clip only detail rows;
- for later entries, stop before including an entry whose full height would exceed the remaining viewport;
- never partially render the five-row online base block.

Do not solve this by changing `render_online` to draw partial base rows. Do not reserve a global five rows when the top candidate is offline or pending.

### Required focused tests

Add or adjust state/layout tests proving all of the following:

1. Normal view, online first entry, height `4` -> empty visible range.
2. Normal view, online first entry, height `5` -> exactly that entry is visible.
3. Normal view, offline or pending first entry, height `1` -> the entry is visible even if a later system is online.
4. Normal view, selected expanded online entry with base plus multiple drive rows, height `5` -> the base entry remains visible and zero detail rows are allocated.
5. Normal view, selected expanded online entry, height `6` -> exactly one valid detail row can be allocated.
6. Condensed view behavior at its existing minimum content height is unchanged.
7. Selection-following viewport tests continue to pass for online-first/offline-last reordering.

Prefer testing `visible_range`, `entry_height`, and `compute_viewport` directly. One Ratatui buffer regression asserting that a four-row normal area does not claim or draw a clipped online block is sufficient; do not multiply snapshot tests.

## Workstream B: correct macOS failure-sentinel handling

### Required implementation

In the contained macOS FFI wrapper:

- treat `getmntinfo` return `0` as `CollectErrorKind::SourceUnavailable`;
- retain the positive-count/non-null-pointer validation;
- copy successful records into owned Rust values exactly as before;
- retain invalid UTF-8 and numeric validation behavior;
- return no partially constructed list after a top-level API failure.

A tiny private pure helper for interpreting the returned count/pointer state is acceptable if it materially improves testability. Do not create a generalized FFI result framework.

### Required focused tests

Use the existing macOS-native test job and existing mock seam. Add only the tests needed to prove:

1. zero native count is classified as top-level enumeration failure;
2. positive count with null pointer remains rejected;
3. the collector maps top-level enumeration failure to `drives: None` while preserving an otherwise successful CPU/memory/swap sample;
4. a successful empty set produced after valid enumeration/filtering remains `Some([])`.

The test for the raw zero sentinel may be attached to a small private helper and run only on macOS. Do not attempt to monkey-patch libc symbols or add an FFI mocking library.

## Workstream C: correct Windows failure-sentinel handling

### Required implementation

In the contained Windows source wrapper:

- after the initial `GetLogicalDriveStringsW` call, return `SourceUnavailable` when the result is `0`;
- if a resized retry is required, also reject `0` from the retry;
- retain existing insufficient-buffer handling and UTF-16 parsing;
- retain fixed/removable filtering and per-drive best-effort `GetDiskFreeSpaceExW` skipping;
- preserve deterministic sorting and deduplication.

A small private length-result helper is acceptable for testing. Do not add `windows-sys` features, another Windows crate, or an alternate logical-volume API.

### Required focused tests

Using the existing Windows-native test job and mock source, prove:

1. zero return from the initial length call is an enumeration failure;
2. zero return from the resized retry is an enumeration failure;
3. the collector maps top-level enumeration failure to `drives: None` while preserving core metrics;
4. valid enumeration whose entries are all filtered or unready remains `Some([])`;
5. existing fixed/removable inclusion and network/optical/RAM exclusion tests remain green.

Do not add tests that assert a hosted runner's exact drive count, names, capacity, or topology.

## Workstream D: reconcile planning status

Update `plans/README.md` so that:

- Phase 48 is described as a completed product roadmap;
- Phases 49 through 53 remain completed;
- Phase 54 is registered as the sole remaining corrective polish pass;
- the historical Phase 000 row no longer calls Phase 48 active;
- completion of Phase 54 can later be recorded with one status-line/table update.

Do not reopen or rewrite the completed roadmap and phase files. Do not add a new umbrella roadmap for three corrections.

## Verification strategy

Verification must remain within Gregg's existing lightweight model.

### During implementation

Run narrow tests while editing:

```text
cargo test -p gregg state
cargo test -p gregg ui
cargo test -p greggd collector
```

Test-name filters may be used instead. Platform-specific raw FFI sentinel tests run only on their native platforms through the existing jobs.

### Local closure

Run the existing default local check for the development platform:

```text
./scripts/check-local.sh
```

or on Windows:

```text
.\scripts\check-local.ps1
```

Do not run release preflight, package publication, installation rehearsals, soak tests, or repeated CI qualification for this pass.

### Hosted closure

One ordinary existing CI run is sufficient. It must show success for the repository's existing Linux, macOS, Windows, and MSRV jobs. No new matrix entries or artifacts are required.

## Acceptance criteria

Phase 54 is complete only when all of the following are true:

### Viewport correctness

- [ ] A normal-view online system is not included when fewer than five content rows are available.
- [ ] A normal-view online system is included at exactly five content rows.
- [ ] Offline and pending one-row entries remain visible in one content row regardless of later online entries.
- [ ] Expanded drive details may be clipped, but the five-row online base block is never clipped or represented as a blank selected entry.
- [ ] Condensed view geometry and controls are unchanged.
- [ ] Selection-following and online-first/offline-last viewport behavior remain correct.

### Drive availability semantics

- [ ] macOS `getmntinfo == 0` is treated as top-level enumeration failure.
- [ ] Windows `GetLogicalDriveStringsW == 0` is treated as top-level enumeration failure on both initial and retry calls.
- [ ] Top-level enumeration failure produces `drives: None` without failing core host metrics.
- [ ] Successful enumeration with no eligible volumes still produces `drives: Some(vec![])`.
- [ ] Individual inaccessible or unready volumes remain skippable without converting the whole sample to failure.
- [ ] No wire-format or protocol-validation change is made.

### Scope and footprint

- [ ] No new dependency is added.
- [ ] No new route, task, watcher, cache, cadence, configuration option, or command is added.
- [ ] No workflow, release automation, evidence mechanism, or additional CI gate is added.
- [ ] Changes remain in the expected narrow files, with any exception explained in the implementation commit.
- [ ] Plan index wording accurately distinguishes completed Roadmap 048 from open corrective Phase 54.

### Verification and closure

- [ ] Focused regression tests cover the four-row/five-row boundary and both native zero-return sentinels.
- [ ] Existing drive protocol, collector, mixed-fleet, and TUI tests remain green.
- [ ] The existing local check passes.
- [ ] One ordinary existing CI run passes across Linux, macOS, Windows, and MSRV.
- [ ] No release, tag, GitHub Release, or crates.io publication is performed.

## Handoff sequence

A smaller implementation model should execute in this order:

1. Add the failing viewport boundary tests.
2. Correct `visible_range` with a local minimum-base-height rule.
3. Run focused `gregg` state/layout/UI tests.
4. Add the macOS zero-return helper/test and correct the wrapper.
5. Confirm macOS collector failure maps to `None` and successful filtering can map to `Some([])`.
6. Add the Windows zero-return helper/tests and correct both native-call paths.
7. Confirm Windows collector failure maps to `None` and successful filtering can map to `Some([])`.
8. Update `plans/README.md` status wording.
9. Run the existing local check.
10. Push one coherent implementation commit and allow one ordinary CI run.
11. After CI succeeds, mark Phase 54 completed in this file and the plan index with the implementation SHA and CI run number.

## Stop conditions

Stop and record a separate future request rather than expanding Phase 54 if implementation appears to require any of the following:

- a protocol revision;
- physical-disk or APFS-container modeling;
- a new mount-enumeration architecture;
- per-volume retry/backoff state;
- new TUI behavior or layout redesign;
- additional platform support;
- new dependencies or workflows;
- release-process changes.

Those are not required to correct the verified defects in this phase.
