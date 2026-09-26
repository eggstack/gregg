# Plan 135: greggd gregg-host adapter cutover and qualification

Status: planned.

Depends on: completed Plans 133-134.

Blocks: Plan 136.

## Objective

Make `greggd` consume the extracted `gregg-host` implementation as the sole production native telemetry source while preserving current `greggd` public collector paths, v1/v2 protocol behavior, readiness, cadence, failure isolation, native behavior, and Rust 1.89 support.

This plan closes the extraction campaign for existing Linux/macOS/Windows platforms. It may correct extraction regressions discovered by qualification, but must not add new telemetry or change intended semantics.

## Compatibility facade

Keep `greggd::collector` as a compatibility facade during this phase.

Where practical, re-export extracted types at the old paths:

~~~rust
pub mod linux {
    pub use gregg_host::linux::*;
}
~~~

and equivalently for macOS, Windows, and collector errors.

Where direct re-export would expose a protocol-neutral type in place of an existing `gregg_protocol` type, retain a thin Gregg-owned compatibility type/adapter rather than breaking source compatibility.

Do not delete `greggd::collector` merely because the implementation now lives elsewhere.

## Gregg-owned adapter

Gregg-specific conversion stays in `greggd`.

The adapter must map:

~~~text
gregg_host::HostIdentity
gregg_host::HostCapabilities
gregg_host::HostSample
gregg_host::CollectError
        |
        v
current gregg-protocol v1/v2 types and current sampler expectations
~~~

### CollectedMetrics compatibility

Preserve `greggd::collector::CollectedMetrics` for the current public/internal contract during this campaign.

It may become:

- a thin Gregg wire-adapter type constructed from `HostSample`; or
- a compatibility alias only if all existing field types and methods remain source-compatible.

Its Gregg-specific methods remain in `greggd`:

- `into_snapshot()`;
- `into_snapshot_v2()`;
- `into_status_payload_v2()`;
- `into_snapshot_pair()`.

The extracted crate must not learn schema versions merely to keep those methods.

### SystemCollector compatibility

Preserve the current `greggd::collector::SystemCollector` trait and the signatures used by `Sampler`, `run`, foreground mode, and Windows SCM mode.

Implement the compatibility trait for the extracted platform collectors or wrap them in small adapter structs.

The adapter must preserve:

- `identity()`;
- `sample()`;
- v1 capability behavior;
- v2 capability behavior;
- `supports_v1_snapshot()`.

Windows must continue returning `false` for v1 support.

## Exact protocol mapping

### Linux

Retain:

~~~text
cpu_iowait = true
load_average = true
swap = true
memory_commit = false
supports_v1 = true
~~~

### macOS

Retain:

~~~text
cpu_iowait = false
load_average = true
swap = true
memory_commit = false
supports_v1 = true
cpu_frequency_hz = absent when current supported native source is unavailable
~~~

### Windows

Retain:

~~~text
cpu_iowait = false
load_average = false
swap = false
memory_commit = true
supports_v1 = false
~~~

Do not fabricate zero load/swap merely to make the neutral host model resemble v1.

## Limit mapping

Construct `gregg-host` collection limits from current `gregg-protocol` constants so the daemon's payload bounds do not change.

Add deterministic assertions/tests covering every mapped bound. If the neutral crate default differs, that is acceptable for external consumers but `greggd` must remain pinned to current protocol limits.

## Readiness and failure behavior

The existing sampler remains authoritative for daemon readiness.

Preserve exactly:

- Warming: remain warming; no hard-failure count increment;
- CounterReset: rewarm/rebaseline path; no hard-failure count increment;
- SourceUnavailable/Parse/Numeric: current failed-state transition/count behavior;
- current snapshot retention/replacement semantics;
- pre-epoch wall-clock handling;
- v2 validation before publication.

The adapter must not turn optional telemetry absence into a core collector error.

## Runtime ownership

Do not move sampling into a new async task or runtime.

Keep:

- `Sampler<C, Clk>`;
- current configured cadence;
- `spawn_blocking` boundary;
- collector mutex and poisoned-lock recovery;
- current HTTP/server publication flow;
- current shutdown supervision.

The host crate remains synchronous and runtime-neutral.

## Differential qualification

Use the Plan-133 characterization suite to compare the extracted implementation against frozen expectations.

At minimum require:

- identical Linux CPU/memory/swap/load/identity results on fixture sequences;
- identical macOS normalized results and native optional-family behavior;
- identical Windows memory/commit/CPU/topology behavior;
- identical rate warmup/reset/hotplug semantics;
- identical drive normalization and `None`/empty/last-good behavior;
- identical public adapter error kinds;
- identical v1/v2 Rust structures for canonical samples;
- identical serialized JSON fixtures for canonical samples.

Where a direct "old implementation vs new implementation" test is no longer practical after source deletion, Plan-133 golden expectations are the authority.

## Native qualification

Use the existing workflow.

Require:

- Linux workspace tests;
- macOS arm64 `collector::macos` native suite through the facade/extracted crate;
- macOS Intel same suite;
- Windows workspace tests and release SCM smoke;
- MSRV Rust 1.89.

The native tests must exercise the extracted code, not a duplicated old implementation accidentally left behind.

After cutover, remove dead duplicate collector implementation once the compatibility facade no longer delegates to it.

## Dependency and binary review

Run `cargo tree` or equivalent inspection proving:

- `gregg-host` does not pull daemon transport/runtime dependencies;
- `greggd` has not acquired a broad system-information dependency;
- target-specific native dependencies remain target-scoped.

Record stripped release `greggd` size before and after the final cutover using the repository's current release profile. There is no hard shrink requirement, but unexplained material growth (roughly >1% is a useful review trigger) must be investigated and recorded.

Do not retain a regression merely because "it is only an extraction."

## Documentation

Update current-state documentation:

- `architecture/collectors.md`;
- `architecture/greggd-daemon.md`;
- `crates/greggd/README.md`;
- root `README.md` only if crate/module ownership is described there;
- `CHANGELOG.md`;
- `plans/README.md`.

Document that `greggd::collector` is a compatibility facade and `gregg-host` owns native collection.

Do not rewrite historical completed plans as though the extraction always existed.

## Publication decision

Plan 135 should make the extracted crate publication-ready, but it does not need to publish to crates.io to close.

Before publication:

- confirm package name availability;
- ensure crate README explains platform support and semantics;
- ensure license/repository metadata are correct;
- ensure docs.rs target cfgs do not require unsupported native linking on the docs host;
- document Rust 1.89 MSRV;
- document that FreeBSD is not yet supported until Plan 136 closes.

Crates.io release can be manual under the repository's existing release policy.

## Verification

Run:

~~~text
cargo test -p gregg-host --all-targets --all-features
cargo test -p greggd --all-targets --all-features
cargo test -p gregg-protocol --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
./scripts/check-local.sh
~~~

Use the ordinary existing CI workflow at the final implementation SHA.

## Acceptance criteria

- [ ] `greggd` production native telemetry comes only from `gregg-host`.
- [ ] `greggd::collector` remains available as a compatibility facade for the frozen public paths.
- [ ] `CollectedMetrics` and its v1/v2 conversion remain Gregg-owned and source-compatible for the campaign.
- [ ] `SystemCollector` compatibility remains sufficient for the unchanged sampler/run/runtime signatures.
- [ ] Linux/macOS v1 behavior is unchanged.
- [ ] Windows remains v2-only with commit distinct from swap.
- [ ] Every protocol collection limit maps explicitly into the host crate.
- [ ] Warming/reset/hard-failure readiness behavior is unchanged.
- [ ] Optional telemetry failure remains isolated from core readiness.
- [ ] Drive slow-probe semantics and shutdown isolation remain unchanged.
- [ ] Canonical v1/v2 structures and JSON match Plan-133 expectations.
- [ ] Existing Linux/macOS arm64/macOS Intel/Windows/MSRV CI is green on the extracted implementation.
- [ ] Dead duplicate production collector code is removed after cutover.
- [ ] `gregg-host` dependency graph remains runtime/protocol neutral.
- [ ] Release binary-size change is measured and any material growth is explained.
- [ ] Current architecture/docs identify the new ownership truthfully.
- [ ] The crate is publication-ready without requiring publication to close this plan.

## Explicit non-goals

Do not include:

- new metrics;
- protocol-v3;
- client/TUI work;
- clock injection;
- drive-policy redesign;
- Windows >64/multi-group collector redesign;
- FreeBSD/NetBSD/OpenBSD code;
- daemon service/release changes;
- automatic crates.io publication;
- opportunistic dependency migrations unrelated to the extraction.

## Handoff note

Prefer adapters over broad call-site rewrites.

A successful Plan 135 should leave the daemon boring: same HTTP payloads, same health transitions, same native values, same CLI/service behavior. The meaningful change is ownership—the low-level telemetry implementation is now independently reusable and can accept a FreeBSD backend in Plan 136.
