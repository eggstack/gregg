//! Source-level tests for the macOS collector.
//!
//! These tests inject values through [`MockNativeQueries`] so they can
//! exercise edge cases without depending on the host macOS state.
//! Counter-delta math, normalization, error taxonomy, and identity parsing
//! are covered deterministically here.

use gregg_protocol::{MetricCapabilities, StatusSnapshot};

use super::cpu::compute_cpu_percentages;
use super::ffi::{MockNativeQueries, RawCpuTicks, RawSwapUsage, RawVmStats};
use super::identity::collect_identity;
use super::memory::compute_memory;
use super::swap::compute_swap;
use super::{parse_loadavgs, MacOsCollector};
use crate::collector::error::CollectErrorKind;
use crate::collector::SystemCollector;

#[test]
fn first_sample_is_warming() {
    let mock = MockNativeQueries::success();
    let mut collector = MacOsCollector::with_source(mock, None).expect("collector constructs");
    let err = collector.sample().expect_err("first sample warms");
    assert_eq!(err.kind, CollectErrorKind::Warming);
}

#[test]
fn warming_then_valid_sample_produces_protocol_snapshot() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector =
        MacOsCollector::with_source(mock, Some("test-mac")).expect("collector constructs");
    let _ = collector.sample().expect_err("warming");
    let metrics = collector.sample().expect("second sample succeeds");
    let identity = collector.identity().expect("identity");
    let snap: StatusSnapshot = metrics.into_snapshot(
        gregg_protocol::SCHEMA_VERSION_V1,
        1_716_460_800_000,
        1000,
        MetricCapabilities { cpu_iowait: false },
        identity,
    );
    snap.validate().expect("snapshot validates");
    assert!(!snap.capabilities.cpu_iowait);
    assert!(snap.cpu.iowait_pct.is_none());
    assert!(snap.system.name.contains("test-mac"));
    assert_eq!(snap.system.os_name, "macos");
    assert_eq!(snap.cpu.logical_cores, 8);
}

#[test]
fn counter_reset_is_reported_as_typed_error() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector = MacOsCollector::with_source(mock, None).expect("collector constructs");
    // Establish baseline.
    let _ = collector
        .sample()
        .expect_err("warming baseline established");

    // Swap to lower counters.
    let mock_err = MockNativeQueries {
        cpu: RawCpuTicks {
            user: 100,
            system: 50,
            idle: 8000,
            nice: 10,
        },
        ..MockNativeQueries::success()
    };
    *collector.source_mut() = mock_err;
    let err = collector.sample().expect_err("counter reset reported");
    assert_eq!(err.kind, CollectErrorKind::CounterReset);
}

#[test]
fn zero_total_delta_is_typed_error() {
    let ticks = RawCpuTicks {
        user: 1000,
        system: 500,
        idle: 8000,
        nice: 100,
    };
    let err = compute_cpu_percentages(&ticks, &ticks).expect_err("zero delta");
    assert_eq!(err.kind, CollectErrorKind::CounterReset);
}

#[test]
fn cpu_delta_matches_hand_calculated() {
    let prev = RawCpuTicks {
        user: 1000,
        system: 500,
        idle: 8000,
        nice: 100,
    };
    let curr = RawCpuTicks {
        user: 1500,
        system: 700,
        idle: 8500,
        nice: 200,
    };
    let sample = compute_cpu_percentages(&prev, &curr).expect("computes");
    // busy_prev = 1600, busy_curr = 2400, delta_busy = 800
    // total_prev = 9600, total_curr = 10900, delta_total = 1300
    // usage_pct = 800 / 1300 * 100 ≈ 61.5385
    assert!((sample.usage_pct - 61.538_5_f32).abs() < 0.01);
}

#[test]
fn memory_normalization_covers_edge_cases() {
    let raw = RawVmStats {
        free_count: 100_000,
        active_count: 200_000,
        inactive_count: 150_000,
        wire_count: 50_000,
        page_size: 16_384,
    };
    let total = 16_000_000_000;
    let mem = compute_memory(&raw, total).expect("computes");
    // available = (100_000 + 150_000) * 16_384 = 4_096_000_000
    // used = 16_000_000_000 - 4_096_000_000 = 11_904_000_000
    assert_eq!(mem.used_bytes, 11_904_000_000);
    assert_eq!(mem.total_bytes, total);
    let metrics = mem.into_metrics();
    assert!(metrics.usage_pct.is_finite());
    assert!((0.0..=100.0).contains(&metrics.usage_pct));
}

#[test]
fn swap_zero_total_handled() {
    let raw = RawSwapUsage {
        total_bytes: 0,
        used_bytes: 0,
    };
    let swap = compute_swap(&raw);
    assert_eq!(swap.used_bytes, 0);
    assert_eq!(swap.total_bytes, 0);
    let metrics = swap.into_metrics();
    assert!((metrics.usage_pct - 0.0).abs() < f32::EPSILON);
}

#[test]
fn swap_used_exceeding_total_clamped() {
    let raw = RawSwapUsage {
        total_bytes: 1_000_000_000,
        used_bytes: 2_000_000_000,
    };
    let swap = compute_swap(&raw);
    assert_eq!(swap.used_bytes, 1_000_000_000);
}

#[test]
fn loadavgs_normal() {
    let load = parse_loadavgs(&[1.5, 1.0, 0.5]).expect("parses");
    assert!((load.one - 1.5).abs() < 1e-6);
    assert!((load.five - 1.0).abs() < 1e-6);
    assert!((load.fifteen - 0.5).abs() < 1e-6);
}

#[test]
fn loadavgs_negative_rejected() {
    let err = parse_loadavgs(&[-1.0, 1.0, 0.5]).expect_err("negative");
    assert_eq!(err.kind, CollectErrorKind::Parse);
}

#[test]
fn loadavgs_nan_rejected() {
    let err = parse_loadavgs(&[f64::NAN, 1.0, 0.5]).expect_err("nan");
    assert_eq!(err.kind, CollectErrorKind::Parse);
}

#[test]
fn identity_collects_all_fields() {
    let mock = MockNativeQueries::success();
    let identity = collect_identity(&mock, None).expect("identity");
    assert_eq!(identity.hostname, "test-mac.local");
    assert_eq!(identity.os_name, "macos");
    assert_eq!(identity.os_version, "15.0");
    assert_eq!(identity.kernel_name, "Darwin");
    assert_eq!(identity.kernel_release, "24.0.0");
    assert_eq!(identity.architecture, "arm64");
}

#[test]
fn identity_error_propagated() {
    let mut mock = MockNativeQueries::success();
    mock.identity_error = true;
    let err = collect_identity(&mock, None).expect_err("must fail");
    assert_eq!(err.kind, CollectErrorKind::SourceUnavailable);
}

#[test]
fn cpu_error_propagated() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    collector.source_mut().cpu_error = true;
    let err = collector.sample().expect_err("cpu error");
    assert_eq!(err.kind, CollectErrorKind::SourceUnavailable);
}

#[test]
fn vm_error_propagated() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    // First sample warms, then we inject VM error.
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    collector.source_mut().vm_error = true;
    let err = collector.sample().expect_err("vm error");
    assert_eq!(err.kind, CollectErrorKind::SourceUnavailable);
}

#[test]
fn capabilities_mark_iowait_unsupported() {
    let mock = MockNativeQueries::success();
    let collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let caps = collector.capabilities();
    assert!(!caps.cpu_iowait);
}

#[test]
fn repeated_samples_show_no_unbounded_growth() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    for _ in 0..1000 {
        let _ = collector.sample();
    }
}

#[test]
fn protocol_snapshot_validates_with_mock_values() {
    let mut mock = MockNativeQueries::success();
    mock.auto_increment_cpu = true;
    let mut collector = MacOsCollector::with_source(mock, Some("deadpool")).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let metrics = collector.sample().expect("second sample succeeds");
    let identity = collector.identity().expect("identity");
    let snap: StatusSnapshot = metrics.into_snapshot(
        gregg_protocol::SCHEMA_VERSION_V1,
        1_716_460_800_000,
        1000,
        MetricCapabilities { cpu_iowait: false },
        identity,
    );
    snap.validate().expect("snapshot validates");
    assert_eq!(snap.system.name, "deadpool");
    assert_eq!(snap.cpu.logical_cores, 8);
    assert!(!snap.capabilities.cpu_iowait);
    assert!(snap.cpu.iowait_pct.is_none());
}
