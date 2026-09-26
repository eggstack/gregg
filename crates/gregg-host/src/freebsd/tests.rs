//! Host FreeBSD collector tests (mock-based, deterministic).

use super::source::{MockFreeBsdSource, RawCpuTimes, RawNetworkInterface, RawPhysicalMemory};
use super::{compute_cpu_percentages, compute_memory, FreeBsdCollector};
use crate::error::CollectErrorKind;
use crate::HostCollector;

#[test]
fn first_sample_is_warming() {
    let mock = MockFreeBsdSource::success();
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let err = collector.sample().expect_err("first sample warms");
    assert_eq!(err.kind, CollectErrorKind::Warming);
}

#[test]
fn warming_then_valid_sample_yields_host_sample() {
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let sample = collector.sample().expect("second sample succeeds");
    assert!(sample.cpu_usage_pct.is_some());
    assert!(sample.cpu_iowait_pct.is_none());
    assert!(sample.load.is_some());
    assert!(sample.memory.total_bytes > 0);
    assert!(sample.swap.is_none());
    assert!(sample.commit.is_none());
    assert_eq!(sample.cpu_frequency_hz, None);
    let caps = collector.capabilities();
    assert!(!caps.cpu_iowait && caps.load_average);
    assert!(!caps.swap && !caps.memory_commit);
    assert!(caps.drives && caps.disk_io && caps.network);
    assert!(!caps.cpu_frequency);
}

#[test]
fn cpu_delta_matches_hand_calculation() {
    let prev = RawCpuTimes {
        user: 1000,
        nice: 100,
        sys: 500,
        intr: 50,
        idle: 8000,
    };
    let curr = RawCpuTimes {
        user: 1500,
        nice: 100,
        sys: 700,
        intr: 60,
        idle: 8500,
    };
    // busy: prev 1650, curr 2360, delta 710; total: prev 9650, curr 10860,
    // delta 1210; usage = 710/1210*100 ~= 58.6777.
    let pct = compute_cpu_percentages(&prev, &curr).expect("computes");
    assert!((pct - 58.677_7).abs() < 1e-3);
}

#[test]
fn cpu_counter_reset_then_recovery() {
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming baseline");
    collector.source_mut().auto_increment_cpu = false;
    collector.source_mut().cpu = RawCpuTimes {
        user: 10,
        nice: 1,
        sys: 5,
        intr: 0,
        idle: 80,
    };
    let err = collector.sample().expect_err("counter reset");
    assert_eq!(err.kind, CollectErrorKind::CounterReset);
    collector.source_mut().cpu = RawCpuTimes {
        user: 110,
        nice: 11,
        sys: 55,
        intr: 5,
        idle: 580,
    };
    let sample = collector.sample().expect("recovers");
    let cpu = sample.cpu_usage_pct.expect("cpu");
    assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
}

#[test]
fn memory_formula_uses_free_inactive_cache_laundry() {
    let raw = RawPhysicalMemory {
        total_bytes: 16_000_000_000,
        page_size: 4096,
        free_count: 500_000,
        inactive_count: 300_000,
        cache_count: 200_000,
        laundry_count: 50_000,
    };
    // available = 1_050_000 * 4096 = 4_300_800_000;
    // used = 16_000_000_000 - 4_300_800_000 = 11_699_200_000.
    let mem = compute_memory(&raw).expect("computes");
    assert_eq!(mem.used_bytes, 11_699_200_000);
    assert_eq!(mem.total_bytes, 16_000_000_000);
}

#[test]
fn memory_zero_total_yields_zero() {
    let raw = RawPhysicalMemory {
        total_bytes: 0,
        page_size: 4096,
        free_count: 0,
        inactive_count: 0,
        cache_count: 0,
        laundry_count: 0,
    };
    let mem = compute_memory(&raw).expect("zero total");
    assert_eq!((mem.used_bytes, mem.total_bytes), (0, 0));
}

#[test]
fn memory_extreme_page_counts_do_not_overflow() {
    let mock = MockFreeBsdSource {
        memory: RawPhysicalMemory {
            total_bytes: 128_000_000_000,
            page_size: 4096,
            free_count: u64::MAX / 4096,
            inactive_count: 0,
            cache_count: 0,
            laundry_count: 0,
        },
        auto_increment_cpu: true,
        ..MockFreeBsdSource::success()
    };
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let sample = collector.sample().expect("sample succeeds");
    assert!(sample.memory.used_bytes <= sample.memory.total_bytes);
}

#[test]
fn load_negative_rejected() {
    let mut mock = MockFreeBsdSource::success();
    mock.load = [-0.5, 0.4, 0.3];
    mock.auto_increment_cpu = true;
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let err = collector.sample().expect_err("negative load");
    assert_eq!(err.kind, CollectErrorKind::Parse);
}

#[test]
fn identity_fields_are_nonempty() {
    let mock = MockFreeBsdSource::success();
    let collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let identity = collector.identity().expect("identity");
    assert!(!identity.hostname.is_empty());
    assert_eq!(identity.os_name, "freebsd");
    assert_eq!(identity.kernel_name, "FreeBSD");
    assert!(!identity.architecture.is_empty());
    assert!(!identity.os_version.is_empty());
}

#[test]
fn optional_failures_preserve_core_sample() {
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    mock.mounted_error = true;
    mock.disk_error = true;
    mock.network_error = true;
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let sample = collector.sample().expect("core succeeds");
    assert!(sample.cpu_usage_pct.is_some());
    assert!(sample.memory.total_bytes > 0);
    assert_eq!(sample.drives, None);
    assert_eq!(sample.disk_io, None);
    assert_eq!(sample.network, None);
}

#[test]
fn successful_empty_drive_enumeration_is_preserved() {
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    mock.mounted.clear();
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    for _ in 0..200 {
        if let Ok(sample) = collector.sample() {
            if sample.drives.is_some() {
                assert_eq!(sample.drives, Some(Vec::new()));
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("drive refresh did not complete");
}

#[test]
fn disk_appearance_disappearance_and_reset() {
    use super::source::RawDiskIo;
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    mock.disk = vec![RawDiskIo {
        id: "ada0".to_string(),
        name: "ada0".to_string(),
        read_bytes: 1_000,
        write_bytes: 2_000,
    }];
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let baseline = collector.sample().expect("second warms disk baselines");
    assert!(baseline.disk_io.is_none());
    // Device disappears: baseline removed.
    collector.source_mut().disk.clear();
    let gone = collector.sample().expect("core succeeds");
    assert!(gone.disk_io.is_none());
    // Device reappears with fresh counters: warms, no spike.
    collector.source_mut().disk = vec![RawDiskIo {
        id: "ada0".to_string(),
        name: "ada0".to_string(),
        read_bytes: 100,
        write_bytes: 200,
    }];
    let rewarmed = collector.sample().expect("core succeeds");
    assert!(rewarmed.disk_io.is_none());
    // Next interval publishes rates.
    collector.source_mut().disk[0].read_bytes += 1_000;
    collector.source_mut().disk[0].write_bytes += 2_000;
    let published = collector.sample().expect("publishes");
    assert!(published.disk_io.is_some());
    // Counter decrease re-baselines without a spike.
    collector.source_mut().disk[0].read_bytes = 10;
    collector.source_mut().disk[0].write_bytes = 20;
    let wrapped = collector.sample().expect("core succeeds");
    assert!(wrapped.disk_io.is_none());
}

#[test]
fn network_sparse_loopback_and_counter_reset() {
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    mock.network = vec![
        RawNetworkInterface {
            id: "if1".to_string(),
            name: "lo0".to_string(),
            rx_bytes: 10_000,
            tx_bytes: 20_000,
            rx_capacity_bps: None,
            tx_capacity_bps: None,
            is_loopback: true,
            operational: true,
            aggregate_member: false,
        },
        RawNetworkInterface {
            id: "if2".to_string(),
            name: "em0".to_string(),
            rx_bytes: 30_000,
            tx_bytes: 40_000,
            rx_capacity_bps: Some(1_000_000_000),
            tx_capacity_bps: Some(1_000_000_000),
            is_loopback: false,
            operational: true,
            aggregate_member: true,
        },
    ];
    let mut collector = FreeBsdCollector::with_source(mock, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let _ = collector.sample().expect("warms network baselines");
    collector.source_mut().network[0].rx_bytes += 1_000;
    collector.source_mut().network[0].tx_bytes += 2_000;
    collector.source_mut().network[1].rx_bytes += 3_000;
    collector.source_mut().network[1].tx_bytes += 4_000;
    let sample = collector.sample().expect("publishes");
    let network = sample.network.expect("network");
    // Loopback stays detail-only.
    assert!(network
        .interfaces
        .iter()
        .find(|i| i.is_loopback)
        .is_some_and(|i| !i.aggregate_member));
    // Only the non-loopback aggregate member feeds capacity.
    assert_eq!(network.aggregate_rx_capacity_bps, Some(1_000_000_000));
    // Counter reset on every interface re-baselines the whole family.
    collector.source_mut().network[0].rx_bytes = 5;
    collector.source_mut().network[0].tx_bytes = 6;
    collector.source_mut().network[1].rx_bytes = 7;
    collector.source_mut().network[1].tx_bytes = 8;
    let reset = collector.sample().expect("core succeeds");
    assert!(reset.network.is_none());
}

#[test]
fn deterministic_ordering_and_bounds() {
    use super::source::RawDiskIo;
    use crate::model::CollectionLimits;
    let mut mock = MockFreeBsdSource::success();
    mock.auto_increment_cpu = true;
    mock.disk = (0..40)
        .map(|i| RawDiskIo {
            id: format!("disk{i:02}"),
            name: format!("disk{i:02}"),
            read_bytes: 0,
            write_bytes: 0,
        })
        .collect();
    let limits = CollectionLimits::gregg_defaults();
    let mut collector =
        FreeBsdCollector::with_source_and_limits(mock, None, limits).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    let _ = collector.sample().expect("warms");
    for record in collector.source_mut().disk.iter_mut() {
        record.read_bytes += 1_000;
        record.write_bytes += 1_000;
    }
    let sample = collector.sample().expect("publishes");
    let disk = sample.disk_io.expect("disk");
    assert_eq!(disk.devices.len(), limits.max_disk_io_entries);
    let ids: Vec<_> = disk.devices.iter().map(|d| d.id.clone()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

// --- Native FreeBSD qualification (FreeBSD hosts only) -----------------------

/// Native smoke: warms the collector and validates truthful values.
/// Runs in the bounded FreeBSD CI job; zero-rate intervals are valid.
#[cfg(target_os = "freebsd")]
#[test]
fn native_smoke_warms_and_validates() {
    use std::time::Duration;
    let mut collector = FreeBsdCollector::new(None).expect("native collector constructs");
    let identity = collector.identity().expect("native identity");
    assert!(!identity.hostname.is_empty());
    assert!(!identity.architecture.is_empty());
    assert_eq!(identity.os_name, "freebsd");
    let err = collector.sample().expect_err("first sample warms");
    assert_eq!(err.kind, CollectErrorKind::Warming);
    // Bounded warmup for counter baselines + async drive refresh.
    let start = std::time::Instant::now();
    let deadline = Duration::from_secs(20);
    let sample = loop {
        std::thread::sleep(Duration::from_millis(300));
        match collector.sample() {
            Ok(sample) => {
                if sample.drives.is_some() {
                    break sample;
                }
            }
            Err(error) if error.kind == CollectErrorKind::CounterReset => {}
            Err(error) => panic!("native sample failed: {error:?}"),
        }
        assert!(
            start.elapsed() < deadline,
            "native warmup expired without drive enumeration"
        );
    };
    assert!(sample.logical_cores > 0);
    let cpu = sample.cpu_usage_pct.expect("cpu after warmup");
    assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
    assert!(sample.cpu_iowait_pct.is_none());
    assert!(sample.memory.total_bytes > 0);
    assert!(sample.memory.used_bytes <= sample.memory.total_bytes);
    assert!(sample.load.is_some());
    assert!(sample.swap.is_none(), "swap truthfully unsupported");
    assert_eq!(sample.cpu_frequency_hz, None);
    let drives = sample.drives.expect("drives enumerated");
    assert!(!drives.is_empty(), "ordinary VM has a local filesystem");
    let network = sample.network.as_ref();
    assert!(
        network.is_some_and(|network| !network.interfaces.is_empty()),
        "ordinary VM has network interfaces"
    );
    let network = network.expect("network");
    assert!(network
        .interfaces
        .iter()
        .any(|interface| !interface.is_loopback));
}

/// Native write-activity proof: known disk-write traffic must advance write
/// counters (read family stays monotonic; write family strictly advances).
#[cfg(target_os = "freebsd")]
#[test]
fn native_disk_write_traffic_advances_write_counters() {
    use super::source::{FreeBsdSource, NativeFreeBsdSource};
    let source = NativeFreeBsdSource;
    let before = source.disk_io().expect("devstat readable");
    assert!(!before.is_empty());
    let path = std::env::temp_dir().join("gregg-host-direction-probe");
    let payload = vec![0xA5u8; 4_194_304];
    std::fs::write(&path, &payload).expect("probe write");
    // Ensure the write reaches the device counters.
    let _ = std::process::Command::new("sync").output();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let after = source.disk_io().expect("devstat readable");
    let _ = std::fs::remove_file(&path);
    let mut advanced = false;
    for current in &after {
        if let Some(previous) = before.iter().find(|dev| dev.id == current.id) {
            assert!(
                current.read_bytes >= previous.read_bytes
                    && current.write_bytes >= previous.write_bytes,
                "devstat counters must be monotonic per device"
            );
            if current.write_bytes > previous.write_bytes {
                advanced = true;
            }
        }
    }
    assert!(
        advanced,
        "known write traffic must advance a device counter"
    );
}

/// Native counter-activity proof: loopback ping must advance `lo` counters.
///
/// The native loopback smoke proves that the mapped ifmib byte-counter
/// fields are live and advance under known loopback traffic. RX/TX semantic
/// ordering is grounded in the field-for-field FreeBSD `struct if_data` ABI
/// mapping, not inferred from symmetric loopback traffic.
#[cfg(target_os = "freebsd")]
#[test]
fn native_loopback_traffic_advances_lo_counters() {
    use super::source::{FreeBsdSource, NativeFreeBsdSource};
    use std::time::Duration;
    let source = NativeFreeBsdSource;
    let before = source
        .network_interfaces()
        .expect("ifmib readable before traffic");
    let previous = before
        .iter()
        .find(|iface| iface.is_loopback)
        .expect("loopback interface present before traffic");
    let previous_id = previous.id.clone();
    let previous_name = previous.name.clone();
    let previous_rx = previous.rx_bytes;
    let previous_tx = previous.tx_bytes;
    let output = std::process::Command::new("ping")
        .args(["-c", "3", "-t", "2", "127.0.0.1"])
        .output()
        .expect("FreeBSD base-system ping must execute on the qualification image");
    assert!(
        output.status.success(),
        "FreeBSD base-system ping must succeed on the qualification image"
    );
    std::thread::sleep(Duration::from_millis(300));
    let after = source
        .network_interfaces()
        .expect("ifmib readable after traffic");
    let current = after
        .iter()
        .find(|iface| iface.id == previous_id)
        .expect("same loopback interface present after traffic");
    assert_eq!(
        current.name, previous_name,
        "loopback identity must remain stable across traffic"
    );
    assert!(
        current.is_loopback,
        "matched interface must still be loopback after traffic"
    );
    assert!(
        current.rx_bytes >= previous_rx && current.tx_bytes >= previous_tx,
        "loopback counters must be monotonic"
    );
    if current.rx_bytes > previous_rx && current.tx_bytes > previous_tx {
        return;
    }
    // One bounded re-read for counter-accounting visibility only.
    std::thread::sleep(Duration::from_millis(1000));
    let again = source
        .network_interfaces()
        .expect("ifmib readable on visibility retry");
    let current = again
        .iter()
        .find(|iface| iface.id == previous_id)
        .expect("same loopback interface present on visibility retry");
    assert_eq!(
        current.name, previous_name,
        "loopback identity must remain stable on visibility retry"
    );
    assert!(
        current.rx_bytes >= previous_rx && current.tx_bytes >= previous_tx,
        "loopback counters must be monotonic on visibility retry"
    );
    assert!(
        current.rx_bytes > previous_rx,
        "loopback RX must advance under ping traffic"
    );
    assert!(
        current.tx_bytes > previous_tx,
        "loopback TX must advance under ping traffic"
    );
}
