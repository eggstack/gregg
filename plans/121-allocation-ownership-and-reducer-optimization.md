# Plan 121: allocation ownership and reducer optimization

Status: complete; implementation `45582ce`.

Depends on: Plan 120 and the settled Plan-119 transport baseline.

## Objective

Remove avoidable copying and allocation from the daemon sample/publication path, client batch-normalization path, state reducer, and live-metric counter baselines while preserving every current public API and runtime behavior.

This plan is deliberately mechanical. It must not change scheduler architecture, HTTP response caching, TUI formatting policy, collector freshness, protocol shape, or sampling/polling cadence.

## Current costs to remove

### 1. CollectedMetrics is copied to create v1 and v2

On Linux/macOS, Sampler::convert_sample currently uses metrics.clone().into_snapshot(...) and then consumes metrics into StatusPayloadV2.

The clone includes optional Vec-backed v2-only data even though v1 cannot contain drives, disk I/O, CPU frequency, or network payloads.

CollectedMetrics::into_status_payload_v2 also clones drives, disk_io, and network before consuming self because into_snapshot_v2 currently consumes the same structure.

The desired internal path is one ownership-aware conversion in which:

- shared scalar/Copy fields are reused directly;
- the small SystemIdentity is cloned only where both independently owned wire snapshots require it;
- v2-only Vec/String ownership moves into StatusPayloadV2 instead of being deep-cloned;
- existing public conversion methods retain their signatures and behavior.

### 2. Sampler Arcs are dereferenced and deep-cloned before publication

Sampler stores Option<Arc<StatusSnapshot>> and Option<Arc<StatusPayloadV2>>.

sync_sampler_state receives those Arcs but currently calls update_snapshot((*snap).clone(), (*snap_v2).clone()) or the single-version equivalents. ServerState then allocates new Arcs.

Add crate-internal/shared publication methods that accept the existing Arcs. Keep the current public update_snapshot, update_snapshot_v1_only, and update_snapshot_v2_only signatures intact as compatibility wrappers.

At this phase, ready health envelopes may still require a base-snapshot clone because Plan 123 owns the larger PublishedState/health redesign. Plan 121 must nevertheless stop cloning the full v2 payload merely to cross the sampler/server boundary.

### 3. Production batch application borrows data that it owns

run_event_loop receives an owned PollBatch and immediately calls the borrowed AppState::apply_batch(&batch). NormalizedSnapshot constructors therefore clone identity strings, drive names, disk-I/O device strings, network interface strings, and Vec contents, after which the original PollBatch is dropped.

Preserve:

~~~text
pub fn apply_batch(&mut self, batch: &PollBatch)
~~~

for downstream/tests.

Add a crate-internal owned production path, for example apply_batch_owned(PollBatch), and owned NormalizedSnapshot constructors or helpers that move payload-owned strings/collections whenever possible.

Do not change PollOutcome or PollBatch public wire/application semantics merely to optimize this path.

### 4. Batch application performs O(N squared) stable-ID matching

For each PollResult, AppState currently searches self.systems.iter_mut().find(...).

The scheduler normally emits exactly one result per configured endpoint in configured endpoint order. Use that invariant as a fast path without making it a correctness requirement:

1. enumerate the results;
2. if systems[index] exists and has the same stable ID, use that index directly;
3. otherwise fall back to the existing stable-ID search;
4. retain the existing endpoint host/port equivalence guard before applying a result.

This makes the ordinary path O(N) while preserving safe behavior for reordered, partially synthetic, stale, or future test batches.

Do not add a persistent AppState HashMap solely for this optimization unless implementation proves the positional/fallback path is insufficient. A persistent index would add reconciliation state that Gregg does not otherwise need.

### 5. CounterBaselines allocates stable keys repeatedly

CounterBaselines::observe currently performs:

~~~text
self.samples.insert(id.to_owned(), current)
~~~

on every observation.

Use borrowed lookup/update for an occupied String key and allocate id.to_owned() only for a previously unseen identity. Entry/get_mut implementation details are flexible, but stable interfaces/devices must stop allocating a replacement key every sample.

The existing retain_ids behavior may remain as-is in this phase. Do not introduce epoch/generation bookkeeping merely to eliminate its bounded temporary HashSet unless a focused measurement demonstrates that it is a material remaining cost and the implementation stays smaller than the existing code.

## Workstream A: one-pass CollectedMetrics conversion

Implement an internal conversion helper owned by greggd, not gregg-protocol.

Requirements:

- destructure CollectedMetrics once;
- validate CPU/iowait/numeric semantics exactly as the existing conversion methods do;
- produce v1 only when supports_v1_snapshot is true;
- always produce v2 on a successful Ready sample;
- move drives, disk_io, network, and other v2-only owned data directly into StatusPayloadV2;
- preserve validate_v2_payload before publication;
- preserve the public CollectedMetrics conversion methods and their existing tests.

Avoid duplicating validation arithmetic between the old public helpers and the new internal path. Prefer small shared scalar-building helpers if necessary.

## Workstream B: Arc-preserving server handoff

Add internal ServerState publication methods that accept:

~~~text
Arc<StatusSnapshot>
Arc<StatusPayloadV2>
~~~

or their optional single-version variants.

The existing public owned-value methods should wrap the shared methods by constructing Arcs, so downstream/source compatibility is retained.

sync_sampler_state must use the shared methods.

Required behavior:

- Standard Linux/macOS path publishes both versions coherently.
- Windows path remains v2-only and v1 health remains NotServing.
- The fallback v1-only path remains available.
- observed_at selection remains max(v1, v2) on dual publication.
- failure count reset and stale-policy state are unchanged.
- no lock is held across sampler collection or snapshot conversion.

Add a focused test proving Arc identity is retained across the internal shared publication path, using Arc::ptr_eq on a snapshot obtained from the server state. This is deterministic evidence that the full payload is no longer copied at that boundary.

## Workstream C: owned client normalization

Add owned equivalents for v1/v2 normalization while leaving the current borrowed constructors intact.

For v2:

- move SystemIdentity strings;
- move drive names;
- move disk-I/O device id/name/drive_name strings;
- move network interface id/name strings;
- move Vec ownership into newly normalized Vecs while moving each nested String rather than cloning it;
- preserve every optional-field and capability semantic.

The public borrowed constructors remain the reference behavior. Add equality tests showing borrowed and owned normalization produce identical NormalizedSnapshot values for Linux, macOS, Windows, drive-heavy, disk-I/O-heavy, and network-heavy fixtures.

The production event loop should consume PollBatch through the owned reducer path once tests establish parity.

## Workstream D: O(N) ordinary reducer matching

Factor result-to-system resolution so borrowed and owned batch paths share the same generation and endpoint-validity rules.

Required tests:

1. 100-system and 500-system ordered batches map every result correctly through the positional fast path.
2. A deliberately reordered batch still applies correctly through stable-ID fallback.
3. A result with a retained stable ID but superseded host/port remains rejected.
4. Generation wrap behavior remains unchanged.
5. Duplicate or unknown IDs, if constructible in tests, do not mutate the wrong system.
6. First-batch selection/viewport snap remains unchanged.
7. Later batches preserve selection/viewport semantics.

Do not weaken debug assertions or generation checks to make the fast path easier.

## Workstream E: allocation-free stable counter-key updates

Modify CounterBaselines::observe so an existing key is updated in place.

Preserve:

- first observation returns None;
- monotonic positive elapsed intervals return rates;
- counter reset or zero/invalid elapsed time returns None after updating the current baseline exactly as today;
- hotplug/reappearance semantics after retain_ids;
- overflow-safe rate conversion.

Add a focused test that repeatedly observes the same ID and proves map cardinality and semantic results remain stable. A test-only key-allocation/pointer probe is acceptable if small, but do not add a global allocator or production instrumentation solely for this proof.

## API and compatibility constraints

This plan must not remove or change the signatures of:

- AppState::apply_batch;
- NormalizedSnapshot::from_v1, from_v2, or from_v2_payload;
- CollectedMetrics public conversion methods;
- ServerState public update/getter methods;
- protocol types or serde attributes.

Additive pub(crate) helpers are preferred. Additive public helpers are allowed only if there is a real downstream use case; internal optimization alone is not sufficient reason to expand the public API.

## Verification

Focused:

~~~text
cargo test -p greggd sampler
cargo test -p greggd server
cargo test -p greggd collector::rate
cargo test -p gregg state
cargo test -p gregg normalized
cargo test -p gregg main
~~~

Then:

~~~text
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
./scripts/check-local.sh
~~~

Use the existing sustained workload driver for a before/after client fleet sample if useful. Do not add a performance CI threshold.

Record fresh stripped release sizes for gregg and greggd. Investigate unexplained growth above roughly 1%; do not optimize binary size at the expense of the runtime ownership improvements without evidence.

## Acceptance criteria

- [x] Linux/macOS dual-version sample conversion no longer deep-clones v2-only drive/disk/network collections merely to construct v1.
- [x] into_status_payload_v2-compatible behavior is preserved while the new sampler path moves v2-only collections.
- [x] sync_sampler_state no longer deep-clones the full sampler v2 payload before ServerState publication.
- [x] Existing public ServerState update methods retain their signatures and behavior.
- [x] Production event-loop batch application can move owned payload data into normalized state.
- [x] Existing public borrowed normalization and AppState::apply_batch remain available and behavior-compatible.
- [x] Ordered normal batches resolve in O(N) through positional matching; reordered batches retain stable-ID fallback.
- [x] Superseded endpoint protection and generation semantics are unchanged.
- [x] Existing CounterBaselines identities do not allocate a replacement String key every sample.
- [x] No scheduler, HTTP cache, TUI layout, sample cadence, protocol, or dependency redesign is included.
- [x] Focused tests, workspace tests, strict clippy, and the default local check pass.
- [x] Closure records source SHA, measurement environment, structural proof, and final release sizes.

## Closure record

Implementation `45582ce` (Rust 1.98.1, `x86_64-unknown-linux-gnu`, start SHA
`1c82884`) adds the Arc-preservation test, borrowed/owned normalization parity
tests, ordered and reordered 500-system reducer coverage, and the repeated
CounterBaselines identity test. Focused evidence passed: `greggd sampler` 31,
`greggd server` 60, `greggd collector::rate` 5, `gregg state` 51, and
`gregg normalized` 19 tests. The full workspace test run also passed (571
`gregg` tests and all `greggd` targets), as did strict clippy and the default
`./scripts/check-local.sh`.

The structural before/after audit from `1c82884` to `45582ce` shows one-pass
sample conversion, Arc pointer identity across publication, owned reducer
normalization, positional matching with stable-ID fallback, and one stable
counter key (`len() == 1` after repeated observations). A release-mode fixed UI
test loop was collected in an isolated baseline worktree for descriptive
comparison; it is intentionally not a CI threshold. Final stripped sizes are
`gregg` 3,740,592 bytes and `greggd` 2,432,408 bytes. Hosted CI is recorded in
the roadmap/table after the final push.
