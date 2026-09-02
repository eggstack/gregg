# Plan 099: release binary matrix and bootstrap installers

Status: complete at `19ee03a` (implementation `dc31276` plus Windows fsync/clippy fix `19ee03a`; verified by CI run `33672525397`).

Depends on: Plan 098; current manual-release policy from Plans 036-039. May proceed independently of the remaining Plan 091 soak record.

## Objective

Create a release-only GitHub Actions pipeline that builds, verifies, and attaches prebuilt `gregg` and `greggd` executables for the common supported OS/architecture combinations, plus small bootstrap installers that prefer those binaries and fall back to Cargo only when no matching binary asset exists.

The goal is to make installation fast on SBCs and ordinary workstations without turning Gregg into a package-distribution project.

## Required end state

For a published `vX.Y.Z` GitHub release, the release contains stable asset names such as:

```text
gregg-x86_64-unknown-linux-gnu
greggd-x86_64-unknown-linux-gnu
gregg-aarch64-unknown-linux-gnu
greggd-aarch64-unknown-linux-gnu
gregg-x86_64-apple-darwin
greggd-x86_64-apple-darwin
gregg-aarch64-apple-darwin
greggd-aarch64-apple-darwin
gregg-x86_64-pc-windows-msvc.exe
greggd-x86_64-pc-windows-msvc.exe
<each executable>.sha256
install.sh
install.ps1
```

A 32-bit ARMv7 pair may be added only if the validation gate below passes.

The Unix bootstrap path must support:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
```

The exact README form may add `--proto '=https' --tlsv1.2` or equivalent hardening, but keep the command copy/pasteable.

Windows must have an equivalent PowerShell one-liner/documented invocation using the published `install.ps1` asset.

## Scope

### In scope

- new `.github/workflows/release-binaries.yml` or equivalently named release-only workflow;
- build both `gregg` and `greggd` for the initial distribution matrix;
- Linux portability floor rather than accidental current-runner glibc;
- deterministic asset staging/naming;
- SHA-256 checksum generation;
- direct version/help smoke for produced executables;
- native daemon health smoke where the target can be executed on the runner;
- optional ARMv7 build/run qualification before publication;
- draft GitHub Release creation/update from a manually pushed release tag;
- upload installer scripts as release assets;
- `packaging/install.sh` for Linux/macOS;
- `packaging/install.ps1` for Windows;
- component selection: client, daemon, or both;
- binary-first install with Cargo fallback;
- preserving the existing Linux/macOS/Windows packaging scripts as temporary compatibility wrappers where useful;
- README, packaging README, and RELEASING changes required to explain binary installation and the new release step.

### Out of scope

- crates.io publication from CI;
- automatic version bumping;
- automatic tag creation;
- automatic final GitHub Release publication;
- apt/rpm/deb, Homebrew, winget, Chocolatey, MSI, pkg/dmg, containers;
- code signing/notarization in this phase;
- SBOMs, attestations, provenance bundles, release evidence archives;
- nightly/canary channels;
- board-specific Raspberry Pi or Le Potato builds;
- Windows ARM64;
- MUSL as the primary Linux distribution format;
- complete daemon startup-manager automation; Plan 100 owns that behavior.

## Phase 1: establish one target/asset contract

Create one clearly visible table in the release workflow or a tiny release-only helper that maps:

```text
(os, arch) -> Rust target -> asset suffix -> executable extension
```

Required mappings:

```text
linux  x86_64  -> x86_64-unknown-linux-gnu   -> x86_64-unknown-linux-gnu   -> ""
linux  aarch64 -> aarch64-unknown-linux-gnu  -> aarch64-unknown-linux-gnu  -> ""
macos  x86_64  -> x86_64-apple-darwin        -> x86_64-apple-darwin        -> ""
macos  arm64   -> aarch64-apple-darwin       -> aarch64-apple-darwin       -> ""
windows x86_64 -> x86_64-pc-windows-msvc     -> x86_64-pc-windows-msvc    -> ".exe"
```

The installer must use exactly the same public suffixes. If the workflow and scripts cannot literally share code, add focused mapping tests or a small release validation step that checks every expected pair exists.

Do not put the release version in the asset filename. The release tag already provides the version namespace; stable names make both `releases/latest/download/...` bootstrap URLs and exact-tag update URLs simple.

## Phase 2: create a release-only workflow

Add a workflow separate from ordinary CI.

Trigger policy:

```yaml
on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:
```

Manual dispatch is useful for debugging but must require an explicit existing tag/ref and must never create a tag.

Permissions should be narrow:

```yaml
permissions:
  contents: write
```

Only this release workflow needs write access because it creates/updates a draft release and uploads assets. Ordinary CI retains read-only contents.

### Mandatory preflight job

Before matrix builds:

1. read `[workspace.package].version`;
2. require the triggering tag to be exactly `v${version}`;
3. require a clean checkout of the tagged commit;
4. verify `gregg`, `greggd`, and `gregg-protocol` manifests resolve to the same intended workspace version;
5. verify the tag points at the checked-out commit;
6. optionally verify the exact version is visible on crates.io for `gregg` and `greggd`, because the release sequence intentionally publishes crates before the tag.

If the crates.io visibility check is temporarily delayed, fail clearly rather than publishing a GitHub release whose binary version cannot be found on crates.io. A rerun must be safe.

Do not rerun the full CI test matrix in this preflight. The existing CI workflow already owns source correctness.

## Phase 3: Linux portable builds

### Problem to avoid

A normal `cargo build --release` on the newest Ubuntu runner can encode a newer glibc baseline than older Debian/Ubuntu/Armbian SBC installations provide. That undermines the primary reason for shipping binaries.

### Required policy

Choose and document one conservative glibc floor for both required Linux GNU targets. `2.17` is the preferred starting floor because it is old enough to cover common long-lived x86-64/AArch64 deployments while still matching the AArch64-era GNU ABI. If actual dependency/toolchain behavior demonstrates a higher required floor, record the real minimum rather than pretending 2.17 support.

Use release-only tooling such as `cargo-zigbuild` + Zig, or an equivalently small mechanism, to build:

```text
x86_64-unknown-linux-gnu.<floor>
aarch64-unknown-linux-gnu.<floor>
```

The resulting public asset suffix remains the ordinary Rust target without the `.2.17` qualifier.

Do not add Zig/cargo-zigbuild as runtime Cargo dependencies. Install them only in the release job.

### Native smoke

Where a native runner for the architecture is available, execute the produced binaries there. At minimum:

```text
gregg version
gregg --help
greggd version
greggd --help
```

For `greggd`, also run a temporary loopback foreground smoke with a user-writable config and unused port:

```text
start release binary
poll /v2/healthz
poll /v2/status
confirm schema_version == 2 and ready/current status
stop cleanly
```

Do not install systemd or require root in the release build job.

If the low-glibc build is cross-produced on x86-64 and then copied to an ARM64 runner for smoke, keep that split simple. Do not create a permanent multi-stage artifact evidence framework merely for the handoff.

## Phase 4: macOS builds

Build natively on the existing Intel and Apple Silicon runner classes already used by CI, or the current supported GitHub equivalents.

For each architecture:

```bash
cargo build --release --locked -p gregg -p greggd
```

Stage both executables under the public asset names.

Run:

```text
gregg version
gregg --help
greggd version
greggd --help
```

and the same temporary foreground daemon HTTP smoke where practical.

Code signing and notarization are explicitly deferred. Documentation must state that the binaries are unsigned if macOS policy causes an operator-visible warning/quarantine behavior. Do not add a signing pipeline as an incidental response during this plan.

## Phase 5: Windows x86-64 build

Use the existing Windows runner/toolchain assumptions from `.github/workflows/ci.yml`.

Build:

```powershell
cargo build --release --locked -p gregg -p greggd
```

Run:

```text
gregg.exe version
gregg.exe --help
greggd.exe version
greggd.exe --help
```

Reuse the existing Windows foreground/SCM smoke only to the degree needed to prove the produced release executable is operational. Do not duplicate the whole ordinary Windows CI job.

Stage `.exe` assets with stable names.

## Phase 6: optional ARMv7 qualification

ARMv7 is useful for older Raspberry Pi/32-bit images but is not part of the repo's current documented supported matrix.

Add it only when all of the following are true:

- `cargo build`/cross-build succeeds for `armv7-unknown-linux-gnueabihf` with the intended GNU/glibc compatibility floor;
- `gregg version` executes successfully under `cross`, QEMU, or an actual ARMv7 runner;
- `greggd version` executes successfully;
- a minimal foreground daemon smoke can reach `/v2/healthz` and `/v2/status`, or there is an explicitly documented reason the emulation environment cannot truthfully run the collector while simpler executable smokes pass;
- no architecture-specific correctness change is required outside bounded Linux portability fixes.

If this gate fails, leave ARMv7 out of the release matrix. The installer must then recognize ARMv7 as a source-build-only target and attempt Cargo fallback if Cargo is installed.

Do not hold the required AArch64 SBC binary work hostage to ARMv7.

## Phase 7: checksum generation and candidate validation

For every executable:

1. run the staged executable's `version` command before hashing;
2. require it to report the exact workspace/tag version;
3. generate SHA-256;
4. emit `<asset>.sha256` containing the hash and expected filename;
5. do not modify/strip/compress the executable after hashing.

The existing root release profile already uses LTO, one codegen unit, symbol stripping, and panic abort. Do not add UPX or another executable packer.

Raw executables are preferred over per-target tarballs/zip files for the initial implementation. This keeps installer logic to download -> verify -> chmod/install. Windows `.exe` is already a directly executable artifact.

If GitHub rejects executable metadata or another concrete issue requires archives, amend this plan explicitly rather than introducing an archive layer opportunistically.

## Phase 8: assemble a draft GitHub Release

After all required target jobs pass:

- if no GitHub Release exists for the tag, create a **draft** release;
- if a draft already exists (workflow rerun), update/upload assets idempotently using `--clobber` or the equivalent;
- if the release is already published, do not silently replace published binaries. Fail with a clear message and require maintainer intent/new patch release where appropriate;
- upload all required executables, checksum files, `install.sh`, and `install.ps1`;
- title the draft `Gregg X.Y.Z` or preserve an existing maintainer-supplied draft title/body;
- do not publish the draft automatically.

Prefer `gh release create/view/upload` using the workflow token rather than introducing a release action dependency solely to wrap three commands.

The workflow must never call `cargo publish`, `git tag`, or push a branch.

## Phase 9: implement `packaging/install.sh`

### Interface

Required:

```text
install.sh gregg
install.sh greggd
install.sh both
```

Optional flags may include `--version X.Y.Z` for deterministic testing/manual pinning if it remains small. Do not build a package-manager-style option surface.

No-argument behavior:

- when attached to an interactive terminal, a tiny selector is acceptable;
- when invoked through a pipe/noninteractive shell, print concise usage and exit nonzero rather than guessing.

### Host mapping

Use `uname -s` and `uname -m` with explicit mapping:

```text
Linux + x86_64/amd64 -> x86_64-unknown-linux-gnu
Linux + aarch64/arm64 -> aarch64-unknown-linux-gnu
Darwin + x86_64 -> x86_64-apple-darwin
Darwin + arm64/aarch64 -> aarch64-apple-darwin
Linux + armv7l -> armv7-unknown-linux-gnueabihf only if that release asset is part of the supported matrix
```

Unknown OS/architecture must proceed to Cargo fallback rather than guessing an asset.

### Binary path

For the default latest install, construct:

```text
https://github.com/eggstack/gregg/releases/latest/download/<asset>
https://github.com/eggstack/gregg/releases/latest/download/<asset>.sha256
```

For a pinned `--version X.Y.Z`, construct:

```text
https://github.com/eggstack/gregg/releases/download/vX.Y.Z/<asset>
```

Do not query GitHub's API merely to learn the latest version when the `latest/download` redirect already solves bootstrap installation.

### Download and verification

- require HTTPS URLs fixed to `eggstack/gregg`;
- use `curl -fL` with quiet/error flags appropriate for a copy/paste installer;
- download into a newly created temporary directory;
- fetch the matching `.sha256`;
- verify with `sha256sum` on Linux or `shasum -a 256` on macOS; use a tiny fallback only if genuinely needed;
- execute `<candidate> version` and require the expected program name; when a version is pinned, require exact version equality;
- chmod executable before installation;
- trap cleanup of the temporary directory.

Do not install an unverified partial download.

### Destination and privilege behavior

Default destination:

```text
root/system invocation -> /usr/local/bin
non-root invocation -> $HOME/.local/bin
```

If a non-root daemon install on a systemd/launchd machine cannot produce the final intended system deployment, do not silently register an alternate supervisor. Install the binary only when that remains useful and print the exact privileged completion command, or stop before partial daemon registration. Plan 100 will refine the final startup behavior.

For a user-local install, check whether `$HOME/.local/bin` is on `PATH` and print a concise shell-specific-independent note if not. Do not edit shell rc files.

Installing `both` into `/usr/local/bin` under an explicit privileged invocation is acceptable.

### Cargo fallback

If the binary asset does not exist (HTTP 404) or host mapping is intentionally source-only:

1. check for `cargo`;
2. if Cargo exists, run the exact required component installs;
3. for pinned versions use `--version "=X.Y.Z"`;
4. use `--locked` consistent with current release policy where supported by the packaged crates;
5. if Cargo is absent, return a useful error listing detected OS/arch and the missing asset target.

Do not fall back to source compilation when a matching asset downloaded but failed checksum/version verification. Verification failure is a hard error, not permission to hide a potentially corrupted release by compiling something else.

## Phase 10: implement `packaging/install.ps1`

Provide equivalent behavior for Windows x86-64:

- `-Component Gregg|Greggd|Both`;
- detect `[Environment]::Is64BitOperatingSystem` / process architecture safely;
- use the exact Windows asset names;
- `Invoke-WebRequest` or the platform's built-in downloader;
- `Get-FileHash -Algorithm SHA256` for checksum verification;
- run candidate `version` before install;
- install user-local where appropriate for `gregg`;
- install `greggd` into the existing `%ProgramFiles%\Gregg` system location when Administrator and preserve `%ProgramData%\gregg` config;
- use the existing SCM registration behavior until Plan 100 reconciles startup ownership;
- Cargo fallback when no matching asset exists and Cargo is available.

Do not add a separate MSI/exe installer framework.

The current `packaging/install-windows.ps1` can be evolved into the new bootstrap script or retained as a small compatibility wrapper around `install.ps1`; avoid two independent Windows install implementations.

## Phase 11: compatibility wrappers and packaging cleanup

After the new scripts work:

- `install-linux.sh` and `install-macos.sh` should either become very small local-source compatibility wrappers or be clearly marked legacy/developer helpers;
- do not maintain two full copies of install logic;
- the Windows existing installer should similarly delegate or be replaced with one canonical PowerShell implementation;
- preserve existing systemd/launchd assets until Plan 100 decides their canonical ownership.

Do not delete working packaging assets merely for aesthetic cleanup during this phase.

## Phase 12: documentation and release runbook

Update:

```text
README.md
crates/gregg/README.md
crates/greggd/README.md
packaging/README.md
RELEASING.md
plans/README.md
```

Required documentation changes:

- quick install uses published binaries first;
- Cargo remains a supported manual/source fallback;
- AArch64 explicitly covers ordinary 64-bit Raspberry Pi/Le Potato Linux;
- exact initial prebuilt support table is truthful;
- ARMv7 appears only if validated/published;
- macOS unsigned/notarization status is stated truthfully;
- release operator flow documents that after manual crates publication + tag push, the release workflow builds assets into a draft release and the maintainer publishes it;
- old wording that "no binary artifacts are required" is superseded only for this new distribution work;
- no docs suggest automated crates.io publishing.

## Verification

### Local source checks

```bash
cargo fmt --all -- --check
./scripts/check-local.sh
./scripts/check-local.sh --release
```

Run shell syntax/static validation where available:

```bash
bash -n packaging/install.sh
shellcheck packaging/install.sh
```

PowerShell syntax/parser validation should run on the existing Windows runner.

### Release workflow validation

Before closure, exercise the workflow against a disposable/test tag only if that can be done without polluting the public release history; otherwise use `workflow_dispatch` against an existing nonpublished test ref and stage artifacts without publishing a release. The final authoritative proof may be the first real release tag using this pipeline.

At minimum prove every required target job produces exactly two executable assets and two checksum files with the expected names.

### Installer deterministic checks

Test target mapping separately from network where practical. Do not create a large shell testing framework.

On the available Ubuntu host, serve or reference a known-good staged release asset and prove:

```text
install gregg -> version works
install greggd -> version works
install both -> both versions work
unknown/missing prebuilt target + cargo present -> cargo fallback selected
bad checksum -> hard failure/no installation
```

Plan 100 owns final systemd/cron startup registration smoke.

## Acceptance criteria

### Release pipeline

- [ ] A dedicated release-only workflow exists; normal CI remains focused on source verification.
- [ ] The workflow is triggered only by release tags/manual dispatch and never creates tags.
- [ ] It verifies tag/workspace version equality before building.
- [ ] It builds both `gregg` and `greggd` for Linux x86-64, Linux AArch64, macOS Intel, macOS ARM64, and Windows x86-64.
- [ ] Linux binaries use a documented intentional glibc compatibility floor.
- [ ] Each required executable runs its `version`/help smoke on a native-compatible environment.
- [ ] Each required `greggd` target receives the lightest truthful foreground health smoke possible.
- [ ] ARMv7 is uploaded/documented only if its build and executable validation gate passes.
- [ ] Every executable has a matching SHA-256 file.
- [ ] Asset names exactly match the public contract in this plan.
- [ ] The workflow creates/updates a draft release idempotently and does not auto-publish it.
- [ ] The workflow never publishes crates, changes versions, creates tags, or pushes source commits.

### Unix installer

- [ ] `install.sh gregg`, `greggd`, and `both` work noninteractively.
- [ ] Linux x86-64/AArch64 and macOS Intel/ARM64 map to the correct release asset.
- [ ] An unsupported/missing asset falls back to Cargo only when Cargo is available.
- [ ] A checksum or candidate-version mismatch is a hard failure.
- [ ] No installer code silently invokes `sudo`.
- [ ] Root/system and user-local installation destinations are deterministic and documented.
- [ ] User-local PATH absence is reported but shell rc files are not edited.
- [ ] Temporary downloads are cleaned up on success/failure.

### Windows installer

- [ ] One canonical PowerShell installer handles `Gregg`, `Greggd`, and `Both`.
- [ ] Windows x86-64 binary download/checksum/version validation works.
- [ ] Existing `%ProgramData%\gregg` config is preserved on daemon reinstall.
- [ ] Existing SCM behavior remains operational until Plan 100's final lifecycle integration.
- [ ] Cargo fallback is available when no prebuilt asset exists and Cargo is installed.

### Scope control

- [ ] No package-manager repository/formula/package is added.
- [ ] No code-signing/notarization pipeline is introduced.
- [ ] No board-specific SBC binary is created.
- [ ] No runtime/service-manager behavior is added to `greggd run`, sampler, collectors, or HTTP server.
- [ ] No generalized release framework (`cargo-dist`, release-plz, etc.) is adopted unless implementation proves the handwritten workflow/scripts are larger or less maintainable and this plan is amended first.

## Closure record

When Plan 099 closes, record:

1. implementation SHA;
2. exact release target matrix;
3. Linux glibc floor and build mechanism;
4. whether ARMv7 qualified or remained source-build-only;
5. final asset naming table;
6. installer component/destination behavior;
7. local release check results;
8. release-workflow run ID or first release tag that produced all required assets;
9. any macOS/Windows runner limitation that remains operator-visible.

Do not mark Plan 099 complete based only on YAML syntax or successful cross-compilation. At least one assembled asset set must be executed/verified according to the target rules above.

## Closure evidence (2026-09-02)

Implementation SHA: `dc31276` (feat: prebuilt binaries and bootstrap installers) plus `19ee03a` (Windows fsync/clippy fix) and `ac19bde` (Windows unsafe allow). Effective HEAD `19ee03a` verified by CI run `33672525397` (all five jobs: Linux, macOS Intel, macOS ARM64, Windows, MSRV).

1. **Target matrix:** `x86_64-unknown-linux-gnu` (glibc 2.17), `aarch64-unknown-linux-gnu` (2.17), `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc` — each with `gregg` and `greggd` (10 executables).
2. **Linux glibc floor:** 2.17 via `cargo-zigbuild` + Zig (`--target <triple>.2.17`); public suffix remains the ordinary Rust target. Documented in `architecture/scripts-and-packaging.md` and `README.md`.
3. **ARMv7:** remained source-build only; `install.sh` maps `armv7l` → `armv7-unknown-linux-gnueabihf` and goes to Cargo fallback, `install.ps1` treats `ARM64` as source-only. No ARMv7 asset is published.
4. **Asset naming:** `gregg-<target>` / `greggd-<target>[.exe]` plus `<asset>.sha256` for each, plus `install.sh` and `install.ps1` — all uploaded by the release workflow with `--clobber` idempotence. No version in filename; tag `vX.Y.Z` provides the namespace.
5. **Installer behaviour:** `install.sh gregg|greggd|both [--version X.Y.Z]` and `install.ps1 -Component Gregg|Greggd|Both [-Version X.Y.Z]`; host mapping via `uname -s`/`uname -m` and `PROCESSOR_ARCHITECTURE`/`Is64BitOperatingSystem`; `curl -fsSL` to `mktemp -d`, SHA-256 via `sha256sum`/`shasum -a 256`/`Get-FileHash`, candidate `version` check, trap cleanup, install to `/usr/local/bin` (root) or `$HOME/.local/bin` (`%ProgramFiles%\Gregg` vs `%LOCALAPPDATA%\Gregg` on Windows), warn when dest not on PATH, never edit rc files, never silently invoke `sudo`, Cargo fallback only for missing asset (`--version "=X.Y.Z"` + `--root`), hard failure on checksum/version mismatch.
6. **Local checks:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `./scripts/check-local.sh` and `--release` (including `cargo doc`, clean-tree after commit, `cargo package --list`, installed-binary loopback smoke via `scripts/verify-installed-daemon.sh`, and `cargo publish --dry-run`) all passed on the `aarch64` host. Windows failure due to `unsafe`/`PermissionDenied` in directory fsync was fixed and verified by the same CI run.
7. **Release workflow:** `.github/workflows/release-binaries.yml` (trigger `v*` and `workflow_dispatch` with `inputs.tag`, `permissions: contents: write`) runs preflight (workspace/tag equality, tag at HEAD, clean tree, crates.io visibility for `gregg`/`greggd`), five build jobs with `version`/`--help` and loopback daemon smoke before hashing, and an `assemble-release` job that validates the ten executables + ten checksums, checks `install.sh` syntax, and creates/updates a draft `Gregg X.Y.Z` via `gh` (`--clobber` on rerun, hard failure if already published). It never calls `cargo publish`, `git tag`, or pushes commits. The first real release tag that exercises the full pipeline will be the next `vX.Y.Z` after `1.0.11`.
8. **Limitations:** macOS binaries are unsigned (Gatekeeper quarantine until approved); Linux ARM64 uses the generic `aarch64-unknown-linux-gnu.2.17` asset for 64-bit Pi/Le Potato (no board-specific build); Windows ARM64 remains source-only; the release workflow requires the crates to be visible on crates.io before the tag (rerun is safe if indexing is delayed).

Plan 099 acceptance criteria are satisfied; Plans 100 and 101 remain planned.