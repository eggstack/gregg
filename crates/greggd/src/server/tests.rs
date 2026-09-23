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
fn eggserve_runtime_keeps_unlimited_lifetime_and_explicit_bounds() {
    let config = runtime_config().unwrap();

    assert_eq!(config.connection_total_timeout, Duration::ZERO);
    assert_eq!(config.max_connections, Semaphore::MAX_PERMITS);
    assert_eq!(config.max_in_flight_requests, Semaphore::MAX_PERMITS);
    assert_eq!(config.max_headers, 100);
    assert_eq!(config.max_buf_size, 417_792);
    assert_eq!(config.max_header_bytes, 417_792);
    assert_eq!(config.max_request_target_bytes, 65_536);
    assert_eq!(config.max_request_body_bytes, MAX_REQUEST_BODY_BYTES);
    assert_eq!(config.max_requests_per_connection, None);
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

    let routes = ["/", "/v1/status", "/v2/status", "/healthz", "/v2/healthz"];
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
