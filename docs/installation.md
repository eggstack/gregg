# Installation

The default installation path is the bootstrap installer, which downloads
prebuilt binaries from GitHub Releases. Cargo is the fallback for
source-only hosts.

## Linux / macOS

```bash
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh \
  | bash -s -- gregg \
  && export PATH="$HOME/.local/bin:$PATH"
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- greggd
curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sudo bash -s -- both
```

The rootless client form carries a trailing parent-shell
`export PATH="$HOME/.local/bin:$PATH"` so `gregg` is immediately resolvable
without restarting the terminal. A piped installer runs as a child process
and cannot mutate its parent's environment, so the `export` must run in the
invoking shell after the pipeline succeeds. The shorter pipeline without the
trailing `export` remains valid when `~/.local/bin` is already on `PATH` or
when the operator will open a later shell after profile persistence. The
privileged `greggd` command needs no trailing export: it installs to
`/usr/local/bin`, which is usually already on `PATH`.

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
- Installs to `/usr/local/bin` when root, otherwise `$HOME/.local/bin`.
  A non-root install whose destination is absent from the current `PATH`
  persists `$HOME/.local/bin` to the user's shell profile for future shells
  by default: zsh uses `${ZDOTDIR:-$HOME}/.zshrc` (a safe absolute `ZDOTDIR`
  is honored), bash uses `~/.bashrc` on Linux and login-aware selection on
  macOS (existing `~/.bash_profile`, then `~/.bash_login`, then `~/.profile`,
  otherwise `~/.bash_profile`). The edit is idempotent (a recognizable active
  PATH integration or an intact Gregg-managed block is never duplicated;
  comments, commented-out assignments, and unrelated `.local/bin` mentions do
  not suppress integration), append-only (the rest of the profile is preserved byte-for-byte, never
  evaluated or sourced), and skippable with `--no-shell-profile`. Output
  distinguishes current-shell availability from future-shell persistence and
  always prints the exact `export PATH="$HOME/.local/bin:$PATH"` needed for
  the current shell. Root/system installs never edit user or global shell
  startup files (`/etc/profile`, `/etc/paths`, `/etc/zprofile`, and similar
  are untouched); when `/usr/local/bin` is unexpectedly absent from `PATH`
  only bounded advice is printed. The installer never silently invokes
  `sudo`.
- Rerunning at the same scope replaces that scope's component in place:
  output distinguishes a first install from an identified existing-Gregg
  replacement (showing existing and candidate versions). A destination that
  does not identify via its stable `version` command is never overwritten;
  the installer fails with an actionable diagnostic instead. The installer
  never searches `PATH` or crosses privilege boundaries.
- When no matching asset exists (for example `armv7l` or an unknown
  OS/arch) it builds via `cargo install --locked` (with
  `--version "=X.Y.Z"` when pinned) into a private staging root, verifies
  the staged binary exactly as a download, then copies only the executable
  to the destination. Staging is removed afterwards, so no Cargo ownership
  metadata persists beside the bootstrap binary. For `greggd`, the staged
  candidate then follows the same startup/config finalization as a prebuilt
  candidate, including safe SCM stop/replace/register/restart on Windows.
- After a verified `greggd` install it delegates startup to
  `greggd startup install` (see [daemon](daemon.md)). A non-root install on
  a systemd/launchd host prints the exact elevated
  `sudo <exe> startup install --method <...>` command instead of silently
  registering a cron duplicate.
- On a non-root same-scope `greggd` replacement, the installer checks the
  selected default-config daemon with `status` before overwriting it. If it was
  healthy, the new binary is activated with config-specific `stop` followed by
  `croncheck`; first installs and previously stopped daemons remain stopped.
  Prebuilt and staged-Cargo candidates share this path. A post-replacement
  activation failure returns nonzero and prints the exact retry command; the
  verified new binary is not rolled back.

Without a component argument, a terminal-attached run shows a small
selector; a piped run without a component prints usage and exits nonzero.

## Upgrades and uninstall

- Bootstrap rerun: `install.sh greggd` / `install.ps1 -Component Greggd` at
  the same scope reinstalls that scope's component (same replacement rules
  as above; pinned versions still honor the requested tag).
- Installed-binary upgrade: `gregg update` updates the exact invoked client
  binary; `greggd update` updates the exact invoked daemon binary plus a
  manager-aware restart. Do not route `update` through the bootstrap
  scripts.
- Removal: `gregg uninstall [--dry-run] [--purge]` and
  `greggd uninstall [--dry-run] [--purge]` remove only the exact invoked
  binary plus (for the daemon) startup artifacts whose command targets that
  exact binary. Foreign or ambiguous systemd/launchd/cron/SCM artifacts are
  preserved; SCM query uncertainty blocks mutation. Configuration is
  preserved by default; `--purge` is destructive. There is no
  `uninstall --all`: run both commands explicitly. Uninstall never removes
  the generic `$HOME/.local/bin` shell-profile PATH entry: that directory is
  a standard user executable location that may hold unrelated tools or a
  sibling Gregg component, so the profile entry is user-environment
  integration, not component-owned binary state. See
  [client](client.md) and [daemon](daemon.md).

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
`auto` start, restart-on-failure) and starts it. Reruns classify the
destination first (absent/replace/foreign) and report installs vs updates;
the Cargo fallback stages privately as on Unix. Windows ARM64 is
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

Direct source builds require Rust/Cargo 1.89 or newer
(`rust-version = "1.89"`); Cargo enforces the floor automatically.
Prebuilt binary installs need no compiler.

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
  installers fall back to Cargo when available (requires Rust 1.89+).
