//! Plan 133 compatibility characterization and boundary freeze.
//!
//! This module inventories the collector contract that Plans 134-135 must
//! preserve and pins it with deterministic sequence, wire, and public-path
//! tests. It introduces no behavioral change: every test asserts current
//! behavior at the existing boundary (no clock injection, no cadence change,
//! no worker redesign, no protocol change).
//!
//! ## Compatibility inventory (frozen)
//!
//! Public / cross-module surface that must survive extraction:
//!
//! - `crate::collector::SystemCollector` (identity, sample, capabilities,
//!   `capabilities_v2`, `supports_v1_snapshot`)
//! - `crate::collector::CollectedMetrics` + `into_snapshot`,
//!   `into_snapshot_v2`, `into_status_payload_v2`, `into_snapshot_pair`
//! - `crate::collector::error::{CollectError, CollectErrorKind}`
//!   (`Warming`, `SourceUnavailable`, `Parse`, `CounterReset`, `Numeric`,
//!   `IdentityFallback` reserved)
//! - `crate::collector::linux::{LinuxCollector, ProcSource, FileSource,
//!   MemorySource, CpuCounters, compute_percentages, parse_proc_stat, ...}`
//! - `crate::collector::macos::{MacOsCollector, cpu, memory, swap,
//!   identity, normalize, ffi::{MacNativeQueries, MockNativeQueries, ...}}`
//! - `crate::collector::windows::{WindowsCollector, WindowsSource,
//!   MockWindowsSource, raw records, ...}`
//! - `crate::collector::{clamped_usage_pct, finalize_percentage}`
//! - `crate::collector::rate::CounterBaselines` (first warms, actual elapsed
//!   time, disappearance removes, reappearance warms, decrease re-baselines,
//!   source failure clears)
//! - `crate::collector::drives::{DriveCandidate, normalize}`
//! - `DriveRefreshCache` slow-probe policy (one worker, immediate first
//!   request, 30s cadence, bounded channels, last-good retention, panic
//!   containment/backoff, nonblocking poll, drop never joins a blocked worker)
//! - Sampler readiness mapping (`Warming`/`CounterReset` never fail;
//!   `SourceUnavailable`/`Parse`/`Numeric` fail; conversion failure fails;
//!   pre-epoch clock fails without publishing)
//! - v1/v2 capability and `supports_v1_snapshot` behavior (Linux/macOS v1+v2,
//!   Windows v2-only; iowait/swap/commit flags per platform)
//! - `drives: None` before first success, `Some(empty)` on successful empty,
//!   last-success retention after later failure.
//!
//! Responsibilities:
//!
//! - Native collection owns acquisition, delta arithmetic, baselines, drive
//!   normalization, and slow-probe isolation.
//! - The daemon sampler owns cadence, wall-clock timestamps, readiness, and
//!   snapshot publication.
//! - `gregg-protocol` owns schema types, JSON shapes, validation, and status
//!   codes.

#[cfg(test)]
mod tests {
    use crate::collector::error::{CollectError, CollectErrorKind};
    use crate::collector::{CollectedMetrics, SystemCollector};
    use gregg_host::model::DriveMetrics as HostDriveMetrics;
    use gregg_protocol::v2::{
        CommitMetrics, DiskIoPayload, DriveMetrics, MetricCapabilitiesV2, NetworkInterfaceMetrics,
        NetworkPayload, MAX_DISK_IO_ENTRIES, MAX_NETWORK_INTERFACE_ENTRIES,
    };
    use gregg_protocol::{
        LoadAverage, MemoryMetrics, MetricCapabilities, SwapMetrics, SystemIdentity,
        SCHEMA_VERSION_V1,
    };

    fn test_identity() -> SystemIdentity {
        SystemIdentity {
            name: "freeze-test".to_string(),
            hostname: "freeze-host".to_string(),
            os_name: "linux".to_string(),
            os_version: "1.0".to_string(),
            kernel_name: "Linux".to_string(),
            kernel_release: "6.0.0".to_string(),
            architecture: "x86_64".to_string(),
        }
    }

    fn ready_metrics_linux_like() -> CollectedMetrics {
        CollectedMetrics {
            logical_cores: 8,
            cpu_usage_pct: Some(46.969_7),
            cpu_iowait_pct: Some(15.151_5),
            load: LoadAverage {
                one: 1.32,
                five: 0.91,
                fifteen: 0.62,
            },
            memory: MemoryMetrics {
                used_bytes: 6_000_000_000,
                total_bytes: 16_000_000_000,
                usage_pct: 37.5,
            },
            swap: SwapMetrics {
                used_bytes: 0,
                total_bytes: 4_000_000_000,
                usage_pct: 0.0,
            },
            commit: None,
            drives: Some(vec![DriveMetrics {
                name: "/".to_string(),
                used_bytes: 700,
                total_bytes: 1000,
                available_bytes: Some(300),
            }]),
            cpu_frequency_hz: Some(2_400_000_000),
            disk_io: Some(DiskIoPayload {
                aggregate_read_bytes_per_sec: 1024,
                aggregate_write_bytes_per_sec: 2048,
                devices: vec![gregg_protocol::v2::DiskIoMetrics {
                    id: "sda".to_string(),
                    name: "sda".to_string(),
                    read_bytes_per_sec: 1024,
                    write_bytes_per_sec: 2048,
                    drive_name: None,
                }],
            }),
            network: Some(NetworkPayload {
                aggregate_rx_bytes_per_sec: 4096,
                aggregate_tx_bytes_per_sec: 8192,
                aggregate_rx_capacity_bps: Some(1_000_000_000),
                aggregate_tx_capacity_bps: Some(1_000_000_000),
                interfaces: vec![NetworkInterfaceMetrics {
                    id: "eth0".to_string(),
                    name: "eth0".to_string(),
                    rx_bytes_per_sec: 4096,
                    tx_bytes_per_sec: 8192,
                    rx_capacity_bps: Some(1_000_000_000),
                    tx_capacity_bps: Some(1_000_000_000),
                    is_loopback: false,
                    aggregate_member: true,
                }],
            }),
        }
    }

    fn ready_metrics_macos_like() -> CollectedMetrics {
        let mut metrics = ready_metrics_linux_like();
        metrics.cpu_iowait_pct = None;
        metrics.cpu_frequency_hz = None;
        metrics
    }

    fn ready_metrics_windows_like() -> CollectedMetrics {
        CollectedMetrics {
            logical_cores: 8,
            cpu_usage_pct: Some(25.0),
            cpu_iowait_pct: None,
            load: LoadAverage {
                one: 0.0,
                five: 0.0,
                fifteen: 0.0,
            },
            memory: MemoryMetrics {
                used_bytes: 6_000_000_000,
                total_bytes: 16_000_000_000,
                usage_pct: 37.5,
            },
            swap: SwapMetrics {
                used_bytes: 0,
                total_bytes: 0,
                usage_pct: 0.0,
            },
            commit: Some(CommitMetrics {
                used_bytes: 800_000_000,
                limit_bytes: 3_200_000_000,
                usage_pct: 25.0,
            }),
            drives: Some(vec![]),
            cpu_frequency_hz: Some(2_400_000_000),
            disk_io: None,
            network: None,
        }
    }

    // --- Public-path compile characterization -------------------------------

    #[allow(clippy::items_after_statements)]
    #[test]
    fn public_collector_paths_remain_importable() {
        // Shared contract. If any of these paths move during extraction
        // without a facade, this test fails to compile.
        use crate::collector::error::{CollectError as _CollectError, CollectErrorKind as _Kind};
        use crate::collector::{clamped_usage_pct as _clamped, finalize_percentage as _finalize};
        use crate::sampler::{Clock as _Clock, Sampler as _Sampler};
        let _ = _CollectError::warming as fn(String) -> _CollectError;
        let _ = _Kind::Warming;
        let _ = _clamped as fn(u64, u64) -> f32;
        let _ = _finalize as fn(f64) -> Result<f32, _CollectError>;
        let _ = core::mem::size_of::<Box<dyn SystemCollector>>();
        fn _assert_clock<T: _Clock>() {}
        fn _assert_sampler<C: SystemCollector, Cl: _Clock>() {
            let _ = core::mem::size_of::<Option<_Sampler<C, Cl>>>();
        }

        // Platform modules remain at their frozen paths (cfg-gated).
        #[cfg(target_os = "linux")]
        {
            use crate::collector::linux::{
                FileSource as _LinuxFileSource, LinuxCollector as _LinuxCollector,
                MemorySource as _LinuxMemory, ProcSource as _LinuxProc,
            };
            let _ = core::mem::size_of::<Option<_LinuxCollector>>();
            let _ = core::mem::size_of::<Option<_LinuxProc>>();
            let _ = core::mem::size_of::<Option<_LinuxMemory>>();
            fn _assert_file_source<T: _LinuxFileSource>() {}
        }
        #[cfg(target_os = "macos")]
        {
            use crate::collector::macos::ffi::{
                MacNativeQueries as _MacQueries, MockNativeQueries as _MacMock,
            };
            use crate::collector::macos::MacOsCollector as _MacCollector;
            let _ = core::mem::size_of::<
                Option<_MacCollector<crate::collector::macos::ffi::FfiNativeQueries>>,
            >();
            let _ = _MacMock::success as fn() -> _MacMock;
            fn _assert_queries<T: _MacQueries>() {}
        }
        #[cfg(target_os = "windows")]
        {
            use crate::collector::windows::source::{
                MockWindowsSource as _WinMock, WindowsSource as _WinSource,
            };
            use crate::collector::windows::WindowsCollector as _WinCollector;
            let _ = core::mem::size_of::<Option<_WinCollector<_WinMock>>>();
            let _ = _WinMock::success as fn() -> _WinMock;
            fn _assert_source<T: _WinSource>() {}
        }
    }

    // --- Shared rate/baseline characterization ------------------------------

    #[test]
    fn rate_first_warms_second_uses_actual_elapsed() {
        use crate::collector::rate::{CounterBaselines, CounterRates};
        use std::time::{Duration, Instant};
        let mut baselines = CounterBaselines::default();
        let start = Instant::now();
        assert_eq!(baselines.observe("eth0", start, 100, 200), None);
        // 250ms interval: 25/50 bytes -> 100/200 per second.
        assert_eq!(
            baselines.observe("eth0", start + Duration::from_millis(250), 125, 250),
            Some(CounterRates {
                first_per_sec: 100,
                second_per_sec: 200,
            })
        );
    }

    #[test]
    fn rate_disappearance_removes_and_reappearance_warms() {
        use crate::collector::rate::CounterBaselines;
        use std::time::{Duration, Instant};
        let mut baselines = CounterBaselines::default();
        let start = Instant::now();
        assert_eq!(baselines.observe("eth0", start, 100, 100), None);
        baselines.retain_ids(["other"]);
        // Identity is gone; reappearance must warm rather than inherit.
        assert_eq!(
            baselines.observe("eth0", start + Duration::from_secs(1), 200, 200),
            None
        );
    }

    #[test]
    fn rate_counter_decrease_rebaselines_without_spike() {
        use crate::collector::rate::CounterBaselines;
        use std::time::{Duration, Instant};
        let mut baselines = CounterBaselines::default();
        let start = Instant::now();
        assert_eq!(baselines.observe("sda", start, 1000, 2000), None);
        assert!(baselines
            .observe("sda", start + Duration::from_secs(1), 1100, 2200)
            .is_some());
        // Decrease (wrap/reset) must omit, not spike.
        assert_eq!(
            baselines.observe("sda", start + Duration::from_secs(2), 50, 60),
            None
        );
        // Next valid interval recovers from the fresh baseline.
        let recovered = baselines
            .observe("sda", start + Duration::from_secs(3), 150, 160)
            .expect("recovers");
        assert_eq!(recovered.first_per_sec, 100);
        assert_eq!(recovered.second_per_sec, 100);
    }

    #[test]
    fn rate_zero_elapsed_rebaselines() {
        use crate::collector::rate::CounterBaselines;
        use std::time::Instant;
        let mut baselines = CounterBaselines::default();
        let start = Instant::now();
        assert_eq!(baselines.observe("x", start, 10, 10), None);
        assert_eq!(baselines.observe("x", start, 11, 11), None);
    }

    #[test]
    fn rate_clear_after_source_failure_forces_fresh_baseline() {
        use crate::collector::rate::CounterBaselines;
        use std::time::{Duration, Instant};
        let mut baselines = CounterBaselines::default();
        let start = Instant::now();
        assert_eq!(baselines.observe("eth0", start, 100, 100), None);
        baselines.clear();
        assert_eq!(
            baselines.observe("eth0", start + Duration::from_secs(1), 200, 200),
            None,
            "cleared baselines must warm on next observation"
        );
    }

    // --- Drive slow-probe characterization ----------------------------------

    #[test]
    fn drive_cache_poll_is_nonblocking_before_first_result() {
        use crate::collector::DriveRefreshCache;
        use std::time::Duration;
        let cache = DriveRefreshCache::new((), |()| {
            std::thread::sleep(Duration::from_millis(200));
            Ok(Vec::new())
        });
        // Must return immediately with None; core sampling never waits.
        let before = std::time::Instant::now();
        let mut cache = cache;
        assert_eq!(cache.poll(), None);
        assert!(
            before.elapsed() < Duration::from_millis(100),
            "drive poll must not block core sampling"
        );
    }

    #[test]
    fn drive_cache_first_request_is_immediate_and_none_until_success() {
        use crate::collector::DriveRefreshCache;
        let mut cache: DriveRefreshCache = DriveRefreshCache::new((), |()| {
            Ok(vec![HostDriveMetrics {
                name: "/".to_string(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: Some(1),
            }])
        });
        // Immediate first request: a result arrives without an explicit
        // follow-up request.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut observed = None;
        while std::time::Instant::now() < deadline {
            if let Some(drives) = cache.poll() {
                observed = Some(drives);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(observed.is_some(), "first drive request must be immediate");
    }

    #[test]
    fn drive_none_vs_empty_stay_distinguishable_at_conversion() {
        // `None` (unavailable) and `Some(empty)` (successful empty) must
        // survive conversion distinctly in the v2 payload.
        let caps_v2 = MetricCapabilitiesV2 {
            cpu_iowait: false,
            load_average: true,
            swap: true,
            memory_commit: false,
        };
        let mut no_drives = ready_metrics_macos_like();
        no_drives.drives = None;
        let payload_none = no_drives
            .into_status_payload_v2(1_716_460_800_000, 1000, caps_v2, test_identity())
            .expect("converts");
        assert_eq!(payload_none.drives, None);

        let mut empty_drives = ready_metrics_macos_like();
        empty_drives.drives = Some(Vec::new());
        let payload_empty = empty_drives
            .into_status_payload_v2(1_716_460_800_000, 1000, caps_v2, test_identity())
            .expect("converts");
        assert_eq!(payload_empty.drives, Some(Vec::new()));
    }

    // --- Wire-equivalence fixtures ------------------------------------------

    #[test]
    fn linux_ready_sample_produces_v1_and_v2_with_expected_shape() {
        let metrics = ready_metrics_linux_like();
        let identity = test_identity();
        let caps = MetricCapabilities { cpu_iowait: true };
        let caps_v2 = MetricCapabilitiesV2 {
            cpu_iowait: true,
            load_average: true,
            swap: true,
            memory_commit: false,
        };
        let (v1, payload) = metrics
            .into_snapshot_pair(
                SCHEMA_VERSION_V1,
                1_716_460_800_000,
                1000,
                caps,
                caps_v2,
                identity,
                true,
            )
            .expect("converts");
        let v1 = v1.expect("linux supports v1");
        v1.validate().expect("v1 validates");
        payload.validate().expect("v2 validates");
        assert!(v1.capabilities.cpu_iowait);
        assert!(v1.cpu.iowait_pct.is_some());
        assert!(payload.snapshot.capabilities.load_average);
        assert!(payload.snapshot.swap.is_some());
        assert!(payload.snapshot.commit.is_none());
        assert_eq!(payload.cpu_frequency_hz, Some(2_400_000_000));
        assert!(payload.disk_io.is_some());
        assert!(payload.network.is_some());
        assert_eq!(payload.drives.as_ref().map(Vec::len), Some(1));
        // JSON field presence: optional families present, commit absent.
        let json = serde_json::to_value(&payload).expect("serializes");
        assert!(json.get("cpu_frequency_hz").is_some());
        assert!(json.get("disk_io").is_some());
        assert!(json.get("network").is_some());
        assert!(json.get("drives").is_some());
    }

    #[test]
    fn macos_ready_sample_has_no_frequency_and_v1_without_iowait() {
        let metrics = ready_metrics_macos_like();
        let identity = test_identity();
        let caps = MetricCapabilities { cpu_iowait: false };
        let caps_v2 = MetricCapabilitiesV2 {
            cpu_iowait: false,
            load_average: true,
            swap: true,
            memory_commit: false,
        };
        let (v1, payload) = metrics
            .into_snapshot_pair(
                SCHEMA_VERSION_V1,
                1_716_460_800_000,
                1000,
                caps,
                caps_v2,
                identity,
                true,
            )
            .expect("converts");
        let v1 = v1.expect("macos supports v1");
        v1.validate().expect("v1 validates");
        payload.validate().expect("v2 validates");
        assert!(v1.cpu.iowait_pct.is_none());
        assert!(payload.snapshot.cpu.iowait_pct.is_none());
        assert_eq!(payload.cpu_frequency_hz, None);
        let json = serde_json::to_value(&payload).expect("serializes");
        // Absent optionals stay absent (skip_serializing_if), never zero.
        assert!(json.get("cpu_frequency_hz").is_none() || json["cpu_frequency_hz"].is_null());
    }

    #[test]
    fn windows_ready_sample_is_v2_only_with_commit() {
        let metrics = ready_metrics_windows_like();
        let identity = test_identity();
        let caps = MetricCapabilities { cpu_iowait: false };
        let caps_v2 = MetricCapabilitiesV2 {
            cpu_iowait: false,
            load_average: false,
            swap: false,
            memory_commit: true,
        };
        let (v1, payload) = metrics
            .into_snapshot_pair(
                SCHEMA_VERSION_V1,
                1_716_460_800_000,
                1000,
                caps,
                caps_v2,
                identity,
                false,
            )
            .expect("converts");
        assert!(v1.is_none(), "windows must not produce v1");
        payload.validate().expect("v2 validates");
        assert!(payload.snapshot.load.is_none());
        assert!(payload.snapshot.swap.is_none());
        assert!(payload.snapshot.commit.is_some());
        assert!(payload.snapshot.cpu.iowait_pct.is_none());
        assert_eq!(payload.drives, Some(Vec::new()));
    }

    #[test]
    fn optional_absence_preserves_core_v2_validity() {
        let mut metrics = ready_metrics_linux_like();
        metrics.cpu_frequency_hz = None;
        metrics.disk_io = None;
        metrics.network = None;
        metrics.drives = None;
        let caps_v2 = MetricCapabilitiesV2 {
            cpu_iowait: true,
            load_average: true,
            swap: true,
            memory_commit: false,
        };
        let payload = metrics
            .into_status_payload_v2(1_716_460_800_000, 1000, caps_v2, test_identity())
            .expect("converts");
        payload.validate().expect("absence still validates");
        assert_eq!(payload.cpu_frequency_hz, None);
        assert_eq!(payload.disk_io, None);
        assert_eq!(payload.network, None);
        assert_eq!(payload.drives, None);
    }

    #[test]
    fn v2_collection_limits_match_protocol_constants() {
        // Frozen wire bounds the host layer must respect exactly.
        assert_eq!(MAX_DISK_IO_ENTRIES, 32);
        assert_eq!(MAX_NETWORK_INTERFACE_ENTRIES, 32);
        assert_eq!(gregg_protocol::v2::MAX_DRIVE_ENTRIES, 32);
        assert_eq!(gregg_protocol::v2::MAX_DRIVE_NAME_BYTES, 512);
        assert_eq!(gregg_protocol::v2::MAX_LIVE_METRIC_ID_BYTES, 512);
        assert_eq!(gregg_protocol::v2::MAX_LIVE_METRIC_NAME_BYTES, 512);
    }

    // --- Sampler readiness interpretation -----------------------------------

    struct ScriptedCollector {
        samples: std::collections::VecDeque<Result<CollectedMetrics, CollectError>>,
    }

    impl ScriptedCollector {
        fn new(samples: Vec<Result<CollectedMetrics, CollectError>>) -> Self {
            Self {
                samples: samples.into(),
            }
        }

        fn ok_metrics() -> CollectedMetrics {
            CollectedMetrics {
                logical_cores: 4,
                cpu_usage_pct: Some(10.0),
                cpu_iowait_pct: None,
                load: LoadAverage {
                    one: 0.5,
                    five: 0.4,
                    fifteen: 0.3,
                },
                memory: MemoryMetrics {
                    used_bytes: 100,
                    total_bytes: 200,
                    usage_pct: 50.0,
                },
                swap: SwapMetrics {
                    used_bytes: 0,
                    total_bytes: 0,
                    usage_pct: 0.0,
                },
                commit: None,
                drives: None,
                cpu_frequency_hz: None,
                disk_io: None,
                network: None,
            }
        }
    }

    impl SystemCollector for ScriptedCollector {
        fn identity(&self) -> Result<SystemIdentity, CollectError> {
            Ok(test_identity())
        }

        fn sample(&mut self) -> Result<CollectedMetrics, CollectError> {
            self.samples.pop_front().unwrap_or_else(|| {
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "exhausted",
                ))
            })
        }

        fn capabilities(&self) -> MetricCapabilities {
            MetricCapabilities { cpu_iowait: false }
        }
    }

    struct FixedClock;

    impl crate::sampler::Clock for FixedClock {
        fn now_unix_ms(&self) -> u64 {
            1_716_460_800_000
        }

        fn sleep(&self, _dur: std::time::Duration) -> crate::sampler::SleepFuture {
            Box::pin(async {})
        }
    }

    #[test]
    fn sampler_warming_and_counter_reset_do_not_fail() {
        use crate::sampler::Sampler;
        use gregg_protocol::ReadinessState;
        let mut sampler = Sampler::new(
            ScriptedCollector::new(vec![
                Err(CollectError::warming("baseline")),
                Ok(ScriptedCollector::ok_metrics()),
                Err(CollectError::counter_reset("reset")),
                Ok(ScriptedCollector::ok_metrics()),
            ]),
            FixedClock,
        );
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Warming);
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
        sampler.sample_once();
        assert_eq!(
            sampler.readiness(),
            ReadinessState::Ready,
            "CounterReset must not fail readiness"
        );
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Ready);
    }

    #[test]
    fn sampler_hard_failure_transitions_to_failed_and_preserves_snapshot() {
        use crate::sampler::Sampler;
        use gregg_protocol::ReadinessState;
        let mut sampler = Sampler::new(
            ScriptedCollector::new(vec![
                Err(CollectError::warming("baseline")),
                Ok(ScriptedCollector::ok_metrics()),
                Err(CollectError::new(
                    CollectErrorKind::SourceUnavailable,
                    "disk gone",
                )),
            ]),
            FixedClock,
        );
        sampler.sample_once();
        sampler.sample_once();
        let published = sampler.snapshot().expect("published");
        sampler.sample_once();
        assert_eq!(sampler.readiness(), ReadinessState::Failed);
        assert_eq!(sampler.snapshot().as_deref(), Some(published.as_ref()));
    }

    // --- Platform sequence characterization (native-gated) -------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_cpu_warmup_reset_recovery_sequence() {
        use crate::collector::linux::{LinuxCollector, MemorySource, ProcSource};
        use std::path::Path;
        fn source_with(stat: &str) -> ProcSource {
            let fixture = |name: &str| {
                std::fs::read_to_string(format!("crates/greggd/src/collector/test_fixtures/{name}"))
                    .unwrap_or_else(|_| {
                        // When tests run with a different CWD, fall back to the
                        // crate-relative loader used by existing linux tests.
                        String::new()
                    })
            };
            let _ = fixture;
            let mut mem = MemorySource::new().with_logical_cores(4);
            mem.add_file(Path::new("/proc/stat"), stat);
            mem.add_file(Path::new("/proc/loadavg"), "0.50 0.40 0.30 1/100 1\n");
            mem.add_file(
                Path::new("/proc/meminfo"),
                "MemTotal:        16000000 kB\nMemAvailable:    10000000 kB\nSwapTotal:        4000000 kB\nSwapFree:         4000000 kB\n",
            );
            mem.add_file(Path::new("/proc/sys/kernel/hostname"), "freeze-host\n");
            mem.add_file(Path::new("/proc/sys/kernel/osrelease"), "6.8.0-freeze\n");
            mem.add_file(Path::new("/proc/sys/kernel/ostype"), "Linux\n");
            ProcSource::for_memory(mem)
        }
        // Minimal synthetic /proc/stat rows with advancing counters.
        // Note: `MemorySource` mutation via `Arc::get_mut` stops working
        // once the first successful sample clones the source into the
        // drive-refresh worker. The reset/recovery sequence below therefore
        // performs both mutations before the first success (warming with
        // high counters, reset with low counters, recovery with high
        // counters) so no worker exists yet during mutation.
        let stat_low = "cpu  100 0 50 8000 30 5 2 1 0 0\n";
        let stat_high = "cpu  150 0 60 8050 50 6 3 1 0 0\n";
        let mut collector =
            LinuxCollector::with_source(source_with(stat_high), None).expect("constructs");
        assert_eq!(
            collector.identity().expect("identity").hostname,
            "freeze-host"
        );
        let err = collector.sample().expect_err("first warms");
        assert_eq!(err.kind, CollectErrorKind::Warming);
        collector
            .source_mut()
            .memory_source_mut()
            .expect("memory source")
            .add_file(Path::new("/proc/stat"), stat_low);
        let err = collector.sample().expect_err("counter decrease resets");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
        collector
            .source_mut()
            .memory_source_mut()
            .expect("memory source")
            .add_file(Path::new("/proc/stat"), stat_high);
        let recovered = collector.sample().expect("recovers without spike");
        let cpu = recovered.cpu_usage_pct.expect("cpu");
        assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
        assert!(recovered.cpu_iowait_pct.is_some());
        // Hotplug uses a fresh collector so drive-worker Arc sharing does
        // not block fixture mutation: core count is re-read every sample.
        let mut hotplug =
            LinuxCollector::with_source(source_with(stat_low), None).expect("constructs");
        let _ = hotplug.sample().expect_err("warming");
        hotplug
            .source_mut()
            .memory_source_mut()
            .expect("memory source")
            .add_file(Path::new("/proc/stat"), stat_high);
        hotplug
            .source_mut()
            .memory_source_mut()
            .expect("memory source")
            .set_logical_cores(6);
        let after_hotplug = hotplug.sample().expect("hotplug sample");
        assert_eq!(after_hotplug.logical_cores, 6);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_optional_failures_preserve_core_sampling() {
        use crate::collector::linux::{LinuxCollector, MemorySource, ProcSource};
        use std::path::Path;
        // No /sys/block, /proc/net/dev, cpufreq, or mountinfo fixtures:
        // every optional family is unavailable, core must still succeed.
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
        let err = collector.sample().expect_err("warming");
        assert_eq!(err.kind, CollectErrorKind::Warming);
        collector
            .source_mut()
            .memory_source_mut()
            .expect("memory source")
            .add_file(Path::new("/proc/stat"), "cpu  150 0 60 8050 50 6 3 1 0 0\n");
        let metrics = collector.sample().expect("core succeeds");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.memory.total_bytes > 0);
        assert_eq!(metrics.cpu_frequency_hz, None);
        assert_eq!(metrics.disk_io, None);
        assert_eq!(metrics.network, None);
        assert_eq!(metrics.drives, None);
        assert!(collector.capabilities().cpu_iowait);
        let caps_v2 = collector.capabilities_v2();
        assert!(caps_v2.cpu_iowait && caps_v2.load_average && caps_v2.swap);
        assert!(!caps_v2.memory_commit);
        assert!(collector.supports_v1_snapshot());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_cpu_warmup_reset_recovery_sequence() {
        use crate::collector::macos::ffi::{MockNativeQueries, RawCpuTicks};
        use crate::collector::macos::MacOsCollector;
        let mut mock = MockNativeQueries::success();
        mock.auto_increment_cpu = true;
        let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
        assert_eq!(
            collector.identity().expect("identity").hostname,
            "test-mac.local"
        );
        let err = collector.sample().expect_err("first warms");
        assert_eq!(err.kind, CollectErrorKind::Warming);
        let metrics = collector.sample().expect("second yields cpu");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.cpu_iowait_pct.is_none());
        assert_eq!(metrics.cpu_frequency_hz, None);
        *collector.source_mut() = MockNativeQueries {
            cpu: RawCpuTicks {
                user: 10,
                system: 5,
                idle: 80,
                nice: 1,
            },
            ..MockNativeQueries::success()
        };
        let err = collector.sample().expect_err("reset");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
        *collector.source_mut() = MockNativeQueries {
            cpu: RawCpuTicks {
                user: 110,
                system: 55,
                idle: 580,
                nice: 11,
            },
            ..MockNativeQueries::success()
        };
        let recovered = collector.sample().expect("recovers");
        let cpu = recovered.cpu_usage_pct.expect("cpu");
        assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
        assert!(!collector.capabilities().cpu_iowait);
        assert!(collector.supports_v1_snapshot());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_optional_failures_preserve_core_sampling() {
        use crate::collector::macos::ffi::MockNativeQueries;
        use crate::collector::macos::MacOsCollector;
        let mut mock = MockNativeQueries::success();
        mock.auto_increment_cpu = true;
        mock.mounted_error = true;
        mock.mounted.clear();
        mock.disk.clear();
        mock.network.clear();
        let mut collector = MacOsCollector::with_source(mock, None).expect("constructs");
        let _ = collector.sample().expect_err("warming");
        let metrics = collector.sample().expect("core succeeds");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.memory.total_bytes > 0);
        assert_eq!(metrics.drives, None);
        assert_eq!(metrics.disk_io, None);
        assert_eq!(metrics.network, None);
        assert_eq!(metrics.cpu_frequency_hz, None);
        let caps_v2 = collector.capabilities_v2();
        assert!(!caps_v2.cpu_iowait && caps_v2.load_average && caps_v2.swap);
        assert!(!caps_v2.memory_commit);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_cpu_warmup_reset_recovery_sequence() {
        use crate::collector::windows::source::{MockWindowsSource, RawCpuTimes};
        use crate::collector::windows::WindowsCollector;
        let mut mock = MockWindowsSource::success();
        mock.auto_increment_cpu = true;
        let mut collector = WindowsCollector::with_source(mock, None).expect("constructs");
        let err = collector.sample().expect_err("first warms");
        assert_eq!(err.kind, CollectErrorKind::Warming);
        let metrics = collector.sample().expect("second yields cpu");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.cpu_iowait_pct.is_none());
        assert!(metrics.commit.is_some());
        // Counter decrease -> CounterReset, then recovery without spike.
        collector.source_mut().auto_increment_cpu = false;
        collector.source_mut().cpu = RawCpuTimes {
            idle: 10,
            kernel: 20,
            user: 5,
        };
        let err = collector.sample().expect_err("reset");
        assert_eq!(err.kind, CollectErrorKind::CounterReset);
        collector.source_mut().cpu = RawCpuTimes {
            idle: 110,
            kernel: 220,
            user: 105,
        };
        let recovered = collector.sample().expect("recovers");
        let cpu = recovered.cpu_usage_pct.expect("cpu");
        assert!(cpu.is_finite() && (0.0..=100.0).contains(&cpu));
        assert!(!collector.supports_v1_snapshot());
        let caps_v2 = collector.capabilities_v2();
        assert!(!caps_v2.cpu_iowait && !caps_v2.load_average && !caps_v2.swap);
        assert!(caps_v2.memory_commit);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_optional_failures_preserve_core_sampling() {
        use crate::collector::windows::source::MockWindowsSource;
        use crate::collector::windows::WindowsCollector;
        let mut mock = MockWindowsSource::success();
        mock.auto_increment_cpu = true;
        mock.drives_error = true;
        mock.disk.clear();
        mock.network.clear();
        mock.cpu_frequency = None;
        let mut collector = WindowsCollector::with_source(mock, None).expect("constructs");
        let _ = collector.sample().expect_err("warming");
        let metrics = collector.sample().expect("core succeeds");
        assert!(metrics.cpu_usage_pct.is_some());
        assert!(metrics.memory.total_bytes > 0);
        assert!(metrics.commit.is_some());
        assert_eq!(metrics.drives, None);
        assert_eq!(metrics.disk_io, None);
        assert_eq!(metrics.network, None);
        assert_eq!(metrics.cpu_frequency_hz, None);
    }

    // --- Cross-platform CPU math freeze (runs everywhere) --------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn frozen_cpu_math_spot_checks() {
        // Linux hand-calculated interval from existing characterization.
        use crate::collector::linux::{compute_percentages, CpuCounters};
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
        assert!((sample.usage_pct - 46.969_7).abs() < 1e-3);
        assert!((sample.iowait_pct - 15.151_5).abs() < 1e-3);
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn frozen_percentage_helpers_reject_nonfinite() {
        use crate::collector::{clamped_usage_pct, finalize_percentage};
        assert_eq!(clamped_usage_pct(0, 0), 0.0);
        assert_eq!(clamped_usage_pct(u64::MAX, u64::MAX), 100.0);
        assert!(finalize_percentage(f64::NAN).is_err());
        assert!(finalize_percentage(f64::INFINITY).is_err());
    }

    #[test]
    fn frozen_error_taxonomy_meanings() {
        assert_eq!(CollectError::warming("x").kind, CollectErrorKind::Warming);
        assert_eq!(
            CollectError::counter_reset("x").kind,
            CollectErrorKind::CounterReset
        );
        // IdentityFallback remains reserved; display stays stable.
        assert_eq!(format!("{}", CollectErrorKind::Warming), "warming");
        assert_eq!(
            format!("{}", CollectErrorKind::SourceUnavailable),
            "source unavailable"
        );
        assert_eq!(format!("{}", CollectErrorKind::Parse), "parse failure");
        assert_eq!(
            format!("{}", CollectErrorKind::CounterReset),
            "counter reset"
        );
        assert_eq!(format!("{}", CollectErrorKind::Numeric), "numeric failure");
        assert_eq!(
            format!("{}", CollectErrorKind::IdentityFallback),
            "identity fallback"
        );
    }

    #[test]
    fn frozen_drive_normalization_bounds_and_order() {
        use crate::collector::drives::{normalize, DriveCandidate};
        let normalized = normalize(vec![
            DriveCandidate {
                identity: "b".to_string(),
                name: "/b".to_string(),
                total_bytes: 10,
                total_free_bytes: 2,
                available_bytes: 2,
            },
            DriveCandidate {
                identity: "a".to_string(),
                name: "/a".to_string(),
                total_bytes: 10,
                total_free_bytes: 3,
                available_bytes: 3,
            },
        ]);
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].name, "/a");
        assert_eq!(normalized[1].name, "/b");
        assert_eq!(normalized[0].used_bytes, 7);
    }
}
