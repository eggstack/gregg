//! `greggd` library.
//!
//! Provides the daemon components: configuration, service management,
//! metrics collection, periodic sampling, and HTTP server.

/// CLI argument parsing and subcommand dispatch for the daemon.
pub mod cli;
/// Native metrics collection for each supported platform.
pub mod collector;
/// Daemon configuration, validation, file I/O, and atomic persistence.
pub mod config;
/// Local Unix-domain control socket for `greggd stop`.
#[cfg(unix)]
pub mod control;
/// Local-network address resolution utilities.
pub mod net;
/// Foreground daemon entry point and shutdown orchestration.
pub mod run;
/// Periodic sampling loop that collects metrics and manages daemon state.
pub mod sampler;
/// HTTP server for the daemon status and health endpoints.
pub mod server;
/// Windows Service Control Manager integration.
///
/// Compiled on Windows and, for unit tests, on every platform so the
/// injectable SCM fake-adapter tests run deterministically without a
/// Windows host. Production SCM types stay Windows-only inside.
#[cfg(any(target_os = "windows", test))]
pub mod service;
/// Startup installation and restart helpers.
pub mod startup;
/// Read-only local diagnostic status composing config, version, health, and startup state.
pub mod status;
/// Component-safe uninstall.
pub mod uninstall;
/// Binary-first self-update.
pub mod update;
