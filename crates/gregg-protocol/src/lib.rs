//! `gregg-protocol` defines the versioned JSON wire contract shared by the
//! `greggd` daemon and the `gregg` client.
//!
//! The crate is intentionally dependency-light (only `serde`, `serde_json`, and
//! `thiserror`) so it can be consumed by collectors, the HTTP server, the
//! polling engine, and tests without dragging in larger stacks.
//!
//! # Schema version 1
//!
//! Every snapshot carries an explicit
//! [`SCHEMA_VERSION_V1`](constant.SCHEMA_VERSION_V1) so clients can reject
//! incompatible payloads per host without terminating the whole TUI.
//!
//! Numeric values are transported as raw units — bytes for memory and swap,
//! percentages in the closed interval `0.0..=100.0` for utilization, and
//! milliseconds since the Unix epoch for timestamps. No human-formatted
//! strings cross the wire.
//!
//! # Compatibility policy
//!
//! Within schema version 1:
//!
//! - Unknown additive JSON fields are ignored by default.
//! - Required version-1 fields remain required unless explicitly changed to
//!   optional under an additive compatibility decision.
//! - Capability flags control interpretation of optional metrics. A `None`
//!   value paired with a `false` capability is expected; a `None` value
//!   paired with a `true` capability indicates a missing or still-warming
//!   sample.
//!
//! # Examples
//!
//! ```
//! use gregg_protocol::{StatusSnapshot, HealthResponse, ReadinessState, SCHEMA_VERSION_V1};
//!
//! let json = format!(r#"{{
//!     "schema_version": {sv},
//!     "observed_at_unix_ms": 1,
//!     "sample_interval_ms": 1000,
//!     "capabilities": {{ "cpu_iowait": false }},
//!     "system": {{
//!         "name": "mac-mini",
//!         "hostname": "mac-mini.local",
//!         "os_name": "macos",
//!         "os_version": "15.0",
//!         "kernel_name": "Darwin",
//!         "kernel_release": "24.0.0",
//!         "architecture": "arm64"
//!     }},
//!     "cpu": {{ "logical_cores": 8, "usage_pct": 12.5, "iowait_pct": null }},
//!     "load": {{ "one": 1.1, "five": 0.9, "fifteen": 0.6 }},
//!     "memory": {{ "used_bytes": 1, "total_bytes": 2, "usage_pct": 50.0 }},
//!     "swap": {{ "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }}
//! }}"#, sv = SCHEMA_VERSION_V1);
//!
//! let snap: StatusSnapshot = serde_json::from_str(&json).expect("valid snapshot");
//! snap.validate().expect("snapshot validates");
//!
//! let health = HealthResponse::warming();
//! assert_eq!(health.state, ReadinessState::Warming);
//! ```

#![forbid(unsafe_code)]

mod health;
mod snapshot;
mod validate;

#[cfg(feature = "test_support")]
pub mod test_support;

pub use health::{HealthCategory, HealthResponse, ReadinessState};
pub use snapshot::{
    CpuMetrics, LoadAverage, MemoryMetrics, MetricCapabilities, StatusSnapshot, SwapMetrics,
    SystemIdentity,
};
pub use validate::{ValidationViolation, ViolationKind};

/// Schema major version implemented by this crate.
///
/// Wire payloads whose `schema_version` does not match this value are
/// rejected by [`StatusSnapshot::validate`]. Additive changes within version 1
/// are allowed by the compatibility policy; breaking changes require a new
/// schema major and explicit migration handling.
pub const SCHEMA_VERSION_V1: u16 = 1;
