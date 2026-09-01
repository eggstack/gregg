//! Schema-version-2 snapshot validation.
//!
//! Validation is deliberately separate from serde deserialization so that
//! forward-compatible additive changes do not silently change how strict the
//! crate is about individual fields.
//!
//! Accepted risk: as in v1, percentage values are validated independently of
//! their byte-count counterparts; no cross-check exists between a derived
//! `usage_pct` and its `used_bytes`/`total_bytes` inputs.

use std::fmt;

use thiserror::Error;

use crate::v2::{
    CommitMetrics, StatusPayloadV2, StatusSnapshotV2, SwapMetrics, MAX_DRIVE_ENTRIES,
    MAX_DRIVE_NAME_BYTES, SCHEMA_VERSION_V2,
};
use crate::{
    LoadAverage, MemoryMetrics, SystemIdentity, MAX_IDENTITY_FIELD_BYTES, MAX_SAMPLE_INTERVAL_MS,
};

/// A single protocol-invariant violation for v2 snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind}")]
pub struct ValidationViolationV2 {
    /// Field-level violation kind.
    pub kind: ViolationKindV2,
    /// JSON path to the offending field, in dotted lowercase form.
    pub field: String,
}

impl ValidationViolationV2 {
    fn new(kind: ViolationKindV2, field: impl Into<String>) -> Self {
        Self {
            kind,
            field: field.into(),
        }
    }
}

/// The kind of a single protocol-invariant violation for v2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKindV2 {
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
    /// `used_bytes` exceeded `total_bytes` or `limit_bytes`.
    UsedExceedsTotal,
    /// `available_bytes` exceeded `total_bytes`.
    AvailableExceedsTotal,
    /// `cpu_iowait` capability and `iowait_pct` presence disagreed.
    IowaitCapabilityMismatch,
    /// `load_average` capability and `load` presence disagreed.
    LoadCapabilityMismatch,
    /// `swap` capability and `swap` presence disagreed.
    SwapCapabilityMismatch,
    /// `memory_commit` capability and `commit` presence disagreed.
    CommitCapabilityMismatch,
    /// A drive display name was empty.
    EmptyDriveName,
    /// A drive display name exceeded the protocol bound.
    DriveNameTooLong { max_bytes: usize },
    /// The drive collection exceeded the protocol bound.
    TooManyDrives { max_entries: usize },
    /// An identity string was empty or contained NUL padding.
    InvalidIdentityField,
}

impl fmt::Display for ViolationKindV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found } => write!(
                f,
                "unsupported schema_version {found} (expected {SCHEMA_VERSION_V2})"
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
            Self::UsedExceedsTotal => f.write_str("used exceeds total/limit"),
            Self::AvailableExceedsTotal => f.write_str("available exceeds total"),
            Self::IowaitCapabilityMismatch => {
                f.write_str("iowait_pct must be Some(_) iff cpu_iowait capability is true")
            }
            Self::LoadCapabilityMismatch => {
                f.write_str("load must be Some(_) iff load_average capability is true")
            }
            Self::SwapCapabilityMismatch => {
                f.write_str("swap must be Some(_) iff swap capability is true")
            }
            Self::CommitCapabilityMismatch => {
                f.write_str("commit must be Some(_) iff memory_commit capability is true")
            }
            Self::EmptyDriveName => f.write_str("drive name must not be empty"),
            Self::DriveNameTooLong { max_bytes } => {
                write!(f, "drive name exceeds maximum length of {max_bytes} bytes")
            }
            Self::TooManyDrives { max_entries } => {
                write!(
                    f,
                    "drive list exceeds maximum length of {max_entries} entries"
                )
            }
            Self::InvalidIdentityField => {
                f.write_str("identity field must be non-empty and contain no NUL characters")
            }
        }
    }
}

/// Validate a v2 snapshot against every version-2 invariant.
///
/// Returns `Ok(())` or a list of structured violations.
pub fn validate_v2(snap: &StatusSnapshotV2) -> Result<(), Vec<ValidationViolationV2>> {
    let mut violations = Vec::new();

    if snap.schema_version != SCHEMA_VERSION_V2 {
        violations.push(ValidationViolationV2::new(
            ViolationKindV2::UnsupportedSchemaVersion {
                found: snap.schema_version,
            },
            "schema_version",
        ));
    }

    if snap.observed_at_unix_ms == 0 {
        violations.push(ValidationViolationV2::new(
            ViolationKindV2::ZeroNotAllowed,
            "observed_at_unix_ms",
        ));
    }
    if snap.sample_interval_ms == 0 {
        violations.push(ValidationViolationV2::new(
            ViolationKindV2::ZeroNotAllowed,
            "sample_interval_ms",
        ));
    } else if snap.sample_interval_ms > MAX_SAMPLE_INTERVAL_MS {
        violations.push(ValidationViolationV2::new(
            ViolationKindV2::SampleIntervalOutOfRange {
                max_ms: MAX_SAMPLE_INTERVAL_MS,
            },
            "sample_interval_ms",
        ));
    }

    validate_identity_v2(&snap.system, &mut violations);
    validate_cpu_v2(&snap.cpu, snap.capabilities.cpu_iowait, &mut violations);
    validate_load_v2(
        snap.load.as_ref(),
        snap.capabilities.load_average,
        &mut violations,
    );
    validate_memory_v2(&snap.memory, &mut violations);
    validate_swap_v2(snap.swap.as_ref(), snap.capabilities.swap, &mut violations);
    validate_commit_v2(
        snap.commit.as_ref(),
        snap.capabilities.memory_commit,
        &mut violations,
    );

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Validate a flat v2 status payload, including its optional drive data.
pub fn validate_payload_v2(payload: &StatusPayloadV2) -> Result<(), Vec<ValidationViolationV2>> {
    let mut violations = match validate_v2(&payload.snapshot) {
        Ok(()) => Vec::new(),
        Err(violations) => violations,
    };

    if let Some(drives) = &payload.drives {
        if drives.len() > MAX_DRIVE_ENTRIES {
            violations.push(ValidationViolationV2::new(
                ViolationKindV2::TooManyDrives {
                    max_entries: MAX_DRIVE_ENTRIES,
                },
                "drives",
            ));
        }
        // Validate every entry even when the payload exceeds the protocol
        // bound: `TooManyDrives` rejects the payload as a whole, while the
        // per-entry violations give diagnostics visibility into problems in
        // the excess entries.
        for (index, drive) in drives.iter().enumerate() {
            let prefix = format!("drives[{index}]");
            // Drive names are deliberately exempt from the NUL rejection
            // applied to identity fields: that guard targets the Windows
            // hostname NUL-padding regression class, while drive names are
            // display-only labels bounded by `MAX_DRIVE_NAME_BYTES`.
            if drive.name.is_empty() {
                violations.push(ValidationViolationV2::new(
                    ViolationKindV2::EmptyDriveName,
                    format!("{prefix}.name"),
                ));
            }
            if drive.name.len() > MAX_DRIVE_NAME_BYTES {
                violations.push(ValidationViolationV2::new(
                    ViolationKindV2::DriveNameTooLong {
                        max_bytes: MAX_DRIVE_NAME_BYTES,
                    },
                    format!("{prefix}.name"),
                ));
            }
            if drive.total_bytes == 0 {
                violations.push(ValidationViolationV2::new(
                    ViolationKindV2::ZeroNotAllowed,
                    format!("{prefix}.total_bytes"),
                ));
            }
            if drive.used_bytes > drive.total_bytes {
                violations.push(ValidationViolationV2::new(
                    ViolationKindV2::UsedExceedsTotal,
                    format!("{prefix}.used_bytes"),
                ));
            }
            if drive
                .available_bytes
                .is_some_and(|available| available > drive.total_bytes)
            {
                violations.push(ValidationViolationV2::new(
                    ViolationKindV2::AvailableExceedsTotal,
                    format!("{prefix}.available_bytes"),
                ));
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn validate_identity_v2(system: &SystemIdentity, out: &mut Vec<ValidationViolationV2>) {
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
            out.push(ValidationViolationV2::new(
                ViolationKindV2::InvalidIdentityField,
                field,
            ));
        }
    }
}

fn validate_cpu_v2(
    cpu: &crate::v2::CpuMetricsV2,
    cpu_iowait: bool,
    out: &mut Vec<ValidationViolationV2>,
) {
    if cpu.logical_cores == 0 {
        out.push(ValidationViolationV2::new(
            ViolationKindV2::ZeroNotAllowed,
            "cpu.logical_cores",
        ));
    }
    check_percentage_v2(cpu.usage_pct, "cpu.usage_pct", out);
    match cpu.iowait_pct {
        None => {
            if cpu_iowait {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::IowaitCapabilityMismatch,
                    "cpu.iowait_pct",
                ));
            }
        }
        Some(value) => {
            check_percentage_v2(value, "cpu.iowait_pct", out);
            if !cpu_iowait {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::IowaitCapabilityMismatch,
                    "cpu.iowait_pct",
                ));
            }
        }
    }
}

fn validate_load_v2(
    load: Option<&LoadAverage>,
    load_average_capable: bool,
    out: &mut Vec<ValidationViolationV2>,
) {
    match load {
        None => {
            if load_average_capable {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::LoadCapabilityMismatch,
                    "load",
                ));
            }
        }
        Some(l) => {
            check_load_v2(l.one, "load.one", out);
            check_load_v2(l.five, "load.five", out);
            check_load_v2(l.fifteen, "load.fifteen", out);
            if !load_average_capable {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::LoadCapabilityMismatch,
                    "load",
                ));
            }
        }
    }
}

fn check_load_v2(value: f32, field: &str, out: &mut Vec<ValidationViolationV2>) {
    if !value.is_finite() || value < 0.0 {
        out.push(ValidationViolationV2::new(
            ViolationKindV2::LoadValueOutOfRange,
            field,
        ));
    }
}

fn validate_memory_v2(memory: &MemoryMetrics, out: &mut Vec<ValidationViolationV2>) {
    let before = out.len();
    check_percentage_v2(memory.usage_pct, "memory.usage_pct", out);
    let pct_flagged = out.len() > before;
    if memory.used_bytes > memory.total_bytes {
        out.push(ValidationViolationV2::new(
            ViolationKindV2::UsedExceedsTotal,
            "memory.used_bytes",
        ));
    }
    if memory.total_bytes == 0 && (memory.used_bytes > 0 || memory.usage_pct != 0.0) {
        out.push(ValidationViolationV2::new(
            ViolationKindV2::ZeroNotAllowed,
            "memory.total_bytes",
        ));
        if !pct_flagged && memory.usage_pct != 0.0 {
            out.push(ValidationViolationV2::new(
                ViolationKindV2::PercentageOutOfRange,
                "memory.usage_pct",
            ));
        }
    }
}

fn validate_swap_v2(
    swap: Option<&SwapMetrics>,
    swap_capable: bool,
    out: &mut Vec<ValidationViolationV2>,
) {
    match swap {
        None => {
            if swap_capable {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::SwapCapabilityMismatch,
                    "swap",
                ));
            }
        }
        Some(s) => {
            let before = out.len();
            check_percentage_v2(s.usage_pct, "swap.usage_pct", out);
            let pct_flagged = out.len() > before;
            if s.used_bytes > s.total_bytes {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::UsedExceedsTotal,
                    "swap.used_bytes",
                ));
            }
            if s.total_bytes == 0 && (s.used_bytes > 0 || s.usage_pct != 0.0) {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::ZeroNotAllowed,
                    "swap.total_bytes",
                ));
                if !pct_flagged && s.usage_pct != 0.0 {
                    out.push(ValidationViolationV2::new(
                        ViolationKindV2::PercentageOutOfRange,
                        "swap.usage_pct",
                    ));
                }
            }
            if !swap_capable {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::SwapCapabilityMismatch,
                    "swap",
                ));
            }
        }
    }
}

fn validate_commit_v2(
    commit: Option<&CommitMetrics>,
    commit_capable: bool,
    out: &mut Vec<ValidationViolationV2>,
) {
    match commit {
        None => {
            if commit_capable {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::CommitCapabilityMismatch,
                    "commit",
                ));
            }
        }
        Some(c) => {
            let before = out.len();
            check_percentage_v2(c.usage_pct, "commit.usage_pct", out);
            let pct_flagged = out.len() > before;
            if c.used_bytes > c.limit_bytes {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::UsedExceedsTotal,
                    "commit.used_bytes",
                ));
            }
            if c.limit_bytes == 0 && (c.used_bytes > 0 || c.usage_pct != 0.0) {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::ZeroNotAllowed,
                    "commit.limit_bytes",
                ));
                if !pct_flagged && c.usage_pct != 0.0 {
                    out.push(ValidationViolationV2::new(
                        ViolationKindV2::PercentageOutOfRange,
                        "commit.usage_pct",
                    ));
                }
            }
            if !commit_capable {
                out.push(ValidationViolationV2::new(
                    ViolationKindV2::CommitCapabilityMismatch,
                    "commit",
                ));
            }
        }
    }
}

fn check_percentage_v2(value: f32, field: &str, out: &mut Vec<ValidationViolationV2>) {
    if !value.is_finite() {
        out.push(ValidationViolationV2::new(
            ViolationKindV2::PercentageNotFinite,
            field,
        ));
        return;
    }
    if !(0.0..=100.0).contains(&value) {
        out.push(ValidationViolationV2::new(
            ViolationKindV2::PercentageOutOfRange,
            field,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{
        CommitMetrics, CpuMetricsV2, DriveMetrics, MetricCapabilitiesV2, StatusPayloadV2,
        StatusSnapshotV2, SwapMetrics, MAX_DRIVE_ENTRIES, MAX_DRIVE_NAME_BYTES, SCHEMA_VERSION_V2,
    };
    use crate::{LoadAverage, MemoryMetrics, SystemIdentity};

    fn v2_identity() -> SystemIdentity {
        SystemIdentity {
            name: "test".into(),
            hostname: "test.local".into(),
            os_name: "linux".into(),
            os_version: "1.0".into(),
            kernel_name: "Linux".into(),
            kernel_release: "6.0.0".into(),
            architecture: "x86_64".into(),
        }
    }

    fn valid_linux_v2() -> StatusSnapshotV2 {
        StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilitiesV2 {
                cpu_iowait: true,
                load_average: true,
                swap: true,
                memory_commit: false,
            },
            system: v2_identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 8,
                usage_pct: 25.2,
                iowait_pct: Some(0.4),
            },
            load: Some(LoadAverage {
                one: 1.32,
                five: 0.91,
                fifteen: 0.62,
            }),
            memory: MemoryMetrics {
                used_bytes: 5_900_000_000,
                total_bytes: 15_600_000_000,
                usage_pct: 37.8,
            },
            swap: Some(SwapMetrics {
                used_bytes: 0,
                total_bytes: 4_000_000_000,
                usage_pct: 0.0,
            }),
            commit: None,
        }
    }

    fn valid_payload(drives: Option<Vec<DriveMetrics>>) -> StatusPayloadV2 {
        StatusPayloadV2 {
            snapshot: valid_linux_v2(),
            drives,
        }
    }

    #[test]
    fn valid_drive_payloads_include_unavailable_empty_and_populated_states() {
        assert!(valid_payload(None).validate().is_ok());
        assert!(valid_payload(Some(Vec::new())).validate().is_ok());
        assert!(valid_payload(Some(vec![DriveMetrics {
            name: "C:\\".into(),
            used_bytes: 1,
            total_bytes: 2,
            available_bytes: None,
        }]))
        .validate()
        .is_ok());
    }

    #[test]
    fn drive_validation_reports_indexed_fields_and_bounds() {
        let payload = valid_payload(Some(vec![DriveMetrics {
            name: String::new(),
            used_bytes: 3,
            total_bytes: 2,
            available_bytes: None,
        }]));
        let err = payload.validate().unwrap_err();
        assert!(err.iter().any(|v| v.field == "drives[0].name"));
        assert!(err.iter().any(|v| v.field == "drives[0].used_bytes"));

        let too_long = valid_payload(Some(vec![DriveMetrics {
            name: "x".repeat(MAX_DRIVE_NAME_BYTES + 1),
            used_bytes: 0,
            total_bytes: 1,
            available_bytes: None,
        }]));
        assert!(too_long
            .validate()
            .unwrap_err()
            .iter()
            .any(|v| v.field == "drives[0].name"));

        let too_many = valid_payload(Some(
            (0..=MAX_DRIVE_ENTRIES)
                .map(|index| DriveMetrics {
                    name: format!("/{index}"),
                    used_bytes: 0,
                    total_bytes: 1,
                    available_bytes: None,
                })
                .collect(),
        ));
        assert!(too_many
            .validate()
            .unwrap_err()
            .iter()
            .any(|v| v.field == "drives"));
    }

    #[test]
    fn drive_boundaries_at_the_exact_limits_are_valid() {
        // Name length exactly at the bound.
        let max_name = valid_payload(Some(vec![DriveMetrics {
            name: "x".repeat(MAX_DRIVE_NAME_BYTES),
            used_bytes: 0,
            total_bytes: 1,
            available_bytes: None,
        }]));
        max_name.validate().expect("512-byte name is valid");

        // Exactly the maximum number of drives.
        let max_entries = valid_payload(Some(
            (0..MAX_DRIVE_ENTRIES)
                .map(|index| DriveMetrics {
                    name: format!("/{index}"),
                    used_bytes: 0,
                    total_bytes: 1,
                    available_bytes: None,
                })
                .collect(),
        ));
        max_entries.validate().expect("32 drives are valid");

        // `available_bytes` exactly equal to `total_bytes`.
        let available_eq_total = valid_payload(Some(vec![DriveMetrics {
            name: "/".into(),
            used_bytes: 0,
            total_bytes: 10,
            available_bytes: Some(10),
        }]));
        available_eq_total
            .validate()
            .expect("available == total is valid");
    }

    #[test]
    fn excess_drives_beyond_the_limit_are_still_individually_validated() {
        // The last entry (index 32, one past the bound) has an empty name and
        // inverted byte counts. Both must surface as indexed violations even
        // though `TooManyDrives` already rejects the payload.
        let mut drives: Vec<DriveMetrics> = (0..MAX_DRIVE_ENTRIES)
            .map(|index| DriveMetrics {
                name: format!("/{index}"),
                used_bytes: 0,
                total_bytes: 1,
                available_bytes: None,
            })
            .collect();
        drives.push(DriveMetrics {
            name: String::new(),
            used_bytes: 5,
            total_bytes: 1,
            available_bytes: None,
        });
        let err = valid_payload(Some(drives)).validate().unwrap_err();
        assert!(err.iter().any(
            |v| v.field == "drives" && matches!(v.kind, ViolationKindV2::TooManyDrives { .. })
        ));
        assert!(err
            .iter()
            .any(|v| v.field == format!("drives[{MAX_DRIVE_ENTRIES}].name")
                && v.kind == ViolationKindV2::EmptyDriveName));
        assert!(err.iter().any(
            |v| v.field == format!("drives[{MAX_DRIVE_ENTRIES}].used_bytes")
                && v.kind == ViolationKindV2::UsedExceedsTotal
        ));
    }

    #[test]
    fn drive_names_accept_unicode_and_windows_roots() {
        let payload = valid_payload(Some(vec![
            DriveMetrics {
                name: "データ /home".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            },
            DriveMetrics {
                name: "C:\\".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            },
        ]));
        payload.validate().unwrap();
    }

    #[test]
    fn explicit_availability_is_optional_and_bounded_independently() {
        let mut payload = valid_payload(Some(vec![DriveMetrics {
            name: "/".into(),
            used_bytes: 8,
            total_bytes: 10,
            available_bytes: Some(1),
        }]));
        payload.validate().unwrap();
        payload.drives.as_mut().unwrap()[0].available_bytes = Some(11);
        let error = payload.validate().unwrap_err();
        assert!(error.iter().any(|violation| {
            violation.kind == ViolationKindV2::AvailableExceedsTotal
                && violation.field == "drives[0].available_bytes"
        }));
    }

    #[test]
    fn valid_linux_v2_passes() {
        let snap = valid_linux_v2();
        validate_v2(&snap).expect("linux v2 validates");
    }

    #[test]
    fn valid_windows_v2_passes() {
        let snap = StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilitiesV2 {
                cpu_iowait: false,
                load_average: false,
                swap: false,
                memory_commit: true,
            },
            system: v2_identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 4,
                usage_pct: 12.5,
                iowait_pct: None,
            },
            load: None,
            memory: MemoryMetrics {
                used_bytes: 2_000_000_000,
                total_bytes: 8_000_000_000,
                usage_pct: 25.0,
            },
            swap: None,
            commit: Some(crate::v2::CommitMetrics {
                used_bytes: 3_000_000_000,
                limit_bytes: 8_000_000_000,
                usage_pct: 37.5,
            }),
        };
        validate_v2(&snap).expect("windows v2 validates");
    }

    #[test]
    fn valid_macos_v2_passes() {
        let snap = StatusSnapshotV2 {
            schema_version: SCHEMA_VERSION_V2,
            observed_at_unix_ms: 1,
            sample_interval_ms: 1000,
            capabilities: MetricCapabilitiesV2 {
                cpu_iowait: false,
                load_average: true,
                swap: false,
                memory_commit: false,
            },
            system: v2_identity(),
            cpu: CpuMetricsV2 {
                logical_cores: 8,
                usage_pct: 18.7,
                iowait_pct: None,
            },
            load: Some(LoadAverage {
                one: 2.10,
                five: 1.85,
                fifteen: 1.40,
            }),
            memory: MemoryMetrics {
                used_bytes: 9_000_000_000,
                total_bytes: 16_000_000_000,
                usage_pct: 56.25,
            },
            swap: None,
            commit: None,
        };
        validate_v2(&snap).expect("macos v2 validates");
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let mut snap = valid_linux_v2();
        snap.schema_version = 1;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| matches!(
            v.kind,
            ViolationKindV2::UnsupportedSchemaVersion { found: 1 }
        )));
    }

    #[test]
    fn rejects_zero_observed_at() {
        let mut snap = valid_linux_v2();
        snap.observed_at_unix_ms = 0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "observed_at_unix_ms"));
    }

    #[test]
    fn rejects_zero_sample_interval() {
        let mut snap = valid_linux_v2();
        snap.sample_interval_ms = 0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "sample_interval_ms"));
    }

    #[test]
    fn rejects_sample_interval_above_protocol_limit() {
        let mut snap = valid_linux_v2();
        snap.sample_interval_ms = MAX_SAMPLE_INTERVAL_MS + 1;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "sample_interval_ms"
                && matches!(
                    violation.kind,
                    ViolationKindV2::SampleIntervalOutOfRange { .. }
                )
        }));
    }

    #[test]
    fn rejects_invalid_load_value_with_load_specific_violation() {
        let mut snap = valid_linux_v2();
        snap.load.as_mut().unwrap().one = -1.0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "load.one" && violation.kind == ViolationKindV2::LoadValueOutOfRange
        }));
    }

    #[test]
    fn rejects_nonzero_memory_usage_with_zero_total() {
        let mut snap = valid_linux_v2();
        snap.memory.total_bytes = 0;
        snap.memory.usage_pct = 1.0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "memory.total_bytes"
                && violation.kind == ViolationKindV2::ZeroNotAllowed
        }));
        assert!(err.iter().any(|violation| {
            violation.field == "memory.usage_pct"
                && violation.kind == ViolationKindV2::PercentageOutOfRange
        }));
    }

    #[test]
    fn rejects_zero_commit_limit_and_nonzero_usage() {
        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            commit: Some(crate::v2::CommitMetrics {
                used_bytes: 0,
                limit_bytes: 0,
                usage_pct: 1.0,
            }),
            ..valid_linux_v2()
        };
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "commit.limit_bytes"
                && violation.kind == ViolationKindV2::ZeroNotAllowed
        }));
        assert!(err.iter().any(|violation| {
            violation.field == "commit.usage_pct"
                && violation.kind == ViolationKindV2::PercentageOutOfRange
        }));
    }

    #[test]
    fn zero_total_fields_do_not_duplicate_percentage_violations() {
        for swap_capable in [true, false] {
            let mut snap = valid_linux_v2();
            snap.capabilities.swap = swap_capable;
            snap.memory.total_bytes = 0;
            snap.memory.usage_pct = 150.0;
            if let Some(swap) = snap.swap.as_mut() {
                swap.total_bytes = 0;
                swap.usage_pct = 150.0;
            }
            let err = validate_v2(&snap).unwrap_err();
            for field in ["memory.usage_pct", "swap.usage_pct"] {
                let count = err
                    .iter()
                    .filter(|v| v.field == field && v.kind == ViolationKindV2::PercentageOutOfRange)
                    .count();
                assert_eq!(count, 1, "duplicate PercentageOutOfRange for {field}");
            }
        }
    }

    #[test]
    fn zero_commit_limit_does_not_duplicate_percentage_violation() {
        for commit_capable in [true, false] {
            let snap = StatusSnapshotV2 {
                capabilities: MetricCapabilitiesV2 {
                    memory_commit: commit_capable,
                    ..valid_linux_v2().capabilities
                },
                commit: Some(crate::v2::CommitMetrics {
                    used_bytes: 0,
                    limit_bytes: 0,
                    usage_pct: 150.0,
                }),
                ..valid_linux_v2()
            };
            let err = validate_v2(&snap).unwrap_err();
            let count = err
                .iter()
                .filter(|v| {
                    v.field == "commit.usage_pct" && v.kind == ViolationKindV2::PercentageOutOfRange
                })
                .count();
            assert_eq!(count, 1);
        }
    }

    #[test]
    fn rejects_zero_logical_cores() {
        let mut snap = valid_linux_v2();
        snap.cpu.logical_cores = 0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "cpu.logical_cores"));
    }

    #[test]
    fn rejects_whitespace_only_identity_fields() {
        let mut snap = valid_linux_v2();
        snap.system.name = "   ".into();
        snap.system.hostname = "\t\n".into();
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "system.name"));
        assert!(err.iter().any(|v| v.field == "system.hostname"));
    }

    #[test]
    fn rejects_nan_cpu_usage() {
        let mut snap = valid_linux_v2();
        snap.cpu.usage_pct = f32::NAN;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "cpu.usage_pct"));
    }

    #[test]
    fn zero_total_with_nonzero_used_reports_zero_not_allowed() {
        let mut snap = valid_linux_v2();
        snap.memory.total_bytes = 0;
        snap.memory.used_bytes = 5;
        snap.memory.usage_pct = 0.0;
        if let Some(swap) = snap.swap.as_mut() {
            swap.total_bytes = 0;
            swap.used_bytes = 5;
            swap.usage_pct = 0.0;
        }
        let err = validate_v2(&snap).unwrap_err();
        for field in ["memory.total_bytes", "swap.total_bytes"] {
            assert!(
                err.iter()
                    .any(|v| v.field == field && v.kind == ViolationKindV2::ZeroNotAllowed),
                "missing ZeroNotAllowed for {field}"
            );
        }
    }

    #[test]
    fn zero_commit_limit_with_nonzero_used_reports_zero_not_allowed() {
        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            commit: Some(CommitMetrics {
                used_bytes: 5,
                limit_bytes: 0,
                usage_pct: 0.0,
            }),
            ..valid_linux_v2()
        };
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "commit.limit_bytes" && v.kind == ViolationKindV2::ZeroNotAllowed));
    }

    #[test]
    fn rejects_iowait_none_when_capability_true() {
        let mut snap = valid_linux_v2();
        snap.cpu.iowait_pct = None;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::IowaitCapabilityMismatch)));
    }

    #[test]
    fn rejects_iowait_some_when_capability_false() {
        let mut snap = valid_linux_v2();
        snap.capabilities.cpu_iowait = false;
        snap.cpu.iowait_pct = Some(f32::NAN);
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::IowaitCapabilityMismatch)));
        assert!(err.iter().any(|v| {
            v.field == "cpu.iowait_pct" && v.kind == ViolationKindV2::PercentageNotFinite
        }));
    }

    #[test]
    fn rejects_load_none_when_capability_true() {
        let mut snap = valid_linux_v2();
        snap.load = None;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::LoadCapabilityMismatch)));
    }

    #[test]
    fn rejects_load_some_when_capability_false() {
        let mut snap = valid_linux_v2();
        snap.capabilities.load_average = false;
        snap.load.as_mut().unwrap().one = f32::NAN;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::LoadCapabilityMismatch)));
        assert!(err
            .iter()
            .any(|v| v.field == "load.one" && v.kind == ViolationKindV2::LoadValueOutOfRange));
    }

    #[test]
    fn rejects_swap_none_when_capability_true() {
        let mut snap = valid_linux_v2();
        snap.swap = None;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::SwapCapabilityMismatch)));
    }

    #[test]
    fn rejects_swap_some_when_capability_false() {
        let mut snap = valid_linux_v2();
        snap.capabilities.swap = false;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::SwapCapabilityMismatch)));
    }

    #[test]
    fn accepts_zero_commit_metrics_when_capable() {
        // The Windows collector treats a zero commit limit as a legitimate
        // runtime state and reports all-zero commit metrics for it.
        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            commit: Some(CommitMetrics {
                used_bytes: 0,
                limit_bytes: 0,
                usage_pct: 0.0,
            }),
            ..valid_linux_v2()
        };
        assert!(validate_v2(&snap).is_ok());
    }

    #[test]
    fn rejects_commit_zero_limit_with_nonzero_percentage() {
        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            commit: Some(CommitMetrics {
                used_bytes: 0,
                limit_bytes: 0,
                usage_pct: 25.0,
            }),
            ..valid_linux_v2()
        };
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "commit.limit_bytes" && v.kind == ViolationKindV2::ZeroNotAllowed));
        assert!(err
            .iter()
            .any(|v| v.field == "commit.usage_pct"
                && v.kind == ViolationKindV2::PercentageOutOfRange));
    }

    #[test]
    fn capability_mismatch_with_consistent_zeros_reports_only_the_mismatch() {
        let mut snap = valid_linux_v2();
        snap.capabilities.swap = false;
        snap.swap = Some(SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        });
        let err = validate_v2(&snap).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].kind, ViolationKindV2::SwapCapabilityMismatch);

        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: false,
                ..valid_linux_v2().capabilities
            },
            commit: Some(CommitMetrics {
                used_bytes: 0,
                limit_bytes: 0,
                usage_pct: 0.0,
            }),
            ..valid_linux_v2()
        };
        let err = validate_v2(&snap).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].kind, ViolationKindV2::CommitCapabilityMismatch);
    }

    #[test]
    fn rejects_identity_fields_that_are_empty_or_nul_padded() {
        let mut snap = valid_linux_v2();
        snap.system.name = String::new();
        snap.system.hostname = "host\0".into();
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| v.field == "system.name" && v.kind == ViolationKindV2::InvalidIdentityField));
        assert!(err.iter().any(
            |v| v.field == "system.hostname" && v.kind == ViolationKindV2::InvalidIdentityField
        ));
    }

    #[test]
    fn rejects_identity_fields_that_are_too_long() {
        let mut snap = valid_linux_v2();
        snap.system.hostname = "x".repeat(MAX_IDENTITY_FIELD_BYTES + 1);
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| {
            v.field == "system.hostname" && v.kind == ViolationKindV2::InvalidIdentityField
        }));
    }

    #[test]
    fn rejects_commit_none_when_capability_true() {
        let mut snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            ..valid_linux_v2()
        };
        snap.commit = None;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::CommitCapabilityMismatch)));
    }

    #[test]
    fn rejects_commit_some_when_capability_false() {
        let mut snap = valid_linux_v2();
        snap.commit = Some(crate::v2::CommitMetrics {
            used_bytes: 1_000_000_000,
            limit_bytes: 4_000_000_000,
            usage_pct: 25.0,
        });
        let err = validate_v2(&snap).unwrap_err();
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::CommitCapabilityMismatch)));
    }

    #[test]
    fn rejects_used_exceeds_limit_in_commit() {
        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            commit: Some(crate::v2::CommitMetrics {
                used_bytes: 9_000_000_000,
                limit_bytes: 4_000_000_000,
                usage_pct: 25.0,
            }),
            ..valid_linux_v2()
        };
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "commit.used_bytes"));
    }

    #[test]
    fn rejects_used_exceeds_total_in_swap() {
        let mut snap = valid_linux_v2();
        snap.swap = Some(SwapMetrics {
            used_bytes: 5_000_000_000,
            total_bytes: 4_000_000_000,
            usage_pct: 25.0,
        });
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "swap.used_bytes"));
    }

    #[test]
    fn rejects_used_exceeds_total_in_memory() {
        let mut snap = valid_linux_v2();
        snap.memory.used_bytes = 20_000_000_000;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "memory.used_bytes"));
    }

    #[test]
    fn rejects_percentage_over_100() {
        let mut snap = valid_linux_v2();
        snap.cpu.usage_pct = 101.0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "cpu.usage_pct"));
    }

    #[test]
    fn rejects_infinite_percentage() {
        let mut snap = valid_linux_v2();
        snap.cpu.usage_pct = f32::INFINITY;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "cpu.usage_pct"));
    }

    #[test]
    fn multiple_violations_all_reported() {
        let mut snap = valid_linux_v2();
        snap.schema_version = 99;
        snap.observed_at_unix_ms = 0;
        snap.cpu.logical_cores = 0;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.len() >= 3);
    }

    #[test]
    fn rejects_zero_total_drive_bytes() {
        let payload = valid_payload(Some(vec![DriveMetrics {
            name: "/".into(),
            used_bytes: 0,
            total_bytes: 0,
            available_bytes: None,
        }]));
        let err = payload.validate().unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "drives[0].total_bytes"
                && violation.kind == ViolationKindV2::ZeroNotAllowed
        }));
    }

    #[test]
    fn rejects_negative_memory_swap_and_commit_percentages() {
        let mut snap = valid_linux_v2();
        snap.memory.usage_pct = -0.1;
        snap.swap.as_mut().unwrap().usage_pct = -0.1;
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "memory.usage_pct"
                && violation.kind == ViolationKindV2::PercentageOutOfRange
        }));
        assert!(err.iter().any(|violation| {
            violation.field == "swap.usage_pct"
                && violation.kind == ViolationKindV2::PercentageOutOfRange
        }));

        let snap = StatusSnapshotV2 {
            capabilities: MetricCapabilitiesV2 {
                memory_commit: true,
                ..valid_linux_v2().capabilities
            },
            commit: Some(CommitMetrics {
                used_bytes: 0,
                limit_bytes: 4_000_000_000,
                usage_pct: -0.1,
            }),
            ..valid_linux_v2()
        };
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "commit.usage_pct"
                && violation.kind == ViolationKindV2::PercentageOutOfRange
        }));
    }

    #[test]
    fn rejects_negative_iowait() {
        let mut snap = valid_linux_v2();
        snap.cpu.iowait_pct = Some(-0.5);
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|v| v.field == "cpu.iowait_pct"));

        snap.capabilities.cpu_iowait = false;
        snap.cpu.iowait_pct = Some(-0.5);
        let err = validate_v2(&snap).unwrap_err();
        assert!(err.iter().any(|violation| {
            violation.field == "cpu.iowait_pct"
                && violation.kind == ViolationKindV2::PercentageOutOfRange
        }));
        assert!(err
            .iter()
            .any(|v| matches!(v.kind, ViolationKindV2::IowaitCapabilityMismatch)));
    }
}
