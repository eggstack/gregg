# Plan 153: EggPool schema-v1 wire-contract corrective pass

Status: planned.

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

- [ ] Gregg accepts the canonical field placement/names emitted by EggPool schema-v1 `ProxyStatusSnapshot`.
- [ ] Account counts are read from `proxy`.
- [ ] Provider identity comes from `provider_id`.
- [ ] Provider observation comes from `last_observation`.
- [ ] A provider-bearing upstream-shaped payload decodes `Online`.
- [ ] Provider identity/observation and proxy account counts survive normalization.
- [ ] Bounds are 256 providers, 96-byte provider IDs, and 64-byte reason codes, with exact/one-over tests.
- [ ] Status remains capped at 1 MiB and summary at 16 KiB.
- [ ] Plan-152 concurrency, partial success, auth, compatibility, freshness, and rendering remain unchanged.
- [ ] No alias/fallback preserves the never-upstream old fixture as a supported schema.
- [ ] The canonical fixture records EggPool source commit/type provenance and includes ignored canonical fields.
- [ ] No EggPool build/network dependency is added to Gregg tests or CI.
- [ ] Plan 152 receives an append-only correction note and registry status/dependencies are truthful.
- [ ] Focused checks and ordinary CI pass.
- [ ] No dependency/config/endpoint/cadence/worker/pane/daemon/protocol/workflow/release scope is added.

## Stop conditions

Split new work if this requires an EggPool API/schema change, shared generated bindings/crate, EggPool in Gregg's dependency graph, Plan-151 worker changes, summary/cadence changes, drill-down UI, new auth/config fields, or cross-repo CI orchestration.

## Closure record

Not yet implemented.
