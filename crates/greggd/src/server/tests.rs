use super::*;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use gregg_protocol::test_support::LinuxSnapshotBuilder;
use gregg_protocol::{HealthCategory, ReadinessState, StatusSnapshot};
use http_body_util::BodyExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

fn build_test_router(state: ServerState) -> Router {
    Router::new()
        .route("/", axum::routing::get(status_handler))
        .route("/v1/status", axum::routing::get(status_handler))
        .route("/healthz", axum::routing::get(health_handler))
        .fallback(fallback_handler)
        .with_state(state)
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::from("")).unwrap()
}

fn post(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .body(Body::from(""))
        .unwrap()
}

async fn response_body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ===== State Tests =====

#[tokio::test]
async fn new_starts_in_warming_state() {
    let state = ServerState::new();
    assert!(!state.ready.load(Ordering::Acquire));
    assert!(state.snapshot().await.is_none());
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Warming);
}

#[tokio::test]
async fn update_snapshot_makes_ready() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap.clone()).await;

    assert!(state.ready.load(Ordering::Acquire));
    let stored = state.snapshot().await.unwrap();
    assert_eq!(*stored, snap);
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Ready);
}

#[tokio::test]
async fn set_warming_clears_snapshot() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    state.set_warming().await;
    assert!(!state.ready.load(Ordering::Acquire));
    assert!(state.snapshot().await.is_none());
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Warming);
}

#[tokio::test]
async fn set_failed_preserves_snapshot() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap.clone()).await;

    state.set_failed("collector crashed").await;

    assert!(!state.ready.load(Ordering::Acquire));
    // Snapshot is preserved for stale-serving.
    let stored = state.snapshot().await.unwrap();
    assert_eq!(*stored, snap);
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Failed);
    assert_eq!(health.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(health.message.as_deref(), Some("collector crashed"));
    assert_eq!(state.consecutive_failures(), 1);
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
    state.update_snapshot(snap.clone()).await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );

    let body_str = response_body_string(response).await;
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
}

#[tokio::test]
async fn status_warming_returns_503() {
    let state = ServerState::new();
    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body_str = response_body_string(response).await;
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Warming);
}

#[tokio::test]
async fn root_returns_same_as_status() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response).await;
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.system.name, "deadpool");
}

#[tokio::test]
async fn healthz_ready_returns_200() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/healthz")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response).await;
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Ready);
    assert!(parsed.snapshot.is_some());
}

#[tokio::test]
async fn healthz_warming_returns_503() {
    let state = ServerState::new();
    let app = build_test_router(state);
    let response = app.oneshot(get("/healthz")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response).await;
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Warming);
}

#[tokio::test]
async fn post_status_returns_405() {
    let state = ServerState::new();
    let app = build_test_router(state);
    let response = app.oneshot(post("/v1/status")).await.unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn post_unknown_route_returns_404() {
    let state = ServerState::new();
    let app = build_test_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/nonexistent")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn nonexistent_path_returns_404() {
    let state = ServerState::new();
    let app = build_test_router(state);
    let response = app.oneshot(get("/nonexistent")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn response_content_type_is_json() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);

    let response = app.clone().oneshot(get("/v1/status")).await.unwrap();
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );

    let response = app.oneshot(get("/healthz")).await.unwrap();
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn json_body_is_valid_and_parseable() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();

    let body_str = response_body_string(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed["schema_version"], 1);
}

// ===== Concurrency Test =====

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_return_same_snapshot() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap.clone()).await;

    let app = build_test_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

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

    server.abort();
}

// ===== Stale Snapshot Tests =====

fn fresh_snapshot() -> StatusSnapshot {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "millis from SystemTime is well within u64 range"
    )]
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
    state.update_snapshot(snap.clone()).await;

    // Simulate a failure — snapshot is preserved.
    state.set_failed("collector error").await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();

    // Snapshot is within the max age, so it should be served.
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response).await;
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);
}

#[tokio::test]
async fn stale_snapshot_rejected_when_max_failures_exceeded() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    state.update_snapshot(snap.clone()).await;

    // Simulate 3 failures — hits the threshold.
    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;
    state.set_failed("failure 3").await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();

    // Snapshot is stale due to failure count.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response).await;
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
}

#[tokio::test]
async fn healthz_reflects_stale_snapshot() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    state.update_snapshot(snap).await;

    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;
    state.set_failed("failure 3").await;

    let app = build_test_router(state);
    let response = app.oneshot(get("/healthz")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response).await;
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Failed);
}

#[tokio::test]
async fn snapshot_preserved_after_single_failure_not_stale() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    state.update_snapshot(snap.clone()).await;

    state.set_failed("failure 1").await;

    let app = build_test_router(state);

    // /v1/status still serves the snapshot (only 1 failure, threshold is 3).
    let response = app.clone().oneshot(get("/v1/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response).await;
    let parsed: StatusSnapshot = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed, snap);

    // /healthz reports failed.
    let response = app.oneshot(get("/healthz")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn warming_state_serves_503_regardless_of_stale_policy() {
    let state = ServerState::with_stale_policy(0, std::time::Duration::from_secs(3600));

    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_str = response_body_string(response).await;
    let parsed: HealthResponse = serde_json::from_str(&body_str).unwrap();
    assert_eq!(parsed.state, ReadinessState::Warming);
}

#[tokio::test]
async fn failure_count_resets_on_recovery() {
    let state = ServerState::with_stale_policy(3, std::time::Duration::ZERO);
    let snap = fresh_snapshot();
    state.update_snapshot(snap.clone()).await;

    state.set_failed("failure 1").await;
    state.set_failed("failure 2").await;

    // Recovery — reset count.
    state.update_snapshot(snap.clone()).await;
    assert_eq!(state.consecutive_failures(), 0);

    state.set_failed("failure 1").await;
    assert_eq!(state.consecutive_failures(), 1);

    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// ===== Hardening Tests =====

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_request_line_does_not_crash() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

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

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_request_headers_are_bounded() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

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

    server.abort();
}

#[tokio::test]
async fn put_patch_delete_options_return_405_or_404() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);

    let methods = [Method::PUT, Method::DELETE, Method::PATCH, Method::OPTIONS];
    let routes = ["/", "/v1/status", "/healthz"];

    for method in &methods {
        for route in &routes {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(*route)
                        .body(Body::from(""))
                        .unwrap(),
                )
                .await
                .unwrap();

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
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

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

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_during_state_transition() {
    let state = ServerState::new();

    let app = build_test_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Start with a snapshot.
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap.clone()).await;

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
    state.update_snapshot(snap.clone()).await;

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

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rapid_state_updates_are_consistent() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();

    // Rapidly cycle: warming → ready → failed → ready → failed → warming → ready
    state.update_snapshot(snap.clone()).await;
    state.set_failed("failure 1").await;
    state.update_snapshot(snap.clone()).await;
    state.set_failed("failure 2").await;
    state.set_warming().await;
    state.update_snapshot(snap.clone()).await;

    // Final state should be ready with the snapshot.
    assert!(state.ready.load(Ordering::Acquire));
    let stored = state.snapshot().await.unwrap();
    assert_eq!(*stored, snap);
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Ready);

    // Verify the server serves correctly after rapid cycling.
    let app = build_test_router(state);
    let response = app.oneshot(get("/v1/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body_string(response).await;
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
    state.update_snapshot(snap.clone()).await;

    let app = build_test_router(state);

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

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

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_http_version_is_handled_gracefully() {
    let state = ServerState::new();
    let snap = LinuxSnapshotBuilder::default().build();
    state.update_snapshot(snap).await;

    let app = build_test_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

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

    server.abort();
}
