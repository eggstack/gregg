# Phase 084: Plan 083 corrective closure

Status: complete.

Depends on: Plan 083 implementation `4519f8d6e26fb3222c52c9759f479338b3a26b46` and closure record `13456c6f34146637737f42887c3b91750cfa2ce2`.

Implementation: `020188f2720510d762ec20b0cb77a9f52ed6ff10`.
Verification: focused client tests, the default local check, exact Linux CI
clippy/tests, Rust 1.75 compilation, and CI run `32100189772` all passed.

## Objective

Close the small set of concrete issues found in post-implementation review of Plan 083 without reopening its client architecture or broadening Gregg's scope.

Plan 083's main product behavior is retained. The shared metric-row geometry, compact disk text, first-batch viewport snap, explicit-port `gregg add`, `nickname@host:port`, named/unnamed offline rendering, and fixed-cadence offline polling all remain the intended design.

This corrective phase is limited to four items:

1. restore the pre-Plan-083 validation semantics for the existing `gregg add --name` path;
2. add renderer-level geometry tests that actually demonstrate Plan 083's bracket/indent/width claims through the Ratatui `TestBackend`;
3. make offline-row padding use terminal display width rather than UTF-8 byte length;
4. correct stale `default_port` comments/documentation so they describe the post-Plan-083 behavior truthfully.

Do not redesign the UI, endpoint parser, scheduler, daemon, protocol, configuration schema, CI, or release process.

## Baseline findings

### 1. `--name` lost the stricter endpoint-name validation path

Before Plan 083, `cmd_add()` explicitly ran:

```rust
crate::endpoint::validate_name(name)?;
```

for the traditional `--name` argument before mutating configuration.

Plan 083 introduced `parse_add_target()` and correctly validates the inline nickname in:

```text
nickname@host:port
```

through the existing `validate_name()` helper, but the refactored `cmd_add()` no longer performs the same validation for the standalone `--name` source before selecting `final_name`.

This creates asymmetric behavior between these equivalent user-facing forms:

```text
gregg add deadpool@192.168.182.146:11310
gregg add 192.168.182.146:11310 --name deadpool
```

The asymmetry is real because `endpoint::validate_name()` rejects control characters, while `Config::validate()` currently checks configured system names only for empty/trimmed length constraints. A control-character-containing `--name` can therefore reach config mutation through a path that was rejected before Plan 083.

The correction is intentionally small: after resolving the single final name source, run the existing endpoint-name validator once before mutating configuration.

Do not create a second name validator or widen config-schema behavior unless implementation inspection proves that is necessary.

### 2. Plan 083's layout code is plausible, but its closure record overstates renderer-level proof

Plan 083 required normal-view geometry to be demonstrated through the existing Ratatui `TestBackend` at representative widths. The implementation added useful helper-level tests around `MetricGroupLayout`, suffix compaction, unavailable rows, and truncation, but it did not fully add the requested renderer-level matrix.

The current closure checklist marks all of the following as demonstrated:

- exactly four leading spaces on CPU/MEM/SWP-or-COMMIT/DISK rows;
- identical opening `[` terminal columns;
- identical closing `]` terminal columns;
- Windows `COMMIT` participation in that alignment;
- unavailable DISK using the same geometry;
- no row exceeding narrow terminal widths.

Those claims should be backed by tests against the final rendered terminal buffer, not only the layout helper.

Plan 084 must add that missing proof; it does not need another layout refactor if the tests confirm the current implementation.

### 3. Offline dotted padding is byte-counted

`render_offline()` currently derives the fill count from something equivalent to:

```rust
let used = prefix_with_status.len();
let dot_count = total_width.saturating_sub(used);
```

`String::len()` is UTF-8 byte length, not terminal-cell width. A configured Unicode nickname can therefore produce too few or otherwise visually incorrect trailing dots.

Plan 083 already moved metric geometry toward `unicode_width`; offline status padding should use the same display-cell semantics.

The narrow correction is to measure `prefix_with_status` with `UnicodeWidthStr::width()` (or the existing equivalent helper) before computing the dot count.

Do not add a new Unicode/layout dependency.

### 4. `default_port` documentation no longer matches product behavior

After Plan 083, `gregg add` requires an explicit port and does not consume `config.default_port` for system additions.

Current comments still imply that `default_port` fills omitted ports on old/legacy `systems` entries. That is not accurate for the current `SystemEntry` schema because `port: u16` is required on deserialization and has no serde default.

Keep `default_port` in the schema for compatibility unless a separately justified schema-removal phase is created. This phase should only make the current semantics clear.

Preferred wording:

```text
default_port is retained for configuration compatibility but is not used by
`gregg add`, which requires an explicit port.
```

If source inspection identifies a remaining legitimate runtime consumer, document that exact consumer instead of making a broader claim.

Do not delete the field in Plan 084.

## Authoritative behavior after Plan 084

### Name validation

Both user-facing naming forms must pass through the same `endpoint::validate_name()` semantics:

```text
gregg add nickname@host:11310
gregg add host:11310 --name nickname
```

Valid names remain accepted.

Invalid names remain rejected consistently, including at minimum:

- empty/whitespace-only names;
- names exceeding `MAX_ENDPOINT_NAME_LEN`;
- names containing control characters.

Supplying both inline nickname and `--name` remains an ambiguity error.

No configuration schema change is introduced.

### Metric rendering

For every normal-view online system block, the final Ratatui-rendered CPU/MEM/SWP-or-COMMIT/DISK rows must retain the Plan 083 rules:

```text
    CPU  [....................] 25.2% ...
    MEM  [....................] 37.8% ...
    SWP  [....................]  0.0% ...
    DISK [....................] 60.4% ...
```

or on Windows:

```text
    CPU    [..................] ...
    MEM    [..................] ...
    COMMIT [..................] ...
    DISK   [..................] ...
```

The exact number of cells varies with terminal width. The invariant is column alignment, not a hard-coded bar length.

### Offline rendering

Named offline system:

```text
deadpool@192.168.182.146:11310 offline .....
```

Unnamed offline system:

```text
192.168.182.146:11310 offline ...............
```

Unicode configured names must use terminal display-cell width when calculating trailing dot fill.

### `default_port`

The field remains present for configuration compatibility. User-facing and source comments must not claim that current `gregg add` or current required `SystemEntry.port` deserialization uses it to fill an omitted system port unless source inspection demonstrates a real remaining consumer.

## Implementation sequence

### Step 1: restore one final-name validation point

Primary file:

```text
crates/gregg/src/cli.rs
```

Keep the existing parsing order:

1. parse optional inline nickname plus endpoint;
2. reject inline nickname + `--name` ambiguity;
3. enforce explicit port;
4. resolve the single final name source;
5. validate the resolved name with `endpoint::validate_name()` if present;
6. mutate configuration.

Preferred shape:

```rust
let final_name = target
    .name
    .clone()
    .or_else(|| name.map(str::to_owned));

if let Some(final_name) = final_name.as_deref() {
    crate::endpoint::validate_name(final_name)?;
}
```

The exact implementation may differ, but validation must occur before config mutation.

Do not duplicate validation branches for inline and flag forms unless doing so is demonstrably smaller.

### Step 2: add focused parity tests for inline and `--name`

Primary file:

```text
crates/gregg/src/cli.rs
```

Add deterministic tests proving both forms use the same validation semantics.

Required cases:

```text
valid inline nickname -> accepted
valid --name -> accepted
control-character inline nickname -> rejected
control-character --name -> rejected
overlong inline nickname -> rejected
overlong --name -> rejected
empty/whitespace-only inline nickname -> rejected
empty/whitespace-only --name -> rejected
inline nickname + --name -> ambiguous and no config mutation
```

Where Clap makes an empty command-line value awkward to construct, test `cmd_add()` directly.

Also assert failed validation leaves the config unchanged.

Do not broaden this into a new CLI integration harness.

### Step 3: add renderer-level normal-view geometry regression tests

Primary file:

```text
crates/gregg/src/ui/mod.rs
```

Use the existing `TestBackend`/`render_state()` helpers.

Test representative supported widths:

```text
24
32
40
60
80
```

If one of those widths cannot meaningfully show brackets under the intentionally compact degradation path, assert the exact intended fallback there and use the next width where bars exist. Do not force brackets into widths where the product intentionally omits them.

For Linux-shaped rows where bars are present, inspect final rendered rows and assert:

- CPU, MEM, SWP, DISK begin with exactly four spaces;
- all four `[` characters occupy the same terminal column;
- all four `]` characters occupy the same terminal column;
- DISK does not contain the words `used` or `avail`;
- each rendered metric line is at most the backend width in terminal cells;
- percentages survive narrow-width compaction before optional details.

For Windows-shaped rows, assert:

- the third row is `COMMIT`;
- CPU/MEM/COMMIT/DISK opening brackets share one terminal column;
- CPU/MEM/COMMIT/DISK closing brackets share one terminal column;
- the six-character `COMMIT` label does not shift the bracket geometry.

For unavailable drive data, assert:

- DISK uses the truthful unavailable marker;
- DISK does not fabricate `0.0%`;
- where a bar is rendered, its brackets occupy the same columns as the other rows.

Use terminal/display-cell column logic where Unicode could matter. Do not rely on byte index as the proof of terminal alignment.

Retain the existing helper-level tests; they are useful and do not need replacement.

### Step 4: correct offline padding width

Primary file:

```text
crates/gregg/src/ui/system_block.rs
```

Replace UTF-8 byte-count padding with terminal-cell width.

Preferred shape:

```rust
let used = UnicodeWidthStr::width(prefix_with_status.as_str());
let dot_count = usize::from(area.width).saturating_sub(used);
```

or an equivalent existing width helper.

Add renderer coverage with at least one non-ASCII configured name whose UTF-8 byte length differs from terminal display width.

The test should demonstrate that the rendered offline line fills the available width without overflow or under-fill attributable to byte-counting.

Do not alter the `name@host:port offline` / `host:port offline` content model.

### Step 5: reconcile `default_port` source and user documentation

Inspect all current non-historical references before editing.

Expected current files include:

```text
crates/gregg/src/config.rs
crates/gregg/config.example.toml
README.md
crates/gregg/README.md
AGENTS.md
architecture/gregg-client.md
.opencode/skills/gregg-client/SKILL.md
```

Only change files that actually make an inaccurate current-semantics claim.

Requirements:

- keep the field in `Config`;
- do not reintroduce implicit-port `gregg add` behavior;
- do not claim missing `SystemEntry.port` fields are auto-filled if they are not;
- describe `default_port` as compatibility/reserved configuration state unless a real current consumer exists;
- correct the stale `SystemEntry`/`port_was_explicit` comment in `config.rs` so it no longer says `gregg add` resolves omitted ports through `default_port`.

Historical completed plans do not need rewriting merely because they describe pre-Plan-083 behavior accurately for their time.

### Step 6: correct Plan 083 closure wording rather than erasing history

Plan 083's implementation and CI record remain valid historical facts. Do not rewrite the successful implementation SHA or CI run.

Update Plan 083 only enough to state that post-closure review found four narrow follow-up issues and that Plan 084 is the corrective phase.

Do not uncheck every completed Plan 083 criterion. Instead distinguish:

- product behavior that was implemented and remains valid;
- renderer-level proof that was insufficiently demonstrated at closure;
- the concrete `--name` validation regression;
- the Unicode padding and documentation corrections.

Update `plans/README.md` during implementation/closure to add Plan 084 and describe Plan 083 as implemented with corrective follow-up 084 active, then complete once Plan 084's criteria pass.

Do not create Plan 085 solely to record closure.

## Expected production-code surface

Primary:

```text
crates/gregg/src/cli.rs
crates/gregg/src/ui/system_block.rs
```

Tests / renderer proof:

```text
crates/gregg/src/ui/mod.rs
```

Documentation/comments as needed:

```text
crates/gregg/src/config.rs
crates/gregg/config.example.toml
README.md
crates/gregg/README.md
AGENTS.md
architecture/gregg-client.md
.opencode/skills/gregg-client/SKILL.md
plans/083-compact-tui-endpoint-nicknames-and-polling-invariant.md
plans/README.md
this plan
```

Files that should not require product changes:

```text
crates/greggd/**
crates/gregg-protocol/**
crates/gregg/src/scheduler.rs
crates/gregg/src/state.rs
crates/gregg/src/endpoint.rs
.github/workflows/**
scripts/**
packaging/**
```

If implementation starts changing those areas, first establish a concrete defect from this phase that requires it.

## Explicit non-goals

Do not add or redesign any of the following:

- nickname schema or aliases;
- endpoint discovery;
- daemon-reported name synchronization;
- implicit ports on `gregg add`;
- removal of `default_port` from the schema;
- HTTPS support;
- polling backoff or scheduler changes;
- viewport/selection behavior;
- bar-layout architecture beyond fixes required by failing renderer tests;
- condensed-view redesign;
- new terminal/screenshot infrastructure;
- new dependencies;
- new CI workflows/jobs/matrices;
- release automation;
- unrelated cleanup.

## Focused verification

Run the smallest useful checks first. Use exact test names/modules produced by the implementation.

Expected commands:

```bash
cargo fmt --all -- --check
cargo test -p gregg cli
cargo test -p gregg ui
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

Additional CI-equivalent checks completed:

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
RUSTFLAGS=-Dwarnings cargo +1.75 check --workspace --all-features
```

The MSRV check also required removing a Rust-1.75-incompatible lint-reason
attribute from the existing renderer; this was a behavior-neutral syntax
correction and did not broaden the product scope.

If the test filter layout differs, run the nearest exact tests and record the actual commands.

A release preflight is not required for this corrective client-only phase.

No new CI obligation is required. If the existing workflow runs naturally on push, it should remain green, but local deterministic tests are sufficient to establish the corrections.

## Acceptance criteria

### `--name` validation parity

- [x] `gregg add host:port --name NAME` validates `NAME` with the same `endpoint::validate_name()` semantics as inline `nickname@host:port`.
- [x] Valid `--name` remains accepted and persisted through `SystemEntry.name`.
- [x] Control-character `--name` is rejected before config mutation.
- [x] Overlong `--name` is rejected before config mutation.
- [x] Empty/whitespace-only `--name` is rejected before config mutation.
- [x] Equivalent invalid inline nicknames remain rejected.
- [x] Inline nickname + `--name` remains an ambiguity error.
- [x] Failed name validation leaves the existing config unchanged.

### Renderer-level geometry proof

- [x] Final Ratatui-rendered Linux CPU/MEM/SWP/DISK rows begin with exactly four spaces wherever normal metric rows are rendered.
- [x] At representative widths where bars are present, final rendered Linux rows place `[` at one shared terminal column.
- [x] At representative widths where bars are present, final rendered Linux rows place `]` at one shared terminal column.
- [x] Final rendered Windows CPU/MEM/COMMIT/DISK rows use the same opening/closing bracket columns.
- [x] The `COMMIT` label does not shift the metric geometry.
- [x] Unavailable DISK remains truthful and does not fabricate `0.0%`.
- [x] Narrow-width rendering preserves percentage before optional detail.
- [x] No tested metric row exceeds the terminal's display-cell width.
- [x] Existing helper-level layout/truncation tests remain green.

### Offline display width

- [x] Offline dot padding is calculated from terminal display width, not UTF-8 byte length.
- [x] Named ASCII offline rendering remains `name@host:port offline`.
- [x] Unnamed offline rendering remains `host:port offline`.
- [x] A Unicode nickname regression test demonstrates correct bounded padding.
- [x] No new Unicode/layout dependency is added.

### `default_port` documentation truth

- [x] `default_port` remains in the configuration schema.
- [x] Current comments no longer claim `gregg add` uses `default_port` for omitted ports.
- [x] Current comments no longer claim a missing required `SystemEntry.port` is auto-filled unless source inspection proves that behavior exists.
- [x] User-facing examples continue to require explicit ports for `gregg add`.
- [x] Historical plans are not rewritten solely to remove accurate historical implicit-port behavior.

### Scope and closure

- [x] No scheduler, daemon, protocol, state/viewport, or service-runtime product behavior is changed.
- [x] No new dependency is added.
- [x] No new CI workflow/job/matrix/evidence system is added.
- [x] Focused client/UI tests pass.
- [x] `cargo fmt --all -- --check` passes.
- [x] `./scripts/check-local.sh` passes.
- [x] Plan 083 records the existence of this corrective follow-up without erasing its valid implementation/CI history.
- [x] `plans/README.md` identifies Plan 084 accurately.
- [x] Plan 084 records the implementation SHA and checks actually run when complete.

## Completion rule

Plan 084 is complete when the four review findings are corrected and demonstrated by deterministic local tests:

```text
--name and inline nickname validation parity
renderer-level bracket/indent/width proof
Unicode-aware offline padding
truthful default_port documentation
```

The default local check must pass.

A new follow-up plan is warranted only if implementation or review finds another concrete product defect. Do not create a closure-only Plan 085.
