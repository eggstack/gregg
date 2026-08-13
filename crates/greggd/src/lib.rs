//! `greggd` library.
//!
//! Provides the daemon components: configuration, service management,
//! metrics collection, periodic sampling, and HTTP server.

pub mod cli;
pub mod collector;
pub mod config;
#[cfg(unix)]
pub mod control;
pub mod run;
pub mod sampler;
pub mod server;
#[cfg(target_os = "windows")]
pub mod service;
