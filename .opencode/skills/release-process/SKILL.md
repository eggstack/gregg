---
name: release-process
description: Manual release procedure for gregg crates to crates.io
---

## What I do

Guide maintainers through the manual release process for publishing gregg crates.

## When to use me

Use this when preparing a new release of gregg to crates.io and GitHub.

## Prerequisites

- Authenticated crates.io access (`cargo login`)
- Permission to push tags to `eggstack/gregg`
- Permission to create a GitHub Release
- Clean local checkout of the intended release commit
- All workspace crate versions updated consistently

## Release steps

### 1. Sync and verify clean tree

```bash
git fetch origin
git switch main
git pull --ff-only origin main
git status --short
```

### 2. Verify version consistency

The root workspace version is authoritative. All member manifests must contain `version.workspace = true`. Inter-crate dependency versions must match workspace version exactly.

### 3. Run local release preflight

```bash
./scripts/check-local.sh --release
```

### 4. Dry-run and publish gregg-protocol

```bash
cargo publish -p gregg-protocol --dry-run --locked
cargo publish -p gregg-protocol --locked
```

Wait for crates.io availability before continuing.

### 5. Dry-run dependent crates

```bash
cargo publish -p greggd --dry-run --locked
cargo publish -p gregg --dry-run --locked
```

### 6. Publish daemon and client

```bash
cargo publish -p greggd --locked
cargo publish -p gregg --locked
```

### 7. Install and smoke-test

```bash
cargo install greggd --version "=$VERSION" --locked
cargo install gregg --version "=$VERSION" --locked
```

### 8. Create and push annotated tag

```bash
git tag -a "$TAG" -m "gregg $VERSION"
git push origin "$TAG"
```

The tag triggers `.github/workflows/release-binaries.yml` which verifies
workspace/tag version equality and crates.io visibility, builds the five
required targets (Linux x86_64/aarch64 with glibc 2.17 via cargo-zigbuild,
macOS Intel/ARM64, Windows x86_64), runs `version`/`--help` and a loopback
`greggd` smoke before hashing, and assembles a **draft** GitHub Release with
`<program>-<target>[.exe]` assets plus `<asset>.sha256`, `install.sh`, and
`install.ps1`. The workflow creates a draft or `--clobber`-updates an existing
draft and hard-fails if the release is already published; it never calls
`cargo publish`, `git tag`, or pushes commits, and never auto-publishes.

### 9. Publish the GitHub Release (manual)

The draft contains the prebuilt binaries. Review it, then publish:

```bash
gh release edit "$TAG" --draft=false
# or: GitHub UI → Releases → draft → Publish
```

Via GitHub CLI initial creation (if not using the workflow):

```bash
gh release create "$TAG" \
  --title "Gregg $VERSION" \
  --notes-file /path/to/release-notes.md \
  --verify-tag
```

Binary installation after the first binary-bearing release is published:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
```

Until a binary-bearing release exists, Cargo is the current working install
path. Do not present `latest/download` or a source-only release tag as an
available installer asset.

`install.sh`/`install.ps1` are binary-first: they map `uname -s`/`uname -m`
(or Windows arch) to the contract target, download
`https://github.com/eggstack/gregg/releases/latest/download/<asset>` (or
`.../download/vX.Y.Z/<asset>` for pinned), verify SHA-256 and candidate
`version`, trap cleanup, install to `/usr/local/bin` (root) or
`$HOME/.local/bin` (`%ProgramFiles%\Gregg` vs `%LOCALAPPDATA%\Gregg`), and
only fall back to `cargo install --locked` (with `="X.Y.Z"` when pinned) for
`armv7l`/unknown hosts. A checksum/version mismatch is a hard error with no
Cargo fallback; installers never silently invoke `sudo`.

## Publication order

**Mandatory:** `gregg-protocol` → `greggd` → `gregg`

## Policy

- A published crates.io version is immutable
- No automation publishes crates, bumps versions, or creates tags
- The release workflow may create/update **only** a draft GitHub Release from
  prebuilt binaries; final publication remains manual
- Ordinary CI (`ci.yml`) never publishes or builds release binaries
- See `RELEASING.md` for the full operator runbook
