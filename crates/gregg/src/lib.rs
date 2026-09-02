//! `gregg` — compact keyboard-first terminal monitor.
//!
//! Polls [`greggd`](https://crates.io/crates/greggd) endpoints and renders
//! per-system CPU, memory, swap, and disk usage in a live TUI built with
//! [`ratatui`](https://crates.io/crates/ratatui).
//!
//! The crate exposes its internal modules as a library so that
//! [docs.rs](https://docs.rs/gregg) can generate API documentation and
//! downstream tools can reuse individual components (configuration,
//! polling, state management, and rendering).

pub mod action;
pub mod cli;
pub mod clock;
pub mod config;
pub mod eggpool;
pub mod eggpool_endpoint;
pub mod endpoint;
pub mod event;
pub mod input;
pub mod normalized;
pub mod poller;
pub mod scheduler;
pub mod state;
pub mod terminal;
pub mod ui;
pub mod update;

#[cfg(test)]
mod mixed_fleet_evidence;

#[cfg(test)]
mod sustained_workload;
