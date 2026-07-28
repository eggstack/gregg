# Phase 39: concise manual crates.io and GitHub release process

## Objective

Create one short, version-neutral operator runbook for manually publishing Gregg's crates to crates.io and manually creating the corresponding Git tag and GitHub Release.

This phase does not automate publication. It defines a correct sequence, a small preflight, explicit partial-failure handling, and a lightweight post-release smoke. The process must be quick enough that a maintainer can execute it directly without orchestrating workflow stages or preserving evidence bundles.

## Dependency and execution position

Depends on:

- Phase 37 removing automated release machinery;
- Phase 38 establishing the local validation command and minimal CI.

This phase may complete before Windows support. The runbook must remain valid when Windows support later changes package descriptions, dependencies, or documentation.

## Governing invariants

1. All real publication actions are performed manually by an operator.
2. GitHub Actions has no publication role.
3. The runbook contains no hardcoded release version.
4. Publication order respects workspace dependencies.
5. The operator verifies crates.io availability before publishing dependent crates.
6. The Git tag and GitHub Release are created after the intended crates.io publications succeed.
7. A published crates.io version is treated as immutable.
8. Partial publication is handled by inspection and a new version when correction is required, not by attempting overwrite or workflow repair.
9. The repository contains no executable helper that performs the real publish/tag/release actions.
10. Release evidence is the public crates.io records, Git tag, GitHub Release, and normal local command output—not a generated internal manifest.

## Scope

### In scope

- root `RELEASING.md`;
- links from README, CONTRIBUTING, AGENTS, and plan index;
- package order and dependency-index waiting;
- local preflight commands;
- version/changelog checks;
- dry-run packaging and publication checks;
- actual manual `cargo publish` commands shown as operator instructions;
- annotated tag creation and push;
- manual GitHub Release creation through the GitHub UI or `gh` CLI;
- concise post-release verification;
- partial-failure and rollback guidance;
- removal of stale release-process documentation missed by Phase 37.

### Out of scope

- a release workflow;
- a release script that executes publication;
- crates.io API automation;
- automatic changelog generation;
- automatic version bumping;
- automatic tag or GitHub Release creation;
- signing, attestations, SBOM generation, provenance manifests, or binary attachments;
- package-manager distribution;
- backport/release-branch policy;
- nightly or prerelease channels.

## Workstream A: define the operator prerequisites

`RELEASING.md` must begin with a concise prerequisites section.

Required operator state:

- authenticated crates.io access through Cargo's normal credential mechanism;
- permission to push tags to `eggstack/gregg`;
- permission to create a GitHub Release;
- clean local checkout of the intended release commit;
- current `main` fetched from the remote;
- release version selected before preflight;
- all workspace crate versions and inter-crate dependency versions updated consistently;
- changelog and README support tables updated.

Do not document storing a crates.io token in repository files, shell history, GitHub Actions, or plan files.

### Required variables

Use examples like:

```bash
VERSION="X.Y.Z"
TAG="v${VERSION}"
```

Commands should reference those variables or a clearly visible placeholder. No `1.0.1`-specific branches are allowed.

### Workstream A acceptance criteria

- [ ] Prerequisites are complete but short.
- [ ] Credential handling uses normal local tools and no repository secret.
- [ ] Version examples are generic.
- [ ] Required repository permissions are identified.

## Workstream B: version and source preflight

The runbook must require:

```bash
git fetch origin
git switch main
git pull --ff-only origin main
git status --short
```

A nonempty status blocks release unless the operator intentionally commits the changes first.

Verify version consistency using a simple inspection command or a small nonpublishing check. At minimum confirm:

- workspace package version equals `$VERSION`;
- `greggd` depends on the intended `gregg-protocol` version;
- `gregg` depends on the intended `gregg-protocol` version;
- changelog contains the release version/date;
- crate descriptions and supported-platform documentation are current;
- `Cargo.lock` is committed and current where the repository policy requires it.

The check may be implemented as part of `scripts/check-local.sh --release`, but it must not mutate versions or files.

### Local validation sequence

Require:

```bash
./scripts/check-local.sh --full
```

or the exact canonical equivalent from Phase 38.

Then run release-specific package checks separately so they remain visible:

```bash
cargo package -p gregg-protocol --list
cargo publish -p gregg-protocol --dry-run --locked

cargo package -p greggd --list
cargo publish -p greggd --dry-run --locked

cargo package -p gregg --list
cargo publish -p gregg --dry-run --locked
```

If dependent-crate dry-runs cannot resolve the new protocol version before it is published, document the correct staged behavior:

1. fully validate and publish `gregg-protocol`;
2. wait until the exact protocol version resolves from crates.io;
3. run dependent-crate dry-runs and publications.

Do not restore a local sparse registry or simulated Boundary-2 system merely to dry-run all three before publication.

### Workstream B acceptance criteria

- [ ] Clean-tree and remote-sync checks are explicit.
- [ ] Version consistency is checked without mutation.
- [ ] Full local product validation precedes publication.
- [ ] Package contents are inspected for all three crates.
- [ ] Dry-run behavior accounts for registry dependency ordering without local registry simulation.

## Workstream C: publish to crates.io in dependency order

The manual publication order is mandatory:

```text
1. gregg-protocol
2. greggd
3. gregg
```

### Step 1: publish protocol

```bash
cargo publish -p gregg-protocol --locked
```

Then confirm the exact version is available from crates.io before continuing. Acceptable checks include:

```bash
cargo search gregg-protocol --limit 1
```

and a clean temporary consumer project using:

```toml
gregg-protocol = "=X.Y.Z"
```

The consumer check should run `cargo check` against crates.io without a path override.

### Step 2: re-run dependent dry-runs

After protocol availability:

```bash
cargo publish -p greggd --dry-run --locked
cargo publish -p gregg --dry-run --locked
```

This is the authoritative dependent-package package check because it resolves the published protocol version.

### Step 3: publish daemon and client

```bash
cargo publish -p greggd --locked
cargo publish -p gregg --locked
```

Do not publish the two dependent crates concurrently. Sequential publication produces clearer operator state and simpler partial-failure handling.

### Workstream C acceptance criteria

- [ ] Publication order is unambiguous.
- [ ] Protocol registry availability is checked before dependent publication.
- [ ] Dependent dry-runs are repeated after protocol publication.
- [ ] Real publication commands exist only in operator documentation.
- [ ] No command is wrapped by a repository publish script.

## Workstream D: partial publication and failure handling

`RELEASING.md` must include a compact decision table.

### Failure before any publication

Fix the source, rerun checks, and release the same planned version if no crate with that version was uploaded.

### Protocol published, dependent crate fails before upload

Determine whether the failure is local/package configuration or registry propagation.

- If no source correction is needed, fix the local/environment problem and retry the same dependent crate version.
- If source or packaged content must change, bump the workspace to a new version. Do not attempt to replace the published protocol version. Publish the corrected protocol under the new version, then the dependents.

### Protocol and daemon published, client fails

- If the client package can be retried unchanged, retry it.
- If source must change, use a new version for the corrected workspace release.
- Do not create the final Git tag/GitHub Release for an incomplete intended release unless intentionally documenting a partial release.

### Cargo reports a timeout after upload

Do not immediately republish. First inspect crates.io for the exact package/version. Cargo may have uploaded successfully before the index became visible.

### Published package is incorrect

- Yank only when appropriate and consciously chosen.
- Prepare a corrected version.
- Do not delete, overwrite, or reuse the immutable version.
- Document the correction in the changelog/release notes.

### Workstream D acceptance criteria

- [ ] Partial publication states are documented.
- [ ] The runbook distinguishes retrying an unchanged upload from correcting source.
- [ ] Version immutability is explicit.
- [ ] Timeout handling requires registry inspection before retry.
- [ ] No automated rollback is proposed.

## Workstream E: create the annotated tag manually

After all intended crates are visible on crates.io and post-publication install checks pass:

```bash
git status --short
git rev-parse HEAD
git tag -a "$TAG" -m "gregg $VERSION"
git push origin "$TAG"
```

The runbook should instruct the operator to confirm:

- the tag points to the exact release commit;
- the release commit contains the matching versions and changelog;
- no additional source changes occurred after publication;
- the tag name follows `vX.Y.Z`.

Tag signing may be used by operator preference but is not required by this roadmap.

Do not add a workflow that watches tags and publishes or creates releases.

### Workstream E acceptance criteria

- [ ] Tagging occurs after successful crates.io publication.
- [ ] The tag is annotated.
- [ ] Tag target/version consistency is checked.
- [ ] No tag-triggered publication workflow exists.

## Workstream F: create the GitHub Release manually

Document two supported manual paths.

### GitHub UI

1. Open the repository Releases page.
2. Draft a new release.
3. Select the existing annotated tag.
4. Use `Gregg X.Y.Z` as the title.
5. Paste concise release notes derived from the changelog.
6. Mark prerelease only when intentionally publishing a prerelease.
7. Publish the release.

### GitHub CLI

```bash
gh release create "$TAG" \
  --title "Gregg $VERSION" \
  --notes-file /path/to/release-notes.md \
  --verify-tag
```

The CLI is an operator command, not a checked-in script.

No binary artifacts are required. crates.io remains the installation channel. Source archives automatically associated with the GitHub tag are sufficient unless a future plan deliberately adds binaries.

### Release notes minimum content

- concise summary;
- user-visible changes;
- supported-platform changes;
- important fixes;
- known limitations or compatibility notes;
- crates.io installation commands.

Do not include internal evidence IDs or CI-run metadata.

### Workstream F acceptance criteria

- [ ] Both UI and CLI manual methods are documented.
- [ ] GitHub Release creation uses the already-pushed tag.
- [ ] Binary attachments are optional and not required.
- [ ] Release notes are user-focused.
- [ ] No repository automation creates the release.

## Workstream G: post-release verification

Keep post-release verification short and user-representative.

Use clean temporary install roots or normal Cargo install behavior:

```bash
cargo install greggd --version "=$VERSION" --locked
cargo install gregg --version "=$VERSION" --locked

greggd --version
greggd --help
gregg --version
gregg --help
```

On a native supported host, run a short foreground daemon smoke with a temporary config and loopback binding, then query health/status and terminate it cleanly.

For the protocol crate, use a clean temporary consumer project resolving exactly `$VERSION` from crates.io.

The runbook may include optional platform smokes for Linux, macOS, and Windows, but failure to have every platform physically available to the releasing operator does not trigger a release-evidence workflow. Product CI and prior development testing provide representative platform confidence.

### Workstream G acceptance criteria

- [ ] Published daemon/client install from crates.io.
- [ ] `--help` and `--version` work from installed binaries.
- [ ] Protocol resolves in a clean consumer.
- [ ] At least one native loopback daemon smoke is documented.
- [ ] Verification produces no retained evidence bundle.

## Workstream H: documentation integration

Update:

- `README.md` with a short `Releasing` link;
- `CONTRIBUTING.md` to state maintainers publish manually;
- `AGENTS.md` to prohibit adding automated publication without an explicit new decision;
- `plans/README.md` with Phase 39 status;
- `CHANGELOG.md` only when implementing an actual release, not merely this plan.

Recommended README text:

```text
Releases are published manually to crates.io and GitHub. CI never publishes.
Maintainer instructions are in RELEASING.md.
```

### Workstream H acceptance criteria

- [ ] `RELEASING.md` is the single active release source of truth.
- [ ] Other docs link rather than duplicate the full procedure.
- [ ] Agent instructions preserve the no-automated-publishing decision.
- [ ] No stale staged-release instructions remain.

## Required runbook outline

The final `RELEASING.md` should remain compact and use this approximate structure:

```text
Purpose and policy
Prerequisites
Choose/version the release
Sync and clean-tree check
Run local validation
Publish gregg-protocol
Wait for crates.io resolution
Dry-run and publish greggd
Dry-run and publish gregg
Install/consumer smoke
Create and push annotated tag
Create GitHub Release
Partial-failure handling
```

Avoid embedding architecture discussion, historical release rationale, or large troubleshooting catalogs.

## Test cases and manual rehearsal

Before marking the phase complete, rehearse the runbook without publication:

1. substitute a nonpublish test version only in a disposable branch/worktree or use the current version without mutation;
2. run clean-tree/version checks;
3. run the full local validation command;
4. run package listing for all crates;
5. run all feasible dry-runs;
6. verify the script/helper path cannot perform real publication;
7. validate tag commands syntactically without pushing a test tag;
8. validate `gh release create` command construction without creating a release, for example by review rather than execution;
9. inspect all docs for hardcoded release versions;
10. search workflows/scripts for `cargo publish`, `gh release create`, and tag-push commands.

## Phase acceptance criteria

Phase 39 is complete only when:

- [ ] Root `RELEASING.md` exists and is version-neutral.
- [ ] The process is manual for crates.io, tags, and GitHub Releases.
- [ ] Publication order is protocol, daemon, client.
- [ ] Registry availability is checked between protocol and dependent crates.
- [ ] Full local validation precedes publication.
- [ ] Package listing and dry-run checks are explicit.
- [ ] Partial-publication and immutable-version behavior is documented.
- [ ] Annotated tag creation follows successful publication.
- [ ] GitHub Release creation is manual and uses the existing tag.
- [ ] Post-release install/consumer smokes are concise and user-representative.
- [ ] No executable repository automation performs publication, tagging, or GitHub Release creation.
- [ ] Other active docs link to `RELEASING.md` rather than duplicating obsolete procedures.
- [ ] The dry-run rehearsal succeeds.

## Evidence required for completion

Only:

- the committed runbook and documentation links;
- successful local nonpublishing rehearsal commands;
- repository search confirming no automated publication path.

Do not create a release manifest, candidate ledger, provenance bundle, or CI artifact.

## Handoff notes for a smaller implementation model

1. Write the runbook after Phase 38 establishes the exact local command name.
2. Keep operator commands visible; do not hide them in helper scripts.
3. Test package ordering carefully because dependent dry-runs may need the new protocol version in crates.io.
4. Do not add a local registry simulator to avoid that natural staging.
5. Keep the failure-handling section concise and operational.
6. Search every workflow and script before completion to ensure real publish/tag/release commands exist only in documentation.
7. Avoid changing crate versions while implementing this plan unless an actual release is also requested.