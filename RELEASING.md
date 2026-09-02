# Releasing gregg

Releases are performed manually by a maintainer. GitHub Actions verifies
source changes; the release workflow builds prebuilt binaries but never
publishes crates, bumps versions, creates tags, or publishes a GitHub Release.

## Prerequisites

Before starting, ensure you have:

- Authenticated crates.io access through Cargo's normal credential mechanism
  (`cargo login`).
- Permission to push tags to `eggstack/gregg`.
- Permission to create a GitHub Release.
- A clean local checkout of the intended release commit.
- The current `main` fetched from the remote.
- All workspace crate versions and inter-crate dependency versions updated
  consistently.
- Changelog and README support tables updated for the release.

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

The root workspace version is authoritative. Confirm it appears once in
`[workspace.package]` and that every member manifest inherits it via
`version.workspace = true`:

```bash
grep -E '^version' Cargo.toml
grep -E '^version\.workspace' crates/gregg-protocol/Cargo.toml
grep -E '^version\.workspace' crates/greggd/Cargo.toml
grep -E '^version\.workspace' crates/gregg/Cargo.toml
```

All three member manifests must contain exactly `version.workspace = true`,
and the root `Cargo.toml` must contain `version = "$VERSION"` inside
`[workspace.package]`.

Then confirm inter-crate dependency versions match `$VERSION`:

```bash
grep 'gregg-protocol' crates/greggd/Cargo.toml
grep 'gregg-protocol' crates/gregg/Cargo.toml
```

Both `greggd` and `gregg` must reference `gregg-protocol = "=$VERSION"` (or
the exact same version) in their normal dependency and dev-dependency
declarations.

Verify the changelog contains the release version and date. Confirm crate
descriptions and supported-platform documentation are current.

Verify `Cargo.lock` is committed and matches the workspace:

```bash
git diff --name-only Cargo.lock
```

An empty diff means the lock file is current.

## 3. Run local release preflight

```bash
./scripts/check-local.sh --release
```

This runs the short default fmt-and-workspace-test loop, then adds Clippy,
documentation, package-content checks, and the installed-binary loopback smoke.
The smoke
installs the current checkout with `cargo install --path crates/greggd
--locked` and uses `scripts/verify-installed-daemon.sh` to start the
installed binary on a loopback port, poll `/v2/healthz` and `/v2/status`,
and shut the daemon down cleanly.

## 4. Dry-run and publish gregg-protocol

The local pre-publication release preflight (`./scripts/check-local.sh
--release`) already runs the protocol dry-run with `--locked`. Re-run it
manually here to record the result:

```bash
cargo publish -p gregg-protocol --dry-run --locked
cargo publish -p gregg-protocol --locked
```

Wait for crates.io availability before continuing:

```bash
cargo search gregg-protocol --limit 1
```

Confirm the exact `$VERSION` appears.

## 5. Dry-run dependent crates (after protocol publication)

Dependent-crate dry-runs must wait for the new `gregg-protocol` version to
be visible on crates.io. The local release preflight does not run them
because the registry has not yet indexed the new version. Run them
manually here:

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

Verify the protocol crate resolves from crates.io in a clean consumer:

```toml
[dependencies]
gregg-protocol = "=X.Y.Z"
```

Run `cargo check` against it without a path override.

On a native Unix host, run a short foreground daemon smoke with a temporary
config and loopback binding, then query `/v2/healthz` and `/v2/status` and
terminate cleanly. `/v2/status` is the universal cross-platform status
endpoint; Linux and macOS may additionally verify `/v1/status` for
compatibility, but Windows intentionally returns 503 for `/v1/status` (no
truthful v1 snapshot exists) and is not a release failure when v2 is
ready:

```bash
greggd run --config /tmp/test-greggd.toml &
DAEMON_PID=$!
sleep 1
curl -s http://127.0.0.1:11310/v2/healthz
curl -s http://127.0.0.1:11310/v2/status | head -c 200
kill "$DAEMON_PID"
wait "$DAEMON_PID" 2>/dev/null || true
```

On native Windows x86-64, the equivalent foreground smoke uses PowerShell
and a temporary config. Use `try/finally` so the process is always
stopped:

```powershell
$tempDir = Join-Path $env:TEMP "greggd-smoke-$([guid]::NewGuid())"
$configPath = Join-Path $tempDir "greggd.toml"
$port = 11399
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
@"
name = "smoke-test"
host = "127.0.0.1"
port = $port
sample_interval_ms = 250
stale_after_ms = 10000
"@ | Set-Content -LiteralPath $configPath

$process = Start-Process -FilePath greggd.exe `
    -ArgumentList @('--config', "`"$configPath`"", 'run') `
    -PassThru -WindowStyle Hidden
try {
    $ready = $false
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) { throw "greggd exited during startup" }
        try {
            $health = Invoke-RestMethod -Uri "http://127.0.0.1:$port/v2/healthz" -TimeoutSec 2
            if ($health.state -eq 'ready') { $ready = $true; break }
        } catch { }
        Start-Sleep -Milliseconds 200
    }
    if (-not $ready) { throw "greggd did not become ready within 15 seconds" }
    $status = Invoke-RestMethod -Uri "http://127.0.0.1:$port/v2/status" -TimeoutSec 2
    if ($status.schema_version -ne 2) { throw "unexpected v2 schema version" }
} finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit(5000)
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
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

## 9. Release workflow builds binaries into a draft GitHub Release

Pushing the annotated tag triggers `.github/workflows/release-binaries.yml`.
That release-only workflow (triggered only by `v*` tags or manual dispatch)
runs a mandatory preflight (workspace/tag version equality, clean checkout,
tag points at HEAD, and crates.io visibility for `gregg`/`greggd`), then
builds both `gregg` and `greggd` for the five required targets, verifies each
executable's `version`/`--help` and a loopback `greggd` health smoke, hashes
the staged binaries into `<asset>.sha256`, and assembles a draft release.

Linux GNU targets use a conservative glibc 2.17 floor via `cargo-zigbuild`
+ Zig targeting `x86_64-unknown-linux-gnu.2.17` /
`aarch64-unknown-linux-gnu.2.17`; the public asset suffix remains the
ordinary Rust target (e.g., `gregg-x86_64-unknown-linux-gnu`). The workflow
creates a new draft `Gregg X.Y.Z` when no release exists for the tag,
updates an existing draft idempotently with `--clobber`, and **never**
auto-publishes or silently replaces a published release. It uploads:

```text
gregg-x86_64-unknown-linux-gnu + .sha256
greggd-x86_64-unknown-linux-gnu + .sha256
gregg-aarch64-unknown-linux-gnu + .sha256
greggd-aarch64-unknown-linux-gnu + .sha256
gregg-x86_64-apple-darwin + .sha256
greggd-x86_64-apple-darwin + .sha256
gregg-aarch64-apple-darwin + .sha256
greggd-aarch64-apple-darwin + .sha256
gregg-x86_64-pc-windows-msvc.exe + .sha256
greggd-x86_64-pc-windows-msvc.exe + .sha256
install.sh
install.ps1
```

No binary is modified after hashing (the release profile already uses LTO,
single codegen unit, stripped symbols, and aborting panics). A rerun while
the release is still a draft is safe; a published release causes a hard
failure that requires a new patch version. The workflow never calls
`cargo publish`, `git tag`, or pushes source commits.

The update contract assumes crate version, tag, and binary assets are
synchronized: `gregg update`/`greggd update` query `max_stable_version`
from crates.io and request the exact `vX.Y.Z` asset from the GitHub Release.
The binary workflow is not considered complete if one of the required
`gregg`/`greggd` assets is missing for a supported prebuilt target; the
draft must contain all ten executables + ten checksums + both installers
before publication, otherwise installed updaters would fall back to Cargo
or fail.

## 10. Publish the GitHub Release

The draft release contains the prebuilt binaries. Review it, then publish:

### Via GitHub UI

1. Open the repository Releases page.
2. Open the draft for `vX.Y.Z`.
3. Use `Gregg $VERSION` as the title (the workflow already sets it).
4. Paste concise release notes derived from the changelog — include whether
   binaries are available for the five targets, the glibc floor, and that
   macOS binaries are unsigned.
5. Mark prerelease only when intentionally publishing a prerelease.
6. Publish the release.

### Via GitHub CLI

```bash
gh release edit "$TAG" --draft=false
# or, if creating without the workflow:
gh release create "$TAG" \
  --title "Gregg $VERSION" \
  --notes-file /path/to/release-notes.md \
  --verify-tag
```

Release notes must include at minimum:

- concise summary;
- user-visible changes;
- supported-platform changes (including the initial prebuilt matrix);
- important fixes;
- known limitations or compatibility notes (macOS unsigned quarantine, ARMv7 source-only);
- binary installation commands (`install.sh`/`install.ps1` one-liners plus direct asset download);
- crates.io installation commands as fallback.

Do not include internal evidence IDs, CI-run metadata, or evidence artifacts.

### Installation after publication

Preferred (binary-first):

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- greggd
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo sh -s -- both
```

Windows PowerShell:

```powershell
irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
```

Fallback (Cargo):

```bash
cargo install gregg --locked
cargo install greggd --locked
```

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
- No repository automation performs crates.io publication, version bumping,
  or tag creation.
- The release workflow may create/update **only** a draft GitHub Release and
  attach binaries; final publication remains a manual maintainer click.
- Release evidence is the public crates.io records, Git tag, GitHub Release
  (including published binary assets), and normal local command output.
- Ordinary CI (`ci.yml`) never publishes or builds release binaries; only the
  tagged release workflow does.
