# Plan 147: eggfetch 0.2.1 patch adoption

Status: planned.

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

- [ ] `Cargo.lock` resolves published `eggfetch-core 0.2.1`.
- [ ] `crates/gregg/Cargo.toml` retains the lean `version = "0.2"`, `default-features = false`, `standard-http1 + tls-rustls` contract unless a documented 0.2.1 requirement forces a narrower manifest correction.
- [ ] No unrelated dependency is opportunistically upgraded.
- [ ] No Gregg application source changes are made unless compilation demonstrates a real 0.2.1 API incompatibility.
- [ ] Systems polling and EggPool polling semantics remain unchanged.
- [ ] Redirects/retries/advanced routing/proxy/Basic/compression/H2/H3 and other excluded EggFetch capabilities remain absent.
- [ ] Timeout, body-limit, DNS/refused/connect, malformed-payload, and Bearer-auth regressions remain green.
- [ ] Current-main and post-update stripped `gregg` sizes are recorded under the same build conditions.
- [ ] Any material footprint change is attributed before closure.
- [ ] Strict clippy, full workspace tests, Rust 1.89 tests, docs, default local checks, and one existing CI run are green.
- [ ] Current-state documentation is truthful about 0.2.1 where it names the resolved patch.
- [ ] Plans 119 and 125 remain unchanged as historical records.
- [ ] `gregg-update` remains on its settled external-`curl` transport; Plan 126 is not reopened.
- [ ] Plan 091 and Plan 148 remain independent.

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

## Handoff note

Start with the single targeted `cargo update`. If the diff is larger than an EggFetch patch refresh, explain why before proceeding. Compile and run the existing semantic tests before touching Rust source. The expected successful implementation is deliberately boring: `Cargo.lock` advances to 0.2.1, the lean feature graph stays identical, and no client code changes.
