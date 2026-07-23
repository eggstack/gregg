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
async fn set_failed_marks_failed() {
    let state = ServerState::new();
    state.set_failed("collector crashed").await;

    assert!(!state.ready.load(Ordering::Acquire));
    assert!(state.snapshot().await.is_none());
    let health = state.health().await;
    assert_eq!(health.state, ReadinessState::Failed);
    assert_eq!(health.category, Some(HealthCategory::CollectorFailure));
    assert_eq!(health.message.as_deref(), Some("collector crashed"));
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
async fn post_status_returns_404() {
    let state = ServerState::new();
    let app = build_test_router(state);
    let response = app.oneshot(post("/v1/status")).await.unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
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
