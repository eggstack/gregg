# Phase 53: drive and multi-view integration with lightweight closure

Status: implementation complete; hosted CI confirmation pending.

## Objective

Integrate Phases 49 through 52 into one coherent release-ready product change, reconcile active documentation and examples, and close the roadmap using Gregg's existing local checks and ordinary cross-platform CI.

This phase is not a second implementation pass and must not create new validation infrastructure. It exists to catch boundary mismatches between protocol, collectors, normalization, state/layout, and both renderers while preserving the project's intentionally small verification model.

## Dependencies and execution position

Depends on completion of:

- Phase 49 additive v2 drive protocol and normalization;
- Phase 50 native Linux/macOS/Windows collection;
- Phase 51 normal-view viewport/disk rendering;
- Phase 52 condensed view and controls.

No later phase is planned for this roadmap. Genuine defects found here must be fixed narrowly in their owning module. New product requests are recorded separately.

## Governing invariants

1. Closure verifies the requested feature, not unrelated repository perfection.
2. Existing default local checks and ordinary CI are sufficient.
3. No evidence bundle, special qualification workflow, repeated CI run, or manual platform record is required.
4. No release publication, tag, or GitHub Release occurs as part of implementation closure.
5. V1 remains unchanged and old v2 payloads remain compatible.
6. Drive collection remains best-effort and cannot take down core metrics.
7. Both TUI views consume the same normalized snapshot and state reducer.
8. Requested key behavior and selected-system expansion are identical across supported terminals.
9. Documentation describes mounted-local-filesystem semantics, not physical-device semantics.
10. Scope remains limited to the roadmap.

## Scope

### In scope

- cross-crate compatibility review;
- API fixture and endpoint smoke reconciliation;
- mixed old/new daemon/client behavior tests;
- cross-platform structural collector tests through existing jobs;
- mixed-fleet normal/condensed TUI tests;
- response-size and body-cap check;
- documentation and examples;
- changelog/unreleased entry;
- plan-index status handoff guidance;
- one default local check and ordinary CI.

### Out of scope

- new feature implementation beyond required defect correction;
- physical-disk/storage-topology features;
- test framework rewrites;
- broad performance benchmarking;
- long soak tests;
- package publishing or version bump unless maintainers separately schedule a release;
- new workflows, artifacts, evidence manifests, or release automation;
- package-manager distribution;
- LAN discovery, authentication, TLS, history, or alerts.

## Workstream A: cross-crate contract reconciliation

Review the full data path:

```text
native OS source
  -> platform collector
  -> CollectedMetrics.drives
  -> cached v2 status payload
  -> HTTP JSON
  -> client poll/validation
  -> NormalizedSnapshot.drives
  -> aggregate helper
  -> AppState
  -> normal/condensed renderers
```

For each boundary, verify:

- `None`, empty, and populated drive states retain their intended meanings;
- names and order are stable;
- numeric invariants are preserved;
- no aggregate value is recomputed differently by different renderers;
- drive-only failure remains nonfatal;
- response validation occurs before normalization;
- rendering performs no I/O.

Correct only concrete mismatches. Do not introduce a new shared framework solely to make the flow look more uniform.

### Workstream A acceptance criteria

- [ ] One documented semantic meaning exists for every drive state.
- [ ] Both renderers use the same normalized aggregate/detail helpers.
- [ ] No duplicate aggregation logic remains.
- [ ] No collector or renderer bypasses validation/normalization boundaries.

## Workstream B: compatibility matrix

Add or confirm focused automated cases:

| Daemon payload | Client behavior |
| --- | --- |
| v1 Linux/macOS | polls/falls back as before; drives unavailable |
| old v2 without drives | accepted; drives unavailable |
| new v2 with `drives: null`/omitted | accepted; drives unavailable |
| new v2 with empty drives | accepted; no eligible aggregate |
| new v2 with populated drives | accepted; normalized and rendered |
| malformed/invalid drives | rejected as invalid v2; no silent v1 fallback |
| unsupported v2 schema | existing unsupported behavior |

Also confirm:

- upgraded Linux/macOS daemons still serve unchanged v1 responses;
- Windows remains v2-only where existing semantics require it;
- old clients can ignore the additive flat JSON drive field;
- direct public Rust source compatibility matches the Phase 49 decision.

Do not create a matrix runner. Ordinary unit/integration tests are sufficient.

### Workstream B acceptance criteria

- [ ] Old v1 and v2 compatibility is executable in tests.
- [ ] Invalid new drive data is surfaced rather than hidden by fallback.
- [ ] Windows endpoint behavior remains truthful.
- [ ] Source compatibility impact is documented consistently.

## Workstream C: API and native smoke coverage

Use existing daemon server/smoke infrastructure.

Required API assertions for a synthetic populated snapshot:

```text
GET /v2/status succeeds
schema_version == 2
drives is present
name is a string
used_bytes is numeric
total_bytes is numeric
used_bytes <= total_bytes
```

Required unavailable assertion:

```text
/v2/status remains valid when drives are omitted/unavailable
```

V1 assertions:

```text
no drive field added to v1
existing v1 validation/fixtures unchanged
```

Native jobs should assert only structural invariants. They must not require:

- exact mount/drive count;
- exact root name on every runner;
- exact filesystem type;
- exact capacities;
- removable media;
- APFS topology identity.

If a hosted runner returns no eligible drives due to its environment, the platform job may rely on source-mock tests plus successful production helper execution without panic. Do not weaken pure tests to accommodate runner variance.

### Workstream C acceptance criteria

- [ ] Cached v2 API exposes populated synthetic drives.
- [ ] Unavailable drives do not break API readiness.
- [ ] V1 API remains unchanged.
- [ ] Existing Linux/macOS/Windows jobs provide proportionate native confidence.

## Workstream D: mixed-fleet TUI integration tests

Use Ratatui `TestBackend` and existing state/poll fixtures to cover a representative fleet containing:

```text
v1 Linux with no drives
new v2 Linux with multiple drives
new v2 macOS with drives and unsupported I/O-wait
new v2 Windows with drives, unsupported load/I/O-wait, and commit
one offline system
one pending system
```

Required scenarios:

### Normal mode

- several systems render when height permits;
- aggregate disk row appears for new payloads;
- unavailable disk row appears for old payloads;
- Windows still renders COMMIT rather than swap;
- `j`/`k` changes selection and viewport follows;
- selected expansion adds only selected drive rows;
- poll-induced online-first reorder preserves visible selection.

### Condensed mode

- wide columns are semantically correct;
- unsupported values are `—`;
- disk percentage derives from aggregate helper;
- offline/pending do not show stale values;
- fixed width tiers behave as planned;
- expansion adds only selected details;
- view switching preserves selection/expansion.

### Key controls

- `h`/Left and `l`/Right cycle both directions;
- `e` toggles details;
- existing navigation, refresh, quit, first/last, and paging still work.

Avoid giant golden snapshots. Use focused line/substring/style assertions so harmless spacing changes do not cause excessive maintenance.

### Workstream D acceptance criteria

- [ ] One representative mixed-fleet fixture exercises every supported OS and old/new protocol behavior.
- [ ] Both views and all requested controls are covered.
- [ ] Tests remain focused and readable rather than becoming a snapshot suite.

## Workstream E: response bounds and resource sanity

Confirm the Phase 49 maximum drive count/name length fits within the current client body-size cap when serialized with the rest of a v2 snapshot.

Add one deterministic serialization-size test near protocol/poller code:

- construct a maximum-bound valid payload;
- serialize it;
- assert it remains below `MAX_RESPONSE_BYTES` with a small safety margin.

If it does not fit, prefer reducing bounds before increasing the network cap. Any cap change must remain bounded and documented.

Resource sanity review:

- no new thread/task per system or drive;
- no external process;
- no directory traversal;
- one API request per endpoint remains typical;
- one collector sample cadence;
- no warning/error log flood for unavailable removable volumes;
- vectors/strings remain protocol-bounded.

No benchmark suite is required. A code review plus existing tests is sufficient unless a measured regression appears.

### Workstream E acceptance criteria

- [ ] Maximum valid response fits the bounded client read path.
- [ ] No unbounded allocation or new concurrency surface exists.
- [ ] Sampling/polling architecture remains unchanged.

## Workstream F: documentation reconciliation

Update active documentation only.

### Root README

Document:

- v2 per-drive API fields;
- drive means eligible mounted local filesystem;
- aggregate normal-view disk row;
- condensed view example based on `condensed.txt`;
- all requested keys and arrows;
- selected-system `e` behavior;
- unsupported values use `—`;
- LAN/private-network scope remains unchanged.

### Protocol documentation

Document:

- additive v2 representation and old-payload behavior;
- unavailable versus empty versus populated semantics;
- validation bounds;
- source-compatibility wrapper/direct-field decision;
- v1 unchanged.

### Collector/platform documentation

Document concise rules:

- Linux mountinfo/native stat, local eligible mounts, deduplication;
- macOS mounted-volume sum and APFS shared-container caveat;
- Windows fixed/ready removable roots and exclusion of remote/optical/RAM/unready drives;
- collection is best-effort.

### TUI/contributor documentation

Update:

- dynamic normal-entry height;
- condensed one-row base height;
- logical-system scrolling;
- selected-only expansion;
- no persistence.

### Changelog

Add one concise unreleased entry for drive metrics and multi-view TUI. Do not declare a release version or date unless separately authorized.

Do not rewrite historical completed plans. Update Plans 48-53 status in `plans/README.md` only when implementation actually lands.

### Workstream F acceptance criteria

- [ ] Public docs accurately describe behavior and limitations.
- [ ] APFS aggregate wording does not imply unique physical capacity.
- [ ] No documentation advertises physical disk/SMART/history/configurable tables.
- [ ] Changelog remains unreleased/version-neutral.

## Workstream G: lightweight final verification

Use the existing repository model.

During corrective integration, run focused crate checks as needed. Before declaring Phase 53 complete, run:

```text
./scripts/check-local.sh
```

On Windows, the equivalent existing command is:

```text
.\scripts\check-local.ps1
```

One supported local host is sufficient for the local check. Push the final implementation and require the ordinary existing CI workflow to pass its Linux, macOS, Windows, and MSRV jobs according to current repository policy.

Do not require:

- `--release` preflight unless maintainers are preparing an actual release;
- package publish dry-runs;
- a clean release tree solely for implementation closure;
- repeated CI runs;
- exact candidate SHA documents;
- screenshots;
- evidence files;
- manually attached native-host reports.

A single ordinary green CI run at the final implementation SHA is sufficient hosted proof.

### Workstream G acceptance criteria

- [ ] Existing default local validation passes.
- [ ] Ordinary CI passes at the final implementation SHA.
- [ ] No new workflow, artifact, or evidence system was added.
- [ ] No release operation was performed as part of closure.

## Workstream H: scope audit

Before closure, inspect the diff and repository search for accidental expansion.

The implementation must not introduce:

```text
SMART
physical disk inventory
filesystem configuration filters
historical disk samples
alerts/thresholds
new HTTP routes
protocol v3
mount watcher
storage cache service
horizontal scrolling
configurable columns
third view
mouse support
new CI workflow
artifact upload
publish automation
```

References in explicit out-of-scope documentation are acceptable. Product code or active claims are not.

Also verify:

- no shell command execution for collection;
- no drive state persisted to config;
- no renderer I/O;
- no OS-name branching where capability/normalized values suffice.

### Workstream H acceptance criteria

- [ ] Diff is limited to the roadmap's contract.
- [ ] No accidental future feature scaffolding remains.
- [ ] Any discovered nonblocking enhancement is recorded outside this roadmap.

## Expected files

This phase should mostly update tests/docs and narrow defects in files already changed by Phases 49-52:

```text
crates/gregg-protocol tests/fixtures/rustdoc
crates/greggd server/sampler/collector integration tests
crates/gregg mixed-fleet/state/ui tests
README.md
crates/*/README.md as applicable
architecture/protocol.md
architecture/macos-collector-notes.md or focused platform notes
AGENTS.md
CHANGELOG.md
plans/README.md after implementation completion
plans/048-053 status headers after implementation completion
```

Do not create an `evidence/`, `artifacts/`, or new test-orchestration directory.

## Implementation sequence

1. Review the end-to-end semantic path and fix concrete mismatches.
2. Complete compatibility matrix tests.
3. Complete synthetic API and native structural tests.
4. Add one representative mixed-fleet TUI integration fixture.
5. Add bounded maximum-response serialization test.
6. Reconcile README/protocol/platform/TUI/changelog documentation.
7. Perform scope audit.
8. Run focused checks for any correction.
9. Run existing default local validation.
10. Push final implementation and use one ordinary CI run.
11. Mark Plans 48-53 complete only after product criteria and green CI are satisfied.

## Phase acceptance criteria

Phase 53 is complete only when:

- [ ] Phases 49 through 52 are implemented and their criteria are satisfied.
- [ ] The end-to-end drive state semantics are consistent from native source through both renderers.
- [ ] V1 remains unchanged.
- [ ] Old v2 without drives remains accepted.
- [ ] Invalid drive data is rejected without silent fallback.
- [ ] Linux, macOS, and Windows native collection is structurally covered by existing jobs.
- [ ] A mixed old/new, cross-platform fleet is tested in normal and condensed views.
- [ ] All requested keys and selected-only expansion behavior are tested.
- [ ] Maximum valid v2 payload remains within the bounded client response path.
- [ ] Public documentation and changelog are accurate and scope-limited.
- [ ] Existing default local validation passes.
- [ ] One ordinary cross-platform CI run passes at the final implementation SHA.
- [ ] No new workflow, evidence artifact, release automation, or release operation was added.
- [ ] Scope audit finds no physical-disk, history, alerting, configurable-table, discovery, or unrelated monitoring expansion.

## Handoff guidance for a smaller implementation model

- Treat this as integration/closure, not permission to redesign earlier phases.
- Fix defects in the module that owns them; do not add a cross-cutting abstraction unless correctness requires it.
- Keep compatibility tests table-driven and TUI tests focused.
- Use existing local/CI commands exactly as documented.
- Do not produce screenshots or evidence files.
- Do not mark plans complete until the implementation SHA has one ordinary green CI run.
- Record optional enhancements separately and stop.
