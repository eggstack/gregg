# Gregg packaging

This directory contains the bootstrap installers and legacy helpers for local
builds. For normal installs, use the bootstrap installers or the commands in
the [installation guide](../docs/installation.md).

## Files

| Path | Purpose |
| --- | --- |
| `install.sh` | Linux and macOS bootstrap installer |
| `install.ps1` | Windows bootstrap installer |
| `systemd/greggd.service` | Linux systemd unit template |
| `launchd/com.eggstack.greggd.plist` | macOS LaunchDaemon template |
| `install-linux.sh` | Legacy helper for a locally built daemon |
| `install-macos.sh` | Legacy helper for a locally built daemon |
| `install-windows.ps1` | Windows service helper for a local binary |
| `uninstall-windows.ps1` | Compatibility wrapper for `greggd uninstall` |

The service templates are also embedded in the daemon binary so that startup
registration works after a Cargo install without a source checkout.

## Bootstrap installers

Linux or macOS:

```sh
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | sudo bash -s -- greggd
```

Windows PowerShell, run as Administrator to register the daemon service:

```powershell
$installer = Join-Path $env:TEMP 'gregg-install.ps1'
Invoke-WebRequest 'https://github.com/eggstack/gregg/releases/latest/download/install.ps1' -OutFile $installer
& $installer -Component Greggd
```

Use `-Component Gregg` for the client or `-Component Both` for both programs.
A non-Administrator install uses `%LOCALAPPDATA%\Gregg` and does not register
the daemon service.

The installers download release binaries and matching SHA-256 files, verify
the checksum and candidate version, then install the selected component. If no
binary is published for the host and Cargo is available, they build with
`cargo install --locked` in a private staging directory. A checksum or version
mismatch is an error, not a reason to fall back to Cargo. Source builds require
Rust 1.89 or newer. Linux release binaries target glibc 2.17; macOS release
binaries are unsigned. Linux ARMv7 and Windows ARM64 use the Cargo fallback.

On Unix, root installs use `/usr/local/bin`; user installs use
`$HOME/.local/bin`. The user installer can add that directory to supported
shell profiles. Use `--no-shell-profile` to skip profile changes. The installer
does not invoke `sudo` itself.

After installing `greggd`, Unix startup registration is handled by
`greggd startup install`. Linux uses systemd when running and cron otherwise;
macOS uses launchd. A systemd or launchd install requires the binary at
`/usr/local/bin/greggd` and appropriate privileges. If privileges are missing,
the command prints the elevated command instead of silently registering cron.
On Windows, the PowerShell installer owns SCM registration;
`greggd startup install` reports the current service state.

The bootstrap installers can be rerun at the same scope to replace an
identified Gregg binary. To update an installed executable directly, use
`gregg update` or `greggd update`. To remove one component, use its
`uninstall` command. Both support `--dry-run`; configuration is preserved by
default. `--purge` removes the resolved config and also the daemon log on
macOS. See the
[client](../docs/client.md) and [daemon](../docs/daemon.md) guides for details.

## Legacy local-build helpers

These helpers are for developers or operators who already have a local build.

Linux:

```sh
cargo build --release -p greggd
sudo ./packaging/install-linux.sh target/release/greggd
```

macOS:

```sh
cargo build --release -p greggd
sudo ./packaging/install-macos.sh target/release/greggd
```

Windows PowerShell:

```powershell
cargo build --release -p greggd
.\packaging\install-windows.ps1 -SourcePath .\target\release\greggd.exe
```

For systemd, launchd, cron, configuration paths, and network security, see the
[daemon guide](../docs/daemon.md). The daemon listener has no TLS or
authentication and should not be exposed to the public Internet.
