# Releasing gregg

Releases are performed manually by a maintainer. GitHub Actions verifies
source changes and never publishes crates, pushes tags, or creates GitHub
Releases.

## Prerequisites

Before starting, ensure you have:

- Authenticated crates.io access through Cargo's normal credential mechanism
  (`cargo login`).
- Permission to push tags to `eggstack/gregg`.
- Permission to create a GitHub Release.
- A clean local checkout of the intended release commit.
- The current `main` fetched from the remote.

Set the release version and tag before preflight:

```bash
VERSION="X.Y.Z"
TAG="v${VERSION}"
```

## 1. Sync and verify clean tree

```bash
git fetch origin
git switch main
git pull --ff-only origin main
git status --short
```

A nonempty status blocks release. Either commit the changes first or abort.

## 2. Verify version consistency

Confirm workspace and crate versions match the intended release:

```bash
grep '^version' Cargo.toml | head -1
grep '^version' crates/gregg-protocol/Cargo.toml | head -1
grep '^version' crates/greggd/Cargo.toml | head -1
grep '^version' crates/gregg/Cargo.toml | head -1
```

All must show `$VERSION`. Then confirm inter-crate dependency versions:

```bash
grep 'gregg-protocol' crates/greggd/Cargo.toml
grep 'gregg-protocol' crates/gregg/Cargo.toml
```

Both must reference the intended `gregg-protocol` version. Verify the
changelog contains the release version and date.

## 3. Run full local validation

```bash
./scripts/check-local.sh --full
```

This runs fmt, clippy, tests, docs, cargo-deny, shellcheck, python tests,
package content checks, and the installed-binary loopback smoke.

## 4. Publish gregg-protocol

```bash
cargo publish -p gregg-protocol --locked
```

Wait for crates.io availability before continuing:

```bash
cargo search gregg-protocol --limit 1
```

Confirm the exact `$VERSION` appears. Optionally, create a clean temporary
consumer project that resolves `gregg-protocol = "=$VERSION"` from crates.io
and run `cargo check` against it.

## 5. Dry-run dependent crates

After protocol availability is confirmed, re-run dependent dry-runs:

```bash
cargo publish -p greggd --dry-run --locked
cargo publish -p gregg --dry-run --locked
```

This is the authoritative dependent-package check because it resolves the
published protocol version from crates.io.

## 6. Publish daemon and client

Publish sequentially, not concurrently:

```bash
cargo publish -p greggd --locked
cargo publish -p gregg --locked
```

## 7. Install and smoke-test

```bash
cargo install greggd --version "=$VERSION" --locked
cargo install gregg --version "=$VERSION" --locked

greggd --version
greggd --help
gregg --version
gregg --help
```

On a native host, run a short foreground daemon smoke with a temporary config
and loopback binding, then query `/healthz` and `/v1/status` and terminate
cleanly:

```bash
greggd run --config /tmp/test-greggd.toml &
DAEMON_PID=$!
sleep 1
curl -s http://127.0.0.1:11310/healthz
curl -s http://127.0.0.1:11310/v1/status | head -c 200
kill "$DAEMON_PID"
wait "$DAEMON_PID" 2>/dev/null || true
```

## 8. Create and push annotated tag

After all intended crates are visible on crates.io and smokes pass:

```bash
git status --short
git rev-parse HEAD
git tag -a "$TAG" -m "gregg $VERSION"
git push origin "$TAG"
```

Confirm the tag points to the exact release commit, the commit contains the
matching versions and changelog, and no additional source changes occurred
after publication.

## 9. Create GitHub Release

### Via GitHub UI

1. Open the repository Releases page.
2. Draft a new release.
3. Select the existing annotated tag.
4. Use `Gregg $VERSION` as the title.
5. Paste concise release notes derived from the changelog.
6. Mark prerelease only when intentionally publishing a prerelease.
7. Publish the release.

### Via GitHub CLI

```bash
gh release create "$TAG" \
  --title "Gregg $VERSION" \
  --notes-file /path/to/release-notes.md \
  --verify-tag
```

The CLI is an operator command, not a checked-in script. No binary artifacts
are required; crates.io remains the installation channel.

Release notes must include at minimum:

- concise summary;
- user-visible changes;
- supported-platform changes;
- important fixes;
- known limitations or compatibility notes;
- crates.io installation commands.

Do not include internal evidence IDs or CI-run metadata.

## Partial failure handling

### Before any publication

Fix the source, rerun checks, and release the same planned version if no
crate with that version was uploaded.

### Protocol published, dependent crate fails

- If no source correction is needed, fix the local/environment problem and
  retry the same dependent crate version.
- If source or packaged content must change, bump the workspace to a new
  version. Do not attempt to replace the published protocol version. Publish
  the corrected protocol under the new version, then the dependents.

### Protocol and daemon published, client fails

- If the client package can be retried unchanged, retry it.
- If source must change, use a new version for the corrected workspace release.
- Do not create the final Git tag/GitHub Release for an incomplete intended
  release unless intentionally documenting a partial release.

### Cargo reports a timeout after upload

Do not immediately republish. First inspect crates.io for the exact
package/version. Cargo may have uploaded successfully before the index became
visible.

### Published package is incorrect

- Yank only when appropriate and consciously chosen.
- Prepare a corrected version.
- Do not delete, overwrite, or reuse the immutable version.
- Document the correction in the changelog/release notes.

## Policy

- Publication order is mandatory: `gregg-protocol`, `greggd`, `gregg`.
- A published crates.io version is immutable.
- No repository automation performs publication, tagging, or GitHub Release
  creation.
- Release evidence is the public crates.io records, Git tag, GitHub Release,
  and normal local command output.
- CI never publishes.
