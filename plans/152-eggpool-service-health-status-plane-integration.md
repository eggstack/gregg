# Plan 152: EggPool service-health status-plane integration

Status: complete. See the closure record at the end of this file.

Depends on: Plan 151. Also relies on EggPool's current schema-version-1 authenticated GET /api/status contract introduced in EggPool commit 3dc9ece9d7 and still present on current EggPool main. Independent of the remaining Plan 091 soak record and planned Plan 147.

## Objective

Make Gregg distinguish its local EggPool worker lifecycle from the actual operational health of the configured EggPool instance.

Gregg currently consumes only GET /api/stats/summary and therefore knows periodized usage/performance metrics but not whether the proxy is ready, degraded, or unready or whether individual providers are ready, degraded, unavailable, disabled, or unknown. EggPool now exposes those facts through a bounded read-only schema-version-1 GET /api/status endpoint that performs no outbound provider probes.

Add that status plane alongside, not instead of, the existing four-value summary pane. Preserve Gregg's compact product scope: show proxy health and bounded provider-health context, not a new dashboard or drill-down UI.

## Upstream contract to mirror

EggPool schema version 1 currently defines:

~~~text
ProxyStatus
    ready
    degraded
    unready

ProviderStatus
    ready
    degraded
    unavailable
    disabled
    unknown

ProviderObservation
    verified
    failed
    stale
    never
~~~

EggPool's internal/CLI-only proxy unavailable state is not emitted by the server endpoint. Gregg must not deserialize transport failure as if EggPool itself reported ProxyStatus::Unavailable.

The server snapshot also carries:

- proxy.ready and proxy.available;
- version and base_url;
- uptime_seconds;
- model_count;
- routable_accounts and enabled_accounts;
- bounded reason_code;
- provider account/model/probe fields;
- runtime generation/reload/tasks/db/retiring fields.

Gregg does not need to display every field. Decode enough of the versioned schema to validate the contract and support compact health rendering without copying EggPool's full CLI.

## Governing invariants

1. /api/stats/summary remains the source of the existing four periodized metrics.
2. /api/status is a separate current-health plane and never changes metric meaning.
3. Health failure must not erase or suppress a successful summary, and summary failure must not erase or suppress successful health.
4. Public-dashboard summary access does not imply /api/status is public. EggPool keeps /api/status authenticated even when dashboard pages/summary are public.
5. Gregg reuses the configured api_key_env value for both requests when present; no new credential field or config migration is introduced.
6. An absent key may still allow public summary metrics while health reports auth-required/unknown.
7. Older EggPool instances returning 404 for /api/status remain usable for summary metrics; health is explicitly unsupported/unknown.
8. Status polling must not trigger outbound provider probes, quota use, or mutation; Gregg only reads EggPool's status endpoint.
9. Response bodies, credentials, raw upstream errors, prompts, and provider payloads are never rendered or retained.
10. No EggPool, greggd, or gregg-protocol change is required.

## Workstream A: add a typed local schema-v1 health model

Add narrowly scoped serde types under the existing Gregg EggPool client boundary.

Recommended separation:

~~~text
EggpoolProxyHealth
    Ready
    Degraded
    Unready

EggpoolProviderHealth
    Ready
    Degraded
    Unavailable
    Disabled
    Unknown

EggpoolProviderObservation
    Verified
    Failed
    Stale
    Never

EggpoolHealthSnapshot
    schema_version
    proxy
    providers
    selected bounded runtime facts only if useful

EggpoolHealthFetchOutcome
    Online(snapshot)
    AuthenticationRequired
    Forbidden
    Unsupported
    Timeout / ConnectionRefused / DnsFailure / NetworkError
    HttpStatus
    BodyTooLarge
    DecodeError
    UnsupportedSchema
    InvalidStatus
~~~

Names may be adjusted to fit existing style, but do not reuse the local worker-state enum from Plan 151.

Require schema_version == 1 before treating the payload as authoritative. Unknown future enum strings or a future schema version must degrade to an explicit unsupported/unknown health state, not crash the pane and not invalidate summary metrics.

Validate at minimum:

- server proxy status is ready/degraded/unready;
- server proxy.available is true;
- counts fit local integer types;
- optional uptime/latency values are finite/nonnegative where represented as floats;
- provider IDs/reason strings are bounded before rendering;
- provider row count is bounded consistently with EggPool's current 256-row ceiling.

Do not expose raw error text.

## Workstream B: add a separately bounded /api/status fetch

Keep the existing summary bound at 16 KiB.

The status endpoint can legally contain many provider rows; use a separate decoded-body ceiling aligned with EggPool's own bounded status client, currently 1 MiB. Do not widen the summary route to 1 MiB merely for convenience unless the EggFetch client requires one global ceiling and request-local lower caps demonstrably preserve the 16 KiB summary limit.

Transport requirements:

- same scheme/host/port normalization as the summary request;
- GET /api/status with no query;
- request-local Bearer auth from api_key_env when present;
- no redirects, retries, cookies, proxy expansion, or new HTTP dependency;
- existing whole-request deadline semantics;
- 401 and 403 classified separately;
- 404 classified as status Unsupported, not StatsUnavailable;
- oversized/malformed/invalid status isolated to the health plane.

Use the existing lean eggfetch-core profile and connection pooling.

## Workstream C: refresh summary and health independently

After Plan 151 establishes a convergent desired-state worker, each active refresh cycle should be able to obtain:

- the selected-period summary;
- the current health snapshot.

Prefer concurrent independent fetches within the single worker request task so a slow/unavailable health endpoint does not delay a valid summary and vice versa. The two fetches may share the EggFetch client/pool where body-limit semantics permit.

Refresh behavior:

Activation:
- fetch summary + health immediately.

Manual Ctrl-R:
- fetch summary + health immediately.

Passive 60-second refresh:
- fetch summary + health.

Period change:
- summary must refresh immediately for the new period.
- health may be fetched in the same cycle for simplicity; do not add a separate health cadence/cache scheduler merely to avoid one local read.

Cancellation/deactivation:
- abort both in-flight reads with the existing worker request.

The result delivered to AppState must preserve partial success. Do not collapse two outcomes into one success/failure enum.

## Workstream D: model health freshness independently in AppState

Add reducer-owned health fields separate from summary fields, for example:

~~~text
health: Option<EggpoolHealthSnapshot>
last_health_success_at: Option<Instant>
last_health_attempt_at: Option<Instant>
last_health_error: Option<EggpoolHealthFetchOutcome>
~~~

Exact names may vary.

Rules:

- successful health replaces prior health and clears health_error;
- a failed health refresh may keep the previous successful health visible, but rendering must indicate that the latest health refresh failed rather than claiming the stale snapshot is current;
- summary success/failure updates only summary state;
- health success/failure updates only health state;
- stale request generations/periods remain rejected through the existing Plan-151 generation authority;
- a period mismatch applies only to the summary payload; health has no period.

Do not invent a second app-wide generation system.

## Workstream E: compact TUI presentation

Preserve the four metric rows exactly.

Add proxy health as a compact header token or similarly bounded existing-line element:

~~~text
EggPool — Main EggPool    Health: degraded    Window: 1 hour
~~~

At narrow widths, preserve identity/window usability and degrade health text cleanly rather than wrapping the metrics.

Use plain status words as the primary signal; color may supplement them but must not be the only signal.

Provider detail remains compact. It is acceptable to use the existing footer budget for a count summary such as:

~~~text
Providers: 2 ready · 1 degraded · 1 unavailable
~~~

only when there is room and no higher-priority refresh/error diagnostic. Do not add scrolling provider tables, account names, model lists, raw probe errors, runtime diagnostics, charts, or a third pane.

Priority guidance:

1. worker unavailable/local transport diagnostics;
2. current health proxy status;
3. summary refresh failure/staleness;
4. compact provider-count context;
5. low-priority freshness text.

Exact layout may be adjusted by renderer tests, but the four metric labels remain unchanged.

## Workstream F: compatibility cases

Lock down the cases that make the two endpoints meaningfully different.

### Public summary, authenticated health

When EggPool dashboard is public and Gregg has no api_key_env:

- /api/stats/summary may succeed;
- /api/status may return 401;
- Gregg shows metrics;
- health renders auth required/unknown;
- the pane is not treated as wholly offline.

### Older EggPool

When summary exists but /api/status returns 404:

- metrics remain available;
- health renders unsupported/unknown;
- no retry storm or fallback scraping is added.

### Dashboard disabled

When /api/stats/summary returns 404 but /api/status succeeds:

- health remains visible;
- metrics render the existing stats-unavailable state;
- Gregg does not scrape dashboard HTML or read EggPool SQLite.

### Independent malformed/oversized cases

A malformed or oversized status response must not invalidate a good summary. A malformed or oversized summary must not invalidate a good health snapshot.

## Workstream G: documentation and compatibility truth

Update after implementation:

- README.md only as needed for the visible health token;
- crates/gregg/README.md if it describes the pane;
- architecture/gregg-client.md with the explicit two-plane model;
- docs/client.md / docs/display.md where user-facing EggPool behavior belongs;
- .opencode/skills/eggpool/SKILL.md;
- AGENTS.md only if a compact invariant is warranted;
- CHANGELOG.md;
- Plans 056/151 and plans/README.md to record that summary baseline remains valid and Plan 152 is additive post-closure status work.

Do not edit EggPool documentation from the Gregg repository.

## Expected implementation surface

Likely:

~~~text
crates/gregg/src/eggpool.rs
crates/gregg/src/state.rs
crates/gregg/src/ui/eggpool.rs
crates/gregg/src/main.rs            # only for result shape/wiring if required
architecture/gregg-client.md
docs/client.md
docs/display.md
crates/gregg/README.md              # if pane behavior is documented there
README.md                            # if top-level behavior needs a concise note
.opencode/skills/eggpool/SKILL.md
CHANGELOG.md
plans/056-eggpool-summary-pane-roadmap.md
plans/151-eggpool-desired-state-delivery-and-worker-state-corrective-pass.md
plans/152-eggpool-service-health-status-plane-integration.md
plans/README.md
~~~

No expected change:

~~~text
crates/greggd/**
crates/gregg-protocol/**
crates/gregg-host/**
EggPool repository
configuration schema
release/installer workflows
~~~

## Verification

Focused client/status tests:

~~~text
cargo test -p gregg --all-targets --all-features -- eggpool
cargo test -p gregg --all-targets --all-features -- ui::eggpool
cargo test -p gregg --all-targets --all-features -- state::tests
cargo test -p gregg --all-targets --all-features -- main::tests
~~~

Required synthetic HTTP matrix:

- summary success + status ready;
- summary success + status degraded;
- summary success + status unready;
- public summary success + status 401;
- summary success + status 404;
- summary 404 + status success;
- summary malformed + status success;
- summary success + status malformed;
- summary success + status oversized;
- status schema version != 1;
- provider ready/degraded/unavailable/disabled/unknown decode;
- no secret appears in Debug/UI/error strings.

Then run:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use the existing ordinary CI workflow for hosted closure. A live LAN EggPool smoke is optional and must not become a new gate or retained evidence artifact.

## Explicit acceptance criteria

- [x] Gregg has distinct local worker-state and remote `EggPool` health types.
- [x] `/api/status` schema version 1 decodes ready/degraded/unready proxy health.
- [x] Provider ready/degraded/unavailable/disabled/unknown states decode without conflation.
- [x] Server-side status never treats transport failure as proxy-reported unavailable.
- [x] Summary remains the source of the original four metrics with its 16 KiB bound.
- [x] Status has a separately appropriate bounded body ceiling (1 MiB), equal to the `EggPool` status-client bound and never widening the summary route.
- [x] Summary and health fetch independently and preserve partial success.
- [x] Public-summary + authenticated-status behavior is represented truthfully.
- [x] Older EggPool 404 status behavior preserves summary functionality.
- [x] Dashboard-disabled summary does not hide valid service health.
- [x] Unknown/future status schema is explicit and nonfatal.
- [x] No outbound provider probe, provider quota use, or health mutation is introduced.
- [x] Credentials, raw provider errors, prompts, and bodies never reach state rendering or logs.
- [x] TUI keeps the existing four metrics and adds only compact health context.
- [x] No provider/account/model drill-down or generic dashboard architecture is added.
- [x] No config schema, `greggd`, `gregg-protocol`, `EggPool`, dependency, workflow, or release change is required.
- [x] Focused tests and local checks pass (ordinary CI is recorded in the closure record).

## Stop conditions

Stop and split work if implementation would require:

- changing EggPool's schema-v1 /api/status endpoint;
- a second EggPool credential/config field;
- multiple EggPool instances;
- account/model/provider drill-down UI;
- direct database access;
- health-triggered provider probes;
- a generic datasource/status framework;
- a new dependency or CI service;
- changing the four summary metric meanings.

## Closure record

Implementation: `7b88e43` (`feat: add EggPool service-health status plane to the
client pane`).

### Workstream A: typed schema-v1 health model

- Added `EggpoolProxyHealth` (`Ready`/`Degraded`/`Unready`),
  `EggpoolProviderHealth` (`Ready`/`Degraded`/`Unavailable`/`Disabled`/`Unknown`),
  `EggpoolProviderObservation` (`Verified`/`Failed`/`Stale`/`Never`),
  `EggpoolProviderRow`, `EggpoolHealthSnapshot`, and
  `EggpoolHealthFetchOutcome`. They are deliberately distinct from Plan 151's
  `EggpoolWorkerState`.
- `EggpoolStatusWire` decodes only what the pane needs and ignores unknown extra
  fields. A provider row whose `status` is absent or unrecognized decodes as
  `Unknown`; an unrecognized `observation` decodes as `None` rather than a
  fabricated one.
- Validation bounds the contract: `schema_version == 1`, a known proxy status,
  at most 256 provider rows (EggPool's own ceiling), provider IDs ≤ 64 bytes,
  reason codes ≤ 128 bytes, and finite non-negative uptime. A future schema
  version is `UnsupportedSchema`; other violations are `InvalidStatus`. Both are
  explicit and nonfatal.

### Workstream B: separately bounded `/api/status` fetch

- `fetch_health` shares the summary client's transport, deadline, pooling, and
  origin normalization, sends `GET /api/status` with no query, and uses
  request-local bearer auth.
- Credential handling is now shared and explicit: an absent or empty
  `api_key_env` stops the summary request (`MissingApiKeyEnv`, unchanged) but
  still sends the health request, because EggPool keeps `/api/status`
  authenticated even when the dashboard is public. A present-but-unencodable key
  maps to `InvalidSummary` and `InvalidApiKey` respectively and is dropped
  without being sent.
- 401, 403, and 404 are classified separately; 404 is `Unsupported`, not
  `StatsUnavailable`.
- The status route requests a 1 MiB per-request ceiling while the client-wide
  default stays at the 16 KiB summary bound. eggfetch resolves
  `request_limit.or(client_default)`, so the raise is request-local;
  `status_ceiling_is_per_route_and_does_not_widen_the_summary` proves both
  directions (a >32 KiB status payload decodes, an oversized summary is still
  `BodyTooLarge`).

### Workstream C: concurrent independent refresh

- The single worker request task reads both planes with `tokio::join!`, so
  neither delays the other and both are aborted together on supersession or
  deactivation. One `EggpoolResult` now carries `summary` and `health`
  outcomes; partial success is never collapsed. Activation, manual refresh,
  passive 60-second refresh, and period changes all read both planes in the same
  cycle with no second cadence or cache scheduler.

### Workstream D: independent health freshness in `AppState`

- Added `health`, `last_health_success_at`, `last_health_attempt_at`, and
  `last_health_error` to `EggpoolState`, and made
  `apply_eggpool_result_changed` report render-visible change across both planes.
- Rules implemented: success replaces the snapshot and clears the health error; a
  failed refresh keeps the previous snapshot but records the error so the
  renderer marks it stale; a period applies only to the summary payload; the
  existing generation/period rejection still guards both planes, and no second
  generation system was added.

### Workstream E: compact presentation

- Header: `EggPool — <identity>    Health: <ready|degraded|unready|unknown>    Window: <period>`.
  The health token is dropped before the identity or window label is truncated,
  so narrow terminals keep identity/window usability and the four metric rows
  never wrap.
- Footer priority is worker diagnostics, then summary refresh failure, then a
  bounded `Providers: 2 ready · 1 degraded · 1 unavailable` count (or
  `Providers: none reported`). The four metric labels are unchanged and are
  asserted by the renderer tests. Status words carry the signal; color was not
  required.
- No scrolling provider table, account/model list, raw probe error, runtime
  diagnostics, chart, or third pane was added.

### Workstream F: compatibility coverage

- Synthetic loopback matrix: ready/degraded/unready; every provider state and
  observation; public summary with 401 status; summary OK with 404 status;
  summary 404 with status OK; summary malformed with status OK; summary OK with
  status malformed; summary OK with oversized status; per-route ceilings;
  `schema_version != 1`; unknown proxy status; over-long provider ID, reason
  code, negative uptime, and 257 provider rows; unusable credential; invalid
  endpoint; no secret in `Debug`.
- `a_stalled_health_route_does_not_hide_a_summary_failure` proves a status route
  that never answers is a local `Timeout` fact while the summary 503 is
  preserved — transport failure is never rendered as a reported proxy state.
- `worker_delivers_both_planes_and_partial_success` proves one worker result
  carries both planes.

### Workstream G: documentation

- Updated `README.md` (unchanged: the quickstart never described the pane
  contents), `crates/gregg/README.md`, `docs/client.md`,
  `architecture/gregg-client.md`, `.opencode/skills/eggpool/SKILL.md`, and
  `CHANGELOG.md`. `docs/display.md` describes Systems layout only and needed no
  change. `AGENTS.md` needed no new invariant because Plan 151's EggPool
  control rule already forbids conflating local worker state with `EggPool`
  health.
- `plans/056-...md` and `plans/151-...md` keep their records: the summary
  baseline remains valid and Plan 152 is additive post-closure work. `d31d72f`
  and the Phase-61/62 closure records are untouched.

### Verification

- `cargo test -p gregg --all-targets --all-features -- eggpool` (73 tests),
  `-- ui::eggpool` (11 tests), `-- state::tests`, and `-- main::tests` pass.
- `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
  `cargo test --workspace --all-targets --all-features` all pass.
- `./scripts/check-local.sh` passed.
- Ordinary CI is recorded in `plans/README.md` once the workflow run for
  `7b88e43` completes; no new workflow or job was added.

### Scope reconciliation

Only `crates/gregg` plus active documentation and planning records changed.
`greggd`, `gregg-protocol`, `gregg-host`, `gregg-update`, the `EggPool`
repository, the configuration schema, and release/installer workflows are
untouched. No dependency was added, and no second credential field, second
cadence, health-triggered probe, account/model drill-down, database access, or
generic datasource/status framework was introduced.

Future-plan impact: Plan 152 is the last plan in the EggPool corrective line
and unblocks no further plan. Plans 091 and 147 remain independent of it and
keep their existing statuses.
