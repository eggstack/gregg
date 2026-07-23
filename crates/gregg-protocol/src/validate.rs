//! Snapshot validation.
//!
//! Validation is deliberately separate from serde deserialization so that
//! forward-compatible additive changes do not silently change how strict the
//! crate is about individual fields.

use std::fmt;

use thiserror::Error;

use crate::{
    snapshot::{CpuMetrics, LoadAverage, MemoryMetrics, StatusSnapshot, SwapMetrics},
    SCHEMA_VERSION_V1,
};

/// A single protocol-invariant violation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind}")]
pub struct ValidationViolation {
    /// Field-level violation kind.
    pub kind: ViolationKind,
    /// JSON path to the offending field, in dotted lowercase form.
    pub field: String,
}

impl ValidationViolation {
    fn new(kind: ViolationKind, field: impl Into<String>) -> Self {
        Self {
            kind,
            field: field.into(),
        }
    }
}

/// The kind of a single protocol-invariant violation.
///
/// Each variant carries enough information for the caller to log a precise
/// diagnostic without parsing the message string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    /// `schema_version` did not match the supported version.
    UnsupportedSchemaVersion { found: u16 },
    /// An integer count that must be positive was zero.
    ZeroNotAllowed,
    /// A percentage value was not finite (NaN or infinite).
    PercentageNotFinite,
    /// A percentage value was outside the closed `0.0..=100.0` interval.
    PercentageOutOfRange,
    /// `used_bytes` exceeded `total_bytes`.
    UsedExceedsTotal,
    /// `cpu_iowait` capability and `iowait_pct` presence disagreed.
    IowaitCapabilityMismatch,
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found } => write!(
                f,
                "unsupported schema_version {found} (expected {SCHEMA_VERSION_V1})"
            ),
            Self::ZeroNotAllowed => f.write_str("value must be positive"),
            Self::PercentageNotFinite => f.write_str("percentage must be finite"),
            Self::PercentageOutOfRange => f.write_str("percentage must be in 0.0..=100.0"),
            Self::UsedExceedsTotal => f.write_str("used_bytes exceeds total_bytes"),
            Self::IowaitCapabilityMismatch => {
                f.write_str("iowait_pct must be Some(_) iff cpu_iowait capability is true")
            }
        }
    }
}

/// Validate a snapshot against every version-1 invariant.
pub(crate) fn validate(snap: &StatusSnapshot) -> Result<(), Vec<ValidationViolation>> {
    let mut violations = Vec::new();

    if snap.schema_version != SCHEMA_VERSION_V1 {
        violations.push(ValidationViolation::new(
            ViolationKind::UnsupportedSchemaVersion {
                found: snap.schema_version,
            },
            "schema_version",
        ));
    }

    if snap.observed_at_unix_ms == 0 {
        violations.push(ValidationViolation::new(
            ViolationKind::ZeroNotAllowed,
            "observed_at_unix_ms",
        ));
    }
    if snap.sample_interval_ms == 0 {
        violations.push(ValidationViolation::new(
            ViolationKind::ZeroNotAllowed,
            "sample_interval_ms",
        ));
    }

    validate_cpu(&snap.cpu, snap.capabilities.cpu_iowait, &mut violations);
    validate_load(&snap.load, &mut violations);
    validate_memory(&snap.memory, &mut violations);
    validate_swap(&snap.swap, &mut violations);

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn validate_cpu(cpu: &CpuMetrics, cpu_iowait: bool, out: &mut Vec<ValidationViolation>) {
    if cpu.logical_cores == 0 {
        out.push(ValidationViolation::new(
            ViolationKind::ZeroNotAllowed,
            "cpu.logical_cores",
        ));
    }
    check_percentage(cpu.usage_pct, "cpu.usage_pct", out);
    match cpu.iowait_pct {
        None => {
            if cpu_iowait {
                out.push(ValidationViolation::new(
                    ViolationKind::IowaitCapabilityMismatch,
                    "cpu.iowait_pct",
                ));
            }
        }
        Some(value) => {
            if cpu_iowait {
                check_percentage(value, "cpu.iowait_pct", out);
            } else {
                out.push(ValidationViolation::new(
                    ViolationKind::IowaitCapabilityMismatch,
                    "cpu.iowait_pct",
                ));
            }
        }
    }
}

fn validate_load(load: &LoadAverage, out: &mut Vec<ValidationViolation>) {
    check_load(load.one, "load.one", out);
    check_load(load.five, "load.five", out);
    check_load(load.fifteen, "load.fifteen", out);
}

fn check_load(value: f32, field: &str, out: &mut Vec<ValidationViolation>) {
    if !value.is_finite() || value < 0.0 {
        out.push(ValidationViolation::new(
            ViolationKind::PercentageOutOfRange,
            field,
        ));
    }
}

fn validate_memory(memory: &MemoryMetrics, out: &mut Vec<ValidationViolation>) {
    check_percentage(memory.usage_pct, "memory.usage_pct", out);
    if memory.used_bytes > memory.total_bytes {
        out.push(ValidationViolation::new(
            ViolationKind::UsedExceedsTotal,
            "memory.used_bytes",
        ));
    }
}

fn validate_swap(swap: &SwapMetrics, out: &mut Vec<ValidationViolation>) {
    check_percentage(swap.usage_pct, "swap.usage_pct", out);
    if swap.used_bytes > swap.total_bytes {
        out.push(ValidationViolation::new(
            ViolationKind::UsedExceedsTotal,
            "swap.used_bytes",
        ));
    }
    if swap.total_bytes == 0 && swap.usage_pct != 0.0 {
        out.push(ValidationViolation::new(
            ViolationKind::PercentageOutOfRange,
            "swap.usage_pct",
        ));
    }
}

fn check_percentage(value: f32, field: &str, out: &mut Vec<ValidationViolation>) {
    if !value.is_finite() {
        out.push(ValidationViolation::new(
            ViolationKind::PercentageNotFinite,
            field,
        ));
        return;
    }
    if !(0.0..=100.0).contains(&value) {
        out.push(ValidationViolation::new(
            ViolationKind::PercentageOutOfRange,
            field,
        ));
    }
}
