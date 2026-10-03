//! Bounded maintenance scheduling primitives.
//!
//! This module starts with the cron/time boundary qualified by Plan 156.
//! Runtime execution and state ownership are implemented in the scheduler
//! submodules as the plan proceeds.

#[allow(dead_code)] // Plan 157 wires this qualified boundary into config and runtime.
pub(crate) mod schedule;
