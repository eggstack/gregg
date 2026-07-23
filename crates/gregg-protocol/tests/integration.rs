use std::fs;
use std::path::{Path, PathBuf};

use gregg_protocol::{
    CpuMetrics, HealthResponse, LoadAverage, MemoryMetrics, MetricCapabilities, ReadinessState,
    StatusSnapshot, SwapMetrics, SystemIdentity, ValidationViolation, ViolationKind,
    SCHEMA_VERSION_V1,
};

fn fixture(rel: &str) -> Vec<u8> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push(rel);
    fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

fn parse_snapshot(bytes: &[u8]) -> StatusSnapshot {
    serde_json::from_slice(bytes).expect("snapshot fixture parses")
}

#[test]
fn linux_fixture_round_trips() {
    let bytes = fixture("linux-v1.json");
    let snap = parse_snapshot(&bytes);
    snap.validate().expect("linux fixture validates");

    let encoded = serde_json::to_vec(&snap).expect("serialize snapshot");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is JSON");
    let encoded_value: serde_json::Value =
        serde_json::from_slice(&encoded).expect("encoded is JSON");
    assert_eq!(value, encoded_value, "round-trip must be byte-stable");
}

#[test]
fn macos_fixture_marks_iowait_unsupported() {
    let bytes = fixture("macos-v1.json");
    let snap = parse_snapshot(&bytes);
    snap.validate().expect("macos fixture validates");

    let caps = snap.capabilities;
    let iowait = snap.cpu.iowait_pct;
    assert!(!caps.cpu_iowait, "macOS capability must be false");
    assert!(iowait.is_none(), "macOS iowait must be null");
}

#[test]
fn health_ready_fixture_round_trips() {
    let bytes = fixture("health-ready-v1.json");
    let health: HealthResponse = serde_json::from_slice(&bytes).expect("ready health parses");
    assert_eq!(health.state, ReadinessState::Ready);
    assert!(health.snapshot.is_some());
    health
        .snapshot
        .as_ref()
        .expect("snapshot")
        .validate()
        .unwrap();

    let encoded = serde_json::to_vec(&health).expect("serialize ready health");
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let emitted: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(original, emitted);
}

#[test]
fn health_warming_fixture_round_trips() {
    let bytes = fixture("health-warming-v1.json");
    let health: HealthResponse = serde_json::from_slice(&bytes).expect("warming health parses");
    assert_eq!(health.state, ReadinessState::Warming);
    assert!(health.snapshot.is_none());

    let encoded = serde_json::to_vec(&health).expect("serialize warming");
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let emitted: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(original, emitted);
}

#[test]
fn health_collector_failure_fixture_round_trips() {
    let bytes = fixture("health-collector-failure-v1.json");
    let health: HealthResponse = serde_json::from_slice(&bytes).expect("failure health parses");
    assert_eq!(health.state, ReadinessState::Failed);
    assert!(health.snapshot.is_none());

    let encoded = serde_json::to_vec(&health).expect("serialize failure");
    let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let emitted: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(original, emitted);
}

#[test]
fn rejects_unsupported_schema_version() {
    let snap = StatusSnapshot {
        schema_version: 2,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: true },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: Some(0.0),
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::UnsupportedSchemaVersion { found: 2 })));
}

#[test]
fn rejects_zero_logical_cores() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 0,
            usage_pct: 0.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err.iter().any(|v| v.field == "cpu.logical_cores"));
}

#[test]
fn rejects_nan_percentage() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: f32::NAN,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err.iter().any(
        |v| matches!(v.kind, ViolationKind::PercentageNotFinite) && v.field == "cpu.usage_pct"
    ));
}

#[test]
fn rejects_infinite_percentage() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: f32::INFINITY,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::PercentageNotFinite)));
}

#[test]
fn rejects_negative_percentage() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: -0.1,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::PercentageOutOfRange)));
}

#[test]
fn rejects_percentage_above_one_hundred() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 100.0001,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::PercentageOutOfRange)));
}

#[test]
fn rejects_used_greater_than_total_memory() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 100,
            total_bytes: 50,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err.iter().any(
        |v| matches!(v.kind, ViolationKind::UsedExceedsTotal) && v.field == "memory.used_bytes"
    ));
}

#[test]
fn zero_swap_with_zero_percentage_is_valid() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    snap.validate()
        .expect("zero swap with zero percentage is valid");
}

#[test]
fn zero_swap_with_nan_percentage_is_rejected() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: f32::NAN,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err.iter().any(|v| v.field == "swap.usage_pct"));
}

#[test]
fn rejects_iowait_none_when_capability_true() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: true },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::IowaitCapabilityMismatch)));
}

#[test]
fn rejects_iowait_some_when_capability_false() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: Some(0.5),
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::IowaitCapabilityMismatch)));
}

#[test]
fn unknown_additive_fields_are_ignored() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": true, "future_capability": true },
        "system": {
            "name": "x", "hostname": "h", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": 0.0 },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 },
        "future_extension": { "nested": [1, 2, 3] }
    }"#;
    let snap: StatusSnapshot = serde_json::from_str(json).expect("unknown fields ignored");
    snap.validate()
        .expect("snapshot with additive unknowns still validates");
}

fn identity(name: &str) -> SystemIdentity {
    SystemIdentity {
        name: name.into(),
        hostname: format!("{name}.local"),
        os_name: "linux".into(),
        os_version: "1".into(),
        kernel_name: "Linux".into(),
        kernel_release: "1.0".into(),
        architecture: "x86_64".into(),
    }
}

#[test]
fn violation_messages_mention_field_and_reason() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 0,
        sample_interval_ms: 0,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 0,
            usage_pct: f32::NAN,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 5,
            total_bytes: 1,
            usage_pct: 100.5,
        },
        swap: SwapMetrics {
            used_bytes: 1,
            total_bytes: 0,
            usage_pct: 5.0,
        },
    };
    let err = snap.validate().unwrap_err();
    let _: Vec<ValidationViolation> = err;
    let joined = err
        .iter()
        .map(|v| format!("{}: {}", v.field, v.kind))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("observed_at_unix_ms"));
    assert!(joined.contains("cpu.logical_cores"));
    assert!(joined.contains("cpu.usage_pct"));
    assert!(joined.contains("memory.used_bytes"));
    assert!(joined.contains("memory.usage_pct"));
    assert!(joined.contains("swap.used_bytes"));
}

#[test]
fn health_response_accessors() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 0.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 0,
            total_bytes: 1,
            usage_pct: 0.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    let ready = HealthResponse::ready(snap.clone());
    assert_eq!(ready.state, ReadinessState::Ready);
    assert!(ready.snapshot.is_some());

    let warming = HealthResponse::warming();
    assert_eq!(warming.state, ReadinessState::Warming);
    assert!(warming.snapshot.is_none());

    let failed = HealthResponse::failed(gregg_protocol::HealthCategory::CollectorFailure, "boom");
    assert_eq!(failed.state, ReadinessState::Failed);
    assert_eq!(failed.message.as_deref(), Some("boom"));
}

#[test]
fn fixture_paths_exist() {
    for name in [
        "linux-v1.json",
        "macos-v1.json",
        "health-ready-v1.json",
        "health-warming-v1.json",
        "health-collector-failure-v1.json",
    ] {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests/fixtures");
        path.push(name);
        assert!(
            Path::new(&path).exists(),
            "missing fixture {}",
            path.display()
        );
    }
}
