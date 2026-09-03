# Development

For normal installs, prefer the bootstrap installers in
[installation](installation.md). This page covers local builds and
operator-managed packaging.

## Local build and run

```bash
cargo build --release -p greggd
cargo build --release -p gregg
```

Run the daemon unprivileged with a temporary config (avoids root and does
not touch the system service manager):

```bash
greggd run --config /tmp/test-config.toml
```

Fast routine check (format + workspace tests):

```bash
./scripts/check-local.sh          # Linux/macOS
.\scripts\check-local.ps1         # Windows PowerShell
```

## Operator-managed service install (legacy helpers)

These helpers remain for local builds where a checkout is present. They do
not duplicate the bootstrap download/verify logic.

Linux (systemd, requires root):

```bash
cargo build --release -p greggd
sudo ./packaging/install-linux.sh target/release/greggd
sudo systemctl enable --now greggd
```

Or install the binary and let the daemon own registration:

```bash
sudo install -m 755 target/release/greggd /usr/local/bin/greggd
sudo greggd startup install
```

macOS (launchd, requires root):

```bash
cargo build --release -p greggd
sudo ./packaging/install-macos.sh target/release/greggd
```

Windows (PowerShell, requires Administrator):

```powershell
cargo build --release -p greggd
.\packaging\install-windows.ps1 -SourcePath .\target\release\greggd.exe
Get-Service greggd
```

All install scripts are idempotent and preserve existing config. Uninstall
notes (Linux/macOS service removal, Windows
`.\packaging\uninstall-windows.ps1`) live in `packaging/README.md`.

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md).
