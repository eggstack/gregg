# Phase 44: Windows CI integration and release-readiness closure

> Superseded by Phase 46 for verification closure. The implementation and
> native product coverage remain useful, but the manual elevated rehearsal,
> closure summary, and evidence-oriented requirements below are not active
> gates. Ordinary read-only CI is the cross-platform verification source.

## Objective

Close the Windows-support line of work by integrating representative native Windows checks into the simplified source-only CI workflow, proving mixed-platform client behavior, reconciling documentation and packaging, and confirming that the manual release process remains small.

This phase is not a return to release qualification. It is a final product-integration pass with ordinary tests, one short native Windows daemon smoke, and a manual elevated service rehearsal performed outside normal CI.

## Dependency and execution position

Depends on:

- Phase 38 minimal local/CI validation;
- Phase 40 Windows client portability;
- Phase 41 capability-aware protocol v2;
- Phase 42 Windows native collector;
- Phase 43 Windows service lifecycle and packaging.

This is the final phase of Plan 036.

## Governing invariants

1. Windows is added to the simplified CI workflow, not to a release workflow.
2. Ordinary Windows CI requires no administrator privileges.
3. CI validates source and short runtime behavior only.
4. CI uploads no release/evidence artifacts on success.
5. Manual service installation remains a separate maintainer smoke.
6. The manual release runbook remains unchanged in principle: local checks, manual crates.io publication, manual tag, manual GitHub Release.
7. Windows support claims match tested behavior and documented limitations.
8. Mixed v1/v2 fleets remain functional.
9. Linux/macOS support and CI remain proportionate and green.
10. No final evidence ledger, qualification manifest, candidate freeze, or cross-run aggregation is created.

## Scope

### In scope

- Windows job in `.github/workflows/ci.yml`;
- Windows-compatible local validation entry point;
- native Windows unit/integration tests;
- short foreground daemon smoke;
- client-to-Windows-daemon loopback smoke;
- mixed v1/v2 fixture/fleet tests;
- platform documentation and crate metadata;
- packaging-content verification;
- manual elevated service rehearsal checklist/result;
- manual release runbook reconciliation;
- removal of temporary Windows-development exceptions.

### Out of scope

- running service installation in ordinary hosted CI;
- hosted self-managed Windows service runners;
- release publication;
- CI artifacts/attestations/provenance;
- exhaustive architecture matrix;
- Windows ARM64 claim;
- MSI/MSIX/winget/Chocolatey/Scoop;
- automated firewall setup;
- public-internet hardening;
- historical telemetry or alerting;
- performance dashboard infrastructure.

## Workstream A: add Windows to the minimal CI workflow

Extend `.github/workflows/ci.yml` with a readable Windows job.

Recommended job:

```text
windows:
  checkout
  install stable Rust with rustfmt/clippy
  cargo fmt check or rely on Linux format job
  cargo clippy workspace/all targets/all features
  cargo test workspace/all targets/all features
  cargo doc workspace/no deps or rely on Linux docs job
  run Windows native collector tests
  run short foreground daemon/client smoke
```

To avoid unnecessary duplication, the preferred final split is:

- Linux owns format, comprehensive Clippy, full workspace tests, docs, and cargo-deny;
- macOS owns native build/tests and macOS collector smoke;
- Windows owns native build/tests and Windows collector/daemon/client smoke;
- MSRV owns one Linux `cargo check`.

If target-specific code is not exercised by Linux Clippy, run Windows Clippy natively even if some generic checks repeat.

### PowerShell correctness

Use PowerShell syntax explicitly. Do not copy Bash snippets containing:

- `set -euo pipefail`;
- `uname`;
- `/tmp`;
- Unix signal commands;
- shell process substitution;
- `sha256sum`.

Use `$ErrorActionPreference = 'Stop'` and normal process exit checking where needed.

### Permissions and artifacts

Workflow permissions:

```yaml
permissions:
  contents: read
```

No write permissions, environment approvals, crates.io secrets, release tokens, or OIDC are required.

Do not call `actions/upload-artifact` for passing validation. A temporary debug artifact may be added during diagnosis but must be removed before phase completion unless it contains product test output that cannot otherwise be diagnosed; default final state is no upload.

### Workstream A acceptance criteria

- [ ] Native Windows CI job exists and passes.
- [ ] Workflow uses PowerShell-correct commands.
- [ ] Workflow remains source-only and read-only.
- [ ] No success evidence artifacts are uploaded.
- [ ] Linux/macOS/MSRV jobs remain small and readable.
- [ ] No hardcoded release version appears in CI.

## Workstream B: provide a Windows-native local validation command

Phase 38 may have created a shell-oriented local command. Provide a native PowerShell equivalent when needed:

```text
scripts/check-local.ps1
```

Target interface aligned with the Unix command:

```powershell
./scripts/check-local.ps1
./scripts/check-local.ps1 -Full
./scripts/check-local.ps1 -Release
```

The Windows script must remain a thin command runner and match the tier semantics:

- default: format/lint/tests/docs;
- full: product smokes and active helper tests;
- release: nonpublishing package/dry-run preflight.

Do not invoke WSL or Bash. Do not generate evidence manifests.

If a small cross-platform Rust/Python validation runner from Phase 38 already works natively without extra dependencies, use it instead and avoid duplicate scripts. The user-facing command must be straightforward in PowerShell.

### Required tests/rehearsal

- default success;
- command failure returns nonzero;
- `-Full` runs Windows smokes;
- `-Release` performs no actual publication/tag/release;
- help/unknown option behavior;
- child-process cleanup;
- paths with spaces.

### Workstream B acceptance criteria

- [ ] Windows developer can run the canonical checks without Unix tools.
- [ ] Tier behavior matches Phase 38.
- [ ] Release mode is nonpublishing.
- [ ] Script remains thin and readable.
- [ ] Child cleanup is reliable.

## Workstream C: add a short native Windows foreground smoke to CI

The Windows CI smoke should exercise product integration without service installation.

### Smoke topology

```text
WindowsCollector
  -> Sampler
  -> cached v2 snapshot
  -> Axum HTTP server on loopback
  -> gregg client HTTP poller
  -> protocol validation
  -> normalized state
```

### Required sequence

1. build `greggd` and `gregg`;
2. create temporary daemon config with `127.0.0.1` and a safely selected port;
3. start `greggd run` as a child process;
4. wait boundedly for health endpoint;
5. wait boundedly for ready v2 status;
6. validate capability/value semantics;
7. run a client noninteractive poll integration helper or test against the daemon;
8. assert normalized state contains Windows identity, CPU, memory, commit, and absent load/swap/I/O-wait;
9. terminate the daemon cleanly;
10. fail if the child remains running or temp cleanup fails.

Do not launch the interactive TUI in hosted CI. TUI behavior is covered by terminal-independent tests and the Phase 40 manual Windows Terminal smoke.

### Flake controls

- use bounded retry with clear timeout;
- do not assert exact utilization values;
- do not use a globally fixed port without collision handling;
- capture child stdout/stderr in CI logs on failure;
- always cleanup in a `finally` block;
- keep total smoke target under 30 seconds;
- do not rely on network outside loopback.

### Workstream C acceptance criteria

- [ ] Foreground daemon reaches v2 ready state on Windows CI.
- [ ] Client polls and normalizes the real Windows response.
- [ ] Capability semantics are asserted.
- [ ] Smoke is bounded and cleanup-safe.
- [ ] No interactive/elevated behavior is required.

## Workstream D: complete mixed-platform and mixed-version tests

Add deterministic tests representing a real heterogeneous fleet:

```text
v1 Linux daemon
v1 macOS daemon
v2 Linux daemon
v2 macOS daemon
v2 Windows daemon
unreachable endpoint
warming endpoint
malformed v2 endpoint
```

Required assertions:

- v2 preferred where available;
- v1 fallback only for explicit v2 unsupported response;
- malformed v2 does not fall back;
- each platform normalizes into one state model;
- sorting/order remains stable;
- unreachable endpoints collapse to one row and sort according to existing policy;
- reachable systems preserve four-row accounting;
- Windows uses commit row;
- load and I/O-wait unsupported states render correctly;
- refresh/cancellation/batch-generation semantics remain intact;
- one failing endpoint does not block or poison others.

Use fixture servers or mock HTTP clients for deterministic cases. The live Windows smoke covers the real collector/server path separately.

### Workstream D acceptance criteria

- [ ] Full mixed fleet is covered in one integration scenario.
- [ ] Protocol fallback/error behavior is explicit.
- [ ] UI row accounting and ordering remain correct.
- [ ] One endpoint failure cannot cascade to others.
- [ ] Existing scheduler bounds remain enforced.

## Workstream E: reconcile package contents and crate metadata

Review all three manifests.

### `gregg-protocol`

- v2 types/fixtures/docs included;
- description mentions versioned cross-platform protocol without overstating compatibility;
- no daemon/client implementation dependencies introduced.

### `greggd`

- description includes Windows;
- target-specific Windows dependencies are included correctly;
- Windows packaging scripts/docs are included if the crate installation instructions rely on them;
- Linux/macOS packaging remains included as intended;
- no release-evidence files enter the crate package;
- license/readme paths work after packaging.

### `gregg`

- description/support docs include Windows client;
- config example remains platform-neutral;
- no Unix-only runtime file is required;
- target-specific dependencies are correct.

Run:

```text
cargo package -p gregg-protocol --list
cargo package -p greggd --list
cargo package -p gregg --list
```

Inspect for accidental inclusion of:

- plans/archive;
- evidence directories;
- CI files;
- large fixtures not needed at runtime/tests;
- local build output;
- secrets/credentials;
- obsolete release scripts.

### Package dry-runs

Use the manual Phase 39 procedure. Do not add package dry-runs to every CI push if they materially slow iteration. Run them locally before release.

### Workstream E acceptance criteria

- [ ] Package descriptions/support claims are current.
- [ ] Required Windows packaging files are included or retrieval instructions are accurate.
- [ ] Obsolete release/evidence files are excluded.
- [ ] All package lists are reviewed.
- [ ] Local package/dry-run checks pass in release preflight.

## Workstream F: elevated Windows service rehearsal

Ordinary CI does not install the service. Before declaring Windows daemon support release-ready, execute Phase 43's elevated lifecycle smoke on a disposable Windows host.

Record only a concise handoff summary:

```text
host baseline
build/version
install: pass
LocalService account: pass
start/ready/status: pass
stop/start/restart: pass
config mutation/restart: pass
reinstall preserves config: pass
uninstall preserves config: pass
explicit config removal: pass
```

Do not commit machine-specific logs, paths, service dumps, host identifiers, or evidence bundles.

Any failure becomes an ordinary product bug and test addition. Do not create another hosted qualification workflow.

### Workstream F acceptance criteria

- [ ] Elevated lifecycle rehearsal passes on native Windows.
- [ ] Failures found during rehearsal have deterministic regression tests where feasible.
- [ ] No sensitive host data is committed.
- [ ] Service rehearsal remains manual and documented.

## Workstream G: performance and resource sanity

Gregg targets lightweight operation. Perform small sanity checks, not a benchmarking program.

### Windows daemon

Observe under a short idle run:

- process remains stable;
- no unbounded handle/thread growth;
- sample cadence matches config;
- requests do not trigger additional collection;
- memory remains approximately stable;
- CPU overhead is low relative to sample interval;
- service stop completes within deadline.

### Windows client

With a modest mixed fixture fleet:

- concurrent requests remain bounded;
- no task leak across refreshes;
- TUI remains responsive;
- memory remains stable during a short sustained run;
- one offline endpoint does not create rapid retry/spin behavior.

Use existing resource/sustained helpers retained after Phase 37 where appropriate. Output may remain in the console or temporary directory and does not become a release artifact.

Do not set brittle exact performance thresholds based on shared CI hardware. Preserve existing product budgets where already well-founded; otherwise use regression-oriented structural assertions such as bounded task/handle counts.

### Workstream G acceptance criteria

- [ ] No handle/task/thread leak is observed in short native runs.
- [ ] Collection cadence is stable.
- [ ] Request path remains cached/no collection-on-request.
- [ ] Scheduler concurrency remains bounded.
- [ ] No performance evidence framework is added.

## Workstream H: documentation and support-claim closure

Update all active documentation consistently.

### Root README support table

Target claims:

```text
Linux x86-64: supported
Linux ARM64: supported, ordinary hosted CI may be limited
macOS Intel: supported, ordinary hosted CI may be limited
macOS Apple Silicon: supported
Windows x86-64: supported
Windows ARM64: not yet supported/verified
```

Distinguish:

- Windows client installation/use;
- Windows foreground daemon;
- Windows service installation;
- Windows metric differences;
- config paths;
- service account;
- firewall/private-network policy.

### API docs

Document:

- `/v1/status` compatibility on Linux/macOS;
- `/v2/status` on all supported daemon platforms;
- Windows lack of truthful v1 status if applicable;
- capability-driven fields;
- commit versus swap;
- readiness endpoints/status codes.

### Release docs

Confirm `RELEASING.md` still says:

- local validation;
- manual crates.io publication;
- manual tag;
- manual GitHub Release;
- no CI publication;
- optional native Windows service rehearsal is a maintainer check, not a workflow stage.

### Historical docs

Search and remove/mark stale statements about:

- Linux/macOS-only product scope;
- CI release stages;
- Phase-35 qualification;
- finalizer evidence;
- exact-SHA release gates;
- `NoopServiceManager` success on unsupported platforms;
- mandatory Unix load/swap on every platform.

### Workstream H acceptance criteria

- [ ] Support tables and crate descriptions agree.
- [ ] Windows metric limitations are explicit.
- [ ] API version behavior is documented.
- [ ] Config/service/firewall instructions are complete.
- [ ] Manual release policy remains prominent.
- [ ] No stale release-evidence model remains active in docs.

## Workstream I: remove temporary scaffolding and exceptions

During Phases 40-43, temporary accommodations may have been added. Remove or resolve:

- package-selected Windows checks that excluded `greggd` before it compiled;
- TODO stubs for Windows collector/service;
- no-op unsupported service fallbacks;
- client-only support disclaimers superseded by daemon support;
- skipped Windows tests that now have implementations;
- duplicated v1/v2 conversion helpers;
- debug logging or temporary artifact uploads;
- fixed ports in tests;
- broad unsafe allowances;
- broad Windows dependency feature sets;
- hardcoded development paths.

Repository search terms:

```text
TODO windows
cfg(not(unix))
NoopServiceManager
not supported on windows
client-only
phase35
release-finalize
upload-artifact
actions/upload-artifact
cargo publish
```

`cargo publish` should remain only in `RELEASING.md` and historical archived documents, not executable code/workflows.

### Workstream I acceptance criteria

- [ ] No Windows implementation stub remains on the supported path.
- [ ] Unsafe allowances are narrow.
- [ ] Temporary CI/debug behavior is removed.
- [ ] Unsupported service no-op success is gone.
- [ ] Active executable code contains no publish/release action.

## Workstream J: final local and CI closure

### Local Linux/macOS

Run the canonical full local check on available hosts.

### Local Windows

Run:

```powershell
./scripts/check-local.ps1 -Full
```

or the canonical equivalent.

### CI

Require one passing ordinary `ci.yml` run containing:

- Linux;
- macOS;
- Windows;
- MSRV.

No separate qualification workflow is needed.

### Manual release rehearsal

Rehearse Phase 39's release preflight without real publication:

- clean tree;
- versions consistent;
- full local checks;
- package lists;
- feasible dry-runs;
- verify no automation would publish;
- review manual tag/GitHub Release commands.

Do not create a candidate tag or draft release solely for rehearsal.

### Workstream J acceptance criteria

- [ ] Full local checks pass on Windows and available Unix hosts.
- [ ] The one ordinary CI workflow passes across its representative jobs.
- [ ] Manual release preflight remains concise and nonpublishing.
- [ ] No second workflow is required to declare readiness.

## Phase acceptance criteria

Phase 44 is complete only when:

- [ ] Windows is integrated into the simplified read-only CI workflow.
- [ ] Native Windows CI runs workspace tests and a short foreground daemon/client smoke.
- [ ] Windows local validation requires no Unix compatibility layer.
- [ ] Mixed v1/v2 Linux/macOS/Windows fleet tests pass.
- [ ] Package contents and crate metadata accurately include Windows support.
- [ ] Elevated Windows service lifecycle rehearsal passes manually.
- [ ] Short resource/handle/task sanity checks show no structural leak.
- [ ] Active documentation is consistent and complete.
- [ ] Temporary Windows scaffolding and obsolete release references are removed.
- [ ] The canonical full local checks pass.
- [ ] The ordinary CI workflow passes on Linux, macOS, Windows, and MSRV jobs.
- [ ] Manual release preflight remains slim.
- [ ] CI still cannot publish crates, tags, or GitHub Releases.
- [ ] No release-evidence artifact, finalizer, qualification workflow, or candidate ledger is reintroduced.

## Evidence required for completion

Only:

- passing ordinary CI status;
- local full-check summaries;
- concise manual Windows service-rehearsal summary;
- package-list/dry-run summary;
- committed code/documentation changes.

Do not create or commit an evidence bundle, immutable manifest, workflow artifact identity, or final qualification ledger.

## Handoff notes for a smaller implementation model

1. Add the Windows CI job only after local Windows workspace tests pass.
2. Keep service installation out of ordinary CI.
3. Build the foreground daemon/client smoke as a reusable local product test with PowerShell cleanup.
4. Add mixed-fleet deterministic tests before changing documentation claims.
5. Review package lists manually and remove obsolete release files from includes.
6. Perform the elevated service rehearsal on a disposable host and convert discovered logic failures into unit tests.
7. Search for temporary stubs and broad unsafe/feature flags before closure.
8. Finish with one ordinary CI run and a nonpublishing release rehearsal.
9. Do not create a new qualification workflow to prove completion.
