# Plan 154: EggPool canonical status fixture evidence cleanup

Status: complete.

Depends on: completed Plan 153/current main. Independent of the remaining Plan 091 soak record.

Upstream evidence reviewed against: `eggstack/eggpool` main `43c987ea458bd563d5108fd8051ad31185704bb0` (2026-10-02), especially `rust/src/operations/status.rs::{ProxyStatusSnapshot, ProxyHealthSummary, ProviderHealthSummary, RuntimeHealthSummary}`.

## Objective

Make Plan 153's provenance fixture literally match EggPool's canonical schema-v1 serialized shape for both consumed and deliberately ignored fields.

Plan 153 fixed the functional defect: Gregg now consumes the correct nested proxy account counts, `provider_id`, `last_observation`, and producer-owned bounds, and those production fields still match current EggPool main. Post-closure review found only an evidence-quality issue: the fixture/commentary says it carries canonical ignored runtime/provider fields, but some ignored sample fields were invented locally.

Correct that evidence and wording without changing the production decoder, normalized health model, transport, state, worker, rendering, configuration, dependencies, or API behavior.

## Confirmed evidence mismatch

The passing Plan-153 fixture currently uses ignored examples such as:

~~~json
{
  "runtime": {"pid": 4242, "started_at": "2026-10-02T11:00:00Z"},
  "providers": [{
    "account_count": 2,
    "last_probe": {"checked_at": "2026-10-02T11:59:58Z", "latency_ms": 31}
  }]
}
~~~

Those fields are ignored by Gregg, so they do not affect functional decoding. They are not, however, the fields EggPool's current schema-v1 structs serialize.

Current EggPool schema v1 defines the ignored structures as:

~~~text
RuntimeHealthSummary {
    generation: Option<u64>,
    digest_prefix: String,
    reload: String,
    tasks: String,
    db: String,
    retiring: usize,
}

ProviderHealthSummary {
    provider_id: String,
    status: ProviderStatus,
    enabled_accounts: usize,
    total_accounts: usize,
    routable_accounts: usize,
    backoff_accounts: usize,
    unavailable_accounts: usize,
    model_count: Option<usize>,
    last_probe_age_seconds: Option<u64>,
    last_probe_latency_ms: Option<u64>,
    last_probe_status_code: Option<u16>,
    last_observation: ProviderObservation,
    reason_code: Option<String>,
}
~~~

`ProxyStatusSnapshot.observed_at` and the already-used `ProxyHealthSummary` fields are correct in the Plan-153 fixture.

## Governing invariants

1. Production schema-v1 decoding is already correct and must not change unless inspection proves a separate bug.
2. `EggpoolStatusWire`, `normalize_health`, status/summary body ceilings, status classifications, and Plan-151/152 worker behavior are out of scope.
3. The canonical fixture must represent the actual serialized field names and nesting of current EggPool schema v1, including fields Gregg ignores.
4. The fixture may use synthetic *values* but not synthetic *field names or structure* while calling itself canonical.
5. Gregg should continue to ignore canonical fields it does not model.
6. The negative regression for the never-upstream Plan-152 `id` / `observation` / root-count shape remains unchanged.
7. Producer bounds remain 256 provider rows, 96-byte provider IDs, and 64-byte reason codes.
8. No EggPool dependency, networked test, generated binding, shared crate, workflow, or cross-repository orchestration is added.

## Workstream A: make the canonical fixture structurally canonical

Update `canonical_status_body` and the dedicated upstream-shaped regression payload so representative ignored fields use the exact current EggPool schema names.

Use a runtime object shaped like:

~~~json
{
  "generation": 17,
  "digest_prefix": "0123456789ab",
  "reload": "idle",
  "tasks": "4/4",
  "db": "ok",
  "retiring": 0
}
~~~

Use provider details shaped like:

~~~json
{
  "enabled_accounts": 2,
  "total_accounts": 2,
  "routable_accounts": 1,
  "backoff_accounts": 1,
  "unavailable_accounts": 1,
  "model_count": 3,
  "last_probe_age_seconds": 2,
  "last_probe_latency_ms": 31,
  "last_probe_status_code": 200,
  "reason_code": "slow"
}
~~~

Keep the canonical consumed fields alongside them: `provider_id`, `status`, `last_observation`, proxy status/availability/model/account counts, and schema version.

Do not add fields that are not present in the upstream structs merely to exercise unknown-field tolerance. Unknown-field tolerance is already a serde property and is not the purpose of the canonical provenance fixture.

## Workstream B: strengthen evidence assertions without expanding product semantics

The upstream-shaped regression should assert:

- the canonical payload still decodes as `EggpoolHealthFetchOutcome::Online`;
- consumed proxy/provider values still normalize correctly;
- exact canonical ignored runtime/provider fields do not cause decode failure;
- no production type is expanded merely to retain ignored runtime/probe/account details;
- the obsolete Plan-152 shape remains rejected by the existing negative test.

Do not assert on ignored field values by adding them to `EggpoolHealthSnapshot`. The evidence goal is compatibility, not feature expansion.

## Workstream C: consolidate fixture provenance

Keep one authoritative provenance comment for the passing schema fixture and make it exact:

~~~text
repo:          eggstack/eggpool
commit:        43c987ea458bd563d5108fd8051ad31185704bb0
type:          rust/src/operations/status.rs::ProxyStatusSnapshot
nested types:  ProxyHealthSummary / ProviderHealthSummary / RuntimeHealthSummary
serialization: serde JSON
~~~

If implementation prefers retaining Plan 153's original `299a0b...` provenance because that is the commit the functional fix was qualified against, that is acceptable only if the exact serialized structs at that commit are verified to have the same fields. Otherwise refresh the provenance to `43c987ea...` and record why.

Do not imply that a fixture was captured byte-for-byte from a live EggPool response unless it actually was. `Structurally canonical synthetic fixture` is the truthful description if values remain test-authored.

## Workstream D: planning and documentation reconciliation

Append a post-closure evidence note to Plan 153 rather than rewriting its closure record. State explicitly that:

- Plan 153's production wire correction remains valid;
- only the ignored-field examples in its provenance fixture/commentary were overclaimed as canonical;
- Plan 154 owns the evidence cleanup.

Update `plans/README.md` to register Plan 154 and add `153 -> 154`. While Plan 154 is open, Plans 091 and 154 are the in-progress plans.

Update `.opencode/skills/eggpool/SKILL.md` so the canonical ignored-field list uses EggPool's actual runtime/provider field names and records Plan 154 as the active evidence cleanup.

At implementation closure, update `architecture/gregg-client.md` only if its wording implies more provenance fidelity than the corrected fixture demonstrates. User-visible docs should not need changes because runtime behavior is unchanged.

## Expected implementation surface

Likely:

~~~text
crates/gregg/src/eggpool.rs              # tests/fixtures/comments only
.opencode/skills/eggpool/SKILL.md
plans/153-eggpool-schema-v1-wire-contract-corrective-pass.md
plans/154-eggpool-canonical-status-fixture-evidence-cleanup.md
plans/README.md
architecture/gregg-client.md             # only if exact provenance wording needs reconciliation
~~~

Not expected:

~~~text
production EggpoolStatusWire / normalize_health behavior
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

Any production decoder/state/UI change requires a concrete newly discovered defect and should be split from this evidence-only pass.

## Verification

Focused:

~~~text
cargo test -p gregg --all-targets --all-features -- eggpool
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Also inspect the canonical fixture text directly and confirm all representative ignored fields exist in the pinned EggPool structs. A simple source-level assertion/search is acceptable; do not add an EggPool build dependency just to mechanically generate JSON.

Use ordinary existing CI for hosted closure. No new job/matrix.

## Acceptance criteria

- [x] The canonical status fixture contains no runtime/provider field name that is absent from the pinned EggPool schema-v1 structs unless explicitly labeled as a deliberate unknown-field test outside the canonical fixture.
- [x] `runtime` uses `generation`, `digest_prefix`, `reload`, `tasks`, `db`, and `retiring`.
- [x] Provider ignored detail uses canonical account/model/probe/reason fields: `enabled_accounts`, `total_accounts`, `routable_accounts`, `backoff_accounts`, `unavailable_accounts`, `model_count`, `last_probe_age_seconds`, `last_probe_latency_ms`, `last_probe_status_code`, and `reason_code`.
- [x] Canonical consumed fields remain `provider_id`, `status`, `last_observation`, and proxy-nested account counts.
- [x] The canonical fixture still decodes `Online` without expanding Gregg's normalized health model to retain ignored fields.
- [x] The Plan-152 never-upstream shape remains rejected.
- [x] Production decoder, worker, state, UI, auth, cadence, body limits, and bounds are unchanged.
- [x] Fixture provenance names the exact EggPool commit/type(s) actually inspected and does not claim byte-for-byte capture unless performed.
- [x] Plan 153 receives an append-only evidence correction note; its functional closure history is preserved.
- [x] `plans/README.md` registers Plan 154 and `153 -> 154`, with truthful active-plan status.
- [x] Focused/local checks pass.
- [x] Ordinary CI passes.
- [x] No dependency/config/API/workflow/release or EggPool-repository change is introduced.

## Stop conditions

Stop and split a new corrective plan if implementation discovers any mismatch in a field Gregg actually consumes, any new producer-bound drift, or any behavior defect in the Plan-151/152/153 production path. Plan 154 itself must remain evidence/test cleanup only.

## Closure record

Completed locally:

- Replaced the ignored runtime example in `canonical_status_body` and the dedicated upstream-shaped regression with EggPool's `RuntimeHealthSummary` field names.
- Replaced the invented provider `account_count` / nested `last_probe` examples with `ProviderHealthSummary` account, model, probe-age/latency/status, and reason fields.
- Refreshed fixture provenance to EggPool commit `43c987ea458bd563d5108fd8051ad31185704bb0`, identifying `ProxyStatusSnapshot` and all three nested summary types. The fixture is documented as structurally canonical with synthetic values, not a live byte capture.
- Kept consumed fields, normalized health assertions, and the negative never-upstream Plan-152 regression. No production code or normalized-model fields changed.
- Reconciled the EggPool skill and plan index; Plan 091 remains independent and is still gated only on its extended soak record. Plan 154 unblocks no future plan.
- Inspected the pinned upstream source and confirmed the representative ignored field names against the serialized structs.
- Passed `cargo test -p gregg --all-targets --all-features -- eggpool` (75 library + 4 binary tests), `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-targets --all-features`, and `./scripts/check-local.sh`.

Implementation landed in commit `fd2491838eae5b2f168abc0633b10586c690bf23`.
Existing CI run `37053403192` passed all six jobs at that commit: Linux,
macOS arm64, macOS Intel, Windows SCM smoke, MSRV Rust 1.89, and FreeBSD
14.2 native `gregg-host`. No new workflow, job, or matrix was added.

### Scope and future-plan reconciliation

Only `crates/gregg/src/eggpool.rs`, `.opencode/skills/eggpool/SKILL.md`,
`plans/README.md`, and planning records changed. Production decoding, the
normalized health model, worker, state, UI, auth, cadence, body ceilings,
bounds, dependencies, configuration, API behavior, workflows, release
machinery, and the EggPool repository were untouched. All stop conditions
remained clear.

Plan 153's production correction remains valid; its ignored-field fixture
examples alone were overclaimed as canonical. Plan 154 owns and closes that
evidence correction. Future-plan review found no plan blocked by Plan 154 and
no plan newly unblocked by it. Plan 091 remains the only in-progress plan,
gated solely on its own extended soak record.
