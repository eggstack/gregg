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

### 9. Create GitHub Release

Via GitHub CLI:

```bash
gh release create "$TAG" \
  --title "Gregg $VERSION" \
  --notes-file /path/to/release-notes.md \
  --verify-tag
```

## Publication order

**Mandatory:** `gregg-protocol` → `greggd` → `gregg`

## Policy

- A published crates.io version is immutable
- No repository automation performs publication, tagging, or GitHub Release creation
- CI never publishes
- See `RELEASING.md` for the full operator runbook
