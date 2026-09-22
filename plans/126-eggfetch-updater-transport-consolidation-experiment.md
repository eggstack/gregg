# Plan 126: eggfetch updater transport consolidation experiment

Status: complete with result **RETAIN CURL**.

Depends on: completed Plan 125 and the settled self-update contracts from Plans 101-104, 115, and 116.

## Objective

Determine, with an implementation-quality experiment, whether `gregg-update` should replace its external `curl` HTTP transport with `eggfetch-core 0.2` while preserving the exact self-update contract and Gregg's small-binary goals.

This plan is intentionally benchmark/footprint gated. It may close with either:

1. **ADOPT** — eggfetch becomes the shared updater transport and the external `curl` runtime requirement is removed; or
2. **RETAIN CURL** — the experiment demonstrates that the native transport adds disproportionate footprint/complexity or cannot preserve required behavior cleanly.

A no-change result is a valid successful closure. Do not force adoption merely to consolidate on an Eggstack crate.

## Current updater contract

`gregg-update` is shared workspace-internal infrastructure for both `gregg update` and `greggd update`.

Current HTTP ownership in `crates/gregg-update/src/exec.rs` is external `curl`:

- crates.io is the stable-version authority via `crate.max_stable_version`;
- metadata response is bounded to 256 KiB;
- metadata uses a program/version User-Agent;
- metadata network timeout is 15 seconds;
- GitHub release asset and `.sha256` downloads follow redirects;
- release download network timeout is 90 seconds;
- release payload is capped at 64 MiB;
- exact final HTTP 404 for the binary asset is the only network result that permits Cargo fallback;
- checksum 404 is a hard checksum-retrieval error;
- timeout, DNS, TLS, 5xx, malformed status, and other transport failures never fall back to Cargo;
- partial destination files are removed on failure;
- checksum and candidate identity/version are verified before replacement;
- staged replacement and daemon lifecycle ownership remain outside the HTTP transport;
- no internal `sudo` is ever invoked.

External child-process wall-clock bounds currently protect against a hung `curl` executable in addition to curl's own network timeout.

The eggfetch implementation must preserve the application-level contract even though the subprocess-specific failure mode disappears.

## Runtime boundary

Both binaries dispatch update commands synchronously.

- `gregg` creates a Tokio runtime only for the interactive TUI path; normal CLI subcommands, including update, run outside it.
- `greggd` creates a Tokio runtime only for foreground `run`; non-run commands, including update, are synchronous.

Therefore the preferred experiment keeps the existing synchronous `gregg-update` public API and uses a small private current-thread Tokio runtime to drive eggfetch requests.

Do **not** convert `run_simple_update`, `resolve_plan`, `prepare_candidate`, or the daemon update coordination API to async solely for this transport experiment.

The private runtime helper must document and enforce the existing rule that update mechanics are not called from an already-running async runtime.

## Required transport parity

### TLS trust

The updater currently delegates HTTPS verification to the platform curl installation.

For an eggfetch candidate, enable native-root support in addition to Rustls so ordinary platform trust stores remain usable. Do not silently switch the updater to packaged-WebPKI-only trust.

Record the exact trust behavior chosen and any unavoidable difference from curl.

### Redirects

GitHub release asset downloads rely on redirects.

The Plan-125 polling profile deliberately omits redirect following; the updater cannot reuse that behavior unchanged.

Find the smallest supported eggfetch 0.2 feature profile that:

- performs normal HTTP/1.1 HTTPS requests;
- follows redirects for crates.io/GitHub update traffic;
- supports native roots;
- supports explicit environment proxy resolution if proxy parity is retained;
- does not enable unrelated HTTP/2, HTTP/3, compression, cookies, multipart, JSON, or tracing.

Prefer a minimal `standard-http1`-based composition if upstream behavior/tests prove redirects work correctly with that composition. If eggfetch requires the broader `http1` compatibility alias for redirect/proxy support, record the feature expansion instead of hiding it.

### Environment proxy behavior

Curl conventionally honors environment proxy configuration. Removing that behavior is a capability regression for users behind `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, or `NO_PROXY`.

If the eggfetch candidate is adopted, use eggfetch's explicit `ProxyEnvironment` support and test representative proxy/no-proxy resolution. Do not rely on implicit environment reads.

If enabling the eggfetch `proxy` feature materially expands the binary and becomes the reason the experiment fails the footprint gate, record that plainly; do not silently drop proxy parity to make the numbers look better.

### Timeouts

Translate the current curl network budgets into one absolute eggfetch request lifecycle deadline:

- metadata: 15 seconds total;
- asset/checksum download: 90 seconds total.

Retain bounded connect/read/write behavior as needed, but no individual phase may reset or extend the overall total deadline.

Because the transport is in-process, the old 20/100-second child kill/reap margins become obsolete. Remove subprocess-only timeout code only after all curl call sites are gone.

### Response/body bounds

Metadata:

- set a 256 KiB client/request body bound;
- reject over-limit bodies before JSON parsing;
- parse the same `crate.max_stable_version` field;
- preserve stable-version validation.

Binary/checksum download:

- stream response bytes instead of buffering the complete release asset in memory;
- maintain a running byte count;
- fail and remove the partial destination once the release-asset cap would exceed 64 MiB;
- use the same bounded path for fixed-length, chunked, and close-delimited bodies;
- do not enable transparent compression for release assets merely because eggfetch supports it.

If eggfetch exposes a reliable declared-length precheck, it may reject obviously oversized responses early, but streaming accounting remains the authority.

### HTTP status classification

Do not use error-string inspection.

For each request, retain the actual final response status after redirects.

Binary asset:

- 2xx -> success;
- 404 -> `DownloadOutcome::NotFound`;
- every other non-2xx -> `DownloadOutcome::Failed`.

Checksum:

- 2xx -> success;
- 404 -> hard `ChecksumRetrieval` error;
- every other non-2xx -> hard retrieval failure.

Transport/TLS/DNS/timeout failures are always hard failures and never Cargo fallback.

### Partial-file cleanup

Use the existing owner-private staging directory.

On every failed asset/checksum download after destination creation:

- close/drop the file;
- remove the partial path;
- return the appropriate typed update error/outcome.

A retry or fallback must never checksum or execute residue from a prior failed attempt.

## Implementation structure

### A. First build a private eggfetch transport adapter

Keep the updater's higher-level release policy unchanged.

Create a narrow private adapter in `gregg-update` (for example `http.rs` or a small responsibility-owned equivalent) that owns:

- current-thread Tokio runtime construction;
- eggfetch client construction;
- proxy-environment resolution;
- User-Agent request construction;
- bounded metadata fetch;
- bounded streaming file download;
- status-to-updater classification;
- eggfetch-error-to-`UpdateError` mapping.

Do not expose eggfetch types through `gregg-update`'s public API.

Keep:

- `UpdateSpec`;
- `UpdatePlan`;
- `DownloadOutcome`;
- public error categories;
- staging/checksum/candidate validation;
- Cargo fallback;
- replacement;
- daemon lifecycle coordination

source-compatible unless a private helper signature must change.

### B. Preserve a reversible comparison point

Before deleting curl code, record a clean baseline at current main / completed Plan 125:

- `gregg-update` direct dependencies;
- normal dependency-tree line count or equivalent graph snapshot;
- stripped release `gregg` size;
- stripped release `greggd` size;
- updater source LOC for the transport/process helper area;
- updater tests covering curl status/timeout behavior.

Current recorded release sizes before this experiment are approximately:

- `gregg`: 3,740,592 bytes;
- `greggd`: 2,432,408 bytes.

Remeasure the baseline in the same environment rather than relying solely on those historical values.

### C. Exercise both implementations before deciding

During the experiment it is acceptable to keep the curl path behind test-only/private comparison code or a short-lived feature/branch.

Do not ship two selectable production updater transports.

Run equivalent local fixture scenarios against the curl baseline and eggfetch candidate:

1. crates.io metadata 200 within limit;
2. metadata oversized;
3. metadata timeout/stall;
4. release asset direct 200;
5. one or more redirects ending in 200;
6. redirect ending in 404;
7. direct/final 404;
8. 500;
9. connection refused;
10. DNS failure where deterministically testable;
11. body larger than 64 MiB using a bounded/generated fixture;
12. interrupted/truncated body leaves no partial artifact;
13. checksum 404 remains hard failure;
14. proxy route selected from environment;
15. `NO_PROXY` bypass;
16. invalid proxy environment fails closed/redacted;
17. User-Agent reaches the fixture unchanged.

Use local deterministic servers/proxies; do not make ordinary tests depend on crates.io or GitHub availability.

### D. Measure the binary/dependency cost before landing

This is the decisive gate because `greggd` currently does not link eggfetch/Rustls in production.

Measure stripped release sizes for both binaries with:

1. completed Plan-125 baseline;
2. eggfetch updater candidate.

Also record:

- added/removed direct dependencies in `gregg-update`;
- whether `eggfetch-http-connect`, Base64, retry/backoff, native-root, or proxy dependencies enter the daemon graph;
- whether existing Axum/Hyper/Tokio dependencies unify or whether new TLS/network stacks dominate the delta;
- source complexity removed from `exec.rs` (curl discovery/status parsing/child pipe handling);
- any new runtime/proxy adapter complexity.

### E. Adoption decision

Adopt the eggfetch updater only if all behavioral parity tests are green and the footprint/maintenance trade is acceptable for Gregg's small-daemon goal.

A stripped `greggd` increase greater than **5% and at least 128 KiB** is a stop-and-review threshold, not an automatic acceptance. Because external curl contributes zero linked bytes today, such growth requires a concrete maintenance/reliability justification in the closure record. If the broader redirect/native-root/proxy feature set produces a substantially larger increase, prefer RETAIN CURL.

If RETAIN CURL:

- revert the candidate dependency/source changes;
- keep any transport-neutral tests that improve the existing updater contract if they are useful and do not add maintenance burden;
- record the measured reason;
- leave Plan 125's polling-client eggfetch adoption untouched.

If ADOPT:

- remove `find_curl`, curl capture/probe/download helpers, child-process pipe code that exists only for curl, and the `CurlMissing` error if no public/internal compatibility requirement still needs it;
- retain generic bounded child execution for Cargo fallback and candidate-version probes;
- remove curl installation/runtime requirements from docs;
- document proxy/native-root behavior;
- keep the synchronous updater API.

Do not delete generic process-timeout machinery used by Cargo fallback or candidate validation.

## Tests required for adoption

In addition to the parity fixture matrix, preserve or add regressions proving:

- only exact final 404 permits Cargo fallback;
- 404 text inside a 500 body never permits fallback;
- redirects do not change fallback classification except by their final status;
- body limit applies while streaming and partial files are removed;
- timeout during body streaming is hard failure;
- checksum retrieval cannot trigger Cargo fallback;
- metadata remains capped and validated;
- proxy credentials are never rendered in errors/debug output;
- no update path runs inside the daemon's foreground runtime;
- update commands remain synchronous at both binary dispatch boundaries;
- staged candidate verification and replacement behavior are unchanged.

## Verification

Candidate checks:

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p gregg-update
cargo test -p gregg
cargo test -p greggd
cargo test --workspace --all-targets --all-features
cargo +1.89 test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

For an ADOPT result, also run the release local preflight because update/install-facing behavior and dependencies change:

~~~text
./scripts/check-local.sh --release
~~~

Remeasure stripped `gregg` and `greggd` under the established release profile.

Run one ordinary existing CI workflow after the final chosen state. Do not add a permanent benchmark workflow or network-dependent CI test.

## Acceptance criteria

- [x] A private eggfetch 0.2 updater adapter is exercised against deterministic local fixtures before any curl code is deleted.
- [x] Existing synchronous `gregg-update` public APIs remain synchronous and source-compatible.
- [x] No nested-runtime path is introduced.
- [x] crates.io metadata authority, User-Agent, stable-version validation, and 256 KiB cap are preserved.
- [x] GitHub release/checksum redirects work.
- [x] Exact final HTTP 404 remains the only asset result permitting Cargo fallback.
- [x] Checksum 404 and all transport/TLS/DNS/timeout/5xx failures remain hard failures.
- [x] Release downloads remain capped at 64 MiB while streaming and leave no partial file on failure.
- [x] Platform/native trust behavior is documented and tested where deterministic.
- [x] Environment proxy / NO_PROXY behavior is preserved explicitly or the experiment closes RETAIN CURL rather than silently regressing it.
- [x] Baseline and candidate dependency graphs are recorded.
- [x] Baseline and candidate stripped `gregg` and `greggd` sizes are recorded under the same conditions.
- [x] Any material daemon footprint increase receives an explicit adoption/rejection rationale.
- [ ] ~~If ADOPT, curl-only process/discovery/status machinery and docs are removed without touching Cargo/candidate subprocess bounds.~~ N/A — closed RETAIN CURL.
- [x] If RETAIN CURL, the candidate is reverted cleanly and the measured reason is recorded.
- [ ] Full local checks, Rust 1.89 checks, and one ordinary CI run pass at the final chosen state.
- [x] The closure record states one explicit result: ADOPT or RETAIN CURL.

## Explicit non-goals

Do not include:

- updater API redesign;
- async CLI subcommands;
- daemon lifecycle/restart changes;
- installer ownership changes;
- release asset naming changes;
- checksum algorithm changes;
- signature/TUF/Sigstore work;
- automatic update scheduling;
- HTTP/2 or HTTP/3;
- response compression;
- retry/backoff policy changes;
- new public configuration for transport selection;
- two production updater implementations;
- permanent performance CI.

## Closure record

Result: **RETAIN CURL** (no-change successful closure).

An implementation-quality eggfetch 0.2 adapter was built, exercised against
deterministic local fixtures alongside the curl baseline, measured, and then
reverted. The updater contract is unchanged; external `curl` remains the
update transport.

### What was built and proven (then reverted)

A private synchronous adapter (`gregg-update/src/http.rs`, since removed)
drove eggfetch 0.2 on a per-call current-thread Tokio runtime (nested
runtimes refused fail-closed via a `Handle::try_current` guard, never a
nested runtime), with program `User-Agent`, absolute five-field total
deadlines (15 s metadata / 90 s downloads), redirects followed (max 50,
curl parity), transparent decompression disabled, no logical retry, explicit
`ProxyEnvironment::from_env` routing (invalid values fail closed with fully
redacted errors), default native-roots-with-WebPKI-fallback TLS, request
`max_decoded_body_size` for metadata, and streaming asset downloads with a
running 64 MiB cap, declared-length precheck, and partial-file removal on
every failure path (handle dropped before removal for Windows). Failure
mapping used typed eggfetch evidence only (`is_timeout`, `Error::*`,
`network_failure_kind`); no error strings were inspected and proxy URLs
never entered messages. `UpdateSpec`/`UpdatePlan`/`DownloadOutcome`/public
error categories and the staged checksum/candidate/Cargo/lifecycle flow
were untouched.

Fixture results before revert (all green): metadata 200/oversized/stall/
500, asset direct-200/redirect→200/redirect→404/direct-404/500/refused/DNS/
declared-oversize/close-delimited 65 MiB flood/chunked-200/truncated/slow-
trickle-timeout, proxy route selection, `NO_PROXY` bypass, invalid-proxy
fail-closed with redaction, `User-Agent` capture on both paths, and nested-
runtime refusal — 23 adapter tests green, plus 6 equivalent curl-baseline
fixture tests green (real `curl` against the same local servers: direct 200,
redirect→200, 404→`NotFound`, 500 hard failure, metadata capture 200,
metadata oversize rejection).

### Footprint gate (decisive)

Parity requires `redirects` + `tls-native-roots` + `proxy`. The `proxy`
feature pulls the broad `http1` alias (`native-http1`/`advanced-routing`,
`logical-retry`, `redirects`, `basic-auth`) plus `eggfetch-http-connect`,
so the candidate graph contains exactly the capabilities Plans 118/119/125
excluded, and the daemon gains its first TLS stack (`rustls`/`ring`/
`rustls-webpki`/`webpki-roots`/`hyper-rustls`/`tokio-rustls`/
`rustls-native-certs`), `base64`, `getrandom 0.2`, `httpdate`, `url`, and a
second `thiserror` major line. `gregg-update` tree: 39 → 233 lines.

Stripped fat-LTO release sizes, same profile/target/toolchain:

| Binary | Baseline (Plan-125 HEAD) | Candidate | Delta |
|--------|--------------------------|-----------|-------|
| `gregg` | 3,740,592 | 5,510,088 | +1,769,496 (+47.3%) |
| `greggd` | 2,432,408 | 4,989,488 | +2,557,080 (+105.1%) |

(`gregg` links `gregg-update` too, so both binaries embed the updater; no
separate-build unification artifact.) The stop-and-review threshold (>5%
**and** ≥128 KiB on `greggd`) is exceeded ~20×. No maintenance/reliability
justification outweighs doubling the small daemon for a rarely-run command:
curl stays a documented runtime requirement either way, updater error detail
would become less verbose (fixed categories instead of curl stderr), and
trust behavior would shift subtly (rustls+platform-roots vs the platform
verifier curl delegates to). A reduced no-proxy configuration was not
measured because it cannot satisfy the plan's proxy-parity requirement, so
no compliant configuration can pass the gate.

Measurement hygiene note: the first post-revert `greggd` link measured
2,497,960 (+65,552); a forced rebuild reproduced the exact baseline
2,432,408, and `gregg` reproduced 3,740,592 exactly, confirming a stale
incremental link rather than a source delta. Candidate sizes are single
single-build measurements; any staleness there could only understate growth
(stale lean objects), so 4,989,488 is a floor and the 20× margin is robust.

### What was kept

- `exec::parse_stable_version_response` extraction with 6 unit tests
  (previously inline, untestable): valid/missing/empty/non-stable/invalid-
  JSON/oversized.
- 6 hermetic curl-baseline fixture tests (`exec::tests::curl_baseline`)
  with a proxy-env sanitizing guard and background-thread servers: direct
  200, redirect→200, 404→`NotFound`, 500 hard failure, metadata capture,
  metadata oversize. Plus a `tokio` dev-dependency (test-only; zero release
  footprint — production tree is back to 39 lines and both release binaries
  reproduce their baselines byte-for-byte).
- Plan 125's polling-client adoption is untouched.

### Revert verification

`cargo fmt --check`, strict clippy, full workspace tests on stable and
`+1.89` (41 `gregg-update` tests green), `cargo doc --no-deps`, and
`./scripts/check-local.sh` all green at the final state; daemon graph
contains zero eggfetch/rustls; release sizes reproduce baselines exactly.
