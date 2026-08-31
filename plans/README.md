# Gregg plan index

This directory contains Gregg's implementation roadmaps and execution-ready plans.

## Current direction

Gregg remains a small local/LAN system monitor:

- native Linux, macOS, and Windows metric collection;
- a cached read-only JSON daemon API;
- a compact terminal client for a small fleet;
- optional bounded EggPool summary integration;
- local-first verification and manual releases;
- no generalized observability platform, public-internet service, release orchestration, or evidence system.

Local tests remain the primary development path. The one existing GitHub Actions workflow provides Linux checks and native macOS/Windows truth. CI does not publish crates, create releases, upload artifacts, or enforce binary-size gates.

## Roadmap status

Plans 066-079 are complete. Plan 076 implemented the Unix runtime/service-manager separation, HTTP `croncheck`, config-only Unix mutation, and explicit version commands. Plan 077 completed the strict bounded status-line correction, negative-path coverage, stale test cleanup, and planning reconciliation.

Plan 078 implemented the stale client endpoint correction at the existing `Ctrl-R` boundary, HTTP URL input convenience for `gregg add`, and read-only `greggd configprint`. Plan 079 then made replacement delivery reliable under bounded command pressure and corrected Plan 078's live-host record without rewriting the original `.183`/`.182` observation.

Plan 080 implementation landed and its mandatory Ubuntu direct lifecycle smoke passed. Post-closure review then found two product defects: Windows foreground `run` referenced a Unix-only control wrapper and the Unix primary control socket was directory-scoped, allowing configs in the same directory to cross-stop. Plan 081 closed those defects plus permission/stale-socket hardening, preserved the valid Plan 080 historical record, demonstrated the corrected Unix one-daemon and two-daemon same-directory stop-isolation smokes, and passed the existing native CI workflow at implementation SHA `59e17551c211df382c6f0219d0d465ef1c198a8a` in run `31813136597`. Current `main` at the subsequent Plan 081 record commit `6fb005b4a469cdd1ea4baf498fe4a18f5858f3be` also passed the existing workflow in run `31813615708`.

Plan 082 completed the final polish pass. It corrected the remaining Unix control-identity edge so different ordinary path spellings of the same existing explicit config file converge, reconciled the remaining Plan 080/081 status/checklist/provenance wording, and passed existing CI run `31841994426` across all five jobs. It did not reopen the daemon lifecycle architecture or add verification infrastructure.

Plan 083 implemented six bounded client UI/CLI corrections: a shared normal-view metric-row geometry (aligned `[` and `]` across CPU/MEM/SWP-or-COMMIT/DISK with one common `bar_width`), concise disk aggregate text without `used` / `avail` words, fresh-launch viewport snap to `display_order[0]` on the first accepted poll batch only, an explicit-port requirement on `gregg add` accepting the ergonomic `nickname@host:port` form, named versus unnamed offline rendering without duplicate host printing, and a regression test locking in continued polling of offline endpoints across generations. The default local check and remote CI run `32094925174` both passed all five jobs (Linux, macOS arm64, macOS Intel, Windows, MSRV Rust 1.75). Post-closure review identified four narrow corrective items; completed Plan 084 closes them without reopening the client architecture, with CI run `32100189772` green across all five jobs.

Plan 085 corrects four narrow client-rendering defects that survived the Plan 083/084 follow-ups without reopening the daemon, protocol, scheduler, or release architecture: a fleet-wide (not block-local) normal-view metric geometry so opening `[` and closing `]` columns line up across devices, the DISK slash denominator switched from `available_bytes` to `total_bytes` so the percentage and the slash use the same number, a shared selected-system drive-detail table layout so expanded mount/used/total/remaining/percent columns stop drifting between rows, and one shared `CondensedTableLayout` so condensed headings and value columns always sit in the same terminal cell. Plan 086 then closes three narrow boundary defects found in post-implementation review — condensed offline/pending identity was erased by online-only HOST width, expanded-drive fit math did not share the indent/gap/separator constants with the renderer, and per-system suffix resolution used a local label width instead of the fleet `COMMIT` label width — without reopening the daemon, protocol, scheduler, or release architecture. Plan 086 reconciliation is recorded at the end of the plan. The default local check passes; one existing remote CI run is recorded below as evidence.

Plan 087 is a bounded client polish pass that keeps Plans 085/086 geometry and storage corrections intact while adding two compact-pane behaviors and one transient-selection polish. It introduces a strict integer-safe compact suffix policy: when the longest natural metric suffix across the entire online fleet exceeds one quarter of the terminal width, the entire suffix region disappears fleet-wide and the metric rows render as bar-only until the terminal widens again. It separates persistent logical selection from the transient reverse-video highlight: startup leaves the highlight `false`, Systems navigation activates it, and a resettable ten-second event-loop deadline dispatches `Action::ClearSelectionHighlight` while leaving `selected_id` (and `e` drive expansion) untouched. It omits the normal-header `IO` token entirely on unsupported or missing I/O-wait data instead of rendering a placeholder. No daemon, protocol, collector, normalized-capacity, scheduler, endpoint, configuration, dependency, CI, or release behavior changes.

Plan 088 is complete at implementation `58b332b51021e3950fa14d8888a46ed6d069a687`. It closes the three confirmed low-priority findings in the 2026-08-26 workspace bug audit; informational observations and accepted optimizations remain out of scope.

Plan 088 is a narrow corrective pass for the three confirmed low-priority findings in the 2026-08-26 workspace bug audit: route macOS byte percentages through the shared normalization helper, return non-Unix Ctrl-C listener failures through the reusable daemon runtime error boundary, and report duplicate EggPool configuration with a dedicated violation kind. The audit's informational observations and accepted optimizations remain out of scope.

Plan 089 is the completed follow-up corrective pass for the remaining actionable audit
findings: blank sampler identities, IPv6 zone parsing diagnostics, rejected
Systems reload feedback, pre-epoch staleness, large byte-ratio arithmetic,
daemon-name control characters, and CI-blocking clippy diagnostics.

Plan 090 is the completed follow-up for the remaining 2026-08-27 audit
findings: configuration metadata errors, bounded client timeouts, complete v2
capability objects, bounded protocol identities, failed-health categories,
typed DNS classification, injected EggPool deadlines, endpoint normalization,
and preservation of existing daemon config-directory permissions. It closed
in implementation `8193643`.

Plan 091 is in implementation: it hardens long-running `greggd` control and
optional drive collection, and makes `croncheck` identify a responsive Gregg
health endpoint before deciding whether to spawn. Deterministic regressions and
local lifecycle evidence are required before closure; the extended soak remains
manual evidence rather than CI infrastructure.

Plan 092 is complete: it closed the actionable findings from the
2026-08-31 bugs audit around IPv6 zone-ID URL normalization, DNS error
 classification, zero-port validation clarity, and backward-clock snapshot
 staleness, with one low-risk endpoint precondition assertion.

| Plan | Purpose | Status |
| --- | --- | --- |
| [`066-bounded-correctness-and-maintainability-roadmap.md`](066-bounded-correctness-and-maintainability-roadmap.md) | Correct concrete cross-platform defects, retain only justified simplification, and close through bounded verification | complete through 074; CI-backed Windows SCM verification passed |
| [`067-truthful-drive-capacity-semantics.md`](067-truthful-drive-capacity-semantics.md) | Preserve truthful used, free, and caller-available drive capacity | complete |
| [`068-coherent-daemon-state-and-health.md`](068-coherent-daemon-state-and-health.md) | Publish one coherent daemon state and truthful health responses | complete |
| [`069-daemon-cli-runtime-and-test-correctness.md`](069-daemon-cli-runtime-and-test-correctness.md) | Correct config intent, runtime/error boundaries, exit codes, and omitted tests | complete |
| [`070-bounded-client-async-simplification.md`](070-bounded-client-async-simplification.md) | Retain scheduler or EggPool simplification only when smaller and behavior-preserving | complete; no change |
| [`071-measured-footprint-and-lightweight-closure.md`](071-measured-footprint-and-lightweight-closure.md) | Measure safe manifest/profile reductions and retain only verified improvements | complete |
| [`072-windows-service-runtime-and-record-correction.md`](072-windows-service-runtime-and-record-correction.md) | Correct Windows service runtime ownership and nonblocking shutdown | complete |
| [`073-native-windows-scm-entry-and-readiness-correction.md`](073-native-windows-scm-entry-and-readiness-correction.md) | Add native SCM dispatcher/`ServiceMain`, config handoff, and post-bind readiness | complete; operationally verified by 074 |
| [`074-ci-backed-windows-scm-closure.md`](074-ci-backed-windows-scm-closure.md) | Correct the Windows lifecycle smoke and run it in the existing Windows CI job | complete; run `31040689848` passed |
| [`075-configured-name-and-windows-hostname-correction.md`](075-configured-name-and-windows-hostname-correction.md) | Remove the native Windows hostname NUL and honor configured daemon names in foreground and SCM modes | complete; CI run `31189587467` |
| [`076-native-runtime-croncheck-and-version-correction.md`](076-native-runtime-croncheck-and-version-correction.md) | Separate Unix foreground runtime, health probing, config mutation, and version commands | complete; implementation and corrected strict-parser verification recorded |
| [`077-croncheck-strictness-test-cleanup-and-plan076-closure.md`](077-croncheck-strictness-test-cleanup-and-plan076-closure.md) | Bound and tighten `croncheck` status parsing, remove stale disabled tests, and close Plan 076 truthfully | complete |
| [`078-client-endpoint-url-config-reload-and-daemon-configprint.md`](078-client-endpoint-url-config-reload-and-daemon-configprint.md) | Reload stale client endpoints on `Ctrl-R`, accept HTTP URL input for `gregg add`, and add read-only daemon bind-address printing | complete; historical wording corrected by 079 |
| [`079-scheduler-replacement-delivery-and-plan078-record-correction.md`](079-scheduler-replacement-delivery-and-plan078-record-correction.md) | Guarantee scheduler endpoint replacement under bounded command pressure and correct Plan 078's environment record | complete; implementation `49c4c7d` |
| [`080-greggd-runtime-croncheck-and-direct-stop-correction.md`](080-greggd-runtime-croncheck-and-direct-stop-correction.md) | Diagnose/correct the daemon refusal and add direct local Unix `greggd stop` without restoring service-manager coupling | implemented and corrected by completed Plan 081; original Ubuntu lifecycle evidence preserved |
| [`081-plan080-cross-platform-stop-corrective-pass.md`](081-plan080-cross-platform-stop-corrective-pass.md) | Restore Windows foreground compatibility and make Unix stop identity/permissions/stale-socket handling safe | complete; implementation `59e17551`; CI run `31813136597`; Ubuntu one-daemon + two-daemon stop-isolation smokes passed |
| [`082-plan081-control-identity-and-record-polish.md`](082-plan081-control-identity-and-record-polish.md) | Normalize equivalent explicit config path spellings for Unix stop identity and reconcile Plan 080/081 records | complete; focused tests and relative/absolute release smoke passed |
| [`083-compact-tui-endpoint-nicknames-and-polling-invariant.md`](083-compact-tui-endpoint-nicknames-and-polling-invariant.md) | Six bounded client UI/CLI corrections: shared normal-view metric geometry, concise disk text, fresh-launch viewport snap, explicit-port `gregg add` with `nickname@host:port`, named versus unnamed offline rendering, offline-endpoint polling invariant | complete; corrective follow-up 084 closed |
| [`084-plan083-corrective-closure.md`](084-plan083-corrective-closure.md) | Close `--name` validation parity, renderer-level geometry proof, Unicode-aware offline padding, and stale `default_port` documentation | complete; implementation `020188f`; CI run `32100189772` |
| [`085-fleet-wide-tui-column-and-storage-display-correction.md`](085-fleet-wide-tui-column-and-storage-display-correction.md) | Fleet-wide normal-view metric geometry, `<used>/<total>` DISK slash denominator, shared expanded drive-detail table layout, shared condensed-view column layout | complete; closed through Plan 086 |
| [`086-plan085-renderer-boundary-corrective-pass.md`](086-plan085-renderer-boundary-corrective-pass.md) | Condensed offline/pending identity preservation, expanded-drive structural width constants and Compact-before-Minimal degradation, fleet-wide suffix budget | complete |
| [`087-dynamic-compact-metric-suffix-and-transient-selection-polish.md`](087-dynamic-compact-metric-suffix-and-transient-selection-polish.md) | Fleet-wide compact metric suffix when longest natural suffix exceeds terminal-width quarter; logical-vs-visual selection separation with resettable ten-second event-loop deadline; normal-header I/O-wait omission | complete |
| [`088-bugs-audit-corrective-pass.md`](088-bugs-audit-corrective-pass.md) | Correct shared macOS percentage normalization, non-Unix shutdown error propagation, and EggPool duplicate violation semantics | complete; implementation `58b332b` |
| [`089-bugs-audit-corrective-pass.md`](089-bugs-audit-corrective-pass.md) | Correct remaining actionable audit findings and CI-blocking clippy diagnostics | complete; implementation `7f245cc` |
| [`090-bugs-audit-corrective-pass.md`](090-bugs-audit-corrective-pass.md) | Correct remaining 2026-08-27 audit findings with minimal bounded changes | complete; implementation `8193643` |
| [`091-greggd-long-running-stability-and-croncheck-hardening.md`](091-greggd-long-running-stability-and-croncheck-hardening.md) | Harden long-running daemon control, optional drive refresh, and croncheck identity | implementation in progress |
| [`092-bugs-audit-corrective-pass.md`](092-bugs-audit-corrective-pass.md) | Correct IPv6 zone-ID transport, DNS classification, zero-port validation clarity, and future-snapshot staleness | complete; implementation `6efa52c` |

Dependency order:

```text
066 -> 067 -> 068 -> 069 -> 070 -> 071 -> 072 -> 073 -> 074 -> 075 -> 076 -> 077 -> 078 -> 079 -> 080 -> 081 -> 082 -> 083 -> 084 -> 085 -> 086 -> 087 -> 088 -> 089 -> 090 -> 091 -> 092
```

Plan 076 is concrete product-correctness work, not a closure-only record. Plan 077 corrected the remaining bounded `croncheck` issues. Plan 078 added separate live-tested product functionality. Plan 079 is justified by a concrete runtime divergence edge found in source review. Plan 080 is separately justified by the observed daemon refusal and direct-stop product requirement. Plan 081 is separately justified by native Windows breakage and a reproducible cross-config Unix stop-targeting defect. Plan 082 is separately justified by a remaining same-file path-spelling identity edge plus contradictory closure/provenance wording; it is not a closure-only record. Plan 083 is separately justified by six concrete client UI/CLI correctness defects enumerated in its own scope decisions; Plan 084 is separately justified by four concrete post-closure findings and is now closed. Plan 085 is separately justified by four narrow client-renderer defects enumerated in its own scope decisions; it is not a closure-only record. Plan 086 is separately justified by three narrow boundary defects found in Plan 085 post-implementation review and is not a closure-only record. Plan 087 is separately justified by the three bounded client-only visual polish behaviors enumerated in its own scope decisions and is not a closure-only record.

## Execution record for Plan 075

Execution completed:

1. Inspected the existing Windows source, startup paths, foreground smoke, SCM smoke, and relevant documentation.
2. Kept the completed SCM dispatcher, runtime ownership, readiness, and CI architecture unchanged.
3. Truncated `GetComputerNameExW` output using the successful call's returned UTF-16 length.
4. Passed `Some(config.name.as_str())` into native collector construction in foreground and SCM modes.
5. Strengthened the existing Windows foreground and SCM smoke assertions without adding a test harness.
6. Ran focused tests, the default and release local checks, exact Linux CI gates, Rust 1.75 compilation, and one ordinary existing CI run.
7. Recorded the green implementation SHA and workflow run in Plan 075; no corrective phase remained for the Plan 075 scope.

## Verification model

Routine development:

```bash
./scripts/check-local.sh
```

Manual release preflight remains:

```bash
./scripts/check-local.sh --release
```

For Plan 079, focused deterministic local verification was run in addition to the default local check:

```text
cargo fmt --all -- --check
cargo test -p gregg main
cargo test -p gregg scheduler
cargo test -p gregg state
cargo test -p gregg --bin gregg
./scripts/check-local.sh
```

The key Plan 079 proof is a bounded scheduler-command channel test that fills capacity, performs a valid endpoint reload, and demonstrates that `ReplaceEndpoints` is delivered rather than silently dropped. A second A -> B -> C test proves convergence to the latest accepted replacement under bounded capacity.

A second external private-LAN smoke was optional for Plan 079 and did not determine completion. Plan 078 already demonstrated the address-replacement path against a live daemon in the environment available at that time. Plan 079 corrected command-delivery semantics deterministically and corrected the record to preserve both the originating `.183`-working/`.182`-stale report and the later `.182`-reachable smoke environment.

Plan 080's original direct lifecycle proof remains valid historical evidence:

```text
greggd run -> croncheck succeeds -> greggd stop -> daemon exits -> croncheck fails
```

Plan 081 closed the post-080 defects: the Ubuntu one-daemon lifecycle smoke and the two-config same-directory stop-isolation smoke both passed against the corrected config-specific control identity. Existing CI run `31813136597` passed Linux, both macOS jobs, Rust 1.75, and Windows; the Windows job completed workspace tests, release `greggd` build, and SCM lifecycle smoke. No new CI job was added. Later run `31813615708` confirms the documentation-only follow-up commit also left current `main` green; repeated green runs are not a standing requirement.

Plan 082 required focused Unix identity tests, the default local check, and one narrow Ubuntu release-binary smoke proving that a daemon started with one ordinary spelling of an existing config path can be stopped with another spelling of the same file. Those checks passed, and existing CI run `31841994426` passed all five jobs. Plan 082 added no workflow/job/matrix requirement.

A plan does not require:

- a dedicated qualification workflow;
- a second Windows job or matrix;
- a self-hosted or privileged runner;
- uploaded artifacts, logs, screenshots, or evidence bundles;
- immutable candidate SHAs or repeated green runs;
- crates.io publication, tags, or GitHub Releases.

## Completion rule

A phase is complete only when its explicit acceptance criteria are implemented and demonstrated by the lightest appropriate mechanism:

- deterministic unit/integration tests;
- the default local check;
- the release preflight only for release-facing changes;
- native platform CI only where native-platform truth is actually required;
- direct local operational smoke where explicitly required;
- direct documentation inspection for scope and behavior claims.

Do not check boxes based on comments, intent, compilation alone, or an earlier commit that no longer matches HEAD.

Plans 081 and 082 are complete because Plan 081's Ubuntu one-daemon lifecycle smoke, Ubuntu two-config stop-isolation smoke, and native CI run `31813136597` all passed, and Plan 082's same-file identity tests, local checks, release-binary relative/absolute smoke, and record reconciliation all passed. Plan 083 is complete because its client behavior and focused tests passed under the default local check and CI run `32094925174`; Plan 084 is complete because its four corrective findings passed the exact local CI-equivalent checks and CI run `32100189772`.

## Closed scope record for Plan 085

Completed:

- compute one fleet-wide `MetricFleetLayout` from every online system with a current normalized snapshot so the normal-view opening `[` and closing `]` columns line up across devices and survive viewport scrolling;
- switch the normal DISK slash denominator from `aggregate.available_bytes` to `aggregate.total_bytes` while keeping the percentage at `used / total` and preserving explicit `available_bytes` for the expanded remaining-space field;
- replace the per-row drive-detail formatter with one selected-system `DriveTableLayout` so expanded mount/used/total/remaining/percent columns stop drifting between rows, with a documented degradation path for narrow terminals;
- introduce one shared `CondensedTableLayout` so condensed headings and value columns always sit in the same terminal cell and `HOST` is the only flexible/truncatable column;
- update active documentation (`README.md`, `crates/gregg/README.md`, `architecture/gregg-client.md`, `.opencode/skills/gregg-client/SKILL.md`, this index) to describe the fleet geometry, the `<used>/<total>` DISK shape, and the shared expanded drive and condensed layouts;
- run focused renderer tests plus the default local check.

Implementation landed in `f8be3cf2` with clippy cleanup in `29945c3`. Post-implementation review found three narrow boundary defects; they are corrected by completed Plan 086 without reopening the daemon, protocol, scheduler, or release architecture.

Preserved exclusions:

- daemon, protocol, scheduler, state/viewport, drive collector, or release-architecture redesign;
- Plan 067 caller-available semantics, normalized drive model, or KiB/MiB/GiB/TiB unit conversion;
- new dependencies, workflows, jobs, matrices, evidence bundles, or self-daemonization;
- rewriting Plan 067, Plan 083, or Plan 084 historical records;
- horizontal scrolling, mouse controls, themes, snapshot/golden tests, or table-framework dependencies;
- unrelated cleanup or scope expansion beyond the four documented display defects.

## Closed scope record for Plan 086

Completed:

- include all visible system names (online/offline/pending) in the condensed `HOST` width budget and decouple `status_line()` width budgeting from the online numeric table so offline/pending rows always retain a recognizable configured nickname or endpoint host alongside `offline`/`pending`;
- centralize the drive-table structural width constants (`DRIVE_INDENT_CELLS`, `DRIVE_GAP_CELLS`, `DRIVE_SLASH_CELLS`) so the fit calculation and the renderer share the same structural cells, and rewrite `compute_drive_table_layout` so Compact considers a truncated name before falling to Minimal;
- thread the fleet `MetricFleetLayout` through `resolve_system_suffixes` (via the shared `metric_prefix_width` helper) so mixed `SWP`/`COMMIT` fleets budget and render suffixes against the same structural prefix width;
- add deterministic condensed/drive/suffix boundary tests covering the three defects, the aligned-position helper-level drive test, and the mixed-platform fleet budget test;
- reconcile Plan 085's status and acceptance checklist once the corrected behavior is demonstrated;
- update this index to reflect Plan 085 closed through Plan 086 and Plan 086 complete.

Preserved exclusions:

- daemon, protocol, scheduler, state/viewport, normalized-capacity, drive collector, CLI, endpoint, dependency, workflow, or release-process redesign;
- Plan 067 caller-available semantics, normalized drive model, or KiB/MiB/GiB/TiB unit conversion;
- new dependencies, workflows, jobs, matrices, evidence bundles, or self-daemonization;
- horizontal scrolling, mouse controls, themes, snapshot/golden tests, or table-framework dependencies;
- rewriting Plan 067, Plan 083, or Plan 084 historical records;
- a closure-only Plan 087.

## Closed scope record for Plan 084

Completed:

- restore `--name` validation parity with inline `nickname@host:port` before config mutation;
- prove final Ratatui `TestBackend` metric-row indentation, bracket alignment, COMMIT geometry, unavailable DISK truthfulness, and width bounds at representative widths;
- calculate offline dot padding from terminal display width and cover a Unicode nickname;
- make live `default_port` comments and documentation describe compatibility-only state for `gregg add` while retaining the field;
- reconcile Plan 083's follow-up wording and close this plan with implementation `020188f` and CI run `32100189772`.
- remove the Rust 1.75-incompatible lint-reason attribute found during the CI-equivalent MSRV check without changing behavior.

Preserved exclusions:

- endpoint parser, scheduler, state/viewport, daemon, protocol, CI, or release-process redesign;
- schema removal or implicit-port `gregg add` behavior;
- new dependencies, workflows, jobs, matrices, or test infrastructure;
- rewriting historical plan records that accurately describe their former behavior.

## Active scope record for Plan 082

Required:

- normalize the Unix control identity for the same existing config file across relative/absolute and symlink/target spellings where supported;
- preserve deterministic distinct identities for genuinely different config files in the same directory;
- preserve missing implicit default-config behavior without requiring the TOML file to exist;
- correct misleading `canonical` naming/comments if raw path bytes remain anywhere in the identity helper;
- add focused identity tests and one narrow release-binary explicit-path lifecycle smoke;
- replace ambiguous Plan 081 `gh run list --limit 1` provenance with exact run `31813136597`;
- reconcile Plan 081 checkboxes only where closure evidence exists;
- keep Plan 080's valid historical Ubuntu record intact;
- keep CI and release machinery unchanged.

Preserved exclusions:

- control-protocol redesign;
- persistent control registry;
- service-manager integration;
- PID/process discovery;
- new dependencies;
- new workflows/jobs/matrices/evidence bundles;
- unrelated refactoring.

## Closed scope record for Plan 083

Completed:

- introduce `crates/gregg/src/ui/bar.rs` width primitives (`truncate_to_cells`, `render_text_line`) and rewrite `crates/gregg/src/ui/system_block.rs` around `MetricRow`, `build_metric_rows`, `MetricGroupLayout`, `compute_metric_group_layout`, and `render_metric_row` so the four normal metric rows share one label width and one bar width with brackets aligned at the same terminal column, plus `make_bar_string`, `render_drive_details`, and a named-versus-unnamed `render_offline` that never duplicates the host after `name@`;
- shorten the aggregate disk suffix to `used / avail` (no `used` or `avail` words) and emit the unavailable `—` marker instead of a fabricated `0.0%`;
- snap `selected_id` and `viewport_top_id` to `display_order()[0]` only on the first accepted poll batch (`last_applied_generation == 0` snapshot) and preserve ordinary selection/viewport behavior thereafter; `Ctrl-R` does not re-snap;
- require an explicit port on `gregg add` and accept `nickname@host:port`, `http://host:port/` URL form, `[ipv6]:port`, and bare `host:port`; reject host-only, URL-without-port, `nickname@host`, `@host:port`, and the ambiguous combination of inline nickname with `--name`;
- keep `--name` as an alternate explicit form, retain HTTP URL credential/userinfo rejection, keep HTTPS downgraded/never accepted, keep host-only `gregg remove HOST` semantics unchanged, and reuse the existing `SystemEntry.name` schema field (no configuration migration);
- add two `crates/gregg/src/scheduler.rs` regression tests proving offline endpoints are polled again on the next generation and recover automatically when the mock becomes reachable, plus a two-generation failure-only assertion that demonstrates the configured endpoint is never silently suppressed;
- update the user-facing documentation surface (`README.md`, `AGENTS.md`, `architecture/gregg-client.md`, `crates/gregg/config.example.toml`, `.opencode/skills/gregg-client/SKILL.md`) to use explicit-port examples and `nickname@host:port`, show the aligned four-space metric block, and forbid future agents from reintroducing implicit-port `gregg add` examples;
- run focused local checks (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`) and the existing ordinary remote CI workflow once.

Preserved exclusions:

- configuration schema additions or migrations;
- daemon, protocol, scheduler architecture, or polling cadence changes;
- offline backoff, retry queue, or exponential-backoff state machine;
- HTTPS acceptance, credential/userinfo reuse, or generalized URL forms;
- new dependencies, workflows, jobs, matrices, or evidence bundles;
- unrelated TUI redesign or test-suite restructuring.

## Closed scope record for Plan 081

Completed:

- restore Windows foreground `greggd run` compilation by introducing a tiny cfg-aware `run_with_control_path_or_default` dispatch helper that uses the Unix-only control wrapper on Unix and the ordinary `run` on Windows;
- replace the directory-scoped Unix control identity with a config-path-scoped FNV-1a digest so two configs in the same directory cannot cross-stop;
- enforce restrictive `0600` control-socket permissions: a failed `chmod` discards the candidate, the foreground entry point returns `ControlSetupError::NoSecureControl` when no secure candidate succeeds;
- narrow stale-socket cleanup to a tiny `stale_connect_error` helper that only `ConnectionRefused` and `NotFound` authorize, after metadata confirms a socket entry;
- add a deterministic A/B cross-stop regression test plus identity, primary/fallback, and permission-path tests;
- preserve Plan 080's valid Ubuntu root-cause and lifecycle record and append a short correction note rather than rewriting history;
- rerun focused local checks, the Ubuntu one-daemon release-binary lifecycle smoke, and a two-config same-directory stop-isolation smoke;
- pass native CI run `31813136597`, including Windows workspace tests, release `greggd` build, and SCM lifecycle smoke.

Preserved exclusions:

- permanent legacy directory-scoped stop fallback;
- Windows named-pipe redesign;
- Unix service-manager coupling;
- new CI infrastructure.

## Closed scope record for Plan 079

Completed:

- make successful Systems endpoint replacement delivery reliable through the existing bounded scheduler command channel;
- use bounded async backpressure or an equivalently small latest-replacement mechanism;
- explicitly handle a closed scheduler command receiver;
- retain the existing state reconciliation, endpoint host/port stale-result guard, scheduler generation model, and immediate replacement poll;
- add deterministic capacity-pressure tests and an A -> B -> C convergence test;
- correct Plan 078 so it separately records the originating `.183`-working/`.182`-stale report and the later `.182`-reachable closure environment;
- focused local checks and direct planning-record updates.

Preserved exclusions:

- an unbounded scheduler command channel;
- filesystem watcher libraries or continuous hot reload;
- a new background config-monitor subsystem;
- scheduler/actor rewrite, generic priority queue, or watch-channel architecture unless a tiny equivalent is strictly smaller than awaited delivery;
- changes to poll concurrency or endpoint schemas;
- TLS/HTTPS polling or changes to URL-form `gregg add`;
- changes to `greggd configprint` or `croncheck`;
- EggPool redesign;
- broad TUI redesign, test-suite restructuring, or unrelated cleanup;
- new workflows, jobs, matrices, artifacts, evidence bundles, or CI gates;
- release automation or publication work;
- a Plan 080 created only to record Plan 079 closure.

## Completed roadmap groups

| Roadmap | Scope | Status |
| --- | --- | --- |
| [`000-roadmap-v1.md`](000-roadmap-v1.md) with Plans 001-009 | Original workspace, collectors, daemon, client, TUI, and testing foundation | implemented baseline |
| [`036-release-simplification-and-windows-support-roadmap.md`](036-release-simplification-and-windows-support-roadmap.md) with Plans 037-047 | Manual release model, minimal CI, Windows client/collector/service support, and verification simplification | completed |
| [`048-drive-metrics-and-multiview-tui-roadmap.md`](048-drive-metrics-and-multiview-tui-roadmap.md) with Plans 049-055 | Bounded drive records, cross-platform collection, fleet scrolling, normal/condensed views, and drive expansion | completed |
| [`056-eggpool-summary-pane-roadmap.md`](056-eggpool-summary-pane-roadmap.md) with Plans 057-062 | One optional EggPool endpoint, four fixed periods/metrics, bounded worker, and compact second pane | completed |
| [`063-narrow-correctness-and-simplification-roadmap.md`](063-narrow-correctness-and-simplification-roadmap.md) with Plans 064-065 | Windows v2 staleness, strict endpoint parsing, package truth, verification deduplication, and runtime cleanup | completed; CI run `30964819950` passed |

Plans 010-035 describing retired staged release/evidence work remain archived under `plans/archive/v1.0.1-release/` and are not current requirements.
