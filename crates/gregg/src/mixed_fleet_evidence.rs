//! Deterministic mixed-fleet evidence using the production poller and reducer.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::clock::RealClock;
use crate::config::{Config, SystemEntry};
use crate::endpoint::Endpoint;
use crate::poller::PollOutcome;
use crate::scheduler::PollScheduler;
use crate::state::{AppState, Reachability};

struct FixtureProcess {
    child: Child,
    log_path: PathBuf,
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log_path);
    }
}

fn start_fixture(mode: &str) -> (FixtureProcess, u16) {
    let log_path =
        std::env::temp_dir().join(format!("gregg-fleet-{}-{mode}.jsonl", std::process::id()));
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/tests/fleet-fixture.py");
    let mut child = Command::new("python3")
        .arg(script)
        .arg("--port")
        .arg("0")
        .arg("--log")
        .arg(&log_path)
        .arg("--default-mode")
        .arg(mode)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start deterministic fleet fixture");
    let port = {
        use std::io::BufRead;
        let stdout = child.stdout.as_mut().expect("fixture stdout piped");
        let reader = std::io::BufReader::new(stdout);
        let mut port: Option<u16> = None;
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(rest) = line.strip_prefix("PORT=") {
                if let Ok(p) = rest.parse::<u16>() {
                    port = Some(p);
                    break;
                }
            }
        }
        if let Some(p) = port {
            p
        } else {
            let stderr = child
                .stderr
                .take()
                .map(|mut s| {
                    use std::io::Read;
                    let mut buf = String::new();
                    let _ = s.read_to_string(&mut buf);
                    buf
                })
                .unwrap_or_default();
            panic!("fleet fixture {mode} did not report a port: {stderr}");
        }
    };
    let mut fixture = FixtureProcess { child, log_path };
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..200 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(10)).is_ok() {
            return (fixture, port);
        }
        std::thread::sleep(Duration::from_millis(10));
        if let Ok(Some(status)) = fixture.child.try_wait() {
            panic!("fleet fixture {mode} exited before readiness: {status}");
        }
    }
    panic!("fleet fixture {mode} did not become ready");
}

fn endpoint(id: &str, port: u16, name: &str) -> Endpoint {
    Endpoint {
        id: id.to_string(),
        host: "127.0.0.1".to_string(),
        port,
        name: Some(name.to_string()),
    }
}

#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn production_state_engine_tracks_mixed_fleet_and_recovery() {
    let modes = [
        "healthy",
        "slow",
        "timeout",
        "malformed",
        "error",
        "stale",
        "recover",
        "offline",
        "healthy-to-failure",
    ];
    let mut fixtures = Vec::new();
    let mut endpoints = Vec::new();
    for mode in modes {
        let (fixture, port) = start_fixture(mode);
        fixtures.push(fixture);
        endpoints.push(endpoint(mode, port, mode));
    }
    // Port 9 is intentionally not a fixture: it exercises connection refusal.
    endpoints.push(endpoint("refused", 9, "refused"));

    let config = Config {
        request_timeout_ms: 1000,
        max_concurrent_requests: 4,
        systems: endpoints
            .iter()
            .map(|item| SystemEntry {
                id: item.id.clone(),
                host: item.host.clone(),
                port: item.port,
                name: item.name.clone(),
            })
            .collect(),
        ..Config::default()
    };
    let mut state = AppState::from_config(&config);
    let cancel = CancellationToken::new();
    let (refresh_tx, refresh_rx) = mpsc::channel(2);
    let scheduler = PollScheduler::new(
        RealClock,
        crate::poller::HttpClient::new(Duration::from_secs(1)),
        Duration::from_secs(60),
        4,
    );
    let mut batches = scheduler.run(endpoints, cancel.clone(), refresh_rx);

    let first = timeout(Duration::from_secs(5), batches.recv())
        .await
        .expect("first mixed-fleet generation timed out")
        .expect("scheduler closed before first generation");
    assert_eq!(first.results.len(), modes.len() + 1);
    assert!(first.completed_at.duration_since(first.started_at) < Duration::from_secs(3));
    let first_outcomes: HashMap<_, _> = first
        .results
        .iter()
        .map(|result| (result.system_id.as_str(), &result.outcome))
        .collect();
    assert!(matches!(first_outcomes["healthy"], PollOutcome::Online(_)));
    assert!(matches!(first_outcomes["slow"], PollOutcome::Online(_)));
    assert!(matches!(first_outcomes["timeout"], PollOutcome::Timeout));
    assert!(matches!(
        first_outcomes["malformed"],
        PollOutcome::DecodeError
    ));
    assert!(matches!(
        first_outcomes["error"],
        PollOutcome::HttpStatus(500)
    ));
    assert!(matches!(first_outcomes["stale"], PollOutcome::Online(_)));
    assert!(matches!(
        first_outcomes["recover"],
        PollOutcome::HttpStatus(503)
    ));
    assert!(matches!(
        first_outcomes["offline"],
        PollOutcome::HttpStatus(503)
    ));
    assert!(matches!(
        first_outcomes["refused"],
        PollOutcome::ConnectionRefused
    ));
    println!(
        "fleet-transition generation=1 healthy=online slow=online timeout=offline malformed=offline error=offline stale=online recover=offline offline=offline refused=offline"
    );
    state.apply_batch(&first);
    assert_eq!(state.systems.len(), modes.len() + 1);
    assert_eq!(state.last_applied_generation, 1);
    for id in ["healthy", "slow", "stale"] {
        assert_eq!(
            state
                .systems
                .iter()
                .find(|item| item.id == id)
                .unwrap()
                .reachability,
            Reachability::Online
        );
    }
    for id in [
        "timeout",
        "malformed",
        "error",
        "recover",
        "offline",
        "refused",
    ] {
        assert_eq!(
            state
                .systems
                .iter()
                .find(|item| item.id == id)
                .unwrap()
                .reachability,
            Reachability::Offline
        );
    }

    refresh_tx.send(()).await.expect("send recovery refresh");
    let second = timeout(Duration::from_secs(5), batches.recv())
        .await
        .expect("recovery generation timed out")
        .expect("scheduler closed before recovery generation");
    state.apply_batch(&second);
    assert_eq!(state.last_applied_generation, 2);
    assert_eq!(
        state
            .systems
            .iter()
            .find(|item| item.id == "recover")
            .unwrap()
            .reachability,
        Reachability::Online
    );
    println!("fleet-transition generation=2 recover=online");

    // The healthy-to-failure endpoint was online in generation 1 and should
    // now be offline after transitioning to HTTP 500 on the second call.
    assert_eq!(
        state
            .systems
            .iter()
            .find(|item| item.id == "healthy-to-failure")
            .unwrap()
            .reachability,
        Reachability::Offline
    );
    println!("fleet-transition generation=2 healthy-to-failure=offline");

    cancel.cancel();
    drop(refresh_tx);
    drop(batches);
    drop(fixtures);
}
