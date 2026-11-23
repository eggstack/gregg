# Installation

The recommended installer downloads prebuilt release binaries and verifies their
SHA-256 checksums. It falls back to Cargo on platforms without a release asset.
Prebuilt installs do not require Rust.

## Linux and macOS

Install the client in your user account:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg
```

Install the daemon as a system service:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | sudo bash -s -- greggd
```

The client installs to `$HOME/.local/bin`; the privileged daemon install uses
`/usr/local/bin` and registers startup with the detected service manager. The
daemon runs as a dedicated service account, not as root: `greggd` on Linux and a
hidden `_greggd` user on macOS (created by the install if it does not exist).
See [Service account](daemon.md#service-account). The user installer can add
`$HOME/.local/bin` to supported shell profiles. If it is not on the current
shell's `PATH`, run the `export` command printed by the installer. Use
`--no-shell-profile` to skip profile changes.

To pin an install to a published version, replace `1.2.3` below with that
release's version number:

```bash
VERSION=1.2.3
curl -fsSL "https://github.com/eggstack/gregg/releases/download/v${VERSION}/install.sh" \
  | bash -s -- --version "$VERSION" gregg
```

The installer requires Bash. Do not pipe it to `sh`.

## Windows

Use PowerShell. Run as Administrator to install `greggd` as a system service;
without elevation, it installs under your user profile and does not register the
service.

```powershell
$installer = Join-Path $env:TEMP 'gregg-install.ps1'
Invoke-WebRequest 'https://github.com/eggstack/gregg/releases/latest/download/install.ps1' -OutFile $installer
& $installer -Component Gregg
```

For the daemon, use `-Component Greggd`. To install both, use `-Component Both`.
Add `-Version` followed by a published release number to pin the install. The
client and daemon can be installed at the same scope without replacing one
another.

## Cargo

Use Cargo on source-only platforms or when you prefer a source install. Rust
1.89 or newer is required.

```bash
cargo install gregg --locked
cargo install greggd --locked
```

The bootstrap installers use the same locked Cargo fallback when no matching
binary is available and Cargo is installed.

## Direct downloads

Release assets and matching `.sha256` files are listed on the
[GitHub Releases page](https://github.com/eggstack/gregg/releases). Verify the
checksum before installing a downloaded binary. Linux assets target glibc 2.17;
macOS binaries are unsigned. See the supported-target table in the
[README](../README.md). Linux ARMv7 and Windows ARM64 use Cargo when available.

## Updating and uninstalling

Rerun the bootstrap installer at the same installation scope to replace its
component. For an installed binary, use `gregg update` or `greggd update` to
update that exact executable.

Preview removal before making changes:

```bash
gregg uninstall --dry-run
greggd uninstall --dry-run
```

Uninstall each component separately. Configuration is preserved by default;
`--purge` also removes that component's configuration. On macOS,
`greggd uninstall --purge` also removes `/var/log/greggd.log`. The client and
daemon may share an install directory, but uninstall removes only the selected
component and its owned startup entry. Neither command invokes `sudo` itself.
See [Client](client.md) and [Daemon](daemon.md) for platform-specific behavior.
