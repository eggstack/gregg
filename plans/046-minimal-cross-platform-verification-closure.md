# Phase 46: minimal cross-platform verification closure

## Objective

Reduce Gregg's remaining CI, local-validation, and closure complexity to the minimum proportionate contract for a small system-monitoring tool while retaining truthful native coverage for Linux, macOS, and Windows.

This phase corrects the closure model introduced by Plans 044 and 045. The implementation work in those phases is largely useful, but the verification requirements became too elaborate for the size, deployment model, and risk profile of Gregg.

The target end state is:

- one ordinary read-only GitHub Actions workflow;
- Linux owns generic Rust correctness checks;
- macOS and Windows CI jobs exist only because native platform behavior cannot be validated truthfully from Linux;
- Windows CI is the authoritative native Windows verification path;
- no mandatory private/manual Windows host rehearsal;
- no release workflow, candidate workflow, evidence workflow, or provenance workflow;
- no CI publishing;
- one fast local developer command;
- one straightforward local release-preflight command;
- manual crates.io publication and manual GitHub tagging/release creation;
- concise documentation rather than a separate closure-evidence system.

The work must remove complexity rather than move it to different scripts or documentation.

## Baseline

Start from `main` at or after:

```text
7748e8807c998d8d6bfb87fa44b6184229988a35
```

At this baseline:

- the release-script correctness defects identified before Phase 45 are substantially fixed;
- local installation uses `cargo install --path crates/greggd --locked`;
- the universal daemon smoke uses `/v2/healthz` and `/v2/status`;
- pre-publication automated dry-run is protocol-only;
- Windows client, collector, protocol-v2, service, and foreground smoke code exists;
- the repository still carries an over-detailed three-tier local check model, a large fake-command shell regression harness, duplicated generic CI work, and closure criteria that demand more evidence than this project warrants.

## Why this pass is required

Gregg is a compact private-network/SBC-oriented monitor, not a public multi-tenant service or regulated release pipeline. Its verification system should protect core behavior without materially slowing iteration or becoming a product of its own.

The current verification model has several disproportionate elements:

1. Generic format, documentation, Clippy, and broad test work is duplicated across platform jobs.
2. The local scripts expose default, full, and release tiers even though only development and release-preflight workflows are operationally necessary.
3. `scripts/tests/test_check_local.sh` is larger and more complicated than the script it attempts to validate, duplicates production logic, and mostly tests stubs or source text rather than product behavior.
4. Phase 45 requires a manually elevated Windows service rehearsal even though native Windows CI is the only repeatable cross-platform verification channel available to this project.
5. Closure-record requirements resemble evidence infrastructure even though the repository explicitly rejected evidence bundles and staged qualification.
6. Package-list, registry-order, and release sequencing checks are release-operator concerns and should not expand ordinary CI.
7. The CI workflow still contains work that can be owned once by Linux rather than repeated for symmetry.

Phase 46 replaces those requirements with a smaller, explicit contract.

## Governing principles

1. **One workflow.** Keep a single `.github/workflows/ci.yml` for pushes, pull requests, and manual dispatch.
2. **Read-only CI.** The workflow retains only `contents: read` and never publishes, tags, creates releases, uploads success evidence, or requests OIDC.
3. **Linux owns generic checks.** Format, broad Clippy, workspace tests, and docs run once on Linux.
4. **Platform jobs prove native behavior only.** macOS and Windows jobs compile native code and run the smallest native tests required to validate their advertised support.
5. **Windows CI is authoritative.** Do not require a separately maintained Windows machine or a manual Windows evidence record for routine closure.
6. **No fake parity.** Do not run the same broad generic suite on every operating system merely to make the matrix look symmetrical.
7. **No elevation infrastructure.** Do not add self-hosted runners, privilege-escalation actions, scheduled service tests, nested virtualization, or remote Windows orchestration.
8. **No evidence system.** A green ordinary CI run is the hosted verification result. Do not create closure manifests, run-ID registries, uploaded logs, checksums, attestations, or evidence directories.
9. **Two local workflows only.** Preserve a fast development check and one release preflight. Remove an intermediate tier unless it has a distinct real operator use.
10. **Release remains manual.** crates.io publication, annotated tags, and GitHub Releases remain manual operator actions outside CI.
11. **Tests must test product behavior.** Prefer Rust unit/integration tests and real bounded loopback smokes over large fake-command harnesses.
12. **Documentation must match actual support.** If a Windows behavior cannot be exercised reliably in ordinary hosted CI, narrow the claim rather than creating a new verification apparatus.
13. **No unrelated hardening.** This phase does not add public-internet security controls, sustained benchmarks, fuzzing, chaos tests, or package-manager distribution.

## Target verification contract

### Local development

One command should be sufficient for normal development:

```bash
./scripts/check-local.sh
```

Windows may retain the equivalent native entry point:

```powershell
.\scripts\check-local.ps1
```

The default local check should contain only the checks developers need on most changes:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
native collector tests for the current host when they are not already covered clearly by the workspace test command
```

Do not require package listing, source installation, daemon loopback smoke, Git cleanliness, or publish dry-runs in the default tier.

`cargo-deny` may remain an optional explicitly requested local command, but it must not block ordinary development or require a dedicated CI action. If retained in the script, absence of the tool must remain non-fatal.

### Local release preflight

One extended command should perform the nonpublishing release checks:

```bash
./scripts/check-local.sh --release
```

Windows equivalent:

```powershell
.\scripts\check-local.ps1 -Release
```

The release preflight may add only:

- clean-tree verification;
- workspace/member/dependency version consistency;
- `cargo package --list` for the three crates;
- source-path installation of `greggd`;
- one bounded installed-daemon loopback smoke using v2;
- `cargo publish -p gregg-protocol --dry-run --locked`.

Dependent crate dry-runs remain manual after the protocol version is visible on crates.io, as documented in `RELEASING.md`.

There must be no separate `--full`/`-Full` tier unless implementation demonstrates a concrete distinct operator use that cannot be represented by the two workflows above. Backward compatibility for an old flag is not, by itself, sufficient reason to retain a tier; a deprecated alias may temporarily map to `--release` if needed, but it must not maintain separate behavior.

### Hosted CI

The ordinary workflow should contain the following minimal responsibilities.

#### Linux generic job

Linux owns all platform-independent checks:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo doc --workspace --no-deps
```

Optionally retain one shell syntax/static check for shipped Unix installer scripts if it is one direct command and has demonstrated value. Do not add a separate installer-testing framework.

Remove `cargo-deny` from ordinary CI. Dependency review may be performed manually before release or by an optional local command. Gregg's small dependency set and private deployment model do not justify a separate hosted action on every change.

#### macOS native jobs

Retain native jobs only for advertised macOS architectures.

Each macOS job should do the smallest set that proves native compilation and native collector behavior, for example:

```text
cargo check --workspace --all-targets --all-features
cargo test -p greggd --all-features -- collector::macos::ffi::native_tests
```

Do not rerun workspace-wide format, broad Clippy, docs, or the full generic test suite on both macOS architectures.

If both Apple Silicon and Intel remain explicit support claims, keep both native runners. If Intel is no longer an intentional tested support target, update the support documentation and remove that runner rather than emulating or cross-compiling a claim that is not needed.

#### Windows native job

Windows CI is the repeatable source of truth for Windows x86-64 support.

The Windows job should run only the smallest native set needed to validate:

- workspace/native compilation;
- Windows client portability behavior;
- Windows collector behavior;
- Windows service-manager logic that can be exercised without special infrastructure;
- foreground daemon startup and v2 status;
- v2 Windows capability semantics.

A suitable bounded shape is:

```text
cargo check --workspace --all-targets --all-features
cargo test -p gregg --all-targets --all-features
cargo test -p greggd --all-features -- collector::windows
cargo test -p greggd --all-features -- service::windows
cargo test -p greggd --test windows_smoke
```

Exact filters may be adjusted to existing module/test names. The intent is targeted native coverage, not repetition of the complete Linux-owned generic suite.

The foreground smoke must remain bounded and use loopback only. It should validate the native Windows collector through the daemon's `/v2/healthz` and `/v2/status` path.

##### Windows SCM lifecycle policy

Use the existing Windows service lifecycle smoke in hosted CI only if it runs reliably on an unmodified standard `windows-latest` runner with one direct repository command.

Allowed:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File packaging/smoke-windows.ps1
```

Only retain that step if repeated ordinary runs show it is stable and does not require runner customization.

Not allowed:

- self-hosted runners;
- custom VM images;
- remote Windows machines;
- privilege-escalation actions;
- retry workflows;
- scheduled qualification workflows;
- artifact collection for the service smoke;
- manual host evidence as a merge/release gate.

If the standard runner cannot perform the SCM lifecycle reliably, remove the hosted lifecycle step and document the support boundary precisely:

- Windows foreground daemon/client operation is CI-verified;
- service-manager code paths are covered by native unit/integration tests;
- actual administrator installation depends on Windows SCM policy and is not continuously exercised.

Do not claim a continuously verified elevated lifecycle if ordinary CI cannot demonstrate it.

#### MSRV job

Keep one small MSRV job only while the workspace explicitly advertises a fixed `rust-version` such as 1.75:

```text
cargo check --workspace --all-features
```

Do not run tests, docs, package checks, or Clippy in the MSRV job.

If the project decides it does not need an MSRV support promise, update/remove the advertised `rust-version` and delete the job. Do not retain an MSRV job without an explicit compatibility promise.

## Workstream A: collapse local validation to two workflows

### A1. Remove the intermediate tier

Inspect `scripts/check-local.sh`, `scripts/check-local.ps1`, and all active documentation.

Preferred result:

```text
default
release
```

Remove separate `full` behavior. If compatibility is useful for one transition, make `--full`/`-Full` print a deprecation notice and invoke the release tier without maintaining separate branches. Prefer complete removal if no external automation depends on it.

### A2. Keep default checks fast and obvious

The default scripts should be readable top-to-bottom without helper abstractions that obscure ordinary command execution.

Do not add:

- a task runner;
- a script framework;
- parallel orchestration;
- JSON result generation;
- command manifests;
- resumable steps;
- retry logic;
- platform emulation.

### A3. Keep release preflight nonpublishing

The release tier must remain a linear sequence of direct commands. It must not publish or mutate GitHub.

It should fail immediately on the first failed step and clean temporary install/smoke directories with ordinary shell/PowerShell cleanup.

### Workstream A acceptance criteria

- [ ] Normal development has one canonical local command per host shell.
- [ ] Release preparation has one canonical extended command per host shell.
- [ ] No distinct intermediate tier remains without a demonstrated operator use.
- [ ] Default mode performs no package, install, loopback, Git-cleanliness, or publish work.
- [ ] Release mode remains nonpublishing and linear.
- [ ] Both scripts remain understandable without a validation framework.

## Workstream B: delete disproportionate script-test infrastructure

### B1. Remove the large fake-command harness

Delete `scripts/tests/test_check_local.sh` unless it can be reduced to a very small direct smoke that invokes the actual script rather than duplicating implementation functions.

The current harness must not be retained merely because it exists. It is disproportionate because it:

- exceeds the complexity of the script under test;
- copies version-check implementation into a heredoc;
- tests source patterns and stubs rather than the real command path;
- creates maintenance coupling without meaningful product coverage.

### B2. Use direct confidence mechanisms

Use only the following lightweight mechanisms:

- `bash -n scripts/check-local.sh` where useful;
- PowerShell parser/execution on the Windows runner;
- actual execution of the default local script in normal development/CI-owned commands where appropriate;
- actual execution of the release preflight by the release operator;
- Rust tests for protocol, collector, server, client, and service behavior;
- the real bounded daemon smoke.

Do not add Bats, Pester, pytest wrappers, snapshot testing, golden command logs, or a replacement fake-command framework solely for these scripts.

### B3. Tighten the real v2 smoke only where necessary

The installed-daemon verifier should require schema version 2 from `/v2/healthz` and `/v2/status`.

Keep this as a direct correction. Do not create a new endpoint-test suite around the shell verifier; server/protocol semantics belong in Rust tests.

### Workstream B acceptance criteria

- [ ] The 342-line fake-command regression harness is deleted or reduced to a genuinely small direct smoke.
- [ ] No duplicated copy of production version-check logic remains in test scripts.
- [ ] No new shell/PowerShell test framework is introduced.
- [ ] `/v2/healthz` validation requires schema version 2.
- [ ] Product behavior remains covered primarily by Rust tests and the real loopback smoke.

## Workstream C: minimize the ordinary CI workflow

### C1. Remove duplicated generic work

Refactor `.github/workflows/ci.yml` so only Linux runs generic format, broad Clippy, broad workspace tests, and docs.

Remove format/docs/broad Clippy/full generic workspace duplication from Windows and macOS jobs unless a command is required specifically to compile target-gated code.

### C2. Remove nonessential hosted actions

Remove the `cargo-deny` GitHub Action from ordinary CI.

Retain `actions/checkout`, Rust toolchain setup, and optional cache use only. Cache failure must remain non-fatal. Do not add replacement dependency scanners.

### C3. Keep native target coverage explicit

Mac and Windows jobs must contain direct commands whose purpose is obvious from the YAML.

Avoid matrices when a small explicit job is clearer, except a two-entry macOS architecture matrix is acceptable.

### C4. Keep the workflow read-only

The final workflow must not contain:

- `contents: write`;
- `id-token: write`;
- `packages: write`;
- release environments;
- registry tokens;
- `cargo publish`;
- tag commands;
- GitHub Release creation;
- `upload-artifact` for success evidence;
- release-only branches or candidate inputs;
- scheduled qualification.

### Workstream C acceptance criteria

- [ ] Exactly one active CI workflow exists.
- [ ] Linux runs generic fmt, broad Clippy, workspace tests, and docs once.
- [ ] macOS jobs run only native compilation/checks and native collector tests.
- [ ] Windows runs targeted native client/collector/service/foreground tests rather than the complete duplicated Linux suite.
- [ ] `cargo-deny` is absent from ordinary CI.
- [ ] Package listing, source installation, publish dry-runs, and release sequencing are absent from CI.
- [ ] CI permissions remain `contents: read` only.
- [ ] No artifacts, secrets, environments, OIDC, publishing, or release mutation exist.

## Workstream D: make Windows CI the truthful support gate

### D1. Preserve native foreground coverage

Ensure the Windows job executes the existing foreground daemon smoke or an equivalent Rust integration test that validates:

```text
WindowsCollector
-> sampler warmup
-> cached v2 snapshot
-> HTTP server
-> /v2/healthz ready
-> /v2/status schema/capabilities
-> clean bounded child termination
```

Required semantic checks:

- CPU logical cores and utilization are present;
- physical memory is present;
- commit charge is present;
- load average is absent;
- swap is absent;
- CPU I/O wait is absent;
- schema version is 2;
- system identity is nonempty and reports Windows.

The generic client normalization/fallback/TUI behavior may remain in Rust tests. Do not build a second end-to-end client harness unless a concrete native Windows defect requires it.

### D2. Decide the SCM smoke by ordinary-run reliability

Attempt the existing service lifecycle smoke only as a normal Windows CI step.

Keep it if:

- it runs without workflow-level elevation machinery;
- it completes within a bounded short duration;
- it is stable across ordinary reruns;
- it cleans up the service and files;
- it does not require uploaded logs or special secrets.

Remove it as a gate if any of those conditions fail. In that case, rely on native service-manager unit/integration tests and document the unverified administrator-install boundary.

### D3. Remove mandatory manual Windows evidence

Delete active-plan and release documentation requirements for:

- a separately maintained Windows host;
- a manually recorded elevated lifecycle rehearsal;
- resource/handle observations;
- a service evidence summary;
- machine-specific closure notes.

Manual user testing remains welcome but is not a plan, merge, or release gate.

### Workstream D acceptance criteria

- [ ] Windows x86-64 native compile and targeted tests run in ordinary CI.
- [ ] A bounded foreground daemon v2 smoke runs in ordinary Windows CI.
- [ ] Windows capability semantics are asserted natively.
- [ ] SCM lifecycle smoke is retained only if standard hosted CI runs it reliably without special infrastructure.
- [ ] If SCM lifecycle cannot be retained, active documentation narrows the verification claim accurately.
- [ ] No manual Windows host or evidence record is required for closure.

## Workstream E: simplify release verification and documentation

### E1. Preserve the manual release sequence

`RELEASING.md` should continue to describe:

1. clean tree and intended version;
2. local release preflight;
3. manual `gregg-protocol` dry-run and publish;
4. wait for registry indexing;
5. manual dependent dry-runs and publishes;
6. optional native smoke appropriate to the maintainer's host;
7. annotated tag;
8. GitHub Release.

Do not create a cross-platform release qualification matrix. Ordinary CI already supplies hosted native-platform confidence.

### E2. Keep package review manual and release-scoped

`cargo package --list` remains part of the local release preflight. It does not need a separate committed review record, CI job, artifact, or checklist file.

### E3. Remove closure-record templates

Delete the Phase-45-style closure record as an active requirement.

A concise commit or handoff message may state:

```text
local checks passed
ordinary CI passed
publishing not performed
```

No exact format, run-ID registry, machine inventory, or per-job transcription is required.

### E4. Reconcile active plans

Update Plans 044 and 045 with short notes that their heavier verification requirements are superseded by Phase 46.

Do not rewrite historical plan bodies extensively. Add a clear status/supersession note near the top or in their closure sections.

Update `plans/README.md` so Phase 46 is the only open corrective phase.

### Workstream E acceptance criteria

- [ ] Manual crates.io and GitHub release policy remains unchanged.
- [ ] Ordinary CI is the cross-platform verification source; the release runbook does not recreate the matrix manually.
- [ ] Package review remains a local release-preflight action only.
- [ ] No active closure-record template or evidence requirement remains.
- [ ] Plans 044 and 045 clearly defer their excessive verification requirements to Phase 46.
- [ ] The registry identifies Phase 46 as the sole remaining corrective phase until completion.

## Required implementation order

Execute in this order:

1. Add supersession/status notes to Plans 044 and 045.
2. Collapse the local scripts to default and release workflows.
3. Delete or drastically reduce `scripts/tests/test_check_local.sh`.
4. Correct `/v2/healthz` schema validation directly.
5. Simplify `.github/workflows/ci.yml` so Linux owns generic checks.
6. Add/retain only targeted macOS and Windows native commands.
7. Try the existing Windows SCM smoke as one ordinary CI step only if appropriate.
8. Remove it and narrow documentation if standard runner execution is not reliable.
9. Remove mandatory manual Windows rehearsal/evidence language from active docs.
10. Simplify `RELEASING.md` and remove closure-record requirements.
11. Run the normal local default check on the implementation host.
12. Push and require one ordinary CI run at the final SHA.
13. Fix only product or workflow defects exposed by that run; do not add qualification infrastructure.
14. Update the registry and mark Phase 46 complete when the explicit criteria below are satisfied.

## Explicit phase acceptance criteria

Phase 46 is complete only when all of the following are true:

### Repository and workflow shape

- [ ] Exactly one active GitHub Actions workflow exists.
- [ ] The workflow is triggered only by normal push, pull request, and optional manual dispatch.
- [ ] Workflow permissions are `contents: read` only.
- [ ] No CI publishing, tagging, release creation, OIDC, release environments, or registry secrets exist.
- [ ] No workflow uploads success evidence or maintains candidate/qualification state.

### Linux generic verification

- [ ] Linux runs format once.
- [ ] Linux runs broad workspace Clippy once.
- [ ] Linux runs broad workspace tests once.
- [ ] Linux builds documentation once.
- [ ] `cargo-deny` is not an ordinary CI job/action.

### macOS native verification

- [ ] Each advertised macOS architecture compiles the workspace natively.
- [ ] Each advertised macOS architecture runs the native collector/FFI tests.
- [ ] macOS jobs do not duplicate Linux-owned format, docs, broad Clippy, or full generic tests without a documented target-specific reason.

### Windows native verification

- [ ] Windows x86-64 compiles the workspace natively.
- [ ] Windows client portability tests run natively.
- [ ] Windows collector tests run natively.
- [ ] Windows service-manager logic tests that do not require special infrastructure run natively.
- [ ] The bounded foreground daemon v2 smoke runs natively in ordinary CI.
- [ ] The v2 smoke verifies Windows-specific capability semantics.
- [ ] A live SCM lifecycle step exists only if it runs reliably on the unmodified standard runner.
- [ ] No manual Windows host rehearsal is required.
- [ ] Documentation accurately distinguishes CI-verified foreground/native behavior from any administrator-install behavior not exercised continuously.

### MSRV

- [ ] A single check-only MSRV job exists only if an explicit `rust-version` support promise remains.
- [ ] The MSRV job performs no tests, docs, Clippy, packaging, or release work.

### Local validation

- [ ] There is one canonical default local command per shell.
- [ ] There is one canonical release-preflight command per shell.
- [ ] No distinct intermediate tier remains without a concrete operator use.
- [ ] Default mode stays limited to ordinary source correctness.
- [ ] Release mode adds only clean/version/package/source-install/v2-smoke/protocol-dry-run work.
- [ ] Release mode never publishes.
- [ ] Dependent crate dry-runs remain manual after protocol indexing.

### Test and script complexity

- [ ] The large fake-command `test_check_local.sh` harness is removed or reduced to a small direct smoke.
- [ ] No test script duplicates the production version-check implementation.
- [ ] No Bats, Pester, new pytest layer, snapshot suite, or command-manifest framework is added.
- [ ] `/v2/healthz` requires schema version 2 in the real verifier.
- [ ] Product behavior is covered primarily by Rust tests and the bounded real loopback smoke.

### Release and planning documentation

- [ ] Manual crates.io publication remains outside CI.
- [ ] Manual tag and GitHub Release creation remain outside CI.
- [ ] Package review is local/release-scoped and creates no committed evidence record.
- [ ] Active documentation contains no mandatory manual Windows rehearsal requirement.
- [ ] Active documentation contains no closure-record template or run-ID registry requirement.
- [ ] Plans 044 and 045 state that Phase 46 supersedes their excessive verification/closure requirements.
- [ ] `plans/README.md` is reconciled accurately.

### Final proof

- [ ] The default local check passes on the implementation host.
- [ ] The local release preflight is exercised successfully on a clean tree when preparing an actual release, but it is not required on every development commit.
- [ ] One ordinary CI run at the final implementation SHA passes all retained Linux, macOS, Windows, and MSRV jobs.
- [ ] No additional verification workflow or evidence artifact was created to obtain that result.

## Closure rule

A single green ordinary CI run at the final implementation SHA is sufficient hosted cross-platform proof for this phase.

Do not require:

- a manually elevated Windows host;
- a separate Windows rehearsal record;
- repeated green runs;
- an immutable candidate SHA;
- package archives;
- evidence ZIPs;
- artifact IDs;
- checksums;
- provenance;
- a qualification report;
- a release dry-run workflow;
- actual crates.io publication;
- a Git tag or GitHub Release.

A concise final handoff note is sufficient:

```text
Phase 46 final SHA: <sha>
Local default check: pass
Ordinary CI: pass
Windows verification: hosted native CI
Publishing performed: no
```

This note may live in the implementing commit message or plan status. Do not create a separate evidence file solely for closure.

## Handoff guidance for a smaller implementation model

1. This is a deletion/simplification task, not a new testing project.
2. Start by removing duplicate commands, not by adding abstractions.
3. Keep Linux as the only generic-check owner.
4. Keep native macOS and Windows commands explicit and targeted.
5. Do not add a second workflow for Windows or releases.
6. Do not preserve `--full` merely because earlier plans created it.
7. Delete the large shell harness; do not replace it with another framework.
8. Use the existing Rust native tests and foreground Windows smoke.
9. Try the existing SCM smoke only as one normal Windows CI command. If it is unreliable, remove the gate and narrow the support wording.
10. Do not request manual Windows evidence.
11. Do not add `cargo-deny` back to CI.
12. Do not put package or publish checks in CI.
13. Stop once the explicit acceptance criteria are met. Unrelated cleanup belongs in a separate future plan.