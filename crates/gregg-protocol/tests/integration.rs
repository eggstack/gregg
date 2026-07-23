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

#[test]
fn extremely_large_u64_memory_parses_and_validates() {
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
            used_bytes: u64::MAX,
            total_bytes: u64::MAX,
            usage_pct: 100.0,
        },
        swap: SwapMetrics {
            used_bytes: 0,
            total_bytes: 0,
            usage_pct: 0.0,
        },
    };
    snap.validate()
        .expect("u64::MAX used==total with 100.0% validates");
}

#[test]
fn extremely_large_memory_json_round_trips() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": false },
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 18446744073709551615, "total_bytes": 18446744073709551615, "usage_pct": 100.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let snap: StatusSnapshot = serde_json::from_str(json).expect("large u64 parses");
    snap.validate()
        .expect("large u64 memory with used==total validates");
}

#[test]
fn missing_required_json_field_cpu_is_rejected() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": false },
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let result = serde_json::from_str::<StatusSnapshot>(json);
    assert!(result.is_err(), "missing cpu field must fail to parse");
}

#[test]
fn missing_required_json_field_load_is_rejected() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": false },
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let result = serde_json::from_str::<StatusSnapshot>(json);
    assert!(result.is_err(), "missing load field must fail to parse");
}

#[test]
fn missing_required_json_field_system_is_rejected() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": false },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let result = serde_json::from_str::<StatusSnapshot>(json);
    assert!(result.is_err(), "missing system field must fail to parse");
}

#[test]
fn missing_required_json_field_capabilities_is_rejected() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let result = serde_json::from_str::<StatusSnapshot>(json);
    assert!(
        result.is_err(),
        "missing capabilities field must fail to parse"
    );
}

#[test]
fn missing_required_json_field_schema_version_is_rejected() {
    let json = r#"{
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": false },
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let result = serde_json::from_str::<StatusSnapshot>(json);
    assert!(
        result.is_err(),
        "missing schema_version field must fail to parse"
    );
}

#[test]
fn truncated_json_body_is_rejected() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": false },
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_
    "#;
    let result = serde_json::from_str::<StatusSnapshot>(json);
    assert!(result.is_err(), "truncated JSON must fail to parse");
}

#[test]
fn truncated_json_mid_object_is_rejected() {
    let json = r#"{"schema_version":1,"observed_at_unix_ms":1,"sample_interval_ms":1000,"capabilities":{"cpu_iowait":false},"system":{"name":"h","hostname":"h.local","os_name":"linux","os_version":"1","kernel_name":"Linux","kernel_release":"1.0","architecture":"x86_64"},"cpu":{"logical_cores":1,"usage_pct":0.0,"iowait_pct":null},"load":{"one":0.0,"five":0.0,"fifteen":0.0},"memory":{"used_bytes":0,"total_bytes":1,"usage_pct":0.0},"swap":{"used_bytes":0,"total_bytes":0,"usage_pct":0.0}}"#;
    let result = serde_json::from_str::<StatusSnapshot>(&json[..json.len() - 20]);
    assert!(result.is_err(), "truncated JSON at end must fail to parse");
}

#[test]
fn empty_json_body_is_rejected() {
    let result = serde_json::from_str::<StatusSnapshot>("{}");
    assert!(result.is_err(), "empty JSON object must fail to parse");
}

#[test]
fn oversized_valid_json_parses() {
    let long_name = "x".repeat(10_000);
    let json = format!(
        r#"{{
            "schema_version": 1,
            "observed_at_unix_ms": 1,
            "sample_interval_ms": 1000,
            "capabilities": {{ "cpu_iowait": false }},
            "system": {{
                "name": "{long_name}", "hostname": "h.local", "os_name": "linux", "os_version": "1",
                "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
            }},
            "cpu": {{ "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null }},
            "load": {{ "one": 0.0, "five": 0.0, "fifteen": 0.0 }},
            "memory": {{ "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 }},
            "swap": {{ "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }}
        }}"#,
    );
    let snap: StatusSnapshot = serde_json::from_str(&json).expect("large but valid JSON parses");
    snap.validate()
        .expect("oversized valid snapshot with long name validates");
}

#[test]
fn health_response_with_unknown_additive_fields_is_ignored() {
    let json = r#"{
        "schema_version": 1,
        "state": "ready",
        "future_field": "hello",
        "another_future": 42,
        "nested_unknown": { "a": true },
        "snapshot": {
            "schema_version": 1,
            "observed_at_unix_ms": 1,
            "sample_interval_ms": 1000,
            "capabilities": { "cpu_iowait": false },
            "system": {
                "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
                "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
            },
            "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
            "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
            "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
            "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
        }
    }"#;
    let health: HealthResponse =
        serde_json::from_str(json).expect("unknown health fields ignored on parse");
    assert_eq!(health.state, ReadinessState::Ready);
    assert!(health.snapshot.is_some());
}

#[test]
fn version_skew_rejects_wrong_schema_version() {
    let snap = StatusSnapshot {
        schema_version: 0,
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
    let err = snap.validate().unwrap_err();
    assert!(err
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::UnsupportedSchemaVersion { found: 0 })));
}

#[test]
fn multiple_simultaneous_violations_all_reported() {
    let snap = StatusSnapshot {
        schema_version: 99,
        observed_at_unix_ms: 0,
        sample_interval_ms: 0,
        capabilities: MetricCapabilities { cpu_iowait: true },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 0,
            usage_pct: f32::NAN,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: -1.0,
            five: 0.0,
            fifteen: f32::INFINITY,
        },
        memory: MemoryMetrics {
            used_bytes: 100,
            total_bytes: 50,
            usage_pct: 101.0,
        },
        swap: SwapMetrics {
            used_bytes: 10,
            total_bytes: 5,
            usage_pct: f32::NAN,
        },
    };
    let err = snap.validate().unwrap_err();
    assert!(
        err.len() >= 10,
        "expected at least 10 violations, got {}",
        err.len()
    );
    let fields: Vec<&str> = err.iter().map(|v| v.field.as_str()).collect();
    assert!(fields.contains(&"schema_version"));
    assert!(fields.contains(&"observed_at_unix_ms"));
    assert!(fields.contains(&"sample_interval_ms"));
    assert!(fields.contains(&"cpu.logical_cores"));
    assert!(fields.contains(&"cpu.usage_pct"));
    assert!(fields.contains(&"cpu.iowait_pct"));
    assert!(fields.contains(&"load.one"));
    assert!(fields.contains(&"load.fifteen"));
    assert!(fields.contains(&"memory.used_bytes"));
    assert!(fields.contains(&"memory.usage_pct"));
    assert!(fields.contains(&"swap.used_bytes"));
    assert!(fields.contains(&"swap.usage_pct"));
}

#[test]
fn boundary_percentage_zero_exactly_is_valid() {
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
    snap.validate().expect("0.0% boundary is valid");
}

#[test]
fn boundary_percentage_one_hundred_exactly_is_valid() {
    let snap = StatusSnapshot {
        schema_version: SCHEMA_VERSION_V1,
        observed_at_unix_ms: 1,
        sample_interval_ms: 1000,
        capabilities: MetricCapabilities { cpu_iowait: false },
        system: identity("h"),
        cpu: CpuMetrics {
            logical_cores: 1,
            usage_pct: 100.0,
            iowait_pct: None,
        },
        load: LoadAverage {
            one: 0.0,
            five: 0.0,
            fifteen: 0.0,
        },
        memory: MemoryMetrics {
            used_bytes: 1,
            total_bytes: 1,
            usage_pct: 100.0,
        },
        swap: SwapMetrics {
            used_bytes: 1,
            total_bytes: 1,
            usage_pct: 100.0,
        },
    };
    snap.validate().expect("100.0% boundary is valid");
}

#[test]
fn null_iowait_with_capability_true_from_json() {
    let json = r#"{
        "schema_version": 1,
        "observed_at_unix_ms": 1,
        "sample_interval_ms": 1000,
        "capabilities": { "cpu_iowait": true },
        "system": {
            "name": "h", "hostname": "h.local", "os_name": "linux", "os_version": "1",
            "kernel_name": "Linux", "kernel_release": "1.0", "architecture": "x86_64"
        },
        "cpu": { "logical_cores": 1, "usage_pct": 0.0, "iowait_pct": null },
        "load": { "one": 0.0, "five": 0.0, "fifteen": 0.0 },
        "memory": { "used_bytes": 0, "total_bytes": 1, "usage_pct": 0.0 },
        "swap": { "used_bytes": 0, "total_bytes": 0, "usage_pct": 0.0 }
    }"#;
    let snap: StatusSnapshot =
        serde_json::from_str(json).expect("null iowait with capability true parses");
    let err = snap.validate().unwrap_err();
    assert!(
        err.iter()
            .any(|v| matches!(v.kind, ViolationKind::IowaitCapabilityMismatch)),
        "null iowait with capability true must produce IowaitCapabilityMismatch"
    );
}

#[test]
fn negative_load_average_is_rejected() {
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
            one: -0.5,
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
    assert!(err.iter().any(|v| v.field == "load.one"));
}

#[test]
fn nan_load_average_is_rejected() {
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
            five: f32::NAN,
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
    assert!(err.iter().any(|v| v.field == "load.five"));
}

#[test]
fn very_large_load_average_is_valid() {
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
            one: 99999.0,
            five: 50000.0,
            fifteen: 25000.0,
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
        .expect("very large positive load averages are valid");
}

#[test]
fn infinite_load_average_is_rejected() {
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
            fifteen: f32::INFINITY,
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
    assert!(err.iter().any(|v| v.field == "load.fifteen"));
}
