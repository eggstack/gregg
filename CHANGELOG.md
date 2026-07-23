# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and
this project adheres to [Semantic Versioning](https://semver.org/).

## [1.0.0] - 2026-07-23

### Added

- `gregg-protocol` crate: versioned JSON wire types, metric capabilities,
  identity structures, and snapshot validation for schema version 1.
- `greggd` crate: lightweight Linux and macOS metrics daemon with read-only
  HTTP API (`/`, `/v1/status`, `/healthz`), periodic sampling, graceful
  shutdown, TOML configuration, and native service integration (systemd,
  launchd).
- `gregg` crate: compact keyboard-first terminal monitor with endpoint
  management (`add`, `list`, `remove`, `refresh`, `edit`), bounded concurrent
  polling, application state engine, and Ratatui-based four-row-per-system TUI.
- Native Linux metrics collection from `/proc` (CPU, memory, swap, load,
  identity).
- macOS metrics collection from Mach host statistics and sysctl (CPU, memory,
  swap, load, identity).
- Protocol compatibility fixtures for Linux, macOS, and health responses.
- Supply-chain policy via `cargo-deny`.
- CI pipeline: formatting, clippy, tests, docs, and package validation on
  Linux and macOS.

### Known limitations

- macOS does not expose a Linux-equivalent aggregate CPU I/O-wait state.
  This is reported as unsupported (`iowait_pct: null`) rather than
  fabricated as zero.
- The daemon is designed for private-network use only. It does not provide
  TLS, authentication, rate limiting, or other public-internet hardening.
- Per-process inspection, historical telemetry, alerting, and web dashboards
  are explicitly out of scope for version 1.
