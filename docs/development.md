# Development

For normal installs, prefer the bootstrap installers in
[installation](installation.md). This page covers local builds and
operator-managed packaging.

## Local build and run

Local builds require Rust/Cargo 1.89 or newer; normal development uses
the stable toolchain (`rust-toolchain.toml`).

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

The shell and Python surfaces have their own gates, both blocking in CI:

```bash
shellcheck -x scripts/*.sh packaging/*.sh scripts/tests/*.sh
python3 -m pytest scripts/tests -q
```

`pytest` needs the Rust toolchain: one case drives the sustained-workload
runner, which runs `cargo test --no-run` to locate its workload binary, so that
case's timeout covers a build rather than the 2-second workload. Override it with
`GREGG_TEST_BUILD_TIMEOUT_SECONDS` on a slow machine.

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
