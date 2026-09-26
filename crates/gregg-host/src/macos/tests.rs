//! Host macOS collector tests (mock-based, no wire).

use super::ffi::{MockNativeQueries, RawCpuTicks, RawVmStats};
use super::MacOsCollector;
use crate::error::CollectErrorKind;
use crate::HostCollector;

#[test]
fn first_sample_is_warming() {
    let mock = MockNativeQueries::success();
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let err = collector.sample().expect_err("first sample warms");
    assert_eq!(err.kind, CollectErrorKind::Warming);
}

#[test]
fn warming_then_valid_sample_yields_host_sample() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector = MacOsCollector::with_source(mock, Some("test-mac")).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let metrics = collector.sample().expect("second sample succeeds");
    assert!(metrics.cpu_usage_pct.is_some());
    assert!(metrics.cpu_iowait_pct.is_none());
    assert!(metrics.load.is_some());
    assert!(metrics.memory.total_bytes > 0);
    assert!(metrics.swap.is_some());
    assert!(metrics.commit.is_none());
    assert_eq!(metrics.cpu_frequency_hz, None);
    let identity = collector.identity().expect("identity");
    assert!(identity.name.contains("test-mac"));
    assert_eq!(identity.os_name, "macos");
    let caps = collector.capabilities();
    assert!(!caps.cpu_iowait && caps.load_average && caps.swap);
    assert!(!caps.memory_commit);
}

#[test]
fn counter_reset_then_recovery() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming baseline");
    *collector.source_mut() = MockNativeQueries {
        cpu: RawCpuTicks {
            user: 100,
            system: 50,
            idle: 8000,
            nice: 10,
        },
        ..MockNativeQueries::success()
    };
    let err = collector.sample().expect_err("counter reset reported");
    assert_eq!(err.kind, CollectErrorKind::CounterReset);
    *collector.source_mut() = MockNativeQueries {
        cpu: RawCpuTicks {
            user: 600,
            system: 250,
            idle: 4500,
            nice: 60,
        },
        ..MockNativeQueries::success()
    };
    let metrics = collector.sample().expect("recovers");
    let cpu = metrics.cpu_usage_pct.expect("cpu");
    assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
}

#[test]
fn memory_normalization_covers_edge_cases() {
    use super::memory::compute_memory;
    let raw = RawVmStats {
        free_count: 100_000,
        active_count: 200_000,
        inactive_count: 150_000,
        wire_count: 50_000,
        page_size: 16_384,
    };
    let mem = compute_memory(&raw, 16_000_000_000).expect("computes");
    assert_eq!(mem.used_bytes, 11_904_000_000);
}

#[test]
fn drive_failure_preserves_core_sample() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    mock.mounted_error = true;
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let metrics = collector.sample().expect("core metrics remain available");
    assert!(metrics.cpu_usage_pct.is_some());
    assert_eq!(metrics.drives, None);
}

#[test]
fn live_rates_publish_with_loopback_detail() {
    use super::ffi::{RawDiskIo, RawNetworkInterface};
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    mock.disk = vec![RawDiskIo {
        id: "disk0".to_string(),
        name: "disk0".to_string(),
        read_bytes: 1_000,
        write_bytes: 2_000,
    }];
    mock.network = vec![
        RawNetworkInterface {
            id: "lo0".to_string(),
            name: "lo0".to_string(),
            rx_bytes: 10_000,
            tx_bytes: 20_000,
            rx_capacity_bps: Some(1_000_000_000),
            tx_capacity_bps: Some(1_000_000_000),
            is_loopback: true,
            operational: true,
            aggregate_member: false,
        },
        RawNetworkInterface {
            id: "en0".to_string(),
            name: "en0".to_string(),
            rx_bytes: 30_000,
            tx_bytes: 40_000,
            rx_capacity_bps: Some(10_000_000_000),
            tx_capacity_bps: Some(8_000_000_000),
            is_loopback: false,
            operational: true,
            aggregate_member: true,
        },
    ];
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("first warms");
    let baseline = collector.sample().expect("second warms live baselines");
    assert!(baseline.disk_io.is_none());
    assert!(baseline.network.is_none());
    collector.source_mut().disk[0].read_bytes += 1_000;
    collector.source_mut().disk[0].write_bytes += 2_000;
    collector.source_mut().network[0].rx_bytes += 1_000;
    collector.source_mut().network[0].tx_bytes += 2_000;
    collector.source_mut().network[1].rx_bytes += 3_000;
    collector.source_mut().network[1].tx_bytes += 4_000;
    let metrics = collector.sample().expect("third publishes live metrics");
    assert!(metrics.disk_io.is_some());
    let network = metrics.network.expect("network");
    assert!(network.aggregate_rx_bytes_per_sec > 0);
    assert!(network
        .interfaces
        .iter()
        .find(|i| i.is_loopback)
        .is_some_and(|i| !i.aggregate_member));
}
