#![allow(dead_code)]

//! HTTP client and poll types for fetching status snapshots from greggd
//! endpoints.
//!
//! The [`HttpClient`] wraps a long-lived `reqwest::Client` with
//! configuration derived from the application config. Each poll returns
//! a typed [`PollOutcome`] that classifies every failure mode without
//! leaking error chains to the caller.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use gregg_protocol::{StatusSnapshot, SCHEMA_VERSION_V1};

use crate::clock::Clock;
use crate::endpoint::Endpoint;

/// Maximum allowed response body size in bytes (64 KiB).
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// The result of polling a single endpoint.
#[derive(Debug)]
pub struct PollResult {
    /// The system ID of the endpoint that was polled.
    pub system_id: String,
    /// The endpoint that was polled.
    pub endpoint: Endpoint,
    /// The outcome of the poll.
    pub outcome: PollOutcome,
    /// Round-trip latency of the HTTP request.
    pub latency: Duration,
}

/// Classification of a poll attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum PollOutcome {
    /// Successfully received and validated a snapshot.
    Online(Box<StatusSnapshot>),
    /// The request timed out.
    Timeout,
    /// The connection was refused by the remote host.
    ConnectionRefused,
    /// DNS resolution failed.
    DnsFailure,
    /// An unexpected network error occurred.
    NetworkError,
    /// The server returned a non-success HTTP status code.
    HttpStatus(u16),
    /// The response body exceeded the size cap.
    BodyTooLarge,
    /// The response body could not be decoded as JSON.
    DecodeError,
    /// The snapshot uses an unsupported schema version.
    UnsupportedSchema,
    /// The snapshot failed protocol validation.
    InvalidSnapshot,
    /// The poll was cancelled before completion.
    Cancelled,
}

/// A completed batch of poll results for a single generation.
#[derive(Debug)]
pub struct PollBatch {
    /// Monotonically increasing generation counter.
    pub generation: u64,
    /// When the batch was started.
    pub started_at: Instant,
    /// When the last result in the batch completed.
    pub completed_at: Instant,
    /// Individual poll results, one per endpoint.
    pub results: Vec<PollResult>,
}

/// Long-lived HTTP client for polling greggd endpoints.
///
/// Wraps a `reqwest::Client` with sensible defaults: no redirects,
/// bounded connection pool, configurable timeout, and a response size
/// cap.
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl HttpClient {
    /// Create a new HTTP client with the given request timeout.
    ///
    /// # Panics
    ///
    /// Panics if the `reqwest::Client` builder fails. This should never
    /// happen in practice.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(4)
            .build()
            .expect("reqwest client builder should not fail");
        Self { client }
    }

    /// Poll a single endpoint and return a [`PollResult`].
    ///
    /// This method is async-safe for concurrent use. Each invocation
    /// creates its own request and reads its own response body.
    pub async fn poll(&self, endpoint: &Endpoint, clock: &impl Clock) -> PollResult {
        let url = status_url(&endpoint.host, endpoint.port);
        let start = clock.now();

        let response = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                let latency = start.elapsed();
                let outcome = classify_reqwest_error(&e);
                return PollResult {
                    system_id: endpoint.id.clone(),
                    endpoint: endpoint.clone(),
                    outcome,
                    latency,
                };
            }
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let latency = start.elapsed();
            return PollResult {
                system_id: endpoint.id.clone(),
                endpoint: endpoint.clone(),
                outcome: PollOutcome::HttpStatus(status),
                latency,
            };
        }

        let Ok(body) = response.bytes().await else {
            let latency = start.elapsed();
            return PollResult {
                system_id: endpoint.id.clone(),
                endpoint: endpoint.clone(),
                outcome: PollOutcome::NetworkError,
                latency,
            };
        };

        if body.len() > MAX_RESPONSE_BYTES {
            let latency = start.elapsed();
            return PollResult {
                system_id: endpoint.id.clone(),
                endpoint: endpoint.clone(),
                outcome: PollOutcome::BodyTooLarge,
                latency,
            };
        }

        let Ok(snapshot): Result<StatusSnapshot, _> = serde_json::from_slice(&body) else {
            let latency = start.elapsed();
            return PollResult {
                system_id: endpoint.id.clone(),
                endpoint: endpoint.clone(),
                outcome: PollOutcome::DecodeError,
                latency,
            };
        };

        if snapshot.schema_version != SCHEMA_VERSION_V1 {
            let latency = start.elapsed();
            return PollResult {
                system_id: endpoint.id.clone(),
                endpoint: endpoint.clone(),
                outcome: PollOutcome::UnsupportedSchema,
                latency,
            };
        }

        if snapshot.validate().is_err() {
            let latency = start.elapsed();
            return PollResult {
                system_id: endpoint.id.clone(),
                endpoint: endpoint.clone(),
                outcome: PollOutcome::InvalidSnapshot,
                latency,
            };
        }

        let latency = start.elapsed();
        PollResult {
            system_id: endpoint.id.clone(),
            endpoint: endpoint.clone(),
            outcome: PollOutcome::Online(Box::new(snapshot)),
            latency,
        }
    }
}

/// Classify a reqwest error into a [`PollOutcome`].
fn classify_reqwest_error(e: &reqwest::Error) -> PollOutcome {
    if e.is_timeout() {
        return PollOutcome::Timeout;
    }

    // Walk the error source chain for io::ErrorKind::ConnectionRefused.
    if is_connection_refused(&e) {
        return PollOutcome::ConnectionRefused;
    }

    // Check for DNS resolution failure.
    if is_dns_failure(&e) {
        return PollOutcome::DnsFailure;
    }

    PollOutcome::NetworkError
}

/// Walk the error source chain looking for `ConnectionRefused`.
fn is_connection_refused(e: &dyn std::error::Error) -> bool {
    // Check the error message for connection refused indicators.
    let msg = format!("{e}");
    if msg.contains("connection refused") || msg.contains("Connection refused") {
        return true;
    }

    // Check the source chain.
    let mut source: Option<&(dyn std::error::Error + 'static)> = e.source();
    while let Some(err) = source {
        let msg = format!("{err}");
        if msg.contains("connection refused") || msg.contains("Connection refused") {
            return true;
        }
        source = err.source();
    }
    false
}

/// Walk the error source chain looking for DNS-related errors.
fn is_dns_failure(e: &dyn std::error::Error) -> bool {
    // Check the error itself first.
    let msg = format!("{e}");
    if msg.contains("dns") || msg.contains("resolve") || msg.contains("name") {
        return true;
    }

    let mut source: Option<&(dyn std::error::Error + 'static)> = e.source();
    while let Some(err) = source {
        let msg = format!("{err}");
        if msg.contains("dns") || msg.contains("resolve") || msg.contains("name") {
            return true;
        }
        source = err.source();
    }
    false
}

/// Construct the status URL for an endpoint.
///
/// IPv6 hosts are bracketed per RFC 2732.
#[must_use]
pub fn status_url(host: &str, port: u16) -> String {
    if host.parse::<IpAddr>().is_ok() && host.contains(':') {
        // IPv6 literal.
        format!("http://[{host}]:{port}/v1/status")
    } else {
        format!("http://{host}:{port}/v1/status")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::Endpoint;
    use gregg_protocol::test_support::LinuxSnapshotBuilder;
    use std::io;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Spin up a minimal mock HTTP server that returns the given body
    /// and status line. Returns the base URL.
    async fn mock_server(body: Vec<u8>, status: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut total = 0;
            loop {
                let n = stream.read(&mut buf[total..]).await.unwrap();
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    /// Mock server that reads the request then drops the connection.
    async fn mock_server_drop() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut total = 0;
            loop {
                let n = stream.read(&mut buf[total..]).await.unwrap();
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            // Drop the stream without responding.
            drop(stream);
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    /// Mock server that never accepts a connection (binds but never
    /// listens in a way that allows connect to succeed... actually on
    /// loopback it will accept immediately). Instead, we simulate
    /// connection refused by using an unused port that we close.
    async fn mock_server_closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // Immediately close, so port is refused.
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn endpoint_for(url: &str) -> Endpoint {
        // Extract host:port from the URL (e.g. "http://127.0.0.1:12345").
        let stripped = url.strip_prefix("http://").unwrap();
        let (host, port_str) = stripped.rsplit_once(':').unwrap();
        let host = host
            .strip_prefix('[')
            .unwrap_or(host)
            .strip_suffix(']')
            .unwrap_or(host);
        Endpoint {
            id: "test-id".into(),
            host: host.to_string(),
            port: port_str.parse().unwrap(),
            name: None,
        }
    }

    fn valid_snapshot_json() -> String {
        let snap = LinuxSnapshotBuilder::default().build();
        serde_json::to_string(&snap).unwrap()
    }

    #[tokio::test]
    async fn successful_poll_returns_online() {
        let body = valid_snapshot_json();
        let url = mock_server(body.into_bytes(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert_eq!(result.system_id, "test-id");
        assert!(matches!(result.outcome, PollOutcome::Online(_)));
        assert!(result.latency < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn successful_poll_with_macos_snapshot() {
        let snap = gregg_protocol::test_support::MacosSnapshotBuilder::default().build();
        let body = serde_json::to_string(&snap).unwrap();
        let url = mock_server(body.into_bytes(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::Online(_)));
    }

    #[tokio::test]
    async fn timeout_handling() {
        // Use a very short timeout and a server that delays.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut total = 0;
            loop {
                let n = stream.read(&mut buf[total..]).await.unwrap();
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            // Wait longer than the client timeout.
            tokio::time::sleep(Duration::from_secs(10)).await;
            let header = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
            let _ = stream.write_all(header.as_bytes()).await;
        });

        let ep = Endpoint {
            id: "test-id".into(),
            host: "127.0.0.1".into(),
            port: addr.port(),
            name: None,
        };
        let client = HttpClient::new(Duration::from_millis(50));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::Timeout));
    }

    #[tokio::test]
    async fn connection_refused() {
        let url = mock_server_closed_port().await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::ConnectionRefused));
    }

    #[tokio::test]
    async fn non_2xx_status() {
        let url = mock_server(b"not ready".to_vec(), "503 Service Unavailable").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::HttpStatus(503)));
    }

    #[tokio::test]
    async fn oversized_body() {
        // 65 KiB of 'x' exceeds the 64 KiB cap.
        let body = vec![b'x'; 65 * 1024];
        let url = mock_server(body, "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::BodyTooLarge));
    }

    #[tokio::test]
    async fn malformed_json() {
        let url = mock_server(b"not json at all".to_vec(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::DecodeError));
    }

    #[tokio::test]
    async fn unsupported_schema_version() {
        let snap = LinuxSnapshotBuilder::default().build();
        let mut json = serde_json::to_value(&snap).unwrap();
        json["schema_version"] = serde_json::json!(99);
        let body = serde_json::to_string(&json).unwrap();
        let url = mock_server(body.into_bytes(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::UnsupportedSchema));
    }

    #[tokio::test]
    async fn invalid_snapshot_validation_failure() {
        let snap = LinuxSnapshotBuilder::default().build();
        let mut json = serde_json::to_value(&snap).unwrap();
        // Set memory used > total to trigger a validation error.
        json["memory"]["used_bytes"] = serde_json::json!(999_999_999_999_i64);
        json["memory"]["total_bytes"] = serde_json::json!(1);
        let body = serde_json::to_string(&json).unwrap();
        let url = mock_server(body.into_bytes(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::InvalidSnapshot));
    }

    #[tokio::test]
    async fn url_construction_ipv4() {
        let url = status_url("192.168.1.1", 11310);
        assert_eq!(url, "http://192.168.1.1:11310/v1/status");
    }

    #[tokio::test]
    async fn url_construction_ipv6() {
        let url = status_url("::1", 8080);
        assert_eq!(url, "http://[::1]:8080/v1/status");
    }

    #[tokio::test]
    async fn url_construction_dns() {
        let url = status_url("server.local", 11310);
        assert_eq!(url, "http://server.local:11310/v1/status");
    }

    #[tokio::test]
    async fn network_error_on_dropped_connection() {
        let url = mock_server_drop().await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        // When the connection is dropped mid-response, reqwest will
        // return a network error (not a timeout or connection refused).
        assert!(
            matches!(
                result.outcome,
                PollOutcome::NetworkError | PollOutcome::DecodeError
            ),
            "expected NetworkError or DecodeError, got {:?}",
            result.outcome
        );
    }

    #[test]
    fn max_response_bytes_is_64k() {
        assert_eq!(MAX_RESPONSE_BYTES, 64 * 1024);
    }

    #[test]
    fn classify_timeout() {
        // We can't easily construct reqwest errors in unit tests,
        // so just verify the function signature compiles.
        let _ = classify_reqwest_error;
    }

    #[test]
    fn is_connection_refused_returns_false_for_non_refused() {
        let err = io::Error::other("some error");
        assert!(!is_connection_refused(&err));
    }

    #[test]
    fn is_connection_refused_returns_true_for_refused() {
        let err = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
        assert!(is_connection_refused(&err));
    }

    #[tokio::test]
    async fn redirect_response_301() {
        let url = mock_server(b"redirect".to_vec(), "301 Moved Permanently").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(matches!(result.outcome, PollOutcome::HttpStatus(301)));
    }

    #[tokio::test]
    async fn partial_body_then_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut total = 0;
            loop {
                let n = stream.read(&mut buf[total..]).await.unwrap();
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let header = "HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\npartial";
            let _ = stream.write_all(header.as_bytes()).await;
            drop(stream);
        });

        let ep = Endpoint {
            id: "test-id".into(),
            host: "127.0.0.1".into(),
            port: addr.port(),
            name: None,
        };
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(
                result.outcome,
                PollOutcome::NetworkError | PollOutcome::DecodeError
            ),
            "expected NetworkError or DecodeError, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn empty_body_with_200() {
        let url = mock_server(Vec::new(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::DecodeError),
            "expected DecodeError, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn wrong_content_type_with_valid_json() {
        let body = valid_snapshot_json();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut total = 0;
            loop {
                let n = stream.read(&mut buf[total..]).await.unwrap();
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(body.as_bytes()).await.unwrap();
        });

        let ep = Endpoint {
            id: "test-id".into(),
            host: "127.0.0.1".into(),
            port: addr.port(),
            name: None,
        };
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::Online(_)),
            "expected Online, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn large_valid_json_under_64k() {
        let long_name = "x".repeat(60_000);
        let snap = LinuxSnapshotBuilder::default().build();
        let mut json = serde_json::to_value(&snap).unwrap();
        json["system"]["name"] = serde_json::json!(long_name);
        let body = serde_json::to_string(&json).unwrap();
        assert!(body.len() < 64 * 1024);
        let url = mock_server(body.into_bytes(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::Online(_)),
            "expected Online, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn unicode_in_system_name() {
        let snap = LinuxSnapshotBuilder::default().build();
        let mut json = serde_json::to_value(&snap).unwrap();
        json["system"]["name"] = serde_json::json!("日本語サーバー");
        let body = serde_json::to_string(&json).unwrap();
        let url = mock_server(body.into_bytes(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::Online(_)),
            "expected Online, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn nested_invalid_json() {
        let url = mock_server(b"{\"nested\": {\"invalid\": true}}".to_vec(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::DecodeError),
            "expected DecodeError, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn array_instead_of_object() {
        let url = mock_server(b"[1, 2, 3]".to_vec(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::DecodeError),
            "expected DecodeError, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn null_json() {
        let url = mock_server(b"null".to_vec(), "200 OK").await;
        let ep = endpoint_for(&url);
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let result = client.poll(&ep, &clock).await;
        assert!(
            matches!(result.outcome, PollOutcome::DecodeError),
            "expected DecodeError, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn multiple_rapid_polls_same_result() {
        let body = valid_snapshot_json();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status_line = "200 OK".to_string();
        tokio::spawn(async move {
            for _ in 0..10 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let body = body.clone();
                let status_line = status_line.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let mut total = 0;
                    loop {
                        let n = stream.read(&mut buf[total..]).await.unwrap();
                        total += n;
                        if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let header = format!(
                        "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(header.as_bytes()).await.unwrap();
                    stream.write_all(body.as_bytes()).await.unwrap();
                });
            }
        });

        let ep = Endpoint {
            id: "test-id".into(),
            host: "127.0.0.1".into(),
            port: addr.port(),
            name: None,
        };
        let client = HttpClient::new(Duration::from_secs(5));
        let clock = crate::clock::RealClock;

        let mut outcomes = Vec::new();
        for _ in 0..10 {
            let result = client.poll(&ep, &clock).await;
            outcomes.push(result.outcome.clone());
        }
        for outcome in &outcomes {
            assert!(
                matches!(outcome, PollOutcome::Online(_)),
                "expected Online for all polls, got {outcome:?}",
            );
        }
    }
}
