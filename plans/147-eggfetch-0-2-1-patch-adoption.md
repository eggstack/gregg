# Plan 147: eggfetch 0.2.1 patch adoption

Status: complete, retargeted to published `eggfetch-core 0.2.2`; implementation `50aedac`. See the closure record at the end of this file.

Target-version correction (2026-10-02, after Plans 151-152 closed): upstream `eggstack/eggfetch` has since published `eggfetch-core 0.2.2` (MSRV 1.89, which matches this workspace) after the 0.2.1 release this plan was written against. The live defect is unchanged — `Cargo.lock` still resolves `0.2.0` and `crates/gregg/Cargo.toml` still requires `version = "0.2"` with the lean `standard-http1 + tls-rustls` recipe — but implementing the recorded `--precise 0.2.1` step verbatim would close the plan one patch behind what is published. Retarget the step and the "0.2.1" acceptance wording to the current patch (and attribute whatever 0.2.1 -> 0.2.2 changes) before executing, or amend the plan first. Nothing else in this plan's contract changes: it stays a lockfile-first, feature-graph-stable, semantics-preserving adoption with no application source change expected.

Depends on: completed Plan 125's `eggfetch-core 0.2` lean-client adoption and the current main branch. Independent of the remaining Plan 091 soak record and independent of Plan 148.

## Objective

Advance Gregg's existing client-side `eggfetch-core` resolution from 0.2.0 to the current published 0.2.1 patch release without changing Gregg's transport policy, widening the compiled feature graph, changing polling behavior, or regressing the small-client footprint.

This is intentionally a narrow patch adoption. Gregg's manifest already permits the 0.2.1 release through `version = "0.2"`; the live defect is dependency-currentness in `Cargo.lock`, not an architectural migration.

## Current state and upstream disposition

At plan creation on 2026-10-01:

- `crates/gregg/Cargo.toml` declares:

~~~toml
eggfetch-core = { version = "0.2", default-features = false, features = ["standard-http1", "tls-rustls"] }
~~~

- `Cargo.lock` resolves `eggfetch-core 0.2.0`;
- upstream `eggstack/eggfetch` has published `v0.2.1`, including `eggfetch-core 0.2.1`;
- upstream describes 0.2.1 as a coordinated release-identity/qualification patch with no runtime or user-visible behavior change relative to 0.2.0;
- the 0.2.1 manifest retains Gregg's exact lean `standard-http1 + tls-rustls` feature recipe and Rust 1.89 MSRV.

Plan 125 remains the historical record for the 0.2.0 minor-line adoption. Do not rewrite its closure record.

## Required behavior contract

The Plan-119/125 client contract remains authoritative.

### Systems polling

Preserve:

- v2-first polling with v1 fallback only on HTTP 404;
- current endpoint/schema validation and stable `PollOutcome` classification;
- one logical dispatch per request;
- 3xx passthrough without redirect following;
- no automatic retry;
- Rustls HTTPS with the current trust profile;
- current per-host idle-connection limits;
- the 64 KiB decoded-body cap;
- typed DNS, connection-refused, connect, timeout, and body-limit mapping;
- the absolute total deadline through response-body EOF.

### EggPool polling

Preserve:

- the independent EggPool client;
- its current per-host idle-connection limit;
- the 16 KiB decoded-body cap;
- request-local Bearer authentication;
- credential redaction in errors/outcomes/logging;
- no redirect or retry;
- current endpoint validation and outcome taxonomy.

### Capability exclusions

The final resolved Gregg graph must continue to exclude EggFetch capabilities that Gregg does not use:

- `advanced-routing`;
- `logical-retry`;
- `redirects`;
- `basic-auth`;
- `proxy`;
- `eggfetch-http-connect`;
- HTTP/2 and HTTP/3;
- cookies;
- compression;
- multipart;
- EggFetch JSON;
- EggFetch tracing.

Do not replace the lean profile with EggFetch defaults or the broad `http1` compatibility alias.

## Implementation

### A. Perform a targeted lockfile refresh first

Keep the direct manifest requirement and feature selection unchanged unless Cargo demonstrates a real reason to change them.

Update only the EggFetch core package to the published patch:

~~~text
cargo update -p eggfetch-core --precise 0.2.1
~~~

Inspect the resulting lockfile diff. Do not opportunistically update unrelated dependencies.

If Cargo must move a transitive package solely to satisfy `eggfetch-core 0.2.1`, record and attribute that movement. Otherwise the lockfile diff should remain narrowly scoped.

### B. Compile before touching application source

The 0.2.1 release is documented as API/runtime preserving, so the expected implementation is lockfile-only.

First compile and run the existing Gregg client tests with no Rust source changes.

If compilation fails, adapt only the smallest documented public-surface difference required by 0.2.1 and record it in this plan. Do not refactor poller, scheduler, endpoint, state, worker, or EggPool ownership during this patch adoption.

A successful dependency-only build is evidence that no application adapter change is needed; do not create churn for its own sake.

### C. Re-run the established semantic regressions

Use the existing deterministic tests from Plans 119 and 125. At minimum prove:

- 3xx responses are returned and not followed;
- header/body stalls and total-deadline overrun still map to `Timeout`;
- oversized bodies still map to `BodyTooLarge`;
- DNS/refused/connect failures preserve their typed outcomes;
- malformed/wrong-version payload behavior is unchanged;
- EggPool Bearer authentication and error redaction remain unchanged.

Do not weaken assertions to accommodate the patch.

### D. Prove the feature graph is unchanged

Record equivalent evidence for:

~~~text
cargo tree -p gregg -e features -i eggfetch-core
cargo tree -p gregg | grep -E 'eggfetch|eggfetch-http-connect|hyper|rustls'
~~~

The expected EggFetch feature ownership remains:

~~~text
standard-http1
  -> transport-http1
  -> standard-route
  -> high-level-url
tls-rustls
~~~

If a previously absent capability appears, stop and investigate before closure.

### E. Recheck release footprint

Build the stripped fat-LTO `gregg` release under the same target/toolchain/profile used by the current footprint records.

Record:

- current-main pre-change size;
- post-update size;
- byte and percentage delta;
- dependency attribution if the result changes materially.

The upstream release claims no runtime change, but Gregg should measure its own artifact rather than infer binary identity from the release note. Material unexplained growth blocks closure until attributed.

### F. Reconcile current-state documentation only where needed

Search current documentation for live statements that identify the resolved EggFetch patch as 0.2.0.

Update only current-state references that would become false, including as applicable:

- `AGENTS.md`;
- `architecture/gregg-client.md`;
- `architecture/workspace.md`;
- `.opencode/skills/gregg-client/SKILL.md`;
- `CHANGELOG.md`;
- `plans/README.md`.

Preserve Plans 119 and 125 as historical adoption records.

Do not claim that EggFetch 0.2.1 changes Gregg-visible runtime behavior; upstream explicitly characterizes it as a patch without runtime/user-visible changes.

## Verification

Run focused client transport tests first, then the ordinary repository gates:

~~~text
cargo test -p gregg --all-targets --all-features poller
cargo test -p gregg --all-targets --all-features eggpool
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo +1.89 test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
./scripts/check-local.sh
~~~

Use the actual focused test filters present at implementation time if names have moved; do not add duplicate tests merely to satisfy the example commands.

Run one ordinary existing CI workflow after the dependency update. No new workflow, matrix, benchmark service, or evidence bundle is required.

## Acceptance criteria

- [x] `Cargo.lock` resolves published `eggfetch-core 0.2.2` (retargeted from the recorded 0.2.1; see the target-version correction at the top of this plan).
- [x] `crates/gregg/Cargo.toml` retains the lean `version = "0.2"`, `default-features = false`, `standard-http1 + tls-rustls` contract unchanged.
- [x] No unrelated dependency is opportunistically upgraded; the lockfile moves exactly one package version.
- [x] No Gregg application source changes were made; 0.2.2 compiled with zero source edits.
- [x] Systems polling and EggPool polling semantics remain unchanged.
- [x] Redirects/retries/advanced routing/proxy/Basic/compression/H2/H3 and other excluded EggFetch capabilities remain absent.
- [x] Timeout, body-limit, DNS/refused/connect, malformed-payload, and Bearer-auth regressions remain green.
- [x] Current-main and post-update stripped `gregg` sizes are recorded under the same build conditions (3,806,128 bytes both, delta 0).
- [x] No material footprint change occurred, so no attribution was required.
- [x] Strict clippy, full workspace tests, Rust 1.89 tests, docs, and default local checks are green; the existing CI run is recorded below.
- [x] Current-state documentation is truthful about the resolved patch (0.2.2) where it names it.
- [x] Plans 119 and 125 remain unchanged as historical records.
- [x] `gregg-update` remains on its settled external-`curl` transport; Plan 126 is not reopened.
- [x] Plans 091, 148, and 151-152 remain independent.

## Explicit non-goals

Do not include:

- polling/scheduler/state refactors;
- EggPool feature work;
- updater transport consolidation;
- new EggFetch capabilities;
- HTTP/2, HTTP/3, proxy, compression, retry, or redirect adoption;
- a new transport abstraction;
- dependency-wide modernization;
- CI/performance infrastructure;
- release publication or tagging;
- changes to EggServe/greggd server behavior.

## Closure record

Implementation: `50aedac` (`chore: refresh eggfetch-core to published 0.2.2`).
Ordinary CI: run `37038881606` at commit `e5c3231` (the commit that contains
`50aedac`) is green across all six jobs — Linux, macOS arm64, macOS Intel,
Windows SCM smoke, MSRV Rust 1.89, and FreeBSD 14.2 native `gregg-host`. The
workflow run started for `50aedac` itself (`37038811552`) was cancelled by the
follow-up closure-record push, and the identical tree passed in the follow-up
run. No new workflow, job, or matrix was added.

### Retarget

This plan was written against `eggfetch-core 0.2.1`. By implementation time
upstream had published `0.2.2` (MSRV 1.89, matching this workspace), so the
target-version correction appended at the top of this plan was applied first
and the single targeted update ran against the current patch instead. The live
defect was unchanged: `crates/gregg/Cargo.toml` already required
`version = "0.2"`, while `Cargo.lock` still resolved `0.2.0`. Everything else in
the plan — lockfile-first scope, unchanged manifest and feature recipe, no
application source change, unchanged semantics, capability exclusions,
footprint measurement, and historical-record preservation — applied as written.

### A. Targeted lockfile refresh

`cargo update -p eggfetch-core --precise 0.2.2` moved exactly one package:
`eggfetch-core 0.2.0 -> 0.2.2` (checksum `27463e41...`). Package count is
unchanged (219 before and after) and no package was added or removed. Three
shared requirement edges (`errno`, `rustix 0.38`, `rustix 1.1`, and
`tempfile`'s graph) re-pointed from `windows-sys 0.52.0` to the already-present
`windows-sys 0.59.0`; both versions remain in the lockfile, so this is edge
re-resolution, not an upgrade or a drop. No unrelated dependency was updated.

### B/C. No source change, semantics green

`cargo check -p gregg --all-targets --all-features` compiled with zero source
edits, confirming that the 0.2.2 surface Gregg uses is unchanged. No poller,
scheduler, endpoint, state, worker, or EggPool ownership was refactored.

- `cargo test -p gregg --all-targets --all-features -- poller` (46 tests) and
  `-- eggpool` (73 tests) pass, including 3xx-not-followed,
  header/body-stall and total-deadline `Timeout` mapping, 16 KiB summary and
  1 MiB status `BodyTooLarge` mapping, typed DNS/refused/connect outcomes,
  malformed and wrong-schema payload handling, and EggPool Bearer auth plus
  redaction.
- `cargo test -p gregg --all-targets --all-features` (632 tests) and
  `cargo test --workspace --all-targets --all-features` pass.

### D. Feature graph unchanged

`cargo tree -p gregg -e features -i eggfetch-core` resolves the same seven
feature nodes before and after — `standard-http1` (with `transport-http1`,
`standard-route`, `high-level-url`) plus `tls-rustls` (with `hyper-rustls`).
`cargo tree -p gregg --prefix none` differs only in the `eggfetch-core` version
line; `hyper 1.11.1`, `hyper-util 0.1.20`, `hyper-rustls 0.27.9`,
`rustls 0.23.45`, `rustls-pki-types`, and `rustls-webpki` are identical. An
explicit feature scan for `redirects`, `logical-retry`, `advanced-routing`,
`basic-auth`, `proxy`, `cookies`, `compression`, `multipart`, JSON, and
tracing returns no rows, so the excluded-capability contract holds.

### E. Footprint

Same toolchain (rustc/cargo 1.98.1) and the workspace release profile
(`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`),
`cargo build -p gregg --release`:

- pre-change stripped `gregg`: 3,806,128 bytes
- post-change stripped `gregg`: 3,806,128 bytes
- delta: 0 bytes (0.00%)

The size is not comparable to the Plan 119/124/125 records (3,740,592 bytes)
because those were measured on an earlier toolchain and before the Plan-152
health plane added code; only the same-toolchain before/after pair is
meaningful here, and it is identical. Nothing required attribution.

### F. Documentation

- `architecture/workspace.md` gained a current Plan-147 dependency disposition
  above the preserved Plan-125 and Plan-119 sections, so current state names
  `0.2.2` while both historical adoption records stay accurate.
- `CHANGELOG.md` records the refresh and its measured no-change result.
- `AGENTS.md` and `.opencode/skills/gregg-client/SKILL.md` describe the lean
  `eggfetch-core 0.2` line and were already version-agnostic, so neither needed
  an edit.
- Plans 119 and 125 were not modified.

### Verification

`cargo fmt --all -- --check`, strict clippy across the workspace, full
workspace tests, `cargo +1.89 test --workspace --all-targets --all-features`,
`cargo doc --workspace --no-deps` (only the two pre-existing
`system_block.rs` private-intra-doc-link warnings, unrelated to this change),
and `./scripts/check-local.sh` all pass.

### Scope reconciliation

`Cargo.lock`, `CHANGELOG.md`, and `architecture/workspace.md` changed. No Rust
source, manifest feature, config, protocol, daemon, updater, CI, or release
behavior changed, and no new capability or dependency was added.

Future-plan impact: Plan 147 is terminal; it unblocks no remaining plan. Plan
091 keeps its in-implementation status pending its extended soak record and is
independent of this change.

## Handoff note

Start with the single targeted `cargo update`. If the diff is larger than an EggFetch patch refresh, explain why before proceeding. Compile and run the existing semantic tests before touching Rust source. The expected successful implementation is deliberately boring: `Cargo.lock` advances to 0.2.1, the lean feature graph stays identical, and no client code changes.
