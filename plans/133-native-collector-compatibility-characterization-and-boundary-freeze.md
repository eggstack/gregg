# Plan 133: native collector compatibility characterization and boundary freeze

Status: complete.

Depends on: Plan 132 and the current post-Plan-131 collector/runtime baseline.

Blocks: Plans 134-135.

## Objective

Freeze and characterize the current `greggd` native collector contract before extraction so later source movement can be demonstrated as behavior-preserving rather than inferred from compilation.

This plan is verification and boundary preparation. It must not redesign collection, change the wire model, move clock ownership, alter cadence, replace the drive worker, change platform metric definitions, or introduce BSD implementation.

## Compatibility inventory

Document the current externally or cross-module observable surface that must survive the extraction.

At minimum inventory:

- `greggd::collector::SystemCollector`;
- `greggd::collector::CollectedMetrics`;
- `greggd::collector::error::{CollectError, CollectErrorKind}`;
- `greggd::collector::linux::*`, including `ProcSource`, `FileSource`, and `MemorySource`;
- `greggd::collector::macos::*`, including `MacOsCollector`, native query seam, raw records, and mocks;
- `greggd::collector::windows::*`, including `WindowsCollector`, `WindowsSource`, raw records, and mocks;
- collector conversion methods used by the sampler;
- v1/v2 capability and `supports_v1_snapshot()` behavior;
- sampler readiness interpretation of Warming, CounterReset, and hard failures.

Record which paths are intentionally public and therefore need a compatibility facade during Plan 135, even if some could eventually be deprecated.

## Sequence characterization

Existing arithmetic unit tests are necessary but not sufficient. Add deterministic sequence tests around stateful behavior.

### CPU lifecycle

For each supported platform source seam, prove:

1. construction succeeds with stable identity;
2. first sample returns `Warming`;
3. second valid observation yields the expected CPU percentage;
4. a cumulative counter decrease/reset produces the current `CounterReset` behavior;
5. the reset establishes a fresh baseline;
6. the next valid interval recovers without a synthetic spike.

Linux characterization must also preserve logical-core hotplug refresh semantics.

### Disk/network cumulative rates

Using injected/synthetic source records:

- first observation establishes a baseline and emits no rate payload;
- the next observation uses actual monotonic elapsed time;
- identity disappearance removes its baseline;
- identity reappearance warms rather than inheriting stale counters;
- counter decrease/wrap re-baselines rather than spiking;
- source failure clears the affected live baseline and does not fail core CPU/memory sampling;
- loopback/detail and aggregate-member semantics remain unchanged;
- existing capacity aggregation behavior remains unchanged.

Do not move or inject the clock in this plan. Characterize the current behavior at the existing boundary.

### Drive refresh semantics

Add/retain deterministic proof that:

- the worker's first request is immediate;
- core sampling never waits for drive enumeration;
- `None` is returned before the first successful refresh;
- successful empty enumeration is `Some(empty)`;
- a later refresh failure retains the last successful list;
- a contained panic is isolated/retried under the current bounded policy;
- dropping the collector/cache does not wait for a blocked filesystem query;
- at most the existing worker behavior is used; no new worker pool appears.

### Optional-family isolation

For Linux/macOS/Windows where applicable, inject drive, disk-I/O, network, and CPU-frequency failures and prove the current family-specific absence behavior while core CPU/memory collection remains successful.

Preserve the macOS transition-diagnostic behavior established by Plans 128-129.

## Wire-equivalence fixtures

Introduce a small set of canonical collector-result fixtures above the native source layer and serialize them through the existing `greggd` conversion path.

At minimum cover:

- Linux ready sample with load/swap/iowait/drives/frequency/disk/network;
- macOS ready sample with no CPU-frequency value and native optional telemetry;
- Windows ready sample with commit and no load/swap/iowait;
- optional-family absence;
- successful empty drive enumeration.

Record exact v1/v2 expectations.

Required invariants:

- Linux/macOS still produce v1 and v2;
- Windows still produces no v1 status snapshot;
- v2 capability flags remain unchanged;
- optional payload field names/absence remain unchanged;
- `drives: None` and `Some(empty)` remain distinguishable;
- protocol validation still accepts every canonical ready payload.

Prefer Rust-structure equality plus JSON fixture equality where serialization order is deterministic under current structs. Do not create a large snapshot-test framework dependency.

## Public-path compile characterization

Add a compile-only or ordinary unit-test module that imports the public collector paths expected to survive Plan 135.

The goal is to make accidental removal/renaming visible when `greggd::collector` becomes a facade.

Do not promise every incidental private path. Record and test only the current public contract used by repository code or reasonably exposed by the public module structure.

## Native baseline record

Before Plan 134 source movement, record one ordinary CI run at the characterization SHA with:

- Linux;
- macOS arm64;
- macOS Intel;
- Windows;
- MSRV Rust 1.89.

No new CI job is required.

If current main has a transient hosted-runner failure unrelated to collector behavior, use the repository's normal rerun policy and record it truthfully; do not weaken native requirements.

## Documentation

Add a concise extraction boundary note to `architecture/collectors.md` or a dedicated architecture document that states:

- which behavior belongs to native collection;
- which behavior belongs to the daemon sampler;
- which behavior belongs to `gregg-protocol`;
- which public `greggd::collector` paths are temporarily compatibility-stable.

Do not rewrite historical Plans 109, 128, or 129.

## Verification

Run:

~~~text
cargo test -p greggd --all-targets --all-features -- collector
cargo test -p greggd --all-targets --all-features -- sampler
cargo test -p gregg-protocol --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Then use one ordinary existing CI run as the native baseline.

## Acceptance criteria

- [ ] Current public/cross-module collector paths and responsibilities are explicitly inventoried.
- [ ] CPU warmup/reset/recovery sequences are deterministically characterized for Linux, macOS, and Windows.
- [ ] Disk/network baseline, disappearance/reappearance, counter-decrease, and source-failure behavior is characterized.
- [ ] Drive refresh first-result, empty-result, last-success, panic, and blocked-drop semantics are characterized.
- [ ] Optional telemetry failures are proven not to fail core readiness.
- [ ] Canonical Linux/macOS/Windows collector results produce frozen v1/v2 Rust and JSON expectations.
- [ ] Windows v2-only behavior is explicitly frozen.
- [ ] Public collector compatibility imports are covered by a compile/test boundary.
- [ ] No collector formula, timing location, cadence, protocol shape, readiness policy, worker behavior, or platform support is changed.
- [ ] Rust 1.89 and all current native CI jobs are green at the characterization SHA.
- [ ] The architecture boundary is documented for Plan 134/135 handoff.

## Explicit non-goals

Do not include:

- moving collector source into a new crate;
- changing `CollectedMetrics`;
- changing v1/v2 protocol types;
- new metrics;
- richer metric-state enums;
- clock injection;
- slow-probe configurability;
- Windows processor-group expansion;
- BSD code;
- dependency/footprint optimization;
- publication to crates.io.

## Handoff note

Plan 133 should leave the repository behaviorally unchanged but make the collector contract difficult to accidentally alter.

When the characterization suite is green, Plan 134 may move the implementation. If Plan 134 reveals an uncharacterized behavior, add the missing characterization here or in the same corrective pass before changing semantics.

## Closure record

Implemented cumulatively at `a9dab65` plus `a5624a9` plus devstat fix `43b5cf3` alongside Plans 134-136 (single
implementation commit; Plan-133-owned files:
`crates/greggd/src/collector/compat_freeze.rs`,
`architecture/collectors.md` boundary section, and the `compat_freeze`
module declaration in `crates/greggd/src/collector/mod.rs`).

The characterization suite (22 tests) pins: shared rate warmup/actual-
elapsed/disappearance/reappearance/decrease/clear semantics; drive-cache
nonblocking poll, immediate first request, `None`-vs-`Some(empty)`
distinction at conversion; canonical Linux/macOS/Windows ready samples
through `into_snapshot_pair` with v1/v2 shape, capability, Windows-v2-only,
and JSON presence/absence expectations; protocol limit constants;
sampler `Warming`/`CounterReset`-never-fail vs hard-failure mapping;
Linux CPU warmup/reset/recovery plus hotplug refresh; Linux optional-family
absence preserving core sampling; frozen CPU-math spot checks, percentage
helpers, error taxonomy, and drive normalization; and public-path compile
imports for the shared contract plus each native platform seam
(cfg-gated so Linux/macOS/Windows jobs each prove their own collector,
with macOS/Windows sequence tests exercising the same reset/recovery and
optional-isolation contract through their mocks).

No collector formula, timing location, cadence, protocol shape, readiness
policy, worker behavior, or platform support was changed. Local
verification at the implementation SHA: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo test --workspace --all-targets --all-features`, and
`./scripts/check-local.sh` all green. Remote CI run `36219678790` at the
implementation SHA is green across Linux, macOS arm64, macOS Intel,
Windows (incl. SCM smoke), MSRV Rust 1.89, and the new FreeBSD job.

Acceptance: all boxes hold at the implementation SHA. Plans 134-135 used
these frozen expectations as the qualification authority; no
uncharacterized behavior was found during the move.

## Post-closure note (Plan 135 cutover)

During cutover the shared `CollectError`/`CollectErrorKind`,
`CounterBaselines`, drive normalization, `DriveRefreshCache`, and
percentage helpers became re-exports of `gregg-host` at the same
`greggd::collector` paths, and platform collectors became facades
delegating production sampling to `gregg-host`. The characterization
suite was updated only where it constructed the moved `DriveMetrics`
type directly (now the host type with identical fields); every frozen
behavioral expectation is unchanged and green on the extracted
implementation.
