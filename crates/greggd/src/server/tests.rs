use super::*;
use eggserve_primitives::canonical::{ResponseBody, StatusCode};
use eggserve_primitives::connection_info::{ConnectionInfo, Scheme};
use eggserve_primitives::header_block::HeaderBlock;
use eggserve_primitives::method::Method;
use eggserve_primitives::request::Request;
use eggserve_primitives::request_body::RequestBody;
use eggserve_primitives::request_head::RequestHead;
use eggserve_primitives::request_target::RequestTarget;
use eggserve_primitives::version::HttpVersion;
use futures_util::StreamExt;
use gregg_protocol::test_support::{
    LinuxSnapshotBuilder, LinuxSnapshotV2Builder, WindowsSnapshotV2Builder,
};
use gregg_protocol::v2::{DriveMetrics, HealthResponseV2};
use gregg_protocol::{HealthCategory, ReadinessState, StatusSnapshot};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[test]
fn eggserve_runtime_keeps_bounded_lifetime_and_explicit_bounds() {
    let config = runtime_config().unwrap();

    assert_eq!(config.connection_total_timeout, Duration::from_secs(300));
    assert_eq!(config.max_connections, 512);
    assert_eq!(config.max_in_flight_requests, 512);
    assert_eq!(config.max_headers, 100);
    assert_eq!(config.max_buf_size, 417_792);
    assert_eq!(config.max_header_bytes, 417_792);
    assert_eq!(config.max_request_target_bytes, 65_536);
    assert_eq!(config.max_request_body_bytes, MAX_REQUEST_BODY_BYTES);
    assert_eq!(config.max_requests_per_connection, Some(1000));
    assert_eq!(config.graceful_shutdown_timeout, Duration::from_secs(8));
    assert_eq!(config.header_read_timeout, Duration::from_secs(10));
    assert_eq!(config.handler_timeout, Duration::from_secs(30));
    assert_eq!(config.body_read_timeout, Duration::from_secs(30));
    assert_eq!(config.keep_alive_idle_timeout, Duration::from_secs(60));
    assert_eq!(config.response_write_timeout, Duration::from_secs(30));
    assert_eq!(config.response_policy.server_identification, None);
}

#[derive(Clone)]
struct TestRequest {
    method: String,
    target: String,
}

struct TestResponse {
    status: StatusCode,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl TestResponse {
    fn status(&self) -> StatusCode {
        self.status
    }

    fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }
}

fn request(method: &str, target: &str) -> TestRequest {
    TestRequest {
        method: method.to_owned(),
        target: target.to_owned(),
    }
}

fn get(path: &str) -> TestRequest {
    request("GET", path)
}

fn post(path: &str) -> TestRequest {
    request("POST", path)
}

async fn call(state: &ServerState, request: TestRequest) -> TestResponse {
    let method = Method::new(request.method).unwrap();
    let target = RequestTarget::parse(request.target).unwrap();
    let head = RequestHead::new(method, target, HttpVersion::Http11, HeaderBlock::new());
    let connection = ConnectionInfo::with_socket_addrs(
        "127.0.0.1:11310".parse().unwrap(),
        "127.0.0.1:12345".parse().unwrap(),
        Scheme::Http,
        None,
    );
    let request = Request::new(head, RequestBody::empty(), connection);
    let service = http_service(state.clone());
    let mut response = service.call(request).await.unwrap();
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|header| {
            (
                header.name.as_str().to_ascii_lowercase(),
                header.value.to_str().unwrap().to_owned(),
            )
        })
        .collect();
    let body = match response.take_body().unwrap_or(ResponseBody::Empty) {
        ResponseBody::Empty | ResponseBody::EmptyWithLength(_) => Vec::new(),
        ResponseBody::Bytes(bytes) => bytes,
        ResponseBody::Stream(stream) => {
            let (mut stream, _) = stream.into_parts();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                bytes.extend_from_slice(&chunk.unwrap());
            }
            bytes
        }
        ResponseBody::File(_) => panic!("Gregg service does not produce file bodies"),
    };
    TestResponse {
        status,
        headers,
        body,
    }
}

fn response_body_string(response: TestResponse) -> String {
    String::from_utf8(response.body).unwrap()
}

/// Test helper: update snapshot with both v1 and v2 from a v1 snapshot.
async fn update_both(state: &ServerState, snap: StatusSnapshot) {
    let snap_v2 = LinuxSnapshotV2Builder::default().build_payload();
    state.update_snapshot(snap, snap_v2).await;
}

async fn read_raw_response(
    reader: &mut BufReader<TcpStream>,
) -> (String, std::collections::HashMap<String, String>, Vec<u8>) {
    let mut status_line = Vec::new();
    reader.read_until(b'\n', &mut status_line).await.unwrap();
    let status_line = String::from_utf8(status_line).unwrap();
    let mut headers = std::collections::HashMap::new();
    loop {
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).await.unwrap();
        if line == b"\r\n" || line.is_empty() {
            break;
        }
        let line = String::from_utf8(line).unwrap();
        let (name, value) = line.trim_end().split_once(':').unwrap();
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
    let body_len = headers
        .get("content-length")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut body = vec![0; body_len];
    reader.read_exact(&mut body).await.unwrap();
    (status_line, headers, body)
}

fn parse_raw_response(
    response: &[u8],
) -> (String, std::collections::HashMap<String, String>, &[u8]) {
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let (head, body) = response.split_at(separator);
    let body = &body[4..];
    let head = std::str::from_utf8(head).unwrap();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap().to_owned();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_ascii_lowercase(), value.trim().to_owned())
        })
        .collect();
    (status_line, headers, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)]
async fn raw_wire_contract_preserved_by_eggserve_transport() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (control, mut completion) = start(listener, state).await.unwrap();

    let routes = [
        "/",
        "/v1/status",
        "/v2/status",
        "/healthz",
        "/v2/healthz",
        "/v2/scheduler",
        "/v2/scheduler/history",
    ];
    let mut get_lengths = std::collections::HashMap::new();
    for route in routes {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!("GET {route} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let (status_line, headers, body) = parse_raw_response(&response);
        assert!(status_line.contains("200"), "{route}: {status_line}");
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        assert_eq!(
            headers
                .get("content-length")
                .unwrap()
                .parse::<usize>()
                .unwrap(),
            body.len()
        );
        assert!(!headers.contains_key("transfer-encoding"));
        assert!(headers.contains_key("date"));
        assert!(!headers.contains_key("server"));
        get_lengths.insert(route, body.len());
    }

    for route in routes {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!("HEAD {route} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let (status_line, headers, body) = parse_raw_response(&response);
        assert!(status_line.contains("200"), "{route}: {status_line}");
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        assert_eq!(
            headers
                .get("content-length")
                .unwrap()
                .parse::<usize>()
                .unwrap(),
            get_lengths[route]
        );
        assert!(body.is_empty(), "HEAD {route} returned a body");
    }

    for (method, route, expected_status, expected_body) in [
        ("POST", "/v1/status", "405", ""),
        // The scheduler routes are read-only: a mutating verb is rejected
        // exactly like every other known route.
        ("POST", "/v2/scheduler", "405", ""),
        ("DELETE", "/v2/scheduler/history", "405", ""),
        // Nothing under the scheduler prefix is a control plane.
        (
            "GET",
            "/v2/scheduler/run",
            "404",
            "GET /v2/scheduler/run not found",
        ),
        ("GET", "/unknown", "404", "GET /unknown not found"),
        ("BREW", "/unknown", "404", "BREW /unknown not found"),
    ] {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!(
                    "{method} {route} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let (status_line, headers, body) = parse_raw_response(&response);
        assert!(status_line.contains(expected_status), "{status_line}");
        assert_eq!(body, expected_body.as_bytes());
        assert_eq!(
            headers
                .get("content-length")
                .unwrap()
                .parse::<usize>()
                .unwrap(),
            body.len()
        );
        if expected_status == "405" {
            assert_eq!(headers.get("allow").map(String::as_str), Some("GET,HEAD"));
            assert_eq!(headers.get("content-length").map(String::as_str), Some("0"));
        } else {
            assert_eq!(
                headers.get("content-type").map(String::as_str),
                Some("text/plain; charset=utf-8")
            );
        }
    }

    let mut body_request = TcpStream::connect(addr).await.unwrap();
    body_request
        .write_all(
            b"GET /v1/status HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    body_request.read_to_end(&mut response).await.unwrap();
    let (status_line, _, _) = parse_raw_response(&response);
    assert!(status_line.contains("200"), "GET with body: {status_line}");

    let stream = TcpStream::connect(addr).await.unwrap();
    let mut stream = BufReader::new(stream);
    for route in ["/v1/status", "/v2/status"] {
        stream
            .get_mut()
            .write_all(format!("GET {route} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let (status_line, headers, _) = read_raw_response(&mut stream).await;
        assert!(
            status_line.contains("200"),
            "keep-alive {route}: {status_line}"
        );
        assert!(!headers.contains_key("connection"));
    }

    control.shutdown();
    completion.wait().await.unwrap();
}

// ===== State Tests =====

#[tokio::test]
async fn new_starts_in_warming_state() {
    let state = ServerState::new();
    assert_eq!(state.health().await.state, ReadinessState::Warming);
    assert!(state.snapshot().await.is_none());
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Warming);
}

#[tokio::test]
async fn update_snapshot_makes_ready() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap.clone()).await;

    assert_eq!(state.health().await.state, ReadinessState::Ready);
    let stored = state.snapshot().await.unwrap();
    assert_eq!(*stored, snap);
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Ready);
}

#[tokio::test]
async fn internal_publication_retains_sampler_arc_identity() {
    let state = ServerState::new();
    let snapshot = Arc::new(LinuxSnapshotBuilder::default().build());
    let payload = Arc::new(LinuxSnapshotV2Builder::default().build_payload());

    state
        .update_snapshot_arcs(Arc::clone(&snapshot), Arc::clone(&payload))
        .await;

    assert!(Arc::ptr_eq(&state.snapshot().await.unwrap(), &snapshot));
    assert!(Arc::ptr_eq(&state.snapshot_v2().await.unwrap(), &payload));
}

#[tokio::test]
async fn status_serialization_is_cached_per_publication() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .v2_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    let app = state.clone();
    for _ in 0..10 {
        let response = call(&app, get("/v1/status")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(&app, get("/v2/status")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .v2_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(2)
            .build(),
    )
    .await;
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        state
            .v2_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[tokio::test]
async fn v2_only_publication_does_not_prepare_v1_bytes() {
    let state = ServerState::new();
    state
        .update_snapshot_v2_only(WindowsSnapshotV2Builder::default().build_payload())
        .await;
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        state
            .v2_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn missing_status_cache_falls_back_to_on_demand_serialization() {
    let state = ServerState::new();
    let snapshot = LinuxSnapshotBuilder::default().build();
    update_both(&state, snapshot.clone()).await;
    state.published.write().await.status_bytes = None;

    let response = call(&state, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let parsed: StatusSnapshot = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(parsed, snapshot);
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[tokio::test]
async fn set_warming_clears_snapshot() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    state.set_warming().await;
    assert_eq!(state.health().await.state, ReadinessState::Warming);
    assert!(state.snapshot().await.is_none());
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Warming);
}

#[tokio::test]
async fn set_failed_preserves_snapshot() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap.clone()).await;

    state.set_failed("collector crashed").await;

    assert_eq!(state.health().await.state, ReadinessState::Failed);
    // Snapshot is preserved for stale-serving.
    let stored = state.snapshot().await.unwrap();
    assert_eq!(*stored, snap);
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Failed);
    assert_eq!(health.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(health.message.as_deref(), Some("collector crashed"));
    assert_eq!(state.consecutive_failures().await, 1);
}

#[tokio::test]
async fn v1_only_failure_preserves_v2_not_serving_semantics() {
    let state = ServerState::new();
    state
        .update_snapshot_v1_only(LinuxSnapshotBuilder::default().build())
        .await;

    state.set_failed("collector crashed").await;

    assert_eq!(
        state.health_v2().await.category,
        Some(HealthCategory::NotServing)
    );
}

// ===== Config Validation Tests =====

#[test]
fn default_config_is_valid() {
    assert!(Config::default().validate().is_ok());
}

#[test]
fn port_zero_is_invalid() {
    let config = Config {
        port: 0,
        ..Config::default()
    };
    assert_eq!(
        config.validate().unwrap_err(),
        ServerConfigError::InvalidPort(0)
    );
}

#[test]
fn port_65535_is_valid() {
    let config = Config {
        port: 65535,
        ..Config::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn sample_interval_249_is_invalid() {
    let config = Config {
        sample_interval_ms: 249,
        ..Config::default()
    };
    assert_eq!(
        config.validate().unwrap_err(),
        ServerConfigError::InvalidSampleInterval(249)
    );
}

#[test]
fn sample_interval_250_is_valid() {
    let config = Config {
        sample_interval_ms: 250,
        ..Config::default()
    };
    assert!(config.validate().is_ok());
}

#[test]
fn sample_interval_60001_is_invalid() {
    let config = Config {
        sample_interval_ms: 60001,
        ..Config::default()
    };
    assert_eq!(
        config.validate().unwrap_err(),
        ServerConfigError::InvalidSampleInterval(60001)
    );
}

// ===== HTTP Handler Tests =====

#[tokio::test]
async fn status_ready_returns_200_with_json() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap.clone()).await;

    let app = state.clone();
    let response = call(&app, get("/v1/status")).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );

    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
}

#[tokio::test]
async fn v2_status_serializes_synthetic_drives_without_changing_v1() {
    let state = ServerState::new();
    let v1 = LinuxSnapshotBuilder::default().build();
    let payload = LinuxSnapshotV2Builder::default()
        .drives(Some(vec![DriveMetrics {
            name: "/".into(),
            used_bytes: 4,
            total_bytes: 10,
            available_bytes: None,
        }]))
        .build_payload();
    state.update_snapshot(v1.clone(), payload).await;

    let app = state;
    let response = call(&app, get("/v2/status")).await;
    let body = response_body_string(response);
    let parsed: gregg_protocol::v2::StatusPayloadV2 = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed.drives.as_ref().unwrap()[0].name, "/");

    let response = call(&app, get("/v1/status")).await;
    let body = response_body_string(response);
    let parsed_v1: StatusSnapshot = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed_v1, v1);
}

#[tokio::test]
async fn status_warming_returns_503() {
    let state = ServerState::new();
    let app = state;
    let response = call(&app, get("/v1/status")).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Warming);
}

#[tokio::test]
async fn v1_status_v2_only_returns_503_with_health() {
    // Simulate the Windows path: v2 snapshot exists but v1 is None.
    let state = ServerState::new();
    let snap_v2 = LinuxSnapshotV2Builder::default().build_payload();
    state.update_snapshot_v2_only(snap_v2).await;

    let app = state;

    // /v1/status should return 503 because no v1 snapshot was provided.
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
    assert_eq!(parsed.category, Some(HealthCategory::NotServing));
    assert_eq!(
        parsed.message.as_deref(),
        Some("schema v1 status is unavailable on this platform")
    );

    // /v2/status should return 200 with the v2 snapshot.
    let response = call(&app, get("/v2/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed_v2: gregg_protocol::v2::StatusPayloadV2 = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed_v2.snapshot.schema_version, 2);
}

#[tokio::test]
async fn v2_only_state_keeps_all_v1_routes_not_serving_and_v2_ready() {
    let state = ServerState::new();
    state
        .update_snapshot_v2_only(LinuxSnapshotV2Builder::default().build_payload())
        .await;
    let app = state;

    for path in ["/", "/v1/status", "/healthz"] {
        let response = call(&app, get(path)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let body: HealthResponse = serde_json::from_str(&response_body_string(response)).unwrap();
        assert_eq!(body.schema_version, 1);
        assert_eq!(body.state, ReadinessState::Failed);
        assert_eq!(body.category, Some(HealthCategory::NotServing));
        assert!(body.snapshot.is_none());
        assert_eq!(body.message.as_deref(), Some(V1_UNAVAILABLE_MESSAGE));
    }

    let response = call(&app, get("/v2/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = call(&app, get("/v2/healthz")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: gregg_protocol::v2::HealthResponseV2 =
        serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(body.state, ReadinessState::Ready);
    assert!(body.snapshot.is_some());
}

#[tokio::test]
async fn v2_only_failure_keeps_cached_status_but_fails_health() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    state
        .update_snapshot_v2_only(LinuxSnapshotV2Builder::default().build_payload())
        .await;
    state.set_failed("collector crashed").await;
    let app = state;

    let response = call(&app, get("/v2/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = call(&app, get("/v2/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: gregg_protocol::v2::HealthResponseV2 =
        serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(body.state, ReadinessState::Failed);
    assert_eq!(body.category, Some(HealthCategory::CollectorFailure));
    assert!(body.snapshot.is_none());
}

#[tokio::test]
async fn v2_only_failure_keeps_v1_not_serving_after_stale_threshold() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    state
        .update_snapshot_v2_only(WindowsSnapshotV2Builder::default().build_payload())
        .await;
    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;
    state.set_failed("failure 3").await;

    let response = call(&state, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: HealthResponse = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(body.state, ReadinessState::Failed);
    assert_eq!(body.category, Some(HealthCategory::NotServing));
    assert_eq!(body.message.as_deref(), Some(V1_UNAVAILABLE_MESSAGE));
    assert!(body.snapshot.is_none());
}

#[tokio::test]
async fn root_returns_same_as_status() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let response = call(&app, get("/")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.system.name, "deadpool");
}

#[tokio::test]
async fn healthz_ready_returns_200() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let response = call(&app, get("/healthz")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Ready);
    assert!(parsed.snapshot.is_some());
}

#[tokio::test]
async fn healthz_warming_returns_503() {
    let state = ServerState::new();
    let app = state;
    let response = call(&app, get("/healthz")).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Warming);
}

#[tokio::test]
async fn post_status_returns_405() {
    let state = ServerState::new();
    let app = state;
    let response = call(&app, post("/v1/status")).await;

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn post_unknown_route_returns_404() {
    let state = ServerState::new();
    let app = state;
    let response = call(&app, request("POST", "/nonexistent")).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn nonexistent_path_returns_404() {
    let state = ServerState::new();
    let app = state;
    let response = call(&app, get("/nonexistent")).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn response_content_type_is_json() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;

    let response = call(&app, get("/v1/status")).await;
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );

    let response = call(&app, get("/healthz")).await;
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn json_body_is_valid_and_parseable() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let response = call(&app, get("/v1/status")).await;

    let body_str = response_body_string(response);
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["schema_version"], 1);
}

// ===== Concurrency Test =====

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_return_same_snapshot() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap.clone()).await;

    let app = state;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut handles = vec![];
    for _ in 0..50 {
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(
                    b"GET /v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();

            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        }));
    }

    let mut responses = vec![];
    for handle in handles {
        responses.push(handle.await.unwrap());
    }

    assert_eq!(responses.len(), 50);

    for raw_response in &responses {
        let status_line = raw_response.lines().next().unwrap();
        assert!(
            status_line.contains("200"),
            "Expected 200 but got: {status_line}"
        );

        let body = raw_response.split_once("\r\n\r\n").unwrap().1;
        let parsed: StatusSnapshot = serde_json::from_str(body).unwrap();
        assert_eq!(parsed, snap);
    }

    control.shutdown();
    completion.wait().await.unwrap();
}

// ===== Stale Snapshot Tests =====

fn fresh_snapshot() -> StatusSnapshot {
    #[allow(clippy::cast_possible_truncation)]
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    LinuxSnapshotBuilder::default()
        .observed_at_unix_ms(now)
        .build()
}

#[tokio::test]
async fn stale_snapshot_served_when_within_age() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(60));
    let snap = fresh_snapshot();
    update_both(&state, snap.clone()).await;

    // Simulate a failure — snapshot is preserved.
    state.set_failed("collector error").await;

    let app = state;
    let response = call(&app, get("/v1/status")).await;

    // Snapshot is within the max age, so it should be served.
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
}

#[tokio::test]
async fn stale_snapshot_rejected_when_max_failures_exceeded() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    update_both(&state, snap.clone()).await;

    // Simulate 3 failures — hits the threshold.
    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;
    state.set_failed("failure 3").await;

    let app = state;
    let response = call(&app, get("/v1/status")).await;

    // Snapshot is stale due to failure count.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
    assert_eq!(parsed.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(parsed.message.as_deref(), Some("failure 3"));
    assert!(parsed.snapshot.is_none());

    let response = call(&app, get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let health: HealthResponse = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(health.state, ReadinessState::Failed);
    assert_eq!(health.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(health.message.as_deref(), Some("failure 3"));
    assert!(health.snapshot.is_none());
}

#[tokio::test]
async fn stale_v2_snapshot_preserves_latest_failure_message() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    state
        .update_snapshot_v2_only(WindowsSnapshotV2Builder::default().build_payload())
        .await;
    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;
    state.set_failed("failure 3").await;

    let app = state;
    let response = call(&app, get("/v2/status")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let status: HealthResponseV2 = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(status.state, ReadinessState::Failed);
    assert_eq!(status.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(status.message.as_deref(), Some("failure 3"));
    assert!(status.snapshot.is_none());

    let response = call(&app, get("/v2/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let health: HealthResponseV2 = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(health.state, ReadinessState::Failed);
    assert_eq!(health.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(health.message.as_deref(), Some("failure 3"));
    assert!(health.snapshot.is_none());
}

#[tokio::test]
async fn pre_epoch_clock_is_stale_when_age_policy_is_enabled() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(60));
    let snap = fresh_snapshot();
    update_both(&state, snap).await;
    let published = state.published.read().await;
    assert!(state.is_stale(&published, None));
}

#[tokio::test]
async fn healthz_reflects_stale_snapshot() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    update_both(&state, snap).await;

    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;
    state.set_failed("failure 3").await;

    let app = state;
    let response = call(&app, get("/healthz")).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
}

#[tokio::test]
async fn snapshot_preserved_after_single_failure_not_stale() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    update_both(&state, snap.clone()).await;

    state.set_failed("failure 1").await;

    let app = state.clone();

    // /v1/status still serves the snapshot (only 1 failure, threshold is 3).
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    let response = call(&app, get("/v2/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state
            .v2_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    // /healthz reports failed.
    let response = call(&app, get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn warming_state_serves_503_regardless_of_stale_policy() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(3600));

    let app = state;
    let response = call(&app, get("/v1/status")).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Warming);
}

// ===== Age-Based Staleness Tests =====
#[tokio::test]
async fn v2_only_snapshot_ages_out_on_status_and_health() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_millis(100));
    let payload = LinuxSnapshotV2Builder::default()
        .observed_at_unix_ms(1)
        .build_payload();
    state.update_snapshot_v2_only(payload).await;
    let app = state;
    let response = call(&app, get("/v2/status")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponseV2 = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
    assert_eq!(parsed.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(parsed.message.as_deref(), Some("cached snapshot is stale"));
    let response = call(&app, get("/v2/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponseV2 = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
}

#[test]
fn v2_builders_override_sample_interval_and_observed_at() {
    let snap = LinuxSnapshotV2Builder::default()
        .sample_interval_ms(2000)
        .observed_at_unix_ms(1_234_567_890_123)
        .build();
    assert_eq!(snap.sample_interval_ms, 2000);
    assert_eq!(snap.observed_at_unix_ms, 1_234_567_890_123);

    let payload = WindowsSnapshotV2Builder::default()
        .sample_interval_ms(3000)
        .observed_at_unix_ms(1_234_567_890_456)
        .build_payload();
    assert_eq!(payload.snapshot.sample_interval_ms, 3000);
    assert_eq!(payload.snapshot.observed_at_unix_ms, 1_234_567_890_456);
}

#[tokio::test]
async fn stale_snapshot_by_age_returns_503() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_millis(100));

    // Build a snapshot with a timestamp far in the past (but non-zero).
    let snap = LinuxSnapshotBuilder::default()
        .observed_at_unix_ms(1)
        .build();
    update_both(&state, snap).await;

    // Sleep briefly so the snapshot exceeds max_snapshot_age.
    let app = state;
    let response = call(&app, get("/v1/status")).await;

    // HTTP status is 503 and the body must not claim `ready`.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
    assert_eq!(parsed.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(parsed.message.as_deref(), Some("cached snapshot is stale"));

    // /healthz agrees: 503 with a failed body, never a ready body.
    let response = call(&app, get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
}

#[tokio::test]
async fn fresh_snapshot_returns_200() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(60));
    let snap = fresh_snapshot();
    update_both(&state, snap.clone()).await;

    let app = state;
    let response = call(&app, get("/v1/status")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
}

#[tokio::test]
async fn future_snapshot_is_stale_when_clock_goes_backward() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(60));
    let snap = LinuxSnapshotBuilder::default()
        .observed_at_unix_ms(u64::MAX)
        .build();
    update_both(&state, snap).await;

    // `observed_at` lies in the future relative to the current clock
    // (backward jump after sampling): the snapshot must not be served
    // as fresh.
    let app = state;
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response);
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
    assert_eq!(parsed.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(parsed.message.as_deref(), Some("cached snapshot is stale"));
}

#[tokio::test]
async fn snapshot_ahead_of_now_is_stale() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(60));
    let snap = LinuxSnapshotBuilder::default()
        .observed_at_unix_ms(1_000)
        .build();
    update_both(&state, snap).await;

    // `now` precedes `observed_at`: the snapshot is from the future and
    // therefore stale.
    let published = state.published.read().await;
    assert!(state.is_stale(&published, Some(0)));
}

#[tokio::test]
async fn recovery_after_stale_by_age_returns_200() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_millis(100));

    // Publish a stale snapshot (timestamp far in the past, but non-zero).
    let stale_snap = LinuxSnapshotBuilder::default()
        .observed_at_unix_ms(1)
        .build();
    update_both(&state, stale_snap).await;

    // Should be stale now.
    let app = state.clone();
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Recovery: publish a fresh snapshot.
    let fresh_snap = fresh_snapshot();
    update_both(&state, fresh_snap.clone()).await;

    let app = state;
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, fresh_snap);
}

#[tokio::test]
async fn failure_count_resets_on_recovery() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    update_both(&state, snap.clone()).await;

    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;

    // Recovery — reset count.
    update_both(&state, snap.clone()).await;
    assert_eq!(state.consecutive_failures().await, 0);

    state.set_failed("failure 1").await;
    assert_eq!(state.consecutive_failures().await, 1);

    let app = state;
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

// ===== Hardening Tests =====

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_request_line_does_not_crash() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /invalid HTTP/1.0\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    // Read whatever the server sends — it may close or return an error.
    let _ = stream.read_to_end(&mut response).await;

    // Server should still be alive — send another valid request.
    let mut stream2 = TcpStream::connect(addr).await.unwrap();
    stream2
        .write_all(b"GET /v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut resp = String::new();
    stream2.read_to_string(&mut resp).await.unwrap();
    let status_line = resp.lines().next().unwrap();
    assert!(
        status_line.contains("200"),
        "Expected 200 after malformed request, got: {status_line}"
    );

    control.shutdown();
    completion.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_request_headers_are_bounded() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Build a request with a very large header value.
    let large_value = "A".repeat(200_000);
    let request = format!(
        "GET /v1/status HTTP/1.1\r\nHost: localhost\r\nX-Large: {large_value}\r\nConnection: close\r\n\r\n"
    );

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;

    // Server should still be alive for the next request.
    let mut stream2 = TcpStream::connect(addr).await.unwrap();
    stream2
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut resp = String::new();
    stream2.read_to_string(&mut resp).await.unwrap();
    let status_line = resp.lines().next().unwrap();
    assert!(
        status_line.contains("200") || status_line.contains("503"),
        "Server should still respond, got: {status_line}"
    );

    control.shutdown();
    completion.wait().await.unwrap();
}

#[tokio::test]
async fn put_patch_delete_options_return_405_or_404() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;

    let methods = ["PUT", "DELETE", "PATCH", "OPTIONS"];
    let routes = ["/", "/v1/status", "/healthz"];

    for method in &methods {
        for route in &routes {
            let response = call(&app, request(method, route)).await;

            assert!(
                response.status() == StatusCode::METHOD_NOT_ALLOWED
                    || response.status() == StatusCode::NOT_FOUND,
                "Expected 405 or 404 for {method} {route}, got {}",
                response.status()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_with_body_does_not_crash() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Send a GET with Content-Length and a body.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            b"GET /v1/status HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        )
        .await
        .unwrap();

    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();

    let status_line = resp.lines().next().unwrap();
    // GET with body should either be handled (200) or rejected (400/405).
    assert!(
        status_line.contains("200") || status_line.contains("400") || status_line.contains("405"),
        "Unexpected status for GET with body: {status_line}"
    );

    control.shutdown();
    completion.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_during_state_transition() {
    let state = ServerState::new();

    let app = state.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Start with a snapshot.
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap.clone()).await;

    // Send requests while transitioning state.
    let mut handles = vec![];
    for _ in 0..10 {
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(
                    b"GET /v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let mut resp = String::new();
            stream.read_to_string(&mut resp).await.unwrap();
            resp
        }));
    }

    // Transition to warming while requests are in flight.
    state.set_warming().await;
    // Transition back to ready.
    update_both(&state, snap.clone()).await;

    let mut statuses = vec![];
    for h in handles {
        let resp = h.await.unwrap();
        let status_line = resp.lines().next().unwrap().to_string();
        statuses.push(status_line);
    }

    // Every request should have completed with a valid HTTP status.
    assert_eq!(statuses.len(), 10);
    for s in &statuses {
        assert!(
            s.contains("200") || s.contains("503"),
            "Unexpected status: {s}"
        );
    }

    control.shutdown();
    completion.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rapid_state_updates_are_consistent() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();

    // Rapidly cycle: warming → ready → failed → ready → failed → warming → ready
    update_both(&state, snap.clone()).await;
    state.set_failed("failure 1").await;
    update_both(&state, snap.clone()).await;
    state.set_failed("failure 2").await;
    state.set_warming().await;
    update_both(&state, snap.clone()).await;

    // Final state should be ready with the snapshot.
    assert_eq!(state.health().await.state, ReadinessState::Ready);
    let stored = state.snapshot().await.unwrap();
    assert_eq!(*stored, snap);
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Ready);

    // Verify the server serves correctly after rapid cycling.
    let app = state;
    let response = call(&app, get("/v1/status")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response);
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ipv6_loopback_if_available() {
    // Try to bind on IPv6 loopback. Skip gracefully if unavailable.
    let Ok(listener) = TcpListener::bind("[::1]:0").await else {
        return; // IPv6 not available on this host.
    };
    let addr = listener.local_addr().unwrap();

    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap.clone()).await;

    let app = state;

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /v1/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();

    let status_line = resp.lines().next().unwrap();
    assert!(
        status_line.contains("200"),
        "Expected 200 on IPv6, got: {status_line}"
    );

    let body = resp.split_once("\r\n\r\n").unwrap().1;
    let parsed: StatusSnapshot = serde_json::from_str(body).unwrap();
    assert_eq!(parsed, snap);

    control.shutdown();
    completion.wait().await.unwrap();
}

// ===== Plan 139: ready-health memoization and borrowed dispatch =====

#[tokio::test]
async fn plan139_repeated_ready_v1_health_serializes_once() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    for _ in 0..10 {
        let response = call(&state, get("/healthz")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let parsed: gregg_protocol::HealthResponse =
            serde_json::from_str(&response_body_string(response)).unwrap();
        assert_eq!(parsed.state, ReadinessState::Ready);
    }
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "fresh ready v1 health must serialize at most once per publication"
    );
}

#[tokio::test]
async fn plan139_repeated_ready_v2_health_serializes_once() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    for _ in 0..10 {
        let response = call(&state, get("/v2/healthz")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let parsed: HealthResponseV2 =
            serde_json::from_str(&response_body_string(response)).unwrap();
        assert_eq!(parsed.state, ReadinessState::Ready);
    }
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "fresh ready v2 health must serialize at most once per publication"
    );
}

#[tokio::test]
async fn plan139_new_publication_rearms_health_memo() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    let _ = call(&state, get("/healthz")).await;
    let _ = call(&state, get("/v2/healthz")).await;
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(2)
            .build(),
    )
    .await;
    let _ = call(&state, get("/healthz")).await;
    let _ = call(&state, get("/v2/healthz")).await;
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[tokio::test]
async fn plan139_stale_transition_never_serves_cached_ready_bytes() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_millis(100));
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(1)
            .build(),
    )
    .await;
    // Prime the ready memo path on a fresh publication first (no age policy).
    let fresh = ServerState::new();
    update_both(&fresh, LinuxSnapshotBuilder::default().build()).await;
    let ready_body = response_body_string(call(&fresh, get("/healthz")).await);
    assert!(ready_body.contains("\"ready\""));

    // The stale state must return the exact Plan-124 stale envelope.
    let response = call(&state, get("/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_body_string(response);
    assert_ne!(body, ready_body);
    let parsed: gregg_protocol::HealthResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
    assert_eq!(parsed.message.as_deref(), Some("cached snapshot is stale"));

    let response = call(&state, get("/v2/healthz")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let parsed: HealthResponseV2 = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(parsed.message.as_deref(), Some("cached snapshot is stale"));
}

#[tokio::test]
async fn plan139_failure_after_cached_ready_preserves_exact_message() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    let _ = call(&state, get("/healthz")).await;
    let _ = call(&state, get("/v2/healthz")).await;
    state.set_failed("collector crashed").await;
    for path in ["/healthz", "/v2/healthz"] {
        let response = call(&state, get(path)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let body = response_body_string(response);
        assert!(
            body.contains("collector crashed"),
            "{path} must preserve Plan-124 failure message, got {body}"
        );
        assert!(
            !body.contains("\"ready\""),
            "{path} must not serve ready bytes"
        );
    }
}

#[tokio::test]
async fn plan139_not_serving_survives_later_failures() {
    let state = ServerState::new();
    state
        .update_snapshot_v2_only(WindowsSnapshotV2Builder::default().build_payload())
        .await;
    let _ = call(&state, get("/healthz")).await;
    state.set_failed("collector crashed").await;
    let response = call(&state, get("/healthz")).await;
    let parsed: gregg_protocol::HealthResponse =
        serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(parsed.category, Some(HealthCategory::NotServing));
    assert_eq!(
        parsed.message.as_deref(),
        Some("schema v1 status is unavailable on this platform")
    );
    // v1-only compat path mirrors the guarantee.
    let compat = ServerState::new();
    compat
        .update_snapshot_v1_only(LinuxSnapshotBuilder::default().build())
        .await;
    compat.set_failed("boom").await;
    let response = call(&compat, get("/v2/healthz")).await;
    let parsed: HealthResponseV2 = serde_json::from_str(&response_body_string(response)).unwrap();
    assert_eq!(parsed.category, Some(HealthCategory::NotServing));
}

#[test]
fn plan139_borrowed_ready_health_matches_public_types() {
    for builder in [
        LinuxSnapshotBuilder::default().build(),
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(42)
            .build(),
    ] {
        let owned =
            serde_json::to_vec(&gregg_protocol::HealthResponse::ready(builder.clone())).unwrap();
        let borrowed = super::serialize_ready_health_borrowed(&builder).unwrap();
        assert_eq!(owned, borrowed.to_vec());
    }
    for payload in [
        LinuxSnapshotV2Builder::default().build_payload(),
        WindowsSnapshotV2Builder::default().build_payload(),
    ] {
        let owned = serde_json::to_vec(&HealthResponseV2::ready(payload.snapshot.clone())).unwrap();
        let borrowed = super::serialize_ready_health_v2_borrowed(&payload.snapshot).unwrap();
        assert_eq!(owned, borrowed.to_vec());
    }
}

#[tokio::test]
async fn plan139_status_cache_unchanged_by_health_memo() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    for _ in 0..5 {
        let response = call(&state, get("/v1/status")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(&state, get("/healthz")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        state
            .v1_status_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn plan139_known_routes_avoid_fallback_allocation() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    let before = state
        .fallback_bodies_built
        .load(std::sync::atomic::Ordering::Relaxed);
    for path in ["/", "/v1/status", "/v2/status", "/healthz", "/v2/healthz"] {
        let response = call(&state, get(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    assert_eq!(
        state
            .fallback_bodies_built
            .load(std::sync::atomic::Ordering::Relaxed),
        before,
        "known routes must not build 404 fallback bodies"
    );
    let response = call(&state, get("/unknown")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        state
            .fallback_bodies_built
            .load(std::sync::atomic::Ordering::Relaxed),
        before + 1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_http_version_is_handled_gracefully() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    update_both(&state, snap).await;

    let app = state;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (control, mut completion) = start(listener, app).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Send a request with an invalid HTTP version.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"GET / HTTP/0.9\r\n\r\n").await.unwrap();

    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;

    // Server should still be alive — send a valid follow-up.
    let mut stream2 = TcpStream::connect(addr).await.unwrap();
    stream2
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut resp = String::new();
    stream2.read_to_string(&mut resp).await.unwrap();
    let status_line = resp.lines().next().unwrap();
    assert!(
        status_line.contains("200") || status_line.contains("503"),
        "Expected valid response after malformed HTTP version, got: {status_line}"
    );

    control.shutdown();
    completion.wait().await.unwrap();
}

// ===== Plan 144: ready-health single-flight memoization =====

fn assert_response_status(response: &TestResponse, expected_status: StatusCode) {
    assert_eq!(response.status(), expected_status);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn plan144_concurrent_v1_health_serializes_once() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    // Deterministic concurrency gate: arm for 8 so all eight tasks reach
    // the cell boundary together.
    state.test_gate.arm(8);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            (response.status(), response.body)
        }));
    }

    let mut bodies = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        bodies.push(body);
    }

    state.test_gate.reset();

    let first = bodies.first().unwrap();
    for body in &bodies[1..] {
        assert_eq!(
            body, first,
            "all concurrent v1 health bodies must be byte-identical"
        );
    }

    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "8 concurrent first requests must serialize the v1 ready-health body exactly once"
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "v2 path was not exercised"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn plan144_concurrent_v2_health_serializes_once() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    state.test_gate.arm(8);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/v2/healthz")).await;
            (response.status(), response.body)
        }));
    }

    let mut bodies = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        bodies.push(body);
    }

    state.test_gate.reset();

    let first = bodies.first().unwrap();
    for body in &bodies[1..] {
        assert_eq!(
            body, first,
            "all concurrent v2 health bodies must be byte-identical"
        );
    }

    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "8 concurrent first requests must serialize the v2 ready-health body exactly once"
    );
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "v1 path was not exercised"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn plan144_concurrent_mixed_v1_and_v2_serializes_each_once() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    state.test_gate.arm(8);

    let mut handles = Vec::new();
    for _ in 0..4 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            (response.status(), response.body)
        }));
    }
    for _ in 0..4 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/v2/healthz")).await;
            (response.status(), response.body)
        }));
    }

    let mut v1_bodies = Vec::new();
    let mut v2_bodies = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        let raw = String::from_utf8(body).unwrap();
        if raw.contains("\"schema_version\":1") {
            v1_bodies.push(raw);
        } else if raw.contains("\"schema_version\":2") {
            v2_bodies.push(raw);
        } else {
            panic!("unexpected schema_version in body: {raw}");
        }
    }
    state.test_gate.reset();

    let v1_first = v1_bodies.first().unwrap();
    for body in &v1_bodies[1..] {
        assert_eq!(body, v1_first);
    }
    let v2_first = v2_bodies.first().unwrap();
    for body in &v2_bodies[1..] {
        assert_eq!(body, v2_first);
    }

    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan144_new_publication_owns_fresh_cells() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    assert_response_status(&call(&state, get("/v2/healthz")).await, StatusCode::OK);
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    // Publish a new snapshot: cells must be replaced so the next request
    // triggers exactly one fresh init per version.
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(7)
            .build(),
    )
    .await;
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    assert_response_status(&call(&state, get("/v2/healthz")).await, StatusCode::OK);
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );

    // Subsequent requests must reuse the second publication's cell.
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    assert_response_status(&call(&state, get("/v2/healthz")).await, StatusCode::OK);
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_memo_waiter_does_not_serve_ready_after_failure_transition() {
    // The ready-health read guard is released while the memo initializes. A
    // concurrent `set_failed()` installs a fresh cell and moves the state off
    // `Ready`; the waiter must not answer `200` with the detached cell's body.
    let state = ServerState::new();
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(1)
            .build(),
    )
    .await;

    let hold = state.test_gate.hold_serialization();
    let raced = state.clone();
    let first_handle = tokio::spawn(async move {
        let response = call(&raced, get("/healthz")).await;
        response.status()
    });

    TestSerializeGate::wait_until_held(&hold).await;
    state.set_failed("collector failure").await;
    hold.release.notify_one();
    assert_eq!(first_handle.await.unwrap(), StatusCode::SERVICE_UNAVAILABLE);

    // Every later reader must agree.
    assert_response_status(
        &call(&state, get("/healthz")).await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    assert_response_status(
        &call(&state, get("/v2/healthz")).await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_health_memo_waiter_does_not_serve_ready_after_failure_transition() {
    let state = ServerState::new();
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(1)
            .build(),
    )
    .await;

    let hold = state.test_gate.hold_serialization();
    let raced = state.clone();
    let first_handle = tokio::spawn(async move {
        let response = call(&raced, get("/v2/healthz")).await;
        response.status()
    });

    TestSerializeGate::wait_until_held(&hold).await;
    state.set_failed("collector failure").await;
    hold.release.notify_one();
    assert_eq!(first_handle.await.unwrap(), StatusCode::SERVICE_UNAVAILABLE);

    assert_response_status(
        &call(&state, get("/v2/healthz")).await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan144_old_publication_cell_cannot_contaminate_new() {
    let state = ServerState::new();
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(1)
            .build(),
    )
    .await;

    // Spawn one task that races a fresh publication; with two-worker
    // runtime it cannot deadlock because at most one task is in flight.
    let app_first = state.clone();
    let first_handle = tokio::spawn(async move {
        let response = call(&app_first, get("/healthz")).await;
        response.status()
    });

    // Give the first task a moment, then publish a new snapshot. The new
    // snapshot replaces the cell so the first task's eventual init writes
    // only to the orphaned cell.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(2)
            .build(),
    )
    .await;

    assert_eq!(first_handle.await.unwrap(), StatusCode::OK);

    // The new publication must trigger a brand new init exactly once.
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    let v1_count = state
        .v1_health_serializations
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        v1_count >= 2,
        "old init + new publication init must each serialize once (got {v1_count})"
    );
    assert!(
        v1_count <= 2,
        "old cell must not contaminate the new publication (got {v1_count})"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan144_serialization_error_leaves_cell_retryable() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;

    state
        .fail_next_v1_health_serialize
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let failed = call_raw(&state, get("/healthz")).await;
    match failed {
        Err(error) => {
            let message = error.message();
            assert!(
                message.contains("test-injected"),
                "error must surface the injected message: {message}"
            );
        }
        Ok(response) => panic!(
            "expected an error from injected serialization failure, got status {:?}",
            response.status()
        ),
    }
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "injected failure must not count as a successful serialization"
    );

    // A follow-up request with the flag cleared must succeed and populate
    // the cell. The cell must remain retryable; no memoized error result.
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    assert_response_status(&call(&state, get("/healthz")).await, StatusCode::OK);
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "retry must serialize exactly once after the failed init"
    );

    // Recovery must apply independently to v2.
    state
        .fail_next_v2_health_serialize
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let failed_v2 = call_raw(&state, get("/v2/healthz")).await;
    assert!(
        failed_v2.is_err(),
        "v2 injected failure must surface as error"
    );
    assert_response_status(&call(&state, get("/v2/healthz")).await, StatusCode::OK);
    assert_eq!(
        state
            .v2_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

async fn call_raw(
    state: &ServerState,
    request: TestRequest,
) -> Result<TestResponse, eggserve_server::ServiceError> {
    let method = Method::new(request.method).unwrap();
    let target = RequestTarget::parse(request.target).unwrap();
    let head = RequestHead::new(method, target, HttpVersion::Http11, HeaderBlock::new());
    let connection = ConnectionInfo::with_socket_addrs(
        "127.0.0.1:11310".parse().unwrap(),
        "127.0.0.1:12345".parse().unwrap(),
        Scheme::Http,
        None,
    );
    let request = Request::new(head, RequestBody::empty(), connection);
    let service = http_service(state.clone());
    let mut response = service.call(request).await?;
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|header| {
            (
                header.name.as_str().to_ascii_lowercase(),
                header.value.to_str().unwrap().to_owned(),
            )
        })
        .collect();
    let body = match response.take_body().unwrap_or(ResponseBody::Empty) {
        ResponseBody::Empty | ResponseBody::EmptyWithLength(_) => Vec::new(),
        ResponseBody::Bytes(bytes) => bytes,
        ResponseBody::Stream(stream) => {
            let (mut stream, _) = stream.into_parts();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                bytes.extend_from_slice(&chunk.unwrap());
            }
            bytes
        }
        ResponseBody::File(_) => panic!("Gregg service does not produce file bodies"),
    };
    Ok(TestResponse {
        status,
        headers,
        body,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn plan144_failure_after_concurrent_ready_preserves_exact_messages() {
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;
    state.test_gate.arm(8);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            (response.status(), response.body)
        }));
    }
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        let parsed: gregg_protocol::HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.state, ReadinessState::Ready);
    }
    state.test_gate.reset();

    state.set_failed("collector crashed").await;
    for path in ["/healthz", "/v2/healthz"] {
        let response = call(&state, get(path)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let body = response_body_string(response);
        assert!(
            body.contains("collector crashed"),
            "{path} must preserve Plan-124 failure message, got {body}"
        );
        assert!(
            !body.contains("\"ready\""),
            "{path} must not serve ready bytes after failure, got {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn plan144_concurrent_warming_does_not_initialize_cell() {
    let state = ServerState::new();
    // No publication: state is Warming.
    state.test_gate.arm(8);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            (response.status(), response.body)
        }));
    }
    let mut bodies = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        bodies.push(body);
    }
    state.test_gate.reset();
    let first = bodies.first().unwrap();
    for body in &bodies[1..] {
        assert_eq!(body, first, "warming responses must be byte-identical");
    }
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "warming path must not run the ready-health serializer"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn plan144_publication_replaces_cell_under_concurrency() {
    let state = ServerState::new();
    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(1)
            .build(),
    )
    .await;

    state.test_gate.arm(8);
    let mut first_handles = Vec::new();
    for _ in 0..8 {
        let app = state.clone();
        first_handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            response.status()
        }));
    }
    for handle in first_handles {
        assert_eq!(handle.await.unwrap(), StatusCode::OK);
    }
    let after_first_batch = state
        .v1_health_serializations
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(after_first_batch, 1);

    update_both(
        &state,
        LinuxSnapshotBuilder::default()
            .observed_at_unix_ms(2)
            .build(),
    )
    .await;

    state.test_gate.arm(8);
    let mut second_handles = Vec::new();
    for _ in 0..8 {
        let app = state.clone();
        second_handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            (response.status(), response.body)
        }));
    }
    let mut second_bodies = Vec::new();
    for handle in second_handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        second_bodies.push(body);
    }
    state.test_gate.reset();
    let first_second = second_bodies.first().unwrap();
    for body in &second_bodies[1..] {
        assert_eq!(body, first_second);
    }
    assert_eq!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed),
        2,
        "new publication must trigger exactly one new successful serialization"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan144_no_lock_held_across_init_for_concurrent_burst() {
    // This test exercises the readiness response path under heavy
    // concurrency without a gate; it proves that single-flight holds
    // without deterministic barriers and that follow-up requests remain
    // byte-identical to the first successful response.
    let state = ServerState::new();
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;

    let mut handles = Vec::new();
    for _ in 0..32 {
        let app = state.clone();
        handles.push(tokio::spawn(async move {
            let response = call(&app, get("/healthz")).await;
            (response.status(), response.body)
        }));
    }

    let mut bodies = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        bodies.push(body);
    }

    let first = bodies.first().unwrap();
    for body in &bodies[1..] {
        assert_eq!(body, first);
    }

    assert!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed)
            <= 4,
        "32 concurrent first requests must serialize at most a small number of times"
    );
    assert!(
        state
            .v1_health_serializations
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 1
    );
}

// ===== Plan 163: scheduler observability routes =====

/// Build a scheduler publication handle serving a representative live state
/// plus two bounded terminal records, without a real child process.
///
/// Covers the shapes the routes must distinguish: a successful run with output
/// tails, a load-expired occurrence that never ran a child, a running job, and
/// a load-high delayed job with its published load decision.
async fn scheduler_publisher_with_state() -> crate::scheduler::observation::SchedulerPublisher {
    use crate::scheduler::observation::{OutputTail, SchedulerObserver, TerminalRecord};
    use gregg_protocol::{
        SchedulerJobStateV2, SchedulerJobV2, SchedulerLoadGateV2, SchedulerOutcomeV2,
    };

    let publisher = crate::scheduler::observation::SchedulerPublisher::empty();
    let names = vec!["backup".to_owned(), "rotate".to_owned()];
    let mut observer = SchedulerObserver::new(&names, 5, 1_700_000_000_000, publisher.clone());

    let mut stdout = OutputTail::new();
    stdout.push(b"wrote 3 archives");
    let mut stderr = OutputTail::new();
    stderr.push(b"warning: disk 91% full");
    observer.record_terminal(
        0,
        TerminalRecord {
            scheduled_unix_ms: 1_700_000_000_000,
            started_unix_ms: Some(1_700_000_001_000),
            finished_unix_ms: 1_700_000_002_000,
            delay_ms: 1_000,
            coalesced: false,
            exit_code: Some(0),
            signal: None,
            duration_ms: Some(1_000),
            stdout,
            stderr,
        },
        SchedulerOutcomeV2::Success,
    );
    observer.record_terminal(
        1,
        TerminalRecord::without_child(
            1_700_000_000_000,
            1_700_000_060_000,
            60_000,
            false,
            SchedulerOutcomeV2::LoadExpired,
        ),
        SchedulerOutcomeV2::LoadExpired,
    );

    let job = |name: &str, state: SchedulerJobStateV2| SchedulerJobV2 {
        name: name.to_owned(),
        schedule: "0 3 * * *".to_owned(),
        next_due_unix_ms: 1_700_000_100_000,
        state,
        load: None,
        pending_since_unix_ms: None,
        next_retry_unix_ms: None,
        running_since_unix_ms: None,
        last: None,
    };

    let mut running = job("backup", SchedulerJobStateV2::Running);
    running.running_since_unix_ms = Some(1_700_000_001_000);
    running.last = observer.last_summary(0);
    observer
        .publish(
            vec![running, job("rotate", SchedulerJobStateV2::Idle)],
            1_700_000_002_000,
        )
        .await;

    // Move `rotate` to its final load-delayed shape, which is what the routes
    // serve for the rest of these tests.
    let mut delayed = job("rotate", SchedulerJobStateV2::LoadHigh);
    delayed.load = Some(SchedulerLoadGateV2 {
        window: "15m".to_owned(),
        threshold: 8.0,
        observed: Some(9.24),
    });
    delayed.pending_since_unix_ms = Some(1_700_000_000_000);
    delayed.next_retry_unix_ms = Some(1_700_000_060_000);
    delayed.last = observer.last_summary(1);
    let mut idle = job("backup", SchedulerJobStateV2::Idle);
    idle.last = observer.last_summary(0);
    observer
        .publish(vec![idle, delayed], 1_700_000_003_000)
        .await;

    publisher
}

#[tokio::test]
async fn scheduler_routes_serve_valid_documents() {
    let publisher = scheduler_publisher_with_state().await;
    let state = ServerState::with_stale_policy_and_scheduler(0, Duration::ZERO, publisher);

    let summary = call(&state, get("/v2/scheduler")).await;
    assert_eq!(summary.status(), StatusCode::OK);
    assert_eq!(
        summary.headers().get("content-type").map(String::as_str),
        Some("application/json")
    );
    let document: gregg_protocol::SchedulerSummaryV2 =
        serde_json::from_slice(&summary.body).expect("valid summary JSON");
    document.validate().expect("summary validates");
    assert_eq!(document.jobs.len(), 2);
    assert_eq!(
        document.jobs[1].state,
        gregg_protocol::SchedulerJobStateV2::LoadHigh
    );
    assert_eq!(document.jobs[1].next_retry_unix_ms, Some(1_700_000_060_000));
    assert_eq!(
        document.jobs[1]
            .load
            .as_ref()
            .and_then(|gate| gate.observed),
        Some(9.24)
    );

    let history = call(&state, get("/v2/scheduler/history")).await;
    assert_eq!(history.status(), StatusCode::OK);
    let document: gregg_protocol::SchedulerHistoryV2 =
        serde_json::from_slice(&history.body).expect("valid history JSON");
    document.validate().expect("history validates");
    assert_eq!(document.jobs.len(), 2);
    assert_eq!(document.history_revision, summary_revision(&summary.body));
}

fn summary_revision(body: &[u8]) -> u64 {
    let value: serde_json::Value = serde_json::from_slice(body).expect("summary JSON");
    value["history_revision"]
        .as_u64()
        .expect("numeric revision")
}

#[tokio::test]
async fn scheduler_history_carries_bounded_output_and_non_child_outcomes() {
    let publisher = scheduler_publisher_with_state().await;
    let state = ServerState::with_stale_policy_and_scheduler(0, Duration::ZERO, publisher);
    let response = call(&state, get("/v2/scheduler/history")).await;
    let document: gregg_protocol::SchedulerHistoryV2 =
        serde_json::from_slice(&response.body).expect("valid history JSON");

    let mut saw_success = false;
    let mut saw_expired = false;
    for job in &document.jobs {
        for record in &job.records {
            if record.outcome == gregg_protocol::SchedulerOutcomeV2::Success {
                saw_success = true;
                assert_eq!(record.exit_code, Some(0));
                assert_eq!(record.stdout.text, "wrote 3 archives");
                assert!(!record.stdout.truncated);
                assert_eq!(record.stderr.text, "warning: disk 91% full");
            }
            if record.outcome == gregg_protocol::SchedulerOutcomeV2::LoadExpired {
                saw_expired = true;
                // A load-expired occurrence never ran a child; fabricating an
                // exit code or duration would be a lie.
                assert_eq!(record.started_unix_ms, None);
                assert_eq!(record.duration_ms, None);
                assert_eq!(record.exit_code, None);
                assert!(!record.coalesced);
            }
        }
    }
    assert!(saw_success, "a successful run must be visible");
    assert!(
        saw_expired,
        "a load-expired occurrence must be visible even without a child"
    );
}

/// GET/HEAD header parity. The "HEAD carries no body" proof lives in
/// `raw_wire_contract_preserved_by_eggserve_transport`, because an in-process
/// service call returns the body that the transport then strips.
#[tokio::test]
async fn scheduler_routes_match_get_and_head_headers() {
    let publisher = scheduler_publisher_with_state().await;
    let state = ServerState::with_stale_policy_and_scheduler(0, Duration::ZERO, publisher);
    for route in ["/v2/scheduler", "/v2/scheduler/history"] {
        let get_response = call(&state, get(route)).await;
        let head_response = call(&state, request("HEAD", route)).await;
        assert_eq!(get_response.status(), head_response.status(), "{route}");
        assert_eq!(
            get_response.headers().get("content-type"),
            head_response.headers().get("content-type"),
            "{route}"
        );
        assert_eq!(
            get_response.headers().get("content-length"),
            head_response.headers().get("content-length"),
            "{route} HEAD must advertise the same length"
        );
        assert_ne!(get_response.body.len(), 0);
    }
}

#[tokio::test]
async fn scheduler_routes_reject_methods_and_unknown_paths() {
    let state = ServerState::new();
    for route in ["/v2/scheduler", "/v2/scheduler/history"] {
        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            let response = call(&state, request(method, route)).await;
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {route}"
            );
        }
    }
    // No scheduler control plane exists: nothing under a scheduler path may
    // accept a mutating verb or a plausible job-control sub-path.
    for path in [
        "/v2/scheduler/run",
        "/v2/scheduler/jobs",
        "/v2/scheduler/cancel",
    ] {
        assert_eq!(
            call(&state, get(path)).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
        assert_eq!(
            call(&state, post(path)).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_daemon_with_no_jobs_serves_a_valid_empty_scheduler_document() {
    // A pre-existing server state (no scheduler task at all) must still
    // answer 200 with a valid empty document, never 404.
    let state = ServerState::new();
    for route in ["/v2/scheduler", "/v2/scheduler/history"] {
        let response = call(&state, get(route)).await;
        assert_eq!(response.status(), StatusCode::OK, "{route}");
    }
    let response = call(&state, get("/v2/scheduler")).await;
    let document: gregg_protocol::SchedulerSummaryV2 =
        serde_json::from_slice(&response.body).expect("valid empty summary");
    assert_eq!(document.jobs.len(), 0);
    assert!(document.validate().is_ok());
}

#[tokio::test]
async fn scheduler_documents_stay_separate_from_the_metrics_payload() {
    // The whole point of additive scheduler routes: an ordinary metrics poll
    // must never carry command output.
    let publisher = scheduler_publisher_with_state().await;
    let state = ServerState::with_stale_policy_and_scheduler(0, Duration::ZERO, publisher);
    update_both(&state, LinuxSnapshotBuilder::default().build()).await;

    let status = call(&state, get("/v2/status")).await;
    let body = String::from_utf8_lossy(&status.body);
    assert!(!body.contains("scheduler"));
    assert!(!body.contains("wrote 3 archives"));

    let health = call(&state, get("/v2/healthz")).await;
    let health_body = String::from_utf8_lossy(&health.body);
    assert!(!health_body.contains("wrote 3 archives"));
}

#[tokio::test]
async fn scheduler_history_body_respects_the_frozen_client_cap() {
    let publisher = scheduler_publisher_with_state().await;
    let state = ServerState::with_stale_policy_and_scheduler(0, Duration::ZERO, publisher);
    let response = call(&state, get("/v2/scheduler/history")).await;
    assert!(
        response.body.len() < gregg_protocol::MAX_SCHEDULER_HISTORY_BODY_BYTES,
        "history body {} exceeds the client cap",
        response.body.len()
    );
    let summary = call(&state, get("/v2/scheduler")).await;
    assert!(
        summary.body.len() < gregg_protocol::MAX_SCHEDULER_SUMMARY_BODY_BYTES,
        "summary body {} exceeds the client cap",
        summary.body.len()
    );
}

#[tokio::test]
async fn the_summary_revision_matches_the_history_document() {
    // The client refetches history only when the summary's revision changes,
    // so the two documents must always agree for the same publication.
    let publisher = scheduler_publisher_with_state().await;
    let publication = publisher.current().await;
    let summary: gregg_protocol::SchedulerSummaryV2 =
        serde_json::from_slice(&publication.summary_bytes).expect("summary JSON");
    let history: gregg_protocol::SchedulerHistoryV2 =
        serde_json::from_slice(&publication.history_bytes).expect("history JSON");
    assert_eq!(summary.epoch, history.epoch, "epoch must match");
    assert_eq!(summary.history_revision, history.history_revision);
    assert_eq!(summary.jobs.len(), history.jobs.len());
}
