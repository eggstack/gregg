//! Sustained workload driver for mixed-fleet release evidence.
//!
//! Exercises the production `PollScheduler`, `HttpClient`, and endpoint
//! state reduction for a configured monotonic duration. Writes a
//! machine-readable summary when the duration completes.
//!
//! This module is `#[cfg(test)]`-only and is invoked as an ignored test
//! by the external runner script.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::clock::RealClock;
use crate::config::{Config, SystemEntry};
use crate::endpoint::Endpoint;
use crate::poller::{HttpClient, PollOutcome};
use crate::scheduler::PollScheduler;
use crate::state::AppState;

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
    let log_path = std::env::temp_dir().join(format!(
        "gregg-sustained-{}-{mode}.jsonl",
        std::process::id()
    ));
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

fn endpoint_for(id: &str, port: u16, name: &str) -> Endpoint {
    Endpoint {
        id: id.to_string(),
        host: "127.0.0.1".to_string(),
        port,
        name: Some(name.to_string()),
    }
}

#[derive(Debug, Serialize)]
struct SustainedSummary {
    requested_duration_secs: u64,
    observed_duration_secs: f64,
    endpoint_count: usize,
    completed_generations: u64,
    first_generation: u64,
    last_generation: u64,
    max_concurrent_polls: usize,
    online_results: u64,
    offline_results: u64,
    observed_transitions: Vec<String>,
    clean_shutdown: bool,
    panic_or_join_failure: Option<String>,
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "current_thread")]
#[ignore = "sustained workload driver — run via scripts/run-mixed-fleet-sustained.py"]
async fn mixed_fleet_sustained_workload() {
    let requested_secs: u64 = std::env::var("GREGG_SUSTAINED_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let summary_path = std::env::var("GREGG_SUSTAINED_SUMMARY")
        .ok()
        .map(PathBuf::from);

    // The `timeout` fixture sleeps 3 seconds, which exceeds short smoke-test
    // durations.  Exclude it when the requested duration is too brief for it
    // to contribute at least one result.
    let mut modes: Vec<&str> = vec![
        "healthy",
        "slow",
        "malformed",
        "error",
        "stale",
        "recover",
        "offline",
        "healthy-to-failure",
    ];
    if requested_secs >= 5 {
        modes.insert(2, "timeout");
    }
    let mut fixtures = Vec::new();
    let mut endpoints = Vec::new();
    for &mode in &modes {
        let (fixture, port) = start_fixture(mode);
        fixtures.push(fixture);
        endpoints.push(endpoint_for(mode, port, mode));
    }
    // Port 9 is intentionally unreachable: exercises connection refusal.
    endpoints.push(endpoint_for("refused", 9, "refused"));

    let endpoint_count = endpoints.len();

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
    let (_refresh_tx, refresh_rx) = mpsc::channel(2);
    let scheduler = PollScheduler::new(
        RealClock,
        HttpClient::new(Duration::from_secs(2)),
        Duration::from_millis(200),
        4,
    );
    let mut batches = scheduler.run(endpoints, cancel.clone(), refresh_rx);

    let workload_start = Instant::now();
    let mut completed_generations: u64 = 0;
    let mut first_generation: u64 = 0;
    let mut last_generation: u64 = 0;
    let mut max_concurrent_polls: usize = 0;
    let mut online_results: u64 = 0;
    let mut offline_results: u64 = 0;
    let mut observed_transitions: Vec<String> = Vec::new();
    let mut seen_online = HashMap::<String, bool>::new();
    let mut clean_shutdown = true;
    let mut panic_or_join_failure: Option<String> = None;

    let deadline = workload_start + Duration::from_secs(requested_secs);

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }

        match timeout(remaining, batches.recv()).await {
            Ok(Some(batch)) => {
                let gen = batch.generation;
                if first_generation == 0 {
                    first_generation = gen;
                }
                completed_generations += 1;

                let concurrent = batch.results.len();
                if concurrent > max_concurrent_polls {
                    max_concurrent_polls = concurrent;
                }

                // Track per-endpoint outcomes for transitions.
                for result in &batch.results {
                    let _was_online = matches!(seen_online.get(&result.system_id), Some(true));
                    let is_online = matches!(&result.outcome, PollOutcome::Online(_));

                    if is_online {
                        online_results += 1;
                    } else {
                        offline_results += 1;
                    }

                    if let Some(prev) = seen_online.get(&result.system_id) {
                        if *prev != is_online {
                            let transition = if is_online {
                                format!("{}:offline->online", result.system_id)
                            } else {
                                format!("{}:online->offline", result.system_id)
                            };
                            observed_transitions.push(transition);
                        }
                    }
                    seen_online.insert(result.system_id.clone(), is_online);
                }

                state.apply_batch(&batch);

                // Verify every generation has exactly one result per endpoint.
                assert_eq!(
                    batch.results.len(),
                    endpoint_count,
                    "generation {gen}: expected {endpoint_count} results, got {}",
                    batch.results.len()
                );

                // Verify generation numbers are strictly increasing.
                assert!(
                    gen > last_generation || completed_generations == 1,
                    "generation numbers must be strictly increasing"
                );
                last_generation = gen;
            }
            Ok(None) => {
                clean_shutdown = false;
                panic_or_join_failure = Some("scheduler channel closed unexpectedly".to_string());
                break;
            }
            Err(_) => {
                // Timeout — duration elapsed.
                break;
            }
        }
    }

    cancel.cancel();

    // Verify at least one online and one offline/error result.
    assert!(
        online_results > 0,
        "must have at least one online result during sustained run"
    );
    assert!(
        offline_results > 0,
        "must have at least one offline/error result during sustained run"
    );

    // Verify at least one state transition was observed.
    assert!(
        !observed_transitions.is_empty(),
        "must observe at least one endpoint state transition"
    );

    // Verify online/offline transition is observed.
    let has_online_offline = observed_transitions
        .iter()
        .any(|t| t.contains("online->offline"));
    let has_offline_online = observed_transitions
        .iter()
        .any(|t| t.contains("offline->online"));
    assert!(
        has_online_offline || has_offline_online,
        "must observe at least one online<->offline transition"
    );

    let observed_duration = workload_start.elapsed().as_secs_f64();

    let summary = SustainedSummary {
        requested_duration_secs: requested_secs,
        observed_duration_secs: observed_duration,
        endpoint_count,
        completed_generations,
        first_generation,
        last_generation,
        max_concurrent_polls,
        online_results,
        offline_results,
        observed_transitions,
        clean_shutdown,
        panic_or_join_failure,
    };

    if let Some(path) = summary_path {
        let json = serde_json::to_string_pretty(&summary).expect("summary serializes");
        std::fs::write(&path, json).expect("write sustained summary");
        eprintln!("sustained summary written to {}", path.display());
    }

    eprintln!("{summary:#?}");
}
