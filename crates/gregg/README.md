# gregg

[![Crates.io](https://img.shields.io/crates/v/gregg.svg)](https://crates.io/crates/gregg)
[![Docs.rs](https://docs.rs/gregg/badge.svg)](https://docs.rs/gregg)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Downloads](https://img.shields.io/crates/d/gregg.svg)](https://crates.io/crates/gregg)

Compact keyboard-first terminal monitor for observing system metrics across
multiple machines.

## Installation

The current published `v1.0.12` release is source-only. Cargo is the working
installation path until the first binary-bearing release; the prebuilt
installer is the intended path afterward.

```sh
# Current working installation
cargo install gregg --locked

# After a binary-bearing release is published
# curl -fsSL https://github.com/eggstack/gregg/releases/latest/download/install.sh | sh -s -- gregg
```

Prebuilt assets (glibc 2.17 on Linux, unsigned on macOS):

- `gregg-x86_64-unknown-linux-gnu` · `gregg-aarch64-unknown-linux-gnu` (covers 64-bit Raspberry Pi/Le Potato)
- `gregg-x86_64-apple-darwin` · `gregg-aarch64-apple-darwin`
- `gregg-x86_64-pc-windows-msvc.exe`

Linux ARMv7 (`armv7l`) is source-build only and uses `cargo install` when available.

## Usage

Start the TUI:

```sh
gregg
```

Manage endpoints:

```sh
gregg add 192.168.1.10:11310
gregg add deadpool.local:11320
gregg add deadpool@192.168.1.10:11310          # `nickname@host:port` form
gregg add http://192.168.1.10:11310/
gregg add 192.168.1.10:11310 --name deadpool  # explicit `--name` instead of `@`
gregg list
gregg remove 192.168.1.10                     # host-only remove is still supported
gregg refresh 30
gregg edit
gregg update                                 # binary-first self-update to latest stable crates.io version
```

`gregg add` requires an explicit port. Host-only input such as
`gregg add 192.168.1.10` is rejected; supply `host:port`, an HTTP URL
(`http://host:port/`), or `nickname@host:port`. Combining the inline
`nickname@` form with `--name` is rejected as ambiguous. The retained
`default_port` setting is configuration-compatible but is not used by
`gregg add`.

`gregg update` queries the latest stable `gregg` crate on crates.io
(`max_stable_version`), compares with the compiled-in version, and if newer
downloads the exact `vX.Y.Z` GitHub Release asset for the current host
plus `.sha256`, verifies checksum and candidate `version` before any
replacement, then atomically replaces the current executable (via
`self-replace`, same-filesystem rename on Unix). Missing asset (404)
falls back to `cargo install --locked --version "=X.Y.Z"` staged to a temp
`--root`; checksum/version mismatch or transport failure never falls back.
No background checks or `sudo` internally; permission failures print
`sudo gregg update`.

IPv6 link-local zone identifiers are accepted in bare `%eth0` or URL-escaped
`%25eth0` form and are stored in URL-safe `%25` form for polling.
Bracketed endpoint syntax is reserved for IPv6 literals; values such as
`[server.local]:11310` are rejected.
The optional EggPool client reports `InvalidEndpoint` for zone-ID authorities
because the pinned URL representation cannot safely encode that authority;
the systems poller remains the supported zone-ID transport.

The client configuration accepts `request_timeout_ms` from 100 through 60,000
milliseconds. Values outside that range are rejected during config validation.

## Navigation

| Key | Action |
| --- | --- |
| `j` / Down | Next system |
| `k` / Up | Previous system |
| `h` / Left | Previous view |
| `l` / Right | Next view |
| `v` | Toggle Normal/Condensed Systems view |
| `e` | Expand or collapse selected-system drives |
| `Ctrl-R` | Reload Systems config and refresh, or refresh EggPool |

If a Systems config reload is missing, malformed, or invalid, the last-known-good
configuration remains active and the error is shown in the diagnostic line until
a later reload succeeds.
If the bounded scheduler channel is full, delivery remains ordered while the
event loop continues processing input and poll results.

`selected_id` is the persistent logical selection that drives `e`
(drive expansion) and viewport behavior. The reverse-video highlight
on the selected device is transient: it activates when you navigate,
and disappears after about ten seconds of inactivity. Leaving the
Systems pane or returning to it does not extend or re-trigger the
highlight, and `e` continues to operate on the same logical system
after the highlight fades.

## Requirements

Each monitored host must have `greggd` running and reachable on the
configured port (default 11310).

The normal view uses five rows for an online system, including aggregate disk
capacity. All four metric rows share a fleet-wide `bar_width` so the opening
`[` and closing `]` columns align across every online system. When the
longest natural metric suffix across the entire online fleet exceeds one
quarter of the terminal width, every normal-view metric row collapses to
bar-only — the `[` and `]` brackets remain aligned and the percentage, core
count, and byte-count suffix disappears. Resizing wider restores them
dynamically without restart. The aggregate DISK suffix renders
`<used bytes> / <total bytes>` so the denominator matches the percentage;
explicit caller-available space is preserved by the normalized model and
surfaced only through the expanded per-drive rows. The condensed view uses
one comparison row per system. `e` adds bounded detail rows for valid
mounted-local-filesystem records belonging only to the selected online
system; the expanded rows share one table layout so mount names, used,
total, remaining, and percentage columns stay aligned across drives. View
and expansion state are not persisted.

## Supported platforms

| Platform | Architecture | Rust target | Status |
| --- | --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` | Supported (glibc 2.17) |
| Linux | ARM64 (64-bit Pi/Le Potato) | `aarch64-unknown-linux-gnu` | Supported (glibc 2.17) |
| macOS | Intel (x86-64) | `x86_64-apple-darwin` | Supported (unsigned) |
| macOS | Apple Silicon (arm64) | `aarch64-apple-darwin` | Supported (unsigned) |
| Windows | x86-64 | `x86_64-pc-windows-msvc` | Supported |

macOS binaries are unsigned; Gatekeeper may require approval via System
Settings or `xattr -d com.apple.quarantine`. Linux ARMv7 is source-only.

On Windows, configuration is stored in `%APPDATA%\gregg\gregg.toml`.
Cross-process config locking uses Windows-native `LockFileEx`.
The optional `gregg eggpool add HOST` command configures one EggPool source,
using HTTP port `11300` by default. Use `--https`, `--name`,
`--api-key-env ENV_NAME`, and `--replace` as needed. Only the environment
variable name is persisted; the secret is never read by these commands.
Adding a second source without `--replace` reports a configuration conflict,
not a name-validation error.
The TUI consumes only `/api/stats/summary` and requires EggPool's dashboard/
statistics routes to be enabled. It displays accounted tokens, provider
cache-read share, output tokens per second, and average TTFT for `1h`, `24h`,
`7d`, or `30d`; it does not display a request-level cache hit rate or invent
values when a metric is unavailable. `j`/Down and `k`/Up select the period,
while `h`/Left and `l`/Right enter or leave the pane. `Ctrl-R` refreshes only
the active pane, and EggPool's active refresh cadence is fixed at 60 seconds.
Omit `[eggpool]` to remove the pane and all EggPool worker/network activity.
Editor fallbacks: `hx`, `vim`, `vi` (Unix) or `hx`, `code`, `notepad` (Windows).
Malformed or URL-unrepresentable EggPool endpoints are reported as invalid
endpoints rather than generic network failures.

## Links

- Repository: <https://github.com/eggstack/gregg>
- Project: <https://github.com/eggstack/gregg>

## License

MIT
