# Phase 45: release-script correctness and Windows closure

> Superseded by Phase 46 for verification closure. The release-script and
> product corrections remain landed, while the manual Windows rehearsal,
> closure-record format, and evidence-oriented requirements below are no
> longer active gates.

## Objective

Close the remaining correctness and verification gaps after implementation of Plans 037 through 044 without rebuilding release orchestration or expanding CI.

This is a narrow closure pass. It must:

- repair the Unix and Windows local validation scripts so they validate the current checkout rather than a previously published crate;
- make release-version checks match the workspace's actual `version.workspace = true` manifest convention;
- keep pre-publication dry-runs truthful for a three-crate dependency chain;
- make the documented installed-daemon smoke valid on Linux, macOS, and Windows;
- complete the light native/CI/manual checks required to support the current Windows claims;
- reconcile Plans 040 through 044 and the plan registry with the implementation that has already landed.

The result should be a small, dependable local-first release process and an accurately documented Windows support state. No new release workflow, evidence framework, package registry simulator, or generalized validation system is permitted.

## Baseline and reason for this pass

Use current `main` at or after:

```text
188fec3bca7b5eb815c93e28d60570f59c0d5e97
```

Plans 037 through 044 were substantially implemented in the 25 commits following the Plan-036 registry commit. The repository now contains:

- one read-only source CI workflow;
- no automated publication/finalization workflows;
- a manual `RELEASING.md`;
- Unix and PowerShell local-check entry points;
- protocol v2;
- Windows client support;
- a native Windows collector;
- Windows SCM lifecycle and packaging;
- Windows and mixed-version tests.

The remaining defects are narrow but material:

1. `scripts/check-local.sh --release` assumes member manifests contain explicit `version = "..."` fields even though all members use `version.workspace = true`.
2. `scripts/check-local.ps1 -Release` makes the same incorrect assumption and may index an empty regex match.
3. `RELEASING.md` instructs maintainers to verify member versions with a grep that does not match the manifests' workspace-inheritance form.
4. Both local full-check scripts run `cargo install greggd` without `--path`, which tests the crates.io version rather than the current checkout.
5. Both release tiers attempt dependent-crate publish dry-runs before the new `gregg-protocol` version is available from crates.io.
6. Dry-runs in the local scripts omit `--locked`.
7. The cross-platform release smoke in `RELEASING.md` queries `/v1/status`, which is intentionally unavailable on Windows; `/v2/status` is the truthful universal status endpoint.
8. The registry still lists Plans 040 through 044 as planned even though implementation has landed.
9. Final native Windows service rehearsal, package review, ordinary CI confirmation, and concise closure reporting are not yet represented accurately.

## Dependency and execution position

Phase 45 depends on the implementation already landed for Plans 037 through 044.

It supersedes no product architecture. It is the final closure phase for Plan 036.

Dependency graph:

```text
37 -> 38 -> 39
37 -> 40 -> 41 -> 42 -> 43
38 + 40 + 41 + 42 + 43 -> 44
39 + 40 + 41 + 42 + 43 + 44 -> 45
```

No real crates.io publication, release tag, or GitHub Release is required to complete Phase 45.

## Governing invariants

1. CI remains read-only and source-focused.
2. CI never publishes crates, pushes tags, creates GitHub Releases, or uploads success evidence bundles.
3. Local validation tests the current checkout.
4. The default local tier remains suitable for iterative development.
5. Full/release tiers may be slower, but must remain straightforward command runners rather than orchestration systems.
6. Workspace members continue to inherit the workspace version through `version.workspace = true` unless a separate deliberate manifest change is made.
7. Registry dependency ordering is respected: `gregg-protocol` must exist on crates.io before authoritative dependent-crate dry-runs.
8. No local sparse registry, fake crates.io index, package provenance chain, candidate freeze, or finalizer is introduced.
9. `/v2/status` is the universal cross-platform status smoke endpoint.
10. `/v1/status` compatibility checks are optional and platform-specific to Linux/macOS.
11. Windows service installation remains a manual elevated rehearsal, not hosted CI.
12. Completion evidence is limited to normal command results and a concise plan/registry status update.
13. Existing Linux/macOS behavior and manual release policy must not regress.
14. Do not broaden this pass into performance rewrites, new package managers, Windows ARM64, public-network hardening, or automatic releases.

## Scope

### In scope

- `scripts/check-local.sh`;
- `scripts/check-local.ps1`;
- `RELEASING.md`;
- `README.md`, `CONTRIBUTING.md`, and `AGENTS.md` only where local/release commands need correction;
- existing installed-daemon/product smoke helpers;
- package-content review for all three crates;
- ordinary `.github/workflows/ci.yml` verification and only minimal corrections required for a clean run;
- Windows foreground/client smoke confirmation;
- manual elevated Windows SCM lifecycle rehearsal;
- plan files 040 through 044 status notes where needed;
- `plans/README.md` registry reconciliation;
- small regression tests for script behavior where practical.

### Out of scope

- real publication to crates.io;
- creating a real release tag or GitHub Release;
- adding a release workflow;
- adding workflow artifacts, attestations, provenance, or evidence manifests;
- local crates.io emulation or a sparse registry;
- release candidate selection/finalization;
- duplicating all generic Linux checks on every platform solely for symmetry;
- new benchmark infrastructure;
- Windows ARM64;
- MSI/MSIX, winget, Chocolatey, Scoop, Homebrew, or other package-manager automation;
- service installation in ordinary GitHub-hosted CI;
- unrelated product refactors.

## Workstream A: correct workspace and dependency version validation

### A1. Preserve one simple manifest convention

The workspace currently defines the version once:

```toml
[workspace.package]
version = "X.Y.Z"
```

Every member manifest currently inherits it:

```toml
version.workspace = true
```

The release checks should validate that convention directly rather than searching member manifests for a nonexistent explicit version.

Required checks:

1. read the workspace version from root `Cargo.toml`;
2. require each of these manifests to contain `version.workspace = true`:
   - `crates/gregg-protocol/Cargo.toml`;
   - `crates/greggd/Cargo.toml`;
   - `crates/gregg/Cargo.toml`;
3. require the normal dependency declaration for `gregg-protocol` in `greggd` to use the same version;
4. require the normal dependency declaration for `gregg-protocol` in `gregg` to use the same version;
5. require any duplicate dev-dependency declaration that includes a registry version to match the same value;
6. fail with the exact manifest and mismatched value when a check fails.

Do not write a generic TOML framework. A small, explicit check for this three-crate workspace is preferred.

### A2. Keep Bash and PowerShell behavior equivalent

Update both scripts with the same semantic contract:

```text
workspace version exists
all three members inherit workspace version
all inter-crate registry constraints match workspace version
clean-tree check runs only in release mode
```

Avoid silently skipping failed matches. In PowerShell, never index `.Matches[0]` before confirming a match exists.

### A3. Correct the runbook

Replace the incorrect member-version grep instructions in `RELEASING.md` with commands that show:

- the root workspace version;
- the three `version.workspace = true` lines;
- the two production `gregg-protocol` dependency constraints;
- any versioned dev-dependency constraint that must remain synchronized.

The runbook should state that the root workspace version is authoritative.

### Workstream A acceptance criteria

- [ ] `scripts/check-local.sh --release` does not fail merely because members use `version.workspace = true`.
- [ ] `scripts/check-local.ps1 -Release` does not index an empty match and validates the same contract.
- [ ] A deliberately mismatched `gregg-protocol` dependency version fails with a clear message.
- [ ] A member missing `version.workspace = true` fails with a clear message.
- [ ] Root workspace version remains the single authoritative package version.
- [ ] `RELEASING.md` describes the actual manifest layout.
- [ ] No TOML parser framework or release metadata generator is added.

## Workstream B: make installed-binary checks use the current checkout

### B1. Correct source installation

Replace registry installation in both local scripts:

```text
cargo install greggd --root <temp> --debug
```

with source-path installation of the current checkout, using the lockfile:

```text
cargo install --path crates/greggd --locked --root <temp> --debug
```

Use platform-correct executable paths.

The command must fail if source installation fails. Do not mask failure with `|| true` or equivalent.

### B2. Run a real bounded product smoke

The existing full tier calls the step an installed-binary loopback smoke but currently runs only help output. Make the name and behavior agree.

Preferred Unix implementation:

1. install the current checkout under a temporary root;
2. reuse `scripts/verify-installed-daemon.sh` if its current interface can verify an arbitrary binary without release-specific assumptions;
3. otherwise run an equally small bounded loopback smoke:
   - create a temporary config;
   - bind to loopback and a collision-safe port;
   - start the installed `greggd` binary;
   - poll `/healthz` and `/v2/status` until ready or timeout;
   - validate JSON success at a basic product level;
   - stop the process;
   - verify no child remains;
   - remove temporary files.

Preferred Windows implementation:

- use the existing Windows foreground smoke/integration helper rather than reproducing service installation;
- if the PowerShell full-check script launches the installed executable directly, use bounded startup, `try/finally`, `/healthz`, `/v2/status`, and process cleanup;
- do not require administrator privileges.

If reusing existing helpers would create complex glue, reduce the full-tier step to an accurately named source-install-and-help smoke and leave the live loopback path to the existing native integration tests. Do not retain a misleading name.

### B3. Preserve package-install isolation

The temporary install root must not modify the maintainer's normal Cargo bin directory.

### Workstream B acceptance criteria

- [ ] Both scripts install `greggd` from `crates/greggd` in the current checkout.
- [ ] Both scripts use `--locked`.
- [ ] The installed binary's version/help is checked.
- [ ] Any step named loopback smoke actually starts and queries the daemon.
- [ ] Failure of the installed binary or daemon smoke fails the full tier.
- [ ] Temporary install/config/process resources are cleaned on success and failure.
- [ ] No crates.io version is used as a substitute for current source validation.

## Workstream C: make release dry-run sequencing truthful and slim

### C1. Local pre-publication release tier

Before any new package version exists on crates.io, the authoritative local release preflight can dry-run only the independent protocol crate:

```text
cargo publish -p gregg-protocol --dry-run --locked
```

Keep package-list checks for all three crates:

```text
cargo package -p gregg-protocol --list
cargo package -p greggd --list
cargo package -p gregg --list
```

Remove dependent-crate `cargo publish --dry-run` commands from the automated local release tier unless the exact protocol version is already present on crates.io and the command is explicitly operator-invoked.

Do not detect registry state automatically. Do not add retries or polling to the local script.

### C2. Manual post-protocol dry-runs

Keep the authoritative dependent sequence in `RELEASING.md`:

1. publish `gregg-protocol` manually;
2. wait until the exact version is visible on crates.io;
3. run:

```text
cargo publish -p greggd --dry-run --locked
cargo publish -p gregg --dry-run --locked
```

4. publish the daemon and client manually.

The runbook should clearly distinguish:

- local pre-publication release preflight;
- post-protocol registry dry-runs;
- actual publication.

### C3. Ensure every dry-run is locked

All release dry-run and publish examples should use `--locked` unless there is a documented Cargo limitation requiring otherwise.

### Workstream C acceptance criteria

- [ ] Unix and Windows release tiers dry-run `gregg-protocol` with `--locked`.
- [ ] They do not automatically dry-run unpublished dependent versions.
- [ ] The dependent dry-runs remain documented after protocol indexing.
- [ ] No local registry simulation is added.
- [ ] No network polling/retry logic is added to local validation scripts.
- [ ] Actual publication remains manual.

## Workstream D: correct cross-platform release smoke documentation

### D1. Use v2 as the universal endpoint

Update the installed-daemon smoke in `RELEASING.md` so the cross-platform status query is:

```text
/v2/status
```

Keep `/healthz` or `/v2/healthz` as the readiness query according to existing daemon semantics.

The runbook must explain:

- Linux/macOS may additionally verify `/v1/status` for compatibility;
- Windows intentionally does not produce a truthful v1 snapshot;
- Windows returning 503 from `/v1/status` is not a release failure when v2 is ready.

### D2. Provide platform-correct examples

The main manual sequence may remain Unix-oriented, but add a concise PowerShell equivalent for Windows foreground verification. It should use:

- a temporary config path;
- `Start-Process` or an equivalent direct process launch;
- bounded readiness polling;
- `Invoke-RestMethod` for `/healthz` and `/v2/status`;
- `try/finally` cleanup.

Do not add a large release script. These are operator commands or a reference to the existing smoke helper.

### Workstream D acceptance criteria

- [ ] The universal runbook smoke uses `/v2/status`.
- [ ] Linux/macOS v1 compatibility is documented as optional/additional.
- [ ] Windows v1 unavailability is documented as intentional.
- [ ] A concise native PowerShell verification path exists.
- [ ] No new publishing automation is introduced.

## Workstream E: test and simplify the local validation entry points

### E1. Bash script behavior

Add or update lightweight tests for:

- `--help` exits zero;
- unknown option exits nonzero;
- default/full/release mode selection;
- release mode accepts `version.workspace = true`;
- dependency mismatch fails;
- source install command includes `--path` and `--locked`;
- release tier dry-runs protocol only;
- child cleanup on smoke failure.

Tests may use PATH-injected fake commands when appropriate. Keep them deterministic and fast.

### E2. PowerShell script behavior

At minimum, perform a native rehearsal of:

- default mode success;
- `-Full` success;
- `-Release` reaches the corrected version check and protocol dry-run;
- a forced child command failure returns nonzero;
- paths with spaces work;
- temporary child cleanup works.

Add Pester only if it is already present. Do not add a new test framework solely for this plan. Small direct PowerShell test helpers or manual commands are acceptable.

### E3. Keep the default tier iterative

Do not add packaging, installed-binary, sustained, or service tests to the default tier. The default remains:

```text
fmt
clippy
tests
docs
cargo-deny when available
platform-native collector tests
```

Full/release tiers may contain the heavier checks.

### E4. Optional CI trimming

Only after CI is green, consider these small reductions:

- Linux owns generic format/docs and comprehensive generic checks;
- Windows retains native Clippy/tests because target-specific code needs native compilation;
- macOS retains native tests for both advertised architectures only if both remain intentional support claims;
- do not add package dry-runs to CI.

CI trimming is optional. Correctness fixes and a passing workflow take precedence.

### Workstream E acceptance criteria

- [ ] Both local entry points have demonstrated failure propagation.
- [ ] The version and source-install regressions are covered directly or by deterministic command assertions.
- [ ] The default tier does not grow.
- [ ] Full/release modes remain readable and bounded.
- [ ] No new heavy test framework is introduced.
- [ ] CI remains one read-only workflow.

## Workstream F: complete native Windows closure checks

### F1. Foreground daemon/client path

On native Windows x86-64, run the existing ordinary product checks:

```powershell
.\scripts\check-local.ps1 -Full
```

Confirm the live or integration smoke exercises:

```text
WindowsCollector
-> sampler warmup
-> cached v2 snapshot
-> HTTP server
-> client poller
-> normalized state
```

Required semantic assertions:

- CPU and physical memory present;
- commit present;
- load absent;
- swap absent;
- CPU I/O-wait absent;
- client renders COMMIT rather than SWAP;
- `/v1/status` does not fabricate zeros;
- v2-to-v1 fallback occurs only on explicit v2 unsupported response.

### F2. Elevated service rehearsal

On a disposable or appropriate native Windows host, run the existing elevated lifecycle smoke from Plan 43.

Required lifecycle:

1. build the current checkout;
2. install service using the current PowerShell installer;
3. verify service account is `NT AUTHORITY\LocalService` or the documented equivalent;
4. start and wait for running/ready;
5. query `/healthz` and `/v2/status`;
6. stop;
7. start again;
8. restart;
9. verify bind/config failure is surfaced;
10. reinstall and confirm documented config-preservation behavior;
11. uninstall while preserving config;
12. reinstall and uninstall with explicit config removal;
13. verify service and child process are gone.

Record only a concise result summary in the implementation handoff or plan status. Do not commit machine identifiers, service dumps, or logs.

### F3. Short resource sanity

During the service/foreground rehearsal, observe only structural regressions:

- no obvious process/handle/thread leak over a short run;
- sample cadence continues;
- request path remains cached;
- stop completes within the existing timeout;
- no rapid retry loop on an offline endpoint.

Do not create benchmark thresholds or an evidence framework.

### Workstream F acceptance criteria

- [ ] Native Windows full local check passes after script corrections.
- [ ] Foreground daemon/client v2 integration passes.
- [ ] Elevated install/start/query/restart/stop/uninstall rehearsal passes.
- [ ] LocalService account and config-preservation behavior match documentation.
- [ ] No structural process/handle/task leak is observed in a short run.
- [ ] Only a concise summary is retained.

## Workstream G: package and ordinary CI closure

### G1. Package content review

Run and inspect:

```text
cargo package -p gregg-protocol --list
cargo package -p greggd --list
cargo package -p gregg --list
```

Confirm:

- v2 protocol source and required fixtures are included;
- Windows daemon source is included;
- Windows installation files referenced by crate documentation are included or the documentation points to the repository accurately;
- licenses/readmes resolve correctly;
- no release-evidence directories or retired scripts enter packages;
- no secrets, local paths, build output, or archived plans enter packages unintentionally.

Do not require package archives to be uploaded as CI artifacts.

### G2. Release dry-run rehearsal

Run the corrected nonpublishing release tier on an available Unix host and Windows host where practical:

```text
./scripts/check-local.sh --release
.\scripts\check-local.ps1 -Release
```

The expected automated dry-run is protocol-only. Review the manual dependent commands in `RELEASING.md`; do not publish.

### G3. Ordinary CI

Require one passing ordinary `ci.yml` execution at the final implementation SHA with:

- Linux;
- macOS Apple Silicon;
- macOS Intel while it remains an advertised supported target;
- Windows x86-64;
- MSRV.

The workflow must still have only:

```yaml
permissions:
  contents: read
```

No release secrets, environments, OIDC, upload-artifact success records, or write permissions are allowed.

If a hosted runner is temporarily unavailable, distinguish infrastructure failure from product failure. Do not create a replacement qualification workflow.

### Workstream G acceptance criteria

- [ ] All three package lists are reviewed.
- [ ] Required Windows source/docs/install files are packaged correctly.
- [ ] Retired release machinery is absent from packages.
- [ ] Corrected Unix release preflight passes.
- [ ] Corrected Windows release preflight passes or any platform-limited protocol dry-run behavior is documented precisely.
- [ ] One ordinary CI run passes all intended jobs.
- [ ] CI remains read-only and nonpublishing.

## Workstream H: reconcile plans, registry, and active documentation

### H1. Update phase statuses truthfully

After implementation and checks, update Plans 040 through 044 with concise closure notes.

Recommended status model:

- Phase 040: completed when native client build/config/lock/editor/poll/TUI checks are demonstrated;
- Phase 041: completed when v2 compatibility and fallback tests pass;
- Phase 042: completed when native collector and foreground daemon v2 smoke pass;
- Phase 043: completed only after elevated SCM rehearsal passes;
- Phase 044: completed only after ordinary CI, mixed-fleet tests, package review, docs, and Windows closure pass;
- Phase 045: completed when all criteria in this file are met.

Do not mark a phase complete based only on source presence.

### H2. Update the registry

Change `plans/README.md` so it no longer calls implemented phases merely planned.

While Phase 45 is open, use statuses such as:

```text
implementation landed; closure pending Phase 45
```

After closure, mark 040 through 045 completed and Plan 036 completed/superseded as appropriate.

Update the dependency summary to include Phase 45.

### H3. Documentation consistency search

Search active files for stale or contradictory statements:

```text
cargo install greggd --root
version = grep member manifests
/v1/status release smoke
Linux or macOS only
client-only Windows
NoopServiceManager
release-finalize
phase35
upload-artifact
cargo publish
```

Expected outcomes:

- `cargo publish` appears in `RELEASING.md`, local nonpublishing release scripts, and archived historical plans only;
- old release workflow names appear only in archive/history where clearly marked;
- active support tables agree on Windows x86-64 and Windows ARM64 unsupported/unverified;
- local command documentation matches actual script options.

### Workstream H acceptance criteria

- [ ] Plans 040 through 045 have truthful status notes.
- [ ] `plans/README.md` identifies Phase 45 as the only remaining closure phase while open.
- [ ] Registry dependency graph includes Phase 45.
- [ ] Active docs contain no stale version-check or v1 universal-smoke instructions.
- [ ] Active docs consistently describe manual publishing and read-only CI.
- [ ] Windows support claims match completed native checks.

## Required implementation order

Use this order to minimize churn:

1. Correct version validation in both local scripts.
2. Correct source-path installation and failure propagation.
3. Correct protocol-only pre-publication dry-run sequencing.
4. Update `RELEASING.md` version and endpoint instructions.
5. Add/update lightweight script regressions.
6. Run fast local checks on the implementation host.
7. Run full/release local checks on Unix.
8. Run native Windows full check and foreground integration.
9. Run elevated Windows service rehearsal.
10. Review all package lists.
11. Push final code/docs.
12. Require one ordinary CI run at the final SHA.
13. Reconcile Plans 040 through 045 and `plans/README.md`.

Do not mark the plan complete before steps 8, 9, and 12 are resolved truthfully.

## Suggested command matrix

### Linux/macOS development

```bash
./scripts/check-local.sh
./scripts/check-local.sh --full
./scripts/check-local.sh --release
```

### Windows development

```powershell
.\scripts\check-local.ps1
.\scripts\check-local.ps1 -Full
.\scripts\check-local.ps1 -Release
```

### Package review

```bash
cargo package -p gregg-protocol --list
cargo package -p greggd --list
cargo package -p gregg --list
```

### Manual release sequencing rehearsal

Pre-publication:

```bash
cargo publish -p gregg-protocol --dry-run --locked
```

Post-protocol publication, documented but not executed for this phase:

```bash
cargo publish -p greggd --dry-run --locked
cargo publish -p gregg --dry-run --locked
```

### Repository policy search

```bash
rg -n "release-finalize|phase35|upload-artifact|actions/upload-artifact" \
  .github scripts architecture README.md RELEASING.md AGENTS.md CONTRIBUTING.md plans/README.md

rg -n "cargo install greggd --root|/v1/status|version\.workspace|gregg-protocol.*version" \
  scripts RELEASING.md README.md AGENTS.md CONTRIBUTING.md
```

Interpret search results; archived historical plans are allowed to mention retired mechanisms.

## Phase acceptance criteria

Phase 45 is complete only when all of the following are true:

- [ ] Unix release-version validation matches `version.workspace = true`.
- [ ] PowerShell release-version validation matches `version.workspace = true`.
- [ ] Inter-crate `gregg-protocol` registry constraints are checked against the workspace version.
- [ ] Local full checks install `greggd` from the current checkout with `--path` and `--locked`.
- [ ] Any installed-binary smoke failure propagates nonzero.
- [ ] Any step called a loopback smoke performs a real bounded loopback query, or is renamed to state its smaller behavior accurately.
- [ ] Local release tiers dry-run only `gregg-protocol` before publication.
- [ ] All local dry-run commands use `--locked`.
- [ ] `RELEASING.md` preserves manual protocol-first publication and dependent dry-runs after indexing.
- [ ] Cross-platform release smoke uses `/v2/status`.
- [ ] Linux/macOS v1 compatibility and Windows v1 unavailability are documented correctly.
- [ ] Lightweight regression coverage exists for the corrected script behavior.
- [ ] Unix full and release checks pass.
- [ ] Windows full and release checks pass, subject only to clearly documented non-product registry availability constraints.
- [ ] Native Windows foreground daemon/client integration passes.
- [ ] Elevated Windows service lifecycle rehearsal passes.
- [ ] Short Windows resource/process sanity reveals no structural leak.
- [ ] All three package lists are reviewed and clean.
- [ ] One ordinary CI run passes Linux, macOS, Windows, and MSRV jobs at the final SHA.
- [ ] CI retains read-only permissions and has no publishing/evidence behavior.
- [ ] Plans 040 through 045 and the registry are reconciled truthfully.
- [ ] No new release workflow, candidate system, provenance framework, evidence bundle, or local registry simulation exists.

## Closure record format

Keep the final handoff summary concise:

```text
Phase 45 final SHA: <sha>
Unix default/full/release: pass|fail with one-line reason
Windows default/full/release: pass|fail with one-line reason
Windows foreground v2 smoke: pass|fail
Windows elevated SCM rehearsal: pass|fail
Package lists: reviewed
Ordinary CI run: <run id or link>, all jobs pass|remaining infrastructure issue
Registry/plans reconciled: yes|no
Publishing performed: no
```

Do not commit raw logs, generated manifests, host identifiers, checksums, or artifact bundles.

## Handoff guidance for a smaller implementation model

1. Do not rewrite the release system. Fix the three files first: `scripts/check-local.sh`, `scripts/check-local.ps1`, and `RELEASING.md`.
2. Treat `version.workspace = true` as required for all three members; compare only dependency constraints to the root workspace version.
3. Add `--path crates/greggd --locked` to source installation. Never use the registry package to test the checkout.
4. Remove dependent publish dry-runs from automated pre-publication scripts. Leave them in the manual post-protocol runbook.
5. Use `/v2/status` for universal smokes.
6. Reuse existing daemon and Windows smoke helpers before writing new code.
7. Do not add a new workflow, action, artifact, schema, or evidence file.
8. Run native Windows service checks manually; do not attempt elevation in ordinary CI.
9. Update plan statuses only after the corresponding native/CI checks are complete.
10. Stop when the explicit acceptance criteria above are satisfied; unrelated cleanup belongs in a separate future plan.
