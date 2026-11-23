# Plan 195: Eggpack contract and planning adoption

Status: implementation complete; existing native CI pending.

Source: user request to assess and plan a safe Eggpack adoption; upstream and
workspace review performed 2026-11-19.

Depends on: Plans 099 and 104 (five-target release contract), 190 (pinned
release tooling), 194 (release preflight), and the current post-194 main.

## Objective

Adopt Eggpack as a source-pinned validator for Gregg's existing binary-release
contract and release plan. Run it inside the existing tagged/manual release
workflow, without changing the build, smoke, checksum, asset-upload, or draft
release behavior. This is a deliberately narrow first adoption: it proves that
Eggpack can describe Gregg's two binaries, five targets, and v-prefixed tags
without becoming a second release authority.

Eggpack must not take authority away from the existing release preflight. The
current script remains responsible for workspace version, tag/HEAD identity,
crate visibility, and release eligibility; scripts/release-targets.txt remains
the canonical target list. Eggpack validates a representation of that contract
and its build/qualification bindings against the canonical list.

## Research findings

Eggpack's schema supports direct assets and SHA-256 sidecars, with explicit
product, target, asset, and install names. Its planning and validation surface
can describe the release identity and build/qualification bindings without
performing network access. The producer core supports cargo-zigbuild and
records compatibility-floor status, but native qualification alone is not an
independent proof of a declared glibc floor. Preserve Gregg's existing
2.17-targeted cargo-zigbuild command and do not relabel its evidence.

The Eggpack CLI is documented as a source-built workspace tool, not a
crates.io dependency. Build it from one immutable 40-character upstream Git
revision with its lockfile enforced. Do not use main/latest, add Eggpack crates
to Gregg's runtime dependency graph, or add a CI job. The CLI, contract, and
producer design are documented in the upstream Eggpack README, contract/core
READMEs, CLI reference, and release-identity policy (links below).

## Frozen Gregg contract

The plan must preserve all of the following:

- Five targets: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
  x86_64-apple-darwin, aarch64-apple-darwin, and x86_64-pc-windows-msvc.
- Separate gregg and greggd executables named by program plus target, with the
  .exe suffix only on Windows; the release tag, not the filename, carries the
  version.
- One .sha256 sidecar for each executable, with the current digest and
  filename format; install.sh and install.ps1 remain release assets.
- Linux GNU artifacts keep the glibc 2.17 floor through the current
  cargo-zigbuild invocation.
- Existing version/help and native daemon health smokes, exact-asset checks,
  installer checks, and the tag/version/HEAD/crates.io preflight remain gates.
- The release workflow remains tag/manual-dispatch driven and draft-only. It
  must not create tags, publish crates, publish a final release, or add release
  permissions beyond those already present.

The current contract and implementation are recorded in
scripts/release-targets.txt, crates/gregg-update/src/target.rs,
scripts/release-check-assets.sh, packaging/install.sh,
packaging/install.ps1, and .github/workflows/release-binaries.yml.

## Scope

In scope:

- Add a small Eggpack configuration set under packaging/eggpack/ for the
  distribution contract, pack policy, build bindings, and qualification
  bindings. Model the vX.Y.Z tag as release identity X.Y.Z while retaining the
  exact source tag and source revision as separate facts.
- Prove how Eggpack represents the two separately downloadable binaries. Use
  separate direct contracts unless a bundle representation demonstrably
  produces the exact existing public filenames; never rename or combine the
  release assets to fit the tool.
- Store the upstream Eggpack source revision once under packaging/eggpack/ and
  have the existing release preflight job build the CLI with --locked from
  that revision.
- Run Eggpack check and plan validation in the existing release preflight job.
  Keep generated plans and any temporary validation output outside the
  uploaded asset directory.
- Add a deterministic contract test that compares Eggpack's expanded target
  and asset names with scripts/release-targets.txt and the existing Rust,
  installer, and asset-check mappings. The current target file remains the
  source of truth; no second hand-maintained target table is allowed.
- Update architecture/scripts-and-packaging.md, RELEASING.md, the release
  process skill, and plans/README.md to describe the limited integration.

Out of scope:

- Replacing Cargo build steps, checksum generation, asset staging, smoke tests,
  scripts/release-check-assets.sh, or GitHub draft assembly with Eggpack.
- Eggpack-generated GitHub Actions workflows, a new workflow/job/matrix, or
  release-manifest.json as a published asset or retained evidence bundle.
- Any change to gregg-update, the self-update policy, installer behavior,
  release asset names, version authority, crate publication order, or runtime
  dependencies.
- Eggup adoption, service-manager changes, signatures, provenance, SBOMs, or
  claims that SHA-256 establishes release authenticity.

A later full producer cutover would need a separate plan after this contract
pilot passes and after its scope is reconciled with the repository's current
manual-release/no-generalized-evidence direction.

## Phase 1: establish the contract

1. Pin a reviewed Eggpack commit in one checked-in revision file. The release
   workflow must fetch exactly that commit and build eggpack-cli with the
   upstream lockfile; a missing revision or build failure is a hard preflight
   failure.
2. Create the contract, pack policy, build bindings, and qualification
   bindings. Keep the five target triples in exact parity with
   scripts/release-targets.txt. Bind both Cargo packages/binaries and preserve
   the current version/help smoke commands. Keep the daemon health smoke in
   Gregg's existing native smoke because it owns Gregg-specific temporary
   config and HTTP readiness behavior.
3. Derive the release ID from the already-validated vX.Y.Z tag. Pass the
   checked-out 40-character source revision to Eggpack. Do not let Eggpack
   choose a tag, version, source checkout, or public release state.
4. Add fixture coverage for supported targets, the Windows executable suffix,
   both product names, checksum sidecars, and tag-to-release-ID mapping. Include
   negative cases for a missing/extra target, wrong product name, wrong suffix,
   and invalid tag/release identity.

## Phase 2: shadow and enable the gate

1. Run Eggpack validation against deterministic fixtures in the existing
   scripts test job and against a release-plan fixture locally. Do not add a
   job or publish/modify a GitHub Release for this validation.
2. Add Eggpack's pinned CLI build and check/plan invocation to the existing
   release-binaries preflight job after scripts/release-preflight.sh succeeds.
   Preserve all current workflow permissions and the release-preflight script's
   tag, version, HEAD, and registry checks.
3. For one candidate artifact inventory, compare Eggpack's expanded contract
   with the existing release-check-assets.sh result. If Eggpack requires
   replacing build, qualification, checksum, or upload steps to perform this
   comparison, stop and keep the current pipeline rather than silently widening
   this plan.
4. Once the fixture and inventory comparisons pass, make Eggpack validation a
   required preflight step. The existing asset checker and native smoke gates
   remain required and unchanged.

## Tests and evidence

- Contract tests prove exact equality with the five targets and expected
  gregg/greggd filenames and .sha256 sidecars. Tests must fail if either the
  Eggpack config or current release mapping drifts.
- The pinned Eggpack CLI builds with --locked and runs check/plan successfully
  at Rust 1.89. Invalid configs, tag mappings, and target inventories fail
  before any release artifact is uploaded.
- Existing scripts/release-preflight.sh and release-check-assets.sh behavior
  remains unchanged; the release workflow still produces the same expected
  executable, sidecar, and installer inventory.
- Existing version/help, daemon loopback health, installer parse, draft-only,
  and platform build steps remain in the same workflow and pass on their
  existing native runners.
- Run cargo fmt --all -- --check, ./scripts/check-local.sh --release,
  shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh, and
  python3 -m pytest scripts/tests -q. Run the existing native CI workflow;
  do not add a CI job or matrix.

## Acceptance criteria

- [x] Eggpack is pinned to one immutable upstream commit and built with its
      lockfile; Gregg's Cargo.toml and Cargo.lock gain no Eggpack runtime or
      build dependency.
- [x] Eggpack contract and bindings represent both binaries and exactly the
      five existing targets, preserving all asset and checksum names.
- [x] scripts/release-targets.txt remains canonical and a regression test
      detects drift between it and the Eggpack inputs.
- [x] The existing release preflight job runs Eggpack check/plan only after
      Gregg's tag/version/HEAD/crates.io preflight succeeds; failures stop the
      release before artifact upload.
- [x] The workflow, release permissions, target matrix, Linux 2.17 build,
      native smokes, installers, release assets, and draft-only behavior are
      unchanged; no new job, workflow, or public manifest is added.
- [x] Focused script tests, the version preflight, shellcheck, pytest, workflow
      YAML parsing, and the actionlint workflow checks (excluding documented
      pre-existing ShellCheck notices) pass locally.
- [ ] The existing native CI workflow completes green on this implementation;
      hosted CI was not run during this implementation.

## Implementation record

The initial implementation pinned Eggpack CLI source revision
`242654ab29e59ebbd3beca0a0ff276f4df4df873`. After Eggpack v0.1.2 was
released, the pin was refreshed to its signed/tagged source commit
`35b3fc26de63437947aa4f36764c17bd0ebb9f65`; it builds with Rust 1.89 and
its upstream lockfile. Separate direct contracts cover `gregg` and `greggd`;
shared producer policy and product-specific build/qualification
bindings remain under `packaging/eggpack/`. The contract test exercises the
expanded names and drift/mutation cases against the canonical target table.

`scripts/release-eggpack-preflight.sh` validates the tag/version/HEAD identity,
derives targets from `scripts/release-targets.txt`, and runs Eggpack check and
plan with generated plans confined to temporary scratch space. The existing
release workflow invokes it only after `scripts/release-preflight.sh` succeeds.
No Eggpack dependency was added to Gregg, and the existing target matrix,
builds, Linux floor, smokes, checksums, installers, asset checks, draft assembly,
and updater are unchanged.

Local evidence: the pinned CLI built successfully; the script suite passed
(74 passed, 2 skipped); ShellCheck, shell syntax checks, workflow YAML parsing,
actionlint with `SC2034` and `SC2016` exclusions, the existing version
preflight, and `git diff --check` passed. Unfiltered actionlint reports those
five ShellCheck notices on existing release-workflow steps. The full release
wrapper was not run because local tag `v1.0.15` does not point to this HEAD;
the version preflight was run with Git identity checks skipped. Hosted native
CI remains the only outstanding acceptance item.

## Risks and stop conditions

- If Eggpack cannot express Gregg's two independent executable assets without
  changing their public names or layout, retain the current pipeline and close
  this plan as not adopted rather than adding translation machinery that
  becomes a second release system.
- The pack policy duplicates some target-specific build facts. The parity test
  must compare those target identities to scripts/release-targets.txt; only
  strategy/host/floor facts may be independently specified.
- Eggpack's floor status distinguishes toolchain-bound from independently
  verified floors. Gregg's 2.17 floor remains bound to the existing Zig target
  and must not be described as independently proven by a native smoke.
- The CLI is source-built. A moving branch, unpinned action/tool reference, or
  unlocked build is a release supply-chain regression and fails acceptance.
- SHA-256 and Eggpack's ReleaseManifest describe integrity, not authenticity.
  This plan does not increase the trust claim made by Gregg's releases.

## References

- Gregg target and asset contract: scripts/release-targets.txt;
  architecture/scripts-and-packaging.md; Plan 099.
- Current release workflow: .github/workflows/release-binaries.yml and
  scripts/release-preflight.sh.
- Eggpack overview: https://raw.githubusercontent.com/eggstack/eggpack/main/README.md
- Eggpack contract: https://raw.githubusercontent.com/eggstack/eggpack/main/crates/eggpack-contract/README.md
- Eggpack build and floor policy: https://raw.githubusercontent.com/eggstack/eggpack/main/crates/eggpack-core/README.md
- Eggpack CLI: https://raw.githubusercontent.com/eggstack/eggpack/main/crates/eggpack-cli/README.md
- Eggpack release identity: https://raw.githubusercontent.com/eggstack/eggpack/main/docs/release-identity-policy.md
