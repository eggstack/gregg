#!/usr/bin/env bash
# Install Zig (pinned stable) and cargo-zigbuild for the Linux glibc-2.17
# release builds. Shared by the x86_64 and AArch64 Linux release jobs so the
# version and install steps have one implementation.
#
#   bash scripts/release-install-zig.sh
#
# Env override: ZIG_VERSION (default 0.14.0). Appends the install dir to
# $GITHUB_PATH when running in GitHub Actions; otherwise prepends to PATH
# for the current shell via $HOME/.local/zig.
set -euo pipefail

ZIG_VERSION="${ZIG_VERSION:-0.14.0}"
case "$(uname -m)" in
  x86_64) ZIG_ARCH="x86_64" ;;
  aarch64|arm64) ZIG_ARCH="aarch64" ;;
  *) echo "error: unsupported arch $(uname -m)" >&2; exit 1 ;;
esac
curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-${ZIG_ARCH}-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz
mkdir -p "$HOME/.local/zig"
tar -xf /tmp/zig.tar.xz -C "$HOME/.local/zig" --strip-components=1
rm -f /tmp/zig.tar.xz
if [[ -n "${GITHUB_PATH:-}" ]]; then
  echo "$HOME/.local/zig" >> "$GITHUB_PATH"
fi
export PATH="$HOME/.local/zig:$PATH"
zig version
cargo install cargo-zigbuild --locked
cargo zigbuild --help
