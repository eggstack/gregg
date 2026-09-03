# Installation

The default installation path is the bootstrap installer, which downloads
prebuilt binaries from GitHub Releases. Cargo is the fallback for
source-only hosts.

## Linux / macOS

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | bash -s -- gregg
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- greggd
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- both
```

Pinned version:

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/download/v1.0.12/install.sh | bash -s -- --version 1.0.12 gregg
./packaging/install.sh --version 1.0.12 greggd
```

The script requires bash. Piping to `sh` fails on Debian/Ubuntu, where `sh`
is dash; the script exits nonzero with a clear error when run without bash.

How it works:

- Maps `uname -s`/`uname -m` to the Rust target in the table below
  (`Linux`+`aarch64` → `aarch64-unknown-linux-gnu`,
  `Darwin`+`arm64` → `aarch64-apple-darwin`).
- Downloads the matching `gregg-<target>`/`greggd-<target>` asset and its
  `.sha256` from `releases/latest/download` (or `releases/download/vX.Y.Z`
  when pinned) into a fresh `mktemp -d` (cleaned up on success and failure).
- Verifies SHA-256 (`sha256sum` on Linux, `shasum -a 256` on macOS) before
  any execution, then requires `<candidate> version` to print the expected
  program name (and the exact version when pinned). A checksum or version
  mismatch is a hard error with no Cargo fallback.
- Installs to `/usr/local/bin` when root, otherwise `$HOME/.local/bin`,
  and warns when the destination is not on `PATH` (add
  `export PATH="$HOME/.local/bin:$PATH"` to your shell profile). It never
  edits shell rc files and never silently invokes `sudo`.
- When no matching asset exists (for example `armv7l` or an unknown
  OS/arch) it falls back to `cargo install --locked` (with
  `--version "=X.Y.Z"` when pinned) if Cargo is available.
- After a verified `greggd` install it delegates startup to
  `greggd startup install` (see [daemon](daemon.md)). A non-root install on
  a systemd/launchd host prints the exact elevated
  `sudo <exe> startup install --method <...>` command instead of silently
  registering a cron duplicate.

Without a component argument, a terminal-attached run shows a small
selector; a piped run without a component prints usage and exits nonzero.

## Windows (PowerShell)

Run as Administrator for a system install (service + `%ProgramFiles%\Gregg`),
or as a regular user for `%LOCALAPPDATA%\Gregg` without service registration:

```powershell
irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
.\install.ps1 -Component Gregg
.\install.ps1 -Component Greggd
.\install.ps1 -Component Both -Version 1.0.12
```

The script detects `AMD64` → `x86_64-pc-windows-msvc`, downloads the
matching `.exe` plus `.sha256` to a private temp dir, verifies with
`Get-FileHash -Algorithm SHA256` and a candidate `version` check, then
installs. An existing `%ProgramData%\gregg\greggd.toml` is preserved. When
Administrator, it registers the `greggd` service (`NT AUTHORITY\LocalService`,
`auto` start, restart-on-failure) and starts it. Windows ARM64 is
source-build only and uses the Cargo fallback when available.

## Direct download (no bootstrap)

```bash
curl -fsSL -o greggd-x86_64-unknown-linux-gnu https://github.com/eggstack/gregg/releases/latest/download/greggd-x86_64-unknown-linux-gnu
curl -fsSL -o greggd-x86_64-unknown-linux-gnu.sha256 https://github.com/eggstack/gregg/releases/latest/download/greggd-x86_64-unknown-linux-gnu.sha256
sha256sum -c greggd-x86_64-unknown-linux-gnu.sha256 && chmod +x greggd-x86_64-unknown-linux-gnu && sudo install -m 755 greggd-x86_64-unknown-linux-gnu /usr/local/bin/greggd
```

Substitute your platform's asset suffix from the supported-targets table in
the README.

## Cargo (source installs and source-only hosts)

```bash
cargo install gregg --locked
cargo install greggd --locked
```

## Platform notes

- Linux GNU assets use a glibc 2.17 floor so they run on long-lived
  Debian/Ubuntu/Armbian SBC images.
- macOS binaries are unsigned; Gatekeeper may quarantine them until approved
  via System Settings or `xattr -d com.apple.quarantine`.
- Linux ARMv7 (`armv7l`) and Windows ARM64 are source-build only; the
  installers fall back to Cargo when available.
