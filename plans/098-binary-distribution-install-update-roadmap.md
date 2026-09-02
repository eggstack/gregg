# Plan 098: binary distribution, bootstrap installation, and update roadmap

Status: planned; coordination roadmap for Plans 099-101.

Depends on: the completed manual-release simplification in Plans 036-039, the current cross-platform runtime/service work through Plan 097, and the final `croncheck` semantics from Plan 091 where Plan 100 uses cron as a supervisor.

## Objective

Add a deliberately small binary-distribution layer to Gregg so common systems can install `gregg`, `greggd`, or both without compiling Rust locally, while preserving crates.io as the source-build fallback and preserving the existing manual release philosophy.

The user-facing end state is:

```text
GitHub Release
    |
    +-- prebuilt gregg/greggd binaries for common OS/architectures
    +-- SHA-256 checksum files
    +-- install.sh
    +-- install.ps1

curl/PowerShell installer
    |
    +-- detect OS + architecture
    +-- download matching prebuilt release binary when available
    +-- verify it
    +-- install gregg, greggd, or both
    +-- for greggd, configure the native startup mechanism when allowed
    +-- if no matching asset exists, fall back to Cargo when Cargo is available

installed binary
    |
    +-- gregg update
    +-- greggd update
    +-- greggd restart
    +-- greggd startup install|instructions
```

This work is primarily for small fleets and SBCs where a local release compile is slow. Raspberry Pi and Le Potato are not separate binary targets: ordinary 64-bit Linux images use the generic AArch64 Linux artifact. A 32-bit ARMv7 artifact is allowed only after an explicit build/run validation in Plan 099; it must not be declared supported merely because cross-compilation succeeds.

## Governing design decisions

### 1. GitHub distributes binaries; crates.io remains the source fallback

The release tag remains the identity joining the workspace version, crates.io versions, and GitHub assets:

```text
workspace X.Y.Z
    == crates.io gregg X.Y.Z
    == crates.io greggd X.Y.Z
    == Git tag vX.Y.Z
    == GitHub Release vX.Y.Z binary assets
```

Plans 036-039 intentionally removed release orchestration. This roadmap supersedes only their statement that GitHub releases never contain binary attachments and Actions never creates release artifacts. It does **not** restore automated crates.io publication, version mutation, tag creation, changelog generation, release evidence bundles, or package-manager publication.

The intended release sequence becomes:

```text
local release preflight
-> maintainer publishes crates.io packages manually
-> maintainer creates/pushes annotated vX.Y.Z tag
-> tag-triggered binary workflow builds/verifies assets
-> workflow creates or updates a DRAFT GitHub Release
-> maintainer reviews and publishes the GitHub Release manually
```

The final publish click remains manual. The workflow may create the draft because attaching many target assets by hand defeats the purpose of this line of work.

### 2. Ordinary CI remains ordinary CI

Do not expand `.github/workflows/ci.yml` into a release builder. Add one release-only workflow triggered by release tags/manual dispatch. Routine pull requests must not build every distribution artifact.

No nightly release matrix, self-hosted runner, SBOM job, signing service, provenance system, or binary-size gate is required.

### 3. Use generic target triples, not board-specific builds

Initial required prebuilt targets:

| Platform | Rust target | Intended hosts |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | ordinary x86-64 Linux |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | Raspberry Pi 4/5 64-bit, Le Potato 64-bit, other AArch64 Linux SBCs |
| macOS Intel | `x86_64-apple-darwin` | Intel Macs |
| macOS ARM64 | `aarch64-apple-darwin` | Apple Silicon |
| Windows x86-64 | `x86_64-pc-windows-msvc` | supported Windows x86-64 |

Optional candidate after validation:

| Platform | Rust target | Rule |
| --- | --- | --- |
| Linux ARMv7 hard-float | `armv7-unknown-linux-gnueabihf` | publish only after Plan 099 proves build plus executable smoke under an appropriate runner/emulator |

Windows ARM64, Linux MUSL variants, FreeBSD, Android, and board-tuned builds are out of scope for this roadmap.

Linux artifacts must be intentionally portable across older SBC distributions. Do not accidentally make the minimum glibc version equal to whatever happens to ship on the current GitHub runner. Plan 099 must establish a documented glibc floor, preferably through release-only `cargo-zigbuild`/Zig targeting or an equivalently small mechanism. Do not add Zig or cross-compilation libraries to runtime dependencies.

### 4. Keep the daemon runtime independent from service managers

Plans 076-082 deliberately separated `greggd run` from systemd/launchd. Preserve that boundary.

Allowed:

```text
CLI/deployment boundary:
  greggd startup ...
  greggd restart
  greggd update
  packaging/install.sh
  packaging/install.ps1
```

Not allowed:

```text
collector -> systemctl
sampler   -> launchctl
run.rs    -> service-manager ownership
HTTP API  -> update/restart endpoint
```

Plan 100 may execute native service-manager commands from an explicit CLI lifecycle command. That is not permission to restore service-manager behavior to normal foreground runtime or config mutation.

### 5. One asset naming contract must be shared by release, installer, and updater

Use stable asset names within each tagged release; the GitHub release/tag already carries the version. Preferred shape:

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
<asset>.sha256
install.sh
install.ps1
```

Do not independently encode target names in three unrelated places without tests or one clearly shared table/contract. If the implementation uses a small script/helper to stage names, keep it release-only and readable.

### 6. Bootstrap installation must be noninteractive-friendly

The Unix installer supports explicit component selection:

```text
install.sh gregg
install.sh greggd
install.sh both
```

The README copy/paste path must therefore work through a pipe, for example:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
```

The exact documented hardening flags may be tightened during implementation. Do not require a TTY menu for the normal fleet path. An optional interactive selector is acceptable only if it adds little code and never replaces explicit arguments.

On Windows, provide an equivalent PowerShell installer with explicit `Gregg`, `Greggd`, or `Both` component selection.

### 7. Privilege escalation is explicit

Install/update code must not silently invoke `sudo`, pop an elevation prompt, or hide privilege failure.

When already privileged, perform the system installation/startup steps. When privileges are insufficient, perform only safe unprivileged work and print the exact elevated command required to finish, or fail before creating an internally inconsistent daemon deployment.

In particular, do not silently fall back from a detected systemd host to a cron-managed duplicate simply because systemd registration needs root. An operator can explicitly choose `--method cron` when that is desired.

### 8. Update is binary-first and source-build second

Both binaries gain `update`.

Required policy:

```text
local compile-time version
-> query latest stable version of own crate on crates.io
-> if current: exit success
-> map current OS/arch to release target
-> try exact GitHub vX.Y.Z asset
-> verify candidate and version
-> replace executable safely
-> greggd: restart the managed/running daemon when appropriate
-> if exact asset is absent: compile/install exact crates.io version with Cargo if available
-> otherwise: actionable unsupported-target error
```

Do not use `main` branch as the update source of truth. Do not update to a GitHub release version newer than crates.io. The crates.io stable crate version requested by the user remains the update authority.

## Roadmap phases

### Plan 099 — release binary matrix and bootstrap installers

Deliver:

- release-only GitHub Actions workflow;
- portable target builds for both binaries;
- deterministic asset names and SHA-256 files;
- draft-release asset assembly;
- Unix and Windows binary-first installers;
- Cargo fallback for missing target assets;
- target/install documentation and release-runbook changes necessary for the new artifact channel.

Plan 099 deliberately does not own the complete service-manager lifecycle. Before Plan 100 lands, daemon installation may retain/print the existing packaging path. Plan 100 becomes authoritative for automatic startup registration.

### Plan 100 — greggd startup installation and restart

Deliver:

- `greggd startup install`;
- `greggd startup instructions`;
- explicit/detected `systemd`, `launchd`, and `cron` behavior on Unix;
- preservation of Windows SCM semantics through the Windows installer/SCM code;
- Unix `greggd restart` at the CLI/deployment boundary;
- installer integration so a privileged daemon bootstrap can configure startup automatically and an unprivileged bootstrap prints exact completion instructions;
- systemd/cron/launchd documentation.

The cron path must use the hardened `croncheck` contract from Plan 091. Plan 100 cannot close against an older ambiguous TCP-only watchdog implementation.

### Plan 101 — binary-first CLI update and release integration

Deliver:

- `gregg update`;
- `greggd update`;
- crates.io stable-version check;
- exact-tag GitHub asset download;
- checksum and candidate-version verification;
- safe Unix and Windows executable replacement;
- Cargo exact-version fallback;
- daemon restart integration using Plan 100 rather than a second lifecycle implementation;
- final README/crate README/RELEASING/plan-index reconciliation.

## Dependency order

```text
                 +--> 099 release/assets/install contract --+
098 roadmap -----+                                      +----> 101 update
                 +--> 100 startup/restart --------------+
                         ^
                         |
                Plan 091 croncheck semantics
```

Plan 099 may begin independently of the remaining manual soak record in Plan 091. Plan 100's cron implementation and closure must use the final Plan 091 behavior. Plan 101 should not begin replacement/restart integration until the asset naming and daemon lifecycle contracts are stable.

## Verification philosophy

This line of work is release-facing, so it legitimately adds one release workflow. It does not change the local-first development model.

Use:

- focused unit/parser/platform mapping tests;
- `./scripts/check-local.sh` for ordinary source changes;
- `./scripts/check-local.sh --release` for release-facing closure;
- native release-job execution for targets GitHub can run natively;
- one QEMU/cross smoke only if ARMv7 is actually published;
- shellcheck/PowerShell syntax checks for installers;
- direct installer/update smoke on the available Ubuntu host;
- existing Windows CI/release runner for Windows behavior that cannot be exercised locally.

Do not add generalized artifact evidence storage. The published/draft GitHub release assets and normal workflow logs are sufficient release evidence.

## Roadmap acceptance criteria

- [ ] Plans 099-101 are implemented in dependency order or with explicitly safe parallelism.
- [ ] Common Linux x86-64/AArch64, macOS Intel/ARM64, and Windows x86-64 users can install without compiling Rust.
- [ ] A 64-bit Raspberry Pi/Le Potato uses the ordinary AArch64 Linux asset rather than a board-specific build.
- [ ] ARMv7 is advertised only if it passes Plan 099's executable validation.
- [ ] Linux release portability uses a documented glibc floor rather than accidental runner glibc.
- [ ] The normal CI workflow is not converted into a release matrix.
- [ ] crates.io publication remains manual.
- [ ] tag creation remains manual.
- [ ] the binary workflow may create/update only a draft GitHub release; final publication remains manual.
- [ ] `greggd run`, collectors, sampler, and HTTP API remain service-manager/update unaware.
- [ ] installers never silently escalate privileges.
- [ ] `greggd` startup registration is idempotent and does not create competing supervisors by default.
- [ ] `gregg update` and `greggd update` use crates.io stable version as authority, exact GitHub tag assets as the binary path, and Cargo only as fallback.
- [ ] source-build-only/unsupported targets receive a useful fallback or diagnostic rather than a misleading success.
- [ ] documentation shows a copy/paste installation path suitable for SSH fleet deployment.
- [ ] no apt, rpm, Homebrew, MSI, winget, Chocolatey, container image, signing service, SBOM/provenance framework, or auto-publication scope is added.

## Closure record

This roadmap is complete only when Plans 099-101 are each truthfully closed. Record their implementation SHAs and the first release tag that successfully exercises the complete binary asset + installer + updater contract. Do not rewrite Plans 036-039; update the active plan index to state that Plan 098 narrowly supersedes their former no-binary-asset/no-release-workflow policy while preserving manual publication.

Correction note (Plan 102): Plans 099-101 remain historically implemented and
CI-verified at their recorded SHAs/runs, but source review found release-
readiness defects in update activation, restart safety, process timeouts, and
installation documentation. The roadmap is not treated as release-ready until
Plan 102 closes; the first binary-bearing release remains the live proof of
installer and updater consumption.
