//! Host Linux collector tests.
//!
//! Fixture-driven coverage moved with the implementation (Plan 134).
//! Asserts on [`HostSample`](crate::model::HostSample) directly; wire
//! validation stays in `greggd`.

use std::path::Path;

use super::fixtures::read_fixture;
use super::identity::collect_identity;
use super::memory::{compute_memory, compute_swap, parse_meminfo};
use super::source::{MemorySource, ProcSource};
use super::{compute_percentages, parse_proc_stat, LinuxCollector};
use crate::error::CollectErrorKind;
use crate::HostCollector;

fn source_from(
    fixtures: &[(&str, &str)],
    cores: usize,
    os_release_fixture: Option<&str>,
) -> ProcSource {
    let mut mem = MemorySource::new().with_logical_cores(cores);
    for (fixture_name, path) in fixtures {
        mem = mem.with_file(Path::new(path), read_fixture(fixture_name));
    }
    if !mem.has_file("/proc/sys/kernel/hostname") {
        mem = mem.with_file(Path::new("/proc/sys/kernel/hostname"), "test-host\n");
    }
    let mut source = ProcSource::for_memory(mem);
    if let Some(name) = os_release_fixture {
        let path = Path::new("/etc/os-release").to_path_buf();
        source
            .memory_source_mut()
            .expect("memory source")
            .add_file(&path, read_fixture(name));
        source = source.with_os_release_path(path);
    }
    source
}

#[test]
fn parses_aggregate_cpu_row() {
    let raw = read_fixture("ubuntu_x86_64_proc_stat_a.txt");
    let parsed = parse_proc_stat(&raw).expect("parses");
    let aggregate = parsed.aggregate.expect("aggregate present");
    assert_eq!(aggregate.user, 100);
    assert_eq!(aggregate.idle, 8000);
    assert_eq!(aggregate.iowait, 30);
}

#[test]
fn counter_delta_matches_hand_calculated_fixture() {
    use super::CpuCounters;
    let prev = CpuCounters {
        user: 100,
        nice: 0,
        system: 50,
        idle: 8000,
        iowait: 30,
        irq: 5,
        softirq: 2,
        steal: 1,
    };
    let curr = CpuCounters {
        user: 150,
        nice: 0,
        system: 60,
        idle: 8050,
        iowait: 50,
        irq: 6,
        softirq: 3,
        steal: 1,
    };
    let sample = compute_percentages(&prev, &curr).expect("computes");
    assert!((sample.usage_pct - 46.969_7_f32).abs() < 1e-3);
    assert!((sample.iowait_pct - 15.151_5_f32).abs() < 1e-3);
}

#[test]
fn first_sample_is_warming() {
    let mut collector = LinuxCollector::with_source(
        source_from(
            &[
                ("ubuntu_x86_64_proc_stat_a.txt", "/proc/stat"),
                ("ubuntu_x86_64_proc_loadavg.txt", "/proc/loadavg"),
                ("ubuntu_x86_64_proc_meminfo.txt", "/proc/meminfo"),
            ],
            4,
            Some("ubuntu_x86_64_os_release.txt"),
        ),
        None,
    )
    .expect("collector constructs");
    let err = collector.sample().expect_err("first sample warms");
    assert_eq!(err.kind, CollectErrorKind::Warming);
}

#[test]
fn warming_then_valid_sample_yields_host_sample() {
    let mut collector = LinuxCollector::with_source(
        source_from(
            &[
                ("ubuntu_x86_64_proc_stat_a.txt", "/proc/stat"),
                ("ubuntu_x86_64_proc_loadavg.txt", "/proc/loadavg"),
                ("ubuntu_x86_64_proc_meminfo.txt", "/proc/meminfo"),
            ],
            8,
            Some("ubuntu_x86_64_os_release.txt"),
        ),
        Some("deadpool"),
    )
    .expect("collector constructs");
    let _ = collector.sample().expect_err("warming");
    collector
        .source_mut()
        .memory_source_mut()
        .expect("memory source")
        .add_file("/proc/stat", read_fixture("ubuntu_x86_64_proc_stat_b.txt"));
    let metrics = collector.sample().expect("second sample succeeds");
    assert_eq!(metrics.logical_cores, 8);
    assert!((metrics.cpu_usage_pct.expect("cpu") - 46.969_7_f32).abs() < 1e-3);
    assert!((metrics.cpu_iowait_pct.expect("iowait") - 15.151_5_f32).abs() < 1e-3);
    let load = metrics.load.expect("load");
    assert!((load.one - 1.32).abs() < 1e-3);
    assert!(metrics.memory.total_bytes > 0);
    assert!(metrics.swap.is_some());
    assert!(metrics.commit.is_none());
    let identity = collector.identity().expect("identity");
    assert!(identity.name.contains("deadpool"));
    assert_eq!(identity.os_name, "Ubuntu 24.04 LTS");
    let caps = collector.capabilities();
    assert!(caps.cpu_iowait && caps.load_average && caps.swap);
    assert!(!caps.memory_commit);
}

#[test]
fn counter_reset_then_recovery_without_spike() {
    let mut collector = LinuxCollector::with_source(
        source_from(
            &[
                ("cpu_reset_proc_stat_a.txt", "/proc/stat"),
                ("ubuntu_x86_64_proc_loadavg.txt", "/proc/loadavg"),
                ("ubuntu_x86_64_proc_meminfo.txt", "/proc/meminfo"),
            ],
            8,
            Some("ubuntu_x86_64_os_release.txt"),
        ),
        None,
    )
    .expect("collector constructs");
    let _ = collector.sample().expect_err("warming baseline");
    collector
        .source_mut()
        .memory_source_mut()
        .expect("memory source")
        .add_file("/proc/stat", read_fixture("cpu_reset_proc_stat_b.txt"));
    let err = collector.sample().expect_err("counter reset reported");
    assert_eq!(err.kind, CollectErrorKind::CounterReset);
    collector
        .source_mut()
        .memory_source_mut()
        .expect("memory source")
        .add_file("/proc/stat", read_fixture("ubuntu_x86_64_proc_stat_b.txt"));
    // Recovery would need a fresh monotonic increase over the reset
    // baseline; at minimum the collector must not spike and must either
    // warm, reset, or yield a finite percentage.
    match collector.sample() {
        Ok(metrics) => {
            let cpu = metrics.cpu_usage_pct.expect("cpu");
            assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
        }
        Err(err) => assert!(
            matches!(
                err.kind,
                CollectErrorKind::Warming | CollectErrorKind::CounterReset
            ),
            "unexpected kind {:?}",
            err.kind
        ),
    }
}

#[test]
fn zero_swap_is_handled_without_panic() {
    let raw = read_fixture("zero_swap_proc_meminfo.txt");
    let parsed = parse_meminfo(&raw).expect("parses");
    let swap = compute_swap(&parsed).expect("zero swap parses");
    assert_eq!(swap.total_bytes, 0);
    assert_eq!(swap.used_bytes, 0);
}

#[test]
fn memory_fallback_used_when_memavailable_missing() {
    let raw = read_fixture("missing_mem_available_proc_meminfo.txt");
    let parsed = parse_meminfo(&raw).expect("parses");
    let mem = compute_memory(&parsed).expect("fallback computes");
    assert!(mem.fallback_used);
}

#[test]
fn cpu_hotplug_refreshes_core_count() {
    let mut collector = LinuxCollector::with_source(
        source_from(
            &[
                ("hotplug_2core_proc_stat_a.txt", "/proc/stat"),
                ("container_proc_loadavg.txt", "/proc/loadavg"),
                ("container_proc_meminfo.txt", "/proc/meminfo"),
            ],
            2,
            None,
        ),
        None,
    )
    .expect("collector constructs");
    let _ = collector.sample().expect_err("warming");
    collector
        .source_mut()
        .memory_source_mut()
        .expect("memory source")
        .add_file("/proc/stat", read_fixture("hotplug_4core_proc_stat_b.txt"));
    collector
        .source_mut()
        .memory_source_mut()
        .expect("memory source")
        .set_logical_cores(4);
    let metrics = collector.sample().expect("sample after hotplug succeeds");
    assert_eq!(metrics.logical_cores, 4);
}

#[test]
fn optional_families_absent_preserve_core_sample() {
    let mut mem = MemorySource::new().with_logical_cores(2);
    mem.add_file(Path::new("/proc/stat"), "cpu  100 0 50 8000 30 5 2 1 0 0\n");
    mem.add_file(Path::new("/proc/loadavg"), "0.10 0.20 0.30 1/50 1\n");
    mem.add_file(
        Path::new("/proc/meminfo"),
        "MemTotal:        8000000 kB\nMemAvailable:     4000000 kB\nSwapTotal:              0 kB\nSwapFree:               0 kB\n",
    );
    mem.add_file(Path::new("/proc/sys/kernel/hostname"), "freeze-host\n");
    let mut collector =
        LinuxCollector::with_source(ProcSource::for_memory(mem), None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    collector
        .source_mut()
        .memory_source_mut()
        .expect("memory source")
        .add_file(Path::new("/proc/stat"), "cpu  150 0 60 8050 50 6 3 1 0 0\n");
    let metrics = collector.sample().expect("core succeeds");
    assert!(metrics.cpu_usage_pct.is_some());
    assert_eq!(metrics.cpu_frequency_hz, None);
    assert_eq!(metrics.disk_io, None);
    assert_eq!(metrics.network, None);
    assert_eq!(metrics.drives, None);
}

#[test]
fn identity_uses_pretty_name_when_present() {
    let source = source_from(
        &[
            ("ubuntu_x86_64_proc_stat_a.txt", "/proc/stat"),
            ("ubuntu_x86_64_proc_loadavg.txt", "/proc/loadavg"),
            ("ubuntu_x86_64_proc_meminfo.txt", "/proc/meminfo"),
        ],
        8,
        Some("ubuntu_x86_64_os_release.txt"),
    );
    let identity = collect_identity(&source, None).expect("identity");
    assert_eq!(identity.os_name, "Ubuntu 24.04 LTS");
}

#[test]
fn plan141_collector_steady_cpufreq_reuses_structure() {
    use super::source::MemorySource;
    use super::source::ProcSource;
    // Pre-populate three stat snapshots and switch `stat_path` between
    // samples so no post-`drive_refresh` fixture mutation is required.
    let mut mem = MemorySource::new().with_logical_cores(4);
    mem.add_file(
        Path::new("/proc/stat_a"),
        "cpu  100 0 50 8000 30 5 2 1 0 0\n",
    );
    mem.add_file(
        Path::new("/proc/stat_b"),
        "cpu  150 0 60 8050 50 6 3 1 0 0\n",
    );
    mem.add_file(
        Path::new("/proc/stat_c"),
        "cpu  200 0 70 8100 70 7 4 1 0 0\n",
    );
    mem.add_file(Path::new("/proc/loadavg"), "0.10 0.20 0.30 1/50 1\n");
    mem.add_file(
        Path::new("/proc/meminfo"),
        "MemTotal:        8000000 kB\nMemAvailable:     4000000 kB\nSwapTotal:              0 kB\nSwapFree:               0 kB\n",
    );
    mem.add_file(Path::new("/proc/sys/kernel/hostname"), "cache-host\n");
    mem.add_file(
        Path::new("/sys/devices/system/cpu/cpufreq/policy0/affected_cpus"),
        "0-1\n",
    );
    mem.add_file(
        Path::new("/sys/devices/system/cpu/cpufreq/policy0/cpuinfo_cur_freq"),
        "2000000\n",
    );
    mem.add_file(
        Path::new("/sys/devices/system/cpu/cpufreq/policy1/affected_cpus"),
        "2-3\n",
    );
    mem.add_file(
        Path::new("/sys/devices/system/cpu/cpufreq/policy1/cpuinfo_cur_freq"),
        "1000000\n",
    );
    let probe = mem.clone();
    let source = ProcSource::for_memory(mem).with_stat_path("/proc/stat_a");
    let mut collector = LinuxCollector::with_source(source, None).expect("constructs");
    let _ = collector.sample().expect_err("warming");
    collector
        .source_mut()
        .set_stat_path(Path::new("/proc/stat_b").to_path_buf());
    let first = collector.sample().expect("steady sample");
    assert_eq!(first.cpu_frequency_hz, Some(1_500_000_000));
    let after_first = probe.call_counts();
    let membership_first = after_first.reads_containing("affected_cpus")
        + after_first.reads_containing("related_cpus");
    collector
        .source_mut()
        .set_stat_path(Path::new("/proc/stat_c").to_path_buf());
    let second = collector.sample().expect("second steady sample");
    assert_eq!(second.cpu_frequency_hz, first.cpu_frequency_hz);
    let after_second = probe.call_counts();
    assert_eq!(
        after_second.reads_containing("affected_cpus")
            + after_second.reads_containing("related_cpus"),
        membership_first,
        "collector steady state must reuse CPUFreq structure"
    );
}
