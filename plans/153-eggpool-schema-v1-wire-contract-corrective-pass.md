# Plan 153: EggPool schema-v1 wire-contract corrective pass

Status: complete. See the closure record at the end of this file.

Depends on: completed Plan 152/current main. Independent of the remaining Plan 091 soak record.

Upstream contract reviewed against: eggstack/eggpool `299a0b3657667af509742a184e658c14df22d406` (2026-10-02), especially `rust/src/operations/status.rs::ProxyStatusSnapshot` and `rust/src/server/health.rs`.

## Objective

Correct Gregg's Plan-152 `/api/status` decoder and tests so they match EggPool's actual schema-version-1 JSON. Preserve the valid Plan-151 worker and Plan-152 dual-plane transport/state/UI architecture.

Post-closure review found that Gregg's synthetic fixture validates a Gregg-invented shape rather than EggPool's serialized shape. A real provider-bearing status response therefore fails decode even though Gregg CI is green.

## Confirmed mismatch

Gregg currently expects:

~~~text
root.routable_accounts
root.enabled_accounts
providers[].id
providers[].observation
~~~

EggPool schema v1 emits:

~~~text
proxy.routable_accounts
proxy.enabled_accounts
providers[].provider_id
providers[].last_observation
~~~

Because `EggpoolProviderWire.id` is required, a real provider row fails serde decoding. With no providers, proxy health can decode but the misplaced account counts silently become `None`.

Bounds also drifted:

~~~text
                         Gregg     EggPool
provider rows             256        256
provider ID bytes          64         96
reason-code bytes         128         64
status body ceiling     1 MiB      1 MiB
~~~

The EggPool producer constants are `MAX_STATUS_PROVIDERS = 256`, `MAX_PROVIDER_ID_CHARS = 96` (documented as bytes retained in status output), and `MAX_REASON_CODE_CHARS = 64`.

## Scope decisions

1. Fix only the private schema-v1 consumer and its qualification fixtures.
2. Keep `/api/stats/summary` unchanged as the source of the existing four metrics.
3. Keep `/api/status` independently bounded/authenticated and concurrently fetched with summary.
4. Keep Plan-151 `EggpoolDesiredState`/watch convergence unchanged.
5. Keep Plan-152 AppState freshness, partial-success behavior, and TUI layout unchanged.
6. Do not add compatibility aliases for Gregg's never-upstream `id`, `observation`, or root-account-count shape.
7. Unknown extra JSON fields remain tolerated.
8. Add no dependency, config field, endpoint, cadence, pane, workflow, or EggPool-side change.

## Workstream A: correct the wire mirror

Make the private decoder consume these canonical fields:

~~~text
root.schema_version
root.proxy.status
root.proxy.available
root.proxy.uptime_seconds
root.proxy.model_count
root.proxy.routable_accounts
root.proxy.enabled_accounts
root.proxy.reason_code
root.providers[].provider_id
root.providers[].status
root.providers[].last_observation
~~~

Fields Gregg does not display may stay unmodeled and ignored, including `observed_at`, `runtime`, `proxy.ready`, `proxy.version`, `proxy.base_url`, provider account counts, probe detail, and provider reason codes.

Normalize:
- proxy account counts from `wire.proxy`;
- `EggpoolProviderRow.id` from `provider_id`;
- `EggpoolProviderRow.observation` from `last_observation`.

Preserve existing ready/degraded/unready proxy mappings, provider state mappings, future-schema handling, and the existing policy for unknown provider status/observation values. Unknown proxy status remains `InvalidStatus`. Do not accept EggPool's CLI-only proxy `Unavailable` as a server-reported state.

## Workstream B: align producer-owned bounds

Set:

~~~text
MAX_STATUS_PROVIDER_ROWS = 256
MAX_PROVIDER_ID_BYTES = 96
MAX_STATUS_REASON_BYTES = 64
MAX_STATUS_RESPONSE_BYTES = 1024 * 1024
~~~

Add exact-boundary tests for 96/97-byte provider IDs, 64/65-byte reason codes, and 256/257 provider rows. Use ASCII for exact byte-boundary fixtures; do not expand this plan into EggPool's identifier-truncation implementation.

## Workstream C: add an upstream-provenance schema fixture

Replace the self-authored passing status shape with one canonical schema-v1 fixture or fixture constructor explicitly tied to:

~~~text
repo: eggstack/eggpool
commit: 299a0b3657667af509742a184e658c14df22d406
type: rust/src/operations/status.rs::ProxyStatusSnapshot
serialization: serde JSON
~~~

The passing payload must contain:
- `schema_version: 1`;
- representative `observed_at` and `runtime`;
- proxy status/available/model_count/routable_accounts/enabled_accounts plus representative ignored proxy fields;
- at least one provider with `provider_id`, a real status, `last_observation`, and representative ignored provider fields.

Assert that it decodes `Online`, preserves provider ID/observation, reads account counts from `proxy`, and ignores the extra canonical fields.

Add a negative regression proving the old Gregg-only provider payload (`id` without `provider_id`) does not remain a supported passing schema. Do not add aliases for it.

## Workstream D: move the existing Plan-152 matrix onto the canonical shape

Update the existing helpers/tests rather than maintaining two matrices. Retain coverage for:
- all proxy and provider states;
- verified/failed/stale/never observations;
- public summary + status 401;
- summary OK + status 404;
- summary 404 + status OK;
- malformed/oversized/future-schema/invalid-proxy cases;
- invalid endpoint/credential and no-secret behavior;
- stalled health independent of summary;
- worker partial success across both planes;
- separate 16 KiB summary and 1 MiB status ceilings.

## Workstream E: planning/docs reconciliation

Append a post-closure correction note to Plan 152; do not rewrite its historical implementation or CI record.

Register Plan 153 in `plans/README.md`, add `152 -> 153`, and replace the stale claims that Plans 151-152 are terminal / Plan 091 is the only in-progress plan. While 153 is open, active plans are 091 and 153.

Update `.opencode/skills/eggpool/SKILL.md` with the canonical field names/nesting and 96/64 producer bounds so implementation agents do not copy the stale fixture.

Only update architecture/user docs during implementation if they contain an exact incorrect wire detail. The visible pane contract does not change.

## Expected implementation surface

Likely:
~~~text
crates/gregg/src/eggpool.rs
crates/gregg/tests/fixtures/...   # only if a file is cleaner than inline fixture
.opencode/skills/eggpool/SKILL.md
CHANGELOG.md                      # concise corrective entry if warranted
plans/152-eggpool-service-health-status-plane-integration.md
plans/153-eggpool-schema-v1-wire-contract-corrective-pass.md
plans/README.md
~~~

Not expected:
~~~text
crates/gregg/src/main.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/eggpool.rs
crates/greggd/**
crates/gregg-protocol/**
crates/gregg-host/**
crates/gregg-update/**
Cargo.toml / Cargo.lock
EggPool repository
config schema
workflows / release / installers
~~~

## Verification

Run:

~~~text
cargo test -p gregg --all-targets --all-features -- eggpool
cargo test -p gregg --all-targets --all-features -- ui::eggpool
cargo test -p gregg --all-targets --all-features -- state::tests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

The focused EggPool suite must contain a passing payload with the exact tokens `provider_id`, `last_observation`, nested `routable_accounts`/`enabled_accounts`, and representative `runtime`, without serde aliases for the obsolete Gregg-only names.

Use ordinary existing CI for hosted closure. No new job/matrix.

## Acceptance criteria

- [x] Gregg accepts the canonical field placement/names emitted by EggPool schema-v1 `ProxyStatusSnapshot`.
- [x] Account counts are read from `proxy`.
- [x] Provider identity comes from `provider_id`.
- [x] Provider observation comes from `last_observation`.
- [x] A provider-bearing upstream-shaped payload decodes `Online`.
- [x] Provider identity/observation and proxy account counts survive normalization.
- [x] Bounds are 256 providers, 96-byte provider IDs, and 64-byte reason codes, with exact/one-over tests.
- [x] Status remains capped at 1 MiB and summary at 16 KiB.
- [x] Plan-152 concurrency, partial success, auth, compatibility, freshness, and rendering remain unchanged.
- [x] No alias/fallback preserves the never-upstream old fixture as a supported schema.
- [x] The canonical fixture records EggPool source commit/type provenance and includes ignored canonical fields.
- [x] No EggPool build/network dependency is added to Gregg tests or CI.
- [x] Plan 152 receives an append-only correction note and registry status/dependencies are truthful.
- [x] Focused checks and ordinary CI pass.
- [x] No dependency/config/endpoint/cadence/worker/pane/daemon/protocol/workflow/release scope is added.

## Stop conditions

Split new work if this requires an EggPool API/schema change, shared generated bindings/crate, EggPool in Gregg's dependency graph, Plan-151 worker changes, summary/cadence changes, drill-down UI, new auth/config fields, or cross-repo CI orchestration.

## Closure record

Implementation: `195724c1a1787e22b7288062a5fc45c51739a628`. The work was bounded to the private schema-v1 consumer, its qualification fixtures, active documentation, and planning records.

### Workstream A: corrected wire mirror

`crates/gregg/src/eggpool.rs` now decodes the canonical field set:

- `EggpoolProxyWire` gained `routable_accounts` / `enabled_accounts`, and
  `EggpoolStatusWire` no longer declares them at the document root.
- `EggpoolProviderWire` requires `provider_id` and reads `last_observation`
  instead of `id` / `observation`.
- `normalize_health` sources the account counts from `wire.proxy` and maps
  `provider_id` -> `EggpoolProviderRow.id` and `last_observation` ->
  `EggpoolProviderRow.observation`. The public model, the ready/degraded/
  unready proxy mapping, the provider-state and unknown-value policies, the
  future-schema handling, and the rejection of an unknown proxy status are
  unchanged, and EggPool's CLI-only proxy `unavailable` state is still not
  accepted as a server-reported value.
- `observed_at`, `runtime`, `proxy.ready`, `proxy.version`, `proxy.base_url`,
  provider account counts, probe detail, and the provider reason code remain
  unmodeled and ignored, and no serde alias or fallback was added for any
  never-upstream name.

### Workstream B: producer-owned bounds

`MAX_STATUS_PROVIDER_ROWS = 256`, `MAX_PROVIDER_ID_BYTES = 96`,
`MAX_STATUS_REASON_BYTES = 64`, and `MAX_STATUS_RESPONSE_BYTES = 1024 * 1024`
now match EggPool's `MAX_STATUS_PROVIDERS`, `MAX_PROVIDER_ID_CHARS`,
`MAX_REASON_CODE_CHARS`, and status-client ceiling. The summary bound stays
16 KiB and is still applied per route.

`bounded_status_contract_matches_the_producer_bounds_exactly` asserts the
three constants against literal 96/64/256 values and then proves each limit is
exact using ASCII fixtures: 96 bytes decodes and 97 is rejected, 64 decodes and
65 is rejected, 256 rows decode and 257 are rejected. Literal counts are used
deliberately so a drifted bound fails the test instead of moving the
expectation with the constant. Negative uptime remains rejected in the same
test.

Mutation-checked rather than assumed: reverting the bounds to the old 64/128/256
values and the field names to `id` / `observation` fails 13 EggPool tests
including all three new ones, and moving the account counts back to the
document root fails `canonical_upstream_status_snapshot_decodes_and_ignores_unmodeled_fields`,
`gregg_local_status_shape_is_not_a_supported_schema`, and
`health_decodes_ready_degraded_and_unready_proxy_states`. The suite is not
vacuous.

### Workstream C: upstream-provenance fixture

`canonical_status_body` replaces `healthy_status_body` as the single passing
status fixture for the module. Its doc comment records the provenance —
`eggstack/eggpool`, commit `299a0b3657667af509742a184e658c14df22d406`, type
`rust/src/operations/status.rs::ProxyStatusSnapshot`, serde JSON — and states
that a payload that does not match that shape is not a passing schema.

`canonical_upstream_status_snapshot_decodes_and_ignores_unmodeled_fields` reads
a full upstream-shaped payload carrying `schema_version: 1`, root `observed_at`
and `runtime`, `proxy.status` / `available` / `uptime_seconds` /
`model_count` / `routable_accounts` / `enabled_accounts` plus the ignored
`proxy.ready` / `proxy.version` / `proxy.base_url`, and one provider row with
`provider_id`, a real status, `last_observation`, and ignored provider account
counts, probe detail, and reason code. It asserts `Online`, `schema_version`,
proxy `Ready`, `available`, `uptime_seconds`, `model_count`, counts `5`/`6`
read from `proxy`, the preserved provider id `openai`, the provider
`Degraded`/`Stale` state, and that the provider's own `reason_code` is not
mistaken for the proxy's.

`gregg_local_status_shape_is_not_a_supported_schema` is the negative
regression: the Plan-152 payload with `id`/`observation` and root counts is
`DecodeError`, and a provider-free payload carrying only root counts still
decodes the proxy but reports both counts as absent. No alias exists for the
old shape.

### Workstream D: the existing matrix on the canonical shape

Rather than keeping two matrices, every existing passing status fixture now
uses the canonical constructor, and
`health_decodes_every_provider_state_without_conflation` emits
`provider_id` / `last_observation` rows directly. Retained coverage:
ready/degraded/unready proxy; every provider state including unrecognized and
absent; verified/failed/stale/never observations; public summary + 401 status;
summary OK + 404 status; summary 404 + status OK; malformed status and
malformed summary each independent of the other plane; oversized status;
per-route 16 KiB summary and 1 MiB status ceilings; 403 and 503 status codes;
future schema and invalid proxy status; bounded rejection; unusable
credential; invalid endpoint; no secret in `Debug`; a stalled health route that
does not hide a summary failure; and one worker result carrying both planes.
`crates/gregg/src/state.rs`, `crates/gregg/src/ui/eggpool.rs`,
`crates/gregg/src/main.rs`, `greggd`, `gregg-protocol`, `gregg-host`, and
`gregg-update` are untouched, which is why `state::tests` and `ui::eggpool`
needed no change.

The focused EggPool suite contains a passing payload with the exact tokens
`provider_id`, `last_observation`, `proxy`-nested
`routable_accounts`/`enabled_accounts`, and a representative `runtime`, and
`grep` confirms no `alias` attribute exists in the module.

### Workstream E: planning and documentation reconciliation

- Plan 152 already carried its append-only post-closure correction note
  recording this defect; it is preserved verbatim and its historical
  implementation and CI record are not rewritten.
- `plans/README.md` records Plan 153 as complete, adds `152 -> 153`, and states
  that the EggPool corrective line is finished.
- `.opencode/skills/eggpool/SKILL.md` gained a "Schema-v1 wire contract"
  section with the canonical field list, the ignored-field list, the upstream
  provenance, and the explicit prohibition on aliases for the Plan-152 shape;
  the stale `≤64-byte provider IDs, ≤128-byte reason codes` bound line is
  corrected to 96/64 and the corrective-plan status is updated.
- `architecture/gregg-client.md` records the producer-aligned 96/64 bounds and
  the canonical names/placement.
- `CHANGELOG.md` gained a concise `Fixed` entry describing the corrected wire
  contract, the reason the previous fixture hid the defect, and the realigned
  bounds.
- `README.md`, `docs/client.md`, and `crates/gregg/README.md` were inspected:
  they describe the visible pane contract only and contained no incorrect wire
  detail, so no change was warranted. The visible pane contract is unchanged.

### Verification

```text
cargo test -p gregg --all-targets --all-features -- eggpool    # 75 + 4 passed
cargo test -p gregg --all-targets --all-features -- ui::eggpool # 11 passed
cargo test -p gregg --all-targets --all-features -- state::tests # 57 passed
cargo fmt --all -- --check                                     # clean
cargo clippy --workspace --all-targets --all-features -- -D warnings  # clean
cargo test --workspace --all-targets --all-features            # all suites passed
./scripts/check-local.sh                                       # all checks passed
```

Ordinary existing CI is the hosted closure mechanism; no new job, matrix, or
workflow was added, and no EggPool build or network access is required by any
test. Exact CI run IDs are recorded in `plans/README.md` after the closure
push.

### Scope reconciliation

Only `crates/gregg/src/eggpool.rs` plus documentation and planning records
changed. No dependency was added, no `Cargo.toml`/`Cargo.lock` change exists,
and no configuration field, endpoint, cadence, worker contract, pane,
`greggd`/`gregg-protocol`/`gregg-host`/`gregg-update` surface, workflow, or
release step was touched. The stop conditions were not reached: no EggPool
API/schema change, shared binding crate, EggPool dependency, Plan-151 worker
change, summary/cadence change, drill-down UI, new credential field, or
cross-repo CI orchestration was required.

Future-plan impact: Plan 153 is terminal for the EggPool corrective line and
unblocks no further plan. Plan 091 remains the only in-progress plan, still
gated solely on its own extended soak record. No other planned or in-progress
plan depended on Plan 153. The pre-existing stale `Status: planned` header on
the retired Plan 064 file remains the unenumerated artifact already recorded in
Plan 146's closure and is not remade here.
