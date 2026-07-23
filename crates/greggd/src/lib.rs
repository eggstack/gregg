//! `greggd` library.
//!
//! Phase 2 adds the Linux collector module behind `cfg(target_os = "linux")`.
//! Phase 3 will add the macOS collector. Phase 4 wires the sampler and HTTP
//! server. Until then the binary only reports its protocol schema version
//! so the workspace remains packageable.

pub mod collector;
