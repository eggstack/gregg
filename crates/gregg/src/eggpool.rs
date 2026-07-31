//! Bounded `EggPool` summary client and pane refresh worker.

#![allow(dead_code)]

use std::env;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Deserialize;
use url::Url;

use crate::config::EggpoolEntry;

const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// The four fixed rolling windows supported by `EggPool`'s summary API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EggpoolPeriod {
    /// The most recent hour.
    Hour,
    /// The most recent day.
    Day,
    /// The most recent week.
    Week,
    /// The most recent month.
    Month,
}

impl EggpoolPeriod {
    /// Return the exact API query value.
    #[must_use]
    pub const fn api_value(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    /// Return the human-readable period label.
    #[must_use]
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::Hour => "1 hour",
            Self::Day => "1 day",
            Self::Week => "7 days",
            Self::Month => "30 days",
        }
    }

    /// Move to the next longer period, clamping at one month.
    #[must_use]
    pub const fn longer(self) -> Self {
        match self {
            Self::Hour => Self::Day,
            Self::Day => Self::Week,
            Self::Week | Self::Month => Self::Month,
        }
    }

    /// Move to the next shorter period, clamping at one hour.
    #[must_use]
    pub const fn shorter(self) -> Self {
        match self {
            Self::Hour | Self::Day => Self::Hour,
            Self::Week => Self::Day,
            Self::Month => Self::Week,
        }
    }
}

#[derive(Debug, Deserialize)]
struct EggpoolSummaryWire {
    period: String,
    accounted_tokens: u64,
    cache_read_ratio: Option<f64>,
    tokens_per_second: f64,
    avg_ttft_ms: f64,
    streamed_requests: u64,
}

/// Validated, display-ready summary values.
#[derive(Debug, Clone, PartialEq)]
pub struct EggpoolSummary {
    /// Tokens accounted for by `EggPool`'s summary semantics.
    pub accounted_tokens: u64,
    /// Provider cache-read share, when `EggPool` can calculate it.
    pub cache_read_ratio: Option<f64>,
    /// Output tokens per second.
    pub output_tokens_per_second: f64,
    /// Average time to first token, unavailable when there were no streams.
    pub avg_ttft_ms: Option<f64>,
    /// The period represented by this summary.
    pub period: EggpoolPeriod,
}

/// A safe, stable classification of one `EggPool` fetch attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum EggpoolFetchOutcome {
    /// A validated summary was received.
    Online(EggpoolSummary),
    /// The configured environment variable was absent or empty.
    MissingApiKeyEnv { name: String },
    /// `EggPool` rejected the API key.
    Unauthorized,
    /// The API key lacks permission.
    Forbidden,
    /// The statistics routes are disabled or unavailable.
    StatsUnavailable,
    /// The request exceeded its timeout.
    Timeout,
    /// The host refused the connection.
    ConnectionRefused,
    /// DNS resolution failed.
    DnsFailure,
    /// Another network error occurred.
    NetworkError,
    /// `EggPool` returned another HTTP status.
    HttpStatus(u16),
    /// The response exceeded the bounded body limit.
    BodyTooLarge,
    /// The response was not valid JSON of the expected shape.
    DecodeError,
    /// The JSON decoded but failed semantic validation.
    InvalidSummary,
    /// The request was superseded or the worker was shut down.
    Cancelled,
}

/// One completed or superseded worker request.
#[derive(Debug)]
pub struct EggpoolResult {
    /// Worker generation for stale-result rejection.
    pub generation: u64,
    /// Period requested by this attempt.
    pub period: EggpoolPeriod,
    /// Request start time.
    pub started_at: Instant,
    /// Request completion time.
    pub completed_at: Instant,
    /// Stable request outcome.
    pub outcome: EggpoolFetchOutcome,
}

type EnvLookup = Arc<dyn Fn(&str) -> Option<OsString> + Send + Sync>;

/// Long-lived, bounded client for `EggPool`'s summary endpoint.
#[derive(Clone)]
pub struct EggpoolClient {
    client: reqwest::Client,
    env_lookup: EnvLookup,
}

impl EggpoolClient {
    /// Build a client with redirects disabled and a bounded idle pool.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self::with_env_lookup(timeout, Arc::new(|name| env::var_os(name)))
    }

    fn with_env_lookup(timeout: Duration, env_lookup: EnvLookup) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(2)
            .build()
            .expect("reqwest client builder should not fail");
        Self { client, env_lookup }
    }

    /// Fetch one validated summary. No automatic retry or alternate endpoint
    /// is attempted.
    pub async fn fetch(
        &self,
        endpoint: &EggpoolEntry,
        period: EggpoolPeriod,
    ) -> EggpoolFetchOutcome {
        let auth = match endpoint.api_key_env.as_deref() {
            None => None,
            Some(name) => match (self.env_lookup)(name) {
                Some(value) if !value.is_empty() => match value.into_string() {
                    Ok(value) => Some(value),
                    Err(_) => return missing_key(name),
                },
                _ => return missing_key(name),
            },
        };

        let Ok(url) = summary_url(endpoint, period) else {
            return EggpoolFetchOutcome::NetworkError;
        };
        let mut request = self.client.get(url);
        if let Some(value) = auth {
            let Ok(mut header) = reqwest::header::HeaderValue::from_str(&format!("Bearer {value}"))
            else {
                return EggpoolFetchOutcome::MissingApiKeyEnv {
                    name: endpoint.api_key_env.clone().unwrap_or_default(),
                };
            };
            header.set_sensitive(true);
            request = request.header(reqwest::header::AUTHORIZATION, header);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return classify_request_error(&error),
        };
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return match status {
                401 => EggpoolFetchOutcome::Unauthorized,
                403 => EggpoolFetchOutcome::Forbidden,
                404 => EggpoolFetchOutcome::StatsUnavailable,
                status => EggpoolFetchOutcome::HttpStatus(status),
            };
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return EggpoolFetchOutcome::BodyTooLarge;
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                return EggpoolFetchOutcome::NetworkError;
            };
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return EggpoolFetchOutcome::BodyTooLarge;
            }
            body.extend_from_slice(&chunk);
        }
        let Ok(wire) = serde_json::from_slice::<EggpoolSummaryWire>(&body) else {
            return EggpoolFetchOutcome::DecodeError;
        };
        normalize_summary(&wire, period).map_or(
            EggpoolFetchOutcome::InvalidSummary,
            EggpoolFetchOutcome::Online,
        )
    }
}

fn missing_key(name: &str) -> EggpoolFetchOutcome {
    EggpoolFetchOutcome::MissingApiKeyEnv {
        name: name.to_string(),
    }
}

fn summary_url(endpoint: &EggpoolEntry, period: EggpoolPeriod) -> Result<Url, ()> {
    let host = if endpoint.host.contains(':') {
        format!("[{}]", endpoint.host)
    } else {
        endpoint.host.clone()
    };
    let mut url = Url::parse(&format!(
        "{}://{}:{}/api/stats/summary",
        endpoint.scheme, host, endpoint.port
    ))
    .map_err(|_| ())?;
    url.query_pairs_mut()
        .append_pair("period", period.api_value());
    Ok(url)
}

fn normalize_summary(
    wire: &EggpoolSummaryWire,
    requested: EggpoolPeriod,
) -> Result<EggpoolSummary, ()> {
    if wire.period != requested.api_value()
        || !wire.tokens_per_second.is_finite()
        || wire.tokens_per_second < 0.0
        || !wire.avg_ttft_ms.is_finite()
        || wire.avg_ttft_ms < 0.0
        || wire
            .cache_read_ratio
            .is_some_and(|ratio| !ratio.is_finite() || !(0.0..=1.0).contains(&ratio))
    {
        return Err(());
    }
    Ok(EggpoolSummary {
        period: requested,
        accounted_tokens: wire.accounted_tokens,
        cache_read_ratio: wire.cache_read_ratio,
        output_tokens_per_second: wire.tokens_per_second,
        avg_ttft_ms: (wire.streamed_requests > 0).then_some(wire.avg_ttft_ms),
    })
}

fn classify_request_error(error: &reqwest::Error) -> EggpoolFetchOutcome {
    if error.is_timeout() {
        return EggpoolFetchOutcome::Timeout;
    }
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = current {
        let message = error.to_string().to_ascii_lowercase();
        if message.contains("connection refused") {
            return EggpoolFetchOutcome::ConnectionRefused;
        }
        if message.contains("dns") || message.contains("resolve") || message.contains("name") {
            return EggpoolFetchOutcome::DnsFailure;
        }
        current = error.source();
    }
    EggpoolFetchOutcome::NetworkError
}

/// Commands accepted by the single optional `EggPool` worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EggpoolCommand {
    /// Activate the pane and fetch immediately.
    Activate { period: EggpoolPeriod },
    /// Deactivate periodic refreshes.
    Deactivate,
    /// Change period and fetch immediately.
    SetPeriod(EggpoolPeriod),
    /// Fetch immediately for the current/requested period.
    Refresh(EggpoolPeriod),
    /// Stop the worker promptly.
    Shutdown,
}

/// Handle for the optional worker's command and result channels.
pub struct EggpoolWorker {
    /// Send commands to the worker.
    pub commands: tokio::sync::mpsc::Sender<EggpoolCommand>,
    /// Receive completed results.
    pub results: tokio::sync::mpsc::Receiver<EggpoolResult>,
}

/// Start one worker for one configured `EggPool` endpoint.
pub fn spawn_worker(client: EggpoolClient, endpoint: EggpoolEntry) -> EggpoolWorker {
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(4);
    let (result_tx, result_rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        let mut active = false;
        let mut period = EggpoolPeriod::Hour;
        let mut generation: u64 = 0;
        let mut request: Option<
            tokio::task::JoinHandle<(u64, EggpoolPeriod, Instant, EggpoolFetchOutcome)>,
        > = None;
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + REFRESH_INTERVAL,
            REFRESH_INTERVAL,
        );
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                command = command_rx.recv() => match command {
                    Some(EggpoolCommand::Activate { period: requested }) => {
                        if let Some(old_request) = request.take() {
                            old_request.abort();
                        }
                        active = true;
                        period = requested;
                        generation = generation.wrapping_add(1);
                        request = Some(start_request(&client, &endpoint, period, generation));
                    }
                    Some(EggpoolCommand::Deactivate) => active = false,
                    Some(EggpoolCommand::SetPeriod(requested) | EggpoolCommand::Refresh(requested)) => {
                        period = requested;
                        if active {
                            if let Some(old_request) = request.take() {
                                old_request.abort();
                            }
                            generation = generation.wrapping_add(1);
                            request = Some(start_request(&client, &endpoint, period, generation));
                        }
                    }
                    Some(EggpoolCommand::Shutdown) | None => {
                        if let Some(request) = request { request.abort(); }
                        break;
                    }
                },
                _ = ticker.tick(), if active && request.is_none() => {
                    generation = generation.wrapping_add(1);
                    request = Some(start_request(&client, &endpoint, period, generation));
                }
                completed = async { request.as_mut()?.await.ok() }, if request.is_some() => {
                    request = None;
                    if let Some((generation, period, started_at, outcome)) = completed {
                        let _ = result_tx.send(EggpoolResult { generation, period, started_at, completed_at: Instant::now(), outcome }).await;
                    }
                }
            }
        }
    });
    EggpoolWorker {
        commands: command_tx,
        results: result_rx,
    }
}

fn start_request(
    client: &EggpoolClient,
    endpoint: &EggpoolEntry,
    period: EggpoolPeriod,
    generation: u64,
) -> tokio::task::JoinHandle<(u64, EggpoolPeriod, Instant, EggpoolFetchOutcome)> {
    let client = client.clone();
    let endpoint = endpoint.clone();
    tokio::spawn(async move {
        let started_at = Instant::now();
        let outcome = client.fetch(&endpoint, period).await;
        (generation, period, started_at, outcome)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EggpoolScheme;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn endpoint(port: u16, api_key_env: Option<&str>) -> EggpoolEntry {
        EggpoolEntry {
            id: "id".into(),
            host: "127.0.0.1".into(),
            port,
            scheme: EggpoolScheme::Http,
            name: None,
            api_key_env: api_key_env.map(str::to_string),
        }
    }

    fn body(period: &str) -> String {
        format!(
            r#"{{"period":"{period}","accounted_tokens":42,"cache_read_ratio":null,"tokens_per_second":1.5,"avg_ttft_ms":12.0,"streamed_requests":0}}"#
        )
    }

    async fn server(response: String) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let mut used = 0;
            loop {
                let count = stream.read(&mut request[used..]).await.unwrap();
                if count == 0 {
                    break;
                }
                used += count;
                if request[..used]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    break;
                }
            }
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request[..used]).into_owned()
        });
        (port, task)
    }

    #[test]
    fn periods_are_exhaustive_and_clamped() {
        let all = [
            EggpoolPeriod::Hour,
            EggpoolPeriod::Day,
            EggpoolPeriod::Week,
            EggpoolPeriod::Month,
        ];
        assert_eq!(
            all.map(EggpoolPeriod::api_value),
            ["1h", "24h", "7d", "30d"]
        );
        assert_eq!(
            all.map(EggpoolPeriod::display_label),
            ["1 hour", "1 day", "7 days", "30 days"]
        );
        assert_eq!(EggpoolPeriod::Hour.shorter(), EggpoolPeriod::Hour);
        assert_eq!(EggpoolPeriod::Month.longer(), EggpoolPeriod::Month);
        assert_eq!(EggpoolPeriod::Hour.longer().shorter(), EggpoolPeriod::Hour);
    }

    #[tokio::test]
    async fn public_request_uses_fixed_path_and_no_auth() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body("1h").len(),
            body("1h")
        );
        let (port, task) = server(response).await;
        let result = EggpoolClient::new(Duration::from_secs(2))
            .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
            .await;
        assert!(
            matches!(result, EggpoolFetchOutcome::Online(summary) if summary.cache_read_ratio.is_none() && summary.avg_ttft_ms.is_none())
        );
        let request = task.await.unwrap();
        assert!(request.starts_with("GET /api/stats/summary?period=1h HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
    }

    #[tokio::test]
    async fn protected_request_sends_injected_bearer_without_retaining_secret() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body("24h").len(),
            body("24h")
        );
        let (port, task) = server(response).await;
        let client = EggpoolClient::with_env_lookup(
            Duration::from_secs(2),
            Arc::new(|_| Some(OsString::from("secret-value"))),
        );
        let result = client
            .fetch(&endpoint(port, Some("KEY")), EggpoolPeriod::Day)
            .await;
        assert!(matches!(result, EggpoolFetchOutcome::Online(_)));
        assert!(!format!("{result:?}").contains("secret-value"));
        let request = task.await.unwrap();
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer secret-value"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn missing_or_empty_key_does_not_send_request() {
        let client = EggpoolClient::with_env_lookup(Duration::from_secs(2), Arc::new(|_| None));
        let result = client
            .fetch(&endpoint(1, Some("KEY")), EggpoolPeriod::Hour)
            .await;
        assert_eq!(
            result,
            EggpoolFetchOutcome::MissingApiKeyEnv { name: "KEY".into() }
        );
        let client = EggpoolClient::with_env_lookup(
            Duration::from_secs(2),
            Arc::new(|_| Some(OsString::new())),
        );
        assert_eq!(
            client
                .fetch(&endpoint(1, Some("KEY")), EggpoolPeriod::Hour)
                .await,
            EggpoolFetchOutcome::MissingApiKeyEnv { name: "KEY".into() }
        );
    }

    #[tokio::test]
    async fn statuses_decode_semantics_and_body_limit_are_stable() {
        for (status, expected) in [
            ("401 Unauthorized", EggpoolFetchOutcome::Unauthorized),
            ("403 Forbidden", EggpoolFetchOutcome::Forbidden),
            ("404 Not Found", EggpoolFetchOutcome::StatsUnavailable),
            ("500 Error", EggpoolFetchOutcome::HttpStatus(500)),
        ] {
            let response = format!("HTTP/1.1 {status}\r\nContent-Length: 3\r\n\r\nno!");
            let (port, _) = server(response).await;
            assert_eq!(
                EggpoolClient::new(Duration::from_secs(2))
                    .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
                    .await,
                expected
            );
        }
        let response = "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nno!".to_string();
        let (port, _) = server(response).await;
        assert_eq!(
            EggpoolClient::new(Duration::from_secs(2))
                .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
                .await,
            EggpoolFetchOutcome::DecodeError
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            MAX_RESPONSE_BYTES + 1,
            "x".repeat(MAX_RESPONSE_BYTES + 1)
        );
        let (port, _) = server(response).await;
        assert_eq!(
            EggpoolClient::new(Duration::from_secs(2))
                .fetch(&endpoint(port, None), EggpoolPeriod::Hour)
                .await,
            EggpoolFetchOutcome::BodyTooLarge
        );
    }

    #[tokio::test]
    async fn worker_fetches_activation_and_suppresses_inactive_ticks() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body("1h").len(),
            body("1h")
        );
        let (port, _) = server(response).await;
        let mut worker = spawn_worker(
            EggpoolClient::new(Duration::from_secs(2)),
            endpoint(port, None),
        );
        worker
            .commands
            .send(EggpoolCommand::Deactivate)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), worker.results.recv())
                .await
                .is_err()
        );
    }

    #[test]
    fn invalid_summary_is_rejected() {
        let wire = EggpoolSummaryWire {
            period: "1d".into(),
            accounted_tokens: 1,
            cache_read_ratio: Some(2.0),
            tokens_per_second: 1.0,
            avg_ttft_ms: 1.0,
            streamed_requests: 1,
        };
        assert!(normalize_summary(&wire, EggpoolPeriod::Hour).is_err());
    }
}
