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
# Pinned SHA-256 for the Zig release tarballs (from
# https://ziglang.org/download/index.json `shasum` fields). Verified before
# extraction so a compromised CDN/MITM cannot inject the compiler that builds
# all Linux release assets. Fail closed on mismatch.
case "$ZIG_ARCH" in
  x86_64) ZIG_SHA256_DEFAULT="473ec26806133cf4d1918caf1a410f8403a13d979726a9045b421b685031a982" ;;
  aarch64) ZIG_SHA256_DEFAULT="ab64e3ea277f6fc5f3d723dcd95d9ce1ab282c8ed0f431b4de880d30df891e4f" ;;
esac
if [[ "$ZIG_VERSION" != "0.14.0" && -z "${ZIG_SHA256:-}" ]]; then
  echo "error: no pinned SHA-256 for Zig $ZIG_VERSION ($ZIG_ARCH); set ZIG_SHA256 explicitly" >&2
  exit 1
fi
ZIG_SHA256="${ZIG_SHA256:-$ZIG_SHA256_DEFAULT}"
curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-${ZIG_ARCH}-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz
echo "${ZIG_SHA256}  /tmp/zig.tar.xz" | sha256sum -c -
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
