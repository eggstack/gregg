//! `greggd` library.
//!
//! Provides the daemon components: configuration, service management,
//! metrics collection, periodic sampling, and HTTP server.

pub mod cli;
pub mod collector;
pub mod config;
pub mod run;
pub mod sampler;
pub mod server;
pub mod service;
