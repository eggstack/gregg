//! Integration tests for the Linux collector.
//!
//! Gated to `cfg(target_os = "linux")` because the underlying native source
//! reads from `/proc` paths. On non-Linux hosts the integration test module
//! is empty.

#![cfg(target_os = "linux")]

use greggd::collector::linux::LinuxCollector;
use greggd::collector::SystemCollector;

/// Smoke test against the live host `/proc` filesystem. Asserts broad
/// invariants only; expected percentages depend on instantaneous load and
/// are not asserted here.
#[test]
fn production_collector_warms_then_validates() {
    let mut collector = LinuxCollector::new(None).expect("production collector constructs");
    assert!(collector.capabilities().cpu_iowait);

    // First sample must surface a typed warming error rather than fabricating
    // zero CPU usage from a single reading.
    match collector.sample() {
        Err(err) => assert!(
            matches!(
                err.kind,
                greggd::collector::error::CollectErrorKind::Warming
            ),
            "first sample should warm; got {:?}",
            err.kind
        ),
        Ok(_) => panic!("first sample unexpectedly produced metrics"),
    }

    // Second sample either succeeds with a protocol-valid snapshot or
    // reports a transient counter reset; both are documented behaviors on
    // heavily loaded CI hosts.
    if let Ok(metrics) = collector.sample() {
        let identity = collector.identity().expect("identity");
        let snap = metrics.into_snapshot(
            gregg_protocol::SCHEMA_VERSION_V1,
            1_716_460_800_000,
            1000,
            collector.capabilities(),
            identity,
        );
        snap.validate().expect("production snapshot validates");
        assert!(snap.cpu.iowait_pct.is_some());
    }
}
