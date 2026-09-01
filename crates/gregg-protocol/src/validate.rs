//! Snapshot validation.
//!
//! Validation is deliberately separate from serde deserialization so that
//! forward-compatible additive changes do not silently change how strict the
//! crate is about individual fields.
//!
//! Accepted risk: percentage values and their byte-count counterparts are
//! validated independently (`usage_pct` in `0.0..=100.0`; `used_bytes <=
//! total_bytes`). A payload with internally contradictory but individually
//! valid values passes validation by design: the daemon is the source of
//! truth and no cross-check between derived and raw values exists on the
//! wire contract level.

use std::fmt;

use thiserror::Error;

use crate::{
    snapshot::{CpuMetrics, LoadAverage, MemoryMetrics, StatusSnapshot, SwapMetrics},
    SystemIdentity, MAX_IDENTITY_FIELD_BYTES, MAX_SAMPLE_INTERVAL_MS, SCHEMA_VERSION_V1,
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
    /// The sampling cadence exceeded the protocol maximum.
    SampleIntervalOutOfRange { max_ms: u64 },
    /// A percentage value was not finite (NaN or infinite).
    PercentageNotFinite,
    /// A percentage value was outside the closed `0.0..=100.0` interval.
    PercentageOutOfRange,
    /// A load average was non-finite or negative.
    LoadValueOutOfRange,
    /// `used_bytes` exceeded `total_bytes`.
    UsedExceedsTotal,
    /// `cpu_iowait` capability and `iowait_pct` presence disagreed.
    IowaitCapabilityMismatch,
    /// An identity string was empty or contained NUL padding.
    InvalidIdentityField,
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found } => write!(
                f,
                "unsupported schema_version {found} (expected {SCHEMA_VERSION_V1})"
            ),
            Self::ZeroNotAllowed => f.write_str("value must be positive"),
            Self::SampleIntervalOutOfRange { max_ms } => {
                write!(f, "sample interval must be at most {max_ms} ms")
            }
            Self::PercentageNotFinite => f.write_str("percentage must be finite"),
            Self::PercentageOutOfRange => f.write_str("percentage must be in 0.0..=100.0"),
            Self::LoadValueOutOfRange => {
                f.write_str("load average must be finite and non-negative")
            }
            Self::UsedExceedsTotal => f.write_str("used_bytes exceeds total_bytes"),
            Self::IowaitCapabilityMismatch => {
                f.write_str("iowait_pct must be Some(_) iff cpu_iowait capability is true")
            }
            Self::InvalidIdentityField => {
                f.write_str("identity field must be non-empty and contain no NUL characters")
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
    } else if snap.sample_interval_ms > MAX_SAMPLE_INTERVAL_MS {
        violations.push(ValidationViolation::new(
            ViolationKind::SampleIntervalOutOfRange {
                max_ms: MAX_SAMPLE_INTERVAL_MS,
            },
            "sample_interval_ms",
        ));
    }

    validate_identity(&snap.system, &mut violations);
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

fn validate_identity(system: &SystemIdentity, out: &mut Vec<ValidationViolation>) {
    let fields = [
        ("system.name", &system.name),
        ("system.hostname", &system.hostname),
        ("system.os_name", &system.os_name),
        ("system.os_version", &system.os_version),
        ("system.kernel_name", &system.kernel_name),
        ("system.kernel_release", &system.kernel_release),
        ("system.architecture", &system.architecture),
    ];
    for (field, value) in fields {
        if value.trim().is_empty() || value.contains('\0') || value.len() > MAX_IDENTITY_FIELD_BYTES
        {
            out.push(ValidationViolation::new(
                ViolationKind::InvalidIdentityField,
                field,
            ));
        }
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
            check_percentage(value, "cpu.iowait_pct", out);
            if !cpu_iowait {
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
            ViolationKind::LoadValueOutOfRange,
            field,
        ));
    }
}

fn validate_memory(memory: &MemoryMetrics, out: &mut Vec<ValidationViolation>) {
    let before = out.len();
    check_percentage(memory.usage_pct, "memory.usage_pct", out);
    let pct_flagged = out.len() > before;
    if memory.used_bytes > memory.total_bytes {
        out.push(ValidationViolation::new(
            ViolationKind::UsedExceedsTotal,
            "memory.used_bytes",
        ));
    }
    if memory.total_bytes == 0 && (memory.used_bytes > 0 || memory.usage_pct != 0.0) {
        out.push(ValidationViolation::new(
            ViolationKind::ZeroNotAllowed,
            "memory.total_bytes",
        ));
        if !pct_flagged && memory.usage_pct != 0.0 {
            out.push(ValidationViolation::new(
                ViolationKind::PercentageOutOfRange,
                "memory.usage_pct",
            ));
        }
    }
}

fn validate_swap(swap: &SwapMetrics, out: &mut Vec<ValidationViolation>) {
    let before = out.len();
    check_percentage(swap.usage_pct, "swap.usage_pct", out);
    let pct_flagged = out.len() > before;
    if swap.used_bytes > swap.total_bytes {
        out.push(ValidationViolation::new(
            ViolationKind::UsedExceedsTotal,
            "swap.used_bytes",
        ));
    }
    if swap.total_bytes == 0 && (swap.used_bytes > 0 || swap.usage_pct != 0.0) {
        out.push(ValidationViolation::new(
            ViolationKind::ZeroNotAllowed,
            "swap.total_bytes",
        ));
        if !pct_flagged && swap.usage_pct != 0.0 {
            out.push(ValidationViolation::new(
                ViolationKind::PercentageOutOfRange,
                "swap.usage_pct",
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::MetricCapabilities;

    fn valid_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            schema_version: SCHEMA_VERSION_V1,
            observed_at_unix_ms: 1,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilities { cpu_iowait: true },
            system: SystemIdentity {
                name: "test".into(),
                hostname: "test.local".into(),
                os_name: "linux".into(),
                os_version: "1.0".into(),
                kernel_name: "Linux".into(),
                kernel_release: "6.0.0".into(),
                architecture: "x86_64".into(),
            },
            cpu: CpuMetrics {
                logical_cores: 8,
                usage_pct: 25.2,
                iowait_pct: Some(0.4),
            },
            load: LoadAverage {
                one: 1.32,
                five: 0.91,
                fifteen: 0.62,
            },
            memory: MemoryMetrics {
                used_bytes: 5_900_000_000,
                total_bytes: 15_600_000_000,
                usage_pct: 37.8,
            },
            swap: SwapMetrics {
                used_bytes: 0,
                total_bytes: 4_000_000_000,
                usage_pct: 0.0,
            },
        }
    }

    #[test]
    fn accepts_well_formed_identity() {
        assert!(validate(&valid_snapshot()).is_ok());
    }

    #[test]
    fn rejects_identity_fields_that_are_empty_or_nul_padded() {
        let mut snap = valid_snapshot();
        snap.system.name = String::new();
        snap.system.hostname = "host\0".into();
        let err = validate(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "system.name" && v.kind == ViolationKind::InvalidIdentityField));
        assert!(
            err.iter()
                .any(|v| v.field == "system.hostname"
                    && v.kind == ViolationKind::InvalidIdentityField)
        );
    }

    #[test]
    fn rejects_identity_fields_that_are_whitespace_only() {
        let mut snap = valid_snapshot();
        snap.system.name = "   ".into();
        snap.system.hostname = "\t\n".into();
        let err = validate(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "system.name"));
        assert!(err.iter().any(|v| v.field == "system.hostname"));
    }

    #[test]
    fn rejects_identity_fields_that_are_too_long() {
        let mut snap = valid_snapshot();
        snap.system.hostname = "x".repeat(MAX_IDENTITY_FIELD_BYTES + 1);
        let err = validate(&snap).unwrap_err();
        assert!(err.iter().any(|v| {
            v.field == "system.hostname" && v.kind == ViolationKind::InvalidIdentityField
        }));
    }

    #[test]
    fn zero_total_does_not_duplicate_percentage_violations() {
        let mut snap = valid_snapshot();
        snap.memory.total_bytes = 0;
        snap.memory.usage_pct = 150.0;
        snap.swap.total_bytes = 0;
        snap.swap.usage_pct = 150.0;
        let err = validate(&snap).unwrap_err();
        for field in ["memory.usage_pct", "swap.usage_pct"] {
            let count = err
                .iter()
                .filter(|v| v.field == field && v.kind == ViolationKind::PercentageOutOfRange)
                .count();
            assert_eq!(count, 1, "duplicate PercentageOutOfRange for {field}");
        }
    }

    #[test]
    fn zero_total_with_nonzero_used_reports_zero_not_allowed() {
        let mut snap = valid_snapshot();
        snap.memory.total_bytes = 0;
        snap.memory.used_bytes = 5;
        snap.memory.usage_pct = 0.0;
        snap.swap.total_bytes = 0;
        snap.swap.used_bytes = 5;
        snap.swap.usage_pct = 0.0;
        let err = validate(&snap).unwrap_err();
        for field in ["memory.total_bytes", "swap.total_bytes"] {
            assert!(
                err.iter()
                    .any(|v| v.field == field && v.kind == ViolationKind::ZeroNotAllowed),
                "missing ZeroNotAllowed for {field}"
            );
        }
        assert!(
            !err.iter().any(|v| v.field.ends_with("usage_pct")),
            "valid zero percentages must not be flagged: {err:?}"
        );
    }

    #[test]
    fn all_zero_memory_and_swap_remain_valid() {
        let mut snap = valid_snapshot();
        snap.memory = MemoryMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        };
        snap.swap = SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        };
        assert!(validate(&snap).is_ok());
    }

    #[test]
    fn iowait_value_is_validated_even_when_capability_flag_is_false() {
        let mut snap = valid_snapshot();
        snap.capabilities.cpu_iowait = false;
        snap.cpu.iowait_pct = Some(f32::NAN);
        let err = validate(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "cpu.iowait_pct" && v.kind == ViolationKind::PercentageNotFinite));
        assert!(err
            .iter()
            .any(|v| v.field == "cpu.iowait_pct"
                && v.kind == ViolationKind::IowaitCapabilityMismatch));

        snap.cpu.iowait_pct = Some(150.0);
        let err = validate(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "cpu.iowait_pct" && v.kind == ViolationKind::PercentageOutOfRange));

        snap.cpu.iowait_pct = Some(-0.5);
        let err = validate(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "cpu.iowait_pct" && v.kind == ViolationKind::PercentageOutOfRange));
    }
}
