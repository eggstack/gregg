# Phase 10: crates.io publication and version-1 release closure

## Objective

Publish mutually compatible version `1.0.0` releases of `gregg-protocol`, `greggd`, and `gregg` to crates.io, tag the exact source, and provide reproducible installation and upgrade documentation.

This phase is release closure, not feature development. Any code change must address a release-blocking issue and must rerun the affected phase-9 evidence.

## Publication order

Publish in dependency order:

1. `gregg-protocol` `1.0.0`
2. `greggd` `1.0.0`
3. `gregg` `1.0.0`

Wait until crates.io indexing makes each dependency resolvable before publishing dependents. Do not work around indexing delays by changing dependency sources or publishing packages with path-only dependencies.

## License and ownership closure

Before release:

- Select and add the project license file(s).
- Use one accurate SPDX license expression in all package manifests.
- Verify all direct and transitive dependency licenses against project policy.
- Confirm repository ownership and crates.io owner/team membership for all three crate names.
- Enable crates.io trusted publishing or securely managed token-based publishing according to current crates.io capabilities and organization policy.
- Document who can yank/release and the recovery procedure for a compromised release credential.

Do not publish with placeholder ownership, missing license files, or an ambiguous license expression.

## Manifest completion

Each package must include:

- `name = "..."`
- `version = "1.0.0"`
- Rust edition.
- Declared MSRV through `rust-version`.
- Accurate description.
- Repository and homepage.
- License expression and included license files.
- Readme.
- Keywords and crates.io categories.
- Explicit binary/library targets where defaults are unclear.
- Intentional feature declarations and default feature set.
- Package include/exclude policy.

Workspace dependency declarations for published crates should combine version and local path:

```toml
gregg-protocol = { version = "1.0.0", path = "../gregg-protocol" }
```

Inspect generated manifests inside `.crate` archives to confirm Cargo rewrites local paths as expected.

## Public API review

### `gregg-protocol`

Because this is a library crate, perform a deliberate semver review:

- Public types and fields needed by downstream consumers are intentional.
- Internal construction helpers are not accidentally public.
- Units and optional-field semantics are documented.
- Schema version and compatibility policy are prominent.
- Serde attributes are stable and fixture-protected.
- `#[non_exhaustive]` is used only where it improves forward compatibility without making ordinary construction needlessly difficult.
- Error/validation types are useful and not tied to daemon/client internals.

Generate and inspect rustdoc with warnings denied where practical.

### Binary crates

The `greggd` and `gregg` packages may contain internal libraries for testability. Decide whether those library APIs are public/stable or package-private implementation details. If they are not intended for downstream use, minimize exported items and document that the supported interface is the binary/CLI.

Review `--help`, examples, exit codes, config locations, and platform support as public version-1 contracts.

## Documentation closure

Update root README with verified rather than prospective instructions:

- Supported target table.
- crates.io installation commands.
- Binary names and command examples.
- Daemon config paths and sample config.
- systemd installation/use.
- launchd installation/use.
- TUI config commands and navigation.
- API example and schema documentation link.
- Private-network exposure warning.
- macOS I/O-wait limitation.
- Resource measurements or link to evidence.
- Scope/non-goals.

Add per-crate README content if crates.io rendering from the root README would be confusing. The protocol crate should contain a concise serialization example. The binaries should contain installation and minimal invocation examples.

Create:

```text
CHANGELOG.md
SECURITY.md
CONTRIBUTING.md
```

`SECURITY.md` should describe supported versions and a private vulnerability-reporting channel. It must not imply public-internet hardening beyond the project scope.

## Version and schema compatibility

Before tagging, document:

- Crate version `1.0.0` does not imply schema version equals package version; the JSON schema remains explicitly versioned.
- Version-1 client behavior toward unknown schema majors.
- Additive optional-field policy.
- Supported daemon/client version skew.
- Config-version migration policy.
- MSRV policy and how increases will be communicated semantically.

Test the final `gregg 1.0.0` client against canonical and running `greggd 1.0.0` instances on Linux and macOS.

## Package inspection

For every crate, run from a clean tree:

```text
cargo package -p <crate>
cargo package -p <crate> --list
```

Inspect package contents for:

- Missing README/license/templates/fixtures needed at runtime or for documentation.
- Accidental plan files or large test artifacts in binary packages unless intentionally included.
- Secrets, local absolute paths, editor files, or generated benchmark outputs.
- Unnecessary CI/repository files.
- Correct normalized Cargo.toml.
- Correct executable targets.

Unpack each `.crate` archive into a clean temporary directory and run its package verification tests. Install binary packages from the local packaged archive rather than the workspace:

```text
cargo install --path <unpacked-greggd-package>
cargo install --path <unpacked-gregg-package>
```

Verify `greggd --help`, `gregg --help`, config-path behavior, and a loopback daemon/client smoke test using those installed binaries.

## Release CI

Create a tag/release workflow only after manual dry runs succeed. It should:

- Trigger on an annotated version tag or manual protected invocation.
- Re-run formatting, linting, tests, docs, package validation, and supported target builds.
- Build release artifacts for intended target triples if binary artifacts are distributed through GitHub Releases.
- Generate checksums.
- Attach service templates/install documentation where appropriate.
- Use trusted publishing or environment-protected secrets.
- Prevent partial unordered publication where possible.

Crates.io publication should remain an explicit protected action. Avoid automatically publishing every tag without review.

If binary archives are shipped, define deterministic archive layout and naming, for example:

```text
greggd-1.0.0-aarch64-unknown-linux-gnu.tar.gz
gregg-1.0.0-aarch64-apple-darwin.tar.gz
```

Do not claim a target as supported solely because an archive cross-compiled successfully.

## Release commit and tagging

Prepare one release commit containing only version/metadata/changelog/documentation changes required for `1.0.0`. Require a clean tree and green CI.

Use an annotated tag:

```text
v1.0.0
```

The tag must point to the exact commit used to produce all crates.io packages and GitHub artifacts. Record package checksums and published crate URLs in the GitHub release notes.

Do not retag or force-move a public release tag. Correct post-publication errors through a new patch release; yank only when necessary and document the reason.

## Publication runbook

1. Freeze merges except release blockers.
2. Confirm phase-9 evidence references the release candidate commit or rerun after changes.
3. Confirm crates.io names and ownership.
4. Run full CI and native platform smoke tests.
5. Run package/list/unpack/install verification for all three crates.
6. Commit final version/changelog/docs changes.
7. Run CI on the final commit.
8. Create annotated `v1.0.0` tag.
9. Publish `gregg-protocol` and wait for indexing.
10. Install/compile a dependent test against crates.io `gregg-protocol = "1.0.0"`.
11. Publish `greggd`; wait and verify crates.io installation.
12. Publish `gregg`; wait and verify crates.io installation.
13. Produce GitHub release artifacts/checksums from the same tag.
14. Perform fresh-machine smoke tests using crates.io-installed binaries.
15. Publish release notes and update repository badges/status.

If any publication fails, stop. Do not bump arbitrary package versions until the failure mode and crates.io state are understood. A crate version already uploaded cannot be overwritten.

## Fresh-install verification

From clean environments, verify at least:

- Linux x86-64: install daemon/client, run loopback, install systemd service.
- Linux ARM64: install or use release artifact, run daemon/client, confirm resource suitability.
- macOS Intel: install daemon/client, native sample, launchd lifecycle.
- macOS Apple Silicon: same native/launchd checks.

For each:

- `greggd run` reaches ready state.
- `/v1/status` validates.
- `gregg add`, `list`, `refresh`, `remove`, and `edit` behave as documented.
- The TUI renders the host and restores the terminal.
- Service configuration persists through restart.

Record exact OS/Rust versions and commands.

## Post-release checks

Immediately after release:

- Verify docs.rs builds `gregg-protocol` successfully.
- Verify crates.io metadata/readmes render correctly.
- Verify `cargo install greggd --version 1.0.0` and `cargo install gregg --version 1.0.0` from a clean Cargo home.
- Verify no path/git dependency leaked into normalized manifests.
- Open a version-1 tracking issue for confirmed post-release defects rather than silently changing contracts.
- Preserve canonical version-1 protocol/config fixtures for future compatibility tests.

## Acceptance criteria

Phase 10 and Gregg version 1 are complete when:

1. A project license and dependency-license policy are finalized and represented accurately in every package.
2. Crates.io ownership and protected publication credentials are established for all three crate names.
3. Public protocol/library API and binary CLI/config contracts receive final semver review.
4. README, per-crate docs, changelog, security policy, and contribution guidance reflect verified implementation.
5. All package archives are inspected, unpacked, built, tested, and locally installed from clean directories.
6. The final release commit passes the entire phase-9 matrix with no undocumented release blocker.
7. Annotated tag `v1.0.0` points to the exact published source.
8. `gregg-protocol`, `greggd`, and `gregg` version `1.0.0` are published in dependency order and resolvable from crates.io.
9. docs.rs succeeds for the protocol crate and crates.io installation succeeds for both binaries.
10. Fresh-install smoke tests pass on Linux x86-64, Linux ARM64, macOS Intel, and macOS Apple Silicon at the documented evidence level.
11. GitHub release artifacts, if provided, have checksums and correspond to the same tag.
12. Known limitations—including local-network security posture and unavailable macOS I/O wait—are clearly documented.
13. No release tag is moved and no published version is treated as replaceable.
14. The version-1 compatibility fixtures and release evidence are committed for future regression testing.
